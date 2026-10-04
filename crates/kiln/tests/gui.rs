//! GUI 를 헤드리스로 띄워 실제 데몬과 연결된 상태에서 입력·분할·스냅샷을 검증한다.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln::app::KilnApp;
use std::path::PathBuf;
use std::time::{Duration, Instant};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Public README image: real renderer and isolated PTYs, entirely synthetic text.
#[test]
#[ignore = "regenerates the public documentation screenshot"]
fn public_workspace_preview() {
    use kiln::app::Action;
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let (base,proj)=setup("public-preview");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {fn drop(&mut self){shutdown(&self.0);}}
    let _cleanup=Cleanup(base.clone());
    unsafe {std::env::set_var("SHELL","/bin/sh");}
    let mut h=Harness::builder().with_size([1440.0,860.0]).wgpu().build_eframe(|cc|{
        let mut app=KilnApp::new(&cc.egui_ctx,Some(proj.clone()));app.debug_set_sidebar_width(248.0);app
    });
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some()));
    h.state_mut().debug_begin_project_rename("Acme workspace");h.run_steps(2);
    h.key_press(egui::Key::Enter);h.run_steps(2);
    let client=kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(),None).unwrap();
    let samples=[
        ("Connect the frontend and API","running","\x1b[1;36mACME / CHECKOUT\x1b[0m\n\nConnect the checkout page to the payments API.\nKeep the frontend and backend contract aligned.\n\n\x1b[90mWorkspace\x1b[0m\n  apps/web       React + TypeScript\n  services/api   Go\n  packages/ui    Shared components\n\n\x1b[32m✓\x1b[0m Read workspace instructions\n\x1b[32m✓\x1b[0m Compare request and response types\n\x1b[32m✓\x1b[0m Add payment validation\n\n\x1b[36mWorking on error handling…\x1b[0m\n\n  services/api/payment/handler.go\n  apps/web/src/checkout.tsx\n\n"),
        ("Review the integration tests","waiting","\x1b[1;36mINTEGRATION REVIEW\x1b[0m\n\nChecked the checkout flow across both repositories.\n\n\x1b[32mPASS\x1b[0m  successful payment\n\x1b[32mPASS\x1b[0m  invalid card details\n\x1b[32mPASS\x1b[0m  network retry\n\x1b[32mPASS\x1b[0m  duplicate submission\n\n\x1b[90m24 tests passed · 0 failed\x1b[0m\n\nThe API and frontend agree on the error response.\n\n\x1b[33mOne decision needs your input\x1b[0m\nShould a timed-out payment retry automatically,\nor ask the customer to try again?\n\n")
    ];
    for (i,(title,state,body)) in samples.iter().enumerate() {
        if i>0 {h.key_press_modifiers(primary(),egui::Key::D);assert!(pump_until(&mut h,10,|h|h.state().debug_pane_count()==2&&h.state().debug_focused_session().is_some()));}
        let sid=h.state().debug_focused_session().unwrap();
        let path=base.join(format!("demo-{i}.sh"));
        let text=format!("\x1b[2J\x1b[H\x1b]0;{title}\x07\x1b]777;kiln-agent;{state}\x07{body}");
        std::fs::write(&path,format!("printf '%s' '{}'\nsleep 120\n",text.replace('\'',"'\\''"))).unwrap();
        client.send(kiln_proto::ClientMsg::Input{session:sid,data:format!("sh '{}'\r",path.display()).into_bytes()});
        assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some_and(|s|s.contains(if i==0{"Working on error"}else{"One decision"}))));
    }
    h.state_mut().debug_queue_action(Action::RevealPane(1));h.run_steps(5);
    let out=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/images");std::fs::create_dir_all(&out).unwrap();
    h.render().unwrap().save(out.join("workspace.png")).unwrap();
}

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
        std::env::set_var("KILN_ACCOUNTS_SANDBOX", base.join("accounts"));
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

#[cfg(unix)]
#[test]
fn agent_limit_message_offers_account_switch() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("limit");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));

    // argv[0] 이 claude 인 프로세스가 한도 메시지를 출력한 채 떠 있다.
    h.event(egui::Event::Text("m='hit your session limit'; (printf \"You've $m · resets 3pm\\n\"; exec -a claude sleep 600)".into()));
    h.key_press(egui::Key::Enter);
    assert!(
        pump_until(&mut h, 15, |h| h.state().debug_toast_titles().iter().any(|t| t == "사용량 한도")),
        "limit toast missing: {:?} / {:?}",
        h.state().debug_toast_titles(),
        h.state().debug_focused_text()
    );
    h.run_steps(3);
    save_shot(&mut h, "app_limit_toast");

    // The same recovery must remain available in the persistent inbox.
    h.get_by_label("알림 센터").click();
    h.run_steps(4);
    save_shot(&mut h, "app_limit_inbox");
    // 등록된 다른 계정이 없으면 전환 대신 안내한다.
    h.get_by_label("다음 계정으로 전환").click();
    assert!(pump_until(&mut h, 10, |h| h.state().debug_toast_titles().iter().any(|t| t == "전환할 계정이 없습니다")), "{:?}", h.state().debug_toast_titles());
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

