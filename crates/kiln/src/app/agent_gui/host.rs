//! Persistent worker. Pipe/JSON/command I/O happens here, never on the GUI thread.
use super::providers::{Adapter, Effects};
use super::*;
use journal::Journal;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command as Process, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
#[path = "presentation.rs"]
mod presentation;
static TERMINATE: AtomicBool = AtomicBool::new(false);
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static CONSOLE_CONTROL_READY: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn signal(signal: libc::c_int) {
    // Interactive Ctrl-C belongs to the native CLI while it owns this PTY.
    if signal == libc::SIGINT && TERMINAL_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    TERMINATE.store(true, Ordering::Relaxed);
}
#[cfg(unix)]
fn handlers() {
    // SAFETY: handler only writes a lock-free atomic, never allocates or touches the journal.
    unsafe {
        for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
            libc::signal(sig, signal as *const () as usize);
        }
    }
}
#[cfg(windows)]
unsafe extern "system" fn console_control(event: u32) -> windows_sys::core::BOOL {
    if event == windows_sys::Win32::System::Console::CTRL_C_EVENT && TERMINAL_ACTIVE.load(Ordering::Relaxed) { 1 } else { 0 }
}
#[cfg(windows)]
fn handlers() {
    // Only suppress this host's default Ctrl-C exit while the native child
    // owns input. Each provider keeps its own console control handler.
    let registered = unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_control), 1) } != 0;
    CONSOLE_CONTROL_READY.store(registered, Ordering::Relaxed);
}
#[cfg(not(any(unix, windows)))]
fn handlers() {}
#[cfg(unix)]
struct OwnedGroup(i32);
#[cfg(unix)]
impl OwnedGroup {
    fn current() -> Result<Self> {
        // SAFETY: read-only process identity queries.
        let (pid, group, session) = unsafe { (libc::getpid(), libc::getpgrp(), libc::getsid(0)) };
        ensure!(
            pid > 1 && pid == group && pid == session,
            "GUI agent host must own its PTY session process group"
        );
        Ok(Self(pid))
    }
    fn owns(&self) -> bool {
        unsafe {
            libc::getpid() == self.0 && libc::getpgrp() == self.0 && libc::getsid(0) == self.0
        }
    }
    fn children(&self) -> Vec<i32> {
        self.inspect_children().unwrap_or_default()
    }
    fn inspect_children(&self) -> Result<Vec<i32>> {
        ensure!(self.owns(), "PTY process-group ownership is unconfirmed");
        #[cfg(target_os = "macos")]
        let pids = {
            let mut pids = vec![0i32; 65536];
            let size = (pids.len() * std::mem::size_of::<i32>()) as i32;
            // SAFETY: buffer is sized for `size`; PROC_PGRP_ONLY=2 is a read-only group query.
            let bytes =
                unsafe { libc::proc_listpids(2, self.0 as u32, pids.as_mut_ptr().cast(), size) };
            ensure!(bytes > 0 && bytes < size, "Could not inspect the complete PTY process group");
            pids.truncate(bytes.max(0) as usize / std::mem::size_of::<i32>());
            pids
        };
        #[cfg(not(target_os = "macos"))]
        let pids = fs::read_dir("/proc")?
            .filter_map(|entry|entry.ok())
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .collect::<Vec<_>>();
        Ok(pids.into_iter()
            .filter(|pid| {
                *pid != self.0
                    && *pid > 1
                    && unsafe { libc::getpgid(*pid) == self.0 && libc::getsid(*pid) == self.0 }
            })
            .collect())
    }
    fn backend_is_alone(&self, backend: u32) -> bool {
        self.inspect_children().is_ok_and(|children|children.len() == 1 && children[0] == backend as i32)
    }
    fn handoff_children_exited(&self) -> bool {
        self.inspect_children().is_ok_and(|children|children.is_empty())
    }
    fn cleanup_children(&self) {
        for sig in [libc::SIGTERM, libc::SIGKILL] {
            for pid in self.children() {
                if self.owns()
                    && unsafe { libc::getpgid(pid) == self.0 && libc::getsid(pid) == self.0 }
                {
                    unsafe {
                        libc::kill(pid, sig);
                    }
                }
            }
            if sig == libc::SIGTERM {
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    fn terminate(&self, journal: &mut Journal) -> Result<()> {
        let persisted = journal.push(Event::Lifecycle(Lifecycle::Stopped));
        let flushed = journal.flush();
        if self.owns() {
            // The handler keeps the leader alive until durable state is flushed and descendants
            // have received TERM. KILL only our still-owned session group, including this worker.
            unsafe {
                libc::kill(-self.0, libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(200));
            if self.owns() {
                unsafe {
                    libc::kill(-self.0, libc::SIGKILL);
                }
            }
        }
        persisted?;
        flushed?;
        Ok(())
    }
}
#[cfg(not(unix))]
struct OwnedGroup;
#[cfg(not(unix))]
impl OwnedGroup {
    fn current() -> Result<Self> {
        Ok(Self)
    }
    fn cleanup_children(&self) {}
    #[cfg(windows)]
    fn inspect_children(&self) -> Result<Vec<u32>> {
        use windows_sys::Win32::System::JobObjects::{JobObjectBasicProcessIdList, QueryInformationJobObject};
        ensure!(CONSOLE_CONTROL_READY.load(Ordering::Relaxed), "Native terminal Ctrl-C protection is unavailable");
        #[repr(C)]
        struct ProcessList { assigned: u32, listed: u32, pids: [usize; 4096] }
        let mut list = ProcessList { assigned: 0, listed: 0, pids: [0; 4096] };
        // NULL selects this host's current per-PTY job, assigned by the daemon.
        // A missing job, truncated list or unlisted host fails closed.
        let listed = unsafe { QueryInformationJobObject(std::ptr::null_mut(), JobObjectBasicProcessIdList, (&mut list as *mut ProcessList).cast(), std::mem::size_of::<ProcessList>() as u32, std::ptr::null_mut()) };
        ensure!(listed != 0 && list.assigned == list.listed && list.listed as usize <= list.pids.len(), "Could not inspect the complete owned terminal job");
        let pids = &list.pids[..list.listed as usize];
        let own = std::process::id() as usize;
        ensure!(pids.contains(&own), "Terminal host is absent from its job");
        Ok(pids.iter().copied().filter(|pid|*pid != own).map(|pid|pid as u32).collect())
    }
    fn backend_is_alone(&self, backend: u32) -> bool {
        #[cfg(windows)]
        return self.inspect_children().is_ok_and(|children|children == [backend]);
        #[cfg(not(windows))]
        { let _ = backend; false }
    }
    fn handoff_children_exited(&self) -> bool {
        #[cfg(windows)]
        return self.inspect_children().is_ok_and(|children|children.is_empty());
        #[cfg(not(windows))]
        false
    }
    fn terminate(&self, j: &mut Journal) -> Result<()> {
        j.push(Event::Lifecycle(Lifecycle::Stopped))?;
        j.flush()
    }
}

fn claim_command(journal: &mut Journal, q: &QueuedCommand) -> Result<bool> {
    if journal.transcript.commands.contains_key(&q.id) {
        return Ok(false);
    }
    if q.generation != journal.generation {
        journal.push(Event::CommandState {
            id: q.id.clone(),
            state: CommandPhase::Rejected,
        })?;
        return Ok(false);
    }
    if let Command::Respond { request_id, .. } = &q.command
        && !journal.transcript.requests.contains_key(request_id)
    {
        journal.push(Event::CommandState {
            id: q.id.clone(),
            state: CommandPhase::Rejected,
        })?;
        return Ok(false);
    }
    journal.push(Event::CommandState {
        id: q.id.clone(),
        state: CommandPhase::Dispatched,
    })?;
    Ok(true)
}
fn claim_initial(dir: &Path) -> Result<bool> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(dir.join("initial-intent")) {
        Ok(mut f) => {
            f.write_all(b"attempted; never automatically replay\n")?;
            f.sync_all()?;
            journal::sync_directory(dir)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.into()),
    }
}

enum Input {
    Json(Value),
    Eof,
    Error(&'static str),
    WriteError,
}
struct Backend {
    child: Child,
    out: crossbeam_channel::Sender<Value>,
    input: crossbeam_channel::Receiver<Input>,
    draining: Option<(Instant, String)>,
}
impl Backend {
    fn spawn(mut command: Process) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        kiln_common::process::hide_window(&mut command);
        let mut child = command.spawn().context("could not start agent CLI")?;
        let (tx, rx) = crossbeam_channel::bounded(128);
        let (out, write) = crossbeam_channel::bounded::<Value>(16);
        let stdout = child.stdout.take().context("missing agent output")?;
        let mut stdin = child.stdin.take().context("missing agent input")?;
        let read_tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match reader
                    .by_ref()
                    .take((journal::MAX_RECORD + 2) as u64)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) => {
                        let _ = read_tx.send(Input::Eof);
                        break;
                    }
                    Ok(n) if n > journal::MAX_RECORD + 1 => {
                        let _ = read_tx.send(Input::Error("agent output exceeded the frame limit"));
                        break;
                    }
                    Ok(_) => {
                        if bytes.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        match serde_json::from_slice(&bytes) {
                            Ok(value) => {
                                if read_tx.send(Input::Json(value)).is_err() {
                                    break;
                                }
                            }
                            Err(_) => {
                                let _ = read_tx.send(Input::Error("agent emitted invalid JSON"));
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        let _ = read_tx.send(Input::Error("agent output pipe failed"));
                        break;
                    }
                }
            }
        });
        std::thread::spawn(move || {
            for value in write {
                if serde_json::to_writer(&mut stdin, &value).is_err()
                    || stdin.write_all(b"\n").is_err()
                    || stdin.flush().is_err()
                {
                    let _ = tx.send(Input::WriteError);
                    break;
                }
            }
        });
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut buf = [0; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            child,
            out,
            input: rx,
            draining: None,
        })
    }
    fn begin_drain(&mut self, reason: String) {
        self.draining
            .get_or_insert_with(|| (Instant::now(), reason));
    }
    fn poll_exit(&mut self) -> Result<()> {
        if let Some(status) = self.child.try_wait()? {
            self.begin_drain(format!("agent CLI exited ({status})"));
        }
        Ok(())
    }
    fn effects(
        &mut self,
        journal: &mut Journal,
        effects: Effects,
        active: &mut Option<String>,
    ) -> Result<()> {
        for event in effects.events {
            if let Event::Lifecycle(state) = &event {
                let state = match state {
                    Lifecycle::Running => "running",
                    Lifecycle::Waiting => "waiting",
                    Lifecycle::Ready => "done",
                    Lifecycle::Disconnected | Lifecycle::Failed => "failed",
                    _ => "unknown",
                };
                let mut out = std::io::stdout().lock();
                let _ = write!(out, "\x1b]777;kiln-agent;{state}\x07");
                let _ = out.flush();
            }
            journal.push(event)?;
        }
        if effects.cancelled
            && let Some(id) = active.take()
        {
            journal.push(Event::CommandState {
                id,
                state: CommandPhase::Rejected,
            })?;
        }
        if effects.accepted
            && let Some(id) = active.take()
        {
            journal.push(Event::CommandState {
                id,
                state: CommandPhase::Accepted,
            })?;
        }
        for value in effects.writes {
            if self.draining.is_some() {
                break;
            }
            if self.out.try_send(value).is_err() {
                self.begin_drain("agent input queue is full or disconnected".into());
                break;
            }
        }
        Ok(())
    }
    fn stop(&mut self, group: &OwnedGroup) {
        let _ = self.child.kill();
        group.cleanup_children();
        let _ = self.child.wait();
    }
    fn stop_for_handoff(&mut self) {
        // The idle provider is the only process proven safe to stop. Never
        // signal its process group: a background build/server must survive.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn command(config: &Config, adapter: &Adapter, accounts: &AccountManager) -> Result<Process> {
    let mut command = base_command(config, accounts)?;
    command.args(adapter.arguments());
    if !adapter.is_resume() {
        config.launch_options.validate(config.descriptor.tool)?;
        command.args(config.launch_options.arguments());
    }
    Ok(command)
}
fn base_command(config: &Config, accounts: &AccountManager) -> Result<Process> {
    // Only validate the original profile. Never call launch_environment or active here.
    if config
        .environment
        .iter()
        .any(|(key, _)| key == "KILN_AGENT_PROFILE")
    {
        accounts.validate_launch_environment(config.descriptor.tool, &config.environment)?;
    }
    let (program, lead) = super::super::agent_launch::launcher(&config.program);
    let mut command = Process::new(program);
    command
        .args(lead)
        .envs(config.environment.iter().cloned())
        .current_dir(&config.descriptor.cwd);
    if config.descriptor.tool == Tool::Codex
        && config
            .environment
            .iter()
            .any(|(key, _)| key == "KILN_AGENT_PROFILE")
    {
        let store = if config
            .environment
            .iter()
            .any(|(k, v)| k == "KILN_CODEX_AUTH_STORE" && v == "keyring")
        {
            "keyring"
        } else {
            "file"
        };
        command.args(["-c", &format!("cli_auth_credentials_store=\"{store}\"")]);
    }
    // The only resolved connection values exist in this final Command's environment.
    if let Some(launch) = accounts.connection_launch(config.descriptor.tool, &config.environment)? {
        for key in launch.remove_env {
            command.env_remove(key);
        }
        command.args(launch.args).envs(launch.env);
    }
    Ok(command)
}
fn mailbox(dir: &Path) -> Result<Vec<(PathBuf, QueuedCommand)>> {
    let queue = dir.join("commands");
    journal::private_dir(&queue)?;
    let mut paths = fs::read_dir(&queue)?
        .take(257)
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(paths.len() <= 256, "agent command queue is full");
    paths.sort();
    paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|path| {
            let command = journal::read(&path)?;
            Ok((path, command))
        })
        .collect()
}
fn resume_target(transcript: &Transcript) -> Result<Option<String>> {
    if let Some(id) = &transcript.provider_session_id {
        ensure!(
            !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control),
            "invalid native conversation ID"
        );
        return Ok(Some(id.clone()));
    }
    ensure!(
        !transcript
            .entries
            .iter()
            .any(|entry| matches!(entry,Entry::Message(m)if m.role==Role::User)),
        "The previous submission has no confirmed native conversation ID. Its outcome is unconfirmed; open a new conversation to retry explicitly."
    );
    Ok(None)
}
fn user_event(id: &str, c: &Command) -> Option<Event> {
    if let Command::Submit {
        text, attachments, ..
    }
    | Command::SubmitWithSkills {
        text, attachments, ..
    } = c
    {
        Some(Event::Message {
            message: Message {
                id: format!("user-{id}"),
                role: Role::User,
                text: text.clone(),
                attachments: attachments.clone(),
            },
            append: false,
        })
    } else {
        None
    }
}
fn dispatch(
    backend: &mut Backend,
    adapter: &mut Adapter,
    j: &mut Journal,
    q: QueuedCommand,
    runtime_id: &str,
    active: &mut Option<String>,
) -> Result<()> {
    if !claim_command(j, &q)? {
        return Ok(());
    }
    let validation = if let Command::Submit {
        text, attachments, ..
    }
    | Command::SubmitWithSkills {
        text, attachments, ..
    } = &q.command
    {
        if text.len() > 120_000
            || text.contains('\0')
            || (text.trim().is_empty() && attachments.is_empty())
        {
            Err(anyhow::anyhow!("invalid or oversized agent prompt"))
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    match validation.and_then(|()| adapter.command(&q.command, runtime_id)) {
        Ok(effects) => {
            if let Some(event) = user_event(&q.id, &q.command) {
                j.push(event)?;
                *active = Some(q.id.clone());
            }
            backend.effects(j, effects, active)?;
            if !matches!(
                q.command,
                Command::Submit { .. } | Command::SubmitWithSkills { .. }
            ) {
                j.push(Event::CommandState {
                    id: q.id,
                    state: if backend.draining.is_some() {
                        CommandPhase::Unconfirmed
                    } else {
                        CommandPhase::Accepted
                    },
                })?;
            }
        }
        Err(error) => {
            j.push(Event::CommandState {
                id: q.id,
                state: CommandPhase::Rejected,
            })?;
            j.push(Event::Error(error.to_string()))?;
        }
    }
    Ok(())
}
/// Hidden CLI entry. The daemon's ordinary PTY/Job Object remains the lifecycle owner.
pub fn serve(id: &str) -> Result<()> {
    let dir = journal::directory(id)?;
    journal::private_dir(&dir)?;
    let lock = journal::open(&dir.join("host.lock"), false)?;
    // A client health probe briefly tests this same OS lock. Allow that probe to
    // finish without mistaking it for a second long-lived worker on restart.
    let lock_deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(error) if Instant::now() >= lock_deadline => {
                return Err(error)
                    .context("another GUI agent worker already owns this conversation");
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let config: Config = journal::read(&dir.join("config.json"))?;
    ensure!(config.descriptor.id == id, "agent config identity mismatch");
    let mut journal = Journal::open(&dir, journal::unique())?;
    journal.push(Event::Generation)?;
    let report_path = dir.join("extension-report.json");
    if report_path.exists() {
        let report = kiln_accounts::extensions::ExtensionReport::read_prepared(&report_path)
            .unwrap_or_else(|_| kiln_accounts::extensions::ExtensionReport {
                entries: vec![kiln_accounts::extensions::ExtensionStatus {
                    kind: "extensions".into(), name: "Prepared extension status".into(),
                    state: "failed".into(), reason: "PREPARED_REPORT_INVALID".into(),
                }],
            });
        for (index, entry) in report.entries.into_iter().enumerate() {
            journal.push(Event::Controls(ControlEvent::Status(NativeStatus {
                subject: format!("profile_extension:{index}"),
                fields: [
                    ("kind".into(), ControlValue::Text(entry.kind)),
                    ("name".into(), ControlValue::Text(entry.name)),
                    ("state".into(), ControlValue::Text(entry.state)),
                    ("reason".into(), ControlValue::Text(entry.reason)),
                ].into(),
                unavailable: None,
            })))?;
        }
    }
    let group = OwnedGroup::current()?;
    handlers();
    let accounts = AccountManager::with_env(kiln_accounts::Env::real());
    loop {
        let result = (|| -> Result<bool> {
            if journal.transcript.presentation == Presentation::Terminal {
                presentation::serve_terminal(&config, &accounts, &dir, &group, &mut journal)?;
                if TERMINATE.load(Ordering::Relaxed) { return Ok(false); }
            }
            let resume = resume_target(&journal.transcript)?;
            let mut adapter = Adapter::new(
                config.descriptor.tool,
                config.descriptor.cwd.clone(),
                resume,
            );
            if !adapter.is_resume() {
                adapter.launch_options(config.launch_options.clone());
                journal.push(Event::Controls(ControlEvent::Status(
                    config.launch_options.metadata(),
                )))?;
            }
            let mut backend = Backend::spawn(command(&config, &adapter, &accounts)?)?;
            let mut active = None;
            backend.effects(&mut journal, adapter.start(), &mut active)?;
            let started = Instant::now();
            let mut initialized = false;
            let mut terminal_handoff = false;
            let outcome = (|| -> Result<()> {
                loop {
                    if TERMINATE.load(Ordering::Relaxed) {
                        group.terminate(&mut journal)?;
                        return Ok(());
                    }
                    backend.poll_exit()?;
                    let mut batch = adapter.tick();
                    for input in backend.input.try_iter().take(128).collect::<Vec<_>>() {
                        match input {
                            Input::Json(value) => match adapter.ingest(value) {
                                Ok(effects) => batch.merge(effects),
                                Err(error) => {
                                    backend.begin_drain(error.to_string());
                                    backend.effects(&mut journal, batch, &mut active)?;
                                    return Err(error);
                                }
                            },
                            Input::Eof => {
                                backend.begin_drain("agent output closed".into());
                                backend.effects(&mut journal, batch, &mut active)?;
                                anyhow::bail!(
                                    "agent output closed; the conversation is disconnected"
                                );
                            }
                            Input::Error(message) => {
                                backend.begin_drain(message.into());
                                backend.effects(&mut journal, batch, &mut active)?;
                                anyhow::bail!(message);
                            }
                            Input::WriteError => {
                                backend.begin_drain("agent input pipe failed".into());
                            }
                        }
                    }
                    backend.effects(&mut journal, batch, &mut active)?;
                    backend.poll_exit()?;
                    if let Some((since, reason)) = &backend.draining {
                        ensure!(
                            since.elapsed() < Duration::from_secs(2),
                            "{reason}; agent output drain timed out"
                        );
                        std::thread::sleep(Duration::from_millis(15));
                        continue;
                    }
                    if matches!(journal.transcript.lifecycle, Lifecycle::Ready) {
                        initialized = true;
                    }
                    ensure!(
                        initialized || started.elapsed() < Duration::from_secs(60),
                        "agent initialization timed out"
                    );
                    if initialized
                        && config
                            .initial_prompt
                            .as_ref()
                            .is_some_and(|p| !p.trim().is_empty())
                        && claim_initial(&dir)?
                    {
                        let q = QueuedCommand {
                            id: journal::unique(),
                            generation: journal.generation.clone(),
                            command: Command::Submit {
                                text: config.initial_prompt.clone().unwrap(),
                                attachments: vec![],
                                model: None,
                            },
                        };
                        dispatch(&mut backend, &mut adapter, &mut journal, q, id, &mut active)?;
                    }
                    for (path, q) in mailbox(&dir)? {
                        if backend.draining.is_some() {
                            break;
                        }
                        if let Command::SetPresentation(target) = q.command {
                            if claim_command(&mut journal, &q)? {
                                let handoff = target == Presentation::Terminal
                                    && presentation::can_handoff(&journal.transcript, active.is_some())
                                    && group.backend_is_alone(backend.child.id())
                                    && mailbox(&dir)?.len() == 1;
                                let same = target == Presentation::Gui;
                                journal.push(Event::CommandState {
                                    id: q.id,
                                    state: if handoff || same { CommandPhase::Accepted } else { CommandPhase::Rejected },
                                })?;
                                if handoff {
                                    terminal_handoff = true;
                                } else if !same {
                                    journal.push(Event::Error("Finish the current response, approvals, queued work and background processes before switching to the terminal. A confirmed native conversation and exclusive process ownership are required.".into()))?;
                                }
                            }
                            fs::remove_file(path)?;
                            if terminal_handoff { return Ok(()); }
                            continue;
                        }
                        if matches!(q.command, Command::Restart) {
                            journal.push(Event::CommandState {
                                id: q.id,
                                state: CommandPhase::Rejected,
                            })?;
                            fs::remove_file(path)?;
                            continue;
                        }
                        dispatch(&mut backend, &mut adapter, &mut journal, q, id, &mut active)?;
                        fs::remove_file(path)?;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            })();
            // Flush before cleaning up even on JSON, EOF, journal or write failures.
            if let Some(id) = active.take() {
                let _ = journal.push(Event::CommandState {
                    id,
                    state: CommandPhase::Unconfirmed,
                });
            }
            let _ = journal.flush();
            if terminal_handoff {
                backend.stop_for_handoff();
                ensure!(group.handoff_children_exited(), "Background processes appeared during handoff; they were left running. The native terminal was not started.");
            } else { backend.stop(&group); }
            outcome?;
            if terminal_handoff {
                journal.push(Event::Presentation { current: Presentation::Terminal, return_requested: false })?;
                journal.flush()?;
            }
            Ok(terminal_handoff)
        })();
        if TERMINATE.load(Ordering::Relaxed) {
            group.terminate(&mut journal)?;
            return Ok(());
        }
        if let Err(error) = result {
            journal.push(Event::Error(error.to_string()))?;
            journal.push(Event::Lifecycle(Lifecycle::Disconnected))?;
            journal.flush()?;
        } else if matches!(result, Ok(true)) {
            continue;
        }
        // A disconnected worker remains available for an explicit native-session restart.
        'disconnected: loop {
            if TERMINATE.load(Ordering::Relaxed) {
                group.terminate(&mut journal)?;
                return Ok(());
            }
            for (path, q) in mailbox(&dir)? {
                let restart =
                    matches!(q.command, Command::Restart) && claim_command(&mut journal, &q)?;
                if !restart && !journal.transcript.commands.contains_key(&q.id) {
                    journal.push(Event::CommandState {
                        id: q.id.clone(),
                        state: CommandPhase::Rejected,
                    })?;
                }
                fs::remove_file(path)?;
                if restart {
                    journal.push(Event::CommandState {
                        id: q.id,
                        state: CommandPhase::Accepted,
                    })?;
                    journal.generation = journal::unique();
                    journal.push(Event::Generation)?;
                    break 'disconnected;
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicates_and_stale_approvals_are_never_dispatched() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path(), "new".into()).unwrap();
        j.push(Event::Generation).unwrap();
        let q = QueuedCommand {
            id: "command1".into(),
            generation: "new".into(),
            command: Command::Interrupt,
        };
        assert!(claim_command(&mut j, &q).unwrap());
        assert!(!claim_command(&mut j, &q).unwrap());
        let q = QueuedCommand {
            id: "command2".into(),
            generation: "old".into(),
            command: Command::Respond {
                request_id: "request".into(),
                decision: Decision::Allow,
                answers: Default::default(),
            },
        };
        assert!(!claim_command(&mut j, &q).unwrap());
    }
    #[test]
    fn initial_intent_is_durable_before_dispatch_and_never_replayed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claim_initial(dir.path()).unwrap());
        assert!(!claim_initial(dir.path()).unwrap());
        assert!(dir.path().join("initial-intent").is_file());
    }
}

#[cfg(all(test, unix))]
mod subprocess_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::os::unix::process::CommandExt;
    #[test]
    fn outbound_queue_failure_retires_deferred_prompts_but_publishes_final_events() {
        use serde_json::json;
        for tool in [Tool::Claude, Tool::Antigravity] {
            let root = tempfile::tempdir().unwrap();
            let mut journal = Journal::open(root.path(), "generation".into()).unwrap();
            journal.push(Event::Generation).unwrap();
            let mut adapter = Adapter::new(tool, root.path().into(), None);
            if tool == Tool::Claude {
                adapter.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{}}})).unwrap();
                adapter
                    .ingest(json!({"type":"system","model":"old"}))
                    .unwrap();
            } else {
                adapter.ingest(json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{}}})).unwrap();
                adapter.ingest(json!({"id":"session","result":{"sessionId":"native","models":{"currentModelId":"old","availableModels":[{"modelId":"old","name":"Old"},{"modelId":"new","name":"New"}]}}})).unwrap();
            }
            let mut backend = Backend::spawn(Process::new("/bin/cat")).unwrap();
            let (out, received) = crossbeam_channel::bounded(1);
            backend.out = out;
            let mut active = Some("submission".into());
            let control = adapter
                .command(
                    &Command::Submit {
                        text: "must remain unsent".into(),
                        attachments: vec![],
                        model: Some("new".into()),
                    },
                    "fixture",
                )
                .unwrap();
            let identity = if tool == Tool::Claude {
                control.writes[0]["request_id"].clone()
            } else {
                control.writes[0]["id"].clone()
            };
            backend.effects(&mut journal, control, &mut active).unwrap();
            received.recv().unwrap();
            backend
                .effects(
                    &mut journal,
                    Effects {
                        writes: vec![json!({"before_failure":true}), json!({"fails":true})],
                        ..Default::default()
                    },
                    &mut active,
                )
                .unwrap();
            assert_eq!(received.recv().unwrap(), json!({"before_failure":true}));
            let acknowledgement = if tool == Tool::Claude {
                json!({"type":"control_response","response":{"request_id":identity,"subtype":"success","response":{}}})
            } else {
                json!({"id":identity,"result":{}})
            };
            let effects = adapter.ingest(acknowledgement).unwrap();
            assert_eq!(effects.writes.len(), 1);
            backend.effects(&mut journal, effects, &mut active).unwrap();
            assert!(
                received.try_recv().is_err(),
                "no deferred prompt reaches the outbound queue"
            );
            backend
                .effects(
                    &mut journal,
                    Effects {
                        events: vec![Event::Session("final-session".into())],
                        ..Default::default()
                    },
                    &mut active,
                )
                .unwrap();
            assert_eq!(
                journal.transcript.provider_session_id.as_deref(),
                Some("final-session")
            );
            assert_eq!(journal.transcript.selected_model.as_deref(), Some("new"));
            assert_eq!(active.as_deref(), Some("submission"));
        }
    }
    #[test]
    fn worker_entry() {
        let Ok(id) = std::env::var("KILN_GUI_FIXTURE_ID") else {
            return;
        };
        serve(&id).unwrap();
    }
    fn worker(config: &Config, root: &Path) -> Child {
        let dir = root.join("agent-gui").join(&config.descriptor.id);
        journal::private_dir(&dir).unwrap();
        journal::atomic(
            &dir.join("config.json"),
            &serde_json::to_vec(config).unwrap(),
        )
        .unwrap();
        let mut command = Process::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "app::agent_gui::host::subprocess_tests::worker_entry",
                "--nocapture",
            ])
            .env("KILN_GUI_FIXTURE_ID", &config.descriptor.id)
            .env("KILN_CONFIG_DIR", root)
            .env("HOME", root)
            .env("KILN_ACCOUNTS_SANDBOX", root)
            .env("CLAUDE_CONFIG_DIR", root.join("claude"))
            .env("CODEX_HOME", root.join("codex"))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for key in [
            "KILN_SOCKET",
            "KILN_SESSION",
            "KILN_INTEGRATION_DIR",
            "KILN_MACOS_RESPONSIBILITY_PID",
            "KILN_ORIGINAL_ZDOTDIR",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GOOGLE_API_KEY",
        ] {
            command.env_remove(key);
        }
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().unwrap()
    }
    fn wait_transcript(dir: &Path, predicate: impl Fn(&Transcript) -> bool) -> Transcript {
        let start = Instant::now();
        let mut transcript = Transcript::default();
        let mut offset = 0;
        loop {
            if let Ok(batch) = journal::tail(&dir.join("events.jsonl"), &mut offset) {
                for envelope in batch {
                    transcript.apply(&envelope);
                }
            }
            if predicate(&transcript) {
                return transcript;
            }
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "fixture timeout; state={:?}, errors={:?}, requests={:?}",
                transcript.lifecycle,
                transcript.errors,
                transcript.requests
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn send(dir: &Path, t: &Transcript, command: Command) -> String {
        let id = journal::unique();
        journal::enqueue(
            dir,
            &QueuedCommand {
                id: id.clone(),
                generation: t.generation.clone(),
                command,
            },
        )
        .unwrap();
        id
    }
    fn stop(child: &mut Child) {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGHUP);
        }
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < Duration::from_secs(4));
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn script(root: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(name);
        let python = kiln_common::paths::find_executable("python3", &[])
            .expect("test fixtures require python3");
        fs::write(&path, format!("#!{}\n{}", python.display(), body)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn config(root: &Path, program: PathBuf, tool: Tool) -> Config {
        Config {
            descriptor: Descriptor {
                id: journal::unique(),
                tool,
                cwd: root.into(),
                account_label: "Fixture".into(),
            },
            program,
            environment: vec![],
            launch_options: LaunchOptions::Inherited,
            initial_prompt: Some("fixture task".into()),
        }
    }
    #[test]
    fn real_subprocesses_require_actual_allow_deny_questions_and_interrupt_responses() {
        for tool in Tool::ALL {
            let root = tempfile::tempdir().unwrap();
            let program = script(root.path(), "provider", PROTOCOL_FIXTURE);
            let mut config = config(root.path(), program, tool);
            config
                .environment
                .push(("FIXTURE_PROVIDER".into(), tool.key().into()));
            let dir = root.path().join("agent-gui").join(&config.descriptor.id);
            let mut child = worker(&config, root.path());
            let t = wait_transcript(&dir, |t| !t.requests.is_empty());
            let request = t.requests.values().next().unwrap().id.clone();
            send(
                &dir,
                &t,
                Command::Respond {
                    request_id: request,
                    decision: Decision::Deny,
                    answers: Default::default(),
                },
            );
            let t = wait_transcript(&dir, |t| {
                t.requests.values().any(|r| r.title.contains("allow"))
            });
            let request = t.requests.values().next().unwrap().id.clone();
            send(
                &dir,
                &t,
                Command::Respond {
                    request_id: request,
                    decision: Decision::Allow,
                    answers: Default::default(),
                },
            );
            let t = wait_transcript(&dir, |t| {
                t.requests.values().any(|r| {
                    !r.questions.is_empty()
                        || (tool == Tool::Antigravity && r.title == "allow question")
                })
            });
            let request = t.requests.values().next().unwrap();
            let question_id = request
                .questions
                .first()
                .map(|q| q.id.clone())
                .unwrap_or_default();
            let request_id = request.id.clone();
            send(
                &dir,
                &t,
                Command::Respond {
                    request_id,
                    decision: Decision::Allow,
                    answers: BTreeMap::from([(question_id, vec!["answer".into()])]),
                },
            );
            let t = wait_transcript(&dir, |t| {
                t.entries.iter().any(|entry|matches!(entry,Entry::Tool(tool)if tool.output.contains("response verified")))&&t.lifecycle==Lifecycle::Ready
            });
            assert!(t.provider_session_id.is_some());
            assert!(t.errors.is_empty(), "{:?}", t.errors);
            send(
                &dir,
                &t,
                Command::Submit {
                    text: "wait for interrupt".into(),
                    attachments: vec![],
                    model: None,
                },
            );
            let t = wait_transcript(&dir, |t| {
                t.lifecycle == Lifecycle::Running
                    && t.entries
                        .iter()
                        .filter(|e| matches!(e,Entry::Message(m)if m.role==Role::User))
                        .count()
                        == 2
            });
            std::thread::sleep(Duration::from_millis(60));
            send(&dir, &t, Command::Interrupt);
            let t = wait_transcript(&dir, |t| {
                t.lifecycle == Lifecycle::Ready
                    && t.entries.iter().any(
                        |e| matches!(e,Entry::Message(m)if m.text.contains("interrupted verified")),
                    )
            });
            assert!(t.errors.is_empty(), "{:?}", t.errors);
            stop(&mut child);
        }
    }
    #[test]
    fn worker_sighup_cleans_resistant_children_and_grandchildren_without_signaling_foreign_group() {
        let root = tempfile::tempdir().unwrap();
        let program = script(root.path(), "resistant", RESISTANT_FIXTURE);
        let mut config = config(root.path(), program, Tool::Claude);
        config.initial_prompt = None;
        config.environment.push((
            "PID_FILE".into(),
            root.path().join("pids").to_string_lossy().into_owned(),
        ));
        let mut foreign = Process::new("/bin/sh")
            .args(["-c", "trap '' HUP TERM; while :; do sleep 1; done"])
            .process_group(0)
            .spawn()
            .unwrap();
        let mut child = worker(&config, root.path());
        let start = Instant::now();
        let pids = loop {
            if let Ok(pids) = fs::read_to_string(root.path().join("pids")) {
                let values = pids
                    .lines()
                    .filter_map(|p| p.parse::<i32>().ok())
                    .collect::<Vec<_>>();
                if values.len() == 3 {
                    break values;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        };
        stop(&mut child);
        for pid in pids {
            let start = Instant::now();
            while alive(pid) {
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "owned PID survived: {pid}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(
            foreign.try_wait().unwrap().is_none(),
            "foreign process group was signaled"
        );
        unsafe {
            libc::kill(-(foreign.id() as i32), libc::SIGKILL);
        }
        let _ = foreign.wait();
    }

    #[test]
    fn replacing_an_exited_worker_resumes_native_session_without_replaying_initial_task() {
        for tool in Tool::ALL {
            let root = tempfile::tempdir().unwrap();
            let program = script(root.path(), "resume-provider", PROTOCOL_FIXTURE);
            let mut config = config(root.path(), program, tool);
            config.initial_prompt = None;
            config.environment.extend([
                ("FIXTURE_PROVIDER".into(), tool.key().into()),
                (
                    "FIXTURE_RESUME_FILE".into(),
                    root.path()
                        .join("has-started")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ]);
            let dir = root.path().join("agent-gui").join(&config.descriptor.id);
            let mut child = worker(&config, root.path());
            let first = wait_transcript(&dir, |t| {
                t.lifecycle == Lifecycle::Ready && t.provider_session_id.is_some()
            });
            let generation = first.generation.clone();
            stop(&mut child);
            let mut second = worker(&config, root.path());
            let resumed = wait_transcript(&dir, |t| {
                t.generation != generation && t.lifecycle == Lifecycle::Ready
            });
            assert_eq!(resumed.provider_session_id, first.provider_session_id);
            assert_eq!(resumed.entries, first.entries);
            assert!(resumed.errors.is_empty(), "{:?}", resumed.errors);
            stop(&mut second);
        }
    }

    #[test]
    fn same_host_handoff_waits_for_native_exit_preserves_work_and_resumes_without_prompt_replay() {
        use serde_json::json;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let native_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let namespace = cwd.join("claude");
        let project = namespace.join("projects/fixture");
        fs::create_dir_all(&project).unwrap();
        let native_file = project.join(format!("{native_id}.jsonl"));
        let row = |kind: &str, id: &str, parent: Option<&str>, text: &str| json!({
            "type":kind,"uuid":id,"parentUuid":parent,"sessionId":native_id,"cwd":cwd,
            "message":{"role":kind,"content":[{"type":"text","text":text}],"stop_reason":if kind=="assistant" {Some("end_turn")} else {None}}
        });
        fs::write(&native_file, format!("{}\n{}\n", row("user", "u1", None, "existing task"), row("assistant", "a1", Some("u1"), "existing answer"))).unwrap();
        let prefix = r#"import os,sys,json,time
root=os.environ['HANDOFF_ROOT']
with open(root+'/launches','a') as log: log.write(json.dumps({'args':sys.argv[1:],'cwd':os.getcwd(),'home':os.environ.get('CLAUDE_CONFIG_DIR')})+'\n')
if '-p' not in sys.argv:
 assert sys.argv[1:]==['--resume','aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee']
 with open(root+'/native-pid','w') as f: f.write(str(os.getpid()))
 print('NATIVE TERMINAL',flush=True)
 while not os.path.exists(root+'/exit-native'): time.sleep(.02)
 sid='aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'
 with open(os.environ['HANDOFF_NATIVE_FILE'],'a') as history:
  for kind,uid,parent,text in [('user','u2','a1','terminal follow-up'),('assistant','a2','u2','terminal answer')]:
   history.write(json.dumps({'type':kind,'uuid':uid,'parentUuid':parent,'sessionId':sid,'cwd':os.getcwd(),'message':{'role':kind,'content':[{'type':'text','text':text}],'stop_reason':'end_turn' if kind=='assistant' else None}})+'\n')
 sys.exit(0)
"#;
        let provider = PROTOCOL_FIXTURE.replace("sid='fixture-session'", &format!("sid='{native_id}'"))
            .replace(" elif typ=='user' or m in ['turn/start','session/prompt']:\n  turn+=1", " elif typ=='user' or m in ['turn/start','session/prompt']:\n  with open(root+'/prompts','a') as log: log.write(json.dumps(v)+'\\n')\n  finish()\n  continue\n  turn+=1");
        let program = script(&cwd, "handoff-provider", &format!("{prefix}\n{provider}"));
        let mut config = config(&cwd, program, Tool::Claude);
        config.initial_prompt = Some("existing task".into());
        config.environment = vec![
            ("FIXTURE_PROVIDER".into(), "claude".into()),
            ("FIXTURE_RESUME_FILE".into(), cwd.join("started").to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(), namespace.to_string_lossy().into_owned()),
            ("HANDOFF_ROOT".into(), cwd.to_string_lossy().into_owned()),
            ("HANDOFF_NATIVE_FILE".into(), native_file.to_string_lossy().into_owned()),
        ];
        let dir = cwd.join("agent-gui").join(&config.descriptor.id);
        let mut worker = worker(&config, &cwd);
        let first = wait_transcript(&dir, |t|t.lifecycle == Lifecycle::Ready && t.provider_session_id.as_deref() == Some(native_id)
            && t.entries.iter().any(|entry| matches!(entry, Entry::Message(message) if message.role == Role::User && message.text == "existing task"))
            && t.commands.values().all(|phase|*phase == CommandPhase::Accepted));
        send(&dir, &first, Command::SetPresentation(Presentation::Terminal));
        let terminal = wait_transcript(&dir, |t|t.presentation == Presentation::Terminal && cwd.join("native-pid").exists());
        let native_pid: i32 = fs::read_to_string(cwd.join("native-pid")).unwrap().parse().unwrap();
        assert!(alive(native_pid));
        assert!(worker.try_wait().unwrap().is_none());
        // Bad transport data must not terminate the native task or start a GUI writer.
        fs::write(dir.join("commands/invalid.json"), b"not-json").unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(alive(native_pid));
        assert_eq!(fs::read_to_string(cwd.join("launches")).unwrap().lines().count(), 2);
        fs::remove_file(dir.join("commands/invalid.json")).unwrap();
        send(&dir, &terminal, Command::SetPresentation(Presentation::Gui));
        wait_transcript(&dir, |t|t.presentation == Presentation::Terminal && t.return_requested);
        assert!(alive(native_pid), "GUI tab must leave the native task running");
        fs::write(cwd.join("exit-native"), b"exit naturally").unwrap();
        let resumed = wait_transcript(&dir, |t| t.presentation == Presentation::Gui && t.lifecycle == Lifecycle::Ready && t.generation != first.generation);
        assert_eq!(resumed.provider_session_id.as_deref(), Some(native_id));
        assert_eq!(resumed.entries.iter().filter(|entry|matches!(entry, Entry::Message(message) if message.text == "terminal follow-up")).count(), 1);
        assert_eq!(resumed.entries.iter().filter(|entry|matches!(entry, Entry::Message(message) if message.text == "terminal answer")).count(), 1);
        let launches: Vec<Value> = fs::read_to_string(cwd.join("launches")).unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect();
        assert_eq!(launches.len(), 3);
        assert!(launches.iter().all(|launch|launch["cwd"] == cwd.to_string_lossy().as_ref() && launch["home"] == namespace.to_string_lossy().as_ref()));
        assert!(launches[2]["args"].as_array().unwrap().iter().any(|arg|arg == "--resume"));
        assert!(resumed.commands.values().all(|phase|*phase == CommandPhase::Accepted));
        assert_eq!(fs::read_to_string(cwd.join("prompts")).unwrap().lines().count(), 1, "The original request is never replayed during handoff");
        assert_eq!(resumed.entries.iter().filter(|entry|matches!(entry, Entry::Message(message) if message.role == Role::User && message.text == "existing task")).count(), 1);
        stop(&mut worker);
    }
    #[test]
    fn idle_handoff_rejects_background_descendants_without_signaling_their_work() {
        let root = tempfile::tempdir().unwrap();
        let prefix = r#"import subprocess,os,sys
background=subprocess.Popen([sys.executable,'-c',"import subprocess,os,time; child=subprocess.Popen(['sleep','60']); open(os.environ['BACKGROUND_PIDS'],'w').write(str(os.getpid())+'\\n'+str(child.pid)); time.sleep(60)"])
"#;
        let provider = PROTOCOL_FIXTURE.replace("sid='fixture-session'", "sid='aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee'");
        let program = script(root.path(), "background-provider", &format!("{prefix}\n{provider}"));
        let mut config = config(root.path(), program, Tool::Claude);
        config.initial_prompt = None;
        config.environment = vec![
            ("FIXTURE_PROVIDER".into(), "claude".into()),
            ("BACKGROUND_PIDS".into(), root.path().join("background-pids").to_string_lossy().into_owned()),
        ];
        let dir = root.path().join("agent-gui").join(&config.descriptor.id);
        let mut worker = worker(&config, root.path());
        let first = wait_transcript(&dir, |t|t.lifecycle == Lifecycle::Ready && root.path().join("background-pids").is_file());
        let pids: Vec<i32> = fs::read_to_string(root.path().join("background-pids")).unwrap().lines().map(|line|line.parse().unwrap()).collect();
        assert_eq!(pids.len(), 2);
        let request = send(&dir, &first, Command::SetPresentation(Presentation::Terminal));
        let rejected = wait_transcript(&dir, |t|t.commands.get(&request) == Some(&CommandPhase::Rejected));
        assert_eq!(rejected.presentation, Presentation::Gui);
        assert_eq!(rejected.lifecycle, Lifecycle::Ready);
        assert_eq!(rejected.generation, first.generation);
        assert!(pids.iter().all(|pid|alive(*pid)), "An idle presentation switch must never signal a background server or grandchild");
        assert!(worker.try_wait().unwrap().is_none());
        // Explicitly close only this fixture's owned group after the survival assertions.
        stop(&mut worker);
    }
    #[test]
    fn disconnected_worker_restarts_with_new_generation_and_same_native_session() {
        for tool in Tool::ALL {
            let root = tempfile::tempdir().unwrap();
            let program = script(root.path(), "restart-provider", PROTOCOL_FIXTURE);
            let mut config = config(root.path(), program, tool);
            config.initial_prompt = None;
            config.environment.extend([
                ("FIXTURE_PROVIDER".into(), tool.key().into()),
                (
                    "FIXTURE_RESUME_FILE".into(),
                    root.path()
                        .join("has-started")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ("FIXTURE_EXIT_ONCE".into(), "1".into()),
            ]);
            let dir = root.path().join("agent-gui").join(&config.descriptor.id);
            let mut child = worker(&config, root.path());
            let disconnected = wait_transcript(&dir, |t| t.lifecycle == Lifecycle::Disconnected);
            assert_eq!(
                disconnected.provider_session_id.as_deref(),
                Some("fixture-session")
            );
            send(&dir, &disconnected, Command::Restart);
            let restarted = wait_transcript(&dir, |t| {
                t.generation != disconnected.generation && t.lifecycle == Lifecycle::Ready
            });
            assert_eq!(
                restarted.provider_session_id,
                disconnected.provider_session_id
            );
            assert_eq!(restarted.entries.len(), 0);
            stop(&mut child);
        }
    }
    #[test]
    fn sealed_journal_rejects_same_id_worker_restart_before_launching_or_replaying_pending_submission()
     {
        let root = tempfile::tempdir().unwrap();
        let boot = root.path().join("provider-booted");
        let program = script(
            root.path(),
            "must-not-launch",
            &format!(
                "from pathlib import Path\nPath({:?}).write_text('launched')\n",
                boot.to_string_lossy()
            ),
        );
        let mut config = config(root.path(), program, Tool::Claude);
        config.initial_prompt = Some("never replay initial".into());
        let dir = root.path().join("agent-gui").join(&config.descriptor.id);
        journal::private_dir(&dir).unwrap();
        let mut j = Journal::open(&dir, "old".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Lifecycle(Lifecycle::Running)).unwrap();
        journal::enqueue(
            &dir,
            &QueuedCommand {
                id: "pending".into(),
                generation: "old".into(),
                command: Command::Submit {
                    text: "never replay queued".into(),
                    attachments: vec![],
                    model: None,
                },
            },
        )
        .unwrap();
        journal::atomic(
            &dir.join("failure.json"),
            br#"{"reason":"fixture exhausted journal; retained history is read-only"}"#,
        )
        .unwrap();
        let mut replacement = worker(&config, root.path());
        let start = Instant::now();
        loop {
            if let Some(status) = replacement.try_wait().unwrap() {
                assert!(!status.success());
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!boot.exists());
        assert!(dir.join("commands/pending.json").is_file());
        assert!(!dir.join("initial-intent").exists());
        let mut reopened = Journal::open(&dir, "retry".into()).unwrap();
        assert_eq!(
            reopened.transcript.provider_session_id.as_deref(),
            Some("native")
        );
        assert!(reopened.push(Event::Generation).is_err());
    }
    fn alive(pid: i32) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of_val(&info) as i32;
            let n = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            n == size && info.pbi_status != 5
        }
        #[cfg(not(target_os = "macos"))]
        {
            fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .is_some_and(|stat| {
                    !stat
                        .split(')')
                        .nth(1)
                        .unwrap_or_default()
                        .trim_start()
                        .starts_with('Z')
                })
        }
    }
    const RESISTANT_FIXTURE: &str = r#"import os,signal,time
signal.signal(signal.SIGHUP,signal.SIG_IGN)
signal.signal(signal.SIGTERM,signal.SIG_IGN)
def record():
 with open(os.environ['PID_FILE'],'a') as f: f.write(str(os.getpid())+'\n'); f.flush(); os.fsync(f.fileno())
record()
if os.fork()==0:
 record()
 if os.fork()==0:
  record()
  while True: time.sleep(.1)
 while True: time.sleep(.1)
while True: time.sleep(.1)
"#;
    const PROTOCOL_FIXTURE: &str = r#"import os,sys,json,time
p=os.environ['FIXTURE_PROVIDER'];sid='fixture-session';turn=0
resume_file=os.environ.get('FIXTURE_RESUME_FILE');resumed=bool(resume_file and os.path.exists(resume_file))
if resume_file: open(resume_file,'w').close()
if resumed and p=='claude': assert '--resume' in sys.argv and sys.argv[sys.argv.index('--resume')+1]==sid

def read():
 line=sys.stdin.readline()
 assert line, 'unexpected EOF'
 return json.loads(line)
def emit(v):
 line=json.dumps(v)+'\n';mid=len(line)//2
 sys.stdout.write(line[:mid]);sys.stdout.flush();time.sleep(.002);sys.stdout.write(line[mid:]);sys.stdout.flush()
def result(i,r): emit({'jsonrpc':'2.0','id':i,'result':r})
def request(i,m,p): emit({'jsonrpc':'2.0','id':i,'method':m,'params':p})
def notice(m,p): emit({'jsonrpc':'2.0','method':m,'params':p})
def claude_request(i,name): emit({'type':'control_request','request_id':i,'request':{'subtype':'can_use_tool','tool_name':name,'tool_use_id':'tool1','input':{'content':'fixture','questions':[{'question':'Choose','options':[{'label':'answer','description':''}]}] if name=='AskUserQuestion' else []}}})
def finish():
 if p=='claude': emit({'type':'result','session_id':sid,'is_error':False})
 elif p=='codex': notice('turn/completed',{'turn':{'id':'turn1','status':'completed','error':None}})
 else: result('prompt',{'stopReason':'end_turn'})
def permission(i,title):
 if p=='claude': claude_request(i,title)
 elif p=='codex': request(i,'item/commandExecution/requestApproval',{'itemId':'tool1','reason':title})
 else: request(i,'session/request_permission',{'sessionId':sid,'toolCall':{'toolCallId':'tool1','title':title},'options':[{'optionId':'allow','name':'Allow','kind':'allow_once'},{'optionId':'deny','name':'Deny','kind':'reject_once'}]})
def check_permission(v,allow):
 if p=='claude': assert v['response']['response']['behavior']==('allow' if allow else 'deny'); assert not allow or v['response']['response']['updatedInput']['content']=='fixture'
 elif p=='codex': assert v['result']['decision']==('accept' if allow else 'decline')
 else: assert v['result']['outcome']['optionId']==('allow' if allow else 'deny')
while True:
 v=read();m=v.get('method','');typ=v.get('type','')
 if p=='claude' and typ=='control_request':
  sub=v['request']['subtype']
  if sub=='initialize':
   emit({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':{'models':[]}}});emit({'type':'system','subtype':'init','session_id':sid,'model':'fixture'})
   if os.environ.get('FIXTURE_EXIT_ONCE') and not resumed: sys.exit(0)
  elif sub=='interrupt': emit({'type':'assistant','message':{'id':'interrupt','content':[{'type':'text','text':'interrupted verified'}]}});finish()
 elif m=='initialize': result(v['id'],{'protocolVersion':1,'agentCapabilities':{'loadSession':True}} if p=='antigravity' else {})
 elif m=='model/list': result(v['id'],{'data':[]})
 elif m in ['thread/start','thread/resume']:
  assert not resume_file or (m=='thread/resume')==resumed
  if resumed: assert v['params']['threadId']==sid
  result(v['id'],{'thread':{'id':sid},'model':'fixture'})
  if os.environ.get('FIXTURE_EXIT_ONCE') and not resumed: sys.exit(0)
 elif m in ['session/new','session/load']:
  assert not resume_file or (m=='session/load')==resumed
  if resumed:
   assert v['params']['sessionId']==sid
   notice('session/update',{'sessionId':sid,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'old native history replay'}}})
  result(v['id'],{'sessionId':sid})
  if os.environ.get('FIXTURE_EXIT_ONCE') and not resumed: sys.exit(0)
 elif m=='turn/interrupt': notice('item/completed',{'item':{'id':'interrupt','type':'agentMessage','text':'interrupted verified'}});result(v['id'],{});finish()
 elif m=='session/cancel': notice('session/update',{'sessionId':sid,'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'interrupted verified'}}});finish()
 elif typ=='user' or m in ['turn/start','session/prompt']:
  turn+=1
  if p=='codex': result(v['id'],{'turn':{'id':'turn1'}})
  if turn>1: continue
  permission('deny','deny operation');check_permission(read(),False)
  permission('allow','allow operation');check_permission(read(),True)
  if p=='claude': claude_request('question','AskUserQuestion')
  elif p=='codex': request('question','item/tool/requestUserInput',{'questions':[{'id':'choice','question':'Choose','header':'Choose','options':[{'label':'answer','description':''}]}]})
  else:
   request('unknown','future/method',{});assert read()['error']['code']==-32601
   permission('question','allow question');v=read();check_permission(v,True)
  if p!='antigravity':
   answer=read()
   if p=='claude': assert answer['response']['response']['updatedInput']['answers']['Choose']=='answer'
   else: assert answer['result']['answers']['choice']['answers']==['answer']
  if p=='claude': emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'tool1','content':'response verified'}]}})
  elif p=='codex': notice('item/completed',{'item':{'id':'tool1','type':'commandExecution','command':'fixture tool','status':'completed','aggregatedOutput':'response verified'}})
  else: notice('session/update',{'sessionId':sid,'update':{'sessionUpdate':'tool_call_update','toolCallId':'tool1','status':'completed','rawOutput':'response verified'}})
  finish()
"#;
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use kiln_accounts::connection::{Connection, Preset};
    #[test]
    fn host_resolves_the_pinned_connection_only_at_final_child_launch() {
        for tool in [Tool::Claude, Tool::Codex] {
            let dir = tempfile::tempdir().unwrap();
            let (env, _) = kiln_accounts::Env::sandbox(dir.path(), false);
            let manager = AccountManager::with_env(env);
            let connection = |key: &str| Connection {
                preset: Preset::Gateway,
                base_url: "https://fixture.invalid/v1".into(),
                key: key.into(),
                ..Default::default()
            };
            let first = manager
                .save_connection(tool, None, "first", connection("fixture-first-key"))
                .unwrap();
            let second = manager
                .save_connection(tool, None, "second", connection("fixture-second-key"))
                .unwrap();
            manager.switch_to(tool, &first.id).unwrap();
            let pinned = manager.launch_environment(tool).unwrap();
            manager.switch_to(tool, &second.id).unwrap();
            let config = Config {
                descriptor: Descriptor {
                    id: "fixture".into(),
                    tool,
                    cwd: dir.path().into(),
                    account_label: "first".into(),
                },
                program: "fixture-agent".into(),
                environment: pinned,
                initial_prompt: None,
                launch_options: LaunchOptions::Inherited,
            };
            let serialized = serde_json::to_string(&config).unwrap();
            assert!(!serialized.contains("fixture-first-key"));
            assert!(!serialized.contains("fixture-second-key"));
            let adapter = Adapter::new(tool, dir.path().into(), None);
            let cmd = command(&config, &adapter, &manager).unwrap();
            let key = if tool == Tool::Claude {
                "ANTHROPIC_AUTH_TOKEN"
            } else {
                "KILN_CODEX_API_KEY"
            };
            assert!(
                cmd.get_envs()
                    .any(|(k, v)| k == key && v == Some(std::ffi::OsStr::new("fixture-first-key")))
            );
            assert_eq!(manager.active(tool).as_deref(), Some(second.id.as_str()));
            if tool == Tool::Codex {
                assert!(
                    cmd.get_args()
                        .any(|arg| arg == "cli_auth_credentials_store=\"file\"")
                );
            }
        }
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::*;
    use kiln_accounts::CredentialStore;
    use serde_json::json;
    #[test]
    fn child_keeps_the_original_subscription_namespace_after_account_switch() {
        let root = tempfile::tempdir().unwrap();
        let (env, store) = kiln_accounts::Env::sandbox(root.path(), true);
        let manager = AccountManager::with_env(env.clone());
        let login = |marker: &str, email: &str| {
            store.set(kiln_accounts::claude::CLAUDE_SERVICE,"claude-code-user",&json!({"claudeAiOauth":{"accessToken":format!("fixture-{marker}"),"refreshToken":format!("fixture-refresh-{marker}"),"expiresAt":4102444800000i64}}).to_string()).unwrap();
            fs::create_dir_all(&env.home).unwrap();
            fs::write(
                env.home.join(".claude.json"),
                json!({"oauthAccount":{"emailAddress":email,"accountUuid":email}}).to_string(),
            )
            .unwrap();
        };
        login("first", "first@example.test");
        let first = manager.save_current(Tool::Claude, "First").unwrap();
        let pinned = manager.launch_environment(Tool::Claude).unwrap();
        login("second", "second@example.test");
        let second = manager.save_current(Tool::Claude, "Second").unwrap();
        assert_eq!(
            manager.active(Tool::Claude).as_deref(),
            Some(second.id.as_str())
        );
        let config = Config {
            descriptor: Descriptor {
                id: "fixture".into(),
                tool: Tool::Claude,
                cwd: root.path().into(),
                account_label: "First".into(),
            },
            program: "fixture-agent".into(),
            environment: pinned,
            initial_prompt: None,
            launch_options: LaunchOptions::Inherited,
        };
        let child = command(
            &config,
            &Adapter::new(Tool::Claude, root.path().into(), None),
            &manager,
        )
        .unwrap();
        assert!(
            child
                .get_envs()
                .any(|(k, v)| k == "KILN_AGENT_PROFILE"
                    && v == Some(std::ffi::OsStr::new(&first.id)))
        );
        assert!(
            !serde_json::to_string(&config)
                .unwrap()
                .contains("fixture-first")
        );
        assert_eq!(
            manager.active(Tool::Claude).as_deref(),
            Some(second.id.as_str())
        );
    }
}
