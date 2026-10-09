//! Real, isolated SELECT→edit→refresh paths. No credentials or user DBs.
use kiln_db::{
    ConnConfig, ConsoleSession, DbManager, Driver, ResultEditCell, ResultEditPlan,
    ResultReadOnlyReason, Value,
};

fn fixture() -> (
    tempfile::TempDir,
    DbManager,
    kiln_db::ConnId,
    ConsoleSession,
) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("result-edit.db");
    std::fs::write(&file, []).unwrap();
    let manager = DbManager::in_memory();
    let id = manager.add(
        ConnConfig {
            driver: Driver::Sqlite,
            file: file.to_string_lossy().into_owned(),
            ..Default::default()
        },
        None,
    );
    manager.block_on(manager.query(id,"CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE, note TEXT); INSERT INTO items VALUES(1,'Alpha',NULL),(2,'Beta','original'); CREATE VIEW item_view AS SELECT * FROM items; CREATE TABLE no_key(name TEXT); INSERT INTO no_key VALUES('same'),('same'); CREATE TABLE unique_key(code TEXT NOT NULL UNIQUE,value TEXT); INSERT INTO unique_key VALUES('one','a'); CREATE TABLE generated(id INTEGER PRIMARY KEY, value INTEGER, doubled INTEGER GENERATED ALWAYS AS (value*2) STORED); INSERT INTO generated(id,value) VALUES(1,3);",None)).unwrap();
    let session = manager.block_on(manager.open_session(id)).unwrap();
    (dir, manager, id, session)
}
fn plan(manager: &DbManager, session: &ConsoleSession, sql: &str) -> ResultEditPlan {
    let result = manager.block_on(session.run_editable(sql, None)).unwrap();
    result
        .editing
        .unwrap_or_else(|reason| panic!("{sql}: {reason}"))
}
fn cell(row: usize, column: usize, value: Value) -> ResultEditCell {
    ResultEditCell { row, column, value }
}
fn name(manager: &DbManager, id: kiln_db::ConnId, key: i64) -> String {
    manager
        .block_on(manager.query(id, &format!("SELECT name FROM items WHERE id={key}"), None))
        .unwrap()
        .result
        .scalar_string()
        .unwrap()
}
#[test]
fn selected_aliases_quoted_names_filters_and_subsequent_edits_use_base_columns() {
    let (_d, m, id, s) = fixture();
    let p = plan(
        &m,
        &s,
        "SELECT i.name AS label,i.id AS identity,i.note FROM main.items AS i WHERE i.id=2 ORDER BY i.id LIMIT 10",
    );
    assert_eq!(p.key_columns(), &[1]);
    assert_eq!(p.columns()[0].name, "name");
    assert_eq!(p.table().schema.as_deref(), Some("main"));
    let injection = "x'; DELETE FROM items; --";
    assert_eq!(
        m.block_on(s.submit_result_edits(&p, &[cell(0, 0, Value::Text(injection.into()))]))
            .unwrap(),
        1
    );
    assert_eq!(name(&m, id, 2), injection);
    assert_eq!(name(&m, id, 1), "Alpha");
    let refreshed = m.block_on(s.refresh_result_edit(&p, None)).unwrap();
    assert_eq!(refreshed.outcome.result.rows.len(), 1);
    let p = refreshed.editing.unwrap();
    assert_eq!(
        m.block_on(s.submit_result_edits(&p, &[cell(0, 0, Value::Text("second".into()))]))
            .unwrap(),
        1
    );
    assert_eq!(name(&m, id, 2), "second");
    m.block_on(m.query(id,"CREATE TABLE \"테이블 이름\"(\"키 번호\" INTEGER PRIMARY KEY,\"본문\" TEXT); INSERT INTO \"테이블 이름\" VALUES(1,'before')",None)).unwrap();
    let p = plan(
        &m,
        &s,
        "SELECT \"키 번호\" AS \"키 별칭\", \"본문\" FROM main.\"테이블 이름\" WHERE \"키 번호\"=1",
    );
    m.block_on(s.submit_result_edits(&p, &[cell(0, 1, Value::Text("after".into()))]))
        .unwrap();
    assert_eq!(
        m.block_on(m.query(id, "SELECT \"본문\" FROM \"테이블 이름\"", None))
            .unwrap()
            .result
            .scalar_string()
            .as_deref(),
        Some("after")
    );
}
#[test]
fn unsafe_query_shapes_missing_keys_and_ambiguous_projection_stay_read_only() {
    let (_d, m, _id, s) = fixture();
    for sql in [
        "SELECT i.* FROM items i JOIN items j ON i.id=j.id",
        "SELECT * FROM items, no_key",
        "SELECT id, upper(name) AS name FROM items",
        "SELECT id, count(*) FROM items GROUP BY id",
        "SELECT DISTINCT * FROM items",
        "SELECT * FROM items UNION ALL SELECT * FROM items",
        "WITH a AS(SELECT * FROM items) SELECT * FROM a",
        "SELECT * FROM (SELECT * FROM items)",
        "SELECT id,id,name FROM items",
        "SELECT * FROM item_view",
        "SELECT * FROM no_key",
        "SELECT name FROM items",
        "SELECT * FROM items WHERE id IN(SELECT id FROM items)",
    ] {
        let r = m.block_on(s.run_editable(sql, None)).unwrap();
        assert!(r.editing.is_err(), "{sql} unexpectedly editable");
    }
    let r = m
        .block_on(s.run_editable("SELECT * FROM no_key", None))
        .unwrap();
    assert_eq!(r.editing.unwrap_err(), ResultReadOnlyReason::NoUniqueKey);
    let r = m
        .block_on(s.run_editable("SELECT name FROM items", None))
        .unwrap();
    assert_eq!(r.editing.unwrap_err(), ResultReadOnlyReason::KeyNotSelected);
}
#[test]
fn alternate_unique_key_generated_columns_empty_results_and_casefolded_table_work() {
    let (_d, m, id, s) = fixture();
    let p = plan(&m, &s, "SELECT value, code AS identifier FROM unique_key");
    assert_eq!(p.key_columns(), &[1]);
    m.block_on(s.submit_result_edits(&p, &[cell(0, 0, Value::Text("b".into()))]))
        .unwrap();
    let p = plan(&m, &s, "SELECT * FROM generated");
    assert!(p.can_edit_column(1));
    assert!(!p.can_edit_column(2));
    assert!(
        m.block_on(s.submit_result_edits(&p, &[cell(0, 2, Value::Int(99))]))
            .is_err()
    );
    m.block_on(s.submit_result_edits(&p, &[cell(0, 1, Value::Int(4))]))
        .unwrap();
    assert_eq!(
        m.block_on(m.query(id, "SELECT doubled FROM generated", None))
            .unwrap()
            .result
            .scalar_string()
            .as_deref(),
        Some("8")
    );
    assert_eq!(
        plan(&m, &s, "SELECT * FROM ITEMS WHERE id=999").row_count(),
        0
    );
}
#[test]
fn stale_case_only_changes_and_deleted_rows_roll_back_the_entire_batch() {
    let (_d, m, id, s) = fixture();
    let p = plan(&m, &s, "SELECT * FROM items ORDER BY id");
    m.block_on(m.query(id, "UPDATE items SET name='BETA' WHERE id=2", None))
        .unwrap();
    let error = m
        .block_on(s.submit_result_edits(
            &p,
            &[
                cell(0, 1, Value::Text("mine1".into())),
                cell(1, 1, Value::Text("mine2".into())),
            ],
        ))
        .unwrap_err();
    assert_eq!(error.index, 1);
    assert_eq!(name(&m, id, 1), "Alpha");
    assert_eq!(name(&m, id, 2), "BETA");
    let p = plan(&m, &s, "SELECT * FROM items ORDER BY id");
    m.block_on(m.query(id, "DELETE FROM items WHERE id=2", None))
        .unwrap();
    assert!(
        m.block_on(s.submit_result_edits(
            &p,
            &[
                cell(0, 1, Value::Text("mine".into())),
                cell(1, 1, Value::Text("deleted".into()))
            ]
        ))
        .is_err()
    );
    assert_eq!(name(&m, id, 1), "Alpha");
}
#[test]
fn changed_schema_multiple_matches_never_commit_even_an_earlier_successful_update() {
    let (_d, m, id, s) = fixture();
    let p = plan(&m, &s, "SELECT * FROM items ORDER BY id");
    m.block_on(m.query(id,"DROP VIEW item_view; DROP TABLE items; CREATE TABLE items(id INTEGER,name TEXT,note TEXT); INSERT INTO items VALUES(1,'Alpha',NULL),(2,'Beta','original'),(2,'Beta','original');",None)).unwrap();
    let error = m
        .block_on(s.submit_result_edits(
            &p,
            &[
                cell(0, 1, Value::Text("must rollback".into())),
                cell(1, 1, Value::Text("must reject".into())),
            ],
        ))
        .unwrap_err();
    assert_eq!(error.index, 0);
    assert_eq!(name(&m, id, 1), "Alpha");
    assert_eq!(
        m.block_on(m.query(id, "SELECT count(*) FROM items WHERE name='Beta'", None))
            .unwrap()
            .result
            .scalar_string()
            .as_deref(),
        Some("2")
    );
}
#[test]
fn user_transaction_is_not_committed_and_temporary_shadow_is_not_misidentified() {
    let (_d, m, id, s) = fixture();
    m.block_on(s.run("BEGIN", None)).unwrap();
    m.block_on(s.run("UPDATE items SET note='uncommitted' WHERE id=1", None))
        .unwrap();
    let p = plan(&m, &s, "SELECT * FROM items WHERE id=1");
    assert!(
        m.block_on(s.submit_result_edits(&p, &[cell(0, 1, Value::Text("mine".into()))]))
            .is_err()
    );
    m.block_on(s.run("ROLLBACK", None)).unwrap();
    assert_eq!(name(&m, id, 1), "Alpha");
    assert!(
        m.block_on(m.query(id, "SELECT note FROM items WHERE id=1", None))
            .unwrap()
            .result
            .rows[0][0]
            .is_null()
    );
    m.block_on(s.run("CREATE TEMP TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO temp.items VALUES(1,'temporary')",None)).unwrap();
    let r = m
        .block_on(s.run_editable("SELECT * FROM items", None))
        .unwrap();
    assert_eq!(r.editing.unwrap_err(), ResultReadOnlyReason::NotBaseTable);
    let r = m
        .block_on(s.run_editable("SELECT * FROM main.items", None))
        .unwrap();
    assert!(r.editing.is_ok());
    assert_eq!(name(&m, id, 1), "Alpha");
}
#[test]
fn plans_cannot_cross_console_sessions_and_invalid_cells_do_not_mutate() {
    let (_d, m, id, s) = fixture();
    let p = plan(&m, &s, "SELECT * FROM items");
    let other = m.block_on(m.open_session(id)).unwrap();
    assert!(
        m.block_on(
            other.submit_result_edits(&p, &[cell(0, 1, Value::Text("wrong session".into()))])
        )
        .is_err()
    );
    for cells in [
        vec![cell(99, 1, Value::Null)],
        vec![cell(0, 99, Value::Null)],
        vec![
            cell(0, 1, Value::Null),
            cell(0, 1, Value::Text("duplicate".into())),
        ],
    ] {
        assert!(m.block_on(s.submit_result_edits(&p, &cells)).is_err());
    }
    assert_eq!(name(&m, id, 1), "Alpha");
    assert_eq!(
        m.block_on(s.submit_result_edits(&p, &[cell(0, 1, Value::Text("Alpha".into()))]))
            .unwrap(),
        0
    );
}