#[test]
fn notification_inbox_and_narrow_settings_are_operable() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("inbox");
    let mut h = Harness::builder().with_size([900.0, 640.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.state_mut().debug_toast("검토가 필요합니다", "작업 내용을 확인하고 다음 단계를 선택하세요.");
    h.run_steps(3);
    h.get_by_label("알림 센터").click();
    h.run_steps(4);
    assert!(h.query_by_label("검토가 필요합니다").is_some());
    assert!(h.query_by_label("읽음으로 표시").is_some());
    save_shot(&mut h, "app_notifications");
    let theme_ctx = h.ctx.clone();
    h.state_mut().debug_set_theme(&theme_ctx, "kiln-light");
    h.run_steps(3);
    save_shot(&mut h, "app_notifications_light");
    h.state_mut().debug_set_theme(&theme_ctx, "kiln-dark");
    h.run_steps(3);
    h.get_by_label("모두 읽음").click();
    h.run_steps(3);
    assert!(h.query_by_label("읽음으로 표시").is_none());
    h.get_by_label("방해 금지 켜기").click();
    h.run_steps(3);
    assert!(h.query_by_label("방해 금지 끄기").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.state_mut().debug_toast("조용히 보관한 알림", "방해 금지 상태에서도 알림 센터에 보관합니다.");
    h.run_steps(3);
    assert!(!h.state().debug_toast_titles().contains(&"조용히 보관한 알림".to_string()));
    h.get_by_label("알림 센터").click();
    h.run_steps(4);
    assert!(h.query_by_label("조용히 보관한 알림").is_some());
    h.get_by_label("읽은 알림 지우기").click();
    h.run_steps(3);
    assert!(h.query_by_label("검토가 필요합니다").is_none());
    assert!(h.query_by_label("조용히 보관한 알림").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.set_size(egui::vec2(720.0, 440.0));
    h.run_steps(3);
    save_shot(&mut h, "app_minimum_window");
    h.state_mut().debug_open_settings(2);
    h.run_steps(4);
    save_shot(&mut h, "app_settings_narrow");
    assert!(h.query_by_label("에이전트 팝업과 시스템 알림을 끕니다. 알림 센터에는 계속 보관됩니다.").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.state_mut().debug_open_palette();
    h.run_steps(3);
    h.event(egui::Event::Text("존재하지않는검색어".into()));
    h.run_steps(3);
    assert!(h.query_by_label("검색 결과가 없습니다").is_some());
    save_shot(&mut h, "app_palette_empty");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    shutdown(&base);
}

#[test]
fn reading_inbox_acknowledges_daemon_attention() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("attention");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    let client = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None).unwrap();
    let sessions = || match client.request(|req| kiln_proto::ClientMsg::ListSessions { req }, Duration::from_secs(2)).unwrap() {
        kiln_proto::ServerMsg::Sessions { sessions, .. } => sessions,
        other => panic!("unexpected {other:?}"),
    };
    let first = sessions()[0].id;
    h.key_press_modifiers(cmd(), egui::Key::T);
    assert!(pump_until(&mut h, 5, |_| sessions().len() == 2));
    client.send(kiln_proto::ClientMsg::Input { session: first, data: b"printf '\\033]777;notify;Review ready;Please check\\007'\r".to_vec() });
    assert!(pump_until(&mut h, 5, |h| h.state().debug_toast_titles().iter().any(|t| t == "Review ready")));
    assert!(sessions().iter().find(|s| s.id == first).unwrap().attention);
    h.get_by_label("알림 센터").click();
    h.run_steps(4);
    h.get_by_label("읽음으로 표시").click();
    assert!(pump_until(&mut h, 5, |_| !sessions().iter().find(|s| s.id == first).unwrap().attention));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    client.send(kiln_proto::ClientMsg::Input { session: first, data: b"printf '\\033]777;notify;Second review;Please check\\007'\r".to_vec() });
    assert!(pump_until(&mut h, 5, |h| h.state().debug_toast_titles().iter().any(|t| t == "Second review")));
    h.key_press_modifiers(cmd_shift(), egui::Key::U);
    assert!(pump_until(&mut h, 5, |_| !sessions().iter().find(|s| s.id == first).unwrap().attention));
    h.get_by_label("알림 센터").click();
    h.run_steps(4);
    assert!(h.query_by_label("읽음으로 표시").is_none());
    shutdown(&base);
}

fn edit_file_without_saving(h: &mut Harness<'_, KilnApp>, path: PathBuf) {
    h.state_mut().debug_queue_action(kiln::app::Action::OpenLink(kiln::app::terminal::LinkTarget::File { path, line: None, col: None }));
    h.run_steps(5);
    h.key_press_modifiers(primary(), egui::Key::End);
    h.event(egui::Event::Text("// unsaved close guard".into()));
    h.run_steps(2);
    assert!(!h.state().debug_unsaved_items().is_empty(), "editor must be dirty before testing close protection");
}

fn request_native_close(h: &mut Harness<'_, KilnApp>) {
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
    h.step();
}

fn has_viewport_command(h: &Harness<'_, KilnApp>, command: egui::ViewportCommand) -> bool {
    h.output().viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|output| output.commands.contains(&command))
}

#[test]
fn native_quit_protects_unsaved_files_in_inactive_workspaces() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("quit-guard");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    edit_file_without_saving(&mut h, proj.join("src/main.rs"));
    let other = base.join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    h.state_mut().debug_queue_action(kiln::app::Action::NewWorkspace(Some(other)));
    h.run_steps(3);
    request_native_close(&mut h);
    assert!(has_viewport_command(&h, egui::ViewportCommand::CancelClose));
    let commands = &h.output().viewport_output[&egui::ViewportId::ROOT].commands;
    let order: Vec<_> = [egui::ViewportCommand::Visible(true), egui::ViewportCommand::Minimized(false), egui::ViewportCommand::Focus]
        .iter().map(|command| commands.iter().position(|candidate| candidate == command).expect("quit guard must reveal its prompt")).collect();
    assert!(order.windows(2).all(|positions| positions[0] < positions[1]), "restore visibility before requesting focus");
    assert!(h.query_by_label("저장하지 않은 변경이 있습니다").is_some());
    assert!(h.query_by_label_contains("main.rs").is_some());
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    h.get_by_label("취소").click();
    h.run_steps(2);
    assert_eq!(h.state().debug_unsaved_items().len(), 1);
    assert!(!has_viewport_command(&h, egui::ViewportCommand::Close));
    assert!(!std::fs::read_to_string(proj.join("src/main.rs")).unwrap().contains("unsaved close guard"));
    request_native_close(&mut h);
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    h.get_by_label("변경 버리고 종료").click();
    h.step();
    assert!(has_viewport_command(&h, egui::ViewportCommand::Close));
    shutdown(&base);
}

#[test]
fn workspace_close_requires_explicit_discard_and_cancel_preserves_editor() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("workspace-guard");
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    edit_file_without_saving(&mut h, proj.join("src/main.rs"));
    h.state_mut().debug_queue_action(kiln::app::Action::CloseWorkspace(0));
    h.run_steps(2);
    assert!(h.query_by_label("변경 버리고 작업 공간 닫기").is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().debug_workspace_count(), 1);
    assert_eq!(h.state().debug_unsaved_items().len(), 1);
    // Saving after cancellation must still write the original editor buffer.
    h.key_press_modifiers(primary(), egui::Key::S);
    h.run_steps(3);
    assert!(std::fs::read_to_string(proj.join("src/main.rs")).unwrap().contains("unsaved close guard"));
    assert!(h.state().debug_unsaved_items().is_empty());
    edit_file_without_saving(&mut h, proj.join("src/main.rs"));
    h.state_mut().debug_queue_action(kiln::app::Action::CloseWorkspace(0));
    h.run_steps(2);
    h.get_by_label("변경 버리고 작업 공간 닫기").click();
    h.run_steps(2);
    assert!(h.state().debug_unsaved_items().is_empty());
    assert!(!h.state().debug_active_tab_title().contains("main.rs"));
    request_native_close(&mut h);
    assert!(!has_viewport_command(&h, egui::ViewportCommand::CancelClose), "clean workspaces should close without a loss prompt");
    shutdown(&base);
}

