//! Postgres 통합 테스트. `KILN_TEST_PG_URL` 이 없으면 건너뛴다(tests/run_docker_tests.sh 참고).

use kiln_db::edit::TableRef;
use kiln_db::{ChangeSet, ConnId, DbManager, RowInsert, RowUpdate, TableKind, Value};
use std::time::{Duration, Instant};

fn setup(schema: &str) -> Option<(DbManager, ConnId)> {
    let Ok(url) = std::env::var("KILN_TEST_PG_URL") else {
        eprintln!("KILN_TEST_PG_URL not set; skipping");
        return None;
    };
    let m = DbManager::in_memory();
    let id = m.import_url(&url).unwrap();
    let ddl = format!(
        r#"
        DROP SCHEMA IF EXISTS {schema} CASCADE;
        CREATE SCHEMA {schema};
        CREATE TYPE {schema}.mood AS ENUM ('sad', 'ok', 'happy');
        CREATE TABLE {schema}.things (
            id serial PRIMARY KEY,
            name varchar(20) NOT NULL DEFAULT 'x',
            j jsonb,
            u uuid,
            ts timestamptz,
            n numeric(10,2),
            arr int[],
            b bytea,
            m {schema}.mood,
            bits bit(3),
            vb varbit,
            t text,
            d date,
            tm time,
            iv interval,
            ip inet,
            flag boolean,
            big bigint,
            f float8,
            pt point
        );
        COMMENT ON TABLE {schema}.things IS 'all the types';
        CREATE INDEX things_name_idx ON {schema}.things (name, id);
        CREATE TABLE {schema}.child (
            id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            thing_id int NOT NULL REFERENCES {schema}.things(id) ON DELETE CASCADE
        );
        CREATE VIEW {schema}.named AS SELECT id, name FROM {schema}.things;
        INSERT INTO {schema}.things (name, j, u, ts, n, arr, b, m, bits, vb, t, d, tm, iv, ip, flag, big, f, pt) VALUES
          ('alpha', '{{"a": [1, 2]}}', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '2024-05-06 07:08:09+00', 12.5,
           '{{1,2,3}}', '\xdeadbeef', 'happy', B'101', B'11', E'multi\nline', '2024-02-29', '13:14:15',
           '1 day 2 hours', '10.0.0.1', true, 9007199254740993, 1.25, '(1,2)'),
          ('beta', 'null', NULL, NULL, NULL, '{{}}', NULL, 'sad', B'000', NULL, NULL, NULL, NULL, NULL, NULL, false, -1, -0.5, NULL);
        INSERT INTO {schema}.child (thing_id) VALUES (1), (1);
        "#
    );
    for r in kiln_db::sql::split_statements(&ddl, kiln_db::Driver::Postgres) {
        m.block_on(m.query(id, &ddl[r], None)).unwrap();
    }
    Some((m, id))
}

