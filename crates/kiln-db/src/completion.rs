//! Local, metadata-backed SQL suggestions. No query text or row data leaves the DB connection.
use crate::sql::{self, TokKind, Token};
use crate::{ColumnDef, ConnId, DbManager, Driver, Job, TableInfo, TableRef};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Column,
    Table,
    Schema,
    Keyword,
    Function,
}
impl Kind {
    pub fn label(self) -> &'static str {
        kiln_common::i18n::tr(match self {
            Self::Column => "컬럼",
            Self::Table => "테이블",
            Self::Schema => "스키마",
            Self::Keyword => "키워드",
            Self::Function => "함수",
        })
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub label: String,
    pub detail: String,
    pub insert: String,
    pub kind: Kind,
    /// Put the caret inside function parentheses, rather than after them.
    pub back: usize,
}
#[derive(Default, Clone)]
pub(crate) struct Catalog {
    pub schemas: Vec<String>,
    pub tables: Vec<TableInfo>,
    pub columns: BTreeMap<(String, String), Vec<ColumnDef>>,
}
impl Catalog {
    fn resolve(&self, schema: Option<&str>, table: &str) -> Option<&TableInfo> {
        let mut matches = self
            .tables
            .iter()
            .filter(|t| t.name == table && schema.is_none_or(|s| t.schema.as_deref() == Some(s)));
        let first = matches.next()?;
        // Never guess search_path/current database when names collide.
        matches.next().is_none().then_some(first)
    }
}
fn key(t: &TableInfo) -> (String, String) {
    (t.schema.clone().unwrap_or_default(), t.name.clone())
}
struct CatalogResult {
    catalog: Catalog,
    error: Option<String>,
}
pub(crate) struct Metadata {
    pub catalog: Catalog,
    pub error: Option<String>,
    epoch: Option<u64>,
    catalog_job: Option<Job<CatalogResult>>,
    column_job: Option<((String, String), Job<crate::DbResult<Vec<ColumnDef>>>)>,
    attempted: bool,
    failed_columns: BTreeSet<(String, String)>,
}
impl Default for Metadata {
    fn default() -> Self {
        Self {
            catalog: Catalog::default(),
            error: None,
            epoch: None,
            catalog_job: None,
            column_job: None,
            attempted: false,
            failed_columns: BTreeSet::new(),
        }
    }
}
impl Drop for Metadata {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl Metadata {
    fn cancel(&mut self) {
        if let Some(mut j) = self.catalog_job.take() {
            j.abort();
        }
        if let Some((_, mut j)) = self.column_job.take() {
            j.abort();
        }
    }
    pub fn reset(&mut self) {
        self.cancel();
        self.catalog = Catalog::default();
        self.error = None;
        self.attempted = false;
        self.failed_columns.clear();
    }
    pub fn matches_connection(&self, m: &DbManager, id: ConnId) -> bool {
        self.epoch == Some(m.connection_epoch(id))
    }
    pub fn loading(&self) -> bool {
        self.catalog_job.is_some() || self.column_job.is_some()
    }
    pub fn poll(&mut self, m: &DbManager, id: ConnId, needed: &[(String, String)]) {
        let epoch = m.connection_epoch(id);
        if self.epoch != Some(epoch) {
            self.reset();
            self.epoch = Some(epoch);
        }
        // Completion never reconnects a connection the user explicitly closed.
        let Some(pool) = m.connected_pool(id) else {
            return;
        };
        let Some(driver) = m.driver(id) else {
            return;
        };
        if !self.attempted {
            self.attempted = true;
            let pool = pool.clone();
            self.catalog_job = Some(m.spawn(async move {
                let mut catalog = Catalog::default();
                let mut error = None;
                let result = tokio::time::timeout(Duration::from_secs(12), async {
                    catalog.schemas = crate::meta::list_schemas(&pool, driver).await?;
                    for schema in &catalog.schemas {
                        match crate::meta::list_tables(&pool, driver, schema).await {
                            Ok(t) => catalog.tables.extend(t),
                            Err(e) => {
                                error = Some(e.to_string());
                            }
                        }
                    }
                    Ok::<_, crate::DbError>(())
                })
                .await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => error = Some(e.to_string()),
                    Err(_) => {
                        error = Some(
                            kiln_common::i18n::tr("메타데이터 조회 시간이 초과되었습니다").into(),
                        )
                    }
                }
                CatalogResult { catalog, error }
            }));
        }
        if let Some(j) = &mut self.catalog_job {
            if let Some(result) = j.poll() {
                self.catalog = result.catalog;
                self.error = result.error;
                self.catalog_job = None;
            } else if !j.is_running() {
                self.error = Some(kiln_common::i18n::tr("메타데이터 조회가 중단되었습니다").into());
                self.catalog_job = None;
            }
        }
        if let Some((table, j)) = &mut self.column_job {
            if let Some(result) = j.poll() {
                match result {
                    Ok(cols) => {
                        self.catalog.columns.insert(table.clone(), cols);
                    }
                    Err(e) => {
                        self.error = Some(e.to_string());
                        self.failed_columns.insert(table.clone());
                    }
                }
                self.column_job = None;
            } else if !j.is_running() {
                self.error = Some(kiln_common::i18n::tr("메타데이터 조회가 중단되었습니다").into());
                self.failed_columns.insert(table.clone());
                self.column_job = None;
            }
        }
        if self.catalog_job.is_none() && self.column_job.is_none() {
            if let Some(table) = needed.iter().find(|k| {
                !self.catalog.columns.contains_key(*k) && !self.failed_columns.contains(*k)
            }) {
                let table = table.clone();
                let pool = pool.clone();
                let t = TableRef::new(Some(table.0.clone()), table.1.clone());
                self.column_job = Some((
                    table,
                    m.spawn(async move {
                        match tokio::time::timeout(
                            Duration::from_secs(12),
                            crate::meta::table_details(
                                &pool,
                                driver,
                                t.schema.as_deref(),
                                &t.table,
                            ),
                        )
                        .await
                        {
                            Ok(result) => result.map(|d| d.columns),
                            Err(_) => Err(crate::DbError::msg(kiln_common::i18n::tr(
                                "메타데이터 조회 시간이 초과되었습니다",
                            ))),
                        }
                    }),
                ));
            }
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub replace: Range<usize>,
    pub prefix: String,
    pub qualifier: Vec<String>,
    pub tables_only: bool,
    pub quoted: bool,
    pub bindings: Vec<(String, (String, String))>,
    pub blocked: BTreeSet<String>,
}
fn quoted_body(s: &str, at_end: bool) -> String {
    let q = s.chars().next().expect("quoted token");
    let body = &s[q.len_utf8()..];
    // A doubled quote belongs to the name; strip only an actual closing delimiter.
    let body = if at_end && s.chars().rev().take_while(|c| *c == q).count() % 2 == 1 {
        body.strip_suffix(q).unwrap_or(body)
    } else {
        body
    };
    body.replace(&format!("{q}{q}"), &q.to_string())
}
fn ident(sql: &str, t: Token, driver: Driver) -> Option<String> {
    match t.kind {
        TokKind::Word => Some(if driver == Driver::Postgres {
            sql[t.start..t.end].to_ascii_lowercase()
        } else {
            sql[t.start..t.end].to_string()
        }),
        TokKind::QuotedIdent => Some(quoted_body(&sql[t.start..t.end], true)),
        _ => None,
    }
}
fn punct(sql: &str, t: Token, s: &str) -> bool {
    t.kind == TokKind::Punct && &sql[t.start..t.end] == s
}
fn cte_names(sql: &str, tokens: &[Token], driver: Driver) -> BTreeSet<String> {
    if !tokens
        .first()
        .is_some_and(|t| sql[t.start..t.end].eq_ignore_ascii_case("WITH"))
    {
        return BTreeSet::new();
    }
    let mut ctes = BTreeSet::new();
    let mut d = 0;
    let mut previous = None;
    for t in tokens {
        if punct(sql, *t, "(") {
            d += 1;
        } else if punct(sql, *t, ")") {
            d -= 1;
        } else if d == 0 {
            if sql[t.start..t.end].eq_ignore_ascii_case("SELECT") {
                break;
            }
            if sql[t.start..t.end].eq_ignore_ascii_case("AS") {
                if let Some(name) = previous.take() {
                    ctes.insert(name);
                }
            }
            if let Some(name) = ident(sql, *t, driver) {
                previous = Some(name);
            }
        }
    }
    ctes
}
pub(crate) fn context(
    sql: &str,
    driver: Driver,
    caret: usize,
    catalog: &Catalog,
) -> Option<Context> {
    let caret = sql.char_indices().nth(caret).map_or(sql.len(), |(i, _)| i);
    let all = sql::tokenize(sql, driver);
    // At the end of an unfinished string/comment it still belongs to that token.
    if all.iter().any(|t| {
        matches!(t.kind, TokKind::Comment | TokKind::Str | TokKind::Param)
            && t.start < caret
            && caret <= t.end
    }) {
        return None;
    }
    let start = all
        .iter()
        .filter(|t| t.kind == TokKind::Semicolon && t.end <= caret)
        .map(|t| t.end)
        .next_back()
        .unwrap_or(0);
    let end = all
        .iter()
        .find(|t| t.kind == TokKind::Semicolon && t.start >= caret)
        .map_or(sql.len(), |t| t.start);
    let mut tokens: Vec<_> = all
        .into_iter()
        .filter(|t| {
            t.start >= start && t.end <= end && !matches!(t.kind, TokKind::Space | TokKind::Comment)
        })
        .collect();
    let mut ctes = cte_names(sql, &tokens, driver);
    // Restrict metadata bindings to the innermost SELECT subquery enclosing the caret.
    let mut stack = Vec::new();
    let mut scopes = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if punct(sql, *t, "(") {
            stack.push(i);
        } else if punct(sql, *t, ")") {
            if let Some(a) = stack.pop() {
                scopes.push((a, i));
            }
        }
    }
    for a in stack {
        scopes.push((a, tokens.len()));
    }
    for &(a, b) in &scopes {
        if tokens[a].end <= caret && (b == tokens.len() || caret <= tokens[b].start) {
            ctes.extend(cte_names(sql, &tokens[a + 1..b], driver));
        }
    }
    if let Some(&(a, b)) = scopes
        .iter()
        .filter(|&&(a, b)| {
            tokens[a].end <= caret
                && (b == tokens.len() || caret <= tokens[b].start)
                && tokens.get(a + 1).is_some_and(|t| {
                    sql[t.start..t.end].eq_ignore_ascii_case("SELECT")
                        || sql[t.start..t.end].eq_ignore_ascii_case("WITH")
                })
        })
        .max_by_key(|(a, _)| *a)
    {
        tokens = tokens[a + 1..b].to_vec();
    }
    let mut depth = 0;
    let mut flat = Vec::new();
    for t in &tokens {
        if punct(sql, *t, "(") {
            depth += 1;
        } else if punct(sql, *t, ")") {
            depth -= 1;
        } else if depth == 0 {
            flat.push(*t);
        }
    }
    // UNION branches have independent aliases.
    let begin = flat
        .iter()
        .filter(|t| sql[t.start..t.end].eq_ignore_ascii_case("UNION") && t.end < caret)
        .map(|t| t.end)
        .next_back()
        .unwrap_or(start);
    let finish = flat
        .iter()
        .find(|t| sql[t.start..t.end].eq_ignore_ascii_case("UNION") && t.start >= caret)
        .map_or(end, |t| t.start);
    flat.retain(|t| t.start >= begin && t.end <= finish);
    let current = tokens
        .iter()
        .find(|t| {
            t.start < caret
                && caret <= t.end
                && matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
        })
        .copied();
    let (replace, prefix, quoted) = if let Some(t) = current {
        let quoted = t.kind == TokKind::QuotedIdent;
        let s = &sql[t.start..caret];
        let prefix = if quoted {
            quoted_body(s, caret == t.end)
        } else {
            s.to_string()
        };
        (t.start..t.end, prefix, quoted)
    } else {
        (caret..caret, String::new(), false)
    };
    let before: Vec<_> = tokens
        .iter()
        .filter(|t| t.end <= replace.start)
        .copied()
        .collect();
    let mut qualifier = Vec::new();
    let mut n = before.len();
    while n >= 2 && punct(sql, before[n - 1], ".") {
        if let Some(s) = ident(sql, before[n - 2], driver) {
            qualifier.insert(0, s);
            n -= 2;
        } else {
            break;
        }
    }
    let clause = flat
        .iter()
        .filter(|t| t.end <= replace.start)
        .filter_map(|t| ident(sql, *t, driver))
        .rev()
        .find(|s| {
            matches!(
                s.to_ascii_uppercase().as_str(),
                "SELECT"
                    | "FROM"
                    | "JOIN"
                    | "ON"
                    | "WHERE"
                    | "SET"
                    | "UPDATE"
                    | "INTO"
                    | "GROUP"
                    | "ORDER"
                    | "HAVING"
                    | "VALUES"
                    | "RETURNING"
            )
        });
    let insert_columns = tokens
        .first()
        .is_some_and(|t| sql[t.start..t.end].eq_ignore_ascii_case("INSERT"))
        && scopes.iter().any(|&(a, b)| {
            tokens[a].end <= caret
                && (b == tokens.len() || caret <= tokens[b].start)
                && !tokens[..a].iter().any(|t| {
                    sql[t.start..t.end].eq_ignore_ascii_case("VALUES") || punct(sql, *t, "(")
                })
        });
    let tables_only = !insert_columns
        && clause.is_some_and(|s| {
            matches!(
                s.to_ascii_uppercase().as_str(),
                "FROM" | "JOIN" | "UPDATE" | "INTO"
            )
        });
    let mut blocked = ctes.clone();
    let mut bindings = Vec::new();
    let mut table_clause = false;
    let mut expect = false;
    let mut i = 0;
    while i < flat.len() {
        let t = flat[i];
        let word = ident(sql, t, driver).unwrap_or_default();
        let up = word.to_ascii_uppercase();
        if matches!(up.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            expect = true;
            table_clause = true;
            i += 1;
            continue;
        }
        if matches!(
            up.as_str(),
            "WHERE" | "ON" | "SET" | "GROUP" | "ORDER" | "HAVING" | "VALUES" | "RETURNING"
        ) {
            table_clause = false;
            expect = false;
        }
        if table_clause && punct(sql, t, ",") {
            expect = true;
            i += 1;
            continue;
        }
        if expect {
            expect = false;
            // Skip derived table aliases: flattened tokens must not treat an AS/alias as a table.
            if tokens.iter().any(|q| {
                q.start < t.start && q.end > flat[i.saturating_sub(1)].end && punct(sql, *q, ")")
            }) {
                let alias = if up == "AS" {
                    flat.get(i + 1).and_then(|t| ident(sql, *t, driver))
                } else {
                    ident(sql, t, driver)
                };
                if let Some(alias) = alias {
                    blocked.insert(alias);
                }
                i += 1;
                continue;
            }
            if let Some(mut name) = ident(sql, t, driver) {
                let mut schema = None;
                i += 1;
                if i + 1 < flat.len() && punct(sql, flat[i], ".") {
                    schema = Some(name);
                    name = ident(sql, flat[i + 1], driver).unwrap_or_default();
                    i += 2;
                }
                let alias = if flat
                    .get(i)
                    .is_some_and(|t| sql[t.start..t.end].eq_ignore_ascii_case("AS"))
                {
                    i += 1;
                    flat.get(i).and_then(|t| ident(sql, *t, driver))
                } else {
                    flat.get(i)
                        .and_then(|t| ident(sql, *t, driver))
                        .filter(|s| !sql::is_keyword(s))
                };
                if schema.is_none() && ctes.contains(&name) {
                    if let Some(alias) = alias {
                        blocked.insert(alias);
                    }
                    continue;
                }
                if let Some(table) = catalog.resolve(schema.as_deref(), &name) {
                    bindings.push((alias.unwrap_or_else(|| name.clone()), key(table)));
                }
                continue;
            }
        }
        i += 1;
    }
    Some(Context {
        replace,
        prefix,
        qualifier,
        tables_only,
        quoted,
        bindings,
        blocked,
    })
}
fn insert_ident(driver: Driver, name: &str, force: bool) -> String {
    if !force
        && !sql::is_keyword(name)
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        name.into()
    } else {
        sql::quote_ident(driver, name)
    }
}
const COMMON: &[&str] = &[
    "SELECT",
    "FROM",
    "JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "INNER JOIN",
    "CROSS JOIN",
    "ON",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS NULL",
    "IS NOT NULL",
    "AS",
    "DISTINCT",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "ASC",
    "DESC",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "CREATE TABLE",
    "ALTER TABLE",
    "DROP TABLE",
    "WITH",
    "UNION",
    "UNION ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "EXISTS",
    "IN",
    "BETWEEN",
    "LIKE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "CURRENT_TIMESTAMP",
];
pub(crate) fn candidates(c: &Context, driver: Driver, catalog: &Catalog) -> Vec<Candidate> {
    let mut out = Vec::new();
    let prefix = c.prefix.to_lowercase();
    let qualified = !c.qualifier.is_empty();
    let targets: Vec<(String, (String, String))> = if qualified && !c.tables_only {
        let qualifier = c.qualifier.last().unwrap();
        let bound: Vec<_> = c
            .bindings
            .iter()
            .filter(|(alias, k)| alias == qualifier || &k.1 == qualifier && c.qualifier.len() == 1)
            .map(|(a, k)| (a.clone(), k.clone()))
            .collect();
        if !bound.is_empty() {
            bound
        } else if c.qualifier.len() == 1 && c.blocked.contains(qualifier) {
            Vec::new()
        } else {
            catalog
                .resolve(
                    c.qualifier
                        .get(c.qualifier.len().wrapping_sub(2))
                        .map(String::as_str),
                    qualifier,
                )
                .map(|t| (qualifier.clone(), key(t)))
                .into_iter()
                .collect()
        }
    } else {
        c.bindings.clone()
    };
    if !c.tables_only {
        for (alias, k) in &targets {
            if let Some(cols) = catalog.columns.get(k) {
                for col in cols {
                    let duplicate = targets
                        .iter()
                        .filter(|(_, k)| {
                            catalog
                                .columns
                                .get(k)
                                .is_some_and(|cols| cols.iter().any(|c| c.name == col.name))
                        })
                        .count()
                        > 1;
                    let mut insert = insert_ident(driver, &col.name, c.quoted);
                    let label = if duplicate && !qualified {
                        insert = format!("{}.{}", insert_ident(driver, alias, false), insert);
                        format!("{}.{}", alias, col.name)
                    } else {
                        col.name.clone()
                    };
                    // Filter on the column itself as well as a disambiguating alias.
                    if col.name.to_lowercase().starts_with(&prefix) {
                        out.push(Candidate {
                            label,
                            detail: format!("{}.{} · {}", k.0, k.1, col.data_type),
                            insert,
                            kind: Kind::Column,
                            back: 0,
                        });
                    }
                }
            }
        }
    }
    // Release closure borrow before direct column pushes above (closure recreated below).
    let mut add = |label: String, detail: String, insert: String, kind, back| {
        if label.to_lowercase().starts_with(&prefix) {
            out.push(Candidate {
                label,
                detail,
                insert,
                kind,
                back,
            });
        }
    };
    let schema = c.qualifier.last().filter(|s| catalog.schemas.contains(*s));
    if c.tables_only || !qualified || schema.is_some() {
        for t in &catalog.tables {
            if qualified && schema.is_none_or(|s| t.schema.as_ref() != Some(s)) {
                continue;
            }
            let insert = if qualified {
                insert_ident(driver, &t.name, c.quoted)
            } else {
                sql::qualified(driver, t.schema.as_deref(), &t.name)
            };
            add(
                t.name.clone(),
                t.schema.clone().unwrap_or_default(),
                insert,
                Kind::Table,
                0,
            );
        }
        if !qualified {
            for s in &catalog.schemas {
                add(
                    s.clone(),
                    driver.label().into(),
                    format!("{}.", insert_ident(driver, s, c.quoted)),
                    Kind::Schema,
                    0,
                );
            }
        }
    }
    if !qualified && !c.quoted {
        let dialect: &[&str] = match driver {
            Driver::Postgres => &["ILIKE", "RETURNING", "ON CONFLICT"],
            Driver::Sqlite => &["PRAGMA", "RETURNING", "ON CONFLICT"],
            _ => &["SHOW", "DESCRIBE", "ON DUPLICATE KEY UPDATE"],
        };
        for s in COMMON.iter().chain(dialect) {
            add(
                (*s).into(),
                driver.label().into(),
                (*s).into(),
                Kind::Keyword,
                0,
            );
        }
        if !c.tables_only {
            let functions: &[&str] = match driver {
                Driver::Postgres => &["NOW", "DATE_TRUNC", "STRING_AGG"],
                Driver::Sqlite => &["IFNULL", "STRFTIME", "JULIANDAY", "GROUP_CONCAT"],
                _ => &["IFNULL", "DATE_FORMAT", "JSON_EXTRACT", "GROUP_CONCAT"],
            };
            for s in [
                "COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "NULLIF", "LENGTH", "LOWER",
                "UPPER", "ABS", "ROUND",
            ]
            .iter()
            .chain(functions)
            {
                add(
                    (*s).into(),
                    format!("{}() · {}", s, driver.label()),
                    format!("{s}()"),
                    Kind::Function,
                    1,
                );
            }
        }
    }
    out.sort_by(|a, b| {
        (a.kind, a.label.to_lowercase(), &a.detail).cmp(&(
            b.kind,
            b.label.to_lowercase(),
            &b.detail,
        ))
    });
    out.dedup_by(|a, b| a.insert == b.insert && a.kind == b.kind);
    out.truncate(200);
    out
}
pub(crate) fn needed(c: &Context, catalog: &Catalog) -> Vec<(String, String)> {
    let mut keys: Vec<_> = c.bindings.iter().map(|(_, k)| k.clone()).collect();
    if !c.tables_only {
        if let Some(table) = c
            .qualifier
            .last()
            .filter(|t| c.qualifier.len() > 1 || !c.blocked.contains(*t))
        {
            if let Some(t) = catalog.resolve(
                c.qualifier
                    .get(c.qualifier.len().wrapping_sub(2))
                    .map(String::as_str),
                table,
            ) {
                keys.push(key(t));
            }
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Catalog {
        let mut c = Catalog {
            schemas: vec!["public".into(), "archive".into()],
            ..Default::default()
        };
        for (schema, name, cols) in [
            ("public", "users", vec!["id", "email", "이름", "select"]),
            ("public", "orders", vec!["id", "user_id", "total"]),
            ("archive", "events", vec!["id", "at"]),
        ] {
            c.tables.push(TableInfo {
                schema: Some(schema.into()),
                name: name.into(),
                kind: crate::TableKind::Table,
                row_estimate: None,
                comment: String::new(),
            });
            c.columns.insert(
                (schema.into(), name.into()),
                cols.into_iter()
                    .map(|name| ColumnDef {
                        name: name.into(),
                        data_type: "TEXT".into(),
                        cast_type: "text".into(),
                        nullable: true,
                        default: None,
                        pk_ordinal: 0,
                        auto_increment: false,
                        class: crate::TypeClass::Text,
                    })
                    .collect(),
            );
        }
        c
    }
    fn suggest(sql: &str, driver: Driver, c: &Catalog) -> (Context, Vec<Candidate>) {
        let caret = sql.find('|').unwrap();
        let text = sql.replace('|', "");
        let context = context(&text, driver, sql[..caret].chars().count(), c).unwrap();
        let items = candidates(&context, driver, c);
        (context, items)
    }
    #[test]
    fn tables_aliases_and_columns_use_current_sql_context() {
        let c = catalog();
        for text in [
            "SELECT * FROM us|",
            "SELECT * FROM public.us|",
            "SELECT * FROM users JOIN ord|",
            "DELETE FROM us|",
            "UPDATE us|",
        ] {
            let (ctx, items) = suggest(text, Driver::Postgres, &c);
            assert!(ctx.tables_only);
            assert!(
                items.iter().any(|i| i.kind == Kind::Table),
                "{text}: {items:?}"
            );
            assert!(!items.iter().any(|i| i.kind == Kind::Column));
        }
        for text in [
            "SELECT u.em| FROM users u",
            "SELECT u.em| FROM public.users AS u",
            "SELECT * FROM users u WHERE u.em|",
            "SELECT MAX(u.em|) FROM users u",
            "UPDATE users SET em|",
        ] {
            let (_, items) = suggest(text, Driver::Postgres, &c);
            assert!(
                items.iter().any(|i| i.label == "email"),
                "{text}: {items:?}"
            );
            assert!(!items.iter().any(|i| i.label == "total"));
        }
        let (_, items) = suggest(
            "SELECT id| FROM users u JOIN orders o ON u.id=o.user_id",
            Driver::Postgres,
            &c,
        );
        assert!(items.iter().any(|i| i.insert == "u.id"));
        assert!(items.iter().any(|i| i.insert == "o.id"));
        let (_, items) = suggest(
            "SELECT o.| FROM users u JOIN orders o ON u.id=o.user_id",
            Driver::Postgres,
            &c,
        );
        assert!(items.iter().any(|i| i.label == "total"));
        assert!(!items.iter().any(|i| i.label == "email"));
    }
    #[test]
    fn identifiers_replace_full_token_and_preserve_dialect_quotes_unicode() {
        let c = catalog();
        let (ctx, items) = suggest(
            "SELECT '🙂'; SELECT u.\"이|름\" FROM users u",
            Driver::Postgres,
            &c,
        );
        assert_eq!(ctx.prefix, "이");
        assert_eq!(items[0].insert, "\"이름\"");
        let text = "SELECT '🙂'; SELECT u.\"이름\" FROM users u";
        assert_eq!(&text[ctx.replace], "\"이름\"");
        let (_, items) = suggest("SELECT u.`sel|ect` FROM users u", Driver::MySql, &c);
        assert_eq!(items[0].insert, "`select`");
        let (_, items) = suggest("SELECT * FROM us|ers", Driver::Postgres, &c);
        assert_eq!(items[0].insert, "\"public\".\"users\"");
    }
    #[test]
    fn scope_does_not_leak_across_statements_union_or_subqueries() {
        let c = catalog();
        for text in [
            "SELECT * FROM users u; SEL|",
            "SELECT * FROM users u; |",
            "SELECT email FROM users u UNION SELECT em| FROM orders o",
            "SELECT (SELECT u.em| FROM orders u) FROM users u",
            "WITH users AS (SELECT id FROM orders) SELECT users.| FROM users",
        ] {
            let (_, items) = suggest(text, Driver::Postgres, &c);
            assert!(
                !items.iter().any(|i| i.label == "email"),
                "{text}: {items:?}"
            );
        }
        let (_, items) = suggest(
            "WITH users (id) AS (SELECT id FROM orders) SELECT users.| FROM users",
            Driver::Postgres,
            &c,
        );
        assert!(!items.iter().any(|i| i.label == "email"));
    }
    #[test]
    fn strings_comments_and_dollar_quotes_never_produce_completions() {
        for (driver, text) in [
            (Driver::Postgres, "SELECT 'us|"),
            (Driver::Postgres, "SELECT $body$us|$body$"),
            (Driver::Postgres, "-- us|"),
            (Driver::MySql, "# us|"),
            (Driver::Sqlite, "/* us| */"),
        ] {
            let caret = text.find('|').unwrap();
            assert!(
                context(
                    &text.replace('|', ""),
                    driver,
                    text[..caret].chars().count(),
                    &catalog()
                )
                .is_none()
            );
        }
    }
    #[test]
    fn same_named_tables_are_qualified_and_never_guess_columns() {
        let mut c = catalog();
        let mut t = c.tables[0].clone();
        t.schema = Some("archive".into());
        c.tables.push(t);
        let (_, items) = suggest("SELECT * FROM us|", Driver::Postgres, &c);
        assert_eq!(items.iter().filter(|i| i.kind == Kind::Table).count(), 2);
        let (_, items) = suggest("SELECT u.em| FROM users u", Driver::Postgres, &c);
        assert!(!items.iter().any(|i| i.kind == Kind::Column));
        let (_, items) = suggest("SELECT u.em| FROM public.users u", Driver::Postgres, &c);
        assert!(items.iter().any(|i| i.label == "email"));
    }
    #[test]
    fn self_join_cte_aliases_and_insert_columns() {
        let c = catalog();
        let (_, items) = suggest(
            "SELECT id| FROM users u JOIN users v ON u.id=v.id",
            Driver::Postgres,
            &c,
        );
        assert!(items.iter().any(|i| i.insert == "u.id"));
        assert!(items.iter().any(|i| i.insert == "v.id"));
        for text in [
            "WITH x AS (SELECT id FROM orders) SELECT users.| FROM x users",
            "WITH users AS (SELECT id FROM orders) SELECT (SELECT users.| FROM users)",
            "SELECT (WITH users AS (SELECT id FROM orders) SELECT users.| FROM users)",
        ] {
            let (_, items) = suggest(text, Driver::Postgres, &c);
            assert!(
                !items.iter().any(|i| i.kind == Kind::Column),
                "{text}: {items:?}"
            );
        }
        for text in [
            "SELECT u.em| FROM USERS u",
            "INSERT INTO users (em|) VALUES ('x')",
            "WITH users AS (SELECT id FROM orders) SELECT u.em| FROM public.users u",
        ] {
            let (_, items) = suggest(text, Driver::Postgres, &c);
            assert!(
                items.iter().any(|i| i.label == "email"),
                "{text}: {items:?}"
            );
        }
    }
    #[test]
    fn doubled_quotes_preserve_delimiters_in_identifier_names() {
        assert_eq!(quoted_body("\"\"\"users\"\"\"", true), "\"users\"");
        assert_eq!(quoted_body("\"name\"\"", true), "name\"");
        assert_eq!(quoted_body("`odd``table`", true), "odd`table");
    }
    #[test]
    fn dialect_functions_and_keywords_are_separate() {
        let c = catalog();
        let (_, pg) = suggest("SELECT IFN|", Driver::Postgres, &c);
        assert!(pg.is_empty());
        for d in [Driver::MySql, Driver::MariaDb, Driver::Sqlite] {
            let (_, items) = suggest("SELECT IFN|", d, &c);
            assert_eq!(items[0].insert, "IFNULL()");
            assert_eq!(items[0].back, 1);
        }
        let (_, pg) = suggest("SELECT * FROM users WHERE email IL|", Driver::Postgres, &c);
        assert!(pg.iter().any(|i| i.label == "ILIKE"));
        let (_, mysql) = suggest("SELECT * FROM users WHERE email IL|", Driver::MySql, &c);
        assert!(!mysql.iter().any(|i| i.label == "ILIKE"));
    }
}
