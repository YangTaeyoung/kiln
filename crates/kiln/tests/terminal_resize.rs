//! Continuous divider movement reaches a real PTY before the pointer is released.
#![cfg(unix)]
use egui::{Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use kiln::app::KilnApp;
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(client) = Client::connect(&self.0.to_string_lossy(), None) {
            client.send(ClientMsg::Shutdown);
        }
    }
}
fn wait(h: &mut Harness<'_, KilnApp>, ready: impl Fn(&Harness<'_, KilnApp>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "isolated terminal did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn pointer(h: &mut Harness<'_, KilnApp>, point: Pos2, pressed: Option<bool>) {
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    h.event(egui::Event::PointerMoved(point));
    if let Some(pressed) = pressed {
        h.event(egui::Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::ALT,
        });
    }
    h.step();
}
fn records(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn sessions(client: &Client) -> Vec<(u64, u32)> {
    let ServerMsg::Sessions { sessions, .. } = client
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(5),
        )
        .unwrap()
    else {
        panic!("session list missing")
    };
    let mut result: Vec<_> = sessions
        .into_iter()
        .map(|session| (session.id, session.pid))
        .collect();
    result.sort();
    result
}
#[test]
fn moving_divider_updates_terminal_size_without_starving_input_or_replacing_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let project = dir.path().join("workspace");
    std::fs::create_dir(&project).unwrap();
    let _cleanup = Cleanup(socket.clone());
    unsafe {
        for (key, value) in [
            ("KILN_SOCKET", socket.clone()),
            ("KILN_CONFIG_DIR", dir.path().join("config")),
            ("KILN_ACCOUNTS_SANDBOX", dir.path().join("accounts")),
        ] {
            std::env::set_var(key, value);
        }
        for key in [
            "KILN_NO_AUTO_UPGRADE",
            "KILN_DB_NO_KEYCHAIN",
            "KILN_REMOTE_NO_KEYCHAIN",
        ] {
            std::env::set_var(key, "1");
        }
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("SHELL", "/bin/sh");
    }
    let script = dir.path().join("resize.py");
    let log = dir.path().join("resize.jsonl");
    std::fs::write(&script,r#"import os, sys, tty, termios, signal, json, time
log = open(sys.argv[1], 'a', buffering=1)
def resize(*_):
    size = os.get_terminal_size(0)
    log.write(json.dumps({'kind':'resize', 'cols':size.columns, 'rows':size.lines, 'at':time.monotonic()})+'\n')
old = termios.tcgetattr(0)
try:
    tty.setraw(0)
    signal.signal(signal.SIGWINCH, resize)
    resize()
    os.write(1, b'RESIZE_PROBE_READY\r\n')
    while True:
        data = os.read(0, 1)
        if not data: break
        log.write(json.dumps({'kind':'input', 'text':data.decode('ascii')})+'\n')
finally:
    termios.tcsetattr(0, termios.TCSANOW, old)
"#).unwrap();
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_step_dt(1.0 / 120.0)
        .build_eframe(|cc| {
            let mut app = KilnApp::new(&cc.egui_ctx, Some(project));
            app.debug_set_shell("/bin/sh".into());
            app
        });
    wait(&mut h, |h| h.state().debug_focused_text().is_some());
    let ctx = h.ctx.clone();
    h.state_mut().debug_split(&ctx, false);
    wait(&mut h, |h| {
        h.state().debug_pane_sessions().len() == 2 && h.state().debug_focused_text().is_some()
    });
    let client = Client::connect(&socket.to_string_lossy(), None).unwrap();
    let original_sessions = sessions(&client);
    let session = h.state().debug_focused_session().unwrap();
    let python = std::env::var("KILN_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let command = format!(
        "'{}' -u '{}' '{}'\r",
        python,
        script.display(),
        log.display()
    );
    client.send(ClientMsg::Input {
        session,
        data: command.into_bytes(),
    });
    wait(&mut h, |_| !records(&log).is_empty());
    let before = records(&log).len();
    let mut rects = h.state().debug_pane_rects();
    rects.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    let start = pos2(
        (rects[0].1.right() + rects[1].1.left()) * 0.5,
        rects[0].1.center().y,
    );
    pointer(&mut h, start, Some(true));
    for step in 1..=40 {
        pointer(&mut h, start + egui::vec2(step as f32 * 4.0, 0.0), None);
        std::thread::sleep(Duration::from_millis(8));
    }
    let during = records(&log).len() - before;
    assert!(
        during >= 3,
        "PTY resize updates must arrive while the button is still held; got {during}"
    );
    pointer(&mut h, start + egui::vec2(160.0, 0.0), Some(false));
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(3);
    h.event(egui::Event::Text("x".into()));
    h.step();
    wait(&mut h, |_| {
        records(&log)
            .iter()
            .any(|event| event["kind"] == "input" && event["text"] == "x")
    });
    assert_eq!(
        sessions(&client),
        original_sessions,
        "divider movement must preserve every session and shell PID"
    );
    println!(
        "real PTY received {during} resize updates during 40 moving-pointer steps; input and shell PIDs preserved"
    );
}
