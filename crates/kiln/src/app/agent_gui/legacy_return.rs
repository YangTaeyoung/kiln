//! Cooperative adoption of a tracked CLI that predates the persistent GUI host.
use super::*;
use kiln_common::{paths::Canonical, task::Task};
use kiln_proto::{ClientMsg, ServerMsg, SessionId};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    process::{Command as Process, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CHORD: &str = "ctrl+x ctrl+f";
const RELOAD: &str = "command:reload-plugins";
const KEYS: &[u8] = b"\x18\x06";

pub struct NativeImport {
    pub session: SessionId,
    pub path: PathBuf,
    pub task: Option<Task<std::result::Result<SpawnSpec, String>>>,
    bridge: LegacyReturn,
}
impl NativeImport {
    pub fn new(
        ctx: &egui::Context,
        session: SessionId,
        pid: u32,
        path: PathBuf,
        socket: String,
    ) -> Self {
        Self {
            session,
            path: path.clone(),
            task: None,
            bridge: LegacyReturn::start(ctx, Source::Native(path), session, pid, socket),
        }
    }
    pub fn request_cancel(&self) {
        self.bridge.request_cancel();
    }
    pub fn cancel_requested(&self) -> bool {
        self.bridge.cancel_requested()
    }
    pub fn cancelled(&mut self) -> bool {
        self.bridge.cancelled()
    }
    pub fn bridge_pending(&mut self) -> bool {
        self.bridge.task.is_pending()
    }
    /// Called only after the daemon reports actual exit; disk reads stay off the UI thread.
    pub fn prepare_task(
        &mut self,
        ctx: &egui::Context,
    ) -> Task<std::result::Result<SpawnSpec, String>> {
        let path = self.path.clone();

        Task::spawn(ctx, move || {
            super::prepare_existing_native(&path).map_err(|error| error.to_string())
        })
    }
    pub fn label(&self) -> &'static str {
        self.bridge.label()
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.bridge.take_error()
    }
}
pub(super) struct LegacyReturn {
    cancel: Arc<AtomicBool>,
    requested: Arc<AtomicBool>,
    armed: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    phase: Arc<Mutex<String>>,
    task: Task<std::result::Result<(), String>>,
    error: Option<String>,
}
pub(super) enum Source {
    Native(PathBuf),
    Gui {
        worker: String,
        conversation: String,
    },
}
impl LegacyReturn {
    pub(super) fn start(
        ctx: &egui::Context,
        source: Source,
        session: SessionId,
        pid: u32,
        socket: String,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let requested = Arc::new(AtomicBool::new(false));
        let cancelling = requested.clone();
        let armed = Arc::new(AtomicBool::new(false));
        let active = armed.clone();
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let waiter = wake.clone();
        let phase = Arc::new(Mutex::new("starting".into()));
        let stopped = cancel.clone();
        let progress = phase.clone();
        let repaint = ctx.clone();
        let task = Task::spawn(ctx, move || {
            let result = run(
                &source,
                session,
                pid,
                &socket,
                &stopped,
                &cancelling,
                &active,
                &waiter,
                |value| {
                    if let Ok(mut current) = progress.lock() {
                        *current = value.into();
                    }
                    repaint.request_repaint();
                },
            );
            if result.is_err() {
                if let Ok(mut current) = progress.lock() {
                    *current = "unavailable".into();
                }
            }
            result.map_err(|error| error.to_string())
        });
        Self {
            cancel,
            requested,
            armed,
            wake,
            phase,
            task,
            error: None,
        }
    }
    pub(super) fn take_error(&mut self) -> Option<String> {
        let error = self.task.take().and_then(|result| result.err());
        if let Some(error) = &error {
            self.error = Some(error.clone());
        }
        error
    }
    pub(super) fn request_cancel(&self) {
        let _guard = self.wake.0.lock().ok();
        self.requested.store(true, Ordering::Release);
        if let Ok(mut phase) = self.phase.lock() {
            if *phase == "manual"
                || (*phase == "unavailable" && !self.armed.load(Ordering::Acquire))
            {
                *phase = "cancelled".into();
            }
        }
        self.wake.1.notify_all();
    }
    pub(super) fn cancel_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
    pub(super) fn cancelled(&mut self) -> bool {
        !self.task.is_pending() && self.phase.lock().is_ok_and(|phase| *phase == "cancelled")
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn label(&self) -> &'static str {
        if self.cancel_requested() {
            return "전환 취소를 확인하고 있습니다. 현재 대화와 작업은 계속 유지됩니다.";
        }
        match self.phase.lock().ok().as_deref().map(String::as_str) {
            Some("starting") => {
                "기존 대화에 전환 연결을 준비하고 있습니다. TUI를 선택하면 취소합니다."
            }
            Some("working") => "현재 작업이 끝나면 GUI로 전환합니다. TUI를 선택하면 취소합니다.",
            Some("draft") => {
                "터미널에 작성 중인 입력이 있습니다. 입력을 보내거나 비우면 GUI로 전환합니다."
            }
            Some("background") => {
                "백그라운드 작업을 유지하고 있습니다. 작업이 끝나면 GUI로 전환합니다."
            }
            Some("manual") => {
                "대화로 돌아가려면 CLI에서 /exit 또는 Ctrl-D로 종료하세요. 현재 작업은 계속 실행됩니다."
            }
            Some("ready" | "closing") => "같은 대화를 GUI로 전환하고 있습니다.",
            _ => "자동 전환 연결을 확인하지 못했습니다. 현재 대화와 작업은 계속 유지됩니다.",
        }
    }
}
impl Drop for LegacyReturn {
    fn drop(&mut self) {
        let _guard = self.wake.0.lock().ok();
        self.cancel.store(true, Ordering::Release);
        self.wake.1.notify_all();
    }
}

