//! MySQL 통합 테스트. `KILN_TEST_MYSQL_URL` 이 없으면 건너뛴다(tests/run_docker_tests.sh 참고).

use kiln_db::edit::TableRef;
use kiln_db::{ChangeSet, ConnId, DbManager, RowInsert, RowUpdate, TableKind, Value};
use std::time::{Duration, Instant};

fn setup(db: &str) -> Option<(DbManager, ConnId)> {
    let Ok(url) = std::env::var("KILN_TEST_MYSQL_URL") else {
        eprintln!("KILN_TEST_MYSQL_URL not set; skipping");
        return None;
    };
    let m = DbManager::in_memory();
    let id = m.import_url(&url).unwrap();
    let ddl = format!(
        r#"
        DROP DATABASE IF EXISTS {db};
        CREATE DATABASE {db};
        CREATE TABLE {db}.things (
            id INT AUTO_INCREMENT PRIMARY KEY,
            name VARCHAR(20) NOT NULL DEFAULT 'x',
            j JSON,
            dt DATETIME(3),
            ts TIMESTAMP NULL,
            dec_v DECIMAL(10,2),
            bl BLOB,
            vb VARBINARY(8),
            bits BIT(3),
            e ENUM('sad','ok','happy'),
            st SET('a','b','c'),
            flag TINYINT(1),
            big BIGINT UNSIGNED,
            d DATE,
            tm TIME,
            yr YEAR,
            f DOUBLE,
            t TEXT,
            KEY things_name_idx (name, id)
        ) COMMENT 'all the types';
        CREATE TABLE {db}.child (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            thing_id INT NOT NULL,
            CONSTRAINT fk_child_thing FOREIGN KEY (thing_id) REFERENCES {db}.things(id) ON DELETE CASCADE
        );
        CREATE VIEW {db}.named AS SELECT id, name FROM {db}.things;
        INSERT INTO {db}.things (name, j, dt, ts, dec_v, bl, vb, bits, e, st, flag, big, d, tm, yr, f, t) VALUES
          ('alpha', '{{"a": [1, 2]}}', '2024-05-06 07:08:09.123', '2024-05-06 07:08:09', 12.5, X'DEADBEEF', X'0102',
           b'101', 'happy', 'a,c', 1, 18446744073709551615, '2024-02-29', '-01:02:03', 2024, 1.25, 'multi\nline'),
          ('beta', NULL, NULL, NULL, NULL, NULL, NULL, b'000', 'sad', '', 0, 0, NULL, NULL, NULL, -0.5, NULL);
        INSERT INTO {db}.child (thing_id) VALUES (1), (1);
        "#
    );
    for r in kiln_db::sql::split_statements(&ddl, kiln_db::Driver::MySql) {
        m.block_on(m.query(id, &ddl[r], None)).unwrap();
    }
    Some((m, id))
}

#[test]
fn mysql_introspection_and_ddl() {
    let Some((m, id)) = setup("kiln_my_intro") else {
        return;
    };
    let schemas = m.block_on(m.list_schemas(id)).unwrap();
    assert!(schemas.contains(&"kiln_my_intro".to_string()));
    assert!(!schemas.contains(&"mysql".to_string()));
    let tables = m.block_on(m.list_tables(id, "kiln_my_intro")).unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("child", TableKind::Table),
            ("named", TableKind::View),
            ("things", TableKind::Table)
        ]
    );
    let t = TableRef::new(Some("kiln_my_intro".into()), "things");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    assert_eq!(det.columns.len(), 18);
    assert!(det.columns[0].is_pk() && det.columns[0].auto_increment);
    assert_eq!(det.columns[1].data_type, "varchar(20)");
    assert_eq!(det.columns[1].default.as_deref(), Some("x"));
    assert_eq!(det.columns[11].class, kiln_db::TypeClass::Bool);
    assert!(det.indexes.iter().any(|i| i.primary));
    assert!(
        det.indexes
            .iter()
            .any(|i| i.name == "things_name_idx" && i.columns == vec!["name", "id"])
    );
    let cd = m
        .block_on(m.table_details(id, &TableRef::new(Some("kiln_my_intro".into()), "child")))
        .unwrap();
    assert_eq!(cd.foreign_keys[0].name, "fk_child_thing");
    assert_eq!(cd.foreign_keys[0].on_delete, "CASCADE");
    let ddl = m.block_on(m.table_ddl(id, &t)).unwrap();
    assert!(ddl.starts_with("CREATE TABLE `things`"), "{ddl}");
    assert!(ddl.contains("COMMENT='all the types'"), "{ddl}");
}

