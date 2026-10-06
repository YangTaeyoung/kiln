//! Reviewed, dialect-aware schema changes. Preparing a plan never executes DDL.

use crate::driver::{DbError, DbPool, DbResult};
use crate::sql::{TokKind, Token, qualified, quote_ident, quote_literal, tokenize};
use crate::{ColumnDef, ConnId, DbManager, Driver, TableDetails, TableKind, TableRef};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, Row};
use std::sync::atomic::{AtomicU64, Ordering};
static TEMP_ID: AtomicU64 = AtomicU64::new(1);
// Raw catalogs avoid pg_get_expr/indexdef taking a relation lock on a second
// connection while the DDL transaction owns ACCESS EXCLUSIVE.
fn pg_catalog_stamp(table: &TableRef) -> String {
    let ns = quote_literal(
        Driver::Postgres,
        table.schema.as_deref().unwrap_or("public"),
    );
    let name = quote_literal(Driver::Postgres, &table.table);
    format!(
        "SELECT jsonb_build_object('table',jsonb_build_array(c.oid,c.relkind,c.reloptions,c.relpartbound::text), \
        'columns',(SELECT jsonb_agg(to_jsonb(a) ORDER BY a.attnum) FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0), \
        'defaults',(SELECT jsonb_agg(to_jsonb(d) ORDER BY d.adnum) FROM pg_attrdef d WHERE d.adrelid=c.oid), \
        'constraints',(SELECT jsonb_agg(to_jsonb(k) ORDER BY k.oid) FROM pg_constraint k WHERE k.conrelid=c.oid), \
        'indexes',(SELECT jsonb_agg(jsonb_build_array(to_jsonb(i),x.relname,x.relam,x.reloptions) ORDER BY i.indexrelid) FROM pg_index i JOIN pg_class x ON x.oid=i.indexrelid WHERE i.indrelid=c.oid), \
        'triggers',(SELECT jsonb_agg(to_jsonb(t) ORDER BY t.oid) FROM pg_trigger t WHERE t.tgrelid=c.oid), \
        'rules',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.oid) FROM pg_rewrite r WHERE r.ev_class=c.oid))::text \
        FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={ns} AND c.relname={name}"
    )
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
}