#[cfg(target_os = "macos")]
#[test]
fn escape_reaches_terminal_while_wide_explorer_stays_open() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("escape-dock");
    std::fs::write(proj.join("escape_probe.py"), "import sys, termios, tty\nfd = sys.stdin.fileno()\nold = termios.tcgetattr(fd)\ntry:\n    tty.setraw(fd)\n    print('READY-FOR-ESCAPE', flush=True)\n    char = sys.stdin.read(1)\nfinally:\n    termios.tcsetattr(fd, termios.TCSADRAIN, old)\nprint('RECEIVED-BYTE-' + str(ord(char)), flush=True)\n").unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.event(egui::Event::Text("/usr/bin/python3 escape_probe.py".into()));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("READY-FOR-ESCAPE"))));
    h.key_press_modifiers(cmd_shift(), egui::Key::E);
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("도구 닫기").is_some()));
    // Opening Explorer intentionally gives its tree keyboard focus. Return to
    // the terminal before checking that a dock does not intercept TUI Escape.
    h.get_by_label("터미널 화면").click();
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    assert!(pump_until(&mut h, 5, |h| h.state().debug_focused_text().is_some_and(|t| t.contains("RECEIVED-BYTE-27"))), "Escape did not reach the raw-mode terminal");
    assert!(h.query_by_label("도구 닫기").is_some(), "wide dock must remain open on terminal Escape");
    shutdown(&base);
}

#[test]
fn sidebar_toggle_changes_layout_at_720_points() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("sidebar-720");
    let mut h = Harness::builder().with_size([720.0, 520.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    assert!(h.query_by_label("작업 공간").is_some());
    h.key_press_modifiers(cmd(), egui::Key::B);
    h.run_steps(3);
    assert!(h.query_by_label("작업 공간").is_none(), "explicit sidebar toggle must collapse even at a narrow width");
    h.key_press_modifiers(cmd(), egui::Key::B);
    h.run_steps(3);
    assert!(h.query_by_label("작업 공간").is_some(), "explicit toggle must restore workspace labels");
    shutdown(&base);
}

#[test]
fn recent_work_search_reopens_editor_across_workspaces_and_restores_input() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("recent-work");
    let mut h = Harness::builder().with_size([720.0, 520.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some()));
    edit_file_without_saving(&mut h, proj.join("src/main.rs"));
    let other = base.join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    h.state_mut().debug_queue_action(kiln::app::Action::NewWorkspace(Some(other)));
    h.run_steps(3);
    h.key_press_modifiers(cmd(), egui::Key::J);
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().debug_active_tab_title().contains("main.rs"), "empty recent switcher should select the previous task");
    h.state_mut().debug_queue_action(kiln::app::Action::SelectWorkspace(1));
    h.run_steps(3);
    h.key_press_modifiers(cmd(), egui::Key::J);
    h.run_steps(3);
    save_shot(&mut h, "recent_work_switcher");
    h.event(egui::Event::Text("main.rs".into()));
    h.run_steps(2);
    {
        use egui_kittest::kittest::NodeT;
        let row=h.get_by_label_contains("main.rs");
        let label=row.accesskit_node().label().unwrap_or_default();
        assert!(label.contains("proj") && label.contains("패널"),"search result must expose project and panel context: {label}");
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().debug_active_tab_title().contains("main.rs"));
    h.key_press_modifiers(primary(), egui::Key::S);
    h.run_steps(3);
    assert!(std::fs::read_to_string(proj.join("src/main.rs")).unwrap().contains("unsaved close guard"));
    h.key_press_modifiers(cmd(), egui::Key::W);
    h.run_steps(3);
    h.key_press_modifiers(cmd(), egui::Key::J);
    h.run_steps(3);
    h.event(egui::Event::Text("main.rs".into()));
    h.run_steps(2);
    assert!(h.query_by_label_contains("main.rs").is_none(), "closed tasks must leave the switcher");
    assert!(h.query_by_label("검색 결과가 없습니다").is_some());
    shutdown(&base);
}

#[test]
fn saved_command_requires_preview_then_runs_once_in_selected_directory() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("saved-command");
    let cwd = base.join("folder with spaces");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(base.join("cfg")).unwrap();
    let marker = cwd.join("executions.txt");
    let entries = serde_json::json!([{ "name": "Local verification", "command": "printf 'ran-once\\n' >> executions.txt", "cwd": cwd }]);
    std::fs::write(base.join("cfg/commands.json"), serde_json::to_vec(&entries).unwrap()).unwrap();
    let mut h = Harness::builder().with_size([1024.0, 768.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj)));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some()));
    assert!(!marker.exists(), "loading saved commands must never execute them");
    h.key_press_modifiers(cmd_shift(), egui::Key::J);
    h.run_steps(3);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.query_by_label("새 터미널에서 실행").is_some());
    save_shot(&mut h, "saved_command_preview");
    assert!(!marker.exists(), "selecting a command must only preview it");
    h.get_by_label("새 터미널에서 실행").click();
    assert!(pump_until(&mut h, 10, |_| marker.exists()));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran-once\n");
    h.run_steps(5);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran-once\n", "rendering must not repeat a launch");
    shutdown(&base);
}

#[test]
fn native_quit_protects_unsaved_launcher_draft() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("launcher-quit");
    let mut h = Harness::builder().with_size([1024.0, 768.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj)));
    h.key_press_modifiers(cmd_shift(), egui::Key::J);
    h.run_steps(3);
    h.get_by_label("새 명령").click();
    h.run_steps(3);
    h.event(egui::Event::Text("Retain this draft".into()));
    h.run_steps(3);
    assert!(h.state().debug_unsaved_items().iter().any(|item| item.contains("저장 명령")));
    request_native_close(&mut h);
    assert!(has_viewport_command(&h, egui::ViewportCommand::CancelClose));
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    h.get_by_label("취소").click();
    h.run_steps(3);
    assert!(h.query_by_label("명령 저장").is_some());
    assert!(h.state().debug_unsaved_items().iter().any(|item| item.contains("저장 명령")));
    assert!(!base.join("cfg/commands.json").exists(), "canceling quit must neither save nor run the draft");
    shutdown(&base);
}

