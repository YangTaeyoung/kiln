//! 자동 줄 바꿈, 다중 커서, 코드 접기를 egui_kittest 로 구동하는 UI 테스트와 스냅샷.

mod common;

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use kiln_editor::{Editor, FoldRange, Pos, Selection};

const SAMPLE: &str = r#"use std::collections::HashMap;

/// Counts word frequencies in `text` and returns a map from each lowercase word to the number of times it appears in the input.
pub fn word_counts(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        // 소문자로 정규화한 뒤 개수를 센다. 긴 한국어 주석도 화면 너비에 맞춰 자연스럽게 다음 줄로 넘어가야 한다.
        let key = word.to_lowercase();
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let text = "the quick brown fox jumps over the lazy dog and keeps running far beyond the edge of the window";
    let counts = word_counts(text);
    println!("{} unique words", counts.len());
}
"#;

fn harness_sized(ed: Editor, w: f32, h: f32) -> Harness<'static, Editor> {
    let h = Harness::builder().with_size(egui::vec2(w, h)).wgpu().build_ui_state(
        |ui, ed: &mut Editor| {
            ed.ui(ui);
        },
        ed,
    );
    common::apply_theme(&h.ctx);
    h
}

fn harness(ed: Editor) -> Harness<'static, Editor> {
    harness_sized(ed, 820.0, 460.0)
}

fn focus(h: &mut Harness<'static, Editor>) {
    let id = h.state().id();
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run();
}

fn click(h: &mut Harness<'static, Editor>, p: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(p));
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers });
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run();
}

/// `line`(0부터) 번째 시각 줄 가운데의 y.
fn row_y(h: &Harness<'static, Editor>, row: usize) -> f32 {
    let (row_h, _, text) = h.state().layout_info();
    text.top() + row_h * row as f32 + row_h / 2.0
}

fn type_text(h: &mut Harness<'static, Editor>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
}

#[test]
fn alt_z_toggles_word_wrap_and_arrows_move_by_visual_row() {
    let long = format!("{}\nend", "word ".repeat(60));
    let mut h = harness_sized(Editor::from_text("a.txt", &long), 420.0, 300.0);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::ALT, Key::Z);
    // macOS 에서 Option+Z 가 함께 보내는 글자는 삽입되지 않는다.
    h.event(Event::Text("Ω".into()));
    h.run();
    assert!(h.state().word_wrap());
    assert!(!h.state().text().contains('Ω'));
    h.key_press(Key::ArrowDown);
    h.run();
    let st = h.state().status();
    assert_eq!(st.line, 1, "아래 화살표는 같은 줄의 다음 시각 줄로 간다");
    assert!(st.col > 1);
    h.key_press(Key::End);
    h.run();
    let col = h.state().status().col;
    assert!(col < 300, "End 는 시각 줄 끝에서 멈춘다: {col}");
    h.key_press_modifiers(Modifiers::ALT, Key::Z);
    h.run();
    assert!(!h.state().word_wrap());
}

#[test]
fn snapshot_editor_word_wrap() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.set_word_wrap(true);
    ed.goto(7, 20);
    let mut h = harness_sized(ed, 620.0, 460.0);
    h.run();
    focus(&mut h);
    h.run();
    h.snapshot("editor_word_wrap");
}

#[test]
fn alt_click_adds_cursors_and_typing_edits_all() {
    let mut h = harness(Editor::from_text("a.txt", "alpha\nbeta\ngamma\n"));
    h.run();
    focus(&mut h);
    let x = h.state().layout_info().2.left() + 300.0;
    // 줄 끝 오른쪽을 클릭하면 줄 끝에 커서가 놓인다.
    let (y0, y1, y2) = (row_y(&h, 0), row_y(&h, 1), row_y(&h, 2));
    click(&mut h, egui::pos2(x, y0), Modifiers::NONE);
    click(&mut h, egui::pos2(x, y1), Modifiers::ALT);
    click(&mut h, egui::pos2(x, y2), Modifiers::ALT);
    assert_eq!(h.state().cursor_count(), 3);
    type_text(&mut h, "!");
    assert_eq!(h.state().text(), "alpha!\nbeta!\ngamma!\n");
    h.key_press(Key::Backspace);
    h.key_press(Key::ArrowLeft);
    type_text(&mut h, "_");
    assert_eq!(h.state().text(), "alph_a\nbet_a\ngamm_a\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().text(), "alpha\nbeta\ngamma\n");
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().cursor_count(), 1);
}

#[test]
fn cmd_d_and_add_cursor_below_via_shortcuts() {
    let mut ed = Editor::from_text("a.rs", "let foo = foo + 1;\nlet bar = foo;\n");
    ed.set_selection(Selection::caret(Pos::new(0, 5)));
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.run();
    assert_eq!(h.state().cursor_count(), 3);
    type_text(&mut h, "qux");
    assert_eq!(h.state().text(), "let qux = qux + 1;\nlet bar = qux;\n");
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().cursor_count(), 1);
    h.state_mut().set_selection(Selection::caret(Pos::new(0, 0)));
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowDown);
    h.run();
    assert_eq!(h.state().cursor_count(), 2);
    type_text(&mut h, "// ");
    assert_eq!(h.state().text(), "// let qux = qux + 1;\n// let bar = qux;\n");
}

#[test]
fn snapshot_editor_multi_cursor() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.set_selection(Selection::new(Pos::new(4, 12), Pos::new(4, 18)));
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.run();
    h.state_mut().add_cursor(Pos::new(13, 4));
    h.run();
    h.snapshot("editor_multi_cursor");
}

#[test]
fn fold_shortcuts_and_gutter_chevron() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.goto(6, 9);
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::OpenBracket);
    h.run();
    assert_eq!(h.state().folded_ranges(), vec![FoldRange { start: 5, end: 8 }]);
    assert_eq!(h.state().status().line, 6);
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(h.state().status().line, 10, "접힌 줄은 건너뛴다");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::CloseBracket);
    h.key_press(Key::ArrowUp);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::CloseBracket);
    h.run();
    assert!(h.state().folded_ranges().is_empty());

    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    h.run();
    assert!(h.state().folded_ranges().len() >= 3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.key_press_modifiers(Modifiers::COMMAND, Key::J);
    h.run();
    assert!(h.state().folded_ranges().is_empty());

    // 줄 번호 여백의 접기 화살표를 누르면 접힌다(4번째 줄 `pub fn word_counts`).
    let gutter_x = h.state().layout_info().1.right() - 8.0;
    let y = row_y(&h, 3);
    h.event(Event::PointerMoved(egui::pos2(gutter_x, y)));
    h.run();
    click(&mut h, egui::pos2(gutter_x, y), Modifiers::NONE);
    assert_eq!(h.state().folded_ranges(), vec![FoldRange { start: 3, end: 10 }]);
}

#[test]
fn typing_in_fold_header_unfolds() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.fold_range(FoldRange { start: 3, end: 10 });
    ed.set_selection(Selection::caret(Pos::new(3, 0)));
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    type_text(&mut h, "x");
    assert!(h.state().folded_ranges().is_empty());
}

#[test]
fn snapshot_editor_folded() {
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.fold_range(FoldRange { start: 5, end: 8 });
    ed.fold_range(FoldRange { start: 13, end: 16 });
    ed.goto(4, 1);
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    let r = h.ctx.content_rect();
    h.event(Event::PointerMoved(egui::pos2(r.left() + 30.0, r.top() + 200.0)));
    h.run();
    h.snapshot("editor_folded");
}
