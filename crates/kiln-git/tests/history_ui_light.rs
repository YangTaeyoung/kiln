//! 이력(Log) 화면 라이트 테마 스냅샷. 테마는 전역 값이라 한 테스트 함수 안에서 차례로 그린다.

mod common;
mod history_common;

use egui::vec2;
use egui_kittest::kittest::Queryable;
use history_common::{harness, row, settle, ui_repo};
use kiln_common::Theme;

#[test]
fn history_light_theme_snapshots() {
    Theme::set_current("kiln-light");
    let r = ui_repo();

    let mut h = harness(r.path.clone(), vec2(1180.0, 520.0));
    settle(&mut h);
    h.snapshot("history_view_light");

    row(&h, "Tweak graph colors").click();
    h.run_steps(2);
    row(&h, "Polish detail pane").click_modifiers(egui::Modifiers::SHIFT);
    h.run_steps(2);
    row(&h, "Fix date column").click_secondary();
    h.run_steps(3);
    h.snapshot("history_context_menu_light");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    row(&h, "Add log view").click();
    h.run_steps(2);
    row(&h, "Add log view").click_secondary();
    h.run_steps(3);
    h.get_by_label_contains("고급 작업").click();h.run_steps(2);
    h.get_by_label("이 커밋부터 대화형 리베이스…").click();
    settle(&mut h);
    h.get_by_label("리베이스: Fix date column").click();
    h.run_steps(1);
    h.key_press(egui::Key::S);
    h.run_steps(2);
    h.get_by_label("리베이스: Tweak graph colors").click();
    h.run_steps(1);
    h.key_press(egui::Key::D);
    h.run_steps(2);
    h.get_by_label("리베이스: Fix date column").click();
    h.run_steps(2);
    h.snapshot("history_rebase_dialog_light");
    drop(h);

    Theme::set_current("kiln-dark");
}
