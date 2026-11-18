//! Upgrade regression: the original CLI is already running without a Kiln mod.
#![cfg(unix)]
use kiln::app::agent_gui::{self, Entry, Lifecycle, NativeImport, Transcript};
use kiln_accounts::Tool;
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SpawnSpec};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const ID: &str = "c6360000-1111-4222-8333-444444444444";
/// Loopback-only synthetic inference, needed because native background shell
/// completion can start a follow-up turn. No real account or network API is used.
struct LocalModel {
    endpoint: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl LocalModel {
    fn start() -> Self {
        use std::{
            io::{Read, Write},
            sync::{
                Arc, Mutex,
                atomic::{AtomicBool, Ordering},
            },
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let observed = prompts.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 8192];
                let mut header = None;
                let mut length = 0;
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if header.is_none() {
                        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            header = Some(index + 4);
                            length = String::from_utf8_lossy(&bytes[..index])
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|n| n.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                        }
                    }
                    if header.is_some_and(|h| bytes.len() >= h + length)
                        || bytes.len() > 2 * 1024 * 1024
                    {
                        break;
                    }
                }
                let request: serde_json::Value = header
                    .and_then(|h| serde_json::from_slice(&bytes[h..]).ok())
                    .unwrap_or_default();
                if let Some(messages) = request["messages"].as_array() {
                    for message in messages.iter().filter(|m| m["role"] == "user") {
                        if let Some(text) = message["content"].as_str() {
                            observed.lock().unwrap().push(text.into());
                        }
                        if let Some(content) = message["content"].as_array() {
                            for block in content {
                                if let Some(text) = block["text"].as_str() {
                                    observed.lock().unwrap().push(text.into());
                                }
                            }
                        }
                    }
                }
                let message = json!({"id":"msg_owned_loopback","type":"message","role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"Owned local fixture response."}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}});
                let (kind, body) = if request["stream"] == true {
                    let events = [
                        json!({"type":"message_start","message":{"id":"msg_owned_loopback","type":"message","role":"assistant","model":"claude-opus-4-6","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
                        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Owned local fixture response."}}),
                        json!({"type":"content_block_stop","index":0}),
                        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}),
                        json!({"type":"message_stop"}),
                    ];
                    (
                        "text/event-stream",
                        events
                            .iter()
                            .map(|e| {
                                format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)
                            })
                            .collect::<String>(),
                    )
                } else if bytes.starts_with(b"POST /v1/messages/count_tokens") {
                    ("application/json", json!({"input_tokens":1}).to_string())
                } else {
                    ("application/json", message.to_string())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            endpoint,
            stop,
            prompts,
            worker: Some(worker),
        }
    }
}
impl Drop for LocalModel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Runtime {
    _root: tempfile::TempDir,
    base: PathBuf,
    socket: String,
    client: Client,
}
impl Runtime {
    fn start() -> Self {
        let root = tempfile::Builder::new()
            .prefix("kiln-legacy-return-")
            .tempdir_in("/tmp")
            .unwrap();
        let base = root.path().canonicalize().unwrap();
        for name in ["home", "profile", "config", "accounts", "work"] {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        let socket = base.join("d.sock").to_string_lossy().into_owned();
        // This binary has one opt-in test and owns every process-global namespace.
        unsafe {
            for (key, value) in [
                ("HOME", base.join("home")),
                ("KILN_CONFIG_DIR", base.join("config")),
                ("KILN_SOCKET", base.join("d.sock")),
                ("KILN_ACCOUNTS_SANDBOX", base.join("accounts")),
            ] {
                std::env::set_var(key, value);
            }
            for key in [
                "KILN_SESSION",
                "KILN_INTEGRATION_DIR",
                "KILN_MACOS_RESPONSIBILITY_PID",
                "KILN_ORIGINAL_ZDOTDIR",
                "CLAUDE_CONFIG_DIR",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "KILN_NATIVE_RETURN_TOKEN",
                "KILN_NATIVE_RETURN_PLUGIN",
            ] {
                std::env::remove_var(key);
            }
            std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
            std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
            std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
            std::env::set_var("KILN_REMOTE_NO_KEYCHAIN", "1");
        }
        let client =
            Client::connect_or_spawn(&socket, Path::new(env!("CARGO_BIN_EXE_kiln")), None).unwrap();
        Self {
            _root: root,
            base,
            socket,
            client,
        }
    }
    fn create(&self, spec: SpawnSpec) -> u64 {
        match self
            .client
            .request(
                |req| ClientMsg::Create { req, spec },
                Duration::from_secs(10),
            )
            .unwrap()
        {
            ServerMsg::Created { session, .. } => session,
            other => panic!("{other:?}"),
        }
    }
    fn session(&self, id: u64) -> SessionInfo {
        match self
            .client
            .request(
                |req| ClientMsg::ListSessions { req },
                Duration::from_secs(3),
            )
            .unwrap()
        {
            ServerMsg::Sessions { sessions, .. } => {
                sessions.into_iter().find(|info| info.id == id).unwrap()
            }
            other => panic!("{other:?}"),
        }
    }
    fn text(&self, id: u64) -> String {
        match self
            .client
            .request(
                |req| ClientMsg::ReadText {
                    req,
                    session: id,
                    history: 0,
                },
                Duration::from_secs(3),
            )
            .unwrap()
        {
            ServerMsg::Text { text, .. } => text,
            other => panic!("{other:?}"),
        }
    }
    fn input(&self, id: u64, data: &[u8]) {
        self.client.send(ClientMsg::Input {
            session: id,
            data: data.into(),
        });
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let replacement = tempfile::tempdir().unwrap();
            let kept = std::mem::replace(&mut self._root, replacement).keep();
            eprintln!("kept failed fixture {}", kept.display());
        }
        if let Ok(ServerMsg::Sessions { sessions, .. }) = self.client.request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(2),
        ) {
            for session in sessions {
                if std::thread::panicking() {
                    let _ = fs::write(
                        self.base
                            .join(format!("failed-terminal-{}.txt", session.id)),
                        self.text(session.id),
                    );
                }
                self.client.send(ClientMsg::Kill {
                    session: session.id,
                });
            }
        }
        self.client.send(ClientMsg::Shutdown);
        std::thread::sleep(Duration::from_millis(350));
    }
}
fn wait(timeout: u64, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(timeout),
            "native upgrade scenario timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires installed native Claude; isolated accounts, real daemon and PTY, no model requests"]