#[test]
fn recovery_center_reports_saved_editor_draft_after_restart() {
    let _serial = SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let (base,proj)=setup("recovery-roundtrip");
    let file=proj.join("src/main.rs");
    let original=std::fs::read_to_string(&file).unwrap();
    let mut h=Harness::builder().with_size([900.0,640.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some()));
    h.state_mut().debug_queue_action(kiln::app::Action::OpenLink(kiln::app::terminal::LinkTarget::File{path:file.clone(),line:None,col:None}));
    h.run_steps(4);
    h.event(egui::Event::Text("// recovered draft\n".into()));h.run_steps(3);
    assert!(!h.state().debug_unsaved_items().is_empty());
    std::thread::sleep(Duration::from_millis(550));h.run_steps(3);
    assert_eq!(std::fs::read_to_string(&file).unwrap(),original,"checkpoint must not save user file");
    drop(h);
    let mut h=Harness::builder().with_size([900.0,640.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    h.state_mut().debug_queue_action(kiln::app::Action::OpenRecovery);h.run_steps(4);
    assert!(h.query_by_label("복원한 내용 열기").is_some());
    assert!(!h.state().debug_unsaved_items().is_empty());
    save_shot(&mut h,"recovery_center");shutdown(&base);
}

#[test]
fn native_quit_preserves_project_composer_and_keeps_confirmation_accessible() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("projects-quit");
    let mut h = Harness::builder().with_size([1024.0,768.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj)));
    h.state_mut().debug_queue_action(kiln::app::Action::OpenProjects);h.run_steps(4);
    h.get_by_label("새 프로젝트").click();h.run_steps(3);
    h.event(egui::Event::Text("Preserve project draft".into()));h.run_steps(3);
    assert!(h.state().debug_unsaved_items().iter().any(|item|item.contains("프로젝트")));
    request_native_close(&mut h);
    assert!(has_viewport_command(&h,egui::ViewportCommand::CancelClose));
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    h.run_steps(3); // Let the newly centered modal finish its first layout.
    h.get_by_label("취소").click();h.run_steps(4);
    save_shot(&mut h,"project_composer_after_quit_cancel");
    assert!(h.query_by_label("프로젝트 열기").is_some());
    assert!(h.state().debug_unsaved_items().iter().any(|item|item.contains("프로젝트")));
    assert!(!base.join("cfg/projects.json").exists());
    assert!(h.query_all_by_value("Preserve project draft").next().is_some());
    h.event(egui::Event::Text(" continued".into())); h.run_steps(3);
    assert!(h.query_all_by_value("Preserve project draft continued").next().is_some(), "canceling quit must restore typing to the project draft");
    save_shot(&mut h,"project_composer_quit_canceled");shutdown(&base);
}

#[test]
fn opening_another_document_preserves_saved_documents_and_reuses_existing_tasks() {
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let(base,proj)=setup("persistent-documents");
    let mut h=Harness::builder().with_size([1200.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    let open=|path|kiln::app::Action::OpenLink(kiln::app::terminal::LinkTarget::File{path,line:None,col:None});
    let first=proj.join("src/main.rs");let second=proj.join("README.md");
    h.state_mut().debug_queue_action(open(first.clone()));h.run_steps(4);
    let tasks=h.state().debug_task_count();
    h.state_mut().debug_queue_action(open(second.clone()));h.run_steps(4);
    let keys=h.state().debug_tool_keys();
    assert!(keys.contains(&format!("file:{}",first.display())));
    assert!(keys.contains(&format!("file:{}",second.display())));
    assert_eq!(h.state().debug_task_count(),tasks+1);
    h.state_mut().debug_queue_action(open(first));h.run_steps(4);
    assert_eq!(h.state().debug_task_count(),tasks+1,"reopening focuses the existing document");
    assert_eq!(h.state().debug_tool_keys().len(),2);
    assert!(h.state().debug_active_tab_title().contains("main.rs"));
    shutdown(&base);
}

#[test]
fn opening_existing_file_and_terminal_commands_reveal_targets_from_zoom() {
    use kiln::app::Action;
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let(base,proj)=setup("zoom-reveal-targets");
    let mut h=Harness::builder().with_size([1200.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    let open=||Action::OpenLink(kiln::app::terminal::LinkTarget::File{path:proj.join("src/main.rs"),line:None,col:None});
    h.state_mut().debug_queue_action(open());h.run_steps(4);
    h.key_press_modifiers(primary() | egui::Modifiers::ALT,egui::Key::ArrowLeft);h.run_steps(3);
    assert_eq!(h.state().debug_active_tab_title(),"terminal","test must zoom the other pane first");
    h.state_mut().debug_queue_action(Action::ToggleZoom(None));h.run_steps(3);
    h.state_mut().debug_queue_action(open());h.run_steps(4);
    assert!(h.state().debug_active_tab_title().contains("main.rs"));
    assert!(h.state().debug_focused_is_visible(),"reused file must replace the old zoom target");
    h.state_mut().debug_queue_action(Action::NewTermAt(proj.clone()));h.run_steps(4);
    assert!(h.state().debug_focused_is_visible(),"new terminal must be visible from zoom");
    h.state_mut().debug_queue_action(Action::ToggleZoom(None));h.run_steps(3);
    h.state_mut().debug_queue_action(Action::RunInTerminal("printf ZOOM-COMMAND-VISIBLE".into()));h.run_steps(4);
    assert!(h.state().debug_focused_is_visible(),"command terminal must be visible from zoom");
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some_and(|text|text.contains("ZOOM-COMMAND-VISIBLE"))));
    shutdown(&base);
}


#[test]
fn command_purpose_titles_survive_foreign_shell_markers_across_split_terminals() {
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let isolated_home=tempfile::tempdir().unwrap();let home=isolated_home.path();
    struct Restore(Vec<(&'static str,Option<std::ffi::OsString>)>);
    impl Drop for Restore {fn drop(&mut self){for (key,value) in &self.0 {unsafe {if let Some(value)=value{std::env::set_var(key,value);}else{std::env::remove_var(key);}}}}}
    let _restore=Restore(["HOME","ZDOTDIR","SHELL","GIT_CONFIG_GLOBAL","GIT_CONFIG_NOSYSTEM"].iter().map(|key|(*key,std::env::var_os(key))).collect());
    unsafe {std::env::set_var("HOME",home);std::env::set_var("ZDOTDIR",home);std::env::set_var("SHELL","/bin/zsh");std::env::set_var("GIT_CONFIG_GLOBAL","/dev/null");std::env::set_var("GIT_CONFIG_NOSYSTEM","1");}
    let (base,proj)=setup("shell-purpose");
    // Isolated reproduction of pre-existing integrations. Never source user configuration.
    std::fs::write(home.join(".zshrc"),"PS1='fixture> '\npreexec() { printf '\\e]133;C\\a\\e]633;C\\a'; }\nprecmd() { printf '\\e]133;D;0\\a\\e]133;A\\a'; }\n").unwrap();
    let mut h=Harness::builder().with_size([1280.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some_and(|t|t.contains("fixture>"))));
    let common_prefix="shared-long-command-context-".repeat(5);
    h.event(egui::Event::Text(format!("echo {common_prefix}kiln-purpose-alpha")));h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some_and(|t|t.matches("kiln-purpose-alpha").count()>=2)));
    h.key_press_modifiers(if cfg!(target_os="macos"){egui::Modifiers::MAC_CMD}else{egui::Modifiers::CTRL|egui::Modifiers::SHIFT},egui::Key::D);
    assert!(pump_until(&mut h,10,|h|h.state().debug_pane_count()==2 && h.state().debug_focused_text().is_some_and(|t|t.contains("fixture>"))));
    h.event(egui::Event::Text(format!("echo {common_prefix}kiln-purpose-beta")));h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some_and(|t|t.matches("kiln-purpose-beta").count()>=2)));
    h.state_mut().debug_queue_action(kiln::app::Action::OpenRecent);h.run_steps(5);
    assert!(h.query_by_label_contains("kiln-purpose-alpha").is_some(),"first terminal command must identify its recent-work row");
    assert!(h.query_by_label_contains("kiln-purpose-beta").is_some(),"second terminal command must identify its recent-work row");
    assert!(h.query_by_label_contains("터미널 1.1 · 최근 명령 · echo").is_some());
    assert!(h.query_by_label_contains("터미널 1.2 · 최근 명령 · echo").is_some());
    save_shot(&mut h,"recent_command_purpose_labels");shutdown(&base);
}