#[test]
fn pg_introspection_and_ddl() {
    let Some((m, id)) = setup("kiln_intro") else {
        return;
    };
    let schemas = m.block_on(m.list_schemas(id)).unwrap();
    assert!(schemas.contains(&"kiln_intro".to_string()));
    assert!(!schemas.iter().any(|s| s.starts_with("pg_")));
    let tables = m.block_on(m.list_tables(id, "kiln_intro")).unwrap();
    let kinds: Vec<_> = tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("child", TableKind::Table),
            ("named", TableKind::View),
            ("things", TableKind::Table)
        ]
    );
    assert_eq!(tables[2].comment, "all the types");

    let t = TableRef::new(Some("kiln_intro".into()), "things");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    assert_eq!(det.columns.len(), 20);
    assert!(det.columns[0].is_pk() && det.columns[0].auto_increment);
    assert_eq!(det.columns[1].data_type, "character varying(20)");
    assert_eq!(det.columns[1].cast_type, "character varying");
    assert!(!det.columns[1].nullable);
    assert_eq!(
        det.columns[1].default.as_deref(),
        Some("'x'::character varying")
    );
    assert!(
        det.indexes
            .iter()
            .any(|i| i.primary && i.columns == vec!["id"])
    );
    assert!(
        det.indexes
            .iter()
            .any(|i| i.name == "things_name_idx" && i.columns == vec!["name", "id"])
    );

    let c = TableRef::new(Some("kiln_intro".into()), "child");
    let cd = m.block_on(m.table_details(id, &c)).unwrap();
    assert!(cd.columns[0].auto_increment, "identity column");
    assert_eq!(cd.foreign_keys[0].ref_table, "things");
    assert_eq!(cd.foreign_keys[0].columns, vec!["thing_id"]);
    assert_eq!(cd.foreign_keys[0].on_delete, "CASCADE");

    let ddl = m.block_on(m.table_ddl(id, &t)).unwrap();
    assert!(
        ddl.starts_with("CREATE TABLE \"kiln_intro\".\"things\" ("),
        "{ddl}"
    );
    assert!(ddl.contains("PRIMARY KEY (id)"), "{ddl}");
    assert!(ddl.contains("CREATE INDEX things_name_idx"), "{ddl}");
    assert!(ddl.contains("COMMENT ON TABLE"), "{ddl}");
    let vddl = m
        .block_on(m.table_ddl(id, &TableRef::new(Some("kiln_intro".into()), "named")))
        .unwrap();
    assert!(vddl.starts_with("CREATE OR REPLACE VIEW"), "{vddl}");
}

