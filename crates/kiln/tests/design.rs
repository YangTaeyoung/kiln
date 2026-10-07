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
    let commit = |file: &str, body: &str, msg: &str| {
        std::fs::write(proj.join(file), body).unwrap();
        git(&["add", "."]);
        git(&["-c", "user.email=dev@kiln.app", "-c", "user.name=양태영", "-c", "commit.gpgsign=false", "commit", "-qm", msg]);
    };
    commit("README.md", "# demo\n", "README 추가");
    git(&["checkout", "-qb", "feature/login"]);
    commit("src/login.rs", "pub fn login() {}\n", "로그인 화면 뼈대");
    commit("src/login.rs", "pub fn login() -> bool { true }\n", "로그인 결과 반환");
    git(&["checkout", "-q", "main"]);
    commit("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.2.0\"\n", "버전 0.2.0");
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "merge", "-q", "--no-ff", "feature/login", "-m", "Merge branch 'feature/login'"]);
    git(&["tag", "v0.2.0"]);
    commit("src/main.rs", "use std::time::Duration;\n\n// Keep agent sessions alive while the workspace is closed.\nfn restore_workspace(project: &str, sessions: &[String]) -> Result<(), String> {\n    let timeout = Duration::from_secs(30);\n    for session in sessions {\n        println!(\"Restoring {project}: {session} (timeout: {timeout:?})\");\n    }\n    Ok(())\n}\n\nfn main() {\n    restore_workspace(\"kiln-demo\", &[\"code-review\".to_owned()]).unwrap();\n}\n", "main 정리");
    std::fs::write(proj.join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    let acc = base.join("accounts/config");
    std::fs::create_dir_all(&acc).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    std::fs::write(
        acc.join("accounts.json"),
        serde_json::json!({
            "claude": {
                "auto_rotate": true,
                "active": "c1",
                "profiles": [
                    {"id": "c1", "tool": "claude", "label": "개인", "email": "me@example.com", "added_unix": 1, "last_usage": {"five_hour": [0.92, now + 3600], "seven_day": [0.41, now + 86400 * 3], "status": "allowed_warning"}},
                    {"id": "c2", "tool": "claude", "label": "회사", "email": "work@example.com", "added_unix": 2, "last_usage": {"five_hour": [0.12, now + 7200], "seven_day": [0.30, now + 86400 * 5], "status": "allowed"}}
                ]
            },
            "codex": {
                "profiles": [
                    {"id": "x1", "tool": "codex", "label": "개인", "email": "me@example.com", "added_unix": 3}
                ],
                "active": "x1"
            }
        })
        .to_string(),
    )
    .unwrap();
    // Keep visual evidence deterministic and never load the user's shell startup files.
    let shell_home = base.join("shell-home");
    std::fs::create_dir_all(&shell_home).unwrap();
    std::fs::write(shell_home.join(".zshrc"), "PROMPT='%F{blue}%1~%f %F{green}❯%f '\n").unwrap();
    // SAFETY: 테스트 시작 시 설정한다.
    unsafe {
        std::env::set_var("HOME", &shell_home);
        std::env::set_var("ZDOTDIR", &shell_home);
        std::env::set_var("SHELL", "/bin/zsh");
        std::env::set_var("KILN_SOCKET", base.join("d.sock"));
        std::env::set_var("KILN_CONFIG_DIR", base.join("cfg"));
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
        std::env::set_var("KILN_ACCOUNTS_SANDBOX", base.join("accounts"));
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
    for (width, height, scale) in [(720.0, 440.0, 1.0), (1024.0, 768.0, 1.0), (1440.0, 900.0, 2.0), (1920.0, 1080.0, 1.0), (2560.0, 1440.0, 1.0)] {
        h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(scale);
        h.ctx.set_zoom_factor(1.0);
        h.run_steps(3);
        h.set_size(egui::vec2(width, height));
        h.run_steps(4);
        assert_eq!(h.ctx.content_rect().size(), egui::vec2(width, height));
        let image = h.render().expect("resolution render");
        assert_eq!(image.dimensions(), ((width * scale) as u32, (height * scale) as u32));
        shot(&mut h, &format!("resolution_{}x{}_{}x", width as u32, height as u32, scale as u32));
    }
    h.state_mut().debug_queue_action(kiln::app::Action::ArrangeGrid);h.run_steps(4);
    shot(&mut h,"balanced_grid_2560");
    h.set_size(egui::vec2(1920.0,1080.0));h.run_steps(4);
    shot(&mut h,"balanced_grid_1920");
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(1.0);
    h.ctx.set_zoom_factor(1.0);
    h.run_steps(3);
    h.set_size(egui::vec2(1440.0, 900.0));
    h.run_steps(4);

    h.key_press_modifiers(cmd_shift(), egui::Key::G);
    h.run_steps(3);
    h.key_press_modifiers(cmd_shift(), egui::Key::R);
    pump(&mut h, 4.0, |_| false);
    shot(&mut h, "03b_github_sheet");
    h.key_press_modifiers(cmd_shift(), egui::Key::R);
    h.run_steps(3);
    h.key_press_modifiers(cmd_shift(), egui::Key::L);
    pump(&mut h, 4.0, |h| h.state().debug_active_tab_title().contains("Git 로그"));
    pump(&mut h, 2.0, |_| false);
    shot(&mut h, "03c_git_history");

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
    h.state_mut().debug_open_settings(3);
    pump(&mut h, 1.0, |_| false);
    shot(&mut h, "06b_settings_accounts");
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

    // Stress navigation with long names and more pages than fit in one titlebar.
    use kiln::app::Action;
    for name in ["customer-platform-production-workspace-with-a-long-name", "국제화-프로젝트-이름이-아주-긴-에이전트-작업-공간"] {
        let path = base.join(name);
        std::fs::create_dir_all(&path).unwrap();
        h.state_mut().debug_queue_action(Action::NewWorkspace(Some(path)));
        h.run_steps(3);
    }
    for _ in 0..8 { h.state_mut().debug_queue_action(Action::NewPage); h.run_steps(3); }
    for width in [720.0, 1024.0, 1920.0] {
        h.set_size(egui::vec2(width, 768.0)); h.run_steps(4);
        shot(&mut h, &format!("navigation_stress_{}", width as u32));
    }

    h.ctx.set_zoom_factor(1.3);
    h.run_steps(3);
    h.set_size(egui::vec2(720.0 / 1.3, 440.0 / 1.3));
    h.state_mut().debug_open_settings(0);
    h.run_steps(4);
    let scaled = h.render().unwrap();
    assert!(scaled.width().abs_diff(720) <= 1 && scaled.height().abs_diff(440) <= 1);
    assert!(h.ctx.content_rect().contains_rect(h.get_by_label("닫기 (Esc)").rect()));
    shot(&mut h, "minimum_settings_130pct");
    h.key_press(egui::Key::Escape); h.run_steps(3);
    h.get_by_label("알림 센터").click(); h.run_steps(4);
    for label in ["알림 닫기 (Esc)", "방해 금지 켜기", "읽은 알림 지우기"] {
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()), "{label} clipped at 130% zoom");
    }
    shot(&mut h, "minimum_inbox_130pct");
    h.key_press(egui::Key::Escape); h.run_steps(4);
    // Navigation stress selected the final workspace and scrolled the original
    // labelled session out of view. Reveal its workspace before checking that
    // the actual clickable row fits the minimum-height sidebar.
    let canonical_project=proj.canonicalize().unwrap();
    h.state_mut().debug_queue_action(Action::SelectWorkspace(0));
    assert!(pump(&mut h,3.0,|h|h.state().debug_active_workspace_root()==canonical_project.as_path()
        && h.query_by_label("빌드 감시 · 세션 열림").is_some()),
        "original workspace not ready: root={}, row_present={}",h.state().debug_active_workspace_root().display(),
        h.query_by_label("빌드 감시 · 세션 열림").is_some());
    h.run_steps(4);
    assert!(h.ctx.content_rect().contains_rect(h.get_by_label("빌드 감시 · 세션 열림").rect()));
    shot(&mut h, "minimum_sidebar_130pct");
    h.state_mut().debug_queue_action(Action::OpenRecent); h.run_steps(4);
    h.get_by_value("전체").click(); h.run_steps(3);
    shot(&mut h, "recent_project_filter_130pct");
    h.key_press(egui::Key::Escape); h.run_steps(3);
    h.key_press(egui::Key::Escape); h.run_steps(3);
    h.state_mut().debug_open_settings(4); h.run_steps(4);
    assert!(h.ctx.content_rect().contains_rect(h.get_by_label("닫기 (Esc)").rect()));
    shot(&mut h, "keybindings_130pct");
    h.key_press(egui::Key::Escape); h.run_steps(3);
    h.ctx.set_zoom_factor(1.0); h.run_steps(3); h.set_size(egui::vec2(1024.0,768.0)); h.run_steps(3);
    h.state_mut().debug_open_settings(4); h.run_steps(4);
    shot(&mut h, "keybindings_1024");
    h.key_press(egui::Key::Escape); h.run_steps(3);
    h.state_mut().debug_queue_action(Action::OpenRecovery); h.run_steps(4);
    shot(&mut h, "recovery_center_empty");
    h.get_by_label("닫기").click(); h.run_steps(3);


    if let Ok(c) = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None) {
        c.send(kiln_proto::ClientMsg::Shutdown);
    }
}
