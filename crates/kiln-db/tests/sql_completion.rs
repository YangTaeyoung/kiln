//! Real SQLite metadata + editor interaction; no external DB or user configuration.
mod common;
use egui::text::{CCursor, CCursorRange};
use egui::{Event, ImeEvent, Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use kiln_db::{ConnConfig, ConnId, DbManager, DbTab};
use std::time::{Duration, Instant};

fn harness(m: DbManager, id: ConnId) -> Harness<'static, DbTab> {
    Harness::builder()
        .with_size(vec2(720.0, 520.0))
        .wgpu()
        .build_ui_state(
            |ui, tab: &mut DbTab| {
                kiln_common::Theme::current().apply(ui.ctx());
                if common::install_korean_font(ui.ctx()) {
                    tab.ui(ui);
                }
            },
            DbTab::console(m, id),
        )
}
fn until(h: &mut Harness<'_, DbTab>, label: &str) {
    let end = Instant::now() + Duration::from_secs(8);
    while h.query_by_label_contains(label).is_none() {
        h.step();
        if Instant::now() >= end {
            h.render()
                .unwrap()
                .save("/tmp/kiln-sql-failure.png")
                .unwrap();
            panic!(
                "no suggestion {label}; SQL={:?}; focus={:?}",
                h.state().console_text(),
                h.ctx.memory(|m| m.focused())
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(3);
}
fn sql(h: &mut Harness<'_, DbTab>, text: &str) {
    let byte = text.find('|').unwrap();
    let caret = text[..byte].chars().count();
    h.state_mut().set_console_text(&text.replace('|', ""));
    h.state_mut().request_focus();
    h.run_steps(3);
    let id = h.ctx.memory(|m| m.focused()).unwrap();
    let mut state = egui::TextEdit::load_state(&h.ctx, id).unwrap();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
    state.store(&h.ctx, id);
    h.run_steps(3);
}
#[test]
fn sql_completion_real_metadata_keyboard_undo_and_execution_are_independent() {
    let (_dir, m, id) = common::sqlite_fixture();
    let mut h = harness(m.clone(), id);
    h.run_steps(4);
    sql(&mut h, "SELECT * FROM cu|");
    until(&mut h, "customers · 테이블");
    h.key_press_modifiers(Modifiers::CTRL, Key::Space);
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT * FROM \"main\".\"customers\"")
    );
    assert!(
        m.history(id).is_empty(),
        "acceptance must never execute SQL"
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().console_text(), Some("SELECT * FROM cu"));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT * FROM \"main\".\"customers\"")
    );
    sql(&mut h, "SELECT c.em| FROM customers c");
    until(&mut h, "email · 컬럼");
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT c.email FROM customers c")
    );
    assert!(m.history(id).is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(3);
    let end = Instant::now() + Duration::from_secs(8);
    while m.history(id).is_empty() {
        h.step();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(m.history(id)[0].ok);
    sql(&mut h, "SELECT c.| FROM customers c");
    until(&mut h, "email · 컬럼");
    let before = h.state().console_text().unwrap().to_string();
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().console_text(), Some(before.as_str()));
    assert!(h.query_by_label_contains("email · 컬럼").is_none());
    h.key_press_modifiers(Modifiers::ALT, Key::Escape);
    h.run_steps(3);
    until(&mut h, "email · 컬럼");
    h.get_by_label_contains("email · 컬럼").click();
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT c.email FROM customers c")
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().console_text(), Some(before.as_str()));
}
#[test]
fn sql_completion_unicode_middle_token_selection_and_ime_preserve_input() {
    let (_dir, m, id) = common::sqlite_fixture();
    m.block_on(m.query(id, "ALTER TABLE customers ADD COLUMN \"이름\" TEXT", None))
        .unwrap();
    let mut h = harness(m.clone(), id);
    h.run_steps(4);
    sql(&mut h, "SELECT '🙂'; SELECT c.\"이|름\" FROM customers c");
    h.key_press_modifiers(Modifiers::CTRL, Key::Space);
    until(&mut h, "이름 · 컬럼");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT '🙂'; SELECT c.\"이름\" FROM customers c")
    );
    sql(&mut h, "SELECT * FROM cu|stomers");
    until(&mut h, "customers · 테이블");
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert_eq!(
        h.state().console_text(),
        Some("SELECT * FROM \"main\".\"customers\"")
    );
    sql(&mut h, "SELECT * FROM cu|");
    until(&mut h, "customers · 테이블");
    h.event(Event::Ime(ImeEvent::Preedit {
        text: "한".into(),
        active_range_chars: None,
    }));
    h.run_steps(2);
    assert!(
        h.query_by_label_contains("customers · 테이블").is_none(),
        "IME owns its candidate Enter"
    );
    h.event(Event::Ime(ImeEvent::Commit("한".into())));
    h.run_steps(3);
    assert!(h.state().console_text().unwrap().contains('한'));
    assert!(m.history(id).is_empty());
    sql(&mut h, "SELECT * FROM customers|");
    let editor = h.ctx.memory(|m| m.focused()).unwrap();
    let mut state = egui::TextEdit::load_state(&h.ctx, editor).unwrap();
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(14), CCursor::new(23))));
    state.store(&h.ctx, editor);
    h.key_press_modifiers(Modifiers::CTRL, Key::Space);
    h.run_steps(3);
    assert!(h.query_by_label_contains("customers · 테이블").is_none());
}
#[test]
fn sql_completion_refresh_and_replaced_connection_use_fresh_metadata() {
    let (_dir, m, id) = common::sqlite_fixture();
    let mut h = harness(m.clone(), id);
    h.run_steps(4);
    sql(&mut h, "SELECT * FROM cu|");
    until(&mut h, "customers · 테이블");
    m.block_on(m.query(id, "CREATE TABLE current_jobs (job_id INTEGER)", None))
        .unwrap();
    h.get_by_label("자동완성 메타데이터 새로고침").click();
    h.run_steps(3);
    sql(&mut h, "SELECT * FROM current_j|");
    until(&mut h, "current_jobs · 테이블");
    let other = tempfile::tempdir().unwrap();
    let file = other.path().join("second.db");
    std::fs::write(&file, []).unwrap();
    let cfg = m.get(id).unwrap();
    m.update(
        ConnConfig {
            file: file.to_string_lossy().into_owned(),
            ..cfg
        },
        None,
    );
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(
        !h.state()
            .console_text()
            .unwrap()
            .contains("\"current_jobs\""),
        "old connection candidate must be rejected immediately"
    );
    m.block_on(m.query(id, "CREATE TABLE replacement (id INTEGER)", None))
        .unwrap();
    sql(&mut h, "SELECT * FROM repl|");
    until(&mut h, "replacement · 테이블");
    sql(&mut h, "SELECT * FROM cu|");
    h.key_press_modifiers(Modifiers::CTRL, Key::Space);
    h.run_steps(10);
    assert!(h.query_by_label_contains("customers · 테이블").is_none());
    m.disconnect(id);
    h.run_steps(12);
    std::thread::sleep(Duration::from_millis(30));
    h.run_steps(3);
    assert_eq!(
        m.status(id),
        kiln_db::ConnStatus::Disconnected,
        "completion must not reconnect a closed connection"
    );
    drop(h);
}
#[test]
fn sql_completion_popup_is_bounded_in_four_languages_themes_and_small_panes() {
    use kiln_common::i18n::{self, Language};
    let (_dir, m, id) = common::sqlite_fixture();
    let mut h = harness(m, id);
    h.run_steps(4);
    for theme in ["kiln-dark", "kiln-light"] {
        kiln_common::Theme::set_current(theme);
        for language in Language::ALL {
            i18n::set_language(language);
            for (width, height, scale) in [(720.0, 520.0, 1.0), (420.0, 380.0, 1.3)] {
                h.ctx.set_zoom_factor(scale);
                h.run_steps(3);
                h.set_size(vec2(width, height));
                h.run_steps(3);
                assert!(
                    (h.ctx.content_rect().width() - width).abs() < 1.0,
                    "viewport expanded: {:?}",
                    h.ctx.content_rect()
                );
                sql(&mut h, "SELECT c.| FROM customers c");
                let label = format!("email · {}", i18n::tr("컬럼"));
                until(&mut h, &label);
                for label in [&label, i18n::tr("자동완성 메타데이터 새로고침")] {
                    assert!(
                        h.ctx
                            .content_rect()
                            .contains_rect(h.get_by_label_contains(label).rect()),
                        "{theme} {language:?} {width}: {label} clipped"
                    );
                }
                h.render()
                    .unwrap()
                    .save(format!(
                        "/tmp/kiln-sql-completion-{theme}-{}-{width}.png",
                        language.code()
                    ))
                    .unwrap();
                h.key_press(Key::Escape);
                h.run_steps(2);
            }
        }
    }
    i18n::set_language(Language::Korean);
    kiln_common::Theme::set_current("kiln-dark");
}