#[test]
fn pg_decodes_many_column_types() {
    let Some((m, id)) = setup("kiln_types") else {
        return;
    };
    let t = TableRef::new(Some("kiln_types".into()), "things");
    let rs = m.block_on(m.fetch_page(id, &t, "", "id", 10, 0)).unwrap();
    let r = &rs.rows[0];
    assert_eq!(r[0], Value::Int(1));
    assert_eq!(r[2], Value::Json("{\"a\": [1, 2]}".into()));
    assert_eq!(
        r[3],
        Value::Uuid("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11".into())
    );
    assert!(matches!(&r[4], Value::Timestamptz(s) if s.starts_with("2024-05-06")));
    assert_eq!(r[5], Value::Decimal("12.50".into()));
    assert_eq!(r[6], Value::Array("{1,2,3}".into()));
    assert_eq!(r[7], Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
    assert_eq!(r[8], Value::Other("happy".into()));
    assert_eq!(r[9], Value::Other("101".into()));
    assert_eq!(r[11], Value::Text("multi\nline".into()));
    assert_eq!(r[12], Value::Date("2024-02-29".into()));
    assert_eq!(r[13], Value::Time("13:14:15".into()));
    assert_eq!(r[14], Value::Other("1 day 02:00:00".into()));
    assert_eq!(r[15], Value::Other("10.0.0.1".into()));
    assert_eq!(r[16], Value::Bool(true));
    assert_eq!(r[17], Value::Int(9007199254740993));
    assert_eq!(r[18], Value::Float(1.25));
    assert_eq!(r[19], Value::Other("(1,2)".into()));
    assert_eq!(&*rs.display[0][11], "multi↵line");
    assert_eq!(rs.rows[1][3], Value::Null);
    assert_eq!(rs.rows[1][2], Value::Json("null".into()));
}

#[test]
fn pg_edits_by_primary_key_with_typed_casts() {
    let Some((m, id)) = setup("kiln_edit") else {
        return;
    };
    let t = TableRef::new(Some("kiln_edit".into()), "things");
    let det = m.block_on(m.table_details(id, &t)).unwrap();
    let col = |n: &str| det.columns.iter().position(|c| c.name == n).unwrap();
    let mut ins = vec![None; det.columns.len()];
    ins[col("j")] = Some(Value::Json("{\"new\": true}".into()));
    let cs = ChangeSet {
        deletes: vec![vec![(0, Value::Int(2))]],
        updates: vec![RowUpdate {
            key: vec![(0, Value::Int(1))],
            sets: vec![
                (col("j"), Value::Json("{\"b\": 2}".into())),
                (
                    col("u"),
                    Value::Uuid("00000000-0000-0000-0000-000000000001".into()),
                ),
                (col("n"), Value::Decimal("99.99".into())),
                (col("arr"), Value::Array("{7,8}".into())),
                (col("b"), Value::Bytes(vec![1, 2, 3])),
                (col("m"), Value::Other("ok".into())),
                (col("bits"), Value::Other("111".into())),
                (
                    col("ts"),
                    Value::Timestamptz("2020-01-01 00:00:00+00".into()),
                ),
                (col("flag"), Value::Bool(false)),
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
    assert_eq!(r[col("n")], Value::Decimal("99.99".into()));
    assert_eq!(r[col("arr")], Value::Array("{7,8}".into()));
    assert_eq!(r[col("b")], Value::Bytes(vec![1, 2, 3]));
    assert_eq!(r[col("m")], Value::Other("ok".into()));
    assert_eq!(r[col("bits")], Value::Other("111".into()));
    assert_eq!(r[col("flag")], Value::Bool(false));
    assert_eq!(r[col("t")], Value::Null);
    let new = &rs.rows[1];
    assert_eq!(new[col("name")], Value::Text("x".into()), "DEFAULT applied");
    assert_eq!(new[col("j")], Value::Json("{\"new\": true}".into()));

    // varchar(20) 초과 → 오류, 앞선 변경까지 롤백.
    let bad = ChangeSet {
        updates: vec![
            RowUpdate {
                key: vec![(0, Value::Int(1))],
                sets: vec![(col("t"), Value::Text("kept?".into()))],
            },
            RowUpdate {
                key: vec![(0, Value::Int(1))],
                sets: vec![(col("name"), Value::Text("x".repeat(30)))],
            },
        ],
        ..Default::default()
    };
    let err = m
        .block_on(m.submit_changes(id, &t, &det.columns, &bad))
        .unwrap_err();
    assert_eq!(err.index, 1);
    assert_eq!(err.error.code.as_deref(), Some("22001"));
    let rs = m
        .block_on(m.fetch_page(id, &t, "id = 1", "", 10, 0))
        .unwrap();
    assert_eq!(rs.rows[0][col("t")], Value::Null);
}

#[test]
fn pg_errors_carry_position_and_cancel_stops_running_query() {
    let Some((m, id)) = setup("kiln_cancel") else {
        return;
    };
    let s = m.block_on(m.open_session(id)).unwrap();
    assert!(s.backend_id().is_some());
    let err = m
        .block_on(s.run("SELECT 1 FROM nosuchtable", None))
        .unwrap_err();
    assert_eq!(err.position, Some(15));
    assert_eq!(err.code.as_deref(), Some("42P01"));

    let s2 = s.clone();
    let job = m.spawn(async move { s2.run("SELECT pg_sleep(30)", None).await });
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    m.block_on(s.cancel()).unwrap();
    let res = job.wait().unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    let err = res.unwrap_err();
    assert_eq!(err.code.as_deref(), Some("57014"), "{err}");
    // 취소 후에도 같은 세션을 계속 쓸 수 있다.
    let ok = m.block_on(s.run("SELECT 42", None)).unwrap();
    assert_eq!(ok.result.rows[0][0], Value::Int(42));
}

#[test]
fn pg_console_session_keeps_transaction_state() {
    let Some((m, id)) = setup("kiln_session") else {
        return;
    };
    let s = m.block_on(m.open_session(id)).unwrap();
    m.block_on(s.run("BEGIN", None)).unwrap();
    m.block_on(s.run("DELETE FROM kiln_session.child", None))
        .unwrap();
    let inside = m
        .block_on(s.run("SELECT count(*) FROM kiln_session.child", None))
        .unwrap();
    assert_eq!(inside.result.rows[0][0], Value::Int(0));
    m.block_on(s.run("ROLLBACK", None)).unwrap();
    let after = m
        .block_on(m.query(id, "SELECT count(*) FROM kiln_session.child", None))
        .unwrap();
    assert_eq!(after.result.rows[0][0], Value::Int(2));
}
