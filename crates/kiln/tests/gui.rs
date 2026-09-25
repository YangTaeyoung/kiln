//! GUI 를 헤드리스로 띄워 실제 데몬과 연결된 상태에서 입력·분할·스냅샷을 검증한다.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln::app::KilnApp;
use std::path::PathBuf;
use std::time::{Duration, Instant};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn setup(tag: &str) -> (PathBuf, PathBuf) {
    let base = PathBuf::from(format!("/tmp/kg-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("cfg")).unwrap();
    std::fs::create_dir_all(base.join("proj/src")).unwrap();
    std::fs::write(base.join("proj/src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    std::fs::write(base.join("proj/README.md"), "# proj\n").unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(base.join("proj")).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-qm", "init"]);
    std::fs::write(base.join("proj/src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    // SAFETY: 테스트 시작 시 단일 스레드에서 설정한다.
    unsafe {
        std::env::set_var("KILN_SOCKET", base.join("d.sock"));
        std::env::set_var("KILN_CONFIG_DIR", base.join("cfg"));
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
    }
    (base.clone(), base.join("proj"))
}

/// 실제 셸 출력(시각, pid, 임시 경로)이 들어가므로 비교하지 않고 검토용 PNG 로 저장한다.
fn save_shot(h: &mut Harness<'_, KilnApp>, name: &str) {
    let img = h.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

fn shutdown(base: &std::path::Path) {
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
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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
    save_shot(&mut h, "app_split_terminal");
    shutdown(&base);
}

/// 실제 입력과 같은 "주 수식키": macOS 는 ⌘, 그 외는 Ctrl(egui 는 command 도 함께 켠다).
fn primary() -> egui::Modifiers {
    if cfg!(target_os = "macos") { egui::Modifiers::MAC_CMD } else { egui::Modifiers { ctrl: true, command: true, ..Default::default() } }
}

fn cmd() -> egui::Modifiers {
    if cfg!(target_os = "macos") { egui::Modifiers::MAC_CMD } else { egui::Modifiers::CTRL | egui::Modifiers::SHIFT }
}

fn cmd_shift() -> egui::Modifiers {
    if cfg!(target_os = "macos") { egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT } else { egui::Modifiers::CTRL | egui::Modifiers::SHIFT | egui::Modifiers::ALT }
}

#[test]
fn explorer_opens_file_in_editor_and_saves() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("editor");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.key_press_modifiers(cmd_shift(), egui::Key::E);
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("src").is_some()), "explorer not shown");
    h.get_by_label("src").click();
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("main.rs").is_some()));
    h.get_by_label("main.rs").click();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 5, |h| h.state().debug_active_tab_title().contains("main.rs")), "editor tab not opened: {}", h.state().debug_active_tab_title());
    h.run_steps(5);
    save_shot(&mut h, "app_explorer_editor");
    // 편집 후 저장.
    h.key_press_modifiers(primary(), egui::Key::End);
    h.event(egui::Event::Text("// edited by kiln".into()));
    h.run_steps(2);
    h.key_press_modifiers(primary(), egui::Key::S);
    h.run_steps(3);
    let content = std::fs::read_to_string(proj.join("src/main.rs")).unwrap();
    assert!(content.contains("// edited by kiln"), "{content}");
    shutdown(&base);
}

#[test]
fn git_and_db_panels_render() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("panels");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.key_press_modifiers(cmd_shift(), egui::Key::G);
    assert!(pump_until(&mut h, 10, |h| h.query_by_label_contains("lib.rs").is_some()), "git panel did not list untracked file");
    h.run_steps(5);
    save_shot(&mut h, "app_git_panel");
    h.key_press_modifiers(cmd_shift(), egui::Key::B);
    h.run_steps(5);
    save_shot(&mut h, "app_db_panel");
    let _ = cmd();
    shutdown(&base);
}

#[test]
fn inline_image_and_osc8_link_render() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("image");
    use base64::Engine;
    let mut img = image::RgbaImage::new(160, 90);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgba([(x * 255 / 160) as u8, (y * 255 / 90) as u8, 200, 255]);
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let seq = format!(
        "\x1b]1337;File=inline=1:{}\x07\x1b]8;;https://github.com/YangTaeyoung\x1b\\OSC8 링크\x1b]8;;\x1b\\\n",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    std::fs::write(proj.join("demo.seq"), seq).unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.event(egui::Event::Text("clear; cat demo.seq".into()));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("OSC8"))));
    assert!(pump_until(&mut h, 5, |h| h.state().debug_image_count() > 0), "image texture not loaded");
    h.run_steps(5);
    save_shot(&mut h, "app_inline_image");
    shutdown(&base);
}