impl From<&ColumnDef> for ColumnSpec {
    fn from(c: &ColumnDef) -> Self {
        Self {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            nullable: c.nullable,
            default: c.default.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexColumn {
    pub name: String,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndexSpec {
    pub name: String,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchemaAction {
    RenameTable { name: String },
    DropTable,
    AddColumn(ColumnSpec),
    AlterColumn { column: String, spec: ColumnSpec },
    RenameColumn { column: String, name: String },
    DropColumn { column: String },
    AddIndex(IndexSpec),
    DropIndex { name: String },
}

#[derive(Clone, Debug)]
pub struct SchemaPlan {
    pub driver: Driver,
    pub table: TableRef,
    pub action: SchemaAction,
    pub sql: Vec<String>,
    pub warning: Option<String>,
    pub epoch: u64,
    snapshot: TableDetails,
    definition: String,
    rebuild: bool,
    catalog_version: Option<i64>,
    reviewed: Option<Reviewed>,
    pg_stamp: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Reviewed {
    driver: Driver,
    table: TableRef,
    action: SchemaAction,
    sql: Vec<String>,
    epoch: u64,
}
impl SchemaPlan {
    fn review_key(&self) -> Reviewed {
        Reviewed {
            driver: self.driver,
            table: self.table.clone(),
            action: self.action.clone(),
            sql: self.sql.clone(),
            epoch: self.epoch,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SchemaResult {
    pub table: Option<TableRef>,
}

fn invalid(message: &'static str) -> DbError {
    DbError::msg(kiln_common::i18n::tr(message))
}

fn check_name(name: &str) -> DbResult<()> {
    if name.trim().is_empty() || name.contains('\0') || name.chars().any(char::is_control) {
        return Err(invalid(
            "이름을 입력하세요. 제어 문자는 사용할 수 없습니다.",
        ));
    }
    Ok(())
}

/// Expressions are SQL, but cannot break out into a second statement/constraint.
fn check_fragment(value: &str, driver: Driver, is_type: bool) -> DbResult<()> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid("타입 또는 기본값 표현식을 확인하세요."));
    }
    let tokens = tokenize(value, driver);
    let mut depth = 0i32;
    for t in &tokens {
        let s = &value[t.start..t.end];
        if matches!(
            t.kind,
            TokKind::Semicolon | TokKind::Comment | TokKind::Param
        ) {
            return Err(invalid(
                "타입 또는 기본값에는 단일 SQL 표현식만 입력하세요.",
            ));
        }
        if t.kind == TokKind::Punct {
            if s == "(" {
                depth += 1;
            }
            if s == ")" {
                depth -= 1;
            }
            if depth < 0 || (s == "," && depth == 0) {
                return Err(invalid("타입 또는 기본값 표현식을 확인하세요."));
            }
        }
        if t.kind == TokKind::Word {
            let upper = s.to_ascii_uppercase();
            if [
                "SELECT",
                "INSERT",
                "UPDATE",
                "DELETE",
                "DROP",
                "ALTER",
                "CREATE",
                "PRAGMA",
                "ATTACH",
                "DETACH",
                "VACUUM",
                "TRUNCATE",
                "REFERENCES",
                "CONSTRAINT",
                "CHECK",
                "GENERATED",
                "COLLATE",
            ]
            .contains(&upper.as_str())
                || (is_type
                    && ["DEFAULT", "NOT", "NULL", "PRIMARY", "UNIQUE", "AS"]
                        .contains(&upper.as_str()))
                || (!is_type
                    && depth == 0
                    && [
                        "DEFAULT",
                        "NOT",
                        "PRIMARY",
                        "UNIQUE",
                        "FOREIGN",
                        "ON",
                        "DEFERRABLE",
                    ]
                    .contains(&upper.as_str()))
            {
                return Err(invalid(
                    "타입 또는 기본값에는 단일 SQL 표현식만 입력하세요.",
                ));
            }
        }
    }
    if depth != 0 {
        return Err(invalid("타입 또는 기본값 표현식을 확인하세요."));
    }
    Ok(())
}

fn check_column(spec: &ColumnSpec, driver: Driver) -> DbResult<()> {
    check_name(&spec.name)?;
    check_fragment(&spec.data_type, driver, true)?;
    if let Some(v) = &spec.default {
        check_fragment(v, driver, false)?;
    }
    Ok(())
}

fn new_column(spec: &ColumnSpec, driver: Driver) -> DbResult<String> {
    check_column(spec, driver)?;
    Ok(format!(
        "{} {}{}{}",
        quote_ident(driver, &spec.name),
        spec.data_type.trim(),
        if spec.nullable { "" } else { " NOT NULL" },
        spec.default
            .as_ref()
            .map(|v| format!(" DEFAULT {}", v.trim()))
            .unwrap_or_default()
    ))
}

fn column<'a>(details: &'a TableDetails, name: &str) -> DbResult<&'a ColumnDef> {
    details
        .columns
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| invalid("컬럼을 찾을 수 없습니다. 구조를 새로고침하세요."))
}

fn ddl_tokens(sql: &str, driver: Driver) -> Vec<Token> {
    let raw = tokenize(sql, driver);
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = raw[i];
        if driver == Driver::Sqlite && &sql[t.start..t.end] == "[" {
            if let Some(end) = (i + 1..raw.len()).find(|&j| &sql[raw[j].start..raw[j].end] == "]") {
                out.push(Token {
                    kind: TokKind::QuotedIdent,
                    start: t.start,
                    end: raw[end].end,
                });
                i = end + 1;
                continue;
            }
        }
        if !matches!(t.kind, TokKind::Space | TokKind::Comment) {
            out.push(t);
        }
        i += 1;
    }
    out
}

fn ident(text: &str) -> String {
    let first = text.chars().next().unwrap_or('\0');
    if matches!(first, '"' | '`' | '\'' | '[') {
        let end = if first == '[' { ']' } else { first };
        return text
            .strip_prefix(first)
            .and_then(|s| s.strip_suffix(end))
            .unwrap_or(text)
            .replace(&format!("{end}{end}"), &end.to_string());
    }
    text.to_owned()
}

/// Top-level column/constraint clauses and table suffix, using lexical boundaries.
fn table_clauses(sql: &str, driver: Driver) -> DbResult<(Vec<String>, String)> {
    let ts = ddl_tokens(sql, driver);
    let first = ts
        .iter()
        .position(|t| &sql[t.start..t.end] == "(")
        .ok_or_else(|| invalid("이 테이블 정의를 안전하게 편집할 수 없습니다."))?;
    let mut depth = 1;
    let mut start = ts[first].end;
    let mut parts = Vec::new();
    for t in &ts[first + 1..] {
        let text = &sql[t.start..t.end];
        if t.kind == TokKind::Punct {
            if text == "(" {
                depth += 1;
            }
            if text == ")" {
                depth -= 1;
            }
            if depth == 0 {
                parts.push(sql[start..t.start].trim().to_owned());
                return Ok((
                    parts,
                    sql[t.end..].trim().trim_end_matches(';').trim().to_owned(),
                ));
            }
            if text == "," && depth == 1 {
                parts.push(sql[start..t.start].trim().to_owned());
                start = t.end;
            }
        }
    }
    Err(invalid("이 테이블 정의를 안전하게 편집할 수 없습니다."))
}

fn clause_column(clause: &str, driver: Driver) -> Option<String> {
    let ts = ddl_tokens(clause, driver);
    let t = ts.first()?;
    let text = &clause[t.start..t.end];
    if t.kind == TokKind::Word
        && ["PRIMARY", "UNIQUE", "CHECK", "CONSTRAINT", "FOREIGN"]
            .iter()
            .any(|k| text.eq_ignore_ascii_case(k))
    {
        return None;
    }
    Some(ident(text))
}

/// Preserve attributes the form does not edit: CHECK/FK/UNIQUE, collation,
/// comments, generation, ON UPDATE and named constraints.
fn rewrite_column(
    clause: &str,
    driver: Driver,
    old: &ColumnDef,
    new: &ColumnSpec,
) -> DbResult<String> {
    let ts = ddl_tokens(clause, driver);
    let name = ts
        .first()
        .ok_or_else(|| invalid("이 테이블 정의를 안전하게 편집할 수 없습니다."))?;
    let mut depth = 0;
    let mut attrs = Vec::<(usize, String)>::new();
    let mut named = None;
    for (i, t) in ts.iter().enumerate().skip(1) {
        let text = &clause[t.start..t.end];
        if t.kind == TokKind::Punct {
            if text == "(" {
                depth += 1;
            }
            if text == ")" {
                depth -= 1;
            }
            continue;
        }
        if depth != 0 || t.kind != TokKind::Word {
            continue;
        }
        let key = text.to_ascii_uppercase();
        let prev = ts
            .get(i.wrapping_sub(1))
            .map(|t| clause[t.start..t.end].to_ascii_uppercase())
            .unwrap_or_default();
        let next = ts
            .get(i + 1)
            .map(|t| clause[t.start..t.end].to_ascii_uppercase())
            .unwrap_or_default();
        // FK action words belong to REFERENCES; never strip SET NULL/DEFAULT
        // while editing this column's nullability/default.
        if (key == "ON" && attrs.last().is_some_and(|(_, k)| k == "REFERENCES"))
            || (["NULL", "DEFAULT"].contains(&key.as_str()) && prev == "SET")
            || (key == "NOT" && next != "NULL")
        {
            continue;
        }
        if key == "CHARACTER" && next != "SET" {
            continue;
        }
        if key == "CONSTRAINT" {
            named = Some(t.start);
            continue;
        }
        if i > 1 && clause[ts[i - 1].start..ts[i - 1].end].eq_ignore_ascii_case("CONSTRAINT") {
            continue;
        }
        if [
            "NOT",
            "NULL",
            "DEFAULT",
            "PRIMARY",
            "UNIQUE",
            "CHECK",
            "REFERENCES",
            "COLLATE",
            "GENERATED",
            "AS",
            "AUTO_INCREMENT",
            "COMMENT",
            "ON",
            "VISIBLE",
            "INVISIBLE",
            "COLUMN_FORMAT",
            "STORAGE",
            "CHARACTER",
        ]
        .contains(&key.as_str())
        {
            // NULL belongs to NOT NULL or DEFAULT NULL, and AS to GENERATED AS.
            if key == "NULL"
                && attrs
                    .last()
                    .is_some_and(|(_, k)| k == "NOT" || k == "DEFAULT")
            {
                continue;
            }
            if key == "AS" && attrs.last().is_some_and(|(_, k)| k == "GENERATED") {
                continue;
            }
            attrs.push((named.take().unwrap_or(t.start), key));
        }
    }
    let type_end = attrs.first().map(|a| a.0).unwrap_or(clause.len());
    let ty = if old.data_type.trim() == new.data_type.trim() {
        clause[name.end..type_end].trim()
    } else {
        new.data_type.trim()
    };
    let mut suffix = String::new();
    for (i, (start, key)) in attrs.iter().enumerate() {
        let end = attrs.get(i + 1).map(|a| a.0).unwrap_or(clause.len());
        if ((key == "NOT" || key == "NULL") && old.nullable != new.nullable)
            || (key == "DEFAULT" && old.default != new.default)
        {
            continue;
        }
        suffix.push(' ');
        suffix.push_str(clause[*start..end].trim());
    }
    if old.nullable != new.nullable && !new.nullable {
        suffix.push_str(" NOT NULL");
    }
    if old.default != new.default
        && let Some(value) = &new.default
    {
        suffix.push_str(" DEFAULT ");
        suffix.push_str(value.trim());
    }
    Ok(format!("{} {ty}{suffix}", quote_ident(driver, &old.name)))
}

async fn prepare(
    m: &DbManager,
    id: ConnId,
    t: &TableRef,
    action: SchemaAction,
) -> DbResult<SchemaPlan> {
    let driver = m
        .driver(id)
        .ok_or_else(|| invalid("연결을 찾을 수 없습니다"))?;
    let epoch = m.connection_epoch(id);
    let pool = m
        .connected_pool(id)
        .ok_or_else(|| invalid("먼저 데이터베이스에 연결하세요."))?;
    let schema = t.schema.as_deref().unwrap_or(match driver {
        Driver::Postgres => "public",
        Driver::Sqlite => "main",
        _ => "",
    });
    if driver == Driver::Sqlite && schema != "main" {
        return Err(invalid(
            "SQLite의 세션 전용 연결 스키마는 이 화면에서 변경할 수 없습니다.",
        ));
    }
    let catalog_version = if driver == Driver::Sqlite {
        pool.query("PRAGMA main.schema_version")
            .await?
            .scalar_string()
            .and_then(|s| s.parse().ok())
    } else {
        None
    };
    let kinds = crate::meta::list_tables(&pool, driver, schema).await?;
    let kind = kinds.iter().find(|x| x.name == t.table).map(|x| x.kind);
    if kind != Some(TableKind::Table) {
        return Err(invalid("일반 테이블만 구조를 변경할 수 있습니다."));
    }
    let pg_stamp = if driver == Driver::Postgres {
        pool.query(&pg_catalog_stamp(t)).await?.scalar_string()
    } else {
        None
    };
    let snapshot = crate::meta::table_details(&pool, driver, t.schema.as_deref(), &t.table).await?;
    let definition = crate::meta::table_ddl(&pool, driver, t.schema.as_deref(), &t.table).await?;
    let mut plan = SchemaPlan {
        driver,
        table: t.clone(),
        action,
        sql: Vec::new(),
        warning: None,
        epoch,
        snapshot,
        definition,
        rebuild: false,
        catalog_version,
        reviewed: None,
        pg_stamp,
    };
    let table = t.sql_name(driver);
    match &plan.action {
        SchemaAction::RenameTable { name } => {
            check_name(name)?;
            if name == &t.table {
                return Err(invalid("현재 이름과 다른 이름을 입력하세요."));
            }
            plan.sql.push(format!(
                "ALTER TABLE {table} RENAME TO {}",
                quote_ident(driver, name)
            ));
        }
        SchemaAction::DropTable => {
            plan.sql.push(format!("DROP TABLE {table}"));
            plan.warning = Some(
                kiln_common::i18n::tr(
                    "테이블과 모든 데이터가 삭제됩니다. 이 작업은 되돌릴 수 없습니다.",
                )
                .into(),
            );
        }
        SchemaAction::AddColumn(spec) => {
            if plan.snapshot.columns.iter().any(|c| c.name == spec.name) {
                return Err(invalid("이미 존재하는 컬럼 이름입니다."));
            }
            plan.sql.push(format!(
                "ALTER TABLE {table} ADD COLUMN {}",
                new_column(spec, driver)?
            ));
        }
        SchemaAction::RenameColumn { column: old, name } => {
            column(&plan.snapshot, old)?;
            check_name(name)?;
            if plan.snapshot.columns.iter().any(|c| c.name == *name) {
                return Err(invalid("이미 존재하는 컬럼 이름입니다."));
            }
            plan.sql.push(format!(
                "ALTER TABLE {table} RENAME COLUMN {} TO {}",
                quote_ident(driver, old),
                quote_ident(driver, name)
            ));
        }
        SchemaAction::DropColumn { column: name } => {
            let c = column(&plan.snapshot, name)?;
            if c.is_pk() {
                return Err(invalid("기본 키 컬럼은 이 화면에서 삭제할 수 없습니다."));
            }
            let indexed = plan.snapshot.indexes.iter().any(|ix| {
                let sources = ix
                    .columns
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(ix.definition.as_str()));
                sources.into_iter().any(|sql| {
                    ddl_tokens(sql, driver).iter().any(|t| {
                        matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
                            && ident(&sql[t.start..t.end]) == *name
                    })
                })
            });
            let fk = plan
                .snapshot
                .foreign_keys
                .iter()
                .any(|fk| fk.columns.contains(name));
            if indexed || fk {
                return Err(invalid(
                    "이 컬럼의 인덱스 또는 외래 키를 먼저 변경하세요. 종속 항목은 자동 삭제하지 않습니다.",
                ));
            }
            plan.sql.push(format!(
                "ALTER TABLE {table} DROP COLUMN {}",
                quote_ident(driver, name)
            ));
            plan.warning = Some(
                kiln_common::i18n::tr(
                    "이 컬럼의 모든 값이 삭제됩니다. 이 작업은 되돌릴 수 없습니다.",
                )
                .into(),
            );
        }
        SchemaAction::AddIndex(spec) => {
            check_name(&spec.name)?;
            if spec.columns.is_empty() {
                return Err(invalid("인덱스에 사용할 컬럼을 선택하세요."));
            }
            if plan.snapshot.indexes.iter().any(|i| i.name == spec.name) {
                return Err(invalid("이미 존재하는 인덱스 이름입니다."));
            }
            let mut seen = std::collections::HashSet::new();
            for c in &spec.columns {
                column(&plan.snapshot, &c.name)?;
                if !seen.insert(&c.name) {
                    return Err(invalid("인덱스 컬럼을 중복 선택할 수 없습니다."));
                }
            }
            let cols = spec
                .columns
                .iter()
                .map(|c| {
                    format!(
                        "{}{}",
                        quote_ident(driver, &c.name),
                        if c.descending { " DESC" } else { " ASC" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let index = if matches!(driver, Driver::Postgres | Driver::Sqlite) {
                qualified(driver, t.schema.as_deref(), &spec.name)
            } else {
                quote_ident(driver, &spec.name)
            };
            // SQLite qualifies the index, rather than the ON table. PG CREATE INDEX
            // derives the index schema from the qualified table, without a schema on its name.
            let index = if driver == Driver::Postgres {
                quote_ident(driver, &spec.name)
            } else {
                index
            };
            let target = if driver == Driver::Sqlite {
                quote_ident(driver, &t.table)
            } else {
                table.clone()
            };
            plan.sql.push(format!(
                "CREATE {}INDEX {index} ON {target} ({cols})",
                if spec.unique { "UNIQUE " } else { "" }
            ));
        }
        SchemaAction::DropIndex { name } => {
            let ix = plan
                .snapshot
                .indexes
                .iter()
                .find(|i| i.name == *name)
                .ok_or_else(|| invalid("인덱스를 찾을 수 없습니다. 구조를 새로고침하세요."))?;
            if ix.primary || ix.constraint {
                return Err(invalid(
                    "기본 키 또는 제약 조건의 인덱스는 이 화면에서 삭제할 수 없습니다.",
                ));
            }
            let index = qualified(driver, t.schema.as_deref(), name);
            plan.sql
                .push(if matches!(driver, Driver::MySql | Driver::MariaDb) {
                    format!("DROP INDEX {} ON {table}", quote_ident(driver, name))
                } else {
                    format!("DROP INDEX {index}")
                });
        }
        SchemaAction::AlterColumn { column: name, spec } => {
            let old = column(&plan.snapshot, name)?.clone();
            check_column(spec, driver)?;
            if spec.name != *name {
                return Err(invalid("컬럼 이름은 이름 변경 메뉴에서 변경하세요."));
            }
            if ColumnSpec::from(&old) == *spec {
                return Err(invalid("변경할 컬럼 속성을 입력하세요."));
            }
            if old.auto_increment {
                return Err(invalid(
                    "자동 생성 컬럼은 이 화면에서 속성을 변경할 수 없습니다. DDL을 확인하세요.",
                ));
            }
            if old.is_pk() && spec.nullable {
                return Err(invalid("기본 키 컬럼은 NULL을 허용할 수 없습니다."));
            }
            if driver == Driver::Postgres {
                let c = quote_ident(driver, name);
                let mut changes = Vec::new();
                if old.data_type != spec.data_type {
                    changes.push(format!("ALTER COLUMN {c} TYPE {}", spec.data_type.trim()));
                }
                if old.nullable != spec.nullable {
                    changes.push(format!(
                        "ALTER COLUMN {c} {} NOT NULL",
                        if spec.nullable { "DROP" } else { "SET" }
                    ));
                }
                if old.default != spec.default {
                    changes.push(format!(
                        "ALTER COLUMN {c} {}",
                        spec.default
                            .as_ref()
                            .map(|v| format!("SET DEFAULT {}", v.trim()))
                            .unwrap_or_else(|| "DROP DEFAULT".into())
                    ));
                }
                plan.sql
                    .push(format!("ALTER TABLE {table} {}", changes.join(", ")));
            } else {
                let raw = if driver == Driver::Sqlite {
                    pool.query(&format!(
                        "SELECT sql FROM main.sqlite_schema WHERE type='table' AND name={}",
                        quote_literal(driver, &t.table)
                    ))
                    .await?
                    .scalar_string()
                    .ok_or_else(|| invalid("이 테이블 정의를 안전하게 편집할 수 없습니다."))?
                } else {
                    plan.definition.clone()
                };
                let (mut clauses, suffix) = table_clauses(&raw, driver)?;
                let ci = clauses
                    .iter()
                    .position(|c| clause_column(c, driver).as_deref() == Some(name))
                    .ok_or_else(|| invalid("이 테이블 정의를 안전하게 편집할 수 없습니다."))?;
                clauses[ci] = rewrite_column(&clauses[ci], driver, &old, spec)?;
                if matches!(driver, Driver::MySql | Driver::MariaDb) {
                    plan.sql
                        .push(format!("ALTER TABLE {table} MODIFY COLUMN {}", clauses[ci]));
                } else {
                    if suffix.to_ascii_uppercase().contains("VIRTUAL") {
                        return Err(invalid(
                            "가상 테이블은 이 화면에서 구조를 변경할 수 없습니다.",
                        ));
                    }
                    let temp = format!(
                        "__kiln_schema_{:x}_{:x}",
                        std::process::id(),
                        TEMP_ID.fetch_add(1, Ordering::Relaxed)
                    );
                    let temp_table = qualified(driver, t.schema.as_deref(), &temp);
                    let mut copy_cols = plan
                        .snapshot
                        .columns
                        .iter()
                        .filter(|c| !c.auto_increment || c.is_pk())
                        .map(|c| quote_ident(driver, &c.name))
                        .collect::<Vec<_>>();
                    let rowid_alias = ["rowid", "_rowid_", "oid"].into_iter().find(|alias| {
                        !plan
                            .snapshot
                            .columns
                            .iter()
                            .any(|c| c.name.eq_ignore_ascii_case(alias))
                    });
                    let integer_pk = plan
                        .snapshot
                        .columns
                        .iter()
                        .any(|c| c.auto_increment && c.is_pk());
                    if !suffix.to_ascii_uppercase().contains("WITHOUT ROWID") && !integer_pk {
                        let alias = rowid_alias.ok_or_else(|| {
                            invalid(
                                "숨겨진 rowid를 보존할 수 없어 이 테이블은 재구성하지 않습니다.",
                            )
                        })?;
                        copy_cols.insert(0, alias.into());
                    }
                    let columns = copy_cols.join(", ");
                    plan.sql.push(format!(
                        "CREATE TABLE {temp_table} (\n  {}\n) {suffix}",
                        clauses.join(",\n  ")
                    ));
                    plan.sql.push(format!(
                        "INSERT INTO {temp_table} ({columns}) SELECT {columns} FROM {table}"
                    ));
                    plan.sql.push(format!("DROP TABLE {table}"));
                    plan.sql.push(format!(
                        "ALTER TABLE {temp_table} RENAME TO {}",
                        quote_ident(driver, &t.table)
                    ));
                    if ddl_tokens(&raw, driver).iter().any(|t| {
                        t.kind == TokKind::Word
                            && raw[t.start..t.end].eq_ignore_ascii_case("AUTOINCREMENT")
                    }) {
                        let seq = pool
                            .query(&format!(
                                "SELECT seq FROM main.sqlite_sequence WHERE name={}",
                                quote_literal(driver, &t.table)
                            ))
                            .await?
                            .scalar_string();
                        if let Some(seq) = seq.and_then(|v| v.parse::<i64>().ok()) {
                            plan.sql.push(format!(
                                "UPDATE main.sqlite_sequence SET seq=MAX(seq,{seq}) WHERE name={}",
                                quote_literal(driver, &t.table)
                            ));
                        }
                    }
                    let objects=pool.query(&format!("SELECT sql FROM {}.sqlite_schema WHERE tbl_name={} AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name",quote_ident(driver,schema),quote_literal(driver,&t.table))).await?;
                    for row in &objects.rows {
                        if let Some(sql) = row.first().and_then(crate::driver::value_string) {
                            plan.sql.push(sql);
                        }
                    }
                    plan.rebuild = true;
                }
            }
            plan.warning=Some(kiln_common::i18n::tr("타입 또는 제약 조건 변경은 기존 데이터를 검사하거나 변환하며 테이블을 잠글 수 있습니다.").into());
        }
    }
    if driver == Driver::Sqlite
        && pool
            .query("PRAGMA main.schema_version")
            .await?
            .scalar_string()
            .and_then(|s| s.parse::<i64>().ok())
            != plan.catalog_version
    {
        return Err(invalid(
            "테이블 구조가 변경되었습니다. 변경 내용을 다시 검토하세요.",
        ));
    }
    if m.connection_epoch(id) != epoch || m.connected_pool(id).is_none() {
        return Err(invalid(
            "연결이 변경되었습니다. 변경 내용을 다시 검토하세요.",
        ));
    }
    if driver == Driver::Postgres
        && pool.query(&pg_catalog_stamp(t)).await?.scalar_string() != plan.pg_stamp
    {
        return Err(invalid(
            "테이블 구조가 변경되었습니다. 변경 내용을 다시 검토하세요.",
        ));
    }
    plan.reviewed = Some(plan.review_key());
    Ok(plan)
}

impl DbManager {
    pub async fn prepare_schema_change(
        &self,
        id: ConnId,
        table: &TableRef,
        action: SchemaAction,
    ) -> DbResult<SchemaPlan> {
        prepare(self, id, table, action).await
    }

    /// Refuse obsolete plans, then execute on the captured existing pool.
    pub async fn apply_schema_change(
        &self,
        id: ConnId,
        plan: SchemaPlan,
    ) -> DbResult<SchemaResult> {
        if plan.reviewed.as_ref() != Some(&plan.review_key()) {
            return Err(invalid(
                "변경 계획이 수정되었습니다. 변경 내용을 다시 검토하세요.",
            ));
        }
        if self.driver(id) != Some(plan.driver) || self.connection_epoch(id) != plan.epoch {
            return Err(invalid(
                "연결이 변경되었습니다. 변경 내용을 다시 검토하세요.",
            ));
        }
        let pool = self
            .connected_pool(id)
            .ok_or_else(|| invalid("먼저 데이터베이스에 연결하세요."))?;
        let fresh = crate::meta::table_details(
            &pool,
            plan.driver,
            plan.table.schema.as_deref(),
            &plan.table.table,
        )
        .await?;
        let ddl = crate::meta::table_ddl(
            &pool,
            plan.driver,
            plan.table.schema.as_deref(),
            &plan.table.table,
        )
        .await?;
        if fresh != plan.snapshot || ddl != plan.definition {
            return Err(invalid(
                "테이블 구조가 변경되었습니다. 변경 내용을 다시 검토하세요.",
            ));
        }
        if self.connection_epoch(id) != plan.epoch {
            return Err(invalid(
                "연결이 변경되었습니다. 변경 내용을 다시 검토하세요.",
            ));
        }
        execute(&pool, &plan).await?;
        self.schema_changed(id);
        let table = match &plan.action {
            SchemaAction::DropTable => None,
            SchemaAction::RenameTable { name } => {
                Some(TableRef::new(plan.table.schema.clone(), name.clone()))
            }
            _ => Some(plan.table),
        };
        Ok(SchemaResult { table })
    }
}

async fn execute(pool: &DbPool, plan: &SchemaPlan) -> DbResult<()> {
    match pool {
        DbPool::Pg(p) => {
            let mut c = p.acquire().await?;
            let mut tx = c.begin().await?;
            sqlx::query("SET LOCAL lock_timeout = '5s'")
                .execute(&mut *tx)
                .await?;
            // Revalidate after acquiring the table lock; preserve reviewed schema.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",
                plan.table.sql_name(plan.driver)
            )))
            .execute(&mut *tx)
            .await?;
            let stamp: Option<String> =
                sqlx::query(sqlx::AssertSqlSafe(pg_catalog_stamp(&plan.table)))
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(|r| r.try_get(0))
                    .transpose()?;
            if stamp != plan.pg_stamp {
                return Err(invalid(
                    "테이블 구조가 변경되었습니다. 변경 내용을 다시 검토하세요.",
                ));
            }
            for sql in &plan.sql {
                sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        DbPool::My(p) => {
            // Every MySQL plan is one ALTER/CREATE/DROP; never promise multi-DDL rollback.
            if plan.sql.len() != 1 {
                return Err(invalid(
                    "이 DB에서는 여러 구조 변경을 한 번에 적용할 수 없습니다.",
                ));
            }
            sqlx::query(sqlx::AssertSqlSafe(plan.sql[0].as_str()))
                .execute(p)
                .await?;
        }
        DbPool::Lite(p) => {
            let c = p.acquire().await?;
            // A detached connection restores/ends all connection-local PRAGMAs on
            // both errors and cancellation, rather than returning FK OFF to the pool.
            let mut c = c.detach();
            if plan.rebuild {
                sqlx::query("PRAGMA foreign_keys=OFF")
                    .execute(&mut c)
                    .await?;
                sqlx::query("PRAGMA legacy_alter_table=ON")
                    .execute(&mut c)
                    .await?;
            }
            let mut tx = c.begin_with("BEGIN IMMEDIATE").await?;
            let stamp: i64 = sqlx::query("PRAGMA main.schema_version")
                .fetch_one(&mut *tx)
                .await?
                .try_get(0)?;
            if plan.catalog_version != Some(stamp) {
                return Err(invalid(
                    "테이블 구조가 변경되었습니다. 변경 내용을 다시 검토하세요.",
                ));
            }
            for sql in &plan.sql {
                sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                    .execute(&mut *tx)
                    .await?;
            }
            if plan.rebuild {
                let fk = sqlx::query("PRAGMA main.foreign_key_check")
                    .fetch_all(&mut *tx)
                    .await?;
                if !fk.is_empty() {
                    return Err(invalid(
                        "외래 키 검증에 실패했습니다. 변경을 적용하지 않았습니다.",
                    ));
                }
            }
            tx.commit().await?;
            c.close().await?;
        }
    }
    Ok(())
}

pub(crate) fn index_definition(ddl: &str, driver: Driver, name: &str) -> Option<String> {
    let (clauses, _) = table_clauses(ddl, driver).ok()?;
    clauses.into_iter().find(|clause| {
        let ts = ddl_tokens(clause, driver);
        if name == "PRIMARY"
            && ts
                .first()
                .is_some_and(|t| clause[t.start..t.end].eq_ignore_ascii_case("PRIMARY"))
        {
            return true;
        }
        ts.windows(2).any(|pair| {
            let key = &clause[pair[0].start..pair[0].end];
            (key.eq_ignore_ascii_case("KEY") || key.eq_ignore_ascii_case("INDEX"))
                && ident(&clause[pair[1].start..pair[1].end]) == name
        })
    })
}

pub(crate) fn index_predicate(ddl: &str, driver: Driver) -> Option<String> {
    let mut depth = 0;
    for t in ddl_tokens(ddl, driver) {
        let text = &ddl[t.start..t.end];
        if t.kind == TokKind::Punct {
            if text == "(" {
                depth += 1;
            }
            if text == ")" {
                depth -= 1;
            }
        }
        if depth == 0 && t.kind == TokKind::Word && text.eq_ignore_ascii_case("WHERE") {
            return Some(ddl[t.end..].trim().trim_end_matches(';').to_owned());
        }
    }
    None
}