#[test]
fn failed_discard_checkpoint_keeps_app_open_and_drafts_recoverable() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("discard-save-failure");
    let file = proj.join("src/main.rs");
    let original = std::fs::read_to_string(&file).unwrap();
    let mut h = Harness::builder().with_size([1024.0, 768.0])
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some()));
    edit_file_without_saving(&mut h, file.clone());
    let state = base.join("cfg/state.json");
    let backup = base.join("cfg/state-before-failure.json");
    if state.exists() { std::fs::rename(&state, &backup).unwrap(); }
    std::fs::create_dir(&state).unwrap();
    h.state_mut().debug_queue_action(kiln::app::Action::QuitConfirmed);
    h.step();
    assert!(!has_viewport_command(&h, egui::ViewportCommand::Close));
    assert!(!h.state().debug_unsaved_items().is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    std::fs::remove_dir(&state).unwrap();
    if backup.exists() { std::fs::rename(&backup, &state).unwrap(); }
    h.state_mut().debug_queue_action(kiln::app::Action::QuitPreservingDrafts);
    h.step();
    assert!(has_viewport_command(&h, egui::ViewportCommand::Close));
    assert!(std::fs::read_to_string(&state).unwrap().contains("unsaved close guard"));
    shutdown(&base);
}

#[test]
fn closing_other_projects_preserves_the_active_project() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for active in 0..3 {
        for closed in 0..3 {
            let (base, proj) = setup(&format!("project-selection-{active}-{closed}"));
            let mut roots=vec![proj.clone()];
            for name in ["second", "third"] {
                let path=base.join(name);std::fs::create_dir_all(&path).unwrap();roots.push(path);
            }
            let mut h=Harness::builder().with_size([1024.0,768.0])
                .build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
            for root in &roots[1..] { h.state_mut().debug_queue_action(kiln::app::Action::NewWorkspace(Some(root.clone()))); }
            h.run_steps(2);
            h.state_mut().debug_queue_action(kiln::app::Action::SelectWorkspace(active));
            h.state_mut().debug_queue_action(kiln::app::Action::CloseWorkspaceConfirmed(closed));
            h.run_steps(2);
            let expected=if active!=closed {&roots[active]} else {&roots[if closed==2 {1} else {closed+1}]};
            assert_eq!(h.state().debug_active_workspace_root().canonicalize().unwrap(),expected.canonicalize().unwrap(),"active={active}, closed={closed}");
            shutdown(&base);
        }
    }
}

#[test]
fn notification_session_navigation_replaces_the_zoom_target() {
    use kiln::app::Action;
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let (base,proj)=setup("notification-zoom");
    let mut h=Harness::builder().with_size([1200.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some()));
    let first=h.state().debug_focused_session().unwrap();
    h.key_press_modifiers(if cfg!(target_os="macos") {egui::Modifiers::MAC_CMD} else {egui::Modifiers::CTRL|egui::Modifiers::SHIFT},egui::Key::D);
    h.run_steps(3);
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some_and(|sid|sid!=first)));
    h.state_mut().debug_queue_action(Action::ToggleZoom(None));h.run_steps(3);
    h.state_mut().debug_queue_action(Action::RevealSession(first));h.run_steps(3);
    assert_eq!(h.state().debug_focused_session(),Some(first));
    assert!(h.state().debug_focused_is_visible(),"notification destination must be visible, not behind another zoomed panel");
    shutdown(&base);
}

#[test]
fn detached_and_existing_sessions_reveal_canvas_behind_wide_sidebar() {
    use kiln::app::Action;
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let(base,proj)=setup("sidebar-reveal");
    let mut h=Harness::builder().with_size([1000.0,720.0]).build_eframe(|cc|{
        let mut app=KilnApp::new(&cc.egui_ctx,Some(proj.clone())); app.debug_set_sidebar_width(320.0); app
    });
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some()));
    let sid=h.state().debug_focused_session().unwrap();
    h.key_press_modifiers(if cfg!(target_os="macos"){egui::Modifiers::MAC_CMD|egui::Modifiers::SHIFT}else{egui::Modifiers::CTRL|egui::Modifiers::SHIFT},egui::Key::E);h.run_steps(3);
    assert!(h.state().debug_sheet_open());
    h.state_mut().debug_queue_action(Action::RevealSession(sid));h.run_steps(3);
    assert!(!h.state().debug_sheet_open(),"1000 - 320 is exclusive even though 1000 - 232 is not");
    h.key_press_modifiers(if cfg!(target_os="macos"){egui::Modifiers::MAC_CMD|egui::Modifiers::SHIFT}else{egui::Modifiers::CTRL|egui::Modifiers::SHIFT},egui::Key::E);h.run_steps(3);
    assert!(h.state().debug_sheet_open());
    h.state_mut().debug_queue_action(Action::AttachSession(sid));h.run_steps(3);
    assert!(!h.state().debug_sheet_open(),"attaching from an inbox must reveal the destination");
    assert_eq!(h.state().debug_focused_session(),Some(sid));
    h.key_press_modifiers(if cfg!(target_os="macos"){egui::Modifiers::MAC_CMD|egui::Modifiers::SHIFT}else{egui::Modifiers::CTRL|egui::Modifiers::SHIFT},egui::Key::E);h.run_steps(3);
    assert!(h.state().debug_sheet_open());
    h.state_mut().debug_queue_action(Action::OpenProjects);h.run_steps(3);
    h.get_by_label("새 프로젝트").click();h.run_steps(3);
    h.event(egui::Event::Text("Existing project".into()));h.run_steps(2);
    h.get_by_label("프로젝트 열기").click();h.run_steps(3);
    assert!(!h.state().debug_sheet_open(),"opening the existing project in terminal mode must reveal its work surface");
    shutdown(&base);
}

