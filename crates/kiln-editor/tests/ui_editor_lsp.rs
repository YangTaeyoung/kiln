//! 가짜 언어 서버를 붙인 편집기를 egui_kittest 로 구동한다: 진단 밑줄, 호버, 완성, 정의·참조, 이름 바꾸기, 서식.

mod common;

use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use kiln_editor::lsp::{LspConfig, LspManager, ServerSpec};
use kiln_editor::{Editor, EditorEvent, Pos, Selection};

const SRC: &str = "fn helper() -> i32 {\n    41 + 1\n}\n\nfn main() {\n    let value = helper();\n    helper(); bad meh\n    external();\n}\n";

fn manager(root: &Path) -> LspManager {
    let mut cfg = LspConfig::default();
    cfg.languages.insert("rust".into(), ServerSpec::new(env!("CARGO_BIN_EXE_kiln-fake-lsp"), &[]));
    LspManager::with_config(root.to_path_buf(), cfg)
}

/// 임시 작업 공간(정규화한 경로)에 `main.rs` 를 만들고 언어 서버를 붙여 연다.
fn setup(text: &str) -> (tempfile::TempDir, PathBuf, Harness<'static, Editor>) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("main.rs");
    std::fs::write(&p, text).unwrap();
    let ed = Editor::open_with_lsp(&p, Some(manager(&root))).unwrap();
    assert!(ed.lsp_attached());
    let h = Harness::builder().with_size(egui::vec2(820.0, 420.0)).wgpu().build_ui_state(
        |ui, ed: &mut Editor| {
            ed.ui(ui);
        },
        ed,
    );
    common::apply_theme(&h.ctx);
    let mut h = h;
    h.run_ok();
    let id = h.state().id();
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run_ok();
    assert!(common::wait_until(&mut h, 10.0, |h| {
        let st = h.state();
        st.lsp().is_some_and(|m| m.is_ready(&p))
    }));
    (dir, root, h)
}

fn type_text(h: &mut Harness<'static, Editor>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run_ok();
}

fn wait_popup(h: &mut Harness<'static, Editor>, kind: &str) -> bool {
    let kind = kind.to_owned();
    common::wait_until(h, 10.0, move |h| h.state().lsp_popup_kind() == Some(kind.as_str()))
}

#[test]
fn diagnostics_squiggles_gutter_markers_and_hover() {
    let (_d, _root, mut h) = setup(SRC);
    assert!(common::wait_until(&mut h, 10.0, |h| h.state().diagnostics().len() == 2));
    h.state_mut().set_selection(Selection::caret(Pos::new(8, 1)));
    h.run_ok();
    h.snapshot("editor_lsp_diagnostics");

    // Cmd+K Cmd+I: 커서 위치 호버. 진단 메시지와 호버 텍스트가 함께 나온다.
    h.state_mut().set_selection(Selection::caret(Pos::new(6, 15)));
    h.run_ok();
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.key_press_modifiers(Modifiers::COMMAND, Key::I);
    assert!(wait_popup(&mut h, "hover"));
    h.snapshot("editor_lsp_hover");
    h.key_press(Key::Escape);
    h.run_ok();
    assert_eq!(h.state().lsp_popup_kind(), None);

    // 마우스를 단어 위에 멈추면 호버가 뜬다.
    let (row_h, _, text) = h.state().layout_info();
    let p = egui::pos2(text.left() + 6.0 + 7.5 * 8.0, text.top() + row_h * 5.5);
    h.event(Event::PointerMoved(p));
    h.run_ok();
    std::thread::sleep(std::time::Duration::from_millis(450));
    assert!(wait_popup(&mut h, "hover"));
}

#[test]
fn completion_popup_filters_and_accepts_text_edit_and_snippet() {
    let (_d, _root, mut h) = setup("fn main() {\n    x\n}\n");
    h.state_mut().set_selection(Selection::caret(Pos::new(1, 5)));
    h.run_ok();
    type_text(&mut h, ".");
    assert!(wait_popup(&mut h, "completion"));
    h.snapshot("editor_lsp_completion");
    type_text(&mut h, "b");
    h.run_ok();
    assert_eq!(h.state().lsp_popup_kind(), Some("completion"));
    h.key_press(Key::Enter);
    h.run_ok();
    assert_eq!(h.state().text(), "fn main() {\n    x.beta_edit\n}\n");
    assert_eq!(h.state().lsp_popup_kind(), None);

    // Ctrl+Space 로 연 목록에서 스니펫을 고르면 자리표시 텍스트로 들어가고 커서는 첫 탭 정지로 간다.
    h.key_press(Key::Enter);
    type_text(&mut h, "pr");
    assert!(wait_popup(&mut h, "completion"));
    h.key_press(Key::Tab);
    h.run_ok();
    assert!(h.state().text().contains("    println!(\"msg\")\n"), "{}", h.state().text());
    let st = h.state().status();
    assert_eq!((st.line, st.col), (3, 15));
    h.key_press_modifiers(Modifiers::CTRL, Key::Space);
    assert!(wait_popup(&mut h, "completion"));
    h.key_press(Key::Escape);
    h.run_ok();
    assert_eq!(h.state().lsp_popup_kind(), None);
}

