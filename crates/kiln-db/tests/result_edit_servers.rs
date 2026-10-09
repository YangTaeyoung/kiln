//! Optional disposable-server fixtures. Requires the same explicit test URLs as
//! pg_backend/mysql_backend; each test owns and removes only a fresh random schema.
use kiln_db::{ConsoleSession, DbManager, Driver, ResultEditCell, Value};
fn run_fixture(variable: &str, driver: Driver) {
    let Ok(url) = std::env::var(variable) else {
        eprintln!("{variable} not set; disposable-server result-edit coverage skipped");
        return;
    };
    let manager = DbManager::in_memory();
    let id = manager.import_url(&url).unwrap();
    // Keep the directory reservation alive until the owned server schema is removed.
    let owned_namespace = tempfile::Builder::new()
        .prefix("kiln_result_")
        .tempdir()
        .unwrap();
    let namespace = owned_namespace
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let quoted = kiln_db::sql::quote_ident(driver, &namespace);
    let create = if driver == Driver::Postgres {
        format!("CREATE SCHEMA {quoted}")
    } else {
        format!("CREATE DATABASE {quoted} CHARACTER SET utf8mb4")
    };
    manager.block_on(manager.query(id, &create, None)).unwrap();
    let test = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let table = format!("{quoted}.{}", kiln_db::sql::quote_ident(driver, "items"));
        let dtype = if driver == Driver::Postgres {
            "TEXT"
        } else {
            "VARCHAR(100) COLLATE utf8mb4_general_ci"
        };
        manager.block_on(manager.query(id,&format!("CREATE TABLE {table}(id INTEGER PRIMARY KEY,name {dtype},note {dtype}); INSERT INTO {table} VALUES(1,'Alpha',NULL),(2,'Beta','original')"),None)).unwrap();
        let session: ConsoleSession = manager.block_on(manager.open_session(id)).unwrap();
        let sql = format!(
            "SELECT name AS label,id AS identifier,note FROM {table} WHERE id>0 ORDER BY id"
        );
        let result = manager.block_on(session.run_editable(&sql, None)).unwrap();
        let plan = result.editing.unwrap();
        assert_eq!(plan.key_columns(), &[1]);
        manager
            .block_on(manager.query(
                id,
                &format!("UPDATE {table} SET name='BETA' WHERE id=2"),
                None,
            ))
            .unwrap();
        let error = manager
            .block_on(session.submit_result_edits(
                &plan,
                &[
                    ResultEditCell {
                        row: 0,
                        column: 0,
                        value: Value::Text("must rollback".into()),
                    },
                    ResultEditCell {
                        row: 1,
                        column: 0,
                        value: Value::Text("stale".into()),
                    },
                ],
            ))
            .unwrap_err();
        assert_eq!(error.index, 1);
        assert_eq!(
            manager
                .block_on(manager.query(id, &format!("SELECT name FROM {table} WHERE id=1"), None))
                .unwrap()
                .result
                .scalar_string()
                .as_deref(),
            Some("Alpha")
        );
        let fresh = manager
            .block_on(session.refresh_result_edit(&plan, None))
            .unwrap()
            .editing
            .unwrap();
        assert_eq!(
            manager
                .block_on(session.submit_result_edits(
                    &fresh,
                    &[ResultEditCell {
                        row: 0,
                        column: 0,
                        value: Value::Text("x'; DELETE FROM items; --".into())
                    }]
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            manager
                .block_on(manager.query(id, &format!("SELECT count(*) FROM {table}"), None))
                .unwrap()
                .result
                .scalar_string()
                .as_deref(),
            Some("2")
        );
        for query in [
            format!("SELECT name FROM {table}"),
            format!("SELECT id,upper(name) FROM {table}"),
            format!("SELECT a.* FROM {table} a JOIN {table} b ON a.id=b.id"),
        ] {
            assert!(
                manager
                    .block_on(session.run_editable(&query, None))
                    .unwrap()
                    .editing
                    .is_err()
            );
        }
        if driver == Driver::Postgres {
            let types = format!("{quoted}.types");
            manager.block_on(manager.query(id,&format!("CREATE TABLE {types}(id INTEGER PRIMARY KEY,label CHARACTER(12),enabled BOOLEAN,weight DOUBLE PRECISION,variable_text VARCHAR(30),plain_text TEXT); INSERT INTO {types} VALUES(1,'Alpha',true,1e30,'with space ','with space ')"),None)).unwrap();
            let proof = manager
                .block_on(session.run_editable(&format!("SELECT * FROM {types}"), None))
                .unwrap()
                .editing
                .unwrap();
            assert_eq!(
                manager
                    .block_on(session.submit_result_edits(
                        &proof,
                        &[ResultEditCell {
                            row: 0,
                            column: 1,
                            value: Value::Text("longer label".into())
                        }]
                    ))
                    .unwrap(),
                1
            );
            assert_eq!(
                manager
                    .block_on(manager.query(id, &format!("SELECT label FROM {types}"), None))
                    .unwrap()
                    .result
                    .scalar_string()
                    .unwrap()
                    .trim_end(),
                "longer label"
            );
            // CHAR padding is normalized, but a concurrent removal of a trailing
            // VARCHAR or TEXT space must still make the snapshot stale.
            for column in ["variable_text", "plain_text"] {
                let proof = manager
                    .block_on(session.run_editable(&format!("SELECT * FROM {types}"), None))
                    .unwrap()
                    .editing
                    .unwrap();
                manager
                    .block_on(manager.query(
                        id,
                        &format!("UPDATE {types} SET {column}='with space' WHERE id=1"),
                        None,
                    ))
                    .unwrap();
                let error = manager
                    .block_on(session.submit_result_edits(
                        &proof,
                        &[ResultEditCell {
                            row: 0,
                            column: 1,
                            value: Value::Text("must reject".into()),
                        }],
                    ))
                    .unwrap_err();
                assert_eq!(error.index, 0);
                assert_eq!(
                    manager
                        .block_on(manager.query(id, &format!("SELECT label FROM {types}"), None))
                        .unwrap()
                        .result
                        .scalar_string()
                        .unwrap()
                        .trim_end(),
                    "longer label"
                );
            }
        }
        manager.block_on(session.run("BEGIN", None)).unwrap();
        manager
            .block_on(session.run(
                &format!("UPDATE {table} SET note='not committed' WHERE id=1"),
                None,
            ))
            .unwrap();
        // Submit through another transaction must not commit this console write.
        let read = manager
            .block_on(manager.query(id, &format!("SELECT note FROM {table} WHERE id=1"), None))
            .unwrap();
        assert!(read.result.rows[0][0].is_null());
        manager.block_on(session.run("ROLLBACK", None)).unwrap();
        let old = manager
            .block_on(session.run_editable(&sql, None))
            .unwrap()
            .editing
            .unwrap();
        let backup = format!(
            "{quoted}.{}",
            kiln_db::sql::quote_ident(driver, "original_items")
        );
        let rename = if driver == Driver::Postgres {
            format!("ALTER TABLE {table} RENAME TO original_items")
        } else {
            format!("RENAME TABLE {table} TO {backup}")
        };
        manager.block_on(manager.query(id, &rename, None)).unwrap();
        manager
            .block_on(manager.query(
                id,
                &format!("CREATE VIEW {table} AS SELECT * FROM {backup}"),
                None,
            ))
            .unwrap();
        let blocked = manager
            .block_on(session.submit_result_edits(
                &old,
                &[ResultEditCell {
                    row: 0,
                    column: 2,
                    value: Value::Text("wrong origin".into()),
                }],
            ))
            .unwrap_err();
        assert_eq!(blocked.index, 0);
        assert!(
            manager
                .block_on(manager.query(id, &format!("SELECT note FROM {backup} WHERE id=1"), None))
                .unwrap()
                .result
                .rows[0][0]
                .is_null()
        );
        if driver == Driver::Postgres {
            manager.block_on(manager.query(id,&format!("DROP VIEW {table}; CREATE TABLE {table} (LIKE {backup} INCLUDING ALL); INSERT INTO {table} SELECT * FROM {backup}"),None)).unwrap();
            let blocked = manager
                .block_on(session.submit_result_edits(
                    &old,
                    &[ResultEditCell {
                        row: 0,
                        column: 2,
                        value: Value::Text("wrong new table".into()),
                    }],
                ))
                .unwrap_err();
            assert_eq!(blocked.index, 0);
            assert!(
                manager
                    .block_on(manager.query(
                        id,
                        &format!("SELECT note FROM {table} WHERE id=1"),
                        None
                    ))
                    .unwrap()
                    .result
                    .rows[0][0]
                    .is_null()
            );
        }
    }));
    let cleanup = if driver == Driver::Postgres {
        format!("DROP SCHEMA {quoted} CASCADE")
    } else {
        format!("DROP DATABASE {quoted}")
    };
    manager.block_on(manager.query(id, &cleanup, None)).unwrap();
    if let Err(error) = test {
        std::panic::resume_unwind(error);
    }
}
#[test]
fn postgres_result_editing() {
    run_fixture("KILN_TEST_PG_URL", Driver::Postgres);
}
#[test]
fn mysql_result_editing() {
    run_fixture("KILN_TEST_MYSQL_URL", Driver::MySql);
}
#[test]
fn mariadb_result_editing() {
    run_fixture("KILN_TEST_MARIA_URL", Driver::MariaDb);
}