#[test]
fn mysql_decodes_many_column_types() {
    let Some((m, id)) = setup("kiln_my_types") else {
        return;
    };
    let t = TableRef::new(Some("kiln_my_types".into()), "things");
    let rs = m.block_on(m.fetch_page(id, &t, "", "id", 10, 0)).unwrap();
    let r = &rs.rows[0];
    assert_eq!(r[0], Value::Int(1));
    assert_eq!(r[1], Value::Text("alpha".into()));
    assert_eq!(r[2], Value::Json("{\"a\": [1, 2]}".into()));
    assert_eq!(r[3], Value::DateTime("2024-05-06 07:08:09.123".into()));
    assert_eq!(r[4], Value::DateTime("2024-05-06 07:08:09".into()));
    assert_eq!(r[5], Value::Decimal("12.50".into()));
    assert_eq!(r[6], Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(r[7], Value::Bytes(vec![1, 2]));
    assert_eq!(r[8], Value::UInt(5));
    assert_eq!(r[9], Value::Text("happy".into()));
    assert_eq!(r[10], Value::Text("a,c".into()));
    assert_eq!(r[11], Value::Bool(true));
    assert_eq!(r[12], Value::UInt(u64::MAX));
    assert_eq!(r[13], Value::Date("2024-02-29".into()));
    assert_eq!(r[14], Value::Time("-01:02:03".into()));
    assert_eq!(r[15], Value::Int(2024));
    assert_eq!(r[16], Value::Float(1.25));
    assert_eq!(r[17], Value::Text("multi\nline".into()));
    assert_eq!(rs.rows[1][2], Value::Null);
    assert_eq!(rs.rows[1][11], Value::Bool(false));
}

#[test]
fn mysql_edits_by_primary_key_and_rolls_back_on_error() {
    let Some((m, id)) = setup("kiln_my_edit") else {
        return;
    };
    let t = TableRef::new(Some("kiln_my_edit".into()), "things");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    let col = |n: &str| det.columns.iter().position(|c| c.name == n).unwrap();
    let mut ins = vec![None; det.columns.len()];
    ins[col("j")] = Some(Value::Json("[1]".into()));
    let cs = ChangeSet {
        deletes: vec![vec![(0, Value::Int(2))]],
        updates: vec![RowUpdate {
            key: vec![(0, Value::Int(1))],
            sets: vec![
                (col("j"), Value::Json("{\"b\": 2}".into())),
                (col("dec_v"), Value::Decimal("99.99".into())),
                (col("bl"), Value::Bytes(vec![1, 2, 3])),
                (col("bits"), Value::UInt(7)),
                (col("e"), Value::Text("ok".into())),
                (col("flag"), Value::Bool(false)),
                (col("dt"), Value::DateTime("2020-01-01 10:00:00".into())),
                (col("t"), Value::Null),
            ],
        }],
        inserts: vec![RowInsert { values: ins }],
    };
    m.block_on(m.submit_changes(id, &t, &det.columns, &cs))
        .unwrap();
    let rs = m.block_on(m.fetch_page(id, &t, "", "id", 10, 0)).unwrap();
    assert_eq!(rs.len(), 2);
    let r = &rs.rows[0];
    assert_eq!(r[col("j")], Value::Json("{\"b\": 2}".into()));
    assert_eq!(r[col("dec_v")], Value::Decimal("99.99".into()));
    assert_eq!(r[col("bl")], Value::Bytes(vec![1, 2, 3]));
    assert_eq!(r[col("bits")], Value::UInt(7));
    assert_eq!(r[col("e")], Value::Text("ok".into()));
    assert_eq!(r[col("flag")], Value::Bool(false));
    assert_eq!(
        r[col("dt")],
        Value::DateTime("2020-01-01 10:00:00.000".into())
    );
    assert_eq!(r[col("t")], Value::Null);
    assert_eq!(rs.rows[1][col("name")], Value::Text("x".into()));

    let bad = ChangeSet {
        updates: vec![
            RowUpdate {
                key: vec![(0, Value::Int(1))],
                sets: vec![(col("t"), Value::Text("kept?".into()))],
            },
            RowUpdate {
                key: vec![(0, Value::Int(1))],
                sets: vec![(col("e"), Value::Text("nope".into()))],
            },
        ],
        ..Default::default()
    };
    let err = m
        .block_on(m.submit_changes(id, &t, &det.columns, &bad))
        .unwrap_err();
    assert_eq!(err.index, 1);
    let rs = m
        .block_on(m.fetch_page(id, &t, "id = 1", "", 10, 0))
        .unwrap();
    assert_eq!(rs.rows[0][col("t")], Value::Null);
}

#[test]
fn mysql_cancel_stops_running_query() {
    let Some((m, id)) = setup("kiln_my_cancel") else {
        return;
    };
    let s = m.block_on(m.open_session(id)).unwrap();
    assert!(s.backend_id().is_some());
    let s2 = s.clone();
    let job = m.spawn(async move { s2.run("SELECT SLEEP(30)", None).await });
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    m.block_on(s.cancel()).unwrap();
    let res = job.wait().unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cancel took {:?}",
        start.elapsed()
    );
    // SLEEP 은 중단되면 1 을 돌려준다.
    if let Ok(out) = res {
        assert_eq!(out.result.rows[0][0], Value::Int(1));
    }
    let ok = m.block_on(s.run("SELECT 42", None)).unwrap();
    assert_eq!(ok.result.rows[0][0], Value::Int(42));
}
