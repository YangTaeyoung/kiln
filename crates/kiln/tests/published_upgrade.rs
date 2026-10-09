//! Opt-in compatibility gate against an explicitly supplied published protocol-5
//! executable. Only disposable sockets, configuration and shell processes are used.
#[cfg(unix)]
mod unix {
    use kiln_daemon::client::Client;
    use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SpawnSpec, read_msg, write_msg};
    use std::{
        os::unix::net::UnixStream,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    struct Legacy {
        stream: UnixStream,
        pid: u32,
        proto: u32,
    }
    impl Legacy {
        fn connect(socket: &Path) -> std::io::Result<Self> {
            let mut stream = UnixStream::connect(socket)?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            // Hello's index/fields and these pre-existing requests are unchanged
            // from v0.1.15; do not use current Client::connect for protocol 5.
            write_msg(
                &mut stream,
                &ClientMsg::Hello {
                    proto: 5,
                    build: "published-upgrade-fixture".into(),
                    client: "fixture".into(),
                },
            )?;
            let Some(ServerMsg::Hello { proto, pid, .. }) = read_msg(&mut stream)? else {
                return Err(std::io::Error::other("missing legacy Hello"));
            };
            Ok(Self { stream, pid, proto })
        }
        fn request(&mut self, msg: ClientMsg, req: u32) -> ServerMsg {
            write_msg(&mut self.stream, &msg).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let message: ServerMsg = read_msg(&mut self.stream)
                    .unwrap()
                    .expect("legacy disconnected");
                let response = match &message {
                    ServerMsg::Created { req, .. }
                    | ServerMsg::Sessions { req, .. }
                    | ServerMsg::Text { req, .. }
                    | ServerMsg::Error { req, .. } => Some(*req),
                    _ => None,
                };
                if response == Some(req) {
                    assert!(!matches!(message, ServerMsg::Error { .. }), "{message:?}");
                    return message;
                }
                assert!(Instant::now() < deadline, "legacy response {req} missing");
            }
        }
        fn text(&mut self, session: u64) -> String {
            match self.request(
                ClientMsg::ReadText {
                    req: 2,
                    session,
                    history: 1000,
                },
                2,
            ) {
                ServerMsg::Text { text, .. } => text,
                message => panic!("{message:?}"),
            }
        }
    }
    struct Owner {
        root: tempfile::TempDir,
        child: Child,
        session: Option<u64>,
        host_endpoints: Vec<String>,
    }
    impl Owner {
        fn socket(&self) -> PathBuf {
            self.root.path().join("d.sock")
        }
        fn start(old: &Path, hosted: bool) -> Self {
            let root = tempfile::Builder::new()
                .prefix("kpub-")
                .tempdir_in("/tmp")
                .unwrap();
            let socket = root.path().join("d.sock");
            let log = std::fs::File::create(root.path().join("daemon.log")).unwrap();
            let child = Command::new(old)
                .args(["daemon", "--socket"])
                .arg(&socket)
                .env("KILN_SOCKET", &socket)
                .env("KILN_CONFIG_DIR", root.path().join("config"))
                .env("KILN_PTY_HOST", if hosted { "1" } else { "0" })
                .env("KILN_NO_AUTO_UPGRADE", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .expect("explicit old daemon executable must start");
            Self {
                root,
                child,
                session: None,
                host_endpoints: Vec::new(),
            }
        }
        fn legacy(&mut self) -> Legacy {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(c) = Legacy::connect(&self.socket()) {
                    return c;
                }
                assert!(
                    self.child.try_wait().unwrap().is_none(),
                    "old daemon exited: {}",
                    self.log()
                );
                assert!(
                    Instant::now() < deadline,
                    "old daemon did not start: {}",
                    self.log()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        fn log(&self) -> String {
            std::fs::read_to_string(self.root.path().join("daemon.log")).unwrap_or_default()
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            // Both old/new daemons understand the unchanged control variants.
            if let Ok(mut c) = Legacy::connect(&self.socket()) {
                if let Some(session) = self.session {
                    let _ = write_msg(&mut c.stream, &ClientMsg::Kill { session });
                }
                let _ = write_msg(&mut c.stream, &ClientMsg::Shutdown);
            }
            // If a failed handover left only owned hosts, terminate those through
            // their own registry endpoints. Never inspect the user's registry.
            let mut endpoints = self.host_endpoints.clone();
            endpoints.extend(
                kiln_daemon::ptyhost::load_registry(self.socket().to_str().unwrap())
                    .into_iter()
                    .map(|h| h.endpoint),
            );
            endpoints.sort();
            endpoints.dedup();
            for endpoint in endpoints {
                if Path::new(&endpoint).parent() != Some(self.root.path()) {
                    continue;
                }
                if let Ok(mut stream) = UnixStream::connect(&endpoint) {
                    use kiln_daemon::ptyhost::{FromHost, ToHost};
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    if write_msg(&mut stream, &ToHost::Attach { replay: false }).is_ok()
                        && matches!(
                            read_msg::<_, FromHost>(&mut stream),
                            Ok(Some(FromHost::Hello { .. }))
                        )
                    {
                        let _ = write_msg(&mut stream, &ToHost::Kill);
                        // Keep this attachment alive to receive Exit. The host
                        // can then finish instead of retaining an undelivered exit.
                        let deadline = Instant::now() + Duration::from_secs(2);
                        while Instant::now() < deadline {
                            match read_msg::<_, FromHost>(&mut stream) {
                                Ok(Some(FromHost::Exit(_))) | Ok(None) | Err(_) => break,
                                Ok(Some(_)) => {}
                            }
                        }
                    }
                }
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // Child::kill targets only the direct process this fixture spawned.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    fn sessions(c: &Client) -> Vec<SessionInfo> {
        match c
            .request(
                |req| ClientMsg::ListSessions { req },
                Duration::from_secs(5),
            )
            .unwrap()
        {
            ServerMsg::Sessions { sessions, .. } => sessions,
            message => panic!("{message:?}"),
        }
    }
    fn text(c: &Client, session: u64) -> String {
        match c
            .request(
                |req| ClientMsg::ReadText {
                    req,
                    session,
                    history: 1000,
                },
                Duration::from_secs(5),
            )
            .unwrap()
        {
            ServerMsg::Text { text, .. } => text,
            message => panic!("{message:?}"),
        }
    }
    fn run(hosted: bool) {
        let Some(old) = std::env::var_os("KILN_TEST_OLD_EXE") else {
            eprintln!("KILN_TEST_OLD_EXE absent; published protocol-5 upgrade NOT verified");
            return;
        };
        let old = PathBuf::from(old)
            .canonicalize()
            .expect("explicit old executable must exist");
        let new = PathBuf::from(env!("CARGO_BIN_EXE_kiln"))
            .canonicalize()
            .unwrap();
        assert_ne!(
            old, new,
            "same-binary execution does not establish published compatibility"
        );
        let mut owner = Owner::start(&old, hosted);
        let mut legacy = owner.legacy();
        assert_eq!(
            legacy.proto, 5,
            "gate requires the published protocol-5 daemon"
        );
        let old_daemon = legacy.pid;
        let spec = SpawnSpec {
            cwd: Some(owner.root.path().to_string_lossy().into_owned()),
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), "stty -echo; printf 'READY=%s\\r\\n' \"$$\"; while IFS= read -r line; do printf 'ACK=%s\\r\\n' \"$line\"; done".into()],
            cols: 80, rows: 24, ..Default::default()
        };
        let session = match legacy.request(ClientMsg::Create { req: 1, spec }, 1) {
            ServerMsg::Created { session, .. } => session,
            message => panic!("{message:?}"),
        };
        owner.session = Some(session);
        owner.host_endpoints =
            kiln_daemon::ptyhost::load_registry(owner.socket().to_str().unwrap())
                .into_iter()
                .map(|h| h.endpoint)
                .collect();
        let deadline = Instant::now() + Duration::from_secs(10);
        let before = loop {
            let t = legacy.text(session);
            if t.lines().any(|line| line.trim().starts_with("READY=")) {
                break t;
            }
            assert!(Instant::now() < deadline, "old shell did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        };
        let shell: u32 = before
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("READY=")
                    .and_then(|pid| pid.parse().ok())
            })
            .unwrap();
        let old_info = match legacy.request(ClientMsg::ListSessions { req: 3 }, 3) {
            ServerMsg::Sessions { sessions, .. } => {
                sessions.into_iter().find(|s| s.id == session).unwrap()
            }
            message => panic!("{message:?}"),
        };
        assert_eq!(old_info.pid, shell);
        assert!(old_info.exited.is_none());
        // This exact public Client path detects protocol mismatch and asks the
        // old owner to upgrade; no manual Upgrade shortcut or synthetic restore.
        let current = Client::connect_or_spawn(owner.socket().to_str().unwrap(), &new, None)
            .unwrap_or_else(|error| panic!("published handover failed: {error}; {}", owner.log()));
        assert_eq!(kiln_proto::PROTO_VERSION, 6);
        if hosted {
            assert_ne!(current.server_pid, old_daemon);
        } else {
            assert_eq!(current.server_pid, old_daemon);
        }
        let after_info = sessions(&current)
            .into_iter()
            .find(|s| s.id == session)
            .expect("original session missing");
        assert_eq!(after_info.pid, shell, "shell was replaced");
        assert_eq!(after_info.created_unix, old_info.created_unix);
        assert!(after_info.exited.is_none());
        assert!(
            text(&current, session).contains(&format!("READY={shell}")),
            "old screen lost"
        );
        current.send(ClientMsg::Input {
            session,
            data: b"after-published-upgrade\r".to_vec(),
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if text(&current, session).contains("ACK=after-published-upgrade") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "retained shell I/O failed: {}",
                owner.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            sessions(&current)
                .into_iter()
                .find(|s| s.id == session)
                .unwrap()
                .pid,
            shell
        );
        current.send(ClientMsg::Kill { session });
        assert!(sessions(&current).iter().all(|s| s.id != session));
        current.send(ClientMsg::Shutdown);
        drop(current);
        let deadline = Instant::now() + Duration::from_secs(3);
        while owner.socket().exists() {
            assert!(
                Instant::now() < deadline,
                "owned upgraded daemon did not shut down"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn published_protocol_5_native_session_survives_protocol_6_upgrade() {
        run(false);
    }
    #[test]
    fn published_protocol_5_hosted_session_survives_protocol_6_upgrade() {
        run(true);
    }
}