#[test]
fn blank_project_name_enter_keeps_rename_open() {
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let(base,proj)=setup("project-rename-empty");
    let mut h=Harness::builder().with_size([1000.0,720.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    h.state_mut().debug_begin_project_rename("   ");h.run_steps(3);
    h.key_press(egui::Key::Enter);h.run_steps(3);
    assert!(h.query_by_label("작업 공간 이름 바꾸기").is_some());
    h.event(egui::Event::Text("Renamed project".into()));h.run_steps(2);
    h.key_press(egui::Key::Enter);h.run_steps(3);
    assert!(h.query_by_label("작업 공간 이름 바꾸기").is_none());
    shutdown(&base);
}

#[test]
fn minimum_quit_confirmation_keeps_all_draft_actions_visible() {
    let _serial=SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let (base,proj)=setup("quit-minimum-actions");
    let mut h=Harness::builder().with_size([1280.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_text().is_some()));
    for n in 0..6 {
        let path=proj.join(format!("src/long_unsaved_document_number_{n}_for_quit_guard.rs"));
        std::fs::write(&path,"fn main() {}\n").unwrap();
        edit_file_without_saving(&mut h,path);
    }
    assert_eq!(h.state().debug_unsaved_items().len(),6);
    h.set_size(egui::vec2(720.0/1.3,440.0/1.3));h.run_steps(3);
    request_native_close(&mut h);
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    h.run_steps(3);
    save_shot(&mut h,"quit_confirmation_minimum_130pct");
    for label in ["작성 내용 남기고 종료","변경 버리고 종료","취소"] {
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}: {:?}, viewport {:?}",h.get_by_label(label).rect(),h.ctx.content_rect());
    }
    h.get_by_label("취소").click();h.run_steps(2);
    assert_eq!(h.state().debug_unsaved_items().len(),6);
    shutdown(&base);
}

#[test]
fn terminal_focus_reporting_tracks_search_tabs_and_modal_with_real_pty() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("terminal-focus");
    struct DaemonCleanup(PathBuf);
    impl Drop for DaemonCleanup { fn drop(&mut self) { shutdown(&self.0); } }
    let _cleanup = DaemonCleanup(base.clone());
    // Only this isolated daemon/session receives input. Capture raw protocol bytes
    // instead of inferring TUI behavior from GUI selection or focus flags.
    std::fs::write(proj.join("focus-probe.py"), r#"import os, sys, tty
from pathlib import Path
tty.setraw(0)
os.write(1, b'\x1b[?1004h')
Path('focus-ready').touch()
with open('focus-events', 'ab', buffering=0) as events:
    while True:
        data = os.read(0, 128)
        if not data:
            break
        events.write(data)
"#).unwrap();
    let events = proj.join("focus-events");
    let mut h = Harness::builder().with_size([1000.0, 700.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    h.event(egui::Event::Text("python3 focus-probe.py".into()));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |_| proj.join("focus-ready").exists()));
    let contains = |bytes: &[u8]| std::fs::read(&events).unwrap_or_default().windows(bytes.len()).any(|window| window == bytes);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[I")), "enabling focus events in the active terminal sends FocusIn");
    std::fs::write(&events, []).unwrap();
    h.key_press_modifiers(cmd(), egui::Key::F);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[O")), "search steals focus from the terminal application");
    std::fs::write(&events, []).unwrap();
    h.key_press(egui::Key::Escape);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[I")), "closing search restores application focus");
    std::fs::write(&events, []).unwrap();
    h.key_press_modifiers(cmd(), egui::Key::T);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[O")), "a hidden terminal receives FocusOut when a new tab opens");
    std::fs::write(&events, []).unwrap();
    h.key_press_modifiers(cmd_shift(), egui::Key::OpenBracket);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[I")), "returning to the tab sends FocusIn");
    std::fs::write(&events, []).unwrap();
    h.state_mut().debug_open_settings(1);
    assert!(pump_until(&mut h, 5, |_| contains(b"\x1b[O")), "settings modal blurs the application even without typing");
}

#[test]
fn quick_terminal_owns_session_size_when_its_main_tab_is_visible() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("quick-size-owner");
    struct DaemonCleanup(PathBuf);
    impl Drop for DaemonCleanup { fn drop(&mut self) { shutdown(&self.0); } }
    let _cleanup = DaemonCleanup(base.clone());
    std::fs::write(proj.join("resize-probe.py"), r#"import os, signal
from pathlib import Path
log = os.open('resize-events', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
signal.signal(signal.SIGWINCH, lambda *_: os.write(log, b'R'))
Path('resize-ready').touch()
while True:
    signal.pause()
"#).unwrap();
    let mut h = Harness::builder().with_size([1200.0, 800.0]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|t| !t.trim().is_empty())));
    let client = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None).unwrap();
    let sessions = || match client.request(|req| kiln_proto::ClientMsg::ListSessions { req }, Duration::from_secs(2)).unwrap() {
        kiln_proto::ServerMsg::Sessions { sessions, .. } => sessions,
        other => panic!("unexpected {other:?}"),
    };
    let initial = sessions()[0].id;
    h.state_mut().debug_queue_action(kiln::app::Action::ToggleQuickTerminal);
    assert!(pump_until(&mut h, 10, |_| sessions().len() == 2));
    let quick = sessions().into_iter().find(|s| s.id != initial).unwrap().id;
    client.send(kiln_proto::ClientMsg::Input { session: quick, data: b"python3 resize-probe.py\r".to_vec() });
    assert!(pump_until(&mut h, 10, |_| proj.join("resize-ready").exists()));
    h.state_mut().debug_queue_action(kiln::app::Action::SelectPage(1));
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("빠른 터미널 창에서 열려 있습니다").is_some()));
    for _ in 0..8 { h.step(); std::thread::sleep(Duration::from_millis(30)); }
    let events = proj.join("resize-events");
    std::fs::write(&events, []).unwrap();
    for _ in 0..24 { h.step(); std::thread::sleep(Duration::from_millis(30)); }
    assert!(std::fs::read(&events).unwrap().is_empty(), "two views must not repeatedly resize the same PTY");
    // Returning transfers ownership and keeps the exact session, instead of
    // manufacturing a second terminal merely to avoid the conflicting sizes.
    h.state_mut().debug_queue_action(kiln::app::Action::ReturnQuickTerminal);
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("빠른 터미널 창에서 열려 있습니다").is_none()));
    assert_eq!(sessions().len(), 2);
    assert!(sessions().iter().any(|s| s.id == quick));
}

