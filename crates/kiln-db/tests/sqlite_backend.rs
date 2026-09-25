//! SQLite 백엔드 통합 테스트(프로세스 내).

use kiln_db::edit::TableRef;
use kiln_db::export::ExportFormat;
use kiln_db::{ChangeSet, ConnConfig, DbManager, Driver, RowInsert, RowUpdate, TableKind, Value};

fn setup() -> (tempfile::TempDir, DbManager, kiln_db::ConnId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    let m = DbManager::in_memory();
    let id = m.add(
        ConnConfig {
            driver: Driver::Sqlite,
            file: path.to_string_lossy().into_owned(),
            name: "t".into(),
            ..Default::default()
        },
        None,
    );
    // 빈 DB 파일을 만든다.
    std::fs::write(&path, b"").unwrap();
    let ddl = r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            email VARCHAR(100),
            active BOOLEAN DEFAULT 1,
            score REAL,
            balance DECIMAL(10,2),
            meta JSON,
            avatar BLOB,
            created DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX idx_users_email ON users(email);
        CREATE TABLE posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            title TEXT
        );
        CREATE VIEW active_users AS SELECT id, name FROM users WHERE active = 1;
        CREATE TABLE nopk (a INTEGER, b TEXT);
    "#;
    for stmt in kiln_db::sql::split_statements(ddl, Driver::Sqlite) {
        m.block_on(m.query(id, &ddl[stmt], None)).unwrap();
    }
    for i in 1..=50 {
        let sql = format!(
            "INSERT INTO users (id, name, email, active, score, balance, meta, avatar) VALUES ({i}, 'user{i:02}', 'u{i}@x.io', {}, {}.5, '{}.25', '{{\"k\":{i}}}', X'DEADBEEF')",
            i % 2,
            i,
            i * 10
        );
        m.block_on(m.query(id, &sql, None)).unwrap();
    }
    (dir, m, id)
}

#[test]
fn introspection_lists_tables_views_columns_indexes_fks_and_ddl() {
    let (_d, m, id) = setup();
    let schemas = m.block_on(m.list_schemas(id)).unwrap();
    assert_eq!(schemas, vec!["main".to_string()]);
    let tables = m.block_on(m.list_tables(id, "main")).unwrap();
    let names: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert!(names.contains(&("users", TableKind::Table)));
    assert!(names.contains(&("active_users", TableKind::View)));
    assert!(!names.iter().any(|(n, _)| n.starts_with("sqlite_")));

    let t = TableRef::new(Some("main".into()), "users");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    assert_eq!(det.columns.len(), 9);
    assert!(det.columns[0].is_pk() && det.columns[0].auto_increment);
    assert!(!det.columns[1].nullable);
    assert_eq!(det.columns[3].default.as_deref(), Some("1"));
    assert!(
        det.indexes
            .iter()
            .any(|i| i.name == "idx_users_email" && i.columns == vec!["email"])
    );
    assert!(
        det.indexes
            .iter()
            .any(|i| i.unique && i.columns == vec!["name"])
    );

    let p = TableRef::new(Some("main".into()), "posts");
    let det = m.block_on(m.table_details(id, &p)).unwrap();
    assert_eq!(det.foreign_keys.len(), 1);
    assert_eq!(det.foreign_keys[0].ref_table, "users");
    assert_eq!(det.foreign_keys[0].on_delete, "CASCADE");
    assert_eq!(det.fk_target("user_id").as_deref(), Some("users.id"));

    let ddl = m.block_on(m.table_ddl(id, &t)).unwrap();
    assert!(ddl.contains("CREATE TABLE users"), "{ddl}");
    assert!(ddl.contains("CREATE INDEX idx_users_email"), "{ddl}");
}

