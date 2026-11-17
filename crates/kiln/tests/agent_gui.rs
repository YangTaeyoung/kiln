//! Structured agents run under the actual isolated daemon and PTY owner.
use kiln::app::agent_gui::{
    AgentPane, Client, Command, CommandPhase, Decision, Descriptor, Entry, Envelope, Lifecycle,
    SavedDraft, ToolStatus, Transcript,
};
use kiln_accounts::Tool;
use kiln_daemon::client::Client as DaemonClient;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SessionInfo, SpawnSpec};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Runtime {
    root: tempfile::TempDir,
    socket: String,
    daemon: DaemonClient,
    program: PathBuf,
}
impl Runtime {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let socket = base.join("d.sock").to_string_lossy().into_owned();
        let python = std::env::var_os("KILN_TEST_PYTHON")
            .map(PathBuf::from)
            .or_else(|| kiln_common::paths::find_executable("python3", &[]))
            .expect("install Python 3 or set KILN_TEST_PYTHON");
        let script = base.join("provider.py");
        fs::write(&script, include_str!("fixtures/agent_gui.py")).unwrap();
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let program = base.join("provider");
            fs::write(
                &program,
                format!(
                    "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
                    python.display(),
                    script.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
            program
        };
        #[cfg(windows)]
        let program = {
            let program = base.join("provider.cmd");
            fs::write(
                &program,
                format!(
                    "@echo off\r\n\"{}\" \"{}\" %*\r\n",
                    python.display(),
                    script.display()
                ),
            )
            .unwrap();
            program
        };
        for name in ["home", "config", "codex", "claude"] {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        // This binary has one test: all process-global fixture paths are set once.
        unsafe {
            for (key, value) in [
                ("HOME", base.join("home")),
                ("USERPROFILE", base.join("home")),
                ("KILN_CONFIG_DIR", base.join("config")),
                ("KILN_SOCKET", base.join("d.sock")),
                ("CODEX_HOME", base.join("codex")),
                ("CLAUDE_CONFIG_DIR", base.join("claude")),
                ("KILN_ACCOUNTS_SANDBOX", base.to_owned()),
            ] {
                std::env::set_var(key, value);
            }
            std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
            std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
            for key in [
                "KILN_SESSION",
                "KILN_INTEGRATION_DIR",
                "KILN_MACOS_RESPONSIBILITY_PID",
                "KILN_ORIGINAL_ZDOTDIR",
                "KILN_AGENT_PROFILE",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "OPENAI_API_KEY",
                "GOOGLE_API_KEY",
            ] {
                std::env::remove_var(key);
            }
        }
        let daemon =
            DaemonClient::connect_or_spawn(&socket, Path::new(env!("CARGO_BIN_EXE_kiln")), None)
                .unwrap();
        Self {
            root,
            socket,
            daemon,
            program,
        }
    }
    fn spawn(&self, tool: Tool, mode: &str) -> (SpawnSpec, Descriptor, SessionId, PathBuf) {
        let id = format!("fixture-{}-{mode}", tool.key());
        let project = self.root.path().join(&id);
        fs::create_dir(&project).unwrap();
        let descriptor = Descriptor {
            id: id.clone(),
            tool,
            cwd: project.clone(),
            account_label: "Isolated fixture".into(),
        };
        let directory = self.root.path().join("config/agent-gui").join(&id);
        fs::create_dir_all(&directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = json!({"descriptor":descriptor,"program":self.program,"environment":[["FIXTURE_PROVIDER",tool.key()],["FIXTURE_MODE",mode]],"initial_prompt":if mode=="conversation" {Some("initial fixture task")} else {None}});
        fs::write(
            directory.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                directory.join("config.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let spec = SpawnSpec {
            program: Some(env!("CARGO_BIN_EXE_kiln").into()),
            args: vec!["agent-gui-host".into(), "--id".into(), id.clone()],
            cwd: Some(project.to_string_lossy().into_owned()),
            env: vec![
                ("KILN_GUI_AGENT".into(), id),
                ("KILN_GUI_TOOL".into(), tool.key().into()),
            ],
            ..Default::default()
        };
        let session = match self
            .daemon
            .request(
                |req| ClientMsg::Create {
                    req,
                    spec: spec.clone(),
                },
                Duration::from_secs(10),
            )
            .unwrap()
        {
            ServerMsg::Created { session, .. } => session,
            other => panic!("{other:?}"),
        };
        (spec, descriptor, session, directory)
    }
    fn session(&self, id: SessionId) -> SessionInfo {
        match self
            .daemon
            .request(
                |req| ClientMsg::ListSessions { req },
                Duration::from_secs(5),
            )
            .unwrap()
        {
            ServerMsg::Sessions { sessions, .. } => sessions
                .into_iter()
                .find(|s| s.id == id)
                .expect("PTY session retained"),
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Ok(client) = DaemonClient::connect(&self.socket, None) {
            client.send(ClientMsg::Shutdown);
        }
    }
}
fn wait(client: &mut Client, transcript: &mut Transcript, predicate: impl Fn(&Transcript) -> bool) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        for event in client.drain() {
            transcript.apply(&event);
        }
        if predicate(transcript) {
            return;
        }
        assert!(
            Instant::now() < until,
            "timeout: lifecycle={:?}, errors={:?}, requests={:?}",
            transcript.lifecycle,
            transcript.errors,
            transcript.requests
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn respond(client: &Client, request_id: String, decision: Decision) -> String {
    client
        .send(Command::Respond {
            request_id,
            decision,
            answers: Default::default(),
        })
        .unwrap()
}
fn wires(project: &Path) -> Vec<Value> {
    fs::read_to_string(project.join("wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn prompts(wires: &[Value]) -> usize {
    wires
        .iter()
        .filter(|v| {
            v["type"] == "user" || v["method"] == "turn/start" || v["method"] == "session/prompt"
        })
        .count()
}
fn decision<'a>(wire: &'a [Value], identity: &str) -> &'a Value {
    let matches = wire
        .iter()
        .filter(|v| v["response"]["request_id"] == identity || v["id"] == identity)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one wire response to {identity}");
    matches[0]
}

fn native_command_stop(runtime: &Runtime) {
    let (_, descriptor, _, directory) = runtime.spawn(Tool::Codex, "codex-process-stop");
    let project = descriptor.cwd.clone();
    let mut client = Client::connect(descriptor);
    let mut transcript = Transcript::default();
    wait(&mut client, &mut transcript, |t| {
        t.lifecycle == Lifecycle::Ready
    });
    let submit = |client: &Client, text: &str| {
        client
            .send(Command::Submit {
                text: text.into(),
                attachments: vec![],
                model: None,
            })
            .unwrap()
    };
    submit(&client, "previous background task");
    wait(&mut client, &mut transcript, |t| {
        t.lifecycle == Lifecycle::Ready
            && t.entries
                .iter()
                .any(|entry| matches!(entry,Entry::Tool(tool) if tool.id=="process-101"))
    });
    submit(&client, "active delayed task");
    wait(&mut client, &mut transcript, |t| {
        t.entries.iter().any(|entry|matches!(entry,Entry::Tool(tool) if tool.id=="process-102"&&tool.status==ToolStatus::Running))
    });
    client.send(Command::Interrupt).unwrap();
    wait(&mut client, &mut transcript, |t| {
        t.lifecycle==Lifecycle::Ready&&t.entries.iter().any(|entry|matches!(entry,Entry::Tool(tool) if tool.id=="process-102"&&tool.status==ToolStatus::Cancelled))
    });
    let until = Instant::now() + Duration::from_secs(2);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until || !project.join("background.txt").exists() {
        assert!(
            Instant::now() < deadline,
            "previous background task did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read(project.join("background.txt")).unwrap(),
        b"completed\n"
    );
    assert!(!project.join("late.txt").exists());
    assert!(transcript.entries.iter().any(|entry|matches!(entry,Entry::Tool(tool) if tool.id=="process-101"&&tool.status==ToolStatus::Running)));
    let last_sequence = transcript.sequence;
    submit(&client, "subsequent task");
    wait(&mut client, &mut transcript, |t| {
        t.sequence > last_sequence && t.lifecycle == Lifecycle::Ready
    });
    let wire = wires(&project);
    let terminate = wire
        .iter()
        .filter(|v| v["method"] == "thread/backgroundTerminals/terminate")
        .collect::<Vec<_>>();
    assert_eq!(terminate.len(), 1);
    assert_eq!(
        terminate[0]["params"],
        json!({"threadId":"fixture-codex","processId":"102"})
    );
    assert_eq!(prompts(&wire), 3);
    let events = fs::read_to_string(directory.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Envelope>(line).unwrap())
        .collect::<Vec<_>>();
    let stopping = events.iter().position(|event|matches!(event.event,kiln::app::agent_gui::Event::Message {ref message,..} if message.text=="active delayed task")).unwrap();
    let cancelled = events.iter().position(|event|matches!(event.event,kiln::app::agent_gui::Event::Tool(ref tool) if tool.id=="process-102"&&tool.status==ToolStatus::Cancelled)).unwrap();
    assert!(
        !events[stopping..cancelled]
            .iter()
            .any(|event| event.event == kiln::app::agent_gui::Event::Lifecycle(Lifecycle::Ready))
    );
    println!(
        "codex process Stop: exact current native process terminated, prior background completed, delayed side effect absent after 2s, next turn accepted"
    );
}
fn inflight_followups(runtime: &Runtime) {
    for tool in Tool::ALL {
        let (_, descriptor, session, _) = runtime.spawn(tool, "followup");
        let project = descriptor.cwd.clone();
        let mut client = Client::connect(descriptor);
        let mut transcript = Transcript::default();
        wait(&mut client, &mut transcript, |t|t.lifecycle == Lifecycle::Ready);
        let original_pid = runtime.session(session).pid;
        let first = client.send(Command::Submit { text: "first task".into(), attachments: vec![], model: None }).unwrap();
        wait(&mut client, &mut transcript, |t|t.commands.get(&first) == Some(&CommandPhase::Accepted));
        assert_eq!(transcript.lifecycle, Lifecycle::Running);
        let second = client.send(Command::Submit { text: "follow-up while running".into(), attachments: vec![], model: None }).unwrap();
        if tool == Tool::Antigravity {
            wait(&mut client, &mut transcript, |t|t.commands.get(&second) == Some(&CommandPhase::Dispatched));
            assert_eq!(prompts(&wires(&project)), 1);
            fs::write(project.join("finish-first"), "finish normally").unwrap();
        }
        wait(&mut client, &mut transcript, |t|t.commands.get(&second) == Some(&CommandPhase::Accepted) && t.lifecycle == Lifecycle::Ready);
        assert_eq!(runtime.session(session).pid, original_pid);
        assert_eq!(transcript.entries.iter().filter(|e|matches!(e, Entry::Message(m) if m.text=="follow-up while running")).count(), 1);
        let wire = wires(&project);
        assert!(!wire.iter().any(|v|v["method"]=="turn/interrupt" || v["method"]=="session/cancel" || v["request"]["subtype"]=="interrupt"));
        assert_eq!(prompts(&wire), if tool == Tool::Codex {1} else {2});
        if tool == Tool::Codex { assert_eq!(wire.iter().filter(|v|v["method"]=="turn/steer").count(), 1); }
    }
}

#[test]
fn daemon_pty_protocol_decisions_restore_stop_and_final_output() {
    let runtime = Runtime::new();
    for tool in Tool::ALL {
        let (spec, descriptor, session, directory) = runtime.spawn(tool, "conversation");
        let project = descriptor.cwd.clone();
        let mut client = Client::connect(descriptor.clone());
        let mut transcript = Transcript::default();
        wait(&mut client, &mut transcript, |t| !t.requests.is_empty());
        let original = runtime.session(session);
        assert!(original.pid > 0);
        assert_eq!(
            transcript.provider_session_id.as_deref(),
            Some(format!("fixture-{}", tool.key()).as_str())
        );
        let native = transcript.provider_session_id.clone();
        let generation = transcript.generation.clone();
        let mut pane = AgentPane::from_spec(&spec).unwrap();
        let mut saved = SavedDraft::default();
        saved.composer.text = "한글 unsent draft\n".into();
        pane.restore_draft(saved.clone());
        drop(pane);
        drop(client);
        let mut restored = AgentPane::from_spec(&spec).unwrap();
        restored.restore_draft(saved.clone());
        let ctx = egui::Context::default();
        restored.tick(&ctx);
        assert_eq!(restored.draft().composer, saved.composer);
        client = Client::connect(descriptor);
        transcript = Transcript::default();
        wait(&mut client, &mut transcript, |t| !t.requests.is_empty());
        assert_eq!(runtime.session(session).pid, original.pid);
        assert_eq!(transcript.provider_session_id, native);
        assert_eq!(transcript.generation, generation);
        assert_eq!(prompts(&wires(&project)), 1);
        let request = transcript.requests.keys().next().unwrap().clone();
        respond(&client, request, Decision::Deny);
        wait(&mut client, &mut transcript, |t| {
            t.lifecycle == Lifecycle::Ready && t.requests.is_empty()
        });
        assert!(!project.join("operation-1.txt").exists());
        client
            .send(Command::Submit {
                text: "allowed turn".into(),
                attachments: vec![],
                model: None,
            })
            .unwrap();
        wait(&mut client, &mut transcript, |t| !t.requests.is_empty());
        let request = transcript.requests.keys().next().unwrap().clone();
        respond(&client, request, Decision::Allow);
        wait(&mut client, &mut transcript, |t| {
            t.lifecycle == Lifecycle::Ready && t.requests.is_empty()
        });
        assert_eq!(
            fs::read_to_string(project.join("operation-2.txt")).unwrap(),
            "fixture result\n"
        );
        client
            .send(Command::Submit {
                text: "stopped turn".into(),
                attachments: vec![],
                model: None,
            })
            .unwrap();
        wait(&mut client, &mut transcript, |t| !t.requests.is_empty());
        let stopped = transcript.requests.keys().next().unwrap().clone();
        client.send(Command::Interrupt).unwrap();
        wait(&mut client, &mut transcript, |t| {
            t.lifecycle == Lifecycle::Ready && t.requests.is_empty()
        });
        let stopped_responses = wires(&project)
            .iter()
            .filter(|v| v["response"]["request_id"] == "operation-3" || v["id"] == "operation-3")
            .count();
        client
            .send(Command::Submit {
                text: "subsequent turn".into(),
                attachments: vec![],
                model: None,
            })
            .unwrap();
        wait(&mut client, &mut transcript, |t| !t.requests.is_empty());
        let live = transcript.requests.keys().next().unwrap().clone();
        let stale = respond(&client, stopped, Decision::Allow);
        wait(&mut client, &mut transcript, |t| {
            t.commands.get(&stale) == Some(&CommandPhase::Rejected)
        });
        assert!(transcript.requests.contains_key(&live));
        respond(&client, live, Decision::Deny);
        wait(&mut client, &mut transcript, |t| {
            t.lifecycle == Lifecycle::Ready && t.requests.is_empty()
        });
        assert!(!project.join("operation-3.txt").exists());
        assert!(!project.join("operation-4.txt").exists());
        let wire = wires(&project);
        assert_eq!(prompts(&wire), 4);
        assert_eq!(wire.iter().filter(|v|v["response"]["request_id"]=="operation-3"||v["id"]=="operation-3").count(), stopped_responses);
        for (identity, allowed) in [
            ("operation-1", false),
            ("operation-2", true),
            ("operation-4", false),
        ] {
            let response = decision(&wire, identity);
            match tool {
                Tool::Claude => assert_eq!(
                    response["response"]["response"]["behavior"],
                    if allowed { "allow" } else { "deny" }
                ),
                Tool::Codex => assert_eq!(
                    response["result"]["decision"],
                    if allowed { "accept" } else { "decline" }
                ),
                Tool::Antigravity => assert_eq!(
                    response["result"]["outcome"]["optionId"],
                    if allowed { "allow" } else { "deny" }
                ),
            }
        }
        assert!(!wire.iter().any(|v| v.get("unexpected_decision").is_some()));
        assert!(wire.iter().any(|v| v["request"]["subtype"] == "interrupt"
            || v["method"] == "turn/interrupt"
            || v["method"] == "session/cancel"));
        let events = fs::read_to_string(directory.join("events.jsonl")).unwrap();
        assert!(events.lines().map(|line|serde_json::from_str::<Envelope>(line).unwrap()).any(|e|matches!(e.event,kiln::app::agent_gui::Event::Tool(ref t) if t.output.contains("written"))));
        println!(
            "{}: daemon session={session}, worker PID={}, same native ID, draft, 4 prompts, allow/deny/Stop/stale verified",
            tool.key(),
            original.pid
        );
    }
    native_command_stop(&runtime);
    inflight_followups(&runtime);
    for tool in Tool::ALL {
        for mode in ["closed-input", "exit", "exit-held"] {
            if mode == "closed-input" && tool != Tool::Claude {
                continue;
            }
            if cfg!(windows) && mode == "exit-held" {
                continue;
            }
            let (_, descriptor, _, directory) = runtime.spawn(tool, mode);
            let project = descriptor.cwd.clone();
            let mut client = Client::connect(descriptor);
            let mut transcript = Transcript::default();
            let started = Instant::now();
            wait(&mut client, &mut transcript, |t| {
                t.lifecycle == Lifecycle::Disconnected
            });
            if mode == "closed-input" {
                assert!(project.join("emitted-final").exists());
            }
            assert_eq!(
                transcript.provider_session_id.as_deref(),
                Some(format!("fixture-{}", tool.key()).as_str())
            );
            assert!(started.elapsed() < Duration::from_secs(8));
            #[cfg(unix)]
            if mode == "exit-held" {
                assert!(
                    transcript
                        .errors
                        .iter()
                        .any(|error| error.contains("output drain timed out"))
                );
                let pid = fs::read_to_string(project.join("descendant.pid"))
                    .unwrap()
                    .parse::<i32>()
                    .unwrap();
                let until = Instant::now() + Duration::from_secs(3);
                while unsafe { libc::kill(pid, 0) } == 0 {
                    assert!(
                        Instant::now() < until,
                        "owned stdout descendant remains alive: {pid}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let events = fs::read_to_string(directory.join("events.jsonl")).unwrap();
            let events = events
                .lines()
                .map(|line| serde_json::from_str::<Envelope>(line).unwrap())
                .collect::<Vec<_>>();
            let session = events
                .iter()
                .position(|e| matches!(e.event, kiln::app::agent_gui::Event::Session(_)))
                .unwrap();
            let disconnected = events
                .iter()
                .position(|e| {
                    matches!(
                        e.event,
                        kiln::app::agent_gui::Event::Lifecycle(Lifecycle::Disconnected)
                    )
                })
                .unwrap();
            assert!(session < disconnected);
            println!(
                "{} {mode}: final session after {} frames retained before Disconnected in {:?}",
                tool.key(),
                if mode == "closed-input" { 512 } else { 1024 },
                started.elapsed()
            );
        }
    }
}
