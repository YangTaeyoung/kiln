//! Real migration from the published native V2 restore format to mixed PTYs.
//! The fixture owns every process/socket/FD; no installed app or user session is used.
#[cfg(unix)]
#[path = "support/upgrade.rs"]
mod upgrade;
#[cfg(unix)]
mod unix {
    use kiln_daemon::{
        client::Client,
        emu::{Dump, Emu},
        pty::Pty,
    };
    use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SpawnSpec};
    use serde::Serialize;
    use std::os::{
        fd::AsRawFd,
        unix::{net::UnixListener, process::CommandExt},
    };
    use std::{
        path::{Path, PathBuf},
        process::{Command, Command as Process, Stdio},
        time::{Duration, Instant},
    };

    // Independent consumer fixture for the immutable, published V2 wire format.
    #[derive(Serialize)]
    struct Native {
        info: SessionInfo,
        fd: i32,
        pid: i32,
        dump: Dump,
    }
    #[derive(Serialize)]
    struct Base {
        next_session: u64,
        listener_fd: i32,
        sessions: Vec<Native>,
    }
    #[derive(Serialize)]
    struct Images {
        session: u64,
        next: u32,
        images: Vec<(u32, kiln_daemon::images::Decoded)>,
    }
    #[derive(Serialize)]
    struct V2 {
        base: Base,
        images: Vec<Images>,
    }

    #[derive(Serialize)]
    struct Hosted {
        info: SessionInfo,
        endpoint: String,
        dump: Dump,
    }
    #[derive(Serialize)]
    struct V3 {
        base: Base,
        hosted: Vec<Hosted>,
        images: Vec<Images>,
    }

    fn fixture(root: &Path) -> ! {
        let socket = root.join("d.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let spec = SpawnSpec {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), shell().into()],
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let pty = Pty::spawn(&spec, 1).unwrap();
        kiln_daemon::pty::set_cloexec(pty.raw_fd(), false).unwrap();
        kiln_daemon::pty::set_cloexec(listener.as_raw_fd(), false).unwrap();
        let info = SessionInfo {
            id: 1,
            pid: pty.pid(),
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let state = V2 {
            base: Base {
                next_session: 2,
                listener_fd: listener.as_raw_fd(),
                sessions: vec![Native {
                    info,
                    fd: pty.raw_fd(),
                    pid: pty.pid() as i32,
                    dump: Emu::new(80, 24).dump(),
                }],
            },
            images: Vec::new(),
        };
        let path = root.join("restore-v2");
        let bytes = if root.join("missing-host").exists() {
            let mut base = state.base;
            base.next_session = 778;
            let missing = Hosted {
                info: SessionInfo {
                    id: 777,
                    pid: 0,
                    name: Some("missing fixture host".into()),
                    cols: 80,
                    rows: 24,
                    ..Default::default()
                },
                endpoint: root.join("absent.sock").to_string_lossy().into_owned(),
                dump: Emu::new(80, 24).dump(),
            };
            let mut bytes = b"KILNUP3\n".to_vec();
            bytes.extend(
                postcard::to_stdvec(&V3 {
                    base,
                    hosted: vec![missing],
                    images: Vec::new(),
                })
                .unwrap(),
            );
            bytes
        } else {
            let mut bytes = b"KILNUP2\n".to_vec();
            bytes.extend(postcard::to_stdvec(&state).unwrap());
            bytes
        };
        std::fs::write(&path, bytes).unwrap();
        let error = Process::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["daemon", "--socket"])
            .arg(&socket)
            .arg("--restore")
            .arg(&path)
            .env("KILN_SOCKET", &socket)
            .env("KILN_CONFIG_DIR", root.join("config"))
            .env("KILN_PTY_HOST", "1")
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .exec();
        panic!("fixture exec failed: {error}");
    }

    fn shell() -> &'static str {
        "stty -echo; printf 'READY=%s\\r\\n' \"$$\"; while IFS= read -r line; do case \"$line\" in exit42) exit 42;; size) stty size;; burst) i=0; while [ $i -lt 2000 ]; do printf 'SEQ%04d\\r\\n' $i; i=$((i+1)); done; printf 'BURST_DONE\\r\\n';; *) printf 'ECHO=%s\\r\\n' \"$line\";; esac; done"
    }

    struct Owner {
        root: tempfile::TempDir,
        child: std::process::Child,
        socket: String,
    }
    impl Owner {
        fn start() -> Self {
            Self::start_variant(false)
        }
        fn start_variant(missing: bool) -> Self {
            let root = tempfile::Builder::new()
                .prefix("kmix-")
                .tempdir_in("/tmp")
                .unwrap();
            let socket = root.path().join("d.sock").to_string_lossy().into_owned();
            if missing {
                std::fs::write(root.path().join("missing-host"), b"fixture").unwrap();
            }
            let child = Command::new(std::env::current_exe().unwrap())
                .arg("--fixture")
                .arg(root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                root,
                child,
                socket,
            }
        }
        fn client(&self) -> Client {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(c) = Client::connect(&self.socket, None) {
                    return c;
                }
                assert!(Instant::now() < deadline, "fixture daemon missing");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            if let Ok(c) = Client::connect(&self.socket, None) {
                for s in sessions(&c) {
                    c.send(ClientMsg::Kill { session: s.id });
                }
                c.send(ClientMsg::Shutdown);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    fn sessions(c: &Client) -> Vec<SessionInfo> {
        match c
            .request(
                |req| ClientMsg::ListSessions { req },
                Duration::from_secs(3),
            )
            .unwrap()
        {
            ServerMsg::Sessions { sessions, .. } => sessions,
            m => panic!("{m:?}"),
        }
    }
    fn text(c: &Client, id: u64) -> String {
        match c
            .request(
                |req| ClientMsg::ReadText {
                    req,
                    session: id,
                    history: 4000,
                },
                Duration::from_secs(3),
            )
            .unwrap()
        {
            ServerMsg::Text { text, .. } => text,
            m => panic!("{m:?}"),
        }
    }
    fn wait(c: &Client, id: u64, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !text(c, id).contains(needle) {
            assert!(
                Instant::now() < deadline,
                "missing {needle} in session {id}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn send(c: &Client, id: u64, line: &str) {
        c.send(ClientMsg::Input {
            session: id,
            data: format!("{line}\n").into_bytes(),
        });
    }
    pub fn run() {
        let args: Vec<_> = std::env::args_os().collect();
        if args.get(1).is_some_and(|s| s == "--fixture") {
            fixture(&PathBuf::from(&args[2]));
        }
        let owner = Owner::start();
        let c = owner.client();
        wait(&c, 1, "READY=");
        let native = sessions(&c)[0].pid;
        let daemon = c.server_pid;
        let spec = SpawnSpec {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), shell().into()],
            cols: 80,
            rows: 24,
            ..Default::default()
        };
        let host = match c
            .request(
                |req| ClientMsg::Create { req, spec },
                Duration::from_secs(10),
            )
            .unwrap()
        {
            ServerMsg::Created { session, .. } => session,
            m => panic!("{m:?}"),
        };
        wait(&c, host, "READY=");
        let host_pid = sessions(&c).into_iter().find(|s| s.id == host).unwrap().pid;
        send(&c, host, "burst");
        c.send(ClientMsg::Upgrade {
            req: 70,
            exe: env!("CARGO_BIN_EXE_kiln").into(),
        });
        super::upgrade::disconnected(&c, 70);
        drop(c);
        let c = owner.client();
        let rows = sessions(&c);
        assert_eq!(
            c.server_pid, daemon,
            "mixed migration must retain the native parent"
        );
        assert!(
            rows.iter().any(|s| s.id == 1 && s.pid == native),
            "native session lost in mixed upgrade"
        );
        assert!(
            rows.iter().any(|s| s.id == host && s.pid == host_pid),
            "host session lost in mixed upgrade"
        );
        wait(&c, host, "BURST_DONE");
        let output = text(&c, host);
        for i in 0..2000 {
            assert!(
                output.contains(&format!("SEQ{i:04}")),
                "host output lost at {i}"
            );
        }
        for id in [1, host] {
            send(&c, id, "after");
            wait(&c, id, "ECHO=after");
            c.send(ClientMsg::Resize {
                session: id,
                cols: 92,
                rows: 31,
            });
            send(&c, id, "size");
            wait(&c, id, "31 92");
        }
        // Failed exec must restore both the original writer Arc and reader.
        let invalid = owner.root.path().join("not-an-executable");
        std::fs::write(&invalid, b"not executable").unwrap();
        let response = c.request(
            |req| ClientMsg::Upgrade {
                req,
                exe: invalid.to_string_lossy().into_owned(),
            },
            Duration::from_secs(10),
        );
        // The upgrade closes clients before exec. Reconnect regardless of response.
        let _ = response;
        drop(c);
        let c = owner.client();
        for id in [1, host] {
            send(&c, id, "rollback");
            wait(&c, id, "ECHO=rollback");
            send(&c, id, "exit42");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let rows = sessions(&c);
            if [1, host]
                .iter()
                .all(|id| rows.iter().any(|s| s.id == *id && s.exited == Some(42)))
            {
                break;
            }
            assert!(Instant::now() < deadline, "exit ownership lost");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Completed hosted sessions outlive their host process and retain history.
        std::thread::sleep(Duration::from_millis(350));
        c.send(ClientMsg::Upgrade {
            req: 71,
            exe: env!("CARGO_BIN_EXE_kiln").into(),
        });
        super::upgrade::disconnected(&c, 71);
        drop(c);
        let c = owner.client();
        assert!(
            sessions(&c)
                .iter()
                .any(|s| s.id == host && s.exited == Some(42))
        );
        assert!(text(&c, host).contains("ECHO=rollback"));
        drop(c);

        // A missing optional endpoint cannot tear down inherited native masters;
        // its recovery record survives another successful same-PID replacement.
        let missing = Owner::start_variant(true);
        let c = missing.client();
        let pid = c.server_pid;
        wait(&c, 1, "READY=");
        send(&c, 1, "optional");
        wait(&c, 1, "ECHO=optional");
        assert!(
            kiln_daemon::ptyhost::load_registry(&missing.socket)
                .iter()
                .any(|r| r.session == 777)
        );
        c.send(ClientMsg::Upgrade {
            req: 72,
            exe: env!("CARGO_BIN_EXE_kiln").into(),
        });
        super::upgrade::disconnected(&c, 72);
        drop(c);
        let c = missing.client();
        assert_eq!(c.server_pid, pid);
        send(&c, 1, "retained");
        wait(&c, 1, "ECHO=retained");
        assert!(
            kiln_daemon::ptyhost::load_registry(&missing.socket)
                .iter()
                .any(|r| r.session == 777)
        );
        println!(
            "mixed migration, queued output, resize, rollback, exit ownership, completed history and optional host isolation passed"
        );
    }
}
fn main() {
    #[cfg(unix)]
    unix::run();
}
