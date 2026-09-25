//! 디자인 검토용 화면 렌더링(비교하지 않고 PNG 로 저장).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln::app::KilnApp;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn setup() -> (PathBuf, PathBuf) {
    let base = PathBuf::from(format!("/tmp/kd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("cfg")).unwrap();
    let proj = base.join("kiln-demo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src/main.rs"), "use std::io;\n\n/// 인사를 출력한다.\nfn main() -> io::Result<()> {\n    let name = \"Kiln\";\n    println!(\"안녕, {name}!\");\n    for i in 0..3 {\n        println!(\"{i}\");\n    }\n    Ok(())\n}\n").unwrap();
    std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(&proj).output().unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "add", "."]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-qm", "init"]);
    std::fs::write(proj.join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    // SAFETY: 테스트 시작 시 설정한다.
    unsafe {
        std::env::set_var("KILN_SOCKET", base.join("d.sock"));
        std::env::set_var("KILN_CONFIG_DIR", base.join("cfg"));
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
        std::env::set_var("PS1", "%F{blue}%~%f %F{green}❯%f ");
    }
    (base, proj)
}

fn pump(h: &mut Harness<'_, KilnApp>, secs: f32, mut f: impl FnMut(&mut Harness<'_, KilnApp>) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < deadline {
        h.step();
        if f(h) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

fn shot(h: &mut Harness<'_, KilnApp>, name: &str) {
    h.run_steps(4);
    let img = h.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/design");
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

fn cmd() -> egui::Modifiers {
    egui::Modifiers::MAC_CMD
}

fn cmd_shift() -> egui::Modifiers {
    egui::Modifiers::MAC_CMD | egui::Modifiers::SHIFT
}

fn type_line(h: &mut Harness<'_, KilnApp>, s: &str) {
    h.event(egui::Event::Text(s.into()));
    h.key_press(egui::Key::Enter);
}

#[test]
#[cfg(target_os = "macos")]
fn design_review_screens() {
    let (base, proj) = setup();
    let mut h = Harness::builder().with_size([1440.0, 900.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump(&mut h, 10.0, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    type_line(&mut h, "printf '\\033]0;빌드 감시\\007'; ls -la");
    pump(&mut h, 3.0, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("Cargo.toml")));
    h.key_press_modifiers(cmd(), egui::Key::D);
    pump(&mut h, 5.0, |h| h.state().debug_pane_count() == 2 && h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty()));
    type_line(&mut h, "printf '\\033]777;notify;claude;입력을 기다리는 중\\007'; git log --oneline");
    h.key_press_modifiers(cmd_shift(), egui::Key::D);
    pump(&mut h, 5.0, |h| h.state().debug_pane_count() == 3);
    pump(&mut h, 2.0, |_| false);
    shot(&mut h, "01_cards");

    h.key_press_modifiers(cmd_shift(), egui::Key::E);
    pump(&mut h, 3.0, |h| h.query_by_label("src").is_some());
    h.get_by_label("src").click();
    pump(&mut h, 2.0, |h| h.query_by_label("main.rs").is_some());
    shot(&mut h, "02_sheet_files");
    h.get_by_label("main.rs").click();
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    pump(&mut h, 3.0, |h| h.state().debug_active_tab_title().contains("main.rs"));
    h.key_press_modifiers(cmd_shift(), egui::Key::G);
    pump(&mut h, 3.0, |_| false);
    shot(&mut h, "03_editor_card_git_sheet");
    h.key_press_modifiers(cmd_shift(), egui::Key::G);
    h.run_steps(3);

    h.state_mut().debug_open_palette();
    pump(&mut h, 1.0, |_| false);
    shot(&mut h, "04_palette");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    h.state_mut().debug_open_settings(0);
    pump(&mut h, 1.0, |_| false);
    shot(&mut h, "05_settings_appearance");
    h.state_mut().debug_open_settings(1);
    pump(&mut h, 1.0, |_| false);
    shot(&mut h, "06_settings_terminal");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    let ctx = h.ctx.clone();
    h.state_mut().debug_set_theme(&ctx, "kiln-light");
    h.state_mut().debug_toast("claude", "입력을 기다리고 있습니다");
    pump(&mut h, 1.0, |_| false);
    shot(&mut h, "07_light_toast");
    h.state_mut().debug_set_theme(&ctx, "ember");
    pump(&mut h, 0.5, |_| false);
    shot(&mut h, "08_ember");
    h.state_mut().debug_set_theme(&ctx, "kiln-dark");

    if let Ok(c) = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None) {
        c.send(kiln_proto::ClientMsg::Shutdown);
    }
}
