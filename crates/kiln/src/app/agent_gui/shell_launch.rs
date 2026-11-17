//! Exact shell-session binding for a foreground agent host; never select a recent conversation.
use super::*;
use std::{fs, io::IsTerminal};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ShellConfig {
    pub shell_session: u64,
    pub host_pid: u32,
    pub parent_session: u32,
    pub record_path: PathBuf,
    pub record_generation: String,
    pub initial_spec: SpawnSpec,
}

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    shell_session: u64,
    host_pid: u32,
    descriptor: Descriptor,
}

fn shell_session() -> Option<u64> {
    std::env::var("KILN_SESSION")
        .ok()?
        .parse()
        .ok()
        .filter(|session| *session != 0)
}

fn foreground_shell() -> bool {
    if shell_session().is_none()
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
    {
        return false;
    }
    #[cfg(unix)]
    unsafe {
        let pid = libc::getpid();
        // A pipeline member, background invocation or shell-session leader is not
        // an exclusive foreground job that can own a reusable structured worker.
        pid > 1
            && libc::getpgrp() == pid
            && libc::tcgetpgrp(0) == pid
            && libc::getsid(0) > 1
            && libc::getsid(0) != pid
    }
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn interactive_arguments(tool: Tool, args: &[String]) -> bool {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-v"))
    {
        return false;
    }
    match tool {
        Tool::Claude => {
            !args.iter().any(|arg| {
                matches!(arg.as_str(), "-p" | "--print")
                    || arg.starts_with("--output-format")
                    || arg.starts_with("--input-format") || arg.starts_with("--print=")
            }) && !args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "auth"
                        | "login"
                        | "logout"
                        | "update"
                        | "upgrade"
                        | "install"
                        | "doctor"
                        | "mcp"
                        | "agents"
                        | "attach"
                        | "logs"
                        | "stop"
                        | "respawn"
                        | "rm"
                        | "plugin"
                        | "remote-control"
                )
            })
        }
        Tool::Codex => {
            !args
                .iter()
                .any(|arg| arg == "--remote" || arg.starts_with("--remote="))
                && !args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "exec"
                            | "e"
                            | "review"
                            | "login"
                            | "logout"
                            | "mcp"
                            | "mcp-server"
                            | "app-server"
                            | "completion"
                            | "sandbox"
                            | "debug"
                            | "apply"
                            | "cloud"
                            | "update"
                    )
                })
        }
        Tool::Antigravity => !args.iter().any(|arg| {
            matches!(arg.as_str(), "-p" | "--prompt" | "update" | "acp")
                || arg.starts_with("--output-format")
        }),
    }
}

/// Noninteractive commands, user wrappers, redirected input and pipeline members keep native execution.
pub fn shell_launch_eligible(tool: Tool, args: &[String]) -> bool {
    interactive_arguments(tool, args) && foreground_shell()
}

fn binding_path(session: u64) -> Result<PathBuf> {
    ensure!(session != 0, "invalid shell session");
    let directory = std::path::absolute(kiln_common::paths::config_dir())?.join("agent-gui-shell");
    journal::private_dir(&directory)?;
    Ok(directory.join(format!("{session}.json")))
}

