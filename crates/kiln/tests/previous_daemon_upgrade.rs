//! Opt-in handover from an owned extraction of the previous protocol-6 release.
//! Every daemon, terminal and socket belongs to the fixture.
#![cfg(unix)]

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SpawnSpec, read_msg, write_msg};
use std::{os::unix::{fs::PermissionsExt, net::UnixStream}, path::{Path, PathBuf}, process::{Child, Command, Stdio}, time::{Duration, Instant}};

struct Owner {
    root: tempfile::TempDir,
    child: Child,
}
impl Owner {
    fn socket(&self) -> String { self.root.path().join("d.sock").to_string_lossy().into_owned() }
    fn connect(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Client::connect(&self.socket(), None) {
                Ok(client) => return client,
                Err(error) => {
                    assert!(Instant::now() < deadline, "fixture daemon did not listen: {error}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
    fn start(old: &Path, hosted: bool) -> Self {
        let root = tempfile::Builder::new().prefix("kprev-").tempdir_in("/tmp").unwrap();
        let socket = root.path().join("d.sock");
        let log = std::fs::File::create(root.path().join("daemon.log")).unwrap();
        let child = Command::new(old).args(["daemon", "--socket"]).arg(&socket)
            .env("KILN_SOCKET", &socket).env("KILN_CONFIG_DIR", root.path().join("config"))
            .env("KILN_ACCOUNTS_SANDBOX", root.path().join("accounts"))
            .env("KILN_DB_NO_KEYCHAIN", "1").env("KILN_NO_AUTO_UPGRADE", "1")
            .env("KILN_PTY_HOST", if hosted { "1" } else { "0" })
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(log).spawn().unwrap();
        Self { root, child }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if let Ok(client) = Client::connect(&self.socket(), None) { client.send(ClientMsg::Shutdown); }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn connect_previous(owner: &Owner) -> (UnixStream, u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match UnixStream::connect(owner.socket()) {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(Instant::now() < deadline, "previous daemon did not listen: {error}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write_msg(&mut stream, &ClientMsg::Hello {
        proto: 6, build: "previous-release-fixture".into(), client: "fixture".into(),
    }).unwrap();
    let Some(ServerMsg::Hello { proto: 6, pid, .. }) = read_msg(&mut stream).unwrap() else {
        panic!("expected the previous release's protocol-6 daemon");
    };
    (stream, pid)
}

fn previous_request(stream: &mut UnixStream, req: u32, message: ClientMsg) -> ServerMsg {
    write_msg(stream, &message).unwrap();
    loop {
        let message = read_msg::<_, ServerMsg>(stream).unwrap().expect("previous daemon disconnected");
        let id = match &message {
            ServerMsg::Created { req, .. } | ServerMsg::Text { req, .. }
            | ServerMsg::Sessions { req, .. } | ServerMsg::Error { req, .. }
            | ServerMsg::Pong { req } => *req,
            _ => continue,
        };
        if id == req { return message; }
    }
}

fn sessions(client: &Client) -> Vec<SessionInfo> {
    match client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Sessions { sessions, .. } => sessions,
        message => panic!("{message:?}"),
    }
}
fn wait_text(client: &Client, session: u64, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let message = client.request(|req| ClientMsg::ReadText { req, session, history: 100 }, Duration::from_secs(2)).unwrap();
        if let ServerMsg::Text { text, .. } = message { if text.contains(needle) { return; } }
        assert!(Instant::now() < deadline, "fixture terminal did not produce {needle}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn verify(hosted: bool) {
    let Some(old) = std::env::var_os("KILN_TEST_PREVIOUS_EXE") else {
        eprintln!("KILN_TEST_PREVIOUS_EXE absent; previous release handover NOT verified");
        return;
    };
    let old = PathBuf::from(old).canonicalize().unwrap();
    let mut owner = Owner::start(&old, hosted);
    let (mut legacy, original_daemon) = connect_previous(&owner);
    let spec = SpawnSpec {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "echo PREVIOUS_READY; while IFS= read -r line; do printf 'ACK=%s\\n' \"$line\"; done".into()],
        cwd: Some(owner.root.path().to_string_lossy().into_owned()),
        cols: 80, rows: 24, ..Default::default()
    };
    let session = match previous_request(&mut legacy, 1, ClientMsg::Create { req: 1, spec }) {
        ServerMsg::Created { session, .. } => session,
        message => panic!("{message:?}"),
    };
    let ready = |stream: &mut UnixStream| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let msg = previous_request(stream, 2, ClientMsg::ReadText { req: 2, session, history: 100 });
            if matches!(msg, ServerMsg::Text { text, .. } if text.contains("PREVIOUS_READY")) { break; }
            assert!(Instant::now() < deadline, "previous shell did not print its marker");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    ready(&mut legacy);
    let ServerMsg::Sessions { sessions: previous, .. } = previous_request(
        &mut legacy, 3, ClientMsg::ListSessions { req: 3 },
    ) else { panic!("missing previous session inventory"); };
    let before = previous.into_iter().find(|s| s.id == session).unwrap();

    // A rejected request leaves the existing socket and shell available.
    let rejected = owner.root.path().join("older-helper");
    std::fs::write(&rejected, "#!/bin/sh\necho 'kiln 0.0.1'\n").unwrap();
    std::fs::set_permissions(&rejected, std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = previous_request(&mut legacy, 4, ClientMsg::Upgrade { req: 4, exe: rejected.to_string_lossy().into_owned() });
    assert!(matches!(error, ServerMsg::Error { message, .. } if message.contains("older helper")));
    assert!(matches!(previous_request(&mut legacy, 5, ClientMsg::Ping { req: 5 }), ServerMsg::Pong { .. }));
    ready(&mut legacy);

    let target = Path::new(env!("CARGO_BIN_EXE_kiln"));
    write_msg(&mut legacy, &ClientMsg::Upgrade { req: 6, exe: target.to_string_lossy().into_owned() }).unwrap();
    while let Some(message) = read_msg::<_, ServerMsg>(&mut legacy).unwrap() {
        if let ServerMsg::Error { req: 6, message } = message {
            panic!("upgrade failed before disconnect: {message}");
        }
    }
    if hosted {
        let deadline = Instant::now() + Duration::from_secs(10);
        while owner.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "previous hosted daemon did not stop");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let current = owner.connect();
    assert_eq!(current.server_build, kiln_daemon::exe_build_id(target));
    if hosted { assert_ne!(current.server_pid, original_daemon); }
    else { assert_eq!(current.server_pid, original_daemon); }
    let after = sessions(&current).into_iter().find(|s| s.id == session).unwrap();
    assert_eq!((after.pid, after.created_unix), (before.pid, before.created_unix), "terminal was replaced");
    assert!(after.exited.is_none());
    wait_text(&current, session, "PREVIOUS_READY");
    current.send(ClientMsg::Input { session, data: b"after-upgrade\r".to_vec() });
    wait_text(&current, session, "ACK=after-upgrade");
}
#[test]
fn previous_release_native_terminal_survives_rejection_and_retry() { verify(false); }
#[test]
fn previous_release_hosted_terminal_survives_rejection_and_retry() { verify(true); }