#[test]
fn paging_sort_and_filter_are_server_side() {
    let (_d, m, id) = setup();
    let t = TableRef::new(Some("main".into()), "users");
    let p1 = m.block_on(m.fetch_page(id, &t, "", "", 20, 0)).unwrap();
    assert_eq!(p1.len(), 20);
    let p3 = m.block_on(m.fetch_page(id, &t, "", "", 20, 40)).unwrap();
    assert_eq!(p3.len(), 10);
    assert_eq!(m.block_on(m.count_rows(id, &t, "")).unwrap(), 50);

    let desc = m
        .block_on(m.fetch_page(
            id,
            &t,
            "",
            &kiln_db::edit::order_for(Driver::Sqlite, "name", true),
            5,
            0,
        ))
        .unwrap();
    assert_eq!(desc.rows[0][1], Value::Text("user50".into()));

    let f = m
        .block_on(m.fetch_page(id, &t, "score > 45.9", "id", 100, 0))
        .unwrap();
    assert_eq!(f.len(), 5);
    assert_eq!(f.rows[0][0], Value::Int(46));
    assert_eq!(m.block_on(m.count_rows(id, &t, "score > 45.9")).unwrap(), 5);
    assert!(
        m.block_on(m.fetch_page(id, &t, "nosuchcol = 1", "", 10, 0))
            .is_err()
    );
}

#[test]
fn values_decode_per_declared_type() {
    let (_d, m, id) = setup();
    let t = TableRef::new(Some("main".into()), "users");
    let rs = m
        .block_on(m.fetch_page(id, &t, "id = 3", "", 10, 0))
        .unwrap();
    let r = &rs.rows[0];
    assert_eq!(r[0], Value::Int(3));
    assert_eq!(r[1], Value::Text("user03".into()));
    assert_eq!(r[3], Value::Bool(true));
    assert_eq!(r[4], Value::Float(3.5));
    assert_eq!(r[6], Value::Json("{\"k\":3}".into()));
    assert_eq!(r[7], Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    assert!(matches!(r[8], Value::DateTime(_)));
    assert_eq!(&*rs.display[0][7], "0xDEADBEEF");
    let rs = m
        .block_on(m.query(
            id,
            "SELECT NULL AS n, 1.5 AS f, x'00ff' AS b, 'a\nb' AS t",
            None,
        ))
        .unwrap()
        .result;
    assert_eq!(rs.rows[0][0], Value::Null);
    assert_eq!(&*rs.display[0][0], "<null>");
    assert_eq!(&*rs.display[0][3], "a↵b");
}

#[test]
fn submit_applies_update_insert_delete_in_one_transaction() {
    let (_d, m, id) = setup();
    let t = TableRef::new(Some("main".into()), "users");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    let mut ins = vec![None; det.columns.len()];
    ins[1] = Some(Value::Text("newbie".into()));
    ins[2] = Some(Value::Null);
    let cs = ChangeSet {
        deletes: vec![vec![(0, Value::Int(2))]],
        updates: vec![RowUpdate {
            key: vec![(0, Value::Int(1))],
            sets: vec![
                (2, Value::Text("changed@x.io".into())),
                (4, Value::Float(9.25)),
            ],
        }],
        inserts: vec![RowInsert { values: ins }],
    };
    let n = m
        .block_on(m.submit_changes(id, &t, &det.columns, &cs))
        .unwrap();
    assert_eq!(n, 3);
    let rs = m
        .block_on(m.fetch_page(id, &t, "id IN (1,2)", "id", 10, 0))
        .unwrap();
    assert_eq!(rs.len(), 1);
    assert_eq!(rs.rows[0][2], Value::Text("changed@x.io".into()));
    assert_eq!(rs.rows[0][4], Value::Float(9.25));
    let rs = m
        .block_on(m.fetch_page(id, &t, "name = 'newbie'", "", 10, 0))
        .unwrap();
    assert_eq!(rs.len(), 1);
    assert_eq!(rs.rows[0][3], Value::Bool(true), "default applied");
    assert_eq!(rs.rows[0][0], Value::Int(51));
}

#[test]
fn submit_rolls_back_everything_when_one_statement_fails() {
    let (_d, m, id) = setup();
    let t = TableRef::new(Some("main".into()), "users");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    let cs = ChangeSet {
        deletes: vec![],
        updates: vec![
            RowUpdate {
                key: vec![(0, Value::Int(1))],
                sets: vec![(2, Value::Text("first@x.io".into()))],
            },
            RowUpdate {
                key: vec![(0, Value::Int(2))],
                sets: vec![(1, Value::Text("user03".into()))],
            },
        ],
        inserts: vec![],
    };
    let err = m
        .block_on(m.submit_changes(id, &t, &det.columns, &cs))
        .unwrap_err();
    assert_eq!(err.index, 1);
    assert!(
        err.error.message.to_lowercase().contains("unique"),
        "{}",
        err.error
    );
    let rs = m
        .block_on(m.fetch_page(id, &t, "id = 1", "", 10, 0))
        .unwrap();
    assert_eq!(rs.rows[0][2], Value::Text("u1@x.io".into()));

    let missing = ChangeSet {
        deletes: vec![vec![(0, Value::Int(999))]],
        ..Default::default()
    };
    let err = m
        .block_on(m.submit_changes(id, &t, &det.columns, &missing))
        .unwrap_err();
    assert!(err.error.message.contains("영향받은 행이 1개여야"));
}

#[test]
fn export_streams_all_rows_to_csv_and_json() {
    let (d, m, id) = setup();
    let csv = d.path().join("out.csv");
    let n = m
        .block_on(m.export_query(
            id,
            "SELECT id, name, meta FROM users ORDER BY id",
            &csv,
            ExportFormat::Csv,
        ))
        .unwrap();
    assert_eq!(n, 50);
    let text = std::fs::read_to_string(&csv).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 51);
    assert_eq!(lines[0], "id,name,meta");
    assert_eq!(lines[1], "1,user01,\"{\"\"k\"\":1}\"");

    let json = d.path().join("out.json");
    let n = m
        .block_on(m.export_query(
            id,
            "SELECT id, meta, avatar FROM users ORDER BY id",
            &json,
            ExportFormat::Json,
        ))
        .unwrap();
    assert_eq!(n, 50);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 50);
    assert_eq!(v[0]["id"], 1);
    assert_eq!(v[0]["meta"], "{\"k\":1}");
    assert_eq!(v[0]["avatar"], "0xDEADBEEF");
}

