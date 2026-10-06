//! GUI intent -> reviewed plan -> real isolated SQLite -> refreshed table UI.
mod common;

use egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln_db::schema::{ColumnSpec, IndexColumn, IndexSpec, SchemaAction};
use kiln_db::{DbManager, DbTab, TableRef, TableSection, Value};
use std::time::{Duration, Instant};

fn wait(
    h: &mut Harness<'_, DbTab>,
    label: &str,
    mut ready: impl FnMut(&Harness<'_, DbTab>) -> bool,
) {
    let start = Instant::now();
    loop {
        h.step();
        if ready(h) {
            h.run_steps(3);
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "Timed out waiting for {label}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn apply_enabled(h: &Harness<'_, DbTab>) -> bool {
    !h.get_by_label("DB에 적용").accesskit_node().is_disabled()
}
fn preview(h: &mut Harness<'_, DbTab>) {
    wait(h, "schema preview enabled", |h| {
        !h.get_by_label("SQL 미리보기")
            .accesskit_node()
            .is_disabled()
    });
    assert!(!apply_enabled(h));
    h.get_by_label("SQL 미리보기").click();
    wait(h, "SQL review", |h| {
        h.query_by_label("SQL과 변경 대상을 확인했습니다.")
            .is_some()
    });
    assert!(!apply_enabled(h));
}
fn apply(h: &mut Harness<'_, DbTab>) {
    h.get_by_label("SQL과 변경 대상을 확인했습니다.").click();
    h.run_steps(2);
    assert!(apply_enabled(h));
    h.get_by_label("DB에 적용").click();
    wait(h, "schema operation to finish", |h| {
        h.query_by_label("SQL 미리보기").is_none()
    });
}
fn schema(m: &DbManager, id: kiln_db::ConnId, table: &str) -> kiln_db::TableDetails {
    m.block_on(m.table_details(id, &TableRef::new(None, table)))
        .unwrap()
}

#[test]
fn reviewed_gui_schema_changes_reach_sqlite_and_refresh_the_table() {
    kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
        let (_dir, m, id) = common::sqlite_fixture();
        m.block_on(m.query(id,"CREATE TABLE schema_gui_items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email VARCHAR(120))",None)).unwrap();
        m.block_on(m.query(
            id,
            "INSERT INTO schema_gui_items SELECT id, name, email FROM customers",
            None,
        ))
        .unwrap();
        let mut tab = DbTab::table(m.clone(), id, None, "schema_gui_items".into());
        tab.show_table_section(TableSection::Structure);
        let mut h = Harness::builder()
            .with_size([1000.0, 1000.0])
            .wgpu()
            .build_ui_state(
                |ui, tab: &mut DbTab| {
                    kiln_common::Theme::current().apply(ui.ctx());
                    if common::install_korean_font(ui.ctx()) {
                        tab.ui(ui);
                    }
                },
                tab,
            );
        wait(&mut h, "initial columns", |h| {
            h.query_by_label("VARCHAR(120)").is_some()
        });
        let original_count = m
            .block_on(m.count_rows(id, &TableRef::new(None, "schema_gui_items"), ""))
            .unwrap();

        h.state_mut()
            .request_schema_action(SchemaAction::AddColumn(ColumnSpec {
                name: "delivery_status".into(),
                data_type: "TEXT".into(),
                nullable: false,
                default: Some("'pending'".into()),
            }));
        h.run_steps(3);
        assert!(
            !schema(&m, id, "schema_gui_items")
                .columns
                .iter()
                .any(|c| c.name == "delivery_status")
        );
        preview(&mut h);
        assert!(
            !schema(&m, id, "schema_gui_items")
                .columns
                .iter()
                .any(|c| c.name == "delivery_status"),
            "Preview must not execute DDL"
        );
        h.render()
            .unwrap()
            .save("/tmp/kiln-schema-live-sql-preview.png")
            .unwrap();
        apply(&mut h);
        wait(&mut h, "refreshed column", |h| {
            h.query_by_label("delivery_status").is_some()
        });
        assert!(
            schema(&m, id, "schema_gui_items")
                .columns
                .iter()
                .any(|c| c.name == "delivery_status" && !c.nullable)
        );
        h.state_mut().show_table_section(TableSection::Data);
        wait(&mut h, "refreshed data header", |h| {
            h.query_by_label("delivery_status").is_some()
        });
        let data = m
            .block_on(m.query(
                id,
                "SELECT delivery_status FROM schema_gui_items ORDER BY id",
                None,
            ))
            .unwrap();
        assert_eq!(data.result.rows.len() as i64, original_count);
        assert!(
            data.result
                .rows
                .iter()
                .all(|r| r[0] == Value::Text("pending".into()))
        );

        h.state_mut().show_table_section(TableSection::Indexes);
        h.state_mut()
            .request_schema_action(SchemaAction::AddIndex(IndexSpec {
                name: "idx_delivery_status".into(),
                columns: vec![
                    IndexColumn {
                        name: "delivery_status".into(),
                        descending: true,
                    },
                    IndexColumn {
                        name: "id".into(),
                        descending: false,
                    },
                ],
                unique: false,
            }));
        h.run_steps(3);
        preview(&mut h);
        assert!(
            !schema(&m, id, "schema_gui_items")
                .indexes
                .iter()
                .any(|i| i.name == "idx_delivery_status")
        );
        apply(&mut h);
        wait(&mut h, "refreshed index", |h| {
            h.query_by_label("idx_delivery_status").is_some()
        });
        let detail = schema(&m, id, "schema_gui_items");
        let index = detail
            .indexes
            .iter()
            .find(|i| i.name == "idx_delivery_status")
            .unwrap();
        assert!(index.definition.contains("delivery_status") && index.definition.contains("DESC"));

        h.state_mut()
            .request_schema_action(SchemaAction::RenameTable {
                name: "schema_gui_items_renamed".into(),
            });
        h.run_steps(3);
        preview(&mut h);
        apply(&mut h);
        assert_eq!(
            h.state().table_ref().unwrap().table,
            "schema_gui_items_renamed"
        );
        assert_eq!(h.state().title(), "schema_gui_items_renamed");
        assert!(
            m.block_on(m.list_tables(id, "main"))
                .unwrap()
                .iter()
                .any(|t| t.name == "schema_gui_items_renamed")
        );
        assert_eq!(
            m.block_on(m.count_rows(id, h.state().table_ref().unwrap(), ""))
                .unwrap(),
            original_count
        );

        // Persist only intent. A restored drop form must still require a new preview,
        // review and exact table-name confirmation, and never replay the operation.
        h.state_mut().request_schema_action(SchemaAction::DropTable);
        h.run_steps(3);
        let draft = h.state().table_draft().unwrap();
        let draft: kiln_db::TableDraft =
            serde_json::from_slice(&serde_json::to_vec(&draft).unwrap()).unwrap();
        let mut restored = DbTab::table(m.clone(), id, None, "schema_gui_items_renamed".into());
        restored.restore_table_draft(&draft);
        *h.state_mut() = restored;
        h.run_steps(5);
        assert!(!apply_enabled(&h));
        assert_eq!(
            m.block_on(m.count_rows(id, h.state().table_ref().unwrap(), ""))
                .unwrap(),
            original_count
        );
        preview(&mut h);
        h.get_by_label("SQL과 변경 대상을 확인했습니다.").click();
        h.run_steps(2);
        assert!(
            !apply_enabled(&h),
            "Review alone cannot authorize table deletion"
        );
        h.set_size(egui::vec2(420.0 / 1.3, 440.0 / 1.3));
        h.run_steps(4);
        for key in ["취소", "SQL 미리보기", "DB에 적용"] {
            assert!(
                h.ctx
                    .content_rect()
                    .contains_rect(h.get_by_label(key).rect()),
                "{key}"
            );
        }
        h.render()
            .unwrap()
            .save("/tmp/kiln-schema-drop-confirm-420-130.png")
            .unwrap();
        h.set_size(egui::vec2(1000.0, 1000.0));
        h.run_steps(4);
        let input = h.get_by_label("테이블 이름");
        input.click();
        h.step();
        h.event(egui::Event::Text("wrong_table".into()));
        h.run_steps(2);
        assert!(!apply_enabled(&h));
        h.key_press_modifiers(Modifiers::COMMAND, Key::A);
        h.event(egui::Event::Text("schema_gui_items_renamed".into()));
        h.run_steps(2);
        assert!(apply_enabled(&h));
        h.get_by_label("DB에 적용").click();
        wait(&mut h, "deleted table state", |h| {
            h.state().table_ref().is_none()
        });
        assert!(
            h.query_by_label("테이블이 삭제되었습니다. 탐색기에서 다른 테이블을 여세요.")
                .is_some()
        );
        assert!(
            !m.block_on(m.list_tables(id, "main"))
                .unwrap()
                .iter()
                .any(|t| t.name == "schema_gui_items_renamed")
        );
        // A real FK failure must leave the form and original fixture rows intact.
        let mut protected = DbTab::table(m.clone(), id, None, "customers".into());
        protected.request_schema_action(SchemaAction::DropTable);
        *h.state_mut() = protected;
        h.run_steps(3);
        preview(&mut h);
        h.get_by_label("SQL과 변경 대상을 확인했습니다.").click();
        h.run_steps(2);
        h.get_by_label("테이블 이름").click();
        h.step();
        h.event(egui::Event::Text("customers".into()));
        h.run_steps(2);
        assert!(apply_enabled(&h));
        h.get_by_label("DB에 적용").click();
        wait(&mut h, "failed deletion with preserved form", |h| {
            h.query_by_label("SQL과 변경 대상을 확인했습니다.")
                .is_none()
                && h.query_by_label("SQL 미리보기")
                    .is_some_and(|n| !n.accesskit_node().is_disabled())
        });
        assert_eq!(h.state().table_ref().unwrap().table, "customers");
        assert!(h.state().table_draft().is_some());
        assert!(!apply_enabled(&h));
        assert_eq!(
            m.block_on(m.count_rows(id, &TableRef::new(None, "customers"), ""))
                .unwrap(),
            12
        );
        assert_eq!(
            m.block_on(m.count_rows(id, &TableRef::new(None, "orders"), ""))
                .unwrap(),
            12
        );
    });
}