#[test]
fn definition_references_and_cross_file_open_event() {
    let (_d, root, mut h) = setup(SRC);
    h.state_mut().set_selection(Selection::caret(Pos::new(5, 18)));
    h.run_ok();
    h.key_press(Key::F12);
    assert!(common::wait_until(&mut h, 10.0, |h| h.state().selection().head == Pos::new(0, 3)));

    h.state_mut().set_selection(Selection::caret(Pos::new(7, 6)));
    h.run_ok();
    h.key_press(Key::F12);
    let mut events = Vec::new();
    assert!(common::wait_until(&mut h, 10.0, |h| {
        events.extend(h.state_mut().take_events());
        !events.is_empty()
    }));
    assert_eq!(events[0], EditorEvent::OpenAt { path: root.join("other.rs"), line: 1, col: 1 });

    // Shift+F12: 참조 목록에서 두 번째 항목으로 이동.
    h.state_mut().set_selection(Selection::caret(Pos::new(0, 4)));
    h.run_ok();
    h.key_press_modifiers(Modifiers::SHIFT, Key::F12);
    assert!(wait_popup(&mut h, "list"));
    h.snapshot("editor_lsp_references");
    h.key_press(Key::ArrowDown);
    h.key_press(Key::Enter);
    h.run_ok();
    assert_eq!(h.state().selection().head, Pos::new(5, 16));
    assert_eq!(h.state().lsp_popup_kind(), None);

    // Cmd+클릭으로도 정의로 간다.
    let (row_h, _, text) = h.state().layout_info();
    let p = egui::pos2(text.left() + 6.0 + 7.5 * 20.0, text.top() + row_h * 5.5);
    h.event(Event::ModifiersChanged(Modifiers::COMMAND));
    h.event(Event::PointerMoved(p));
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::COMMAND });
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::COMMAND });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_ok();
    assert!(common::wait_until(&mut h, 10.0, |h| h.state().selection().head == Pos::new(0, 3)));
}

#[test]
fn rename_edits_open_buffer_and_closed_file_then_format() {
    let (_d, root, mut h) = setup(SRC);
    let other = root.join("other.rs");
    std::fs::write(&other, "pub fn helper() {}\n").unwrap();
    h.state_mut().set_selection(Selection::caret(Pos::new(0, 5)));
    h.run_ok();
    h.key_press(Key::F2);
    assert!(wait_popup(&mut h, "rename"));
    h.snapshot("editor_lsp_rename");
    type_text(&mut h, "assist");
    h.key_press(Key::Enter);
    assert!(common::wait_until(&mut h, 10.0, |h| h.state().text().contains("fn assist()")));
    assert_eq!(h.state().text().matches("assist").count(), 3);
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "pub fn assist() {}\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_ok();
    assert!(h.state().text().contains("fn helper()"), "이름 바꾸기는 되돌리기 한 번으로 돌아간다");

    // 서식: 줄 끝 공백을 지운다.
    h.state_mut().set_selection(Selection::caret(Pos::new(1, 10)));
    h.run_ok();
    type_text(&mut h, "   ");
    assert!(h.state().text().contains("41 + 1   \n"));
    h.key_press_modifiers(Modifiers::SHIFT | Modifiers::ALT, Key::F);
    assert!(common::wait_until(&mut h, 10.0, |h| h.state().text().contains("41 + 1\n")));
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_ok();
    assert!(!h.state().is_dirty());
}

#[test]
fn editor_without_server_shows_injected_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("notes.txt");
    std::fs::write(&p, "첫 줄 텍스트\n두 번째 줄에 문제가 있음\n").unwrap();
    let m = LspManager::with_config(root.clone(), LspConfig::default());
    let ed = Editor::open_with_lsp(&p, Some(m.clone())).unwrap();
    assert!(!ed.lsp_attached());
    use kiln_editor::lsp::{Diagnostic, Position, Range, Severity};
    m.inject_diagnostics(
        p.clone(),
        vec![Diagnostic {
            range: Range { start: Position::new(1, 3), end: Position::new(1, 6) },
            severity: Severity::Warning,
            message: "주입한 경고".into(),
            source: None,
            code: None,
        }],
    );
    let mut h = Harness::builder().with_size(egui::vec2(500.0, 120.0)).wgpu().build_ui_state(
        |ui, ed: &mut Editor| {
            ed.ui(ui);
        },
        ed,
    );
    common::apply_theme(&h.ctx);
    h.run_ok();
    assert_eq!(h.state().diagnostics().len(), 1);
    h.snapshot("editor_lsp_injected_diagnostic");
}

#[test]
fn signature_help_follows_arguments_and_closes_on_paren() {
    let (_d, _root, mut h) = setup("fn main() {\n    \n}\n");
    h.state_mut().set_selection(Selection::caret(Pos::new(1, 4)));
    h.run_ok();
    type_text(&mut h, "f(");
    assert!(wait_popup(&mut h, "signature"));
    type_text(&mut h, "1, ");
    assert!(wait_popup(&mut h, "signature"));
    h.snapshot("editor_lsp_signature");
    type_text(&mut h, "2)");
    assert_eq!(h.state().lsp_popup_kind(), None);
    assert_eq!(h.state().text(), "fn main() {\n    f(1, 2)\n}\n");
}
