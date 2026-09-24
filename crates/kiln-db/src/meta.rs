//! 스키마 탐색 모델과 드라이버별 카탈로그 조회.

use crate::Driver;
use crate::driver::{DbPool, DbResult, ResultSet, value_string};
use crate::sql::{qualified, quote_ident, quote_literal};
use crate::value::TypeClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
    pub row_estimate: Option<i64>,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    /// 표시용 타입(길이 등 포함).
    pub data_type: String,
    /// Postgres 바인딩 캐스트에 쓰는 타입(길이 제외).
    pub cast_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    /// PK 안 순서(1부터). PK 가 아니면 0.
    pub pk_ordinal: u32,
    pub auto_increment: bool,
    pub class: TypeClass,
}

impl ColumnDef {
    pub fn is_pk(&self) -> bool {
        self.pk_ordinal > 0
    }

    /// 새 행에서 값을 비워도 DB 가 채우는지.
    pub fn has_default(&self) -> bool {
        self.default.is_some() || self.auto_increment
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    pub on_update: String,
    pub on_delete: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableDetails {
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

impl TableDetails {
    /// PK 컬럼 인덱스(PK 순서대로).
    pub fn pk_columns(&self) -> Vec<usize> {
        let mut v: Vec<(u32, usize)> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_pk())
            .map(|(i, c)| (c.pk_ordinal, i))
            .collect();
        v.sort();
        v.into_iter().map(|(_, i)| i).collect()
    }

    /// 컬럼이 FK 에 속하면 참조 대상 `table.column`.
    pub fn fk_target(&self, column: &str) -> Option<String> {
        self.foreign_keys.iter().find_map(|fk| {
            fk.columns.iter().position(|c| c == column).map(|i| {
                format!(
                    "{}.{}",
                    fk.ref_table,
                    fk.ref_columns.get(i).cloned().unwrap_or_default()
                )
            })
        })
    }
}

const SEP: char = '\u{1f}';

fn s(rs: &ResultSet, r: usize, c: usize) -> String {
    rs.rows
        .get(r)
        .and_then(|row| row.get(c))
        .and_then(value_string)
        .unwrap_or_default()
}

fn opt(rs: &ResultSet, r: usize, c: usize) -> Option<String> {
    rs.rows
        .get(r)
        .and_then(|row| row.get(c))
        .and_then(value_string)
}

fn truthy(v: &str) -> bool {
    matches!(v, "t" | "true" | "1" | "YES" | "yes" | "TRUE")
}

fn split_sep(v: &str) -> Vec<String> {
    if v.is_empty() {
        Vec::new()
    } else {
        v.split(SEP).map(str::to_string).collect()
    }
}

/// 스키마(pg), 데이터베이스(mysql), 첨부 DB(sqlite) 목록.
pub(crate) async fn list_schemas(pool: &DbPool, driver: Driver) -> DbResult<Vec<String>> {
    let sql = match driver {
        Driver::Postgres => {
            "SELECT nspname::text FROM pg_namespace \
             WHERE nspname NOT LIKE 'pg\\_%' ESCAPE '\\' AND nspname <> 'information_schema' \
             ORDER BY nspname = 'public' DESC, nspname"
        }
        Driver::MySql | Driver::MariaDb => {
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('information_schema','performance_schema','mysql','sys') \
             ORDER BY schema_name = DATABASE() DESC, schema_name"
        }
        Driver::Sqlite => "SELECT name FROM pragma_database_list WHERE name <> 'temp' ORDER BY seq",
    };
    let rs = pool.query(sql).await?;
    Ok((0..rs.len()).map(|r| s(&rs, r, 0)).collect())
}

/// 스키마 안 테이블/뷰 목록.
pub(crate) async fn list_tables(
    pool: &DbPool,
    driver: Driver,
    schema: &str,
) -> DbResult<Vec<TableInfo>> {
    let lit = quote_literal(driver, schema);
    let sql = match driver {
        Driver::Postgres => format!(
            "SELECT c.relname::text, c.relkind::text, GREATEST(c.reltuples, 0)::bigint, \
             COALESCE(obj_description(c.oid, 'pg_class'), '') \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {lit} AND c.relkind IN ('r','p','v','m','f') ORDER BY c.relname"
        ),
        Driver::MySql | Driver::MariaDb => format!(
            "SELECT table_name, table_type, table_rows, COALESCE(table_comment, '') \
             FROM information_schema.tables WHERE table_schema = {lit} ORDER BY table_name"
        ),
        Driver::Sqlite => format!(
            "SELECT name, type, NULL, '' FROM {}.sqlite_master \
             WHERE type IN ('table','view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
            quote_ident(driver, schema)
        ),
    };
    let rs = pool.query(&sql).await?;
    Ok((0..rs.len())
        .map(|r| {
            let kind_s = s(&rs, r, 1).to_ascii_lowercase();
            let kind = match kind_s.as_str() {
                "v" | "view" | "system view" => TableKind::View,
                "m" => TableKind::MaterializedView,
                _ => TableKind::Table,
            };
            TableInfo {
                schema: Some(schema.to_string()),
                name: s(&rs, r, 0),
                kind,
                row_estimate: opt(&rs, r, 2).and_then(|v| v.parse().ok()),
                comment: s(&rs, r, 3),
            }
        })
        .collect())
}

/// 컬럼·인덱스·FK 를 모두 조회한다.
pub(crate) async fn table_details(
    pool: &DbPool,
    driver: Driver,
    schema: Option<&str>,
    table: &str,
) -> DbResult<TableDetails> {
    match driver {
        Driver::Postgres => pg_details(pool, schema.unwrap_or("public"), table).await,
        Driver::MySql | Driver::MariaDb => my_details(pool, schema, table).await,
        Driver::Sqlite => lite_details(pool, schema.unwrap_or("main"), table).await,
    }
}

async fn pg_details(pool: &DbPool, schema: &str, table: &str) -> DbResult<TableDetails> {
    let d = Driver::Postgres;
    let ns = quote_literal(d, schema);
    let tb = quote_literal(d, table);
    let cols = pool
        .query(&format!(
            "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
             pg_get_expr(ad.adbin, ad.adrelid), \
             COALESCE((SELECT array_position(con.conkey, a.attnum) FROM pg_constraint con \
                       WHERE con.conrelid = c.oid AND con.contype = 'p'), 0), \
             format_type(a.atttypid, NULL), a.attidentity::text, a.attgenerated::text \
             FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
             WHERE n.nspname = {ns} AND c.relname = {tb} AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum"
        ))
        .await?;
    let columns = (0..cols.len())
        .map(|r| {
            let data_type = s(&cols, r, 1);
            let default = opt(&cols, r, 3);
            let identity = s(&cols, r, 6);
            let generated = s(&cols, r, 7);
            ColumnDef {
                name: s(&cols, r, 0),
                class: TypeClass::from_type_name(&data_type),
                cast_type: pg_cast_type(&s(&cols, r, 5)),
                data_type,
                nullable: truthy(&s(&cols, r, 2)),
                auto_increment: !identity.is_empty()
                    || !generated.is_empty()
                    || default
                        .as_deref()
                        .is_some_and(|d| d.starts_with("nextval(")),
                default,
                pk_ordinal: s(&cols, r, 4).parse().unwrap_or(0),
            }
        })
        .collect();
    let idx = pool
        .query(&format!(
            "SELECT i.relname::text, ix.indisunique::text, ix.indisprimary::text, \
             (SELECT string_agg(pg_get_indexdef(ix.indexrelid, k, true), chr(31) ORDER BY k) \
              FROM generate_series(1, ix.indnkeyatts) k) \
             FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class c ON c.oid = ix.indrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {ns} AND c.relname = {tb} ORDER BY ix.indisprimary DESC, i.relname"
        ))
        .await?;
    let indexes = (0..idx.len())
        .map(|r| IndexInfo {
            name: s(&idx, r, 0),
            unique: truthy(&s(&idx, r, 1)),
            primary: truthy(&s(&idx, r, 2)),
            columns: split_sep(&s(&idx, r, 3)),
        })
        .collect();
    let fks = pool
        .query(&format!(
            "SELECT con.conname::text, \
             (SELECT string_agg(a.attname, chr(31) ORDER BY k.i) FROM unnest(con.conkey) WITH ORDINALITY k(n, i) \
              JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.n), \
             fn.nspname::text, fc.relname::text, \
             (SELECT string_agg(a.attname, chr(31) ORDER BY k.i) FROM unnest(con.confkey) WITH ORDINALITY k(n, i) \
              JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.n), \
             con.confupdtype::text, con.confdeltype::text \
             FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_class fc ON fc.oid = con.confrelid JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
             WHERE con.contype = 'f' AND n.nspname = {ns} AND c.relname = {tb} ORDER BY con.conname"
        ))
        .await?;
    let action = |c: &str| {
        match c {
            "a" => "NO ACTION",
            "r" => "RESTRICT",
            "c" => "CASCADE",
            "n" => "SET NULL",
            "d" => "SET DEFAULT",
            _ => "",
        }
        .to_string()
    };
    let foreign_keys = (0..fks.len())
        .map(|r| ForeignKeyInfo {
            name: s(&fks, r, 0),
            columns: split_sep(&s(&fks, r, 1)),
            ref_schema: opt(&fks, r, 2),
            ref_table: s(&fks, r, 3),
            ref_columns: split_sep(&s(&fks, r, 4)),
            on_update: action(&s(&fks, r, 5)),
            on_delete: action(&s(&fks, r, 6)),
        })
        .collect();
    Ok(TableDetails {
        columns,
        indexes,
        foreign_keys,
    })
}

/// 길이 제한이 없는 캐스트 타입. 길이 검사는 대입 시점에 컬럼 타입이 한다.
fn pg_cast_type(base: &str) -> String {
    match base {
        "character" => "bpchar".into(),
        "character[]" => "bpchar[]".into(),
        "bit" => "varbit".into(),
        "bit[]" => "varbit[]".into(),
        other => other.to_string(),
    }
}

async fn my_details(pool: &DbPool, schema: Option<&str>, table: &str) -> DbResult<TableDetails> {
    let d = Driver::MySql;
    let ns = schema
        .map(|s| quote_literal(d, s))
        .unwrap_or_else(|| "DATABASE()".into());
    let tb = quote_literal(d, table);
    let cols = pool
        .query(&format!(
            "SELECT column_name, column_type, is_nullable, column_default, column_key, extra, data_type \
             FROM information_schema.columns WHERE table_schema = {ns} AND table_name = {tb} \
             ORDER BY ordinal_position"
        ))
        .await?;
    let stats = pool
        .query(&format!(
            "SELECT index_name, non_unique, column_name FROM information_schema.statistics \
             WHERE table_schema = {ns} AND table_name = {tb} \
             ORDER BY index_name = 'PRIMARY' DESC, index_name, seq_in_index"
        ))
        .await?;
    let mut indexes: Vec<IndexInfo> = Vec::new();
    for r in 0..stats.len() {
        let name = s(&stats, r, 0);
        let col = s(&stats, r, 2);
        match indexes.iter_mut().find(|i| i.name == name) {
            Some(i) => i.columns.push(col),
            None => indexes.push(IndexInfo {
                primary: name == "PRIMARY",
                unique: s(&stats, r, 1) == "0",
                name,
                columns: vec![col],
            }),
        }
    }
    let pk: Vec<String> = indexes
        .iter()
        .find(|i| i.primary)
        .map(|i| i.columns.clone())
        .unwrap_or_default();
    let columns = (0..cols.len())
        .map(|r| {
            let name = s(&cols, r, 0);
            let data_type = s(&cols, r, 1);
            let extra = s(&cols, r, 5).to_ascii_lowercase();
            let base = s(&cols, r, 6).to_ascii_lowercase();
            let class = if data_type.eq_ignore_ascii_case("tinyint(1)") {
                TypeClass::Bool
            } else {
                TypeClass::from_type_name(&data_type)
            };
            let default = opt(&cols, r, 3);
            ColumnDef {
                pk_ordinal: pk
                    .iter()
                    .position(|p| *p == name)
                    .map(|p| p as u32 + 1)
                    .unwrap_or(0),
                name,
                class,
                cast_type: base,
                data_type,
                nullable: s(&cols, r, 2) == "YES",
                auto_increment: extra.contains("auto_increment") || extra.contains("generated"),
                default: default.filter(|d| !d.eq_ignore_ascii_case("null")),
            }
        })
        .collect();
    let fks = pool
        .query(&format!(
            "SELECT k.constraint_name, k.column_name, k.referenced_table_schema, k.referenced_table_name, \
             k.referenced_column_name, r.update_rule, r.delete_rule \
             FROM information_schema.key_column_usage k \
             JOIN information_schema.referential_constraints r \
               ON r.constraint_schema = k.constraint_schema AND r.constraint_name = k.constraint_name \
              AND r.table_name = k.table_name \
             WHERE k.table_schema = {ns} AND k.table_name = {tb} AND k.referenced_table_name IS NOT NULL \
             ORDER BY k.constraint_name, k.ordinal_position"
        ))
        .await?;
    let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
    for r in 0..fks.len() {
        let name = s(&fks, r, 0);
        match foreign_keys.iter_mut().find(|f| f.name == name) {
            Some(f) => {
                f.columns.push(s(&fks, r, 1));
                f.ref_columns.push(s(&fks, r, 4));
            }
            None => foreign_keys.push(ForeignKeyInfo {
                name,
                columns: vec![s(&fks, r, 1)],
                ref_schema: opt(&fks, r, 2),
                ref_table: s(&fks, r, 3),
                ref_columns: vec![s(&fks, r, 4)],
                on_update: s(&fks, r, 5),
                on_delete: s(&fks, r, 6),
            }),
        }
    }
    Ok(TableDetails {
        columns,
        indexes,
        foreign_keys,
    })
}

async fn lite_details(pool: &DbPool, schema: &str, table: &str) -> DbResult<TableDetails> {
    let d = Driver::Sqlite;
    let ns = quote_literal(d, schema);
    let tb = quote_literal(d, table);
    let cols = pool
        .query(&format!(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info({tb}, {ns}) ORDER BY cid"
        ))
        .await?;
    let n_pk = (0..cols.len()).filter(|r| s(&cols, *r, 4) != "0").count();
    let columns = (0..cols.len())
        .map(|r| {
            let data_type = s(&cols, r, 1);
            let pk_ordinal: u32 = s(&cols, r, 4).parse().unwrap_or(0);
            // INTEGER PRIMARY KEY 단일 컬럼은 rowid 별칭이라 자동 증가한다.
            let rowid_alias =
                pk_ordinal > 0 && n_pk == 1 && data_type.eq_ignore_ascii_case("integer");
            ColumnDef {
                name: s(&cols, r, 0),
                class: TypeClass::from_type_name(&data_type),
                cast_type: data_type.clone(),
                data_type,
                nullable: s(&cols, r, 2) == "0",
                default: opt(&cols, r, 3),
                pk_ordinal,
                auto_increment: rowid_alias,
            }
        })
        .collect();
    let idx = pool
        .query(&format!(
            "SELECT il.name, il.\"unique\", il.origin, ii.name \
             FROM pragma_index_list({tb}, {ns}) il, pragma_index_info(il.name, {ns}) ii \
             ORDER BY il.origin = 'pk' DESC, il.name, ii.seqno"
        ))
        .await?;
    let mut indexes: Vec<IndexInfo> = Vec::new();
    for r in 0..idx.len() {
        let name = s(&idx, r, 0);
        let col = s(&idx, r, 3);
        match indexes.iter_mut().find(|i| i.name == name) {
            Some(i) => i.columns.push(col),
            None => indexes.push(IndexInfo {
                unique: s(&idx, r, 1) == "1",
                primary: s(&idx, r, 2) == "pk",
                name,
                columns: vec![col],
            }),
        }
    }
    let fks = pool
        .query(&format!(
            "SELECT id, \"table\", \"from\", \"to\", on_update, on_delete \
             FROM pragma_foreign_key_list({tb}, {ns}) ORDER BY id, seq"
        ))
        .await?;
    let mut foreign_keys: Vec<ForeignKeyInfo> = Vec::new();
    for r in 0..fks.len() {
        let name = format!("fk_{}_{}", table, s(&fks, r, 0));
        match foreign_keys.iter_mut().find(|f| f.name == name) {
            Some(f) => {
                f.columns.push(s(&fks, r, 2));
                f.ref_columns.push(s(&fks, r, 3));
            }
            None => foreign_keys.push(ForeignKeyInfo {
                name,
                columns: vec![s(&fks, r, 2)],
                ref_schema: None,
                ref_table: s(&fks, r, 1),
                ref_columns: vec![s(&fks, r, 3)],
                on_update: s(&fks, r, 4),
                on_delete: s(&fks, r, 5),
            }),
        }
    }
    Ok(TableDetails {
        columns,
        indexes,
        foreign_keys,
    })
}

/// 테이블/뷰 DDL.
pub(crate) async fn table_ddl(
    pool: &DbPool,
    driver: Driver,
    schema: Option<&str>,
    table: &str,
) -> DbResult<String> {
    match driver {
        Driver::Postgres => pg_ddl(pool, schema.unwrap_or("public"), table).await,
        Driver::MySql | Driver::MariaDb => {
            let rs = pool
                .query(&format!(
                    "SHOW CREATE TABLE {}",
                    qualified(driver, schema, table)
                ))
                .await?;
            Ok(format!("{};", s(&rs, 0, 1)))
        }
        Driver::Sqlite => {
            let sch = schema.unwrap_or("main");
            let rs = pool
                .query(&format!(
                    "SELECT sql FROM {}.sqlite_master WHERE tbl_name = {} AND sql IS NOT NULL \
                     ORDER BY type IN ('table','view') DESC, name",
                    quote_ident(driver, sch),
                    quote_literal(driver, table)
                ))
                .await?;
            Ok((0..rs.len())
                .map(|r| format!("{};", s(&rs, r, 0)))
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
    }
}

async fn pg_ddl(pool: &DbPool, schema: &str, table: &str) -> DbResult<String> {
    let d = Driver::Postgres;
    let qname = qualified(d, Some(schema), table);
    let reg = quote_literal(d, &qname);
    let kind = pool
        .query(&format!(
            "SELECT relkind::text FROM pg_class WHERE oid = {reg}::regclass"
        ))
        .await?;
    let kind = s(&kind, 0, 0);
    if kind == "v" || kind == "m" {
        let def = pool
            .query(&format!("SELECT pg_get_viewdef({reg}::regclass, true)"))
            .await?;
        let head = if kind == "m" {
            "CREATE MATERIALIZED VIEW"
        } else {
            "CREATE OR REPLACE VIEW"
        };
        return Ok(format!("{head} {qname} AS\n{}", s(&def, 0, 0).trim_end()));
    }
    let details = pg_details(pool, schema, table).await?;
    let mut lines: Vec<String> = details
        .columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", quote_ident(d, &c.name), c.data_type);
            if let Some(def) = &c.default {
                l.push_str(&format!(" DEFAULT {def}"));
            }
            if !c.nullable {
                l.push_str(" NOT NULL");
            }
            l
        })
        .collect();
    let cons = pool
        .query(&format!(
            "SELECT conname::text, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE conrelid = {reg}::regclass \
             ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, conname"
        ))
        .await?;
    for r in 0..cons.len() {
        lines.push(format!(
            "    CONSTRAINT {} {}",
            quote_ident(d, &s(&cons, r, 0)),
            s(&cons, r, 1)
        ));
    }
    let mut out = format!("CREATE TABLE {qname} (\n{}\n);", lines.join(",\n"));
    let idx = pool
        .query(&format!(
            "SELECT pg_get_indexdef(i.indexrelid) FROM pg_index i \
             WHERE i.indrelid = {reg}::regclass \
             AND NOT EXISTS (SELECT 1 FROM pg_constraint c WHERE c.conindid = i.indexrelid) \
             ORDER BY 1"
        ))
        .await?;
    for r in 0..idx.len() {
        out.push_str(&format!("\n\n{};", s(&idx, r, 0)));
    }
    let comment = pool
        .query(&format!(
            "SELECT obj_description({reg}::regclass, 'pg_class')"
        ))
        .await?;
    if let Some(c) = opt(&comment, 0, 0) {
        out.push_str(&format!(
            "\n\nCOMMENT ON TABLE {qname} IS {};",
            quote_literal(d, &c)
        ));
    }
    Ok(out)
}
