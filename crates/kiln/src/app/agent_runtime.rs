//! Per-launch identity and bounded CLI updates. Never infer identity from the newest transcript.
use kiln_common::paths::Canonical;
use super::{agent_launch, remote_terminal};
use anyhow::{Context, Result, bail, ensure};
use kiln_accounts::{AccountManager, Tool};
use kiln_proto::SpawnSpec;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Record {
    pub tool: Tool,
    pub spec: SpawnSpec,
    pub version: String,
    pub conversation: Option<String>,
    pub generation: String,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default)]
    pub request_started: bool,
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn helper_exe() -> Result<PathBuf> {
    if std::env::var_os("KILN_ACCOUNTS_SANDBOX").is_some() {
        Ok(super::conn::daemon_exe())
    } else {
        Ok(kiln_daemon::client::cli_executable(&std::env::current_exe()?))
    }
}
fn directory() -> PathBuf {
    kiln_common::paths::config_dir().join("agent-sessions")
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    Ok(())
}

fn codex_store_override(spec: &SpawnSpec) -> String {
    let mode = if spec
        .env
        .iter()
        .any(|(key, value)| key == "KILN_CODEX_AUTH_STORE" && value == "keyring")
    {
        "keyring"
    } else {
        "file"
    };
    format!("cli_auth_credentials_store=\"{mode}\"")
}
fn lock_record(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path.with_extension("lock"))?;
    file.try_lock()?;
    Ok(file)
}
pub fn read(path: &Path) -> Result<Record> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "invalid session record"
    );
    let bytes = fs::read(path)?;
    ensure!(bytes.len() < 64 * 1024, "oversized session record");
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn record_path(spec: &SpawnSpec) -> Option<PathBuf> {
    spec.env
        .iter()
        .find(|(k, _)| k == "KILN_AGENT_RECORD")
        .map(|(_, v)| PathBuf::from(v))
}
pub fn managed(tool: Tool, cwd: PathBuf, accounts: &AccountManager) -> Result<SpawnSpec> {
    let mut spec = remote_terminal::agent(tool, cwd).map_err(anyhow::Error::msg)?;
    spec.env = accounts.launch_environment(tool)?;
    if tool == Tool::Codex {
        spec.args.push("--no-daemon".into());
        if spec.env.iter().any(|(k, _)| k == "KILN_AGENT_PROFILE") {
            let store = codex_store_override(&spec);
            spec.args.extend(["-c".into(), store]);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let generation = format!("{}-{stamp}", std::process::id());
    let path = directory().join(format!("{generation}.json"));
    let version = String::new();
    let record = Record {
        tool,
        spec: spec.clone(),
        version,
        conversation: None,
        generation,
        confirmed: false,
        request_started: false,
    };
    private_write(&path, &serde_json::to_vec(&record)?)?;
    attach(&mut spec, &path, &record)?;
    launch_spec(&path, &record, false, None)
}

/// Bare `agy` in a Kiln shell uses the account selected at invocation time.
pub fn run_antigravity_shell(cwd: PathBuf, args: Vec<String>) -> Result<()> {
    let accounts = AccountManager::with_env(kiln_accounts::Env::real());
    let spec = managed(Tool::Antigravity, cwd, &accounts)?;
    let path = record_path(&spec).context("missing launch record")?;
    let mut record = read(&path)?;
    record.spec.args.extend(args);
    private_write(&path, &serde_json::to_vec(&record)?)?;
    run_request(&path, &record.generation, None, false)
}
/// The launch record and generation in a `kiln agent-run --record <path> --generation <id>`
/// command line.
#[cfg(any(windows, test))]
fn launch_in_command_line(args: &[String]) -> Option<(PathBuf, String)> {
    let program = args.first()?.rsplit(['/', '\\']).next()?;
    let program = program.strip_suffix(".exe").or_else(|| program.strip_suffix(".EXE")).unwrap_or(program);
    if !program.eq_ignore_ascii_case("kiln") || args.get(1).map(String::as_str) != Some("agent-run") {
        return None;
    }
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    Some((PathBuf::from(value("--record")?), value("--generation")?))
}

/// The launch of the nearest `kiln agent-run` ancestor of this process. Codex on Windows starts
/// hooks with an empty environment, so its hooks find their launch this way.
#[cfg(windows)]
pub fn launch_from_ancestors() -> Option<(PathBuf, String)> {
    let mut pid = std::process::id();
    for _ in 0..8 {
        pid = kiln_daemon::procinfo::parent_of(pid)?;
        if let Some(launch) = kiln_daemon::procinfo::cmdline(pid).as_deref().and_then(launch_in_command_line) {
            return Some(launch);
        }
    }
    None
}

/// The hook command for `tool` in the shell that CLI runs hooks with. Codex on Windows uses the
/// session's shell (PowerShell by default, or cmd), so the helper is a plain path, shortened to
/// its 8.3 form when that removes spaces, or else PowerShell's `& '...'`. Claude Code runs hooks
/// with Git Bash on Windows and `sh` elsewhere.
fn hook_command(tool: Tool, helper: &Path) -> String {
    #[cfg(windows)]
    if tool == Tool::Codex {
        let plain = |text: &str| text.chars().all(|c| c.is_ascii_alphanumeric() || "\\:._-~".contains(c));
        let short = short_path(helper).unwrap_or_else(|| helper.to_path_buf());
        let text = short.to_string_lossy();
        return if plain(&text) {
            format!("{text} agent-hook")
        } else {
            format!("& '{}' agent-hook", helper.to_string_lossy().replace('\'', "''"))
        };
    }
    let _ = tool;
    format!("{} agent-hook", quote(&helper.to_string_lossy()))
}

/// The 8.3 form of an existing path, when the volume keeps short names.
#[cfg(windows)]
fn short_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `buf` holds `buf.len()` units.
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
}
fn attach(spec: &mut SpawnSpec, path: &Path, record: &Record) -> Result<()> {
    let helper = helper_exe()?;
    let command = hook_command(record.tool, &helper);
    let hooks = serde_json::json!([{ "hooks": [{"type":"command","command":command,"timeout":3}]}]);
    match record.tool {
        // CLI override is an additional config layer; never rewrite user/project hooks.
        Tool::Codex => {
            let hook = &hooks[0]["hooks"][0];
            let toml = format!(
                "hooks.SessionStart=[{{hooks=[{{type=\"command\",command={},timeout=3}}]}}]",
                serde_json::to_string(hook["command"].as_str().unwrap())?
            );
            spec.args.extend([
                "-c".into(),
                toml.clone(),
                "-c".into(),
                toml.replace("hooks.SessionStart=", "hooks.UserPromptSubmit="),
            ]);
        }
        Tool::Claude => spec.args.extend([
            "--settings".into(),
            serde_json::json!({"hooks":{"SessionStart":hooks,"UserPromptSubmit":hooks}})
                .to_string(),
        ]),
        // agy runs no Kiln hook; `observe` records the conversation it holds open.
        Tool::Antigravity => {}
    }
    spec.env.push((
        "KILN_AGENT_RECORD".into(),
        path.to_string_lossy().into_owned(),
    ));
    spec.env
        .push(("KILN_AGENT_GENERATION".into(), record.generation.clone()));
    Ok(())
}
pub fn resume(path: &Path) -> Result<SpawnSpec> {
    let record = read(path)?;
    let id = record
        .conversation
        .as_deref()
        .context("conversation ID is not confirmed")?;
    ensure!(valid_id(id), "invalid conversation ID");
    if record.tool.has_accounts() && (record.tool != Tool::Antigravity || record.spec.env.iter().any(|(k, _)| k == "KILN_AGENT_PROFILE")) {
        let key = match record.tool { Tool::Codex => "CODEX_HOME", Tool::Claude => "CLAUDE_CONFIG_DIR", Tool::Antigravity => "HOME" };
        ensure!(
            record
                .spec
                .env
                .iter()
                .any(|(k, v)| k == "KILN_AGENT_PROFILE" && !v.is_empty()),
            "original account not pinned"
        );
        let config = record
            .spec
            .env
            .iter()
            .find(|(k, _)| k == key)
            .context("original account configuration missing")?;
        ensure!(
            Path::new(&config.1).join("initialized").is_file(),
            "original account directory missing"
        );
    }
    let mut spec = record.spec.clone();
    spec.args = match record.tool {
        Tool::Codex => vec![
            "resume".into(),
            id.into(),
            "--no-daemon".into(),
            "-c".into(),
            codex_store_override(&spec),
        ],
        Tool::Claude => vec!["--resume".into(), id.into()],
        Tool::Antigravity => vec!["--conversation".into(), id.into()],
    };
    attach(&mut spec, path, &record)?;
    Ok(spec)
}
pub fn renew(path: &Path) -> Result<SpawnSpec> {
    let _lock = lock_record(path)?;
    let mut record = read(path)?;
    ensure!(
        record.conversation.as_deref().is_some_and(valid_id),
        "missing conversation ID"
    );
    record.confirmed = false;
    record.version.clear();
    record.generation = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    private_write(path, &serde_json::to_vec(&record)?)?;
    launch_spec(path, &record, true, None)
}
pub(super) fn redirect(path: &Path, generation: &str, id: &str) -> Result<()> {
    ensure!(valid_id(id), "Invalid canonical conversation ID");
    let _lock = lock_record(path)?;
    let mut record = read(path)?;
    ensure!(record.generation == generation, "Conversation transfer was superseded");
    record.conversation = Some(id.into());
    private_write(path, &serde_json::to_vec(&record)?)
}
pub fn request_spec(agent: &SpawnSpec, request: &Path) -> Result<SpawnSpec> {
    let path = record_path(agent).context("missing launch record")?;
    let record = read(&path)?;
    launch_spec(&path, &record, false, Some(request))
}
fn launch_spec(
    path: &Path,
    record: &Record,
    resumed: bool,
    request: Option<&Path>,
) -> Result<SpawnSpec> {
    let mut spec = record.spec.clone();
    // A managed Agy account changes HOME. The Kiln helper must keep using the
    // GUI's config directory for its launch record and request validation.
    spec.env.retain(|(key, _)| key != "KILN_CONFIG_DIR");
    spec.env.push(("KILN_CONFIG_DIR".into(), kiln_common::paths::config_dir().to_string_lossy().into_owned()));
    spec.program = Some(helper_exe()?.to_string_lossy().into_owned());
    spec.args = vec![
        "agent-run".into(),
        "--record".into(),
        path.to_string_lossy().into_owned(),
        "--generation".into(),
        record.generation.clone(),
    ];
    if resumed {
        spec.args.push("--resume".into());
    }
    if let Some(request) = request {
        spec.args
            .extend(["--request".into(), request.to_string_lossy().into_owned()]);
    }
    spec.env.push((
        "KILN_AGENT_RECORD".into(),
        path.to_string_lossy().into_owned(),
    ));
    Ok(spec)
}
fn request_mode(record: &Record, has_request: bool, resumed: bool) -> Result<bool> {
    if has_request && (record.request_started || record.conversation.is_some()) {
        ensure!(
            record.conversation.as_deref().is_some_and(valid_id),
            "{}",
            kiln_common::i18n::tr(
                "이전 요청이 이미 전송되었을 수 있습니다. 대화 ID가 없어 자동 재전송하지 않습니다. 새 작업에서 요청을 확인해 다시 시작하세요."
            )
        );
        return Ok(true);
    }
    Ok(resumed)
}
pub fn run_request(
    path: &Path,
    generation: &str,
    request: Option<&Path>,
    resumed: bool,
) -> Result<()> {
    ensure!(
        path.parent()
            .context("missing record directory")?
            .canonical()?
            == directory().canonical()?,
        "invalid record directory"
    );
    let mut record = read(path)?;
    ensure!(record.generation == generation, "stale CLI launch");
    // This helper is a PTY child, never the UI thread. Probe the executable immediately before exec.
    let version = probe(Path::new(
        record.spec.program.as_ref().context("missing CLI binary")?,
    ))?;
    let initial_mode = request_mode(&record, request.is_some(), resumed)?;
    let payload = if !initial_mode && let Some(request) = request {
        ensure!(
            request
                .parent()
                .context("missing request directory")?
                .canonical()?
                == kiln_common::paths::config_dir()
                    .join("agent-requests")
                    .canonical()?,
            "invalid request directory"
        );
        ensure!(
            fs::symlink_metadata(request)?.file_type().is_file(),
            "invalid request file"
        );
        let payload = fs::read_to_string(request)?;
        ensure!(payload.len() <= 120_000, "request too large");
        Some(payload)
    } else {
        None
    };
    let resumed = {
        let _lock = lock_record(path)?;
        record = read(path)?;
        ensure!(record.generation == generation, "stale CLI launch");
        let resumed = request_mode(&record, request.is_some(), resumed)?;
        record.version = version;
        if request.is_some() && !resumed {
            record.request_started = true;
        }
        if resumed {
            record.confirmed = false;
        }
        private_write(path, &serde_json::to_vec(&record)?)?;
        resumed
    };
    let mut spec = if resumed {
        resume(path)?
    } else {
        record.spec.clone()
    };
    if !resumed {
        attach(&mut spec, path, &record)?;
    }
    let retry = std::env::var("KILN_CAPACITY_RETRY_TURN").ok();
    if let Some(turn) = &retry {
        ensure!(resumed && record.tool == Tool::Codex, "invalid capacity retry");
        ensure!(!super::conversation_sync::handover::blocked(&kiln_common::paths::config_dir(), path, &record), "Conversation ownership transferred; capacity retry cancelled");
        let failure = super::capacity_recovery::failure(&record)?.context("capacity error no longer current")?;
        ensure!(&failure.turn == turn, "capacity retry superseded by another turn");
        spec.args.push(super::capacity_recovery::CONTINUE.into());
    }
    let (program, lead) = agent_launch::launcher(Path::new(&spec.program.context("missing CLI executable")?));
    let mut cmd = Command::new(program);
    if let Some(cwd) = &spec.cwd { cmd.current_dir(cwd); }
    cmd.args(lead).args(&spec.args).envs(spec.env)
        .env("KILN_CONFIG_DIR", kiln_common::paths::config_dir())
        .env_remove("KILN_CAPACITY_RETRY_TURN");
    if !resumed && let Some(payload) = payload {
        // agy takes the initial prompt as --prompt-interactive.
        if record.tool == Tool::Antigravity {
            cmd.arg("--prompt-interactive");
        }
        cmd.arg(payload);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        return Err(cmd.exec().into());
    }
    #[cfg(windows)]
    std::process::exit(run_contained(cmd)?);
}

/// Runs the agent and returns its exit code. Ctrl+C and Ctrl+Break reach the agent through the
/// shared console while the launcher keeps waiting for it; the terminal's job ends both.
#[cfg(windows)]
fn run_contained(mut cmd: Command) -> Result<i32> {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe extern "system" fn keep_waiting(_event: u32) -> i32 {
        1
    }
    // SAFETY: the handler only reports the event as handled.
    unsafe { SetConsoleCtrlHandler(Some(keep_waiting), 1) };
    Ok(cmd.status()?.code().unwrap_or(1))
}
#[derive(Debug, PartialEq)]
enum Installer {
    Brew(PathBuf, bool),
    Npm(PathBuf),
}
fn installer(binary: &Path) -> Result<Installer> {
    installer_with(binary, agent_launch::executable)
}
/// The npm package folder behind a Windows npm launcher (`codex.cmd` or `codex.ps1` next to
/// `node_modules`).
fn npm_launcher_package(binary: &Path) -> Option<PathBuf> {
    let extension = binary.extension()?.to_string_lossy().to_ascii_lowercase();
    if extension != "cmd" && extension != "ps1" {
        return None;
    }
    binary.parent()?.join("node_modules").join("@openai").join("codex").canonical().ok()
}
fn installer_with(binary: &Path, resolve: impl Fn(&str) -> Option<PathBuf>) -> Result<Installer> {
    let resolved = match npm_launcher_package(binary) {
        Some(package) => package,
        None => binary.canonical()?,
    };
    let names: Vec<_> = resolved.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let contains = |run: &[&str]| names.windows(run.len()).any(|window| window.iter().zip(run).all(|(name, part)| name == part));
    if contains(&["Caskroom", "codex"]) || contains(&["Cellar", "codex"]) {
        let brew = resolve("brew").context("Homebrew not found")?;
        let mut cmd = Command::new(&brew);
        kiln_common::process::hide_window(&mut cmd);
        cmd.arg("--prefix");
        let prefix =
            PathBuf::from(String::from_utf8(bounded(cmd, Duration::from_secs(5))?)?.trim())
                .canonical()?;
        let cask = contains(&["Caskroom", "codex"]);
        ensure!(
            resolved.starts_with(prefix.join(if cask { "Caskroom" } else { "Cellar" }).join("codex")),
            "Homebrew prefix does not own this Codex executable"
        );
        return Ok(Installer::Brew(brew, cask));
    }
    if contains(&["node_modules", "@openai", "codex"]) {
        let npm = resolve("npm").context("npm not found")?;
        let mut cmd = Command::new(&npm);
        kiln_common::process::hide_window(&mut cmd);
        cmd.args(["root", "--global"]);
        let root = PathBuf::from(String::from_utf8(bounded(cmd, Duration::from_secs(5))?)?.trim())
            .canonical()?;
        ensure!(
            resolved.starts_with(root.join("@openai").join("codex").canonical()?),
            "npm global root does not own this Codex executable"
        );
        return Ok(Installer::Npm(npm));
    }
    bail!(
        "Codex installation is not managed by a supported package manager; update it with its installer"
    )
}
fn update_command(tool: Tool, binary: &Path) -> Result<Command> {
    if tool == Tool::Claude || tool == Tool::Antigravity {
        let mut cmd = Command::new(binary);
        kiln_common::process::hide_window(&mut cmd);
        cmd.arg("update");
        return Ok(cmd);
    }
    // `codex update` also restarts its shared daemon. Package installation does not.
    match installer(binary)? {
        Installer::Brew(brew, cask) => {
            let mut cmd = Command::new(brew);
            kiln_common::process::hide_window(&mut cmd);
            cmd.args([
                "upgrade",
                if cask { "--cask" } else { "--formula" },
                "codex",
            ]);
            cmd.env("HOMEBREW_NO_AUTO_UPDATE", "1");
            Ok(cmd)
        }
        Installer::Npm(npm) => {
            let mut cmd = Command::new(npm);
            kiln_common::process::hide_window(&mut cmd);
            cmd.args(["install", "--global", "@openai/codex@latest"]);
            Ok(cmd)
        }
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
/// Hidden helper: accepts only a SessionStart UUID, never writes transcript or token data.
pub fn hook(path: &Path, generation: &str, input: impl Read) -> Result<()> {
    match hook_in(&directory(), path, generation, input) {
        Ok(Some(output)) => println!("{output}"),
        Ok(None) => {},
        Err(error) => println!("{}", serde_json::json!({"decision":"block","reason":format!("Kiln could not confirm conversation ownership: {error}")})),
    }
    Ok(())
}
fn hook_in(root: &Path, path: &Path, generation: &str, input: impl Read) -> Result<Option<serde_json::Value>> {
    let root = root.canonical()?;
    ensure!(
        path.parent()
            .context("missing record parent")?
            .canonical()?
            == root,
        "record outside Kiln session directory"
    );
    let _lock = lock_record(path)?;
    let mut record = read(path)?;
    ensure!(record.generation == generation, "stale launch hook");
    let mut bytes = Vec::new();
    input.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 64 * 1024, "hook payload too large");
    let event: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        matches!(
            event["hook_event_name"].as_str(),
            Some("SessionStart" | "UserPromptSubmit")
        ),
        "unexpected hook event"
    );
    let id = event["session_id"]
        .as_str()
        .context("missing conversation ID")?;
    ensure!(valid_id(id), "invalid conversation ID");
    // Team/subagent sessions must never replace the parent panel identity.
    if event.get("agent_id").is_some_and(|v| !v.is_null())
        || event.get("parent_session_id").is_some_and(|v| !v.is_null())
    {
        return Ok(None);
    }
    if !record.confirmed
        && let Some(expected) = &record.conversation
    {
        ensure!(
            expected == id,
            "CLI did not resume the original conversation"
        );
    }
    record.confirmed = true;
    record.conversation = Some(id.to_owned());
    private_write(path, &serde_json::to_vec(&record)?)?;
    if let (Some(config), Some(transcript)) = (root.parent(), event["transcript_path"].as_str()) {
        // Missing/not-yet-created transcript metadata must not break native CLI hooks.
        let _ = super::conversation_sync::track(config, &record, transcript);
    }
    if let Some(config) = root.parent() {
        return super::conversation_sync::handover::event(config, path, &record, event["hook_event_name"].as_str().unwrap(), event["source"].as_str());
    }
    Ok(None)
}
/// agy's data folder (`KILN_ANTIGRAVITY_DIR` in isolated runs); `presence/<conversation>.lock`
/// stays open while a conversation is.
pub(super) fn antigravity_root() -> Option<PathBuf> {
    std::env::var_os("KILN_ANTIGRAVITY_DIR")
        .map(PathBuf::from)
        .or_else(|| kiln_common::paths::home().map(|home| home.join(".gemini").join("antigravity-cli")))
}
/// The single conversation whose presence lock under `root` is among `open` paths.
fn held_conversation(root: &Path, open: &[PathBuf]) -> Option<String> {
    let presence = root.join("presence");
    let mut ids = open.iter().filter(|path| path.parent() == Some(presence.as_path())).filter_map(|path| {
        let id = path.file_name()?.to_str()?.strip_suffix(".lock")?;
        valid_id(id).then(|| id.to_owned())
    });
    let id = ids.next()?;
    ids.all(|other| other == id).then_some(id)
}
/// Records the conversation an agy launch holds open, as the SessionStart hook does for
/// the other CLIs: a resumed launch confirms only its original conversation.
pub fn observe(path: &Path, generation: &str, id: &str) -> Result<()> {
    ensure!(valid_id(id), "invalid conversation ID");
    // A lock briefly held elsewhere (or inherited by a process being spawned) is waited for.
    let mut waits = 0;
    let _lock = loop {
        match lock_record(path) {
            Ok(lock) => break lock,
            Err(error)
                if waits < 25
                    && error.downcast_ref::<std::fs::TryLockError>().is_some_and(|e| matches!(e, std::fs::TryLockError::WouldBlock)) =>
            {
                waits += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    };
    let mut record = read(path)?;
    ensure!(record.tool == Tool::Antigravity, "not an Antigravity launch");
    ensure!(record.generation == generation, "stale launch");
    if record.confirmed && record.conversation.as_deref() == Some(id) {
        return Ok(());
    }
    if !record.confirmed
        && let Some(expected) = &record.conversation
    {
        ensure!(expected == id, "CLI did not resume the original conversation");
    }
    record.confirmed = true;
    record.conversation = Some(id.to_owned());
    private_write(path, &serde_json::to_vec(&record)?)
}
/// Records, off the UI thread, the conversations Kiln-launched agy panels hold open.
#[derive(Default)]
pub struct Observer {
    last: Option<Instant>,
    running: Option<Receiver<Vec<(PathBuf, bool)>>>,
    /// Whether each launch record is an agy launch.
    agy: std::collections::HashMap<PathBuf, bool>,
}
impl Observer {
    /// Whether the next check is due.
    pub fn due(&self) -> bool {
        !cfg!(windows) && self.last.is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
    }
    /// Checks the agy launches among `launches` (record, generation, process) at most every
    /// two seconds.
    pub fn tick(&mut self, mut launches: Vec<(PathBuf, String, u32)>) {
        if cfg!(windows) {
            return;
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(kinds) => self.agy.extend(kinds),
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
            self.running = None;
        }
        if !self.due() {
            return;
        }
        self.last = Some(Instant::now());
        launches.retain(|(path, _, _)| self.agy.get(path) != Some(&false));
        if launches.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.running = Some(rx);
        std::thread::spawn(move || {
            let mut kinds = Vec::new();
            for (path, generation, pid) in launches {
                let Ok(record) = read(&path) else { continue };
                kinds.push((path.clone(), record.tool == Tool::Antigravity));
                if record.tool != Tool::Antigravity || record.generation != generation {
                    continue;
                }
                let root = record.spec.env.iter().find(|(k, _)| k == "ANTIGRAVITY_APP_DATA_DIR")
                    .map(|(_, v)| PathBuf::from(v)).or_else(antigravity_root);
                let id = root.and_then(|root| root.canonical().ok()).and_then(|root| held_conversation(&root, &kiln_daemon::procinfo::open_files(pid)));
                if let Some(id) = id
                    && !(record.confirmed && record.conversation.as_deref() == Some(id.as_str()))
                    && let Err(error) = observe(&path, &generation, &id)
                {
                    log::debug!("agy conversation {id} not recorded: {error:#}");
                }
            }
            let _ = tx.send(kinds);
        });
    }
}
/// The record and generation of a `kiln agent-run` launch spec.
pub fn launch_of(spec: &SpawnSpec) -> Option<(PathBuf, String)> {
    let value = |name: &str| spec.args.iter().position(|a| a == name).and_then(|i| spec.args.get(i + 1)).cloned();
    (spec.args.first().map(String::as_str) == Some("agent-run")).then_some(())?;
    Some((PathBuf::from(value("--record")?), value("--generation")?))
}
fn bounded(mut cmd: Command, timeout: Duration) -> Result<Vec<u8>> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(128 * 1024).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let bytes = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
            ensure!(
                status.success(),
                "CLI command failed (exit {:?}); use its installer to update",
                status.code()
            );
            return Ok(bytes);
        }
        if start.elapsed() > timeout {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("CLI command timed out");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
fn parse_version(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)?;
    let re = regex::Regex::new(r"\b([0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.]+)?)\b")?;
    Ok(re.captures(text).context("CLI version not reported")?[1].into())
}
pub(super) fn probe(binary: &Path) -> Result<String> {
    let mut cmd = Command::new(binary);
    kiln_common::process::hide_window(&mut cmd);
    cmd.arg("--version");
    parse_version(&bounded(cmd, Duration::from_secs(5))?)
}
#[derive(Default, Clone)]
pub struct Version {
    pub installed: String,
    pub latest: String,
    pub binary: Option<PathBuf>,
    pub busy: bool,
    pub error: Option<String>,
    pub installing: bool,
    pub checking_latest: bool,
    pub latest_error: bool,
    /// The CLI's executable was not found.
    pub missing: bool,
}
struct Reply {
    tool: Tool,
    result: Result<Version, String>,
    updated: bool,
    complete: bool,
}
pub struct Updates {
    pub versions: [Version; 3],
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    last: Instant,
    automatic: bool,
    observed: [String; 3],
}
fn index(t: Tool) -> usize {
    match t {
        Tool::Claude => 0,
        Tool::Codex => 1,
        Tool::Antigravity => 2,
    }
}
impl Default for Updates {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            versions: Default::default(),
            tx,
            rx,
            last: Instant::now() - Duration::from_secs(24 * 3600),
            automatic: false,
            observed: Default::default(),
        }
    }
}
impl Updates {
    pub fn request(&mut self, tool: Tool, install: bool) {
        let status = &mut self.versions[index(tool)];
        if status.busy {
            return;
        }
        status.busy = true;
        status.error = None;
        status.installing = install;
        self.request_binary(tool, install, None);
    }
    fn request_binary(&mut self, tool: Tool, install: bool, fixture: Option<PathBuf>) {
        let tx = self.tx.clone();
        let offline = std::env::var_os("KILN_ACCOUNTS_SANDBOX").is_some();
        std::thread::spawn(move || {
            let result = (|| -> Result<(Version, bool)> {
                let Some(binary) = fixture.or_else(|| agent_launch::executable(tool.program())) else {
                    return Ok((Version { missing: true, ..Default::default() }, false));
                };
                version_request(tool, install, binary, &tx, || latest_version(tool, offline))
            })();
            let (updated, result) = match result {
                Ok((v, u)) => (u, Ok(v)),
                Err(e) => (false, Err(e.to_string())),
            };
            let _ = tx.send(Reply {
                tool,
                result,
                updated,
                complete: true,
            });
        });
    }
    #[doc(hidden)]
    pub fn fixture_update(&mut self, tool: Tool, binary: PathBuf) {
        assert!(std::env::var_os("KILN_ACCOUNTS_SANDBOX").is_some());
        self.versions[index(tool)].busy = true;
        self.request_binary(tool, true, Some(binary));
    }
    pub fn tick(&mut self, automatic: bool) -> Vec<Tool> {
        let mut changed = vec![];
        while let Ok(reply) = self.rx.try_recv() {
            let slot = &mut self.versions[index(reply.tool)];
            match reply.result {
                Ok(v) => {
                    let external = self.observed[index(reply.tool)] != v.installed;
                    if reply.complete {
                        self.observed[index(reply.tool)] = v.installed.clone();
                    }
                    *slot = v;
                    if reply.complete && (reply.updated || external) {
                        changed.push(reply.tool);
                    }
                }
                Err(e) => {
                    slot.busy = false;
                    slot.installing = false;
                    slot.checking_latest = false;
                    slot.error = Some(e);
                }
            }
        }
        if std::env::var_os("KILN_ACCOUNTS_SANDBOX").is_some() {
            return changed;
        }
        let enabled = automatic && !self.automatic;
        self.automatic = automatic;
        if enabled || self.last.elapsed() > Duration::from_secs(6 * 3600) {
            self.last = Instant::now();
            for tool in Tool::ALL {
                self.request(tool, automatic);
            }
        }
        changed
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, s: &mut super::state::Settings) {
        use kiln_common::i18n::tr;
        use kiln_common::{
            Theme, fonts,
            icons::{self, Icon},
            widgets::{self, ButtonKind},
        };
        let theme = Theme::current();
        ui.add_space(12.0);
        widgets::divider(ui);
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(tr("CLI 업데이트"))
                .font(fonts::semibold(16.0))
                .color(theme.text),
        );
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.checkbox(
            &mut s.cli_auto_update,
            tr("Codex·Claude Code·Antigravity 자동 업데이트"),
        );
        ui.checkbox(
            &mut s.cli_restart_after_update,
            tr("업데이트 직후 같은 대화로 세션 재시작"),
        );
        if s.cli_restart_after_update {
            ui.add(egui::Label::new(egui::RichText::new(tr("재시작하면 진행 중인 응답이 중단됩니다. 대화 ID를 확인한 패널에만 적용됩니다.")).font(fonts::regular(12.0)).color(theme.text_dim)).wrap());
        }
        ui.add_space(6.0);
        for tool in Tool::ALL {
            let v = self.versions[index(tool)].clone();
            egui::Frame::new().fill(theme.bg_elevated).stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(10).inner_margin(14).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                ui.horizontal_wrapped(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                    let (icon, color) = match tool {
                        Tool::Claude => (Icon::Claude, theme.orange),
                        Tool::Codex => (Icon::Codex, theme.text),
                        Tool::Antigravity => (Icon::Antigravity, theme.purple),
                    };
                    icons::paint(ui.painter(), rect.shrink(2.0), icon, color);
                    ui.label(egui::RichText::new(tool.display_name()).font(fonts::semibold(14.0)).color(theme.text));
                    if !v.installed.is_empty() { widgets::pill(ui, &v.installed, theme.text_dim); }
                });
                if let Some(path) = &v.binary {
                    ui.add(egui::Label::new(egui::RichText::new(path.display().to_string()).font(fonts::regular(11.5)).color(theme.text_faint)).truncate()).on_hover_text(path.display().to_string());
                }
                ui.horizontal_wrapped(|ui| {
                    if v.busy { ui.add(egui::Spinner::new().size(13.0)); }
                    let text = if v.installing { tr("업데이트 중…").to_string() }
                        else if v.checking_latest { tr("최신 버전 확인 중…").to_string() }
                        else if v.latest_error { tr("최신 버전 확인 실패").to_string() }
                        else if !v.latest.is_empty() { format!("{}  {}", tr("최신"), v.latest) }
                        else if v.busy { tr("확인 중…").to_string() }
                        else if v.missing { tr("설치되지 않음").to_string() }
                        else if v.installed.is_empty() { tr("버전 정보 없음").to_string() }
                        else { String::new() };
                    if !text.is_empty() { ui.label(egui::RichText::new(text).font(fonts::regular(12.0)).color(theme.text_dim)); }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!v.busy, |ui| {
                        if widgets::button_with(ui, Some(Icon::Refresh), tr("버전 확인"), ButtonKind::Secondary, true).clicked() { self.request(tool, false); }
                        if ui.add_enabled_ui(!v.missing, |ui| widgets::button_with(ui, Some(Icon::Download), tr("CLI 업데이트"), ButtonKind::Secondary, true)).inner.clicked() { self.request(tool, true); }
                    });
                });
                if v.error.is_some() {
                    ui.add(egui::Label::new(egui::RichText::new(tr("CLI 업데이트 실패. 설치 프로그램 또는 패키지 관리자를 확인하세요.")).font(fonts::regular(12.0)).color(theme.red)).wrap());
                }
            });
            ui.add_space(4.0);
        }
    }
}
fn version_request(
    tool: Tool,
    install: bool,
    binary: PathBuf,
    tx: &Sender<Reply>,
    latest: impl FnOnce() -> Option<String>,
) -> Result<(Version, bool)> {
    let before = probe(&binary)?;
    let mut version = Version {
        installed: before.clone(),
        binary: Some(binary.clone()),
        busy: true,
        installing: install,
        checking_latest: !install,
        ..Default::default()
    };
    let _ = tx.send(Reply {
        tool,
        result: Ok(version.clone()),
        updated: false,
        complete: false,
    });
    if install {
        bounded(update_command(tool, &binary)?, Duration::from_secs(300))?;
        version.installed = probe(&binary)?;
        version.installing = false;
        version.checking_latest = true;
        let _ = tx.send(Reply {
            tool,
            result: Ok(version.clone()),
            updated: false,
            complete: false,
        });
    }
    let latest = latest();
    let changed = version.installed != before;
    version.latest_error = latest.is_none();
    version.latest = latest.unwrap_or_default();
    version.checking_latest = false;
    version.busy = false;
    Ok((version, changed))
}
/// The newest published version, empty when the CLI checks for it itself (agy).
fn latest_version(tool: Tool, offline: bool) -> Option<String> {
    let package = match tool {
        Tool::Codex => "@openai/codex",
        Tool::Claude => "@anthropic-ai/claude-code",
        Tool::Antigravity => return Some(String::new()),
    };
    let mut cmd = Command::new("curl");
    kiln_common::process::hide_window(&mut cmd);
    cmd.args([
        "-q",
        "--fail",
        "--silent",
        "--connect-timeout",
        "3",
        "--max-time",
        "8",
        &format!("https://registry.npmjs.org/{package}/latest"),
    ]);
    if offline {
        Some(String::new())
    } else {
        bounded(cmd, Duration::from_secs(10))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .filter(|version| !version.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_launch_is_found_in_the_agent_run_command_line() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let launch = args(&[r"C:\Kiln\bin\kiln.exe", "agent-run", "--record", r"C:\cfg\agent-sessions\9-1.json", "--generation", "9-1", "--resume"]);
        assert_eq!(launch_in_command_line(&launch), Some((PathBuf::from(r"C:\cfg\agent-sessions\9-1.json"), "9-1".to_string())));
        assert_eq!(launch_in_command_line(&args(&["/usr/local/bin/kiln", "agent-run", "--record", "/c/agent-sessions/1-2.json", "--generation", "1-2"])).map(|l| l.1), Some("1-2".into()));
        assert_eq!(launch_in_command_line(&args(&["node.exe", "agent-run", "--record", "x", "--generation", "1"])), None);
        assert_eq!(launch_in_command_line(&args(&["kiln.exe", "agent-hook"])), None);
        assert_eq!(launch_in_command_line(&args(&["kiln.exe", "agent-run", "--record", "x"])), None);
    }

    #[test]
    fn hook_commands_follow_the_shell_each_cli_uses() {
        let helper = std::path::Path::new(if cfg!(windows) { r"C:\Program Files\Kiln\bin\kiln.exe" } else { "/Applications/Kiln.app/Contents/MacOS/kiln" });
        let claude = hook_command(Tool::Claude, helper);
        assert_eq!(claude, format!("'{}' agent-hook", helper.display()));
        let codex = hook_command(Tool::Codex, helper);
        if cfg!(windows) {
            // PowerShell (Codex's default) and cmd both run an unquoted plain path; other
            // paths use PowerShell's call operator.
            assert_eq!(codex, format!("& '{}' agent-hook", helper.display()));
            let plain = std::path::Path::new(r"C:\Kiln\bin\kiln.exe");
            assert_eq!(hook_command(Tool::Codex, plain), r"C:\Kiln\bin\kiln.exe agent-hook");
            let quote = std::path::Path::new(r"C:\Users\o'brien\kiln.exe");
            assert_eq!(hook_command(Tool::Codex, quote), r"& 'C:\Users\o''brien\kiln.exe' agent-hook");
        } else {
            assert_eq!(codex, claude);
        }
    }


    #[test]
    fn partial_version_replies_never_trigger_restart_and_completion_triggers_once() {
        let mut updates = Updates {
            last: Instant::now(),
            ..Default::default()
        };
        updates.observed[0] = "1.2.2".into();
        let version = Version {
            installed: "1.2.3".into(),
            busy: true,
            checking_latest: true,
            ..Default::default()
        };
        updates
            .tx
            .send(Reply {
                tool: Tool::Claude,
                result: Ok(version.clone()),
                updated: false,
                complete: false,
            })
            .unwrap();
        assert!(updates.tick(false).is_empty());
        assert_eq!(updates.versions[0].installed, "1.2.3");
        assert!(updates.versions[0].busy);
        updates
            .tx
            .send(Reply {
                tool: Tool::Claude,
                result: Ok(Version {
                    busy: false,
                    checking_latest: false,
                    ..version
                }),
                updated: false,
                complete: true,
            })
            .unwrap();
        assert_eq!(updates.tick(false), vec![Tool::Claude]);
        assert!(updates.tick(false).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn resume_preserves_a_verified_keyring_namespace_in_effective_cli_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("initialized"), b"1").unwrap();
        let binary = root.path().join("fixture-codex");
        fs::write(&binary, "#!/bin/sh\nfor arg do [ \"$arg\" = 'cli_auth_credentials_store=\"keyring\"' ] && exit 0; done\nexit 7\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("record.json");
        let record = Record {
            tool: Tool::Codex,
            spec: SpawnSpec {
                program: Some(binary.display().to_string()),
                env: vec![
                    ("CODEX_HOME".into(), root.path().display().to_string()),
                    ("KILN_AGENT_PROFILE".into(), "fixture-profile".into()),
                    ("KILN_CODEX_AUTH_STORE".into(), "keyring".into()),
                ],
                ..Default::default()
            },
            version: "1.2.3".into(),
            conversation: Some("12345678-1234-1234-1234-123456789abc".into()),
            generation: "fixture".into(),
            confirmed: true,
            request_started: false,
        };
        private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
        let spec = resume(&path).unwrap();
        assert!(!root.path().join("auth.json").exists());
        assert_eq!(
            codex_store_override(&spec),
            "cli_auth_credentials_store=\"keyring\""
        );
        let mut command = Command::new(&binary);
        command.args(&spec.args);
        bounded(command, Duration::from_secs(2)).unwrap();
        let mut legacy = record.spec;
        legacy.env.retain(|(key, _)| key != "KILN_CODEX_AUTH_STORE");
        assert_eq!(
            codex_store_override(&legacy),
            "cli_auth_credentials_store=\"file\""
        );
    }

    #[test]
    #[cfg(unix)]
    fn installed_version_arrives_before_catalog_and_is_probed_once() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("fixture-cli");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf x >> \"$0.calls\"\nprintf 'fixture 1.2.3\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let (tx, rx) = channel();
        let (release, held) = channel();
        let path = binary.clone();
        let worker = std::thread::spawn(move || {
            version_request(Tool::Claude, false, path, &tx, || {
                held.recv_timeout(Duration::from_secs(3)).unwrap();
                None // A failed catalog must preserve the known installed version.
            })
            .unwrap()
        });
        let interim = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!interim.complete && !interim.updated);
        let version = interim.result.unwrap();
        assert_eq!(version.installed, "1.2.3");
        assert!(version.busy && version.checking_latest);
        assert!(!worker.is_finished());
        release.send(()).unwrap();
        let (version, changed) = worker.join().unwrap();
        assert_eq!(version.installed, "1.2.3");
        assert!(!changed && !version.busy && version.latest_error);
        assert_eq!(fs::read(binary.with_extension("calls")).unwrap(), b"x");
    }

    #[test]
    fn cli_cards_fit_all_languages_and_keep_version_labels_separate() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT, Queryable},
        };
        use kiln_common::i18n::{Language, tr, with_language};
        for language in Language::ALL {
            with_language(language, || {
                let mut updates = Updates::default();
                updates.versions = std::array::from_fn(|_| Version {
                    installed: "1.2.3".into(),
                    latest: "1.2.4".into(),
                    binary: Some(PathBuf::from("/fixture/very/long/path/to/agent/bin/agent")),
                    ..Default::default()
                });
                let mut h = Harness::builder()
                    .with_size([420., 900.])
                    .wgpu()
                    .build_ui_state(
                        |ui, state: &mut (Updates, super::super::state::Settings)| {
                            let id = egui::Id::new("cli-card-test-fonts");
                            if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                                ui.ctx().data_mut(|d| d.insert_temp(id, true));
                                ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                                return;
                            }
                            kiln_common::Theme::current().apply(ui.ctx());
                            state.0.ui(ui, &mut state.1);
                        },
                        (updates, super::super::state::Settings::default()),
                    );
                h.ctx.set_zoom_factor(1.3);
                h.run_steps(4);
                for label in [tr("버전 확인"), tr("CLI 업데이트")] {
                    let buttons = h
                        .query_all_by_label(label)
                        .filter(|node| {
                            node.accesskit_node().role() == egui::accesskit::Role::Button
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(buttons.len(), Tool::ALL.len());
                    assert!(
                        buttons
                            .iter()
                            .all(|node| h.ctx.content_rect().contains_rect(node.rect())),
                        "{language:?}: {label} clipped"
                    );
                }
                h.render()
                    .unwrap()
                    .save(std::env::temp_dir().join(format!("kiln022-cli-cards-{language:?}.png")))
                    .unwrap();
            });
        }
    }
    #[test]
    fn a_missing_cli_reads_not_installed_and_offers_no_update() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT, Queryable},
        };
        use kiln_common::i18n::tr;
        let mut updates = Updates::default();
        updates.versions = std::array::from_fn(|_| Version { installed: "1.2.3".into(), ..Default::default() });
        updates.versions[index(Tool::Antigravity)] = Version { missing: true, ..Default::default() };
        let mut h = Harness::builder().with_size([420., 900.]).build_ui_state(
            |ui, state: &mut (Updates, super::super::state::Settings)| {
                let id = egui::Id::new("cli-card-test-fonts");
                if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                    ui.ctx().data_mut(|d| d.insert_temp(id, true));
                    ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                    return;
                }
                state.0.ui(ui, &mut state.1);
            },
            (updates, super::super::state::Settings::default()),
        );
        h.run_steps(4);
        h.get_by_label(tr("설치되지 않음"));
        assert!(h.query_by_label(tr("CLI 업데이트 실패. 설치 프로그램 또는 패키지 관리자를 확인하세요.")).is_none());
        let updates: Vec<_> = h
            .query_all_by_label(tr("CLI 업데이트"))
            .filter(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
            .map(|node| node.accesskit_node().is_disabled())
            .collect();
        assert_eq!(updates, vec![false, false, true]);
    }
    #[test]
    fn uuid_validation_rejects_options_and_shell_input() {
        assert!(valid_id("12345678-1234-1234-1234-123456789abc"));
        for id in [
            "--last",
            "$(pwd)",
            "abc",
            "12345678_1234-1234-1234-123456789abc",
        ] {
            assert!(!valid_id(id));
        }
    }
    #[test]
    fn hooks_track_each_panel_and_reject_stale_or_wrong_resume() {
        let d = tempfile::tempdir().unwrap();
        let id = "12345678-1234-1234-1234-123456789abc";
        let other = "22345678-1234-1234-1234-123456789abc";
        let make = |name: &str| {
            let path = d.path().join(name);
            let record = Record {
                tool: Tool::Codex,
                spec: SpawnSpec::default(),
                version: "1.0.0".into(),
                generation: name.into(),
                conversation: None,
                confirmed: false,
                request_started: false,
            };
            private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
            path
        };
        let a = make("a.json");
        let b = make("b.json");
        let event = |id: &str| {
            serde_json::json!({"hook_event_name":"SessionStart","session_id":id,"accessToken":"never-store","transcript_path":"never-store"}).to_string()
        };
        hook_in(d.path(), &a, "a.json", event(id).as_bytes()).unwrap();
        hook_in(d.path(), &b, "b.json", event(other).as_bytes()).unwrap();
        assert_eq!(read(&a).unwrap().conversation.as_deref(), Some(id));
        assert_eq!(read(&b).unwrap().conversation.as_deref(), Some(other));
        assert!(!fs::read_to_string(&a).unwrap().contains("never-store"));
        assert!(hook_in(d.path(), &a, "stale-generation", event(other).as_bytes()).is_err());
        let child=serde_json::json!({"hook_event_name":"SessionStart","session_id":other,"agent_id":"subagent"}).to_string();
        hook_in(d.path(), &a, "a.json", child.as_bytes()).unwrap();
        assert_eq!(read(&a).unwrap().conversation.as_deref(), Some(id));
        let mut record = read(&a).unwrap();
        record.confirmed = false;
        private_write(&a, &serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(hook_in(d.path(), &a, "a.json", event(other).as_bytes()).is_err());
        assert!(!read(&a).unwrap().confirmed);
        hook_in(d.path(), &a, "a.json", event(id).as_bytes()).unwrap();
        assert!(read(&a).unwrap().confirmed);
    }

    #[test]
    fn linked_resume_requests_handover_without_reclaiming_on_an_old_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().canonical().unwrap();
        let dir = config.join("agent-sessions");
        fs::create_dir_all(&dir).unwrap();
        let mut links = vec![];
        let mut records = vec![];
        for (i, tool) in [Tool::Claude, Tool::Codex].into_iter().enumerate() {
            let root = config.join(tool.key());
            fs::create_dir(&root).unwrap();
            let id = format!("{}2345678-1234-1234-1234-123456789abc", i + 1);
            let path = dir.join(format!("{}.json", tool.key()));
            let generation = format!("launch-{i}");
            let record = Record { tool, spec: SpawnSpec {
                cwd: Some(config.to_string_lossy().into_owned()),
                env: vec![(if tool == Tool::Claude { "CLAUDE_CONFIG_DIR" } else { "CODEX_HOME" }.into(), root.to_string_lossy().into_owned())],
                ..Default::default()
            }, version: String::new(), conversation: None, generation: generation.clone(), confirmed: false, request_started: false };
            private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
            links.push(serde_json::json!({"tool":tool,"root":root,"id":id,"group":"linked-fixture","canonical_id":id}));
            records.push((path, generation, id));
        }
        fs::create_dir_all(config.join("conversation-sync")).unwrap();
        fs::write(config.join("conversation-sync/native-links.json"), serde_json::to_vec(&links).unwrap()).unwrap();
        let event = |event: &str, id: &str| serde_json::json!({"hook_event_name":event,"session_id":id}).to_string();
        for (path, generation, id) in &records {
            hook_in(&dir, path, generation, event("SessionStart", id).as_bytes()).unwrap();
        }
        let claims_path = config.join("conversation-sync/owners.json");
        let before = fs::read(&claims_path).expect("resuming the linked peer must request an exclusive handover");
        let owners: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(owners["linked-fixture"]["record"], serde_json::json!(records[1].0));
        assert_eq!(owners["linked-fixture"]["ready"], false);
        let blocked = hook_in(&dir, &records[0].0, &records[0].1, event("UserPromptSubmit", &records[0].2).as_bytes()).unwrap().unwrap();
        assert_eq!(blocked["decision"], "block");
        assert_eq!(fs::read(&claims_path).unwrap(), before, "an old prompt must not steal the conversation back");
        let handover = super::super::conversation_sync::handover::owners(&config).unwrap();
        let sequence = handover["linked-fixture"].sequence;
        let mut winner = read(&records[1].0).unwrap();
        winner.generation = "reloaded".into();
        assert!(!super::super::conversation_sync::handover::pending_matches(&handover["linked-fixture"], &records[1].0, &winner));
        winner.confirmed = false;
        assert!(super::super::conversation_sync::handover::pending_matches(&handover["linked-fixture"], &records[1].0, &winner), "a renewed record must recover if ownership persistence failed before launch");
        private_write(&records[1].0, &serde_json::to_vec(&winner).unwrap()).unwrap();
        super::super::conversation_sync::handover::grant(&config, "linked-fixture", sequence, &records[1].0, &winner).unwrap();
        assert!(hook_in(&dir, &records[1].0, "reloaded", event("SessionStart", &records[1].2).as_bytes()).unwrap().is_none());
        assert!(hook_in(&dir, &records[1].0, "reloaded", event("UserPromptSubmit", &records[1].2).as_bytes()).unwrap().is_none());
        assert!(super::super::conversation_sync::handover::blocked(&config, &records[0].0, &read(&records[0].0).unwrap()));
        assert!(!super::super::conversation_sync::handover::blocked(&config, &records[1].0, &winner));
    }
    #[test]
    fn linked_conversation_without_a_transfer_accepts_prompts_and_compaction() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().canonical().unwrap();
        let dir = config.join("agent-sessions");
        fs::create_dir_all(&dir).unwrap();
        let root = config.join("claude");
        fs::create_dir(&root).unwrap();
        let id = "12345678-1234-1234-1234-123456789abc";
        let path = dir.join("claude.json");
        let record = Record { tool: Tool::Claude, spec: SpawnSpec {
            cwd: Some(config.to_string_lossy().into_owned()),
            env: vec![("CLAUDE_CONFIG_DIR".into(), root.to_string_lossy().into_owned())],
            ..Default::default()
        }, version: String::new(), conversation: Some(id.into()), generation: "launch".into(), confirmed: true, request_started: false };
        private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
        fs::create_dir_all(config.join("conversation-sync")).unwrap();
        let links = serde_json::json!([{"tool":Tool::Claude,"root":root,"id":id,"group":"linked-fixture","canonical_id":id}]);
        fs::write(config.join("conversation-sync/native-links.json"), serde_json::to_vec(&links).unwrap()).unwrap();
        let event = |name: &str, source: &str| serde_json::json!({"hook_event_name":name,"session_id":id,"source":source}).to_string();
        assert!(hook_in(&dir, &path, "launch", event("UserPromptSubmit", "").as_bytes()).unwrap().is_none(), "a published link alone is not a transfer");
        assert!(hook_in(&dir, &path, "launch", event("SessionStart", "compact").as_bytes()).unwrap().is_none());
        assert!(!config.join("conversation-sync/owners.json").exists(), "compaction in the same panel requests no transfer");
        assert!(hook_in(&dir, &path, "launch", event("UserPromptSubmit", "").as_bytes()).unwrap().is_none());
        assert!(hook_in(&dir, &path, "launch", event("SessionStart", "resume").as_bytes()).unwrap().is_none());
        assert!(config.join("conversation-sync/owners.json").exists(), "a resume still requests exclusive ownership");
        assert_eq!(hook_in(&dir, &path, "launch", event("UserPromptSubmit", "").as_bytes()).unwrap().unwrap()["decision"], "block");
    }
    #[test]
    fn unpinned_account_cannot_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("record.json");
        let record = Record {
            tool: Tool::Claude,
            spec: SpawnSpec::default(),
            version: "1.0.0".into(),
            generation: "f".into(),
            conversation: Some("12345678-1234-1234-1234-123456789abc".into()),
            confirmed: true,
            request_started: false,
        };
        private_write(&p, &serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(resume(&p).is_err());
    }
    #[test]
    fn hook_definition_remains_stable_across_generations() {
        let d = tempfile::tempdir().unwrap();
        let base = Record {
            tool: Tool::Codex,
            spec: SpawnSpec::default(),
            version: "1.0.0".into(),
            generation: "old".into(),
            conversation: None,
            confirmed: false,
            request_started: false,
        };
        let mut next = base.clone();
        next.generation = "new".into();
        let mut a = SpawnSpec::default();
        let mut b = SpawnSpec::default();
        attach(&mut a, &d.path().join("a.json"), &base).unwrap();
        attach(&mut b, &d.path().join("b.json"), &next).unwrap();
        assert_eq!(a.args, b.args);
        assert_ne!(a.env, b.env);
        assert!(!a.args.iter().any(|a| a.contains("dangerously-bypass")));
    }
    #[test]
    fn unsupported_installation_never_invokes_shared_updater() {
        let d = tempfile::tempdir().unwrap();
        let binary = d.path().join("custom-codex");
        fs::write(&binary, b"fixture").unwrap();
        assert!(update_command(Tool::Codex, &binary).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn installers_reject_another_package_manager_prefix() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("a/lib/node_modules");
        let package = root.join("@openai/codex/bin");
        fs::create_dir_all(&package).unwrap();
        let binary = package.join("codex");
        fs::write(&binary, b"fixture").unwrap();
        let other = d.path().join("b/lib/node_modules/@openai/codex");
        fs::create_dir_all(&other).unwrap();
        let npm = d.path().join("npm");
        let script = |path: &Path, answer: &Path| {
            fs::write(
                path,
                format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", answer.display()),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        };
        script(&npm, other.parent().unwrap().parent().unwrap());
        assert!(installer_with(&binary, |_| Some(npm.clone())).is_err());
        script(&npm, &root);
        assert_eq!(
            installer_with(&binary, |_| Some(npm.clone())).unwrap(),
            Installer::Npm(npm)
        );
        let prefix = d.path().join("brew-a");
        let cask = prefix.join("Caskroom/codex/1.0/bin");
        fs::create_dir_all(&cask).unwrap();
        let binary = cask.join("codex");
        fs::write(&binary, b"fixture").unwrap();
        let brew = d.path().join("brew");
        script(&brew, d.path());
        assert!(installer_with(&binary, |_| Some(brew.clone())).is_err());
        script(&brew, &prefix);
        assert_eq!(
            installer_with(&binary, |_| Some(brew.clone())).unwrap(),
            Installer::Brew(brew, true)
        );
    }
    #[cfg(windows)]
    #[test]
    fn a_contained_agent_reports_its_exit_code() {
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "exit 3"]);
        assert_eq!(run_contained(cmd).unwrap(), 3);
    }
    #[cfg(windows)]
    #[test]
    fn npm_installation_is_found_from_the_windows_launcher() {
        let d = tempfile::tempdir().unwrap();
        let prefix = d.path().join("npm");
        let package = prefix.join("node_modules").join("@openai").join("codex");
        fs::create_dir_all(package.join("vendor")).unwrap();
        let launcher = prefix.join("codex.cmd");
        fs::write(&launcher, b"@echo off\r\n").unwrap();
        let native = package.join("vendor").join("codex.exe");
        fs::write(&native, b"fixture").unwrap();
        let npm = |name: &str, root: &Path| {
            let path = d.path().join(name);
            fs::write(&path, format!("@echo {}\r\n", root.display())).unwrap();
            path
        };
        let other = npm("other-npm.cmd", &d.path().join("elsewhere"));
        assert!(installer_with(&launcher, |_| Some(other.clone())).is_err());
        let owner = npm("npm.cmd", &prefix.join("node_modules"));
        assert_eq!(installer_with(&launcher, |_| Some(owner.clone())).unwrap(), Installer::Npm(owner.clone()));
        assert_eq!(installer_with(&native, |_| Some(owner.clone())).unwrap(), Installer::Npm(owner));
    }
    #[test]
    fn update_and_immediate_restart_options_are_independent_and_persist() {
        use egui_kittest::{Harness, kittest::Queryable};
        use kiln_common::i18n::tr;
        let mut h = Harness::builder().with_size([680., 480.]).build_ui_state(
            |ui, state: &mut (Updates, super::super::state::Settings)| {
                let id = egui::Id::new("cli-option-test-fonts");
                if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                    ui.ctx().data_mut(|d| d.insert_temp(id, true));
                    ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                    return;
                }
                state.0.ui(ui, &mut state.1)
            },
            (Updates::default(), super::super::state::Settings::default()),
        );
        h.run_steps(2);
        assert!(!h.state().1.cli_auto_update && !h.state().1.cli_restart_after_update);
        h.get_by_label(tr("업데이트 직후 같은 대화로 세션 재시작"))
            .click();
        h.run_steps(2);
        assert!(h.state().1.cli_restart_after_update && !h.state().1.cli_auto_update);
        h.get_by_label(tr("Codex·Claude Code·Antigravity 자동 업데이트"))
            .click();
        h.run_steps(2);
        let saved = serde_json::to_vec(&h.state().1).unwrap();
        let restored: super::super::state::Settings = serde_json::from_slice(&saved).unwrap();
        assert!(restored.cli_auto_update && restored.cli_restart_after_update);
    }
    #[test]
    fn restored_task_never_replays_a_previous_request() {
        let mut record = Record {
            tool: Tool::Codex,
            spec: SpawnSpec::default(),
            version: String::new(),
            conversation: None,
            generation: "fixture".into(),
            confirmed: false,
            request_started: false,
        };
        assert!(!request_mode(&record, true, false).unwrap());
        record.request_started = true;
        assert!(request_mode(&record, true, false).is_err());
        record.conversation = Some("12345678-1234-1234-1234-123456789abc".into());
        assert!(request_mode(&record, true, false).unwrap());
        record.request_started = false;
        assert!(request_mode(&record, true, false).unwrap());
    }
    #[test]
    fn version_parses_real_formats() {
        assert_eq!(parse_version(b"codex-cli 0.162.1\n").unwrap(), "0.162.1");
        assert_eq!(
            parse_version(b"2.1.296 (Claude Code)\n").unwrap(),
            "2.1.296"
        );
        assert!(parse_version(b"broken").is_err());
    }
    #[test]
    fn exact_resume_preserves_account_and_cwd() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("record.json");
        fs::write(d.path().join("initialized"), b"1").unwrap();
        for tool in Tool::ALL {
            let record = Record {
                tool,
                spec: SpawnSpec {
                    program: Some("/fixture/cli".into()),
                    cwd: Some("/same directory".into()),
                    env: vec![
                        (
                            match tool { Tool::Claude => "CLAUDE_CONFIG_DIR", Tool::Codex => "CODEX_HOME", Tool::Antigravity => "HOME" }.into(),
                            d.path().to_string_lossy().into_owned(),
                        ),
                        ("KILN_AGENT_PROFILE".into(), "account-a".into()),
                    ],
                    ..Default::default()
                },
                conversation: Some("12345678-1234-1234-1234-123456789abc".into()),
                version: "1.2.3".into(),
                generation: "fixture".into(),
                confirmed: true,
                request_started: false,
            };
            private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
            let spec = resume(&path).unwrap();
            assert_eq!(spec.cwd, record.spec.cwd);
            assert!(spec.env.contains(&record.spec.env[0]));
            assert!(spec.args.contains(record.conversation.as_ref().unwrap()));
            assert!(!spec.args.iter().any(|s| s == "--last" || s == "--continue"));
        }
    }
    #[test]
    fn agy_resumes_its_exact_conversation_without_an_account() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("record.json");
        let id = "12345678-1234-1234-1234-123456789abc";
        let record = Record {
            tool: Tool::Antigravity,
            spec: SpawnSpec { program: Some("/fixture/agy".into()), cwd: Some("/same directory".into()), ..Default::default() },
            conversation: Some(id.into()),
            version: "1.4.5".into(),
            generation: "fixture".into(),
            confirmed: true,
            request_started: false,
        };
        private_write(&path, &serde_json::to_vec(&record).unwrap()).unwrap();
        let spec = resume(&path).unwrap();
        assert_eq!(spec.args, vec!["--conversation".to_string(), id.into()]);
        assert_eq!(spec.cwd, record.spec.cwd);
        assert!(!spec.env.iter().any(|(k, _)| k == "KILN_AGENT_PROFILE"));
        let claude = Record { tool: Tool::Claude, ..record };
        private_write(&path, &serde_json::to_vec(&claude).unwrap()).unwrap();
        assert!(resume(&path).is_err(), "account tools still need their pinned account");
    }
    #[test]
    fn an_agy_launch_records_only_the_conversation_it_holds_open() {
        let root = Path::new("/agy");
        let (id, other) = ("12345678-1234-1234-1234-123456789abc", "22345678-1234-1234-1234-123456789abc");
        let lock = |id: &str| root.join("presence").join(format!("{id}.lock"));
        assert_eq!(held_conversation(root, &[root.join("conversations").join(format!("{id}.db")), lock(id)]), Some(id.into()));
        assert_eq!(held_conversation(root, &[lock(id), lock(id)]), Some(id.into()));
        assert_eq!(held_conversation(root, &[lock(id), lock(other)]), None);
        assert_eq!(held_conversation(root, &[root.join("presence/fixture.lock"), Path::new("/other/presence").join(format!("{id}.lock"))]), None);
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("record.json");
        let fresh = Record {
            tool: Tool::Antigravity,
            spec: SpawnSpec::default(),
            conversation: None,
            version: "1.4.5".into(),
            generation: "launch".into(),
            confirmed: false,
            request_started: false,
        };
        private_write(&path, &serde_json::to_vec(&fresh).unwrap()).unwrap();
        assert!(observe(&path, "stale", id).is_err());
        observe(&path, "launch", id).unwrap();
        let record = read(&path).unwrap();
        assert!(record.confirmed && record.conversation.as_deref() == Some(id));
        // A new conversation in the same panel replaces a confirmed one.
        observe(&path, "launch", other).unwrap();
        assert_eq!(read(&path).unwrap().conversation.as_deref(), Some(other));
        // A resumed launch confirms only its original conversation.
        let resumed = Record { conversation: Some(id.into()), ..fresh.clone() };
        private_write(&path, &serde_json::to_vec(&resumed).unwrap()).unwrap();
        assert!(observe(&path, "launch", other).is_err());
        assert!(!read(&path).unwrap().confirmed);
        observe(&path, "launch", id).unwrap();
        assert!(read(&path).unwrap().confirmed);
        let codex = Record { tool: Tool::Codex, ..fresh };
        private_write(&path, &serde_json::to_vec(&codex).unwrap()).unwrap();
        assert!(observe(&path, "launch", id).is_err(), "hooks confirm the other CLIs");
        let spec = SpawnSpec { args: ["agent-run", "--record", "/r.json", "--generation", "g-1"].map(String::from).to_vec(), ..Default::default() };
        assert_eq!(launch_of(&spec), Some((PathBuf::from("/r.json"), "g-1".into())));
        assert_eq!(launch_of(&SpawnSpec::default()), None);
    }
    #[test]
    fn missing_identity_cannot_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("r.json");
        let record = Record {
            tool: Tool::Codex,
            spec: SpawnSpec::default(),
            conversation: None,
            version: "1.0.0".into(),
            generation: "f".into(),
            confirmed: false,
            request_started: false,
        };
        private_write(&p, &serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(resume(&p).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn bounded_command_handles_failure_and_timeout() {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "exit 2"]);
        assert!(bounded(c, Duration::from_secs(1)).is_err());
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 20"]);
        let t = Instant::now();
        assert!(bounded(c, Duration::from_millis(80)).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