#[test]
fn workspace_task_rows_reveal_other_workspace_zoomed_split_and_quick_terminal() {
    use kiln::app::Action;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("workspace-task-navigation");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { shutdown(&self.0); } }
    let _cleanup = Cleanup(base.clone());
    let other = base.join("api-services");
    std::fs::create_dir_all(&other).unwrap();
    let mut h = Harness::builder().with_size([1280., 820.]).wgpu()
        .build_eframe(|cc| {
            let mut app = KilnApp::new(&cc.egui_ctx, Some(proj.clone()));
            app.debug_set_sidebar_width(320.);
            app.debug_set_shell("/bin/sh".into());
            app
        });
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| !text.trim().is_empty())));
    let client = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None).unwrap();
    let sessions = || match client.request(|req| kiln_proto::ClientMsg::ListSessions { req }, Duration::from_secs(2)).unwrap() {
        kiln_proto::ServerMsg::Sessions { sessions, .. } => sessions,
        other => panic!("unexpected response: {other:?}"),
    };
    let report = |session, title: &str, activity: &str| {
        let command = format!("printf '\\033]0;{title}\\007\\033]777;kiln-agent;{activity}\\007'; sleep 120\r");
        client.send(kiln_proto::ClientMsg::Input { session, data: command.into_bytes() });
    };
    let first = h.state().debug_focused_session().unwrap();
    report(first, "API contract review", "waiting");
    assert!(pump_until(&mut h, 8, |h| h.query_by_label("API contract review · 입력 대기").is_some()));
    h.key_press_modifiers(primary(), egui::Key::D);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_pane_count() == 2 && h.state().debug_focused_session().is_some_and(|session| session != first)));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| !text.trim().is_empty())));
    let second = h.state().debug_focused_session().unwrap();
    report(second, "Frontend authentication", "running");
    assert!(pump_until(&mut h, 8, |h| h.query_by_label("Frontend authentication · 작업 중").is_some()));
    h.state_mut().debug_queue_action(Action::ToggleZoom(None));
    h.run_steps(3);
    h.state_mut().debug_queue_action(Action::NewWorkspace(Some(other.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_active_workspace_root().canonicalize().ok() == other.canonicalize().ok() && h.state().debug_focused_session().is_some_and(|session| session != first && session != second)));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| !text.trim().is_empty())));
    let third = h.state().debug_focused_session().unwrap();
    report(third, "API tests complete", "done");
    assert!(pump_until(&mut h, 8, |h| h.query_by_label("API tests complete · 완료").is_some()));

    // A click in a nonactive workspace must reveal the exact split, including a
    // split hidden by that page's existing zoom, without creating another PTY.
    h.get_by_label("API contract review · 입력 대기").click();
    assert!(pump_until(&mut h, 5, |h| h.state().debug_focused_session() == Some(first) && h.state().debug_focused_is_visible()));
    assert_eq!(h.state().debug_active_workspace_root().canonicalize().unwrap(), proj.canonicalize().unwrap());
    h.get_by_label("Frontend authentication · 작업 중").click();
    assert!(pump_until(&mut h, 5, |h| h.state().debug_focused_session() == Some(second) && h.state().debug_focused_is_visible()));
    assert_eq!(sessions().len(), 3);

    h.state_mut().debug_queue_action(Action::ToggleQuickTerminal);
    assert!(pump_until(&mut h, 10, |_| sessions().len() == 4));
    let quick = sessions().into_iter().find(|session| ![first, second, third].contains(&session.id)).unwrap().id;
    report(quick, "Release handoff", "waiting");
    assert!(pump_until(&mut h, 8, |h| h.query_by_label("빠른 터미널 · 입력 대기").is_some()));
    // kittest embeds the native quick viewport over the sidebar and fixes its
    // position. The two ordinary rows above use actual pointer clicks; dispatch
    // the same row action here to verify quick-window ownership transfer.
    h.state_mut().debug_queue_action(Action::SelectPage(1));
    assert!(pump_until(&mut h, 5, |h| h.query_by_label("빠른 터미널 창에서 열려 있습니다").is_some()));
    let quick_pane = h.state().debug_focused_pane_id().unwrap();
    h.state_mut().debug_queue_action(Action::RevealPane(quick_pane));
    let revealed = pump_until(&mut h, 5, |h| h.state().debug_focused_session() == Some(quick) && h.state().debug_focused_is_visible() && h.query_by_label("빠른 터미널 창에서 열려 있습니다").is_none());
    assert!(revealed, "quick {quick:?}, focused {:?}, visible {}, placeholder {}", h.state().debug_focused_session(), h.state().debug_focused_is_visible(), h.query_by_label("빠른 터미널 창에서 열려 있습니다").is_some());
    assert_eq!(sessions().len(), 4, "task navigation must reuse the selected PTY");
    h.run_steps(4);
    save_shot(&mut h, "app_workspace_task_sidebar_320");
}

#[test]
fn closing_last_pane_keeps_workspace_and_replaces_terminal() {
    check_closing_last_work_surface(false);
}

#[test]
fn closing_last_page_keeps_workspace_and_replaces_terminal() {
    check_closing_last_work_surface(true);
}

fn check_closing_last_work_surface(close_page: bool) {
    use kiln::app::Action;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup(if close_page { "close-last-page" } else { "close-last-pane" });
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) { shutdown(&self.0); }
    }
    let _cleanup = Cleanup(base.clone());
    let other = base.join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    let mut h = Harness::builder().with_size([1280.0, 800.0])
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_session().is_some()));
    let original = h.state().debug_focused_session().unwrap();
    h.state_mut().debug_queue_action(Action::NewWorkspace(Some(other.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_session().is_some_and(|session| session != original)));
    let retained = h.state().debug_focused_session().unwrap();
    h.state_mut().debug_queue_action(Action::SelectWorkspace(0));
    assert!(pump_until(&mut h, 5, |h| h.state().debug_focused_session() == Some(original)));
    let original_pane = h.state().debug_focused_pane_id().unwrap();
    assert_eq!(h.state().debug_task_count(), 1);
    assert_eq!(h.state().debug_pane_count(), 1);

    // Removing the last page leaves a transient empty pages vector while NewPage
    // determines its cwd. This action previously panicked before creating a PTY.
    h.state_mut().debug_queue_action(if close_page { Action::ClosePage(0, true) } else { Action::ClosePane(original_pane, true) });
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_session().is_some_and(|session| session != original && session != retained)));
    let replacement = h.state().debug_focused_session().unwrap();
    assert_eq!(h.state().debug_workspace_count(), 2);
    assert_eq!(h.state().debug_active_workspace_root().canonicalize().unwrap(), proj.canonicalize().unwrap());
    assert_eq!(h.state().debug_task_count(), 1);
    assert_eq!(h.state().debug_pane_count(), 1);
    assert_ne!(h.state().debug_focused_pane_id(), Some(original_pane));
    let client = kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(), None).unwrap();
    let sessions = match client.request(|req| kiln_proto::ClientMsg::ListSessions { req }, Duration::from_secs(2)).unwrap() {
        kiln_proto::ServerMsg::Sessions { sessions, .. } => sessions,
        response => panic!("unexpected response: {response:?}"),
    };
    assert!(sessions.iter().any(|session| session.id == replacement && session.exited.is_none()));
    assert!(sessions.iter().any(|session| session.id == retained && session.exited.is_none()));
    assert!(sessions.iter().find(|session| session.id == original).is_none_or(|session| session.exited.is_some()));

    h.state_mut().debug_queue_action(Action::SelectWorkspace(1));
    assert!(pump_until(&mut h, 5, |h| h.state().debug_focused_session() == Some(retained)));
    client.send(kiln_proto::ClientMsg::Input { session: retained, data: b"printf 'other-workspace-%s\\n' alive\r".to_vec() });
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| text.contains("other-workspace-alive"))));
    assert_eq!(h.state().debug_active_workspace_root().canonicalize().unwrap(), other.canonicalize().unwrap());
}

