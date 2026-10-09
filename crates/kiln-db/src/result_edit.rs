//! Provenance-checked SELECT editing. SQL recognition is deliberately a strict
//! subset; native driver origins and live catalog metadata are also required.
use crate::driver::{ChangeError, ChangeStmt, DbPool, NativeOrigins, SessionConn};
use crate::sql::{TokKind, quote_ident, quote_literal, tokenize};
use crate::{ColumnDef, DbError, Driver, ResultSet, StmtOutcome, TableRef, TypeClass, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultReadOnlyReason {
    UnsupportedQuery,
    UnknownOrigin,
    NotBaseTable,
    NoUniqueKey,
    KeyNotSelected,
    AmbiguousColumns,
    InvalidRows,
    MetadataUnavailable,
}
impl std::fmt::Display for ResultReadOnlyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::UnsupportedQuery => "단일 테이블의 직접 컬럼 조회 결과만 수정할 수 있습니다",
            Self::UnknownOrigin => "결과 컬럼의 원본을 확인할 수 없어 읽기 전용입니다",
            Self::NotBaseTable => "뷰·임시 테이블·비트랜잭션 테이블은 결과를 수정할 수 없습니다",
            Self::NoUniqueKey => "행을 식별할 기본 키 또는 고유 키가 없어 읽기 전용입니다",
            Self::KeyNotSelected => "행 식별 키의 모든 컬럼을 조회해야 수정할 수 있습니다",
            Self::AmbiguousColumns => "중복되거나 모호한 원본 컬럼이 있어 읽기 전용입니다",
            Self::InvalidRows => "결과 행의 식별 키가 비어 있거나 중복되어 읽기 전용입니다",
            Self::MetadataUnavailable => "원본 메타데이터를 확인하지 못해 읽기 전용입니다",
        };
        f.write_str(kiln_common::i18n::tr(text))
    }
}
#[derive(Clone, Debug)]
pub struct EditableOutcome {
    pub outcome: StmtOutcome,
    pub editing: Result<ResultEditPlan, ResultReadOnlyReason>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ResultEditCell {
    pub row: usize,
    pub column: usize,
    pub value: Value,
}
#[derive(Clone, Debug)]
pub struct ResultEditPlan {
    pub(crate) session: u64,
    pub(crate) generation: u64,
    pub(crate) sql: String,
    driver: Driver,
    table: TableRef,
    columns: Vec<ColumnDef>,
    editable: Vec<bool>,
    keys: Vec<usize>,
    originals: Vec<Vec<Value>>,
    source: Catalog,
}
impl ResultEditPlan {
    pub fn table(&self) -> &TableRef {
        &self.table
    }
    /// Definitions in result order; aliases stay in ResultSet.columns.
    pub fn columns(&self) -> &[ColumnDef] {
        &self.columns
    }
    /// Result-column indices, not base-table ordinals.
    pub fn key_columns(&self) -> &[usize] {
        &self.keys
    }
    pub fn can_edit_column(&self, column: usize) -> bool {
        self.editable.get(column).copied().unwrap_or(false)
    }
    pub fn row_count(&self) -> usize {
        self.originals.len()
    }
    pub(crate) async fn submit(
        &self,
        pool: &DbPool,
        statements: &[ChangeStmt],
    ) -> Result<u64, ChangeError> {
        if statements.is_empty() {
            return Ok(0);
        }
        let invalid = || ChangeError {
            index: 0,
            error: DbError::msg(kiln_common::i18n::tr(
                "원본 테이블이 변경되어 결과를 다시 실행해야 합니다",
            )),
        };
        // A detached connection is physically owned here: if this future is
        // cancelled, closing it rolls back rather than pooling raw BEGIN state.
        let mut conn = pool
            .detach_session()
            .await
            .map_err(|error| ChangeError { index: 0, error })?;
        let begin = if self.driver == Driver::Sqlite {
            "BEGIN IMMEDIATE"
        } else {
            "BEGIN"
        };
        conn.run_sql(begin, None)
            .await
            .map_err(|error| ChangeError { index: 0, error })?;
        let result = async {
            match self.driver {
                Driver::Postgres => {
                    conn.run_sql(
                        "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='30s'",
                        None,
                    )
                    .await
                    .map_err(|error| ChangeError { index: 0, error })?;
                    conn.run_sql(
                        &format!(
                            "LOCK TABLE {} IN ROW EXCLUSIVE MODE",
                            self.table.sql_name(self.driver)
                        ),
                        None,
                    )
                    .await
                    .map_err(|error| ChangeError { index: 0, error })?;
                }
                Driver::MySql | Driver::MariaDb => {
                    conn.run_sql(
                        "SET SESSION lock_wait_timeout=5; SET SESSION innodb_lock_wait_timeout=5",
                        None,
                    )
                    .await
                    .map_err(|error| ChangeError { index: 0, error })?;
                    // SELECT acquires transaction-held MDL before reading the
                    // definition. A view replacement is rejected by catalog().
                    conn.run_sql(
                        &format!("SELECT * FROM {} LIMIT 0", self.table.sql_name(self.driver)),
                        None,
                    )
                    .await
                    .map_err(|error| ChangeError { index: 0, error })?;
                }
                Driver::Sqlite => {}
            }
            let current = catalog(&mut conn, self.driver, &self.table)
                .await
                .map_err(|_| invalid())?;
            if !self.source.same_source(&current) {
                return Err(invalid());
            }
            conn.execute_result_changes(statements).await
        }
        .await;
        match result {
            Ok(total) => {
                conn.run_sql("COMMIT", None)
                    .await
                    .map_err(|error| ChangeError {
                        index: statements.len(),
                        error,
                    })?;
                Ok(total)
            }
            Err(error) => {
                let _ = conn.run_sql("ROLLBACK", None).await;
                Err(error)
            }
        }
    }