struct PublishedBinding {
    path: PathBuf,
    id: String,
    _lock: fs::File,
}
impl Drop for PublishedBinding {
    fn drop(&mut self) {
        if journal::read::<Binding>(&self.path)
            .ok()
            .is_some_and(|binding| binding.descriptor.id == self.id)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Resolve one exact parent PTY binding. Both the publisher and conversation host
/// must still hold their locks; stale files and unreadable ownership are not active agents.
pub fn shell_binding(session: u64) -> Result<Option<SpawnSpec>> {
    let path = binding_path(session)?;
    if !path.exists() {
        return Ok(None);
    }
    let binding: Binding = journal::read(&path)?;
    ensure!(
        binding.shell_session == session && binding.host_pid > 1,
        "invalid shell binding"
    );
    let lock_path = path.with_extension("lock");
    if !lock_path.is_file() {
        return Ok(None);
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => return Ok(None),
        Err(std::fs::TryLockError::WouldBlock) => {}
        Err(error) => return Err(error.into()),
    }
    let dir = journal::directory(&binding.descriptor.id)?;
    if !dir.join("host.lock").is_file() || journal::worker_absent(&dir) {
        return Ok(None);
    }
    let config: Config = journal::read(&dir.join("config.json"))?;
    let shell = config.shell.context("missing shell host metadata")?;
    ensure!(
        config.descriptor == binding.descriptor
            && shell.shell_session == session
            && shell.host_pid == binding.host_pid,
        "shell host identity mismatch"
    );
    host_spec(&binding.descriptor).map(Some)
}

/// Read-only draft restoration for this exact parent terminal. This does not
/// start a worker and never treats an old lock or PID as a live binding.
pub fn saved_shell_spec(session: u64, id: &str) -> Result<SpawnSpec> {
    let dir = journal::directory(id)?;
    let config: Config = journal::read(&dir.join("config.json"))?;
    ensure!(config.descriptor.id == id && config.shell.as_ref().is_some_and(|shell|shell.shell_session == session),
        "Saved shell draft belongs to a different terminal");
    let mut spec = host_spec(&config.descriptor)?;
    spec.env.push(("KILN_SHELL_GUI_DETACHED".into(),"1".into()));
    Ok(spec)
}

/// Runs in the original `kiln agent-shell` foreground process. The parent shell
/// and its terminal session remain alive; the host itself decides when to return.
pub fn serve_shell(
    tool: Tool,
    cwd: PathBuf,
    args: Vec<String>,
    accounts: &AccountManager,
) -> Result<()> {
    ensure!(
        shell_launch_eligible(tool, &args),
        "agent is not an exclusive interactive shell job"
    );
    let session = shell_session().context("missing shell session")?;
    let path = binding_path(session)?;
    let lock = journal::open(&path.with_extension("lock"), false)?;
    lock.try_lock()
        .context("another agent already owns this shell binding")?;
    let (record_path, record, initial_spec) =
        super::super::agent_runtime::prepare_shell_record(tool, cwd.clone(), args, accounts)?;
    let descriptor = Descriptor {
        id: journal::unique(),
        tool,
        cwd,
        account_label: "Original terminal account".into(),
    };
    let dir = journal::directory(&descriptor.id)?;
    journal::private_dir(&dir)?;
    #[cfg(unix)]
    let parent_session = unsafe { libc::getsid(0) as u32 };
    #[cfg(not(unix))]
    let parent_session = 0;
    let config = Config {
        descriptor: descriptor.clone(),
        program: record
            .spec
            .program
            .as_ref()
            .context("missing native agent executable")?
            .into(),
        environment: record.spec.env.clone(),
        initial_prompt: None,
        launch_options: LaunchOptions::Inherited,
        shell: Some(ShellConfig {
            shell_session: session,
            host_pid: std::process::id(),
            parent_session,
            record_path: record_path.clone(),
            record_generation: record.generation,
            initial_spec,
        }),
    };
    journal::atomic(&dir.join("config.json"), &serde_json::to_vec(&config)?)?;
    if let Ok(report) = kiln_accounts::extensions::ExtensionReport::read_prepared(
        &record_path.with_extension("extensions.json"),
    ) {
        report.write_prepared(&dir.join("extension-report.json"))?;
    }
    let mut journal = journal::Journal::open(&dir, journal::unique())?;
    journal.push(Event::Generation)?;
    journal.push(Event::Presentation {
        current: Presentation::Terminal,
        return_requested: false,
    })?;
    journal.flush()?;
    drop(journal);
    let binding = Binding {
        shell_session: session,
        host_pid: std::process::id(),
        descriptor: descriptor.clone(),
    };
    journal::atomic(&path, &serde_json::to_vec(&binding)?)?;
    let published = PublishedBinding {
        path,
        id: descriptor.id.clone(),
        _lock: lock,
    };
    let result = host::serve_status(&descriptor.id);
    // Drop the published binding even when the native process exits unsuccessfully:
    // process::exit does not run destructors, and the parent shell must see its status.
    drop(published);
    match result? {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_management_and_print_modes_are_not_conversation_hosts() {
        for (tool, arguments) in [
            (Tool::Claude, vec!["-p", "query"]),
            (Tool::Claude, vec!["mcp", "list"]),
            (Tool::Claude, vec!["--output-format=json", "query"]),
            (Tool::Codex, vec!["exec", "query"]),
            (Tool::Codex, vec!["--remote=wss://example.invalid"]),
            (Tool::Antigravity, vec!["--prompt", "query"]),
        ] {
            assert!(!interactive_arguments(
                tool,
                &arguments
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>()
            ));
        }
        for tool in Tool::ALL {
            assert!(interactive_arguments(tool, &[]));
        }
        assert!(interactive_arguments(
            Tool::Claude,
            &["--resume".into(), "exact-id".into()]
        ));
        assert!(interactive_arguments(
            Tool::Codex,
            &["--no-daemon".into(), "resume".into(), "exact-id".into()]
        ));
    }
}