#[test]
fn identical_single_row_table_replacement_is_rejected_before_any_update() {
    let (_d, m, id, s) = fixture();
    let proof = plan(&m, &s, "SELECT * FROM items WHERE id=1");
    m.block_on(m.query(id,"DROP VIEW item_view; DROP TABLE items; CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE, note TEXT); INSERT INTO items VALUES(1,'Alpha',NULL)",None)).unwrap();
    let error = m
        .block_on(s.submit_result_edits(&proof, &[cell(0, 1, Value::Text("wrong table".into()))]))
        .unwrap_err();
    assert_eq!(error.index, 0);
    assert_eq!(name(&m, id, 1), "Alpha");
    let new = plan(&m, &s, "SELECT * FROM items WHERE id=1");
    assert_eq!(
        m.block_on(s.submit_result_edits(
            &new,
            &[cell(0, 1, Value::Text("new verified table".into()))]
        ))
        .unwrap(),
        1
    );
    assert_eq!(name(&m, id, 1), "new verified table");
}

#[test]
fn temporary_shadow_indexes_cannot_prove_a_permanent_table_unique_key() {
    let (_d, m, _id, s) = fixture();
    m.block_on(s.run("CREATE TABLE main.shadow_key(code TEXT NOT NULL,value TEXT); INSERT INTO main.shadow_key VALUES('one','a'); CREATE TEMP TABLE shadow_key(code TEXT NOT NULL UNIQUE,value TEXT);",None)).unwrap();
    let result = m
        .block_on(s.run_editable("SELECT * FROM main.shadow_key", None))
        .unwrap();
    assert_eq!(
        result.editing.unwrap_err(),
        ResultReadOnlyReason::NoUniqueKey
    );
    assert_eq!(result.outcome.result.rows[0][0], Value::Text("one".into()));
}