    pub(crate) fn changes(&self, cells: &[ResultEditCell]) -> Result<Vec<ChangeStmt>, DbError> {
        let mut rows: BTreeMap<usize, BTreeMap<usize, &Value>> = BTreeMap::new();
        for cell in cells {
            if cell.row >= self.originals.len() || !self.can_edit_column(cell.column) {
                return Err(DbError::msg(kiln_common::i18n::tr(
                    "수정할 결과 셀을 확인할 수 없습니다",
                )));
            }
            if rows
                .entry(cell.row)
                .or_default()
                .insert(cell.column, &cell.value)
                .is_some()
            {
                return Err(DbError::msg(kiln_common::i18n::tr(
                    "같은 결과 셀의 수정이 중복되었습니다",
                )));
            }
        }
        let mut statements = Vec::new();
        for (row, updates) in rows {
            let original = &self.originals[row];
            let updates: Vec<_> = updates
                .into_iter()
                .filter(|(c, v)| original[*c] != **v)
                .collect();
            if updates.is_empty() {
                continue;
            }
            let mut args = Vec::new();
            let mut bind = |value: Value, column: &ColumnDef| {
                args.push(value);
                match self.driver {
                    Driver::Postgres => format!("${}::{}", args.len(), column.cast_type),
                    _ => "?".to_owned(),
                }
            };
            let sets = updates
                .iter()
                .map(|(c, v)| {
                    format!(
                        "{} = {}",
                        quote_ident(self.driver, &self.columns[*c].name),
                        bind((*v).clone(), &self.columns[*c])
                    )
                })
                .collect::<Vec<_>>();
            // Every projected original value is checked, not just the key. Binary
            // text comparisons detect case-only edits on collated columns.
            let mut guards = Vec::new();
            for (c, value) in original.iter().enumerate() {
                let column = &self.columns[c];
                let name = quote_ident(self.driver, &column.name);
                if value.is_null() {
                    guards.push(format!("{name} IS NULL"));
                    continue;
                }
                let guard = match self.driver {
                    Driver::Postgres => {
                        args.push(match value {
                            Value::Bytes(b) => Value::Text(format!("\\x{}", hex::encode(b))),
                            _ => Value::Text(value.to_text().unwrap_or_default()),
                        });
                        if matches!(value, Value::Float(_) | Value::Bool(_)) {
                            // Booleans and floats have different DB/Rust display
                            // forms; compare in the actual source type.
                            format!(
                                "{name} IS NOT DISTINCT FROM ${}::{}",
                                args.len(),
                                column.cast_type
                            )
                        } else {
                            // Explicit bytewise collation overrides inherited
                            // nondeterministic/case-insensitive column collations.
                            // PostgreSQL's CHAR text output includes padding, but
                            // CHAR -> text removes it. Normalize only that bound
                            // source value through unconstrained bpchar; VARCHAR
                            // and TEXT trailing spaces remain significant.
                            let source_cast = if column.cast_type == "bpchar" {
                                "bpchar::text"
                            } else {
                                "text"
                            };
                            format!(
                                "{name}::text COLLATE pg_catalog.\"C\" IS NOT DISTINCT FROM ${}::{source_cast} COLLATE pg_catalog.\"C\"",
                                args.len()
                            )
                        }
                    }
                    Driver::MySql | Driver::MariaDb => {
                        args.push(value.clone());
                        format!("CAST({name} AS BINARY) <=> CAST(? AS BINARY)")
                    }
                    Driver::Sqlite => {
                        args.push(value.clone());
                        let storage = match value {
                            Value::Bool(_) | Value::Int(_) | Value::UInt(_) => "integer",
                            Value::Float(_) => "real",
                            Value::Bytes(_) => "blob",
                            _ => "text",
                        };
                        format!("({name} COLLATE BINARY IS ? AND typeof({name}) = '{storage}')")
                    }
                };
                guards.push(guard);
            }
            statements.push(ChangeStmt {
                sql: format!(
                    "UPDATE {} SET {} WHERE {}",
                    self.table.sql_name(self.driver),
                    sets.join(", "),
                    guards.join(" AND ")
                ),
                args,
                expect_one: true,
            });
        }
        Ok(statements)
    }
}
#[derive(Clone, Debug)]
struct Ident {
    name: String,
    quoted: bool,
}
impl Ident {
    fn matches(&self, name: &str, driver: Driver) -> bool {
        match driver {
            Driver::Postgres if !self.quoted => self.name.to_lowercase() == name,
            Driver::Postgres => self.name == name,
            _ => self.name.eq_ignore_ascii_case(name),
        }
    }
    fn resolved(&self, driver: Driver) -> String {
        if driver == Driver::Postgres && !self.quoted {
            self.name.to_lowercase()
        } else {
            self.name.clone()
        }
    }
}
struct Select {
    table: TableRef,
    alias: Option<Ident>,
    projection: Vec<Projection>,
}
enum Projection {
    Star(Option<Ident>),
    Column(Vec<Ident>, Option<Ident>),
}
fn ident(sql: &str, t: &crate::sql::Token) -> Option<Ident> {
    let s = &sql[t.start..t.end];
    if t.kind == TokKind::Word {
        return Some(Ident {
            name: s.into(),
            quoted: false,
        });
    }
    if t.kind != TokKind::QuotedIdent || s.len() < 2 {
        return None;
    }
    let q = s.chars().next()?;
    if !s.ends_with(q) {
        return None;
    }
    Some(Ident {
        name: s[1..s.len() - 1].replace(&format!("{q}{q}"), &q.to_string()),
        quoted: true,
    })
}
fn parse(sql: &str, driver: Driver) -> Result<Select, ResultReadOnlyReason> {
    let fail = || ResultReadOnlyReason::UnsupportedQuery;
    let mut tokens: Vec<_> = tokenize(sql, driver)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokKind::Space | TokKind::Comment))
        .collect();
    // MySQL executable comments can change the projection or FROM relation.
    if tokenize(sql, driver).iter().any(|t| {
        t.kind == TokKind::Comment
            && (sql[t.start..t.end].starts_with("/*!") || sql[t.start..t.end].starts_with("/*M!"))
    }) {
        return Err(fail());
    }
    if tokens.last().is_some_and(|t| t.kind == TokKind::Semicolon) {
        tokens.pop();
    }
    let word = |i: usize, w: &str| {
        tokens
            .get(i)
            .is_some_and(|t| t.kind == TokKind::Word && sql[t.start..t.end].eq_ignore_ascii_case(w))
    };
    if !word(0, "SELECT") || tokens.iter().any(|t| t.kind == TokKind::Semicolon) {
        return Err(fail());
    }
    let forbidden = [
        "DISTINCT",
        "JOIN",
        "UNION",
        "INTERSECT",
        "EXCEPT",
        "GROUP",
        "HAVING",
        "WINDOW",
        "WITH",
        "INTO",
        "RETURNING",
        "SELECT",
    ];
    if tokens.iter().skip(1).any(|t| {
        t.kind == TokKind::Word
            && forbidden
                .iter()
                .any(|w| sql[t.start..t.end].eq_ignore_ascii_case(w))
    }) {
        return Err(fail());
    }
    let from = (1..tokens.len())
        .find(|&i| word(i, "FROM"))
        .ok_or_else(fail)?;
    let mut i = from + 1;
    let first = ident(sql, tokens.get(i).ok_or_else(fail)?).ok_or_else(fail)?;
    i += 1;
    let (schema, table) = if tokens.get(i).is_some_and(|t| &sql[t.start..t.end] == ".") {
        i += 1;
        let second = ident(sql, tokens.get(i).ok_or_else(fail)?).ok_or_else(fail)?;
        i += 1;
        (Some(first.resolved(driver)), second.resolved(driver))
    } else {
        (None, first.resolved(driver))
    };
    if word(i, "AS") {
        i += 1;
    }
    let clauses = ["WHERE", "ORDER", "LIMIT", "OFFSET", "FETCH", "FOR"];
    let alias = if tokens
        .get(i)
        .is_some_and(|t| matches!(t.kind, TokKind::Word | TokKind::QuotedIdent))
        && !clauses.iter().any(|w| word(i, w))
    {
        let alias = ident(sql, &tokens[i]).ok_or_else(fail)?;
        i += 1;
        Some(alias)
    } else {
        None
    };
    if i < tokens.len() && !clauses.iter().any(|w| word(i, w)) {
        return Err(fail());
    }
    // Restrict the remaining clause syntax enough to exclude alternate FROMs,
    // table samples/joins and SELECT subqueries, while letting the DB validate
    // ordinary predicates and ordering. Parentheses must be balanced.
    let mut depth = 0i32;
    for t in &tokens[i..] {
        let text = &sql[t.start..t.end];
        if t.kind == TokKind::Word && text.eq_ignore_ascii_case("FROM") {
            return Err(fail());
        }
        if text == "(" {
            depth += 1;
        } else if text == ")" {
            depth -= 1;
            if depth < 0 {
                return Err(fail());
            }
        }
    }
    if depth != 0 {
        return Err(fail());
    }
    let mut projection = Vec::new();
    let mut begin = 1;
    for end in (1..=from).filter(|&j| j == from || &sql[tokens[j].start..tokens[j].end] == ",") {
        let group = &tokens[begin..end];
        begin = end + 1;
        if group.is_empty() {
            return Err(fail());
        }
        if group.len() == 1 && &sql[group[0].start..group[0].end] == "*" {
            projection.push(Projection::Star(None));
            continue;
        }
        if group.len() == 3
            && &sql[group[1].start..group[1].end] == "."
            && &sql[group[2].start..group[2].end] == "*"
        {
            projection.push(Projection::Star(Some(
                ident(sql, &group[0]).ok_or_else(fail)?,
            )));
            continue;
        }
        let mut at = 0;
        let mut path = Vec::new();
        path.push(ident(sql, &group[at]).ok_or_else(fail)?);
        at += 1;
        while group.get(at).is_some_and(|t| &sql[t.start..t.end] == ".") {
            at += 1;
            path.push(ident(sql, group.get(at).ok_or_else(fail)?).ok_or_else(fail)?);
            at += 1;
        }
        if path.len() > 3 {
            return Err(fail());
        }
        if group.get(at).is_some_and(|t| {
            t.kind == TokKind::Word && sql[t.start..t.end].eq_ignore_ascii_case("AS")
        }) {
            at += 1;
        }
        let alias = if at < group.len() {
            let alias = ident(sql, &group[at]).ok_or_else(fail)?;
            at += 1;
            Some(alias)
        } else {
            None
        };
        if at != group.len() {
            return Err(fail());
        }
        projection.push(Projection::Column(path, alias));
    }
    Ok(Select {
        table: TableRef::new(schema, table),
        alias,
        projection,
    })
}
#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    table: TableRef,
    columns: Vec<ColumnDef>,
    generated: Vec<bool>,
    keys: Vec<Vec<String>>,
    origin: String,
    epoch: String,
}
impl Catalog {
    fn same_source(&self, other: &Self) -> bool {
        self.table == other.table
            && self.epoch == other.epoch
            && self.columns == other.columns
            && self.generated == other.generated
            && self.keys == other.keys
    }
}
/// Capture before executing the SELECT, then compare again afterwards. Native
/// prepared metadata warms SQLx's origin cache; actual row metadata is retained.
pub(crate) async fn capture(
    conn: &mut SessionConn,
    driver: Driver,
    sql: &str,
) -> Result<Catalog, ResultReadOnlyReason> {
    let parsed = parse(sql, driver)?;
    let source = catalog(conn, driver, &parsed.table).await?;
    conn.describe_origins(sql)
        .await
        .map_err(|_| ResultReadOnlyReason::MetadataUnavailable)?;
    Ok(source)
}
async fn query(conn: &mut SessionConn, sql: &str) -> Result<ResultSet, ResultReadOnlyReason> {
    conn.run_sql(sql, None)
        .await
        .map(|r| r.result)
        .map_err(|_| ResultReadOnlyReason::MetadataUnavailable)
}
fn text(rs: &ResultSet, r: usize, c: usize) -> String {
    rs.rows
        .get(r)
        .and_then(|r| r.get(c))
        .and_then(crate::driver::value_string)
        .unwrap_or_default()
}
async fn catalog(
    conn: &mut SessionConn,
    driver: Driver,
    table: &TableRef,
) -> Result<Catalog, ResultReadOnlyReason> {
    let readonly = || ResultReadOnlyReason::NotBaseTable;
    let q = |s: &str| quote_literal(driver, s);
    let mut table = table.clone();
    let (cols, origin, epoch) = match driver {
        Driver::Postgres => {
            let rs=query(conn,&format!("SELECT n.nspname::text,c.relname::text,c.relkind::text,c.relpersistence::text,c.oid::regclass::text,c.oid::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=pg_catalog.to_regclass({})",q(&table.sql_name(driver)))).await?;
            if rs.rows.len() != 1
                || !matches!(text(&rs, 0, 2).as_str(), "r" | "p")
                || text(&rs, 0, 3) == "t"
            {
                return Err(readonly());
            }
            table = TableRef::new(Some(text(&rs, 0, 0)), text(&rs, 0, 1));
            let cols=query(conn,&format!("SELECT a.attname::text,pg_catalog.format_type(a.atttypid,a.atttypmod),CASE WHEN tn.nspname='pg_catalog' THEN pg_catalog.format_type(a.atttypid,NULL) ELSE pg_catalog.quote_ident(tn.nspname)||'.'||pg_catalog.quote_ident(t.typname) END,NOT a.attnotnull,a.attgenerated::text,COALESCE((SELECT array_position(con.conkey,a.attnum) FROM pg_catalog.pg_constraint con WHERE con.conrelid=c.oid AND con.contype='p'),0) FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_type t ON t.oid=a.atttypid JOIN pg_catalog.pg_namespace tn ON tn.oid=t.typnamespace WHERE c.oid=pg_catalog.to_regclass({}) AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum",q(&table.sql_name(driver)))).await?;
            (cols, text(&rs, 0, 4), text(&rs, 0, 5))
        }
        Driver::MySql | Driver::MariaDb => {
            if table.schema.is_none() {
                table.schema = Some(
                    query(conn, "SELECT DATABASE()")
                        .await?
                        .scalar_string()
                        .ok_or_else(readonly)?,
                );
            }
            let create = query(
                conn,
                &format!("SHOW CREATE TABLE {}", table.sql_name(driver)),
            )
            .await?;
            let definition = text(&create, 0, 1).to_uppercase();
            if definition.starts_with("CREATE TEMPORARY") || !definition.starts_with("CREATE TABLE")
            {
                return Err(readonly());
            }
            let ns = q(table.schema.as_deref().unwrap());
            let tb = q(&table.table);
            let engine=query(conn,&format!("SELECT engine,create_time FROM information_schema.tables WHERE table_schema={ns} AND table_name={tb} AND table_type='BASE TABLE'")).await?;
            if !text(&engine, 0, 0).eq_ignore_ascii_case("InnoDB") {
                return Err(readonly());
            }
            let cols=query(conn,&format!("SELECT column_name,column_type,data_type,is_nullable,extra,IF(column_key='PRI',ordinal_position,0) FROM information_schema.columns WHERE table_schema={ns} AND table_name={tb} ORDER BY ordinal_position")).await?;
            let origin = format!("{}.{}", table.schema.as_deref().unwrap(), table.table);
            let epoch = format!(
                "{}:{}",
                text(&engine, 0, 1),
                mysql_definition(&text(&create, 0, 1))
            );
            (cols, origin, epoch)
        }
        Driver::Sqlite => {
            // Unqualified SQLite names resolve temp before main, then attached
            // databases by attachment order. Preserve that exact resolution.
            let schemas = if let Some(schema) = &table.schema {
                vec![schema.clone()]
            } else {
                let databases = query(conn, "PRAGMA database_list").await?;
                let mut schemas = vec!["temp".into()];
                schemas.extend(
                    (0..databases.rows.len())
                        .map(|r| text(&databases, r, 1))
                        .filter(|s| s != "temp"),
                );
                schemas
            };
            let mut found = None;
            for schema in schemas {
                let rs = query(
                    conn,
                    &format!(
                        "SELECT name,type,sql FROM {}.sqlite_schema WHERE name={} COLLATE NOCASE",
                        quote_ident(driver, &schema),
                        q(&table.table)
                    ),
                )
                .await?;
                if rs.rows.is_empty() {
                    continue;
                }
                if schema == "temp"
                    || rs.rows.len() != 1
                    || text(&rs, 0, 1) != "table"
                    || text(&rs, 0, 2)
                        .to_ascii_uppercase()
                        .starts_with("CREATE VIRTUAL")
                {
                    return Err(readonly());
                }
                // Pool connections do not share ATTACH state. Only main is safe.
                if schema != "main" {
                    return Err(readonly());
                }
                table.table = text(&rs, 0, 0);
                found = Some(schema);
                break;
            }
            table.schema = Some(found.ok_or_else(readonly)?);
            let rs=query(conn,&format!("SELECT name,type,\"notnull\",pk,hidden FROM pragma_table_xinfo({}, {}) ORDER BY cid",q(&table.table),q(table.schema.as_deref().unwrap()))).await?;
            let rows = rs
                .rows
                .iter()
                .filter(|r| r.get(4).and_then(crate::driver::value_string).as_deref() != Some("1"))
                .map(|r| {
                    vec![
                        r[0].clone(),
                        r[1].clone(),
                        r[1].clone(),
                        Value::Bool(r[2] != Value::Int(1)),
                        r[4].clone(),
                        r[3].clone(),
                    ]
                })
                .collect();
            let epoch = query(conn, "PRAGMA main.schema_version")
                .await?
                .scalar_string()
                .ok_or(ResultReadOnlyReason::MetadataUnavailable)?;
            (ResultSet::new(Vec::new(), rows), table.table.clone(), epoch)
        }
    };
    let mut columns = Vec::new();
    let mut generated = Vec::new();
    for r in 0..cols.rows.len() {
        let ty = text(&cols, r, 1);
        let nullable = match driver {
            Driver::MySql | Driver::MariaDb => text(&cols, r, 3) == "YES",
            _ => matches!(text(&cols, r, 3).as_str(), "1" | "true" | "t"),
        };
        let gen_text = text(&cols, r, 4).to_ascii_lowercase();
        let is_generated = match driver {
            Driver::Sqlite => gen_text != "0",
            Driver::Postgres => !gen_text.is_empty(),
            _ => gen_text.contains("generated"),
        };
        columns.push(ColumnDef {
            name: text(&cols, r, 0),
            data_type: ty.clone(),
            cast_type: if driver == Driver::Postgres {
                crate::meta::pg_cast_type(&text(&cols, r, 2))
            } else {
                text(&cols, r, 2)
            },
            nullable,
            default: None,
            pk_ordinal: text(&cols, r, 5).parse().unwrap_or(0),
            auto_increment: false,
            class: TypeClass::from_type_name(&ty),
        });
        generated.push(is_generated);
    }
    let mut primary: Vec<_> = columns.iter().filter(|c| c.pk_ordinal > 0).collect();
    primary.sort_by_key(|c| c.pk_ordinal);
    let mut keys = Vec::new();
    if !primary.is_empty() {
        keys.push(primary.iter().map(|c| c.name.clone()).collect());
    }
    // Safe alternate unique keys: full, valid, non-partial plain-column indexes.
    match driver {
        Driver::Postgres => {
            let rs=query(conn,&format!("SELECT json_agg(a.attname ORDER BY k.ord)::text FROM pg_catalog.pg_index i JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(num,ord) ON k.ord<=i.indnkeyatts JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.num WHERE i.indrelid=pg_catalog.to_regclass({}) AND i.indisunique AND i.indisvalid AND i.indpred IS NULL AND i.indexprs IS NULL GROUP BY i.indexrelid,i.indisprimary ORDER BY i.indisprimary DESC",q(&table.sql_name(driver)))).await?;
            for r in 0..rs.rows.len() {
                if let Ok(key) = serde_json::from_str::<Vec<String>>(&text(&rs, r, 0)) {
                    keys.push(key);
                }
            }
        }
        Driver::MySql | Driver::MariaDb => {
            let rs=query(conn,&format!("SELECT index_name,column_name,sub_part FROM information_schema.statistics WHERE table_schema={} AND table_name={} AND non_unique=0 ORDER BY index_name='PRIMARY' DESC,index_name,seq_in_index",q(table.schema.as_deref().unwrap()),q(&table.table))).await?;
            let mut indexes: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
            for r in 0..rs.rows.len() {
                let key = indexes
                    .entry(text(&rs, r, 0))
                    .or_insert_with(|| Some(Vec::new()));
                let col = text(&rs, r, 1);
                if col.is_empty() || !text(&rs, r, 2).is_empty() {
                    *key = None;
                } else if let Some(key) = key {
                    key.push(col);
                }
            }
            keys.extend(indexes.into_values().flatten());
        }
        Driver::Sqlite => {
            let rs = query(
                conn,
                &format!(
                    "SELECT name FROM pragma_index_list({}, {}) WHERE \"unique\"=1 AND partial=0",
                    q(&table.table),
                    q(table.schema.as_deref().unwrap())
                ),
            )
            .await?;
            for r in 0..rs.rows.len() {
                let ix = query(
                    conn,
                    &format!(
                        "SELECT name FROM pragma_index_xinfo({}, {}) WHERE key=1 ORDER BY seqno",
                        q(&text(&rs, r, 0)),
                        q(table.schema.as_deref().unwrap())
                    ),
                )
                .await?;
                let key: Vec<_> = (0..ix.rows.len()).map(|r| text(&ix, r, 0)).collect();
                if key.iter().all(|s| !s.is_empty()) {
                    keys.push(key);
                }
            }
        }
    }
    // Nullable unique keys do not provide an identity for NULL rows. Primary
    // keys are retained, and actual result values are checked separately.
    let pk: Vec<_> = primary.iter().map(|c| c.name.clone()).collect();
    keys.retain(|key| {
        !key.is_empty()
            && (*key == pk
                || key
                    .iter()
                    .all(|n| columns.iter().any(|c| &c.name == n && !c.nullable)))
    });
    Ok(Catalog {
        table,
        columns,
        generated,
        keys,
        origin,
        epoch,
    })
}
// SHOW CREATE includes a changing AUTO_INCREMENT table counter. Remove only its
// unquoted numeric value; regular inserts must not invalidate the source proof.
fn mysql_definition(sql: &str) -> String {
    let tokens: Vec<_> = tokenize(sql, Driver::MySql)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokKind::Space | TokKind::Comment))
        .collect();
    let mut result = String::new();
    let mut cursor = 0;
    for triple in tokens.windows(3) {
        if matches!(triple[0].kind, TokKind::Word)
            && sql[triple[0].start..triple[0].end].eq_ignore_ascii_case("AUTO_INCREMENT")
            && &sql[triple[1].start..triple[1].end] == "="
            && matches!(triple[2].kind, TokKind::Number)
        {
            result.push_str(&sql[cursor..triple[2].start]);
            cursor = triple[2].end;
        }
    }
    result.push_str(&sql[cursor..]);
    result
}

