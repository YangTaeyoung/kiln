//! Explicit local-only native CLI smoke. Never uses the user's profile or credentials.
use super::*;
use std::os::unix::{
    io::{AsRawFd, FromRawFd},
    process::CommandExt,
};

const WORKER: &str = "app::agent_gui::host::native_handoff_smoke::worker_entry";
#[test]
#[ignore = "isolated worker entry, invoked only by the explicit native smoke"]
fn worker_entry() {
    let Ok(id) = std::env::var("KILN_NATIVE_HANDOFF_SMOKE_ID") else {
        return;
    };
    serve(&id).unwrap();
}

struct Worker {
    child: Child,
    terminal: fs::File,
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    reading: std::sync::Arc<AtomicBool>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // The child was setsid'ed by this fixture, never a user's shared session.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGHUP);
            }
            let until = Instant::now() + Duration::from_secs(4);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
                let _ = self.child.wait();
            }
        }
        self.reading.store(false, Ordering::Relaxed);
    }
}
fn spawn_worker(config: &Config, root: &Path) -> Worker {
    let dir = root.join("agent-gui").join(&config.descriptor.id);
    journal::private_dir(&dir).unwrap();
    journal::atomic(
        &dir.join("config.json"),
        &serde_json::to_vec(config).unwrap(),
    )
    .unwrap();
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        },
        0
    );
    let terminal = unsafe { fs::File::from_raw_fd(master) };
    let slave = unsafe { fs::File::from_raw_fd(slave) };
    let stdin = slave.try_clone().unwrap();
    let stdout = slave.try_clone().unwrap();
    let mut command = Process::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .envs(config.environment.iter().cloned())
        .env("KILN_CONFIG_DIR", root)
        .env("KILN_NATIVE_HANDOFF_SMOKE_ID", &config.descriptor.id)
        .env("KILN_ACCOUNTS_SANDBOX", root)
        .env("CODEX_HOME", root.join("unused-codex"))
        .args(["--exact", WORKER, "--ignored", "--nocapture"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reading = std::sync::Arc::new(AtomicBool::new(true));
    let mut reader = terminal.try_clone().unwrap();
    unsafe {
        libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let copy = output.clone();
    let flag = reading.clone();
    std::thread::spawn(move || {
        let mut bytes = [0u8; 8192];
        while flag.load(Ordering::Relaxed) {
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    let mut output = copy.lock().unwrap();
                    if output.len() < 512 * 1024 {
                        output.extend_from_slice(&bytes[..n]);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
    });
    Worker {
        child,
        terminal,
        output,
        reading,
    }
}
fn transcript(dir: &Path) -> Transcript {
    let mut value = Transcript::default();
    let mut offset = 0;
    if let Ok(events) = journal::tail(&dir.join("events.jsonl"), &mut offset) {
        for event in events {
            value.apply(&event);
        }
    }
    value
}
fn wait(
    dir: &Path,
    timeout: Duration,
    label: &str,
    predicate: impl Fn(&Transcript) -> bool,
) -> Transcript {
    let start = Instant::now();
    loop {
        let value = transcript(dir);
        if predicate(&value) {
            return value;
        }
        assert!(
            start.elapsed() < timeout,
            "native smoke timeout: {label}; lifecycle={:?}, presentation={:?}, errors={:?}",
            value.lifecycle,
            value.presentation,
            value.errors
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn send(dir: &Path, state: &Transcript, command: Command) -> String {
    let id = journal::unique();
    journal::enqueue(
        dir,
        &QueuedCommand {
            id: id.clone(),
            generation: state.generation.clone(),
            command,
        },
    )
    .unwrap();
    id
}
fn owned_pids(worker: u32) -> Vec<u32> {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = vec![0i32; 4096];
        let count = unsafe {
            libc::proc_listpids(
                2,
                worker,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 4) as i32,
            )
        };
        assert!(count > 0 && count < (buffer.len() * 4) as i32);
        buffer.truncate(count as usize / 4);
        buffer
            .into_iter()
            .filter(|pid| {
                *pid > 1
                    && *pid != worker as i32
                    && unsafe {
                        libc::getpgid(*pid) == worker as i32 && libc::getsid(*pid) == worker as i32
                    }
            })
            .map(|pid| pid as u32)
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        fs::read_dir("/proc")
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                *pid != worker
                    && unsafe {
                        libc::getpgid(*pid as i32) == worker as i32
                            && libc::getsid(*pid as i32) == worker as i32
                    }
            })
            .collect()
    }
}

#[test]
#[ignore = "requires an installed Claude 2.1.327; isolated localhost-only credentials and owned PTY"]
fn installed_claude_mcp_gui_tui_gui_preserves_uuid_without_prompts() {
    native_handoff(false);
}
#[test]
#[ignore = "requires installed Claude; isolated localhost-only credentials and owned PTY"]
fn installed_claude_idle_tui_returns_to_gui_without_manual_exit() {
    native_handoff(false);
}
#[test]
#[ignore = "requires installed Claude; isolated localhost-only credentials and owned PTY"]
fn installed_claude_native_draft_defers_return_and_tui_cancels_it() {
    native_handoff(true);
}
fn native_handoff(guard_draft: bool) {
    use serde_json::json;
    let cli = kiln_common::paths::find_executable("claude", &[])
        .expect("installed Claude CLI")
        .canonicalize()
        .unwrap();
    let python_launcher = kiln_common::paths::find_executable("python3", &[])
        .expect("Python MCP fixture")
        .canonicalize()
        .unwrap();
    // Framework Python launchers exec a different Mach-O. Declare the actual runtime
    // explicitly so this fixture tests an exactly declared MCP, not a guessed alias.
    let python_probe = if cfg!(target_os = "macos") {
        "import ctypes,os; b=ctypes.create_string_buffer(4096); n=ctypes.CDLL(None).proc_pidpath(os.getpid(),b,4096); assert n>0; print(b.value.decode())"
    } else {
        "import os; print(os.readlink('/proc/self/exe'))"
    };
    let output = Process::new(&python_launcher)
        .env_clear()
        .args(["-I", "-S", "-c", python_probe])
        .output()
        .unwrap();
    assert!(output.status.success());
    let python = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        .canonicalize()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let home = root.join("home");
    let profile = home.join(".claude");
    let project = root.join("project");
    for path in [&profile, &project] {
        fs::create_dir_all(path).unwrap();
    }
    let helper = root.join("owned-mcp.py");
    let pid_file = root.join("mcp-pids");
    let eof_file = root.join("mcp-eof");
    fs::write(&helper, r#"import json,os,sys
with open(os.environ['KILN_SMOKE_MCP_PIDS'],'a') as f: f.write(str(os.getpid())+'\n');f.flush()
for line in sys.stdin:
 try: request=json.loads(line)
 except Exception: continue
 if 'id' not in request: continue
 method=request.get('method')
 if method=='initialize': result={'protocolVersion':request.get('params',{}).get('protocolVersion','2024-11-05'),'capabilities':{'tools':{}},'serverInfo':{'name':'kiln-owned-smoke','version':'1'}}
 elif method=='tools/list': result={'tools':[]}
 elif method=='resources/list': result={'resources':[]}
 elif method=='prompts/list': result={'prompts':[]}
 else: result={}
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
with open(os.environ['KILN_SMOKE_MCP_EOF'],'a') as f: f.write(str(os.getpid())+'\n')
"#).unwrap();
    let mcp = json!({"owned-smoke":{"type":"stdio","command":python,"args":[helper],"env":{"KILN_SMOKE_MCP_PIDS":pid_file,"KILN_SMOKE_MCP_EOF":eof_file}}});
    let project_key = project.to_string_lossy().into_owned();
    let config_json = json!({"hasCompletedOnboarding":true,"theme":"dark","numStartups":1,
        "mcpServers":mcp,"projects":{project_key:{"hasTrustDialogAccepted":true,"enableAllProjectMcpServers":true,"allowedTools":[],"enabledMcpjsonServers":["owned-smoke"]}}});
    for path in [home.join(".claude.json"), profile.join(".claude.json")] {
        fs::write(path, serde_json::to_vec(&config_json).unwrap()).unwrap();
    }
    fs::write(
        profile.join("settings.json"),
        br#"{"permissions":{"allow":[]},"enableAllProjectMcpServers":true}"#,
    )
    .unwrap();
    let project_mcp = project.join(".mcp.json");
    let project_bytes = serde_json::to_vec(&json!({"mcpServers":mcp})).unwrap();
    fs::write(&project_mcp, &project_bytes).unwrap();
    let environment = vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        (
            "CLAUDE_CONFIG_DIR".into(),
            profile.to_string_lossy().into_owned(),
        ),
        (
            "PATH".into(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        ),
        ("TERM".into(), "xterm-256color".into()),
        ("LANG".into(), "en_US.UTF-8".into()),
        (
            "ANTHROPIC_API_KEY".into(),
            "sk-ant-api03-kiln-isolated-fixture-no-real-auth".into(),
        ),
        ("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        ("DISABLE_TELEMETRY".into(), "1".into()),
        ("DISABLE_ERROR_REPORTING".into(), "1".into()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        (
            "KILN_SMOKE_MCP_PIDS".into(),
            pid_file.to_string_lossy().into_owned(),
        ),
        (
            "KILN_SMOKE_MCP_EOF".into(),
            eof_file.to_string_lossy().into_owned(),
        ),
    ];
    let version = Process::new(&cli)
        .env_clear()
        .envs(environment.iter().cloned())
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("2.1.327"));
    let config = Config {
        descriptor: Descriptor {
            id: journal::unique(),
            tool: Tool::Claude,
            cwd: project,
            account_label: "Isolated native smoke".into(),
        },
        program: cli.clone(),
        environment,
        initial_prompt: None,
        launch_options: LaunchOptions::Inherited,
        shell: None,
    };
    let dir = root.join("agent-gui").join(&config.descriptor.id);
    let mut worker = spawn_worker(&config, &root);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ready = wait(
            &dir,
            Duration::from_secs(25),
            "initial Ready and owned MCP",
            |t| {
                t.lifecycle == Lifecycle::Ready
                    && t.provider_session_id.is_some()
                    && pid_file.exists()
            },
        );
        let native_id = ready.provider_session_id.as_ref().unwrap();
        assert_eq!(native_id.len(), 36);
        assert!(ready.entries.is_empty());
        let initial_mcp: u32 = fs::read_to_string(&pid_file)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();

        assert_eq!(
            unsafe { libc::getpgid(initial_mcp as i32) },
            worker.child.id() as i32
        );
        assert_eq!(
            unsafe { libc::getsid(initial_mcp as i32) },
            worker.child.id() as i32
        );
        if let Ok(report) = std::env::var("KILN_NATIVE_HANDOFF_SMOKE_REPORT") {
            let processes = handoff_processes::inspect(&owned_pids(worker.child.id())).unwrap();
            let details: Vec<_> = processes.iter().map(|process| json!({"pid":process.pid,"parent":process.parent,"stamp":process.stamp,"executable":process.executable,"argv":process.argv})).collect();
            fs::create_dir_all(&report).unwrap();
            fs::write(
                Path::new(&report).join("native-owned-processes.json"),
                serde_json::to_vec_pretty(&details).unwrap(),
            )
            .unwrap();
        }
        let switch = send(
            &dir,
            &ready,
            Command::SetPresentation(Presentation::Terminal),
        );
        let terminal = wait(&dir, Duration::from_secs(15), "terminal ownership", |t| {
            t.presentation == Presentation::Terminal
                || t.commands.get(&switch) == Some(&CommandPhase::Rejected)
        });
        assert_eq!(
            terminal.presentation,
            Presentation::Terminal,
            "native handoff rejected: {:?}",
            terminal.errors
        );
        let until = Instant::now() + Duration::from_secs(12);
        let native_pid = loop {
            let pids = owned_pids(worker.child.id());
            if let Ok(processes) = handoff_processes::inspect(&pids) {
                if let Some(process) = processes.iter().find(|process| {
                    process.executable.canonicalize().ok().as_ref() == Some(&cli)
                        && !process.argv.iter().any(|arg| arg == "-p")
                }) {
                    break process.pid;
                }
            }
            assert!(
                Instant::now() < until,
                "real native CLI process did not start"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        if let Ok(report) = std::env::var("KILN_NATIVE_HANDOFF_SMOKE_REPORT") {
            let processes = handoff_processes::inspect(&[native_pid]).unwrap();
            let process = &processes[0];
            fs::write(Path::new(&report).join("native-terminal-process.json"), serde_json::to_vec_pretty(&json!({"pid":process.pid,"parent":process.parent,"stamp":process.stamp,"executable":process.executable,"argv":process.argv})).unwrap()).unwrap();
        }
        assert_eq!(
            unsafe { libc::getpgid(initial_mcp as i32) },
            -1,
            "SDK EOF must finish its owned MCP before terminal launch"
        );
        // Wait for the real auth dialog and use its advertised Escape cancellation.
        // Ctrl-D during startup can be ignored by this dialog instead of exiting.
        let dialog_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let output = worker.output.lock().unwrap();
            let text = String::from_utf8_lossy(&output);
            if text.contains("ANTHROPIC_API_KEY") && text.contains("cancel") { break; }
            assert!(Instant::now() < dialog_deadline, "native auth cancellation was not rendered");
            drop(output);
            std::thread::sleep(Duration::from_millis(25));
        }
        worker.terminal.write_all(b"\x1b").unwrap();
        worker.terminal.flush().unwrap();
        let prompt_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let output = worker.output.lock().unwrap();
            if String::from_utf8_lossy(&output).contains("Not logged in") { break; }
            assert!(Instant::now() < prompt_deadline, "isolated native prompt was not rendered");
            drop(output);
            std::thread::sleep(Duration::from_millis(25));
        }
        wait(&dir, Duration::from_secs(12), "cooperative native return handshake", |t| {
            t.controls.status.get("native_terminal_return").and_then(|s|s.fields.get("automatic")) == Some(&ControlValue::Bool(true))
        });
        if guard_draft {
            worker.terminal.write_all(b"preserve this unsubmitted draft").unwrap();
            worker.terminal.flush().unwrap();
            std::thread::sleep(Duration::from_millis(400));
        }
        let request = send(&dir, &terminal, Command::SetPresentation(Presentation::Gui));
        wait(
            &dir,
            Duration::from_secs(5),
            "GUI request keeps native CLI alive",
            |t| t.return_requested && t.commands.get(&request) == Some(&CommandPhase::Accepted),
        );
        assert_eq!(
            unsafe { libc::getpgid(native_pid as i32) },
            worker.child.id() as i32
        );
        if guard_draft {
            std::thread::sleep(Duration::from_millis(900));
            let deferred = transcript(&dir);
            assert_eq!(deferred.presentation, Presentation::Terminal);
            assert!(deferred.return_requested);
            assert_eq!(unsafe {libc::getpgid(native_pid as i32)}, worker.child.id() as i32);
            let cancel = send(&dir, &deferred, Command::SetPresentation(Presentation::Terminal));
            wait(&dir, Duration::from_secs(5), "TUI cancels deferred return", |t| !t.return_requested && t.commands.get(&cancel) == Some(&CommandPhase::Accepted));
            // Clear only the fixture's own unsubmitted text, then prove cancellation
            // remains effective at the now-idle prompt without sending a model turn.
            worker.terminal.write_all(b"\x15").unwrap();
            worker.terminal.flush().unwrap();
            std::thread::sleep(Duration::from_millis(900));
            let cancelled = transcript(&dir);
            assert_eq!(cancelled.presentation, Presentation::Terminal);
            assert!(!cancelled.return_requested);
            assert_eq!(unsafe {libc::getpgid(native_pid as i32)}, worker.child.id() as i32);
            send(&dir, &cancelled, Command::SetPresentation(Presentation::Gui));
        }
        let resumed = wait(
            &dir,
            Duration::from_secs(25),
            "natural native EOF and GUI Ready",
            |t| {
                t.presentation == Presentation::Gui
                    && t.lifecycle == Lifecycle::Ready
                    && t.generation != ready.generation
            },
        );
        assert_eq!(resumed.provider_session_id, ready.provider_session_id);
        assert!(
            resumed.entries.is_empty(),
            "handoff must not send/replay user prompts"
        );
        assert!(resumed.errors.is_empty(), "normal native exit: {:?}", resumed.errors);
        assert!(
            String::from_utf8_lossy(&worker.output.lock().unwrap()).contains("ANTHROPIC_API_KEY"),
            "the real native auth confirmation was rendered"
        );
        assert_eq!(
            fs::read(&project_mcp).unwrap(),
            project_bytes,
            "tab switching never changes MCP preferences"
        );
        for path in [home.join(".claude.json"), profile.join(".claude.json")] {
            let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(value["mcpServers"], mcp);
        }
        println!(
            "native CLI 2.1.327: owned MCP {initial_mcp}; native PTY {native_pid}; same UUID {native_id}; Ready→native prompt→cooperative idle exit0→Ready; draft guard={guard_draft}; zero prompts"
        );
    }));
    if let Ok(report) = std::env::var("KILN_NATIVE_HANDOFF_SMOKE_REPORT") {
        let report = PathBuf::from(report);
        fs::create_dir_all(&report).unwrap();
        fs::write(
            report.join("native-claude-pty.txt"),
            worker.output.lock().unwrap().as_slice(),
        )
        .unwrap();
        if let Ok(events) = fs::read(dir.join("events.jsonl")) {
            fs::write(report.join("native-claude-events.jsonl"), events).unwrap();
        }
    }
    let fixture_pids: Vec<i32> = fs::read_to_string(&pid_file)
        .unwrap_or_default()
        .lines()
        .filter_map(|pid| pid.parse().ok())
        .collect();
    drop(worker);
    let until = Instant::now() + Duration::from_secs(3);
    while fixture_pids
        .iter()
        .any(|pid| unsafe { libc::getpgid(*pid) } != -1)
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(
        fixture_pids
            .iter()
            .all(|pid| unsafe { libc::getpgid(*pid) } == -1),
        "owned MCP fixtures must all exit during cleanup"
    );
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}
