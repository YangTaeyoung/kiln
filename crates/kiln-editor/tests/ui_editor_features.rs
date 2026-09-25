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

/// 고정폭 글꼴 한 글자 폭(px).
fn char_w(h: &Harness<'static, Editor>) -> f32 {
    let font = egui::TextStyle::Monospace.resolve(&h.ctx.global_style());
    h.ctx.fonts_mut(|f| f.glyph_width(&font, 'M'))
}

/// 시각 줄 `row`, 표시 열 `col` 의 화면 좌표.
fn cell(h: &Harness<'static, Editor>, row: usize, col: f32) -> Pos2 {
    let text = h.state().layout_info().2;
    egui::pos2(text.left() + 6.0 + col * char_w(h), row_y(h, row))
}

fn drag(h: &mut Harness<'static, Editor>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.event(Event::PointerMoved(from));
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.run();
    let mid = from + (to - from) * 0.5;
    h.event(Event::PointerMoved(mid));
    h.run();
    h.event(Event::PointerMoved(to));
    h.run();
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run();
}

const COLUMN_TEXT: &str = "let alpha = 1;\nlet b = 2;\nlet gamma = 3;\nx\n";

#[test]
fn alt_drag_makes_column_selection_and_typing_edits_every_line() {
    let mut h = harness(Editor::from_text("a.rs", COLUMN_TEXT));
    h.run();
    focus(&mut h);
    let (from, to) = (cell(&h, 0, 4.0), cell(&h, 3, 9.0));
    drag(&mut h, from, to, Modifiers::ALT);
    let r: Vec<(Pos, Pos)> = h.state().cursors().iter().map(|s| s.range()).collect();
    assert_eq!(
        r,
        vec![
            (Pos::new(0, 4), Pos::new(0, 9)),
            (Pos::new(1, 4), Pos::new(1, 9)),
            (Pos::new(2, 4), Pos::new(2, 9)),
            (Pos::new(3, 1), Pos::new(3, 1)),
        ],
        "시작 열보다 짧은 줄은 줄 끝 커서"
    );
    h.snapshot("editor_column_selection");
    type_text(&mut h, "Z");
    assert_eq!(h.state().text(), "let Z = 1;\nlet Z;\nlet Z = 3;\nxZ\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().text(), COLUMN_TEXT);
    assert_eq!(h.state().cursor_count(), 4, "되돌리면 사각 선택 커서가 모두 돌아온다");
}

#[test]
fn shift_alt_drag_starts_column_selection_at_press_point() {
    let mut h = harness(Editor::from_text("a.rs", COLUMN_TEXT));
    h.run();
    focus(&mut h);
    let (from, to) = (cell(&h, 2, 4.0), cell(&h, 0, 9.0));
    drag(&mut h, from, to, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(h.state().cursor_count(), 3);
    assert_eq!(h.state().selection(), Selection::new(Pos::new(0, 4), Pos::new(0, 9)), "끝 줄의 커서가 주 커서");
}

#[test]
fn alt_click_without_drag_still_adds_one_cursor() {
    let mut h = harness(Editor::from_text("a.rs", COLUMN_TEXT));
    h.run();
    focus(&mut h);
    let p = cell(&h, 2, 3.0);
    click(&mut h, p, Modifiers::ALT);
    assert_eq!(h.state().cursor_count(), 2);
}

#[test]
fn shift_alt_arrows_extend_column_selection() {
    let mut ed = Editor::from_text("a.rs", COLUMN_TEXT);
    ed.set_selection(Selection::caret(Pos::new(0, 4)));
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    let sa = Modifiers::ALT | Modifiers::SHIFT;
    h.key_press_modifiers(sa, Key::ArrowDown);
    h.key_press_modifiers(sa, Key::ArrowDown);
    for _ in 0..5 {
        h.key_press_modifiers(sa, Key::ArrowRight);
    }
    h.run();
    let r: Vec<(Pos, Pos)> = h.state().cursors().iter().map(|s| s.range()).collect();
    assert_eq!(r, vec![(Pos::new(0, 4), Pos::new(0, 9)), (Pos::new(1, 4), Pos::new(1, 9)), (Pos::new(2, 4), Pos::new(2, 9))]);
    type_text(&mut h, "Z");
    assert_eq!(h.state().text(), "let Z = 1;\nlet Z;\nlet Z = 3;\nx\n");
}

#[test]
fn line_command_shortcuts_apply_to_every_cursor() {
    const T: &str = "a();\nb();\nc();\n";
    let mut ed = Editor::from_text("a.rs", T);
    ed.set_cursors(Selection::caret(Pos::new(0, 1)), &[Selection::caret(Pos::new(0, 3)), Selection::caret(Pos::new(2, 0))]);
    let mut h = harness(ed);
    h.run();
    focus(&mut h);
    let heads = |h: &Harness<'static, Editor>| h.state().cursors().iter().map(|s| s.head).collect::<Vec<_>>();

    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run();
    assert_eq!(h.state().text(), "// a();\nb();\n// c();\n");
    assert_eq!(heads(&h), vec![Pos::new(0, 4), Pos::new(0, 6), Pos::new(2, 3)]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run();
    assert_eq!(h.state().text(), T);

    h.key_press_modifiers(Modifiers::COMMAND, Key::CloseBracket);
    h.run();
    assert_eq!(h.state().text(), "    a();\nb();\n    c();\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::OpenBracket);
    h.run();
    assert_eq!(h.state().text(), T);

    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run();
    assert_eq!(h.state().text(), "b();\na();\n\nc();");
    assert_eq!(heads(&h), vec![Pos::new(1, 1), Pos::new(1, 3), Pos::new(3, 0)]);
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run();
    assert_eq!(h.state().text(), T);

    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run();
    assert_eq!(h.state().text(), "a();\n\nb();\nc();\n\n");
    type_text(&mut h, "x");
    assert_eq!(h.state().text(), "a();\nx\nb();\nc();\nx\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().text(), T);
    assert_eq!(heads(&h), vec![Pos::new(0, 1), Pos::new(0, 3), Pos::new(2, 0)]);

    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
    h.run();
    assert_eq!(h.state().text(), "\na();\nb();\n\nc();\n");
    assert_eq!(heads(&h), vec![Pos::new(0, 0), Pos::new(3, 0)]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();

    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::K);
    h.run();
    assert_eq!(h.state().text(), "b();\n");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().text(), T);
    assert_eq!(h.state().cursor_count(), 3);
}

#[test]
fn end_on_wrapped_row_draws_caret_at_row_end() {
    let long = format!("{}\nend", "word ".repeat(30));
    let mut ed = Editor::from_text("a.txt", &long);
    ed.set_word_wrap(true);
    let mut h = harness_sized(ed, 420.0, 200.0);
    h.run();
    focus(&mut h);
    h.key_press(Key::End);
    h.run();
    let sel = h.state().selection();
    assert_eq!(sel.head.line, 0);
    assert!(sel.head.col > 0 && sel.head.col % 5 == 0, "시각 줄 끝(다음 줄 첫 글자 앞): {sel:?}");
    type_text(&mut h, "!");
    let text = h.state().text();
    assert_eq!(&text[sel.head.col - 1..=sel.head.col], " !", "마지막 글자 뒤에 들어간다");
    h.key_press(Key::Backspace);
    h.key_press(Key::Home);
    h.key_press(Key::End);
    h.run();
    h.snapshot("editor_wrap_row_end");
}