fn existing_claude_without_a_mod_keeps_its_draft_and_cancels_then_returns_to_the_same_gui_conversation()
 {
    let cli = std::env::var_os("KILN_LEGACY_CLAUDE")
        .map(PathBuf::from)
        .expect("set KILN_LEGACY_CLAUDE to the old native CLI executable")
        .canonicalize()
        .unwrap();
    let current = kiln_common::paths::find_executable("claude", &[]).unwrap();
    if std::env::var_os("KILN_LEGACY_ONLY_GUI").is_none()
        && std::env::var_os("KILN_LEGACY_ONLY_BACKGROUND").is_none()
    {
        for seeded in [true, false] {
            scenario(&cli, &current, seeded, None, false);
        }
    }
    if let Some(old) = std::env::var_os("KILN_LEGACY_GUI_EXE") {
        scenario(&cli, &current, true, Some(Path::new(&old)), false);
    }
    if std::env::var_os("KILN_LEGACY_ONLY_GUI").is_none() {
        scenario(&cli, &current, true, None, true);
    }
}
fn scenario(cli: &Path, current: &Path, seeded: bool, old_gui: Option<&Path>, background: bool) {
    eprintln!("starting scenario seeded={seeded}");
    let runtime = Runtime::start();
    let model = background.then(LocalModel::start);
    let base = &runtime.base;
    let profile = base.join("profile");
    let project = base.join("work");
    let cfg = json!({"hasCompletedOnboarding":true,"theme":"dark","numStartups":1,"customApiKeyResponses":{"approved":["kiln-owned-test"]},"projects":{project.to_string_lossy().into_owned():{"hasTrustDialogAccepted":true}}});
    for path in [base.join("home/.claude.json"), profile.join(".claude.json")] {
        fs::write(path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    }
    let settings = b"{\"permissions\":{\"allow\":[]},\"alwaysThinkingEnabled\":true}\n";
    fs::write(profile.join("settings.json"), settings).unwrap();
    let original_keys =
        b"{\"bindings\":[],\"$docs\":\"https://code.claude.com/docs/en/keybindings\"}\n";
    fs::write(profile.join("keybindings.json"), original_keys).unwrap();
    let environment: Vec<(String, String)> = vec![
        (
            "HOME".into(),
            base.join("home").to_string_lossy().into_owned(),
        ),
        (
            "CLAUDE_CONFIG_DIR".into(),
            profile.to_string_lossy().into_owned(),
        ),
        ("PATH".into(), std::env::var("PATH").unwrap()),
        (
            "ZDOTDIR".into(),
            base.join("home").to_string_lossy().into_owned(),
        ),
        ("TERM".into(), "xterm-256color".into()),
        ("LANG".into(), "en_US.UTF-8".into()),
        ("ANTHROPIC_API_KEY".into(), "kiln-owned-test".into()),
        (
            "ANTHROPIC_BASE_URL".into(),
            model
                .as_ref()
                .map(|model| model.endpoint.clone())
                .unwrap_or_else(|| "http://127.0.0.1:9".into()),
        ),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("DISABLE_TELEMETRY".into(), "1".into()),
        ("DISABLE_ERROR_REPORTING".into(), "1".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
    ];
    let record = kiln::app::agent_runtime::Record {
        tool: Tool::Claude,
        spec: SpawnSpec {
            program: Some(current.to_string_lossy().into_owned()),
            cwd: Some(project.to_string_lossy().into_owned()),
            env: environment.clone(),
            ..Default::default()
        },
        version: cli.file_name().unwrap().to_string_lossy().into_owned(),
        conversation: Some(ID.into()),
        generation: "owned-legacy-launch".into(),
        confirmed: true,
        request_started: false,
    };
    let record_path = base.join("config/owned-record.json");
    fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
    if seeded {
        let history_dir = profile
            .join("projects")
            .join(project.to_string_lossy().replace('/', "-"));
        fs::create_dir_all(&history_dir).unwrap();
        let user_id = "c6360000-1111-4222-8333-555555555555";
        let rows = [
            json!({"type":"user","uuid":user_id,"parentUuid":null,"sessionId":ID,"cwd":project,"timestamp":"2026-11-18T00:00:00Z","message":{"role":"user","content":"owned original task"}}),
            json!({"type":"assistant","uuid":"c6360000-1111-4222-8333-666666666666","parentUuid":user_id,"sessionId":ID,"cwd":project,"timestamp":"2026-11-18T00:00:01Z","message":{"id":"owned-fixture-answer","type":"message","model":"claude-opus-4-6","role":"assistant","content":[{"type":"text","text":"owned original answer"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}),
        ];
        fs::write(
            history_dir.join(format!("{ID}.jsonl")),
            rows.iter()
                .map(|row| row.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
    }
    if let Some(old_gui) = old_gui {
        old_worker_scenario(&runtime, &record_path, cli, old_gui);
        return;
    }
    let native = runtime.create(SpawnSpec {
        program: Some(cli.to_string_lossy().into_owned()),
        args: vec![
            if seeded { "--resume" } else { "--session-id" }.into(),
            ID.into(),
            "--debug-file".into(),
            base.join("native-debug.log").to_string_lossy().into_owned(),
        ],
        cwd: record.spec.cwd.clone(),
        env: environment,
        cols: 140,
        rows: 45,
        ..Default::default()
    });
    wait(20, || runtime.text(native).contains('❯'));
    if runtime.text(native).contains("API key") {
        runtime.input(native, b"\x1b");
        std::thread::sleep(Duration::from_secs(1));
    }
    eprintln!("native ready seeded={seeded}");
    let pid = runtime.session(native).pid;
    if let Some(destination) = std::env::var_os("KILN_LEGACY_GUI_FIXTURE") {
        let mut launch = record.spec.clone();
        launch.program = Some(cli.to_string_lossy().into_owned());
        launch.env.push((
            "KILN_AGENT_RECORD".into(),
            record_path.to_string_lossy().into_owned(),
        ));
        launch
            .env
            .push(("KILN_AGENT_GENERATION".into(), record.generation.clone()));
        let state = json!({"workspaces":[{"name":"Owned legacy validation","root":project,
            "pages":[{"root":{"Leaf":1},"focused":1,"panes":[{"id":1,"session":native,"cwd":project,"launch":launch}],"title":"Existing native conversation"}],"active_page":0}],
            "active":0,"sidebar_open":true,"settings":{"agent_presentation":"Terminal","do_not_disturb":true,"os_notifications":false,"cli_auto_update":false}});
        fs::write(
            base.join("config/state.json"),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();
        fs::write(destination,serde_json::to_vec_pretty(&json!({"base":base,"socket":runtime.socket,"native_session":native,"native_pid":pid,"conversation":ID})).unwrap()).unwrap();
        eprintln!("owned GUI fixture ready {}", base.display());
        wait(1800, || base.join("finish-gui-fixture").exists());
        return;
    }
    let ctx = egui::Context::default();
    let mut second = if background {
        let started = base.join("shell-started");
        let release = base.join("release-shell");
        let completed = base.join("shell-completed");
        let snapshot = loop {
            match runtime
                .client
                .request(
                    |req| ClientMsg::ReadInputState {
                        req,
                        session: native,
                    },
                    Duration::from_secs(2),
                )
                .unwrap()
            {
                ServerMsg::InputState {
                    pid,
                    revision: Some(revision),
                    text,
                    ..
                } => break (pid, revision, text),
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        fs::write(
            project.join("owned-shell.sh"),
            format!(
                "touch '{}'; while [ ! -f '{}' ]; do sleep 0.1; done; touch '{}'\n",
                started.display(),
                release.display(),
                completed.display()
            ),
        )
        .unwrap();
        runtime.input(native, b"!");
        std::thread::sleep(Duration::from_millis(250));
        runtime.input(native, b"sh ./owned-shell.sh");
        std::thread::sleep(Duration::from_secs(1));
        runtime.input(native, b"\r");
        wait(10, || started.exists());
        let stale = runtime
            .client
            .request(
                |req| ClientMsg::InputIfUnchanged {
                    req,
                    session: native,
                    pid: snapshot.0,
                    revision: snapshot.1,
                    text: snapshot.2,
                    data: b"\x18\x06".to_vec(),
                },
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(
            matches!(
                stale,
                ServerMsg::InputAccepted {
                    accepted: false,
                    ..
                }
            ),
            "Native work started after the idle snapshot; stale control input must be rejected"
        );
        runtime.input(native, b"\x02");
        std::thread::sleep(Duration::from_millis(500));
        let mut import = NativeImport::new(
            &ctx,
            native,
            pid,
            record_path.clone(),
            runtime.socket.clone(),
        );
        // A foreground native shell may defer plugin reload itself. Require
        // preservation of the real process/work, then natural completion below.
        for _ in 0..80 {
            if let Some(error) = import.take_error() {
                panic!("background bootstrap: {error}");
            }
            assert_eq!(runtime.session(native).exited, None);
            assert!(!completed.exists());
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!completed.exists());
        assert_eq!(
            runtime.session(native).exited,
            None,
            "A live native shell must defer GUI return"
        );
        fs::write(&release, b"finish owned shell").unwrap();
        import
    } else {
        runtime.input(native, b"preserve this native draft");
        std::thread::sleep(Duration::from_millis(500));
        let mut first = NativeImport::new(
            &ctx,
            native,
            pid,
            record_path.clone(),
            runtime.socket.clone(),
        );
        eprintln!("waiting for draft bridge seeded={seeded}");
        wait(45, || {
            if let Some(error) = first.take_error() {
                panic!("legacy preparation: {error}");
            }
            first.label().contains("작성 중")
        });
        assert_eq!(runtime.session(native).pid, pid);
        assert_eq!(runtime.session(native).exited, None);
        assert!(runtime.text(native).contains("preserve this native draft"));
        first.request_cancel();
        wait(15, || first.cancelled());
        drop(first);
        wait(15, || {
            !base
                .join("config/native-imports")
                .read_dir()
                .unwrap()
                .any(|entry| entry.unwrap().path().join("probe/return.request").exists())
        });
        runtime.input(native, b"\x15");
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(
            runtime.session(native).exited,
            None,
            "Cancelling GUI before clearing the draft must preserve the original CLI"
        );
        NativeImport::new(
            &ctx,
            native,
            pid,
            record_path.clone(),
            runtime.socket.clone(),
        )
    };
    eprintln!("waiting for exit seeded={seeded}");
    wait(45, || {
        if let Some(error) = second.take_error() {
            panic!("legacy retry: {error}");
        }
        runtime.session(native).exited.is_some()
    });
    assert_eq!(runtime.session(native).exited, Some(0));
    if background {
        assert!(
            base.join("shell-completed").exists(),
            "GUI return must preserve the real shell through natural completion"
        );
    }
    wait(15, || {
        fs::read(profile.join("keybindings.json")).unwrap() == original_keys
    });
    wait(15, || !second.bridge_pending());
    assert!(second.take_error().is_none());
    let spec = second.prepare_task(&ctx).wait().unwrap().unwrap();
    let descriptor = agent_gui::descriptor(&spec).unwrap();
    let gui = runtime.create(spec);
    let mut view = agent_gui::Client::connect(descriptor);
    let mut transcript = Transcript::default();
    eprintln!("waiting for GUI ready seeded={seeded}");
    wait(30, || {
        for event in view.drain() {
            transcript.apply(&event);
        }
        transcript.lifecycle == Lifecycle::Ready
    });
    assert_eq!(transcript.provider_session_id.as_deref(), Some(ID));
    assert_eq!(runtime.session(gui).exited, None);
    let messages: Vec<_> = transcript
        .entries
        .iter()
        .filter_map(|entry| {
            if let Entry::Message(message) = entry {
                Some(message.text.as_str())
            } else {
                None
            }
        })
        .collect();
    let expected = if seeded {
        vec!["owned original task", "owned original answer"]
    } else {
        vec![]
    };
    if background {
        assert!(messages.starts_with(&expected));
        assert!(!messages.iter().any(|text| text.contains("reload-plugins")));
        assert!(
            !model
                .as_ref()
                .unwrap()
                .prompts
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.trim() == "/reload-plugins"),
            "No internal reload command may become a model prompt"
        );
    } else {
        assert_eq!(
            messages, expected,
            "No additional prompt or internal reload command is shown as a model conversation"
        );
    }
    assert!(transcript.errors.is_empty(), "{:?}", transcript.errors);
    drop(second);
    println!(
        "old CLI {}: draft preserved, cancellation preserved PID {pid}, native exit0, original shortcuts restored, same UUID {ID}, GUI Ready, seeded={seeded}, original history preserved; no internal reload model prompts; background inference uses only an owned loopback fixture",
        cli.file_name().unwrap().to_string_lossy()
    );
}

fn old_worker_scenario(runtime: &Runtime, record: &Path, cli: &Path, old_gui: &Path) {
    use agent_gui::{AgentPane, Presentation};
    let mut spec = agent_gui::prepare_existing_native(record).unwrap();
    let descriptor = agent_gui::descriptor(&spec).unwrap();
    let config_path = runtime
        .base
        .join("config/agent-gui")
        .join(&descriptor.id)
        .join("config.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["program"] = json!(cli);
    fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    spec.program = Some(old_gui.to_string_lossy().into_owned());
    let session = runtime.create(spec.clone());
    let pid = runtime.session(session).pid;
    let ctx = egui::Context::default();
    let mut pane = AgentPane::from_spec(&spec).unwrap();
    pane.set_transport_alive(true);
    wait(30, || {
        pane.tick(&ctx);
        pane.transcript.lifecycle == Lifecycle::Ready
    });
    assert_eq!(pane.transcript.provider_session_id.as_deref(), Some(ID));
    assert!(pane.request_presentation(Presentation::Terminal));
    wait(20, || {
        pane.tick(&ctx);
        pane.presentation() == Presentation::Terminal && runtime.text(session).contains('❯')
    });
    if runtime.text(session).contains("API key") {
        runtime.input(session, b"\x1b");
        std::thread::sleep(Duration::from_secs(1));
    }
    assert!(
        !pane
            .transcript
            .controls
            .status
            .contains_key("native_terminal_return"),
        "The fixture must use an actual worker predating the native bridge"
    );
    assert!(pane.request_presentation(Presentation::Gui));
    wait(10, || {
        pane.tick(&ctx);
        pane.transcript.return_requested
    });
    std::thread::sleep(Duration::from_secs(1));
    pane.tick(&ctx);
    assert_eq!(
        pane.presentation(),
        Presentation::Terminal,
        "Baseline old worker is still stuck after the ordinary GUI request"
    );
    eprintln!(
        "old worker root PID={pid}, native foreground={:?}",
        kiln_daemon::procinfo::foreground_pid(pid)
    );
    runtime.input(session, b"preserve old-worker native draft");
    std::thread::sleep(Duration::from_millis(500));
    pane.start_legacy_return(&ctx, session, pid, runtime.socket.clone());
    wait(45, || {
        pane.tick(&ctx);
        if let Some(error) = pane.legacy_return_error() {
            panic!("old worker draft: {error}");
        }
        pane.legacy_return_label()
            .is_some_and(|label| label.contains("작성 중"))
    });
    assert!(pane.request_presentation(Presentation::Terminal));
    wait(15, || {
        pane.tick(&ctx);
        !pane.presentation_pending()
            && !pane.transcript.return_requested
            && pane.legacy_return_label().is_none()
    });
    assert_eq!(runtime.session(session).exited, None);
    assert!(
        runtime
            .text(session)
            .contains("preserve old-worker native draft")
    );
    runtime.input(session, b"\x15");
    std::thread::sleep(Duration::from_millis(500));
    assert!(pane.request_presentation(Presentation::Gui));
    pane.start_legacy_return(&ctx, session, pid, runtime.socket.clone());
    wait(45, || {
        pane.tick(&ctx);
        if let Some(error) = pane.legacy_return_error() {
            panic!("old worker bootstrap: {error}");
        }
        pane.presentation() == Presentation::Gui && pane.transcript.lifecycle == Lifecycle::Ready
    });
    assert_eq!(
        runtime.session(session).pid,
        pid,
        "The old persistent worker and PTY were not restarted"
    );
    assert_eq!(pane.transcript.provider_session_id.as_deref(), Some(ID));
    assert!(
        pane.transcript.errors.is_empty(),
        "{:?}",
        pane.transcript.errors
    );
    let messages: Vec<_> = pane
        .transcript
        .entries
        .iter()
        .filter_map(|entry| {
            if let Entry::Message(message) = entry {
                Some(message.text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(messages, ["owned original task", "owned original answer"]);
    println!(
        "published GUI worker 0.3.4 + native CLI 2.1.326: baseline request remains Terminal, bootstrap returns same live PTY/worker PID {pid} and UUID {ID} to GUI Ready, original messages preserved"
    );
}
