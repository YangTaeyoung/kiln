//! The real daemon/protocol/ZLE bridge, isolated from the user's daemon and rc files.
#![cfg(unix)]
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, ShellCompletion, SpawnSpec};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedDaemon {
    child: Child,
    socket: String,
}
impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        if let Ok(c) = Client::connect(&self.socket, None) {
            c.send(ClientMsg::Shutdown);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Only the child spawned by this fixture, never a discovered daemon PID.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "fixture shell readiness timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn input(c: &Client, session: SessionId, data: &[u8]) {
    c.send(ClientMsg::Input {
        session,
        data: data.to_vec(),
    });
}
fn completion(c: &Client, session: SessionId, buffer: &str, explicit: bool) -> ShellCompletion {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let message =
            c.rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("expected isolated shell completion snapshot");
        if let ServerMsg::ShellCompletion {
            session: id,
            request: Some(r),
        } = message
            && id == session
            && r.buffer == buffer
            && r.explicit == explicit
        {
            return r;
        }
    }
}
fn apply(c: &Client, session: SessionId, revision: u64, buffer: &str) {
    c.send(ClientMsg::ApplyShellCompletion {
        session,
        revision,
        buffer: buffer.into(),
        cursor: buffer.chars().count(),
    });
}
fn hex(value: &str) -> String {
    value.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn daemon_completion_roundtrip_never_executes_and_rejects_stale_or_non_shell_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let socket = root.join("socket").to_string_lossy().into_owned();
    let child = Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["daemon", "--socket", &socket])
        .env("KILN_SOCKET", &socket)
        .env("KILN_CONFIG_DIR", root.join("config"))
        .env("KILN_PTY_HOST", "0")
        .env("HOME", root)
        .env("ZDOTDIR", root)
        .env("HISTFILE", root.join("unused-history"))
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut daemon = OwnedDaemon { child, socket };
    let deadline = Instant::now() + Duration::from_secs(10);
    let c = loop {
        if let Ok(c) = Client::connect(&daemon.socket, None) {
            break c;
        }
        assert!(
            daemon.child.try_wait().unwrap().is_none(),
            "fixture daemon exited"
        );
        assert!(
            Instant::now() < deadline,
            "fixture daemon startup timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let spec = SpawnSpec {
        program: Some("/bin/zsh".into()),
        args: vec!["-f".into()],
        cwd: Some(root.to_string_lossy().into_owned()),
        env: vec![
            ("HOME".into(), root.to_string_lossy().into_owned()),
            ("ZDOTDIR".into(), root.to_string_lossy().into_owned()),
            ("HISTFILE".into(), "/dev/null".into()),
            ("KILN_COMPLETION_PREVIEW".into(), "1".into()),
        ],
        cols: 100,
        rows: 24,
        ..Default::default()
    };
    let session = match c
        .request(
            |req| ClientMsg::Create { req, spec },
            Duration::from_secs(10),
        )
        .unwrap()
    {
        ServerMsg::Created { session, .. } => session,
        other => panic!("unexpected {other:?}"),
    };
    c.send(ClientMsg::Attach {
        session,
        cols: 100,
        rows: 24,
    });
    // Use the shipped script, not a fake OSC producer, for the positive path.
    std::fs::write(
        root.join("integration.zsh"),
        include_str!("../../kiln-daemon/src/shell-integration.zsh"),
    )
    .unwrap();
    input(&c,session,b"source ./integration.zsh; git() { : > executed; }; _fixture_tab() { LBUFFER+='native-tab'; }; zle -N _fixture_tab; bindkey '^I' _fixture_tab; : > ready\r");
    wait_file(&root.join("ready"));
    input(&c, session, b"git st");
    completion(&c, session, "git st", false);
    input(&c, session, b"\0");
    let first = completion(&c, session, "git st", true);
    assert_eq!(
        Path::new(&first.cwd).canonicalize().unwrap(),
        root.canonicalize().unwrap()
    );
    apply(&c, session, first.revision, "git status");
    // This redraw snapshot proves the daemon staged the response and ZLE applied it.
    completion(&c, session, "git status", false);
    assert!(
        !root.join("executed").exists(),
        "acceptance must not press Enter"
    );
    input(&c, session, b"\0");
    let current = completion(&c, session, "git status", true);
    assert_ne!(first.revision, current.revision);
    apply(&c, session, first.revision, "git rejected");
    // An ordered protocol barrier ensures the stale apply has been dispatched.
    c.request(|req| ClientMsg::Ping { req }, Duration::from_secs(3))
        .unwrap();
    assert!(
        !Path::new(&current.request_file).exists(),
        "stale revision must not stage text"
    );
    input(&c, session, b"\0");
    completion(&c, session, "git status", true);
    input(&c, session, b"\x15\t\0");
    completion(&c, session, "native-tab", true);
    assert!(!root.join("executed").exists());

    // A foreground non-Zsh program emitting a structurally valid private OSC cannot
    // gain a completion capability. This also exercises command-start invalidation.
    let payload = format!(
        "77;0;;{};{};;1",
        hex(root.to_str().unwrap()),
        hex(&current.request_file)
    );
    std::fs::write(root.join("foreground.sh"),format!(
        "#!/bin/sh\nprintf '\\033]777;kiln-complete;{}\\007'\n: > foreground-ready\nread hold\n",payload)).unwrap();
    while c.rx.try_recv().is_ok() {}
    input(&c, session, b"\x15/bin/sh ./foreground.sh\r");
    wait_file(&root.join("foreground-ready"));
    let deadline = Instant::now() + Duration::from_millis(300);
    while let Ok(message) =
        c.rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        assert!(
            !matches!(
                message,
                ServerMsg::ShellCompletion {
                    request: Some(_),
                    ..
                }
            ),
            "non-Zsh foreground must not publish completion metadata"
        );
    }
    assert!(!root.join("executed").exists());
    input(&c, session, b"\n");
    c.send(ClientMsg::Kill { session });
}