pub(crate) async fn prepare(
    conn: &mut SessionConn,
    driver: Driver,
    sql: &str,
    result: &ResultSet,
    session: u64,
    generation: u64,
    before: &Catalog,
    origins: &NativeOrigins,
) -> Result<ResultEditPlan, ResultReadOnlyReason> {
    let parsed = parse(sql, driver)?;
    let source = catalog(conn, driver, &parsed.table).await?;
    if !before.same_source(&source) {
        return Err(ResultReadOnlyReason::UnknownOrigin);
    }
    if origins.len() != result.columns.len() {
        return Err(ResultReadOnlyReason::UnknownOrigin);
    }
    let qualifier_ok = |id: &Ident| {
        parsed
            .alias
            .as_ref()
            .is_some_and(|a| id.matches(&a.resolved(driver), driver))
            || id.matches(&parsed.table.table, driver)
    };
    let mut expected = Vec::new();
    for projection in parsed.projection {
        match projection {
            Projection::Star(qualifier) => {
                if qualifier.as_ref().is_some_and(|q| !qualifier_ok(q)) {
                    return Err(ResultReadOnlyReason::UnknownOrigin);
                }
                expected.extend(source.columns.iter().map(|c| (c.name.clone(), None)));
            }
            Projection::Column(path, alias) => {
                if path.len() > 1 && !qualifier_ok(&path[path.len() - 2]) {
                    return Err(ResultReadOnlyReason::UnknownOrigin);
                }
                if path.len() == 3
                    && parsed
                        .table
                        .schema
                        .as_ref()
                        .is_none_or(|s| !path[0].matches(s, driver))
                {
                    return Err(ResultReadOnlyReason::UnknownOrigin);
                }
                let id = path.last().unwrap();
                let column = source
                    .columns
                    .iter()
                    .find(|c| id.matches(&c.name, driver))
                    .ok_or(ResultReadOnlyReason::UnknownOrigin)?;
                expected.push((column.name.clone(), alias.map(|a| a.resolved(driver))));
            }
        }
    }
    if expected.len() != origins.len() {
        return Err(ResultReadOnlyReason::UnknownOrigin);
    }
    let mut columns = Vec::new();
    let mut editable = Vec::new();
    let mut used = BTreeSet::new();
    for (index, ((name, alias), (label, origin))) in expected
        .into_iter()
        .zip(origins.iter().cloned())
        .enumerate()
    {
        let Some((table, column)) = origin else {
            return Err(ResultReadOnlyReason::UnknownOrigin);
        };
        if table != source.origin
            || column != name
            || result.columns[index].name != label
            || alias.as_ref().is_some_and(|a| a != &label)
        {
            return Err(ResultReadOnlyReason::UnknownOrigin);
        }
        if !used.insert(name.clone()) {
            return Err(ResultReadOnlyReason::AmbiguousColumns);
        }
        let at = source
            .columns
            .iter()
            .position(|c| c.name == name)
            .ok_or(ResultReadOnlyReason::UnknownOrigin)?;
        let c = source.columns[at].clone();
        editable.push(
            !source.generated[at]
                && !matches!(
                    c.class,
                    TypeClass::Other | TypeClass::Array | TypeClass::Bytes
                ),
        );
        columns.push(c);
    }
    if source.keys.is_empty() {
        return Err(ResultReadOnlyReason::NoUniqueKey);
    }
    let keys = source
        .keys
        .iter()
        .find_map(|key| {
            key.iter()
                .map(|name| columns.iter().position(|c| &c.name == name))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or(ResultReadOnlyReason::KeyNotSelected)?;
    let mut seen = BTreeSet::new();
    for row in &result.rows {
        if row.len() != columns.len() || keys.iter().any(|&i| row[i].is_null()) {
            return Err(ResultReadOnlyReason::InvalidRows);
        }
        let key: Vec<_> = keys.iter().map(|&i| &row[i]).collect();
        if !seen.insert(serde_json::to_vec(&key).map_err(|_| ResultReadOnlyReason::InvalidRows)?) {
            return Err(ResultReadOnlyReason::InvalidRows);
        }
    }
    Ok(ResultEditPlan {
        session,
        generation,
        sql: sql.into(),
        driver,
        table: source.table.clone(),
        source,
        columns,
        editable,
        keys,
        originals: result.rows.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mysql_definition_ignores_only_live_auto_increment_counter() {
        let make = |count| {
            format!(
                "CREATE TABLE `items` (`id` int AUTO_INCREMENT,`note` varchar(100) DEFAULT 'AUTO_INCREMENT=999') ENGINE=InnoDB AUTO_INCREMENT={count}"
            )
        };
        assert_eq!(mysql_definition(&make(2)), mysql_definition(&make(100)));
        assert!(mysql_definition(&make(2)).contains("'AUTO_INCREMENT=999'"));
        assert_ne!(
            mysql_definition(&make(2)),
            mysql_definition(&make(2).replace("varchar(100)", "varchar(101)"))
        );
    }
    #[tokio::test]
    async fn transaction_executor_rolls_back_prior_write_on_multiple_matches_and_drop() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("owned-transaction.db");
        std::fs::write(&file, []).unwrap();
        let pool = DbPool::Lite(
            sqlx::SqlitePool::connect(&format!("sqlite://{}", file.display()))
                .await
                .unwrap(),
        );
        pool.run_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'one'),(2,'two')",None).await.unwrap();
        let mut conn = pool.detach_session().await.unwrap();
        conn.run_sql("BEGIN IMMEDIATE", None).await.unwrap();
        let changes = vec![
            ChangeStmt {
                sql: "UPDATE items SET name=? WHERE id=1".into(),
                args: vec![Value::Text("first pending".into())],
                expect_one: true,
            },
            ChangeStmt {
                sql: "UPDATE items SET name=?".into(),
                args: vec![Value::Text("multiple".into())],
                expect_one: true,
            },
        ];
        let error = conn.execute_result_changes(&changes).await.unwrap_err();
        assert_eq!(error.index, 1);
        conn.run_sql("ROLLBACK", None).await.unwrap();
        assert_eq!(
            pool.query("SELECT name FROM items WHERE id=1")
                .await
                .unwrap()
                .scalar_string()
                .as_deref(),
            Some("one")
        );
        conn.run_sql("BEGIN IMMEDIATE", None).await.unwrap();
        assert_eq!(conn.execute_result_changes(&changes[..1]).await.unwrap(), 1);
        // The cancellation ownership path closes, never pools, raw BEGIN state.
        drop(conn);
        assert_eq!(
            pool.query("SELECT name FROM items WHERE id=1")
                .await
                .unwrap()
                .scalar_string()
                .as_deref(),
            Some("one")
        );
    }

    #[tokio::test]
    async fn executed_result_keeps_values_but_rejects_replaced_source_proof() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("owned-proof.db");
        std::fs::write(&file, []).unwrap();
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", file.display()))
            .await
            .unwrap();
        let mut conn = DbPool::Lite(pool).detach_session().await.unwrap();
        conn.run_sql("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'Original')",None).await.unwrap();
        let sql = "SELECT * FROM items";
        let before = capture(&mut conn, Driver::Sqlite, sql).await.unwrap();
        let (result, origins) = conn.run_sql_with_origins(sql, None).await.unwrap();
        conn.run_sql("DROP TABLE items; CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'Original')",None).await.unwrap();
        let error = prepare(
            &mut conn,
            Driver::Sqlite,
            sql,
            &result.result,
            1,
            0,
            &before,
            &origins,
        )
        .await
        .unwrap_err();
        assert_eq!(error, ResultReadOnlyReason::UnknownOrigin);
        assert_eq!(result.result.rows[0][1], Value::Text("Original".into()));
    }

    #[test]
    fn postgres_guard_is_bytewise_and_float_uses_its_native_type() {
        let make = |name: &str, ty: &str| ColumnDef {
            name: name.into(),
            data_type: ty.into(),
            cast_type: crate::meta::pg_cast_type(ty),
            nullable: false,
            default: None,
            pk_ordinal: 0,
            auto_increment: false,
            class: TypeClass::from_type_name(ty),
        };
        let plan = ResultEditPlan {
            session: 1,
            generation: 0,
            sql: "SELECT id,name,weight FROM items".into(),
            driver: Driver::Postgres,
            table: TableRef::new(Some("public".into()), "items"),
            columns: vec![
                make("id", "integer"),
                make("name", "character"),
                make("weight", "double precision"),
                make("enabled", "boolean"),
            ],
            editable: vec![true; 4],
            keys: vec![0],
            source: Catalog {
                table: TableRef::new(Some("public".into()), "items"),
                columns: vec![],
                generated: vec![],
                keys: vec![],
                origin: "items".into(),
                epoch: "fixture".into(),
            },
            originals: vec![vec![
                Value::Int(1),
                Value::Text("Alpha       ".into()),
                Value::Float(1e30),
                Value::Bool(true),
            ]],
        };
        let statements = plan
            .changes(&[ResultEditCell {
                row: 0,
                column: 1,
                value: Value::Text("long value".into()),
            }])
            .unwrap();
        assert_eq!(statements.len(), 1);
        let sql = &statements[0].sql;
        assert!(sql.contains("\"name\" = $1::bpchar"));
        assert!(sql.contains("\"name\"::text COLLATE pg_catalog.\"C\" IS NOT DISTINCT FROM $3::bpchar::text COLLATE pg_catalog.\"C\""));
        assert!(sql.contains("\"weight\" IS NOT DISTINCT FROM $4::double precision"));
        assert!(sql.contains("\"enabled\" IS NOT DISTINCT FROM $5::boolean"));
        assert!(statements[0].expect_one);
        for ty in ["text", "character varying"] {
            let mut ordinary_text = plan.clone();
            ordinary_text.columns[1] = make("name", ty);
            let ordinary = ordinary_text
                .changes(&[ResultEditCell {
                    row: 0,
                    column: 1,
                    value: Value::Text("new value".into()),
                }])
                .unwrap();
            assert!(ordinary[0].sql.contains("$3::text COLLATE"));
            assert_eq!(ordinary[0].args[2], Value::Text("Alpha       ".into()));
        }
        assert_eq!(crate::meta::pg_cast_type("bit"), "varbit");
        assert_eq!(
            crate::meta::pg_cast_type("\"types\".\"Custom\""),
            "\"types\".\"Custom\""
        );
    }
}