#[test]
fn language_switch_updates_settings_and_preserves_terminal_content() {
    use kiln_common::i18n::{self, Language};
    use egui_kittest::kittest::{By, NodeT};
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("languages");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) { shutdown(&self.0); i18n::set_language(Language::Korean); }
    }
    let _cleanup = Cleanup(base.clone());
    let mut h = Harness::builder().with_size([1280.0, 800.0]).wgpu()
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(proj.clone())));
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| !text.trim().is_empty())));
    let session = h.state().debug_focused_session();
    let marker = "사용자 원문 / 日本語 / 中文";
    h.event(egui::Event::Text(format!("printf '%s\\n' '{marker}'")));
    h.key_press(egui::Key::Enter);
    assert!(pump_until(&mut h, 10, |h| h.state().debug_focused_text().is_some_and(|text| text.contains(marker))), "terminal: {:?}", h.state().debug_focused_text());
    for language in [Language::English, Language::Japanese, Language::ChineseSimplified, Language::Korean] {
        h.state_mut().debug_open_settings(0);
        h.run_steps(3);
        h.get_by_role_and_label(egui::accesskit::Role::ComboBox, i18n::tr("언어")).click();
        h.run_steps(2);
        h.get_by_label(language.native_name()).click();
        h.run_steps(3);
        assert_eq!(i18n::language(), language);
        assert_eq!(i18n::load_language(), language, "language must survive relaunch");
        for section in 0..6 {
            h.state_mut().debug_open_settings(section);
            h.run_steps(3);
            assert!(h.query_by_label(i18n::tr("닫기 (Esc)")).is_some(), "settings close is accessible: {language:?}, {section}");
            if language != Language::Korean {
                let untranslated: Vec<_> = h.query_all(By::new().predicate(|node| {
                    node.label().is_some_and(|label| {
                        label.chars().any(|c| ('가'..='힣').contains(&c)) && !label.contains(marker) && label != "한국어"
                    })
                })).map(|node| node.accesskit_node().label().unwrap_or_default().to_owned()).collect();
                assert!(untranslated.is_empty(), "untranslated app UI in {language:?}/{section}: {untranslated:?}");
            }
            if section == 0 {
                let directory = PathBuf::from("/tmp/kiln-localization-review");
                std::fs::create_dir_all(&directory).unwrap();
                h.render().unwrap().save(directory.join(format!("settings-{}.png",language.code()))).unwrap();
            }
        }
        h.key_press(egui::Key::Escape); h.run_steps(2);
        assert_eq!(h.state().debug_focused_session(), session);
        assert!(h.state().debug_focused_text().is_some_and(|text| text.contains(marker)), "user text must not be translated");
        // Text content alone misses stale font-atlas UVs: compare every rendered
        // galley against a fresh layout using the current font definitions.
        for clipped in &h.output().shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                let fresh = h.ctx.fonts_mut(|fonts| fonts.layout_job((*text.galley.job).clone()));
                let uv = |galley: &egui::Galley| galley.rows.iter()
                    .flat_map(|row| row.visuals.mesh.vertices.iter().map(|vertex| vertex.uv)).collect::<Vec<_>>();
                assert_eq!(uv(&text.galley), uv(&fresh), "stale font atlas after {language:?}: {:?}", text.galley.job.text);
            }
        }
    }
}

#[cfg(target_os="macos")]
#[test]
fn menu_bar_settings_request_reveals_existing_gui() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (base, proj) = setup("menu-settings");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {fn drop(&mut self){shutdown(&self.0);}}
    let _cleanup = Cleanup(base);
    let mut h = Harness::builder().with_size([1280.0,800.0])
        .build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(proj.clone())));
    h.run_steps(3);
    kiln::native_actions::request_settings().unwrap();
    h.run_steps(2);
    assert!(h.query_by_label("닫기 (Esc)").is_some());
    assert!(h.query_by_label("언어").is_some());
    assert!(!kiln::native_actions::take_settings_request());
}

#[test]
fn agent_title_animation_reports_activity_and_agent_colors_without_hooks() {
    use kiln::app::Action;
    let _serial = SERIAL.lock().unwrap_or_else(|e|e.into_inner());
    let (base,proj) = setup("claude-title-status");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {fn drop(&mut self){shutdown(&self.0);}}
    let _cleanup=Cleanup(base.clone());
    let mut h=Harness::builder().with_size([1280.0,800.0]).build_eframe(|cc| {
        let mut app=KilnApp::new(&cc.egui_ctx,Some(proj.clone()));
        app.debug_set_shell("/bin/sh".into());app
    });
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some()));
    let original=h.state().debug_focused_session().unwrap();
    h.state_mut().debug_queue_action(Action::NewPage);
    assert!(pump_until(&mut h,10,|h|h.state().debug_focused_session().is_some_and(|s|s!=original)&&h.state().debug_focused_text().is_some_and(|text|!text.trim().is_empty())));
    let session=h.state().debug_focused_session().unwrap();
    let client=kiln_daemon::client::Client::connect(&base.join("d.sock").to_string_lossy(),None).unwrap();
    // Replay the actual OSC title tokens emitted by Claude 2.1.289 through a PTY,
    // without hooks, process-name guesses or artificial telemetry.
    let theme=kiln_common::theme::Theme::current();
    for (frame,label,color) in [('◐',"작업 중",Some(theme.orange)),('◑',"작업 중",Some(theme.orange)),('✳',"세션 열림",None),('⠼',"작업 중",Some(theme.blue)),('◐',"작업 중",Some(theme.orange))] {
        client.send(kiln_proto::ClientMsg::Input{session,data:format!("printf '\\033]0;{frame} Claude lifecycle\\007'\r").into_bytes()});
        let expected=format!("Claude lifecycle · {label}");
        assert!(pump_until(&mut h,5,|h| {
            let arcs:Vec<_>=h.output().shapes.iter().filter_map(|shape|match &shape.shape {
                egui::Shape::Path(path) if path.points.len()==33 && path.stroke.width==1.7 => Some(&path.stroke.color),
                _=>None,
            }).collect();
            h.query_by_label(&expected).is_some() && match color {
                Some(color)=>arcs.len()==3 && arcs.iter().all(|actual|**actual==egui::epaint::ColorMode::Solid(color)),
                None=>arcs.is_empty(),
            }
        }),"missing {expected} with expected agent color");
        h.run_steps(2);
        let arcs:Vec<_>=h.output().shapes.iter().filter_map(|shape|match &shape.shape {
            egui::Shape::Path(path) if path.points.len()==33 && path.stroke.width==1.7 => Some(&path.stroke.color),
            _=>None,
        }).collect();
        if let Some(color)=color {
            assert_eq!(arcs.len(),3,"sidebar, tab and panel header must each animate");
            assert!(arcs.iter().all(|actual|**actual==egui::epaint::ColorMode::Solid(color)),"agent colors must agree across all surfaces");
        } else {assert!(arcs.is_empty(),"idle must stop animating");}
        if frame=='◑' || frame=='⠼' {
            h.render().unwrap().save(format!("/tmp/kiln-agent-color-{}.png",if frame=='◑'{"claude"}else{"codex"})).unwrap();
        }
        assert_eq!(h.state().debug_focused_session(),Some(session));
    }
}
