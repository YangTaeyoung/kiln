//! GUI 를 헤드리스로 띄워 실제 데몬과 연결된 상태에서 입력·분할·스냅샷을 검증한다.

use egui_kittest::Harness;
use kiln::app::KilnApp;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let base = PathBuf::from(format!("/tmp/kg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("cfg")).unwrap();
    std::fs::create_dir_all(base.join("proj/src")).unwrap();
    std::fs::write(base.join("proj/src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    // SAFETY: 테스트 시작 시 단일 스레드에서 설정한다.
    unsafe {
        std::env::set_var("KILN_SOCKET", base.join("d.sock"));
        std::env::set_var("KILN_CONFIG_DIR", base.join("cfg"));
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
    }
    (base.clone(), base.join("proj"))
}

fn shutdown(base: &PathBuf) {
    if let Ok(c) = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None) {
        c.send(kiln_proto::ClientMsg::Shutdown);
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn pump_until(h: &mut Harness<'_, KilnApp>, secs: u64, mut f: impl FnMut(&mut Harness<'_, KilnApp>) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        h.step();
        if f(h) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

#[test]
fn terminal_roundtrip_split_and_snapshot() {
    let (base, proj) = setup("main");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())), "shell prompt did not appear");

    h.event(egui::Event::Text("echo kiln-$((20+22))".into()));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("kiln-42"))), "typed command output missing: {:?}", h.state().debug_focused_text());

    // 한글 IME 커밋.
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("echo 한글".into())));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| t.matches("한글").count() >= 2)));

    // 분할(⌘D / Ctrl+Shift+D).
    let m = if cfg!(target_os = "macos") { egui::Modifiers::MAC_CMD } else { egui::Modifiers::CTRL | egui::Modifiers::SHIFT };
    h.key_press_modifiers(m, egui::Key::D);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_pane_count() == 2 && h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.event(egui::Event::Text("printf '\\033[1;31mred\\033[0m \\033[42mgreen-bg\\033[0m\\n'".into()));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("green-bg\n") || t.matches("green-bg").count() >= 2)));
    h.run_steps(3);
    h.snapshot("app_split_terminal");
    shutdown(&base);
}
