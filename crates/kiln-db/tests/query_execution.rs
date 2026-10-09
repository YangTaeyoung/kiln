//! Execution scope is verified by real writes to an isolated SQLite database.
mod common;
use egui::text::{CCursor, CCursorRange};
use egui::{Key, Modifiers, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln_db::{DbManager, DbTab};
use std::time::{Duration, Instant};

#[test]
fn cursor_and_selection_execute_only_the_requested_sql() {
    let (_dir, manager, connection) = common::sqlite_fixture();
    let mut h = Harness::builder()
        .with_size(vec2(1000.0, 650.0))
        .build_ui_state(
            |ui, tab: &mut DbTab| {
                kiln_common::Theme::current().apply(ui.ctx());
                if common::install_korean_font(ui.ctx()) {
                    tab.ui(ui);
                }
            },
            DbTab::console(manager.clone(), connection),
        );
    h.run_steps(4);
    let text = "INSERT INTO audit_log(message) VALUES('must not run');\n    INSERT INTO audit_log(message) VALUES('한글 current');\nINSERT INTO audit_log(message) VALUES('selected one');\nINSERT INTO audit_log(message) VALUES('selected two');";
    h.state_mut().set_console_text(text);
    h.state_mut().request_focus();
    h.run_steps(3);
    let editor = h.ctx.memory(|m| m.focused()).unwrap();
    let caret = text[..text.find("    INSERT").unwrap()].chars().count();
    let mut state = egui::TextEdit::load_state(&h.ctx, editor).unwrap();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
    state.store(&h.ctx, editor);
    // No intervening frame: execution must consume the actual editor state.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    wait(&mut h, &manager, connection, 1);
    let result = manager
        .block_on(manager.query(connection, "SELECT message FROM audit_log", None))
        .unwrap();
    assert_eq!(
        result
            .result
            .rows
            .iter()
            .map(|row| row[0].to_text().unwrap())
            .collect::<Vec<_>>(),
        vec!["한글 current".to_string()]
    );
    let start = text[..text
        .find("INSERT INTO audit_log(message) VALUES('selected one')")
        .unwrap()]
        .chars()
        .count();
    let mut state = egui::TextEdit::load_state(&h.ctx, editor).unwrap();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(start),
        CCursor::new(text.chars().count()),
    )));
    state.store(&h.ctx, editor);
    h.run_steps(2);
    h.get_by_label("실행").click();
    wait(&mut h, &manager, connection, 3);
    let result = manager
        .block_on(manager.query(
            connection,
            "SELECT message FROM audit_log ORDER BY rowid",
            None,
        ))
        .unwrap();
    assert_eq!(
        result
            .result
            .rows
            .iter()
            .map(|row| row[0].to_text().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "한글 current".to_string(),
            "selected one".to_string(),
            "selected two".to_string()
        ]
    );
    // Whitespace selection must not fall through to another statement.
    h.state_mut().request_focus();
    h.run_steps(2);
    let mut state = egui::TextEdit::load_state(&h.ctx, editor).unwrap();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(caret),
        CCursor::new(caret + 4),
    )));
    state.store(&h.ctx, editor);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(4);
    assert_eq!(manager.history(connection).len(), 3);
}
fn wait(h: &mut Harness<'_, DbTab>, m: &DbManager, id: kiln_db::ConnId, count: usize) {
    let end = Instant::now() + Duration::from_secs(10);
    while m.history(id).len() < count
        || !h
            .query_by_label("취소")
            .is_some_and(|n| n.accesskit_node().is_disabled())
    {
        h.step();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(m.history(id).iter().all(|entry| entry.ok));
    h.run_steps(3);
}

#[test]
fn select_result_can_be_edited_and_applied_from_the_grid() {
    let (_dir, manager, connection) = common::sqlite_fixture();
    let mut h = Harness::builder()
        .with_size(vec2(1100.0, 700.0))
        .wgpu()
        .build_ui_state(
            |ui, tab: &mut DbTab| {
                kiln_common::Theme::current().apply(ui.ctx());
                if common::install_korean_font(ui.ctx()) {
                    tab.ui(ui);
                }
            },
            DbTab::console(manager.clone(), connection),
        );
    h.run_steps(4);
    h.state_mut()
        .set_console_text("SELECT id, email FROM customers WHERE id=1;");
    h.state_mut().request_focus();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    wait(&mut h, &manager, connection, 1);
    for expected in ["first@kiln.dev", "second@kiln.dev"] {
        h.run_steps(40);
        let header = h.get_by_label("email").rect();
        click(
            &mut h,
            egui::pos2(header.center().x, header.max.y + 13.0),
            false,
        );
        h.run_steps(3);
        h.key_press(Key::F2);
        h.run_steps(3);
        std::fs::create_dir_all("/tmp/kiln-db-captures").unwrap();
        h.render()
            .unwrap()
            .save(format!("/tmp/kiln-db-captures/after-double-{expected}.png"))
            .unwrap();
        let current = if expected == "first@kiln.dev" {
            "ada@example.com"
        } else {
            "first@kiln.dev"
        };
        h.get_by(|n| {
            n.role() == egui::accesskit::Role::TextInput && n.value().as_deref() == Some(current)
        })
        .focus();
        h.run_steps(2);
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.event(egui::Event::Text(expected.into()));
        h.run_steps(2);
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert_eq!(h.state().pending_changes(), 1);
        assert!(h.state().has_unsaved_changes());
        assert!(
            h.query_by_label("변경 1개 반영").is_some(),
            "email edit was not staged"
        );
        std::fs::create_dir_all("/tmp/kiln-db-captures").unwrap();
        h.render()
            .unwrap()
            .save(format!("/tmp/kiln-db-captures/before-run-{expected}.png"))
            .unwrap();
        let run_rect = h.get_by_label("실행").rect();
        // Running new SQL must preserve the pending edit.
        h.get_by_label("실행").click();
        h.run_steps(2);
        h.render()
            .unwrap()
            .save(format!("/tmp/kiln-db-captures/after-run-{expected}.png"))
            .unwrap();
        assert_eq!(
            h.state().pending_changes(),
            1,
            "value={expected}; run={run_rect:?}; history={:?}",
            manager.history(connection)
        );
        assert_eq!(manager.history(connection).len(), 1);
        h.get_by_label("변경 1개 반영").click();
        let end = Instant::now() + Duration::from_secs(10);
        while h.state().pending_changes() != 0
            || h.query_by_label_contains("1행 변경을 반영했습니다")
                .is_none()
        {
            h.step();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = manager
            .block_on(manager.query(connection, "SELECT email FROM customers WHERE id=1", None))
            .unwrap();
        assert_eq!(result.result.scalar_string().as_deref(), Some(expected));
        h.run_steps(3);
    }
    let output = std::path::Path::new("/tmp/kiln-db-captures");
    std::fs::create_dir_all(output).unwrap();
    h.render()
        .unwrap()
        .save(output.join("select-result-edit.png"))
        .unwrap();
    h.state_mut()
        .set_console_text("SELECT email FROM customers;");
    h.state_mut().request_focus();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    wait(&mut h, &manager, connection, 2);
    assert!(h.query_by_label("읽기 전용").is_some());
    assert!(h.query_by_label("변경 반영").is_none());
}
fn click<S>(h: &mut Harness<'_, S>, position: egui::Pos2, double: bool) {
    h.hover_at(position);
    h.run_steps(1);
    for _ in 0..if double { 2 } else { 1 } {
        h.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
}