#[test]
fn sql_completion_ddl_and_modified_enter_preserve_editor_shortcuts() {
    let (_dir, m, id) = common::sqlite_fixture();
    let mut h = harness(m.clone(), id);
    h.run_steps(4);
    sql(&mut h, "SELECT * FROM cu|");
    until(&mut h, "customers · 테이블");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run_steps(3);
    assert!(h.state().console_text().unwrap().contains("cu\n"));
    assert!(m.history(id).is_empty());
    sql(&mut h, "CREATE TABLE console_jobs (job_id INTEGER)|");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(3);
    let end = Instant::now() + Duration::from_secs(8);
    while m.history(id).is_empty() {
        h.step();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(m.history(id)[0].ok);
    sql(&mut h, "SELECT * FROM console_j|");
    until(&mut h, "console_jobs · 테이블");
}

#[test]
fn sql_completion_server_metadata_when_isolated_backends_are_configured() {
    for (env, driver) in [
        ("KILN_TEST_PG_URL", kiln_db::Driver::Postgres),
        ("KILN_TEST_MYSQL_URL", kiln_db::Driver::MySql),
    ] {
        let Ok(url) = std::env::var(env) else {
            eprintln!("{env} not set; server completion path skipped");
            continue;
        };
        let m = DbManager::in_memory();
        let id = m.import_url(&url).unwrap();
        let schema = if driver == kiln_db::Driver::Postgres {
            format!("completion_{}", std::process::id())
        } else {
            m.get(id).unwrap().database
        };
        if driver == kiln_db::Driver::Postgres {
            m.block_on(m.query(id, &format!("CREATE SCHEMA {schema}"), None))
                .unwrap();
        }
        let table = kiln_db::sql::qualified(driver, Some(&schema), "completion_people");
        m.block_on(m.query(
            id,
            &format!("CREATE TABLE {table} (id INTEGER, name VARCHAR(120))"),
            None,
        ))
        .unwrap();
        let mut h = harness(m.clone(), id);
        h.run_steps(4);
        sql(&mut h, &format!("SELECT * FROM {schema}.completion_p|"));
        until(&mut h, "completion_people · 테이블");
        h.key_press(Key::Tab);
        h.run_steps(3);
        assert!(
            h.state()
                .console_text()
                .unwrap()
                .ends_with("completion_people")
        );
        sql(&mut h, &format!("SELECT p.na| FROM {table} p"));
        until(&mut h, "name · 컬럼");
        h.key_press(Key::Tab);
        h.run_steps(3);
        assert_eq!(
            h.state().console_text().unwrap(),
            format!("SELECT p.name FROM {table} p")
        );
        drop(h);
        m.block_on(m.query(id, &format!("DROP TABLE {table}"), None))
            .unwrap();
        if driver == kiln_db::Driver::Postgres {
            m.block_on(m.query(id, &format!("DROP SCHEMA {schema}"), None))
                .unwrap();
        }
        m.disconnect(id);
    }
}

#[test]
fn sql_completion_long_column_names_can_be_distinguished_before_insertion() {
    let (_dir, m, id) = common::sqlite_fixture();
    let names = [
        "customer_notification_delivery_preference_primary",
        "customer_notification_delivery_preference_secondary",
    ];
    for name in names {
        m.block_on(m.query(
            id,
            &format!("ALTER TABLE customers ADD COLUMN {name} TEXT"),
            None,
        ))
        .unwrap();
    }
    let mut h = harness(m, id);
    h.run_steps(4);
    h.ctx.set_zoom_factor(1.3);
    h.run_steps(3);
    h.set_size(vec2(420.0, 380.0));
    h.run_steps(3);
    sql(&mut h, "SELECT c.customer_notification_| FROM customers c");
    until(&mut h, &format!("{} · 컬럼", names[0]));
    h.render()
        .unwrap()
        .save("/tmp/kiln-sql-completion-long-columns.png")
        .unwrap();
    for name in names {
        let label = format!("{name} · 컬럼");
        let node = h.get_by_label_contains(&label);
        let center = node.rect().center();
        h.event(Event::PointerMoved(center));
        h.run_steps(40);
        h.render()
            .unwrap()
            .save("/tmp/kiln-sql-completion-long-tooltip.png")
            .unwrap();
        assert!(
            h.query_by_label_contains(&format!("{name}\n컬럼"))
                .is_some(),
            "full name must be visible in the hover tooltip"
        );
    }
    h.render()
        .unwrap()
        .save("/tmp/kiln-sql-completion-long-tooltip.png")
        .unwrap();
}
