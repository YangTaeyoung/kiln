//! Opt-in handover from an owned extraction of the previous protocol-6 release.
//! Every daemon, terminal and socket belongs to the fixture.
#![cfg(unix)]

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionInfo, SpawnSpec};
use std::{os::unix::fs::PermissionsExt, path::{Path, PathBuf}, process::{Child, Command, Stdio}, time::{Duration, Instant}};

#[path = "support/upgrade.rs"]
mod upgrade;

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
    let owner = Owner::start(&old, hosted);
    let client = owner.connect();
    let original_daemon = client.server_pid;
    let spec = SpawnSpec {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "echo PREVIOUS_READY; while IFS= read -r line; do printf 'ACK=%s\\n' \"$line\"; done".into()],
        cwd: Some(owner.root.path().to_string_lossy().into_owned()),
        cols: 80, rows: 24, ..Default::default()
    };
    let session = match client.request(|req| ClientMsg::Create { req, spec }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Created { session, .. } => session,
        message => panic!("{message:?}"),
    };
    wait_text(&client, session, "PREVIOUS_READY");
    let before = sessions(&client).into_iter().find(|s| s.id == session).unwrap();

    // A rejected request leaves the existing socket and shell available.
    let rejected = owner.root.path().join("older-helper");
    std::fs::write(&rejected, "#!/bin/sh\necho 'kiln 0.0.1'\n").unwrap();
    std::fs::set_permissions(&rejected, std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = client.request(|req| ClientMsg::Upgrade { req, exe: rejected.to_string_lossy().into_owned() }, Duration::from_secs(5)).unwrap_err();
    assert!(error.to_string().contains("older helper"), "{error}");
    assert!(matches!(client.request(|req| ClientMsg::Ping { req }, Duration::from_secs(2)).unwrap(), ServerMsg::Pong { .. }));
    wait_text(&client, session, "PREVIOUS_READY");

    let target = Path::new(env!("CARGO_BIN_EXE_kiln"));
    let req = client.next_req();
    client.send(ClientMsg::Upgrade { req, exe: target.to_string_lossy().into_owned() });
    upgrade::disconnected(&client, req);
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
