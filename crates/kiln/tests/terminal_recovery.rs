//! Isolated PTY regression for an interrupted TUI synchronized redraw.
//! It never connects to the user's daemon or sends input to real agents.
#![cfg(unix)]

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SpawnSpec, read_msg, write_msg};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    child: Child,
    socket: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn start(hosted: bool) -> (Self, Client) {
        let dir = tempfile::Builder::new().prefix("kiln-sync-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("daemon.sock").to_string_lossy().into_owned();
        let child = Command::new(env!("CARGO_BIN_EXE_kiln"))
            .args(["daemon", "--foreground", "--socket", &socket])
            .env("KILN_SOCKET", &socket)
            .env("KILN_PTY_HOST", if hosted { "1" } else { "0" })
            .env("KILN_CONFIG_DIR", dir.path().join("config"))
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().unwrap();
        let fixture = Self { child, socket, _dir: dir };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(client) = Client::connect(&fixture.socket, None) { return (fixture, client); }
            assert!(Instant::now() < deadline, "isolated daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this fixture's private socket and owned child are touched.
        if let Ok(client) = Client::connect(&self.socket, None) { client.send(ClientMsg::Shutdown); }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() { return; }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(client: &Client, script: &str) -> u64 {
    let response = client.request(|req| ClientMsg::Create { req, spec: SpawnSpec {
        program: Some("/bin/sh".into()), args: vec!["-c".into(), script.into()], cols: 100, rows: 12,
        ..Default::default()
    } }, Duration::from_secs(10)).unwrap();
    let ServerMsg::Created { session, .. } = response else { panic!("unexpected {response:?}") };
    client.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
    session
}

fn wait_frame(client: &Client, session: u64, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = client.rx.recv_timeout(left).unwrap_or_else(|e| panic!("frame never recovered: {text}: {e}"));
        if let ServerMsg::Frame(frame) = msg {
            if frame.session == session && frame.lines.iter().any(|(_, line)| line.text().contains(text)) { return; }
        }
    }
}

fn wait_text(client: &Client, session: u64, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = client.request(|req| ClientMsg::ReadText { req, session, history: 100 }, Duration::from_secs(1)).unwrap();
        if let ServerMsg::Text { text, .. } = response {
            if text.contains(expected) { return; }
        }
        assert!(Instant::now() < deadline, "terminal never recovered: {expected}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn incomplete_tui_redraw_recovers_input_echo_frames_and_reattachment_for_both_pty_modes() {
    for hosted in [false, true] {
        let (fixture, client) = Fixture::start(hosted);
        // Deliberately omit ESU both before and after reading input. The child
        // remains alive, as it does in the reported single-panel freeze.
        let session = spawn(&client, "stty -echo; printf 'READY\\r\\n\\033[?2026hWORKING'; read line; printf '\\r\\n\\033[?2026hECHO=%s' \"$line\"; read line; printf '\\r\\nBURST\\r\\n'; i=0; while [ \"$i\" -lt 3000 ]; do printf 'line %s\\r\\n' \"$i\"; i=$((i+1)); done; printf 'BURST_DONE\\r\\n'; read hold");
        wait_frame(&client, session, "WORKING");
        client.send(ClientMsg::Input { session, data: "한?\n".as_bytes().to_vec() });
        wait_frame(&client, session, "ECHO=한?");
        wait_text(&client, session, "ECHO=한?");

        // A client that stops consuming frames must not stop this PTY or an
        // independent session. Its private socket is dropped after the burst.
        let mut slow = UnixStream::connect(&fixture.socket).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write_msg(&mut slow, &ClientMsg::Hello { proto: kiln_proto::PROTO_VERSION, build: "fixture".into(), client: "slow-fixture".into() }).unwrap();
        assert!(matches!(read_msg::<_, ServerMsg>(&mut slow).unwrap(), Some(ServerMsg::Hello { .. })));
        write_msg(&mut slow, &ClientMsg::Attach { session, cols: 100, rows: 12 }).unwrap();

        let other = spawn(&client, "stty -echo; printf 'OTHER_READY\\r\\n'; read line; printf 'OTHER_ECHO=%s\\r\\n' \"$line\"; read hold");
        wait_frame(&client, other, "OTHER_READY");
        client.send(ClientMsg::Input { session, data: b"burst\n".to_vec() });
        client.send(ClientMsg::Input { session: other, data: b"alive\n".to_vec() });
        wait_text(&client, other, "OTHER_ECHO=alive");
        wait_text(&client, session, "BURST_DONE");
        drop(slow);

        client.send(ClientMsg::Detach { session });
        client.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
        wait_frame(&client, session, "BURST_DONE");
        drop(client);
        let replacement = Client::connect(&fixture.socket, None).unwrap();
        replacement.send(ClientMsg::Attach { session, cols: 100, rows: 12 });
        wait_frame(&replacement, session, "BURST_DONE");
        let response = replacement.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
        let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected response") };
        assert!(sessions.iter().any(|info| info.id == session && info.exited.is_none()));
        replacement.send(ClientMsg::Kill { session });
        replacement.send(ClientMsg::Kill { session: other });
    }
}
