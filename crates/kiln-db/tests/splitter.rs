//! 문장 분리기·자동 LIMIT·값 파싱 단위 테스트.

use kiln_db::sql::{apply_auto_limit, split_statements, statement_at};
use kiln_db::{Driver, TypeClass, Value};

fn split(sql: &str, d: Driver) -> Vec<&str> {
    split_statements(sql, d)
        .into_iter()
        .map(|r| &sql[r])
        .collect()
}

#[test]
fn splits_on_semicolons_outside_quotes_and_comments() {
    let sql = "SELECT 'a;b' ; -- c;d\nSELECT \"x;y\" FROM t /* ; */;\n\n  ";
    assert_eq!(
        split(sql, Driver::Postgres),
        vec!["SELECT 'a;b'", "-- c;d\nSELECT \"x;y\" FROM t /* ; */"]
    );
}

#[test]
fn skips_comment_only_and_empty_statements() {
    assert_eq!(
        split(";;  -- only\n; SELECT 1", Driver::Sqlite),
        vec!["SELECT 1"]
    );
}

#[test]
fn pg_dollar_quoting_keeps_function_body_intact() {
    let sql = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;\
               DO $body$ BEGIN PERFORM 1; END $body$; SELECT $1";
    let parts = split(sql, Driver::Postgres);
    assert_eq!(parts.len(), 3, "{parts:?}");
    assert!(parts[0].ends_with("LANGUAGE plpgsql"));
    assert!(parts[1].starts_with("DO $body$"));
    assert_eq!(parts[2], "SELECT $1");
}

#[test]
fn pg_nested_block_comments_and_e_strings() {
    let sql = "SELECT /* a /* b; */ c; */ 1; SELECT E'it\\'s;'; SELECT 2";
    assert_eq!(split(sql, Driver::Postgres).len(), 3);
}

#[test]
fn mysql_backslash_escapes_hash_comments_and_backticks() {
    let sql = "SELECT 'a\\';b' FROM `t;x`; # c;\nSELECT 2";
    let parts = split(sql, Driver::MySql);
    assert_eq!(parts, vec!["SELECT 'a\\';b' FROM `t;x`", "# c;\nSELECT 2"]);
}

#[test]
fn trigger_bodies_with_begin_end_stay_whole() {
    let sql = "CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET a = 1; DELETE FROM u; END; SELECT 1;";
    let parts = split(sql, Driver::Sqlite);
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(parts[0].ends_with("END"));
}

#[test]
fn statement_at_cursor_picks_enclosing_statement() {
    let sql = "SELECT 1;\nSELECT 2;\n\nSELECT 3";
    let at = |c| statement_at(sql, Driver::Postgres, c).map(|r| &sql[r]);
    assert_eq!(at(3), Some("SELECT 1"));
    assert_eq!(at(12), Some("SELECT 2"));
    assert_eq!(at(sql.len()), Some("SELECT 3"));
    assert_eq!(at(20), Some("SELECT 2"));
}

#[test]
fn auto_limit_only_for_plain_selects_without_limit() {
    let d = Driver::Postgres;
    assert_eq!(
        apply_auto_limit("SELECT * FROM t;", d, 1000).as_deref(),
        Some("SELECT * FROM t LIMIT 1000")
    );
    assert_eq!(apply_auto_limit("SELECT * FROM t LIMIT 5", d, 1000), None);
    assert_eq!(
        apply_auto_limit("select * from (select 1 limit 1) x", d, 10).as_deref(),
        Some("select * from (select 1 limit 1) x LIMIT 10")
    );
    assert_eq!(apply_auto_limit("UPDATE t SET a=1", d, 10), None);
    assert_eq!(apply_auto_limit("SELECT * FROM t FOR UPDATE", d, 10), None);
    assert_eq!(
        apply_auto_limit("SELECT 1 -- hi", d, 10).as_deref(),
        Some("SELECT 1 -- hi\nLIMIT 10")
    );
}

#[test]
fn keyword_table_is_sorted_for_binary_search() {
    let k = kiln_db::sql::KEYWORDS;
    assert!(k.windows(2).all(|w| w[0] < w[1]));
    assert!(kiln_db::sql::is_keyword("select"));
    assert!(!kiln_db::sql::is_keyword("selector"));
}

#[test]
fn type_classes_and_typed_input_parsing() {
    use TypeClass as C;
    assert_eq!(C::from_type_name("character varying(20)"), C::Text);
    assert_eq!(C::from_type_name("INT4[]"), C::Array);
    assert_eq!(
        C::from_type_name("timestamp with time zone"),
        C::Timestamptz
    );
    assert_eq!(
        C::from_type_name("timestamp without time zone"),
        C::DateTime
    );
    assert_eq!(C::from_type_name("BIGINT UNSIGNED"), C::UInt);
    assert_eq!(C::from_type_name("numeric(10,2)"), C::Decimal);
    assert_eq!(C::from_type_name("JSONB"), C::Json);
    assert_eq!(C::from_type_name("varbinary(16)"), C::Bytes);
    assert_eq!(C::from_type_name("bit(3)"), C::Bit);
    assert_eq!(Value::parse_input("42", C::Int), Ok(Value::Int(42)));
    assert!(Value::parse_input("4x", C::Int).is_err());
    assert_eq!(Value::parse_input("yes", C::Bool), Ok(Value::Bool(true)));
    assert!(Value::parse_input("{bad", C::Json).is_err());
    assert_eq!(
        Value::parse_input("0xCAFE", C::Bytes),
        Ok(Value::Bytes(vec![0xca, 0xfe]))
    );
    assert!(Value::parse_input("2024-13-01", C::Date).is_err());
    assert!(Value::parse_input("12.50", C::Decimal).is_ok());
    assert!(Value::parse_input("not-a-uuid", C::Uuid).is_err());
    assert_eq!(Value::from_text("t", C::Bool), Value::Bool(true));
    assert_eq!(
        Value::from_text("\\x0102", C::Bytes),
        Value::Bytes(vec![1, 2])
    );
    assert_eq!(
        Value::from_text("abc", C::Int),
        Value::Decimal("abc".into())
    );
}

#[test]
fn url_import_parses_driver_host_user_password_and_ssl() {
    let (c, pw) = kiln_db::ConnConfig::from_url(
        "postgres://bob:p%40ss@db.example.com:6543/app?sslmode=require",
    )
    .unwrap();
    assert_eq!(c.driver, Driver::Postgres);
    assert_eq!(
        (
            c.host.as_str(),
            c.port,
            c.user.as_str(),
            c.database.as_str()
        ),
        ("db.example.com", 6543, "bob", "app")
    );
    assert_eq!(pw.as_deref(), Some("p@ss"));
    assert_eq!(c.ssl_mode, kiln_db::SslMode::Require);
    let (c, pw) = kiln_db::ConnConfig::from_url("mysql://root@localhost/shop").unwrap();
    assert_eq!((c.driver, c.port, pw), (Driver::MySql, 3306, None));
    let (c, _) = kiln_db::ConnConfig::from_url("sqlite:///tmp/a%20b.db").unwrap();
    assert_eq!((c.driver, c.file.as_str()), (Driver::Sqlite, "/tmp/a b.db"));
    assert!(kiln_db::ConnConfig::from_url("redis://x").is_err());
}