#[test]
fn console_session_keeps_state_and_reports_affected_rows() {
    let (_d, m, id) = setup();
    let s = m.block_on(m.open_session(id)).unwrap();
    m.block_on(s.run("CREATE TEMP TABLE tmp_x (v INTEGER)", None))
        .unwrap();
    let out = m
        .block_on(s.run("INSERT INTO tmp_x VALUES (1), (2), (3)", None))
        .unwrap();
    assert_eq!(out.affected, 3);
    assert!(!out.has_rows);
    let out = m.block_on(s.run("SELECT * FROM tmp_x", Some(2))).unwrap();
    assert!(out.has_rows && out.truncated);
    assert_eq!(out.result.len(), 2);
    let empty = m
        .block_on(s.run("SELECT v, v * 2 AS w FROM tmp_x WHERE v > 100", None))
        .unwrap();
    assert!(empty.has_rows);
    assert_eq!(empty.result.columns.len(), 2);
    let err = m.block_on(s.run("SELEC 1", None)).unwrap_err();
    assert!(err.message.contains("syntax"), "{err}");
}

#[test]
fn table_without_pk_has_no_pk_columns() {
    let (_d, m, id) = setup();
    let det = m
        .block_on(m.table_details(id, &TableRef::new(Some("main".into()), "nopk")))
        .unwrap();
    assert!(det.pk_columns().is_empty());
}
