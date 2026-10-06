//! Editor 위젯을 egui_kittest 로 구동하는 UI 테스트와 스냅샷.

mod common;

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::Editor;

const SAMPLE: &str = r#"use std::collections::HashMap;

/// Counts word frequencies in `text`.
pub fn word_counts(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        // 소문자로 정규화
        let key = word.to_lowercase();
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fast = 1,
    Careful,
}

fn main() {
    let text = "the quick brown fox jumps over the lazy dog";
    let counts = word_counts(text);
    println!("{} unique words, mode {:?}", counts.len(), Mode::Fast);
}
"#;

fn harness(ed: Editor) -> Harness<'static, Editor> {
    let h = Harness::builder()
        .with_size(egui::vec2(820.0, 460.0))
        .wgpu()
        .build_ui_state(
            |ui, ed: &mut Editor| {
                if !common::fonts_ready(ui.ctx()) {
                    return;
                }
                ed.ui(ui);
            },
            ed,
        );
    common::apply_theme(&h.ctx);
    h
}

fn focus(h: &mut Harness<'static, Editor>) {
    let id = h.state().id();
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run();
}

#[test]
fn typing_editing_and_undo_through_ui() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("main.rs");
    std::fs::write(&p, "fn main() {}\n").unwrap();
    let mut h = harness(Editor::open(&p).unwrap());
    h.run();
    focus(&mut h);

    // 커서를 `{` 뒤로 옮긴다.
    h.state_mut().goto(1, 12);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    for c in "let x = 1;".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    assert_eq!(h.state().text(), "fn main() {\n    let x = 1;\n}\n");
    assert!(h.state().is_dirty());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run();
    assert_eq!(h.state().text(), "fn main() {\n    // let x = 1;\n}\n");

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().text(), "fn main() {\n    let x = 1;\n}\n");

    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run();
    assert!(!h.state().is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "fn main() {\n    let x = 1;\n}\n");

    let st = h.state().status();
    assert_eq!((st.line, st.language.as_str()), (2, "Rust"));
}

#[test]
fn clicking_places_caret_and_shift_arrows_select() {
    let mut h = harness(Editor::from_text("a.txt", "hello world\nsecond line\n"));
    h.run();
    // 두 번째 줄 텍스트 위를 클릭한다.
    let (row_h,_,rect) = h.state().layout_info();
    h.hover_at(egui::pos2(rect.left() + 30.0, rect.top() + row_h*1.5));
    h.event(Event::PointerButton {
        pos: egui::pos2(rect.left() + 30.0, rect.top() + row_h*1.5),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(Event::PointerButton {
        pos: egui::pos2(rect.left() + 30.0, rect.top() + row_h*1.5),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    let st = h.state().status();
    assert_eq!(st.line, 2);
    assert!(st.col > 1);
    h.key_press(Key::Home);
    h.key_press_modifiers(Modifiers::SHIFT, Key::End);
    h.run();
    assert_eq!(h.state().status().selected, "second line".len());
    h.event(Event::Text("X".into()));
    h.run();
    assert_eq!(h.state().text(), "hello world\nX\n");
}

#[test]
fn find_and_replace_bar_via_shortcuts() {
    let mut h = harness(Editor::from_text("a.rs", "let foo = foo + FOO;\n"));
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::F);
    h.run();
    for c in "foo".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.get_by_label("대/소문자 구분").click();
    h.run();
    // 바꾸기 입력칸으로 이동해 입력한다.
    let rid = h.state().id().with("find-replace");
    h.ctx.memory_mut(|m| m.request_focus(rid));
    h.run();
    for c in "bar".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.get_by_label("모두 바꾸기 (Cmd/Ctrl+Alt+Enter)").click();
    h.run();
    assert_eq!(h.state().text(), "let bar = bar + FOO;\n");
    h.key_press(Key::Escape);
    h.run();
}

#[test]
fn ctrl_g_goes_to_line() {
    let text: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let mut h = harness(Editor::from_text("a.txt", &text));
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::CTRL, Key::G);
    h.run();
    for c in "150:3".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.key_press(Key::Enter);
    h.run();
    let st = h.state().status();
    assert_eq!((st.line, st.col), (150, 3));
}

#[test]
fn snapshot_editor_rust() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.goto(9, 13);
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.run();
    h.snapshot("editor_rust");
}

#[test]
fn snapshot_editor_find_bar() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.goto(1, 1);
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::F);
    h.run();
    for c in "count".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.snapshot("editor_find_replace");
}

#[test]
fn snapshot_editor_lossy_banner() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("latin1.txt");
    std::fs::write(&bad, b"caf\xe9 au lait\nsecond line\n").unwrap();
    let mut h = harness(Editor::open(&bad).unwrap());
    h.run();
    h.snapshot("editor_lossy_banner");
}

#[test]
fn snapshot_editor_binary() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("blob.bin");
    std::fs::write(&bin, [0u8; 2048]).unwrap();
    let mut h = harness(Editor::open(&bin).unwrap());
    h.run();
    h.snapshot("editor_binary");
}
