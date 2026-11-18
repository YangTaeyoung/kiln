//! Ordinary shell commands use the real daemon, source CLI, injected zsh functions and PTY.
//! All agents/history/auth paths are owned fixtures; no model service or user daemon is used.
#![cfg(unix)]
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln::app::agent_gui::{self, Command, Entry, Lifecycle, Presentation, Role, Transcript};
use kiln::app::{Action, KilnApp};
use kiln_daemon::client::Client as DaemonClient;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SpawnSpec};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command as Process, Stdio},
    time::{Duration, Instant},
};

struct ConfigEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl ConfigEnv {
    fn install(base: &Path, socket: &str) -> Self {
        let values = vec![
            (
                "KILN_CONFIG_DIR",
                Some(base.join("config").into_os_string()),
            ),
            ("HOME", Some(base.join("home").into_os_string())),
            ("ZDOTDIR", Some(base.join("home").into_os_string())),
            ("KILN_SOCKET", Some(socket.into())),
            ("KILN_EXE", Some(env!("CARGO_BIN_EXE_kiln").into())),
            (
                "KILN_ACCOUNTS_SANDBOX",
                Some(base.join("accounts").into_os_string()),
            ),
            (
                "CLAUDE_CONFIG_DIR",
                Some(base.join("claude").into_os_string()),
            ),
            ("CODEX_HOME", Some(base.join("codex").into_os_string())),
            (
                "ANTIGRAVITY_APP_DATA_DIR",
                Some(base.join("antigravity").into_os_string()),
            ),
            (
                "ANTIGRAVITY_GEMINI_DIR",
                Some(base.join("home/.gemini").into_os_string()),
            ),
            (
                "KILN_ANTIGRAVITY_DIR",
                Some(base.join("antigravity").into_os_string()),
            ),
            (
                "XDG_CONFIG_HOME",
                Some(base.join("xdg-config").into_os_string()),
            ),
            (
                "XDG_CACHE_HOME",
                Some(base.join("xdg-cache").into_os_string()),
            ),
            (
                "XDG_DATA_HOME",
                Some(base.join("xdg-data").into_os_string()),
            ),
            (
                "PATH",
                Some(
                    format!(
                        "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                        base.join("bin").display()
                    )
                    .into(),
                ),
            ),
            ("SHELL", Some("/bin/zsh".into())),
            ("KILN_NO_AUTO_UPGRADE", Some("1".into())),
            ("KILN_DB_NO_KEYCHAIN", Some("1".into())),
            ("KILN_REMOTE_NO_KEYCHAIN", Some("1".into())),
            ("KILN_PTY_HOST", Some("0".into())),
            ("FIXTURE_ROOT", Some(base.as_os_str().to_owned())),
            ("ANTHROPIC_API_KEY", None),
            ("OPENAI_API_KEY", None),
            ("GOOGLE_API_KEY", None),
            ("GEMINI_API_KEY", None),
            ("CLAUDE_CODE_OAUTH_TOKEN", None),
            ("CLAUDECODE", None),
            ("CLAUDE_CODE_CHILD_SESSION", None),
            ("KILN_AGENT_RECORD", None),
            ("KILN_AGENT_GENERATION", None),
            ("KILN_AGENT_PROFILE", None),
            ("KILN_SESSION", None),
            ("KILN_INTEGRATION_DIR", None),
            ("KILN_ORIGINAL_ZDOTDIR", None),
            ("KILN_AGENT_LAUNCHER", None),
            ("KILN_MACOS_RESPONSIBILITY_PID", None),
        ];
        let saved = values
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        // This integration target contains one test; install before it creates
        // application/client workers, and restore after the harness is dropped.
        for (key, value) in values {
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
        Self(saved)
    }
}
impl Drop for ConfigEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.iter().rev() {
            unsafe {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
}
struct Fixture {
    root: tempfile::TempDir,
    daemon: Child,
    socket: String,
}
impl Fixture {
    fn start() -> Self { Self::with_native_claude(None) }
    fn with_native_claude(native: Option<&Path>) -> Self {
        let root = tempfile::Builder::new()
            .prefix("kiln-shell-gui-")
            .tempdir_in("/tmp")
            .unwrap();
        let base = root.path().canonicalize().unwrap();
        for name in [
            "home",
            "bin",
            "work",
            "claude",
            "codex",
            "antigravity",
            "config",
        ] {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        fs::write(base.join("home/.zshrc"), "PROMPT='OWNED_SHELL> '\n").unwrap();
        let python = kiln_common::paths::find_executable("python3", &[]).expect("Python fixture");
        for tool in ["claude", "codex", "agy"] {
            let path = base.join("bin").join(tool);
            fs::write(&path, format!("#!{}\n{}", python.display(), PROVIDER)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        if let Some(cli) = native {
            fs::write(base.join("bin/claude"), format!("#!/bin/sh\nexec {} \"$@\"\n", quote(cli))).unwrap();
            let project = base.join("work").to_string_lossy().into_owned();
            let onboarding = serde_json::json!({"hasCompletedOnboarding":true,"theme":"dark","numStartups":1,
                "projects":{project:{"hasTrustDialogAccepted":true,"allowedTools":[]}}});
            for path in [base.join("home/.claude.json"),base.join("claude/.claude.json")] {
                fs::write(path, serde_json::to_vec(&onboarding).unwrap()).unwrap();
            }
        }
        let socket = base.join("d.sock").to_string_lossy().into_owned();
        let log = fs::File::create(base.join("daemon.log")).unwrap();
        let mut command = Process::new(env!("CARGO_BIN_EXE_kiln"));
        command.env_clear();
        if native.is_some() {
            command.env("ANTHROPIC_API_KEY", "sk-ant-api03-kiln-isolated-fixture-no-real-auth")
                .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
                .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
                .env("DISABLE_TELEMETRY", "1").env("DISABLE_ERROR_REPORTING", "1").env("DISABLE_AUTOUPDATER", "1");
        }
        let daemon = command
            .args(["daemon", "--socket", &socket])
            .env("HOME", base.join("home"))
            .env("ZDOTDIR", base.join("home"))
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    base.join("bin").display()
                ),
            )
            .env("SHELL", "/bin/zsh")
            .env("TERM", "xterm-256color")
            .env("CLAUDE_CONFIG_DIR", base.join("claude"))
            .env("CODEX_HOME", base.join("codex"))
            .env("ANTIGRAVITY_APP_DATA_DIR", base.join("antigravity"))
            .env("KILN_ANTIGRAVITY_DIR", base.join("antigravity"))
            .env("KILN_SOCKET", &socket)
            .env("KILN_CONFIG_DIR", base.join("config"))
            .env("KILN_EXE", env!("CARGO_BIN_EXE_kiln"))
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .env("KILN_DB_NO_KEYCHAIN", "1")
            .env("KILN_REMOTE_NO_KEYCHAIN", "1")
            .env("KILN_PTY_HOST", "0")
            .env("FIXTURE_ROOT", &base)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        Self {
            root,
            daemon,
            socket,
        }
    }
    fn base(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap()
    }
    fn connect(&mut self) -> DaemonClient {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(client) = DaemonClient::connect(&self.socket, None) {
                return client;
            }
            assert!(
                self.daemon.try_wait().unwrap().is_none(),
                "owned daemon exited: {}",
                fs::read_to_string(self.base().join("daemon.log")).unwrap_or_default()
            );
            assert!(Instant::now() < until, "owned daemon did not listen");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // This pre-existing background job intentionally survives the foreground
        // host. Tear down only that recorded PID in this owned shell session.
        let number = |name: &str| {
            fs::read_to_string(self.base().join(name))
                .ok()?
                .trim()
                .parse::<i32>()
                .ok()
        };
        if let (Some(background), Some(shell)) = (number("background.pid"), number("shell.pid")) {
            if background > 1
                && shell > 1
                && unsafe {
                    libc::getsid(background) == shell && libc::getpgid(background) == background
                }
            {
                unsafe {
                    libc::kill(background, libc::SIGTERM);
                }
            }
        }
        if let Ok(report) = std::env::var("KILN_SHELL_GUI_REPORT") {
            let report = PathBuf::from(report);
            let _ = fs::create_dir_all(&report);
            for name in [
                "daemon.log",
                "claude-launches.jsonl",
                "codex-launches.jsonl",
                "agy-launches.jsonl",
                "claude-submitted.jsonl",
                "codex-submitted.jsonl",
                "agy-submitted.jsonl",
            ] {
                let source = self.base().join(name);
                if source.is_file() {
                    let _ = fs::copy(source, report.join(name));
                }
            }
            if let Ok(directories) = fs::read_dir(self.base().join("config/agent-gui")) {
                for directory in directories.flatten() {
                    let source = directory.path().join("events.jsonl");
                    if source.is_file() {
                        let _ = fs::copy(
                            source,
                            report.join(format!(
                                "{}-events.jsonl",
                                directory.file_name().to_string_lossy()
                            )),
                        );
                    }
                }
            }
        }
        if let Ok(client) = DaemonClient::connect(&self.socket, None) {
            client.send(ClientMsg::Shutdown);
        }
        let until = Instant::now() + Duration::from_secs(4);
        while self.daemon.try_wait().ok().flatten().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.daemon.try_wait().ok().flatten().is_none() {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }
}
fn input(client: &DaemonClient, session: SessionId, text: &str) {
    client.send(ClientMsg::Input {
        session,
        data: text.as_bytes().to_vec(),
    });
}
fn line(client: &DaemonClient, session: SessionId, text: &str) {
    input(client, session, &format!("{text}\r"));
}
fn text(client: &DaemonClient, session: SessionId) -> String {
    match client
        .request(
            |req| ClientMsg::ReadText {
                req,
                session,
                history: 1000,
            },
            Duration::from_secs(3),
        )
        .unwrap()
    {
        ServerMsg::Text { text, .. } => text,
        message => panic!("{message:?}"),
    }
}
fn wait_text(client: &DaemonClient, session: SessionId, needle: &str) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let output = text(client, session);
        if output.contains(needle) {
            return;
        }
        assert!(Instant::now() < until, "missing {needle}: {output}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn binding(session: SessionId) -> SpawnSpec {
    let until = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(spec) = agent_gui::shell_binding(session).unwrap() {
            return spec;
        }
        assert!(
            Instant::now() < until,
            "ordinary shell agent did not publish a binding"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn no_binding(session: SessionId) {
    let until = Instant::now() + Duration::from_secs(6);
    loop {
        if agent_gui::shell_binding(session).unwrap().is_none() {
            return;
        }
        assert!(
            Instant::now() < until,
            "completed shell command retained a live binding"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn pump(
    client: &mut agent_gui::Client,
    state: &mut Transcript,
    ready: impl Fn(&Transcript) -> bool,
) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        for event in client.drain() {
            state.apply(&event);
        }
        if ready(state) {
            return;
        }
        assert!(
            Instant::now() < until,
            "agent state timeout: {:?}, {:?}, {:?}",
            state.lifecycle,
            state.presentation,
            state.errors
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn pump_ui(h: &mut Harness<'_, KilnApp>, mut ready: impl FnMut(&Harness<'_, KilnApp>) -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(
            Instant::now() < until,
            "shell GUI timeout; focused session={:?}, terminal={:?}, draft={:?}",
            h.state().debug_focused_session(),
            h.state().debug_focused_text(),
            h.state().debug_focused_agent_draft()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn dismiss_toasts(h: &mut Harness<'_, KilnApp>) {
    // Completion notifications from the earlier CLI lanes can legitimately
    // overlay the top-right tabs. Dismiss them through their real UI control.
    for _ in 0..8 {
        let clicked = if let Some(close) = h.query_all_by_label("알림 닫기").next() {
            close.click();
            true
        } else {
            false
        };
        if !clicked {
            return;
        }
        h.run_steps(3);
    }
    assert!(h.query_all_by_label("알림 닫기").next().is_none());
}
fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
fn logs(root: &Path, tool: &str) -> Vec<Value> {
    fs::read_to_string(root.join(format!("{tool}-launches.jsonl")))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
#[ignore = "requires installed Claude 2.1.327; owned shell and localhost-only dummy credentials"]
fn installed_claude_initial_shell_returns_to_gui_when_idle_without_manual_exit() {
    let cli = kiln_common::paths::find_executable("claude", &[]).unwrap().canonicalize().unwrap();
    let mut fixture = Fixture::with_native_claude(Some(&cli));
    let base = fixture.base();
    let _env = ConfigEnv::install(&base, &fixture.socket);
    let daemon = fixture.connect();
    let session = match daemon.request(|req| ClientMsg::Create {req, spec: SpawnSpec {
        program:Some("/bin/zsh".into()), cwd:Some(base.join("work").to_string_lossy().into_owned()), cols:120, rows:40, ..Default::default()
    }}, Duration::from_secs(5)).unwrap() {ServerMsg::Created {session,..}=>session, value=>panic!("{value:?}")};
    wait_text(&daemon, session, "OWNED_SHELL>");
    line(&daemon, session, "claude");
    let spec = binding(session);
    let mut client = agent_gui::Client::connect(agent_gui::descriptor(&spec).unwrap());
    let mut state = Transcript::default();
    wait_text(&daemon, session, "ANTHROPIC_API_KEY");
    input(&daemon, session, "\x1b");
    wait_text(&daemon, session, "Not logged in");
    pump(&mut client, &mut state, |s| s.provider_session_id.is_some()
        && s.controls.status.get("native_terminal_return").and_then(|v|v.fields.get("automatic")) == Some(&agent_gui::ControlValue::Bool(true)));
    let native_id = state.provider_session_id.clone();
    client.send(Command::SetPresentation(Presentation::Gui)).unwrap();
    pump(&mut client, &mut state, |s| s.presentation == Presentation::Gui && s.lifecycle == Lifecycle::Ready);
    assert_eq!(state.provider_session_id, native_id);
    assert!(state.entries.is_empty(), "No prompt was submitted or replayed");
    assert!(state.errors.is_empty(), "{:?}", state.errors);
    println!("installed Claude shell bootstrap → native idle → GUI Ready; same UUID {:?}; zero prompts; no manual exit", native_id);
    daemon.send(ClientMsg::Kill {session});
}

#[test]
fn ordinary_agents_keep_parent_shell_background_jobs_identity_and_native_modes() {
    let mut fixture = Fixture::start();
    let base = fixture.base();
    let _env = ConfigEnv::install(&base, &fixture.socket);
    let daemon = fixture.connect();
    let session = match daemon
        .request(
            |req| ClientMsg::Create {
                req,
                spec: SpawnSpec {
                    program: Some("/bin/zsh".into()),
                    cwd: Some(base.join("work").to_string_lossy().into_owned()),
                    cols: 120,
                    rows: 40,
                    ..Default::default()
                },
            },
            Duration::from_secs(5),
        )
        .unwrap()
    {
        ServerMsg::Created { session, .. } => session,
        message => panic!("{message:?}"),
    };
    wait_text(&daemon, session, "OWNED_SHELL>");
    line(
        &daemon,
        session,
        &format!(
            "echo $$ > {}; sleep 120 & echo $! > {}; echo INIT_$((20+22))",
            quote(&base.join("shell.pid")),
            quote(&base.join("background.pid"))
        ),
    );
    wait_text(&daemon, session, "INIT_42");
    let shell: u32 = fs::read_to_string(base.join("shell.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let background: u32 = fs::read_to_string(base.join("background.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    for tool in ["claude", "codex", "agy"] {
        line(&daemon, session, tool);
        let spec = binding(session);
        let descriptor = agent_gui::descriptor(&spec).unwrap();
        let mut client = agent_gui::Client::connect(descriptor.clone());
        let mut state = Transcript::default();
        pump(&mut client, &mut state, |s| {
            s.presentation == Presentation::Terminal && s.provider_session_id.is_some()
        });
        let native_id = state.provider_session_id.clone().unwrap();
        assert_eq!(native_id.len(), 36);
        let host: Value = serde_json::from_slice(
            &fs::read(
                base.join("config/agent-gui")
                    .join(&descriptor.id)
                    .join("config.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let host_pid = host["shell"]["host_pid"].as_u64().unwrap() as u32;
        assert_ne!(host_pid, shell);
        assert_eq!(unsafe { libc::getpgid(host_pid as i32) }, host_pid as i32);
        assert_ne!(unsafe { libc::getpgid(background as i32) }, host_pid as i32);
        client
            .send(Command::SetPresentation(Presentation::Gui))
            .unwrap();
        pump(&mut client, &mut state, |s| s.return_requested);
        input(&daemon, session, "exit-native\r");
        pump(&mut client, &mut state, |s| {
            s.presentation == Presentation::Gui && s.lifecycle == Lifecycle::Ready
        });
        assert_eq!(state.provider_session_id.as_ref(), Some(&native_id));
        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(state.entries.iter().filter(|entry| matches!(entry, Entry::Message(m) if m.role == Role::User && m.text == format!("original question {tool}"))).count(), 1, "{tool} history: {:?}", state.entries);
        let generation = state.generation.clone();
        client
            .send(Command::SetPresentation(Presentation::Terminal))
            .unwrap();
        pump(&mut client, &mut state, |s| {
            s.presentation == Presentation::Terminal
        });
        wait_text(&daemon, session, &format!("NATIVE_RESUMED_{tool}"));
        client
            .send(Command::SetPresentation(Presentation::Gui))
            .unwrap();
        pump(&mut client, &mut state, |s| s.return_requested);
        input(&daemon, session, "exit-native\r");
        pump(&mut client, &mut state, |s| {
            s.presentation == Presentation::Gui
                && s.lifecycle == Lifecycle::Ready
                && s.generation != generation
        });
        assert_eq!(state.provider_session_id.as_ref(), Some(&native_id));
        assert_eq!(state.entries.iter().filter(|entry| matches!(entry, Entry::Message(m) if m.role == Role::User && m.text == format!("terminal follow-up {tool}"))).count(), 1, "{tool} follow-up history: {:?}", state.entries);
        let launches = logs(&base, tool);
        assert_eq!(launches.len(), 4, "two native and two SDK starts");
        assert!(launches.iter().all(|launch| launch["id"] == native_id
            && launch["cwd"] == base.join("work").to_string_lossy().as_ref()));
        assert!(
            launches
                .iter()
                .all(|launch| launch["profile"] == launches[0]["profile"])
        );
        assert!(
            launches[0]["record"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            launches[2]["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == &native_id)
        );
        assert!(
            !base.join(format!("{tool}-submitted.jsonl")).exists(),
            "switching must never replay/submit user messages"
        );
        assert!(alive(shell) && alive(background));
        unsafe {
            assert_eq!(libc::kill(host_pid as i32, libc::SIGHUP), 0);
        }
        no_binding(session);
        line(&daemon, session, &format!("echo PARENT_{tool}_$((20+22))"));
        wait_text(&daemon, session, &format!("PARENT_{tool}_42"));
        assert!(
            alive(shell) && alive(background),
            "foreground host exit must not signal the parent shell or its earlier server"
        );
        // No GUI request: native return code belongs to the original shell.
        line(
            &daemon,
            session,
            &format!("{tool}; echo NATIVE_STATUS_{tool}:$?"),
        );
        let spec = binding(session);
        let descriptor = agent_gui::descriptor(&spec).unwrap();
        let mut client = agent_gui::Client::connect(descriptor);
        let mut state = Transcript::default();
        pump(&mut client, &mut state, |s| {
            s.presentation == Presentation::Terminal && s.provider_session_id.is_some()
        });
        input(&daemon, session, "return-7\r");
        wait_text(&daemon, session, &format!("NATIVE_STATUS_{tool}:7"));
        no_binding(session);
        for (index, command) in if tool == "claude" {
            vec!["claude -p fixture", "claude auth status"]
        } else if tool == "codex" {
            vec!["codex exec fixture", "codex login status"]
        } else {
            vec!["agy -p fixture", "agy update", "agy acp"]
        }
        .iter()
        .enumerate()
        {
            let before = logs(&base, tool).len();
            let host_count = || fs::read_dir(base.join("config/agent-gui")).unwrap().count();
            let hosts_before = host_count();
            line(
                &daemon,
                session,
                &format!("{command}; echo MGMT_{tool}_{index}:$?"),
            );
            wait_text(&daemon, session, &format!("MGMT_{tool}_{index}:6"));
            assert!(agent_gui::shell_binding(session).unwrap().is_none());
            assert_eq!(
                host_count(),
                hosts_before,
                "native management must never create even a transient GUI host"
            );
            assert_eq!(
                logs(&base, tool).len(),
                before,
                "noninteractive/auth commands must not create a GUI host"
            );
        }
    }
    line(&daemon, session, "echo FINAL_PARENT_$((20+22))");
    wait_text(&daemon, session, "FINAL_PARENT_42");
    assert!(alive(shell) && alive(background));
    // Exercise the actual frontend binding and archived draft path in this same
    // isolated daemon/session, after all three low-level provider lanes completed.
    let mut h = Harness::builder()
        .with_size([1100., 880.])
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(base.join("work"))));
    let ctx = h.ctx.clone();
    h.run_steps(3);
    h.state_mut()
        .debug_apply_action(&ctx, Action::AttachSession(session));
    pump_ui(&mut h, |h| {
        h.state().debug_focused_session() == Some(session)
    });
    line(&daemon, session, "claude");
    let joined = binding(session);
    let descriptor = agent_gui::descriptor(&joined).unwrap();
    let archived_id = descriptor.id.clone();
    pump_ui(&mut h, |h| {
        h.state().debug_focused_session() == Some(session)
            && h.state().debug_focused_agent_draft().is_some()
            && h.query_by_label("GUI")
                .is_some_and(|button| !button.accesskit_node().is_disabled())
            && h.query_by_label("TUI").is_some_and(|button| {
                button.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
            })
    });
    h.run_steps(4);
    dismiss_toasts(&mut h);
    if let Ok(report) = std::env::var("KILN_SHELL_GUI_REPORT") {
        fs::create_dir_all(&report).unwrap();
        fs::write(Path::new(&report).join("ui-shell-tabs-before-click.txt"), format!("session={:?}\nGUI rect={:?} disabled={} selected={:?}\nTUI rect={:?} disabled={} selected={:?}\nspec={:?}\n", h.state().debug_focused_session(), h.get_by_label("GUI").rect(), h.get_by_label("GUI").accesskit_node().is_disabled(), h.get_by_label("GUI").accesskit_node().toggled(), h.get_by_label("TUI").rect(), h.get_by_label("TUI").accesskit_node().is_disabled(), h.get_by_label("TUI").accesskit_node().toggled(), h.state().debug_focused_launch_spec())).unwrap();
        h.render()
            .unwrap()
            .save(Path::new(&report).join("ui-shell-tabs-before-click.png"))
            .unwrap();
    }
    let pane = h.state().debug_focused_pane_id().unwrap();
    dismiss_toasts(&mut h);
    h.get_by_label("GUI").click();
    let mut observer = agent_gui::Client::connect(descriptor.clone());
    let mut state = Transcript::default();
    let until = Instant::now() + Duration::from_secs(12);
    loop {
        h.step();
        for event in observer.drain() {
            state.apply(&event);
        }
        if state.return_requested {
            break;
        }
        assert!(
            Instant::now() < until,
            "the GUI tab did not request the original native host; GUI disabled={} selected={:?} TUI selected={:?}; actual={:?}/{:?} error={:?}",
            h.get_by_label("GUI").accesskit_node().is_disabled(),
            h.get_by_label("GUI").accesskit_node().toggled(),
            h.get_by_label("TUI").accesskit_node().toggled(),
            state.presentation,
            state.lifecycle,
            state.errors
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let native_id = state.provider_session_id.clone().unwrap();
    input(&daemon, session, "exit-native\r");
    pump_ui(&mut h, |h| {
        h.query_by_label("준비됨").is_some() && h.query_by_label("메시지").is_some()
    });
    pump(&mut observer, &mut state, |s| {
        s.lifecycle == Lifecycle::Ready && s.presentation == Presentation::Gui
    });
    assert_eq!(state.provider_session_id.as_ref(), Some(&native_id));
    h.get_by_label("메시지").click();
    h.run_steps(2);
    let korean = "보존할 한글 초안  \n두 번째 줄";
    h.event(egui::Event::Text(korean.into()));
    pump_ui(&mut h, |h| {
        h.state()
            .debug_focused_agent_draft()
            .is_some_and(|draft| draft.composer.text == korean)
    });
    let expected = h.state().debug_focused_agent_draft().unwrap();
    let launch_spec = h.state().debug_focused_launch_spec().unwrap();
    let marker = |spec: &SpawnSpec| {
        spec.env
            .iter()
            .find(|(key, _)| key == "KILN_SHELL_GUI_AGENT")
            .map(|(_, value)| value.clone())
    };
    assert_eq!(marker(&launch_spec).as_deref(), Some(archived_id.as_str()));
    let config: Value = serde_json::from_slice(
        &fs::read(
            base.join("config/agent-gui")
                .join(&archived_id)
                .join("config.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let host_pid = config["shell"]["host_pid"].as_u64().unwrap() as u32;
    let launches_before_exit = logs(&base, "claude").len();
    unsafe {
        assert_eq!(libc::kill(host_pid as i32, libc::SIGHUP), 0);
    }
    no_binding(session);
    let banner =
        "에이전트가 종료되어 초안을 보존했습니다. TUI에서 원래 셸을 계속 사용할 수 있습니다.";
    pump_ui(&mut h, |h| {
        h.query_by_label(banner).is_some()
            && h.query_by_label("보내기")
                .is_some_and(|button| button.accesskit_node().is_disabled())
    });
    assert!(alive(shell) && alive(background));
    assert_eq!(
        h.state().debug_focused_agent_draft(),
        Some(expected.clone())
    );
    assert_eq!(h.state().debug_focused_session(), Some(session));
    if let Ok(report) = std::env::var("KILN_SHELL_GUI_REPORT") {
        fs::create_dir_all(&report).unwrap();
        h.render()
            .unwrap()
            .save(Path::new(&report).join("shell-draft-detached.png"))
            .unwrap();
    }
    dismiss_toasts(&mut h);
    h.get_by_label("TUI").click();
    h.run_steps(5);
    line(&daemon, session, "echo READONLY_PARENT_$((20+22))");
    pump_ui(&mut h, |h| {
        h.state()
            .debug_focused_text()
            .is_some_and(|text| text.contains("READONLY_PARENT_42"))
    });
    assert_eq!(
        h.state().debug_focused_agent_draft(),
        Some(expected.clone())
    );
    assert_eq!(h.state().debug_focused_session(), Some(session));
    // Simulate app reconstruction after the host has died. The old exact ID is
    // restored read-only; neither the parent shell nor a new conversation owns it.
    h.state_mut().debug_checkpoint_restore(&ctx);
    pump_ui(&mut h, |h| {
        h.state().debug_focused_session() == Some(session)
            && h.state().debug_focused_agent_draft() == Some(expected.clone())
            && h.query_by_label(banner).is_some()
    });
    assert_eq!(h.state().debug_focused_pane_id(), Some(pane));
    assert_eq!(
        marker(&h.state().debug_focused_launch_spec().unwrap()).as_deref(),
        Some(archived_id.as_str())
    );
    assert!(h.get_by_label("보내기").accesskit_node().is_disabled());
    assert!(agent_gui::shell_binding(session).unwrap().is_none());
    dismiss_toasts(&mut h);
    h.get_by_label("TUI").click();
    h.run_steps(5);
    line(&daemon, session, "echo RESTORED_PARENT_$((20+22))");
    pump_ui(&mut h, |h| {
        h.state()
            .debug_focused_text()
            .is_some_and(|text| text.contains("RESTORED_PARENT_42"))
    });
    dismiss_toasts(&mut h);
    h.get_by_label("GUI").click();
    pump_ui(&mut h, |h| {
        h.query_by_label(banner).is_some()
            && h.state().debug_focused_agent_draft() == Some(expected.clone())
    });
    assert!(h.get_by_label("보내기").accesskit_node().is_disabled());
    assert_eq!(
        logs(&base, "claude").len(),
        launches_before_exit,
        "read-only previews/restoration must never create another provider"
    );
    assert!(!base.join("claude-submitted.jsonl").exists());
    assert!(alive(shell) && alive(background));
    if let Ok(report) = std::env::var("KILN_SHELL_GUI_REPORT") {
        h.render()
            .unwrap()
            .save(Path::new(&report).join("shell-draft-restored.png"))
            .unwrap();
    }
    drop(observer);
    drop(h);
    daemon.send(ClientMsg::Kill { session });
}

const PROVIDER: &str = r#"import datetime,json,os,pathlib,subprocess,sys,uuid,sqlite3,fcntl
root=pathlib.Path(os.environ['FIXTURE_ROOT']);tool=pathlib.Path(sys.argv[0]).name;args=sys.argv[1:]
assert args.count('--no-daemon')<=1,'ordinary shell invocation duplicated --no-daemon'
if '--version' in args: print('2.1.326' if tool=='claude' else '0.162.1');sys.exit(0)
if tool=='agy' and args==['acp'] and os.isatty(0): print('NATIVE_ACP_BYPASS',flush=True);sys.exit(6)
structured=('--input-format' in args) if tool=='claude' else ('app-server' in args) if tool=='codex' else ('acp' in args)
if not structured and (('-p' in args or (args and args[0] in ['auth','login','exec','review','update']))):
 print('NATIVE_MANAGEMENT_'+tool,flush=True);sys.exit(6)
profile=pathlib.Path(os.environ['CLAUDE_CONFIG_DIR' if tool=='claude' else 'CODEX_HOME' if tool=='codex' else 'ANTIGRAVITY_APP_DATA_DIR'])
record=os.environ.get('KILN_AGENT_RECORD','')
def value(flag):
 return args[args.index(flag)+1] if flag in args else None
cid=value('--resume') or value('resume') or value('--session-id') or value('--conversation')
if cid is None and record:
 cid=json.loads(pathlib.Path(record).read_text()).get('conversation')
cid=cid or str(uuid.uuid4());now=datetime.datetime.now(datetime.timezone.utc).isoformat()
if tool=='claude': path=profile/'projects'/str(pathlib.Path.cwd()).replace('/','-')/(cid+'.jsonl')
elif tool=='agy': path=profile/'brain'/cid/'.system_generated'/'logs'/'transcript_full.jsonl'
else: path=profile/'sessions'/'2026'/'11'/'17'/('rollout-fixture-'+cid+'.jsonl')
def append_pair(question,answer):
 path.parent.mkdir(parents=True,exist_ok=True)
 parent=json.loads(path.read_text().splitlines()[-1]).get('uuid') if tool=='claude' and path.exists() and path.stat().st_size else None
 with path.open('a') as out:
  if tool=='codex' and path.stat().st_size==0:out.write(json.dumps({'timestamp':now,'type':'session_meta','payload':{'id':cid,'cwd':os.getcwd(),'timestamp':now,'originator':'codex_cli_rs','cli_version':'0.162.1','source':'cli'}})+'\n')
  for role,text in [('user',question),('assistant',answer)]:
   if tool=='codex' and role=='user':out.write(json.dumps({'timestamp':now,'type':'event_msg','payload':{'type':'user_message','message':text,'images':[],'local_images':[]}})+'\n')
   if tool=='claude':
    row={'type':role,'uuid':str(uuid.uuid4()),'parentUuid':parent,'sessionId':cid,'cwd':os.getcwd(),'timestamp':now,'message':{'role':role,'content':[{'type':'text','text':text}],'stop_reason':'end_turn' if role=='assistant' else None}};parent=row['uuid']
   elif tool=='agy': row={'step_index':sum(1 for _ in path.open())+(0 if role=='user' else 1),'source':'USER_EXPLICIT' if role=='user' else 'MODEL','type':'USER_INPUT' if role=='user' else 'PLANNER_RESPONSE','status':'DONE','created_at':now,'content':'<USER_REQUEST>\n'+text+'\n</USER_REQUEST>' if role=='user' else text}
   else: row={'timestamp':now,'type':'response_item','payload':{'type':'message','id':str(uuid.uuid4()),'role':role,'content':[{'type':'input_text' if role=='user' else 'output_text','text':text}]}}
   out.write(json.dumps(row)+'\n')
  if tool=='codex':out.write(json.dumps({'timestamp':now,'type':'event_msg','payload':{'type':'task_complete','turn_id':str(uuid.uuid4())}})+'\n')
if tool=='agy' and not structured:
 (profile/'conversations').mkdir(parents=True,exist_ok=True);(profile/'conversations'/(cid+'.db')).touch()
 db=sqlite3.connect(profile/'conversation_summaries.db');db.execute('CREATE TABLE IF NOT EXISTS conversation_summaries(conversation_id TEXT PRIMARY KEY,workspace_uris TEXT,parent_conversation_id TEXT)');db.execute('INSERT OR REPLACE INTO conversation_summaries VALUES (?,?,?)',(cid,json.dumps([pathlib.Path.cwd().as_uri()]),''));db.commit();db.close()
 (profile/'presence').mkdir(parents=True,exist_ok=True);presence=(profile/'presence'/(cid+'.lock')).open('a+');fcntl.flock(presence,fcntl.LOCK_EX|fcntl.LOCK_NB)
if not path.exists() and not structured:append_pair('original question '+tool,'original answer '+tool)
def audit():
 with (root/(tool+'-launches.jsonl')).open('a') as out:out.write(json.dumps({'id':cid,'args':args,'cwd':os.getcwd(),'profile':str(profile),'record':record,'structured':structured,'pid':os.getpid()})+'\n')
if not structured or tool=='claude':audit()
def emit(value):print(json.dumps(value),flush=True)
if not structured:
 if record and tool!='agy':
  payload={'hook_event_name':'SessionStart','session_id':cid,'cwd':os.getcwd(),'transcript_path':str(path)}
  hook=subprocess.run([os.environ['KILN_EXE'],'agent-hook'],input=json.dumps(payload),text=True,capture_output=True)
  assert hook.returncode==0,hook.stderr
  assert not hook.stdout.strip(),hook.stdout
 print(('NATIVE_RESUMED_' if '--resume' in args or 'resume' in args or '--conversation' in args else 'NATIVE_INITIAL_')+tool,flush=True)
 for line in sys.stdin:
  if line.strip()=='return-7':sys.exit(7)
  if line.strip()=='exit-native':
   if '--resume' in args or 'resume' in args or '--conversation' in args:append_pair('terminal follow-up '+tool,'terminal answer '+tool)
   sys.exit(0)
 sys.exit(0)
for line in sys.stdin:
 request=json.loads(line);method=request.get('method','')
 if tool=='claude' and request.get('type')=='control_request':
  emit({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':{'models':[{'value':'fixture','displayName':'Fixture'}]} if request['request']['subtype']=='initialize' else {}}})
  if request['request']['subtype']=='initialize':emit({'type':'system','subtype':'init','session_id':cid,'model':'fixture'})
 elif tool=='codex' and 'id' in request:
  if method=='thread/resume' or method=='thread/start':
   cid=request.get('params',{}).get('threadId',cid);audit();result={'thread':{'id':cid},'model':'fixture'}
  elif method=='model/list':result={'data':[]}
  else:result={}
  emit({'id':request['id'],'result':result})
 elif tool=='agy' and 'id' in request:
  if method=='initialize':result={'protocolVersion':1,'agentCapabilities':{'loadSession':True,'promptCapabilities':{'image':True}}}
  elif method=='session/load':
   cid=request['params']['sessionId'];audit();result={'sessionId':cid,'models':{'currentModelId':'fixture','availableModels':[{'modelId':'fixture','name':'Fixture'}]}}
  else:result={}
  emit({'jsonrpc':'2.0','id':request['id'],'result':result})
 if request.get('type')=='user' or method in ['turn/start','session/prompt']:
  with (root/(tool+'-submitted.jsonl')).open('a') as out:out.write(json.dumps(request)+'\n')
"#;