/// A shared, temporary binding. Never replace an existing shortcut or text draft.
/// Other windows importing the same account share the lease until their reloads finish.
struct KeyLease {
    path: PathBuf,
    recovery: PathBuf,
    _lock: fs::File,
}
static LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<KeyLease>>>> = OnceLock::new();
impl KeyLease {
    fn acquire(profile: &Path) -> Result<Arc<Self>> {
        let path = profile.join("keybindings.json");
        let mut leases = LEASES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow::anyhow!("Native shortcut lease is unavailable"))?;
        if let Some(lease) = leases.get(&path).and_then(Weak::upgrade) {
            return Ok(lease);
        }
        let lock = journal::open(&profile.join("kiln-native-return.lock"), false)?;
        lock.try_lock()
            .context("Another Kiln window is preparing native switching")?;
        let recovery = profile.join("kiln-native-return.recovery.json");
        recover_binding(&path, &recovery)?;
        let original = read_optional(&path)?;
        let (installed, block) = binding_bytes(original.as_deref())?;
        use base64::Engine;
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        // Publish recovery before editing shared settings. A crash at any later
        // point is reconciled under this same OS lock on the next attempt.
        ensure!(
            installed.len() <= 1024 * 1024,
            "Native shortcut configuration is too large to modify safely"
        );
        journal::atomic(
            &recovery,
            &serde_json::to_vec(&json!({"protocol":1,
            "original":original.as_deref().map(encode), "installed":encode(&installed),"block":block}))?,
        )?;
        journal::atomic(&path, &installed)?;
        let lease = Arc::new(Self {
            path: path.clone(),
            recovery,
            _lock: lock,
        });
        leases.insert(path, Arc::downgrade(&lease));
        Ok(lease)
    }
}
impl Drop for KeyLease {
    fn drop(&mut self) {
        // Retain a failed recovery record; never lose the evidence required to
        // retry cleanup, and never overwrite an unsupported concurrent edit.
        let _ = recover_binding(&self.path, &self.recovery);
    }
}
fn recover_binding(path: &Path, recovery: &Path) -> Result<()> {
    if !recovery.try_exists()? {
        return Ok(());
    }
    let value: Value = journal::read(recovery)?;
    ensure!(
        value["protocol"] == 1,
        "Unknown native shortcut recovery record"
    );
    use base64::Engine;
    let decode = |text: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(anyhow::Error::from)
    };
    let original = value["original"].as_str().map(decode).transpose()?;
    let installed = decode(
        value["installed"]
            .as_str()
            .context("Invalid native shortcut recovery")?,
    )?;
    let (expected, block) = binding_bytes(original.as_deref())?;
    ensure!(
        installed == expected && value["block"] == block,
        "Native shortcut recovery ownership changed"
    );
    let current = read_optional(path)?;
    if current.as_deref() == Some(installed.as_slice()) {
        if let Some(original) = original {
            journal::atomic(path, &original)?;
        } else {
            fs::remove_file(path)?;
            journal::sync_directory(path.parent().context("Missing shortcut directory")?)?;
        }
    } else if current != original {
        if let Some(current) = current {
            let mut edited: Value = serde_json::from_slice(&current).context(
                "Native shortcut edits require manual reconciliation; recovery retained",
            )?;
            let bindings = edited
                .get_mut("bindings")
                .and_then(Value::as_array_mut)
                .context("Native shortcut edits have no binding list; recovery retained")?;
            if let Some(index) = bindings.iter().position(|entry| entry == &block) {
                bindings.remove(index);
                journal::atomic(path, &serde_json::to_vec_pretty(&edited)?)?;
            }
        }
    }
    fs::remove_file(recovery)?;
    journal::sync_directory(recovery.parent().context("Missing recovery directory")?)?;
    Ok(())
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 1024 * 1024,
                "Native shortcut configuration is not a regular bounded file"
            );
            Ok(Some(fs::read(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn binding_bytes(original: Option<&[u8]>) -> Result<(Vec<u8>, Value)> {
    let mut value: Value = match original {
        Some(bytes) => serde_json::from_slice(bytes)
            .context("Native shortcut configuration cannot be safely edited")?,
        None => json!({"bindings":[]}),
    };
    let bindings = value
        .get_mut("bindings")
        .and_then(Value::as_array_mut)
        .context("Native shortcut configuration has no binding list")?;
    for block in bindings.iter() {
        let entries = block
            .get("bindings")
            .and_then(Value::as_object)
            .context("Invalid native shortcut block")?;
        for (key, action) in entries {
            let key = key
                .to_ascii_lowercase()
                .split_whitespace()
                .map(|stroke| {
                    stroke
                        .split('+')
                        .map(|part| if part == "control" { "ctrl" } else { part })
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .collect::<Vec<_>>()
                .join(" ");
            ensure!(
                key != CHORD && !key.starts_with(&format!("{CHORD} ")) && key != "ctrl+x",
                "A user shortcut already owns the native switching chord"
            );
            if matches!(key.as_str(), "ctrl+f" | "control+f") {
                ensure!(
                    action.is_null(),
                    "A user shortcut owns the native cursor-forward fallback"
                );
            }
        }
    }
    let block = json!({"context":"Chat","bindings":{CHORD:RELOAD}});
    bindings.push(block.clone());
    Ok((serde_json::to_vec_pretty(&value)?, block))
}

struct Installation {
    command: Process,
    root: PathBuf,
    name: String,
    market: String,
    added: bool,
    installed: bool,
}
impl Installation {
    fn command(&self, args: &[&str], cancel: &AtomicBool) -> Result<()> {
        let mut command = Process::new(self.command.get_program());
        command.env_clear().envs(
            self.command
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        );
        if let Some(cwd) = self.command.get_current_dir() {
            command.current_dir(cwd);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        kiln_common::process::hide_window(&mut command);
        let mut child = command
            .spawn()
            .context("Could not prepare the native switching plugin")?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "Native switching plugin setup failed");
                return Ok(());
            }
            if cancel.load(Ordering::Acquire) || start.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Native switching plugin setup was cancelled or timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Installation {
    fn drop(&mut self) {
        let cancel = AtomicBool::new(false);
        let _ = fs::remove_file(self.root.join("probe/return.request"));
        if self.installed {
            let _ = self.command(
                &["plugin", "uninstall", &self.name, "--scope", "user"],
                &cancel,
            );
        }
        if self.added {
            let _ = self.command(&["plugin", "marketplace", "remove", &self.market], &cancel);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn run(
    source: &Source,
    session: SessionId,
    pid: u32,
    socket: &str,
    cancel: &AtomicBool,
    requested: &AtomicBool,
    armed: &AtomicBool,
    wake: &(Mutex<()>, Condvar),
    phase: impl Fn(&str),
) -> Result<()> {
    let record = match source {
        Source::Native(path) => super::super::agent_runtime::read(path)?,
        Source::Gui {
            worker,
            conversation,
        } => {
            let config: Config = journal::read(&journal::directory(worker)?.join("config.json"))?;
            ensure!(
                config.descriptor.id == *worker,
                "The original GUI worker identity changed"
            );
            super::super::agent_runtime::Record {
                tool: config.descriptor.tool,
                spec: SpawnSpec {
                    program: Some(config.program.to_string_lossy().into_owned()),
                    env: config.environment,
                    cwd: Some(config.descriptor.cwd.to_string_lossy().into_owned()),
                    ..Default::default()
                },
                version: String::new(),
                conversation: Some(conversation.clone()),
                generation: worker.clone(),
                confirmed: true,
                request_started: false,
            }
        }
    };
    if record.tool == Tool::Codex {
        return match source {
            Source::Gui { worker, conversation } => run_codex_gui_return(
                worker, conversation, &record, session, pid, socket, cancel, requested, armed, wake, phase,
            ),
            Source::Native(_) => { phase("manual"); Ok(()) }
        };
    }
    if record.tool != Tool::Claude {
        phase("manual");
        return Ok(());
    }
    ensure!(
        record.confirmed,
        "This terminal has no confirmed Claude conversation"
    );
    let id = record
        .conversation
        .as_deref()
        .context("Missing native conversation identity")?;
    ensure!(
        id.len() == 36
            && id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'),
        "Invalid native conversation identity"
    );
    let program = Path::new(
        record
            .spec
            .program
            .as_deref()
            .context("Missing native CLI executable")?,
    );
    let identity = match source {
        Source::Native(_) => {
            let (path, stamp) = host::legacy_cli_identity(pid, program, &record.version)?;
            (pid, path, stamp)
        }
        Source::Gui { worker, .. } => {
            host::legacy_gui_cli_identity(pid, session, program, worker, id, Tool::Claude)?
        }
    };
    let foreground_owner = match source {
        Source::Gui { worker, .. } => host::legacy_gui_owner(pid, session, worker)?,
        Source::Native(_) => pid,
    };
    if cfg!(windows) {
        phase("manual");
        return Ok(());
    }
    let client = kiln_daemon::client::Client::connect(socket, None)?;
    ensure!(
        client.server_build == kiln_daemon::exe_build_id(&super::super::conn::daemon_exe()),
        "Update the daemon before connecting an existing native terminal"
    );
    ensure!(
        verify_session(&client, session, pid)?,
        "The native terminal already exited"
    );
    let profile = record
        .spec
        .env
        .iter()
        .find(|(key, _)| key == "CLAUDE_CONFIG_DIR")
        .map(|(_, value)| PathBuf::from(value))
        .context("The native account namespace is unavailable")?;
    ensure!(
        profile.is_absolute() && fs::symlink_metadata(&profile)?.is_dir(),
        "The native account namespace is not a private directory"
    );
    let profile = profile.canonical()?;
    let token = journal::unique();
    let root = kiln_common::paths::config_dir()
        .join("native-imports")
        .join(&token);
    let plugin = root.join("probe");
    journal::private_dir(&plugin.join(".claude-plugin"))?;
    journal::private_dir(&plugin.join("hooks"))?;
    journal::private_dir(&root.join(".claude-plugin"))?;
    let market = format!("kiln-return-{}", token.rsplit('-').next().unwrap());
    let plugin_name = format!("kiln-native-return-{}", token.rsplit('-').next().unwrap());
    let name = format!("{plugin_name}@{market}");
    journal::atomic(
        &plugin.join(".claude-plugin/plugin.json"),
        &serde_json::to_vec(
            &json!({"name":plugin_name,"version":"1.0.0","description":"Return this existing Kiln conversation to its graphical view"}),
        )?,
    )?;
    journal::atomic(
        &plugin.join("hooks/hooks.json"),
        br#"{"modules":["./register.js"]}"#,
    )?;
    journal::atomic(
        &plugin.join("hooks/register.js"),
        format!(
            "// Kiln adoption {token}\n{}",
            include_str!("native_return.js")
        )
        .as_bytes(),
    )?;
    journal::atomic(
        &plugin.join("bridge.config"),
        &serde_json::to_vec(
            &json!({"protocol":1,"token":token,"session":id,"helper":super::super::conn::daemon_exe().canonicalize()?,"pid":identity.0,"stamp":identity.2}),
        )?,
    )?;
    journal::atomic(
        &root.join(".claude-plugin/marketplace.json"),
        &serde_json::to_vec(
            &json!({"name":market,"owner":{"name":"Kiln"},"plugins":[{"name":plugin_name,"source":"./probe"}]}),
        )?,
    )?;
    let mut command = Process::new(&identity.1);
    command.env_clear().envs(record.spec.env.iter().cloned());
    for key in [
        "HOME",
        "USERPROFILE",
        "SystemRoot",
        "COMSPEC",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if !record
            .spec
            .env
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    command
        .env("ANTHROPIC_API_KEY", "kiln-local-plugin-setup")
        .env("ANTHROPIC_AUTH_TOKEN", "")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9");
    for key in [
        "DISABLE_TELEMETRY",
        "DISABLE_ERROR_REPORTING",
        "DISABLE_AUTOUPDATER",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    ] {
        command.env(key, "1");
    }
    if let Some(cwd) = &record.spec.cwd {
        command.current_dir(cwd);
    }
    let mut installed = Installation {
        command,
        root,
        name,
        market,
        added: false,
        installed: false,
    };
    let lease = KeyLease::acquire(&profile)?;
    let directory = installed.root.to_string_lossy().into_owned();
    installed.command(&["plugin", "marketplace", "add", &directory], cancel)?;
    installed.added = true;
    installed.command(
        &["plugin", "install", &installed.name, "--scope", "user"],
        cancel,
    )?;
    installed.installed = true;
    let mut lease = Some(lease);
    let mut last_reload = Instant::now();
    let mut available = false;
    let activated_at = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if requested.load(Ordering::Acquire) && !available {
            // No exit lease has ever been published, so cleanup itself revokes
            // this unactivated attempt. Otherwise require native acknowledgment.
            phase("cancelled");
            return Ok(());
        }
        if let Source::Native(path) = source {
            let current = super::super::agent_runtime::read(path)?;
            ensure!(
                current.generation == record.generation
                    && current.conversation.as_deref() == Some(id),
                "Native conversation identity changed"
            );
        }
        let alive = verify_session(&client, session, pid)?;
        if !alive {
            return Ok(());
        }
        let observed = host::legacy_cli_identity(
            identity.0,
            program,
            if matches!(source, Source::Gui { .. }) {
                identity
                    .1
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
            } else {
                &record.version
            },
        );
        if let Ok(observed) = observed {
            ensure!(
                observed == (identity.1.clone(), identity.2.clone()),
                "Native CLI process identity changed"
            );
        } else {
            // This only retires the bootstrap. The application still requires the
            // daemon's actual exit or the original GUI host's presentation event.
            if available {
                return Ok(());
            }
            observed?;
        }
        if !available && activated_at.elapsed() > Duration::from_secs(15) {
            if let Ok(error) = journal::read::<Value>(&plugin.join("bridge.error")) {
                anyhow::bail!(
                    "Native switching bridge failed: {}",
                    error["message"].as_str().unwrap_or("unavailable")
                );
            }
        }
        if let Ok(state) = journal::read::<Value>(&plugin.join("bridge.state")) {
            if state["protocol"] == 1 && state["token"] == token && state["session"] == id {
                if let Some(value) = state["phase"].as_str().filter(|value| {
                    matches!(
                        *value,
                        "starting"
                            | "working"
                            | "draft"
                            | "background"
                            | "ready"
                            | "closing"
                            | "cancelled"
                    )
                }) {
                    if cancel.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    if value == "cancelled"
                        && requested.load(Ordering::Acquire)
                        && state["revision"] == 2
                    {
                        phase("cancelled");
                        return Ok(());
                    }
                    available = true;
                    lease.take();
                    let expires_at =
                        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() + 2500;
                    // Once any lease can exist, local cancellation is never proof.
                    // Retain takeover until the native side acknowledges revocation.
                    armed.store(true, Ordering::Release);
                    let cancelling = requested.load(Ordering::Acquire);
                    journal::atomic(
                        &plugin.join("return.request"),
                        &serde_json::to_vec(
                            &json!({"protocol":1,"token":token,"session":id,"expiresAt":expires_at, "revision":if cancelling {2}else{1}, "action":if cancelling {"cancel"}else{"return"}}),
                        )?,
                    )?;
                    if cancel.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    // Publish progress only after the corresponding lease exists.
                    // Cancellation acknowledgements must never race a later write.
                    phase(value);
                }
            }
        }
        // A command shortcut preserves the native draft. Busy command contexts can
        // ignore it; retry only while this exact CLI still owns this exact PTY.
        if !available
            && last_reload.elapsed() >= Duration::from_secs(3)
            && host::legacy_input_is_foreground(foreground_owner, identity.0)?
        {
            native_reload_if_unchanged(&client, session, pid)?;
            last_reload = Instant::now();
        }
        let guard = wake
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Native cancellation is unavailable"))?;
        if !cancel.load(Ordering::Acquire) {
            let _ = wake.1.wait_timeout(guard, Duration::from_millis(300));
        }
    }
}

/// Codex has no return hook in Kiln's GUI host. Ask its own quit handler only
/// after the native log and the visible composer both prove an idle boundary.
fn run_codex_gui_return(
    worker: &str,
    conversation: &str,
    record: &super::super::agent_runtime::Record,
    session: SessionId,
    pid: u32,
    socket: &str,
    cancel: &AtomicBool,
    requested: &AtomicBool,
    armed: &AtomicBool,
    wake: &(Mutex<()>, Condvar),
    phase: impl Fn(&str),
) -> Result<()> {
    ensure!(record.confirmed && conversation.len() == 36, "Missing confirmed Codex conversation");
    if cfg!(windows) { phase("manual"); return Ok(()); }
    let program = Path::new(record.spec.program.as_deref().context("Missing Codex executable")?);
    let identity = host::legacy_gui_cli_identity(pid, session, program, worker, conversation, Tool::Codex)?;
    let owner = host::legacy_gui_owner(pid, session, worker)?;
    let client = kiln_daemon::client::Client::connect(socket, None)?;
    ensure!(client.server_build == kiln_daemon::exe_build_id(&super::super::conn::daemon_exe()), "Update the daemon before switching this terminal");
    let root = record.spec.env.iter().find(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".codex")))
        .context("Codex home is unavailable")?;
    let mut log = None;
    let mut last_scan = Instant::now() - Duration::from_secs(2);
    let mut stable: Option<(u64, String, Instant)> = None;
    let mut last_send = Instant::now() - Duration::from_secs(3);
    let mut sent = false;
    let mut sent_at = None;
    loop {
        if cancel.load(Ordering::Acquire) { return Ok(()); }
        if requested.load(Ordering::Acquire) {
            if !sent { phase("cancelled"); return Ok(()); }
            // An accepted quit may already be in flight. Keep the host's
            // takeover armed until it observes the native process exit.
            if sent_at.is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(5)) {
                phase("cancelled");
                return Ok(());
            }
            phase("closing");
        }
        if !verify_session(&client, session, pid)? { return Ok(()); }
        match host::legacy_cli_identity(identity.0, program, identity.1.file_name().and_then(|name|name.to_str()).unwrap_or("")) {
            Ok(observed) => ensure!(observed == (identity.1.clone(), identity.2.clone()), "Codex process identity changed"),
            Err(_) if sent => return Ok(()),
            Err(error) => return Err(error),
        }
        if !host::legacy_input_is_foreground(owner, identity.0)? {
            phase("background"); stable = None;
        } else {
            if log.is_none() && last_scan.elapsed() >= Duration::from_secs(2) {
                log = codex_rollout(&root, conversation)?;
                last_scan = Instant::now();
            }
            let status = log.as_ref().map(|path| codex_turn_idle(path, conversation)).transpose()?.flatten();
            let ServerMsg::InputState { pid: observed, revision, text, .. } = client.request(
                |req| ClientMsg::ReadInputState { req, session }, Duration::from_secs(2),
            )? else { anyhow::bail!("Codex terminal state is unavailable"); };
            ensure!(observed == pid, "Codex terminal changed");
            if status == Some(true) && revision.is_some() && codex_idle_screen(&text) {
                let revision = revision.unwrap();
                let started = match &stable {
                    Some((prior_revision, prior_text, started)) if *prior_revision == revision && *prior_text == text => *started,
                    _ => Instant::now(),
                };
                stable = Some((revision, text.clone(), started));
                phase("ready");
                if started.elapsed() >= Duration::from_millis(700) && last_send.elapsed() >= Duration::from_secs(2)
                    && !requested.load(Ordering::Acquire) && !cancel.load(Ordering::Acquire)
                    && host::legacy_input_is_foreground(owner, identity.0)?
                    && codex_turn_idle(log.as_ref().unwrap(), conversation)? == Some(true)
                {
                    let reply = client.request(|req| ClientMsg::InputIfUnchanged {
                        req, session, pid, revision, text, data: vec![0x04],
                    }, Duration::from_secs(2))?;
                    if matches!(reply, ServerMsg::InputAccepted { accepted: true, .. }) {
                        sent = true;
                        sent_at = Some(Instant::now());
                        armed.store(true, Ordering::Release);
                        phase("closing");
                    }
                    last_send = Instant::now();
                    stable = None;
                }
            } else {
                phase(match status { Some(false) => "working", Some(true) => "draft", None => "starting" });
                stable = None;
            }
        }
        let guard = wake.0.lock().map_err(|_| anyhow::anyhow!("Codex return cancellation is unavailable"))?;
        if !cancel.load(Ordering::Acquire) {
            let _ = wake.1.wait_timeout(guard, Duration::from_millis(300));
        }
    }
}

fn codex_rollout(root: &Path, id: &str) -> Result<Option<PathBuf>> {
    let mut found = None;
    let sessions = root.join("sessions");
    if !sessions.is_dir() { return Ok(None); }
    for year in fs::read_dir(&sessions)? {
        let year = year?.path();
        if !year.is_dir() { continue; }
        for month in fs::read_dir(year)? {
            let month = month?.path();
            if !month.is_dir() { continue; }
            for day in fs::read_dir(month)? {
                let day = day?.path();
                if !day.is_dir() { continue; }
                for file in fs::read_dir(day)? {
                    let file = file?.path();
                    if file.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl"))) {
                        ensure!(found.is_none(), "Multiple Codex histories match this conversation");
                        ensure!(file.is_file() && file.canonicalize()?.starts_with(root.canonicalize()?), "Codex history is outside its account");
                        found = Some(file);
                    }
                }
            }
        }
    }
    Ok(found)
}

/// The last turn boundary must be complete. An incomplete or oversized tail
/// leaves the terminal alone until the next observation.
fn codex_turn_idle(path: &Path, id: &str) -> Result<Option<bool>> {
    const TAIL: u64 = 4 * 1024 * 1024;
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut first = String::new();
    {
        let mut reader = BufReader::new((&mut file).take(1024 * 1024));
        reader.read_line(&mut first)?;
    }
    let Ok(meta) = serde_json::from_str::<Value>(&first) else { return Ok(None); };
    ensure!(meta["type"] == "session_meta" && meta["payload"]["id"] == id, "Codex history identity changed");
    let start = size.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file).take(TAIL + 1024 * 1024).read_to_end(&mut bytes)?;
    if start > 0 {
        let Some(skip) = bytes.iter().position(|byte| *byte == b'\n') else { return Ok(None); };
        bytes.drain(..=skip);
    }
    let mut last = None;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() { continue; }
        let Ok(row) = serde_json::from_slice::<Value>(line) else { return Ok(None); };
        if row["type"] != "event_msg" { continue; }
        last = match row["payload"]["type"].as_str() {
            Some("task_started" | "user_message") => Some(false),
            Some("task_complete" | "turn_complete" | "turn_aborted") => Some(true),
            _ => last,
        };
    }
    Ok(last.or_else(|| (start == 0).then_some(true)))
}

fn codex_idle_screen(text: &str) -> bool {
    if text.len() > 1024 * 1024 { return false; }
    if text.contains("Working (") || text.contains("esc to interrupt") || text.contains("Copying ") { return false; }
    let lines: Vec<_> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let Some(prompt) = lines.iter().rposition(|line| *line == "› Ask Codex to do anything") else { return false; };
    let tail = &lines[prompt + 1..];
    tail.first().is_some_and(|line| line.contains(" · "))
        && tail.get(1).is_some_and(|line| line.contains("? for shortcuts"))
        && (tail.len() == 2
            || (tail.len() == 3 && tail[2].contains("warnings") && tail[2].contains("f2 to view")))
}
/// A command shortcut is not a safe reload while Claude is working: native
/// busy contexts can queue its slash command as a model prompt. Require the
/// actual, bounded idle terminal footer before asking the native command router.
fn native_reload_if_unchanged(
    client: &kiln_daemon::client::Client,
    session: SessionId,
    pid: u32,
) -> Result<bool> {
    let ServerMsg::InputState {
        pid: observed,
        revision: Some(revision),
        text,
        ..
    } = client.request(
        |req| ClientMsg::ReadInputState { req, session },
        Duration::from_secs(2),
    )?
    else {
        return Ok(false);
    };
    if observed != pid || !reload_context(&text) {
        return Ok(false);
    }
    let reply = client.request(
        |req| ClientMsg::InputIfUnchanged {
            req,
            session,
            pid,
            revision,
            text,
            data: KEYS.to_vec(),
        },
        Duration::from_secs(2),
    )?;
    Ok(matches!(
        reply,
        ServerMsg::InputAccepted { accepted: true, .. }
    ))
}
fn reload_context(text: &str) -> bool {
    if text.len() > 1024 * 1024 {
        return false;
    }
    let lines: Vec<_> = text.lines().map(str::trim).collect();
    let Some(prompt) = lines.iter().rposition(|line| line.starts_with('❯')) else {
        return false;
    };
    let mut tail = lines[prompt + 1..]
        .iter()
        .copied()
        .filter(|line| !line.is_empty());
    let Some(separator) = tail.next() else {
        return false;
    };
    if separator.chars().count() < 3 || !separator.chars().all(|c| c == '─') {
        return false;
    }
    let Some(footer) = tail.next() else {
        return false;
    };
    if tail.next().is_some() {
        return false;
    }
    matches!(
        footer,
        "? for shortcuts"
            | "⏵⏵ auto mode on (shift+tab to cycle)"
            | "⏵⏵ accept edits on (shift+tab to cycle)"
            | "⏵⏵ bypass permissions on (shift+tab to cycle)"
            | "⏸ plan mode on (shift+tab to cycle)"
    )
}

fn verify_session(
    client: &kiln_daemon::client::Client,
    session: SessionId,
    pid: u32,
) -> Result<bool> {
    let ServerMsg::Sessions { sessions, .. } = client.request(
        |req| ClientMsg::ListSessions { req },
        Duration::from_secs(2),
    )?
    else {
        anyhow::bail!("Native terminal ownership could not be confirmed");
    };
    let info = sessions
        .into_iter()
        .find(|info| info.id == session)
        .context("The original native terminal is unavailable")?;
    ensure!(info.pid == pid, "Native terminal process changed");
    Ok(info.exited.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_return_requires_empty_composer_and_completed_native_turn() {
        let screen = "final answer\n› Ask Codex to do anything\nGPT-6 high · /tmp/project\n← for agents · ? for shortcuts\n⚠ 10 warnings · f2 to view\n";
        assert!(codex_idle_screen(screen));
        for unsafe_screen in [
            screen.replace("Ask Codex to do anything", "unfinished draft"),
            screen.replace("GPT-6 high", "Working (4s • esc to interrupt)"),
            screen.replace("? for shortcuts", "approval required"),
            format!("• Working (0s • esc to interrupt)\n{screen}"),
            format!("{screen}modal dialog\n"),
        ] { assert!(!codex_idle_screen(&unsafe_screen)); }

        let root = tempfile::tempdir().unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        let dir = root.path().join("sessions/2026/11/20");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-2026-11-20T00-00-00-{id}.jsonl"));
        let meta = json!({"type":"session_meta","payload":{"id":id}});
        let started = json!({"type":"event_msg","payload":{"type":"task_started"}});
        let completed = json!({"type":"event_msg","payload":{"type":"task_complete"}});
        fs::write(&path, format!("{meta}\n")).unwrap();
        assert_eq!(codex_rollout(root.path(), id).unwrap(), Some(path.clone()));
        assert_eq!(codex_turn_idle(&path, id).unwrap(), Some(true));
        fs::write(&path, format!("{meta}\n{started}\n")).unwrap();
        assert_eq!(codex_turn_idle(&path, id).unwrap(), Some(false));
        fs::write(&path, format!("{meta}\n{started}\n{completed}\n")).unwrap();
        assert_eq!(codex_turn_idle(&path, id).unwrap(), Some(true));
        assert!(codex_turn_idle(&path, "aaaaaaaa-2222-3333-4444-555555555555").is_err());
    }
    #[test]
    fn native_reload_binding_restores_exact_bytes_and_keeps_concurrent_user_edits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("keybindings.json");
        let original = b"{\"bindings\":[],\"userNote\":\"keep formatting and metadata\"}\n";
        fs::write(&path, original).unwrap();
        {
            let first = KeyLease::acquire(root.path()).unwrap();
            let second = KeyLease::acquire(root.path()).unwrap();
            assert!(Arc::ptr_eq(&first, &second));
            drop(first);
            assert_ne!(fs::read(&path).unwrap(), original);
        }
        assert_eq!(fs::read(&path).unwrap(), original);
        {
            let _lease = KeyLease::acquire(root.path()).unwrap();
            let mut edited: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            edited["userNote"] = json!("changed concurrently");
            edited["bindings"]
                .as_array_mut()
                .unwrap()
                .push(json!({"context":"Chat","bindings":{"meta+p":"chat:modelPicker"}}));
            fs::write(&path, serde_json::to_vec(&edited).unwrap()).unwrap();
        }
        let edited: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(edited["userNote"], "changed concurrently");
        assert_eq!(edited["bindings"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn reload_requires_the_native_idle_footer_and_rejects_busy_shells_and_queued_prompts() {
        for footer in ["⏵⏵ auto mode on (shift+tab to cycle)", "? for shortcuts"] {
            assert!(reload_context(&format!(
                "old output\n───\n❯ preserved draft\n───\n  {footer}\n"
            )));
        }
        for footer in [
            "⏵⏵ auto mode on · esc to interrupt",
            "⏵⏵ auto mode on · 1 shell, 1 monitor",
            "! for shell mode",
            "ctrl+b to run in background",
            "Press up to edit queued messages",
            "unknown future native UI",
        ] {
            assert!(!reload_context(&format!("❯\n───\n{footer}")));
        }
        assert!(!reload_context("❯\n───\n? for shortcuts\napproval pending"));
        assert!(!reload_context(
            "❯\nnot a prompt separator\n? for shortcuts"
        ));
    }
    #[test]
    fn abandoned_shortcut_is_recovered_before_retry_and_preserves_concurrent_edits() {
        use base64::Engine;
        for edited in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("keybindings.json");
            let recovery = root.path().join("kiln-native-return.recovery.json");
            let original = br#"{"bindings":[],"note":"original"}"#;
            let (installed, block) = binding_bytes(Some(original)).unwrap();
            let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
            journal::atomic(&recovery,&serde_json::to_vec(&json!({"protocol":1,"original":encode(original),"installed":encode(&installed),"block":block})).unwrap()).unwrap();
            fs::write(&path, &installed).unwrap();
            if edited {
                let mut value: Value = serde_json::from_slice(&installed).unwrap();
                value["note"] = json!("concurrent");
                fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            {
                let _retry = KeyLease::acquire(root.path()).unwrap();
            }
            assert!(!recovery.exists());
            let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(value["bindings"], json!([]));
            assert_eq!(
                value["note"],
                if edited { "concurrent" } else { "original" }
            );
            if !edited {
                assert_eq!(fs::read(&path).unwrap(), original);
            }
        }
    }
    #[test]
    fn native_reload_never_overwrites_custom_keys_or_unreadable_configuration() {
        for original in [br#"{"bindings":[{"context":"Chat","bindings":{"control+x control+f":"command:help"}}]}"#.as_slice(),
            br#"{"bindings":[{"context":"Chat","bindings":{"ctrl+x ctrl+f ctrl+g":"command:help"}}]}"#.as_slice(),

            br#"{"bindings":[{"context":"Chat","bindings":{"ctrl+x ctrl+f":"command:help"}}]}"#
                .as_slice(),
            br#"{"bindings":[{"context":"Global","bindings":{"ctrl+x":"app:exit"}}]}"#.as_slice(),
            br#"{"bindings":[{"context":"Chat","bindings":{"ctrl+f":"chat:clearInput"}}]}"#
                .as_slice(),
            b"// preserve unsupported JSONC unchanged",
        ] {
            assert!(binding_bytes(Some(original)).is_err());
        }
    }
}
