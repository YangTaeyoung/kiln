//! SQL 텍스트 처리: 식별자/리터럴 인용, 렉서, 문장 분리, 자동 LIMIT.

use crate::Driver;
use std::ops::Range;

/// 드라이버 규칙에 맞게 식별자를 인용한다.
pub fn quote_ident(driver: Driver, name: &str) -> String {
    match driver {
        Driver::MySql | Driver::MariaDb => format!("`{}`", name.replace('`', "``")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// 드라이버 규칙에 맞게 문자열 리터럴을 인용한다.
pub fn quote_literal(driver: Driver, s: &str) -> String {
    match driver {
        Driver::MySql | Driver::MariaDb => {
            format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
        }
        Driver::Postgres if s.contains('\\') => {
            format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
        }
        _ => format!("'{}'", s.replace('\'', "''")),
    }
}

/// `schema.table` 형식의 인용된 이름.
pub fn qualified(driver: Driver, schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => {
            format!("{}.{}", quote_ident(driver, s), quote_ident(driver, table))
        }
        _ => quote_ident(driver, table),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Word,
    QuotedIdent,
    Str,
    Number,
    Comment,
    Punct,
    Space,
    Semicolon,
    Param,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// 드라이버 문법에 맞춰 SQL 을 토큰으로 나눈다. 닫히지 않은 문자열/주석은 끝까지 한 토큰이다.
pub fn tokenize(sql: &str, driver: Driver) -> Vec<Token> {
    let b = sql.as_bytes();
    let n = b.len();
    let mysql = matches!(driver, Driver::MySql | Driver::MariaDb);
    let pg = driver == Driver::Postgres;
    let mut out = Vec::new();
    let mut i = 0usize;
    let char_at = |i: usize| sql[i..].chars().next().unwrap_or('\0');
    while i < n {
        let c = char_at(i);
        let start = i;
        let kind;
        if c.is_whitespace() {
            while i < n && char_at(i).is_whitespace() {
                i += char_at(i).len_utf8();
            }
            kind = TokKind::Space;
        } else if (c == '-' && b.get(i + 1) == Some(&b'-')) || (c == '#' && mysql) {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            kind = TokKind::Comment;
        } else if c == '/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut depth = 1;
            while i < n {
                if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else if pg && b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            kind = TokKind::Comment;
        } else if c == '\'' || ((c == 'E' || c == 'e') && pg && b.get(i + 1) == Some(&b'\'')) {
            let backslash = mysql || c != '\'';
            if c != '\'' {
                i += 1;
            }
            i = scan_quoted(b, i, b'\'', backslash);
            kind = TokKind::Str;
        } else if c == '"' {
            i = scan_quoted(b, i, b'"', mysql);
            kind = if mysql {
                TokKind::Str
            } else {
                TokKind::QuotedIdent
            };
        } else if c == '`' && driver != Driver::Postgres {
            i = scan_quoted(b, i, b'`', false);
            kind = TokKind::QuotedIdent;
        } else if c == '$' && pg {
            // 달러 인용 문자열($tag$...$tag$) 또는 $1 파라미터.
            let mut j = i + 1;
            if j < n && b[j].is_ascii_digit() {
                while j < n && b[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
                kind = TokKind::Param;
            } else {
                while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) {
                    j += 1;
                }
                if j < n && b[j] == b'$' {
                    let tag = &sql[i..=j];
                    match sql[j + 1..].find(tag) {
                        Some(p) => i = j + 1 + p + tag.len(),
                        None => i = n,
                    }
                    kind = TokKind::Str;
                } else {
                    i += 1;
                    kind = TokKind::Punct;
                }
            }
        } else if c == ';' {
            i += 1;
            kind = TokKind::Semicolon;
        } else if c.is_ascii_digit() {
            while i < n
                && (b[i].is_ascii_digit()
                    || b[i] == b'.'
                    || b[i] == b'e'
                    || b[i] == b'E'
                    || b[i] == b'x'
                    || (b[i].is_ascii_hexdigit() && sql[start..i].starts_with("0x")))
            {
                i += 1;
            }
            kind = TokKind::Number;
        } else if is_word_start(c) {
            while i < n && is_word_char(char_at(i)) {
                i += char_at(i).len_utf8();
            }
            kind = TokKind::Word;
        } else if (c == '?' || c == ':') && !pg {
            i += 1;
            kind = TokKind::Param;
        } else {
            i += c.len_utf8();
            kind = TokKind::Punct;
        }
        out.push(Token {
            kind,
            start,
            end: i,
        });
    }
    out
}

fn scan_quoted(b: &[u8], mut i: usize, q: u8, backslash: bool) -> usize {
    let n = b.len();
    i += 1;
    while i < n {
        if backslash && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            if b.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    n
}

fn word_eq(sql: &str, t: &Token, w: &str) -> bool {
    t.kind == TokKind::Word && sql[t.start..t.end].eq_ignore_ascii_case(w)
}

/// 문장 경계를 찾는다. 반환 범위는 앞뒤 공백을 제외하며 주석뿐인 문장은 제외한다.
pub fn split_statements(sql: &str, driver: Driver) -> Vec<Range<usize>> {
    let toks = tokenize(sql, driver);
    let mut out = Vec::new();
    let mut stmt_start = 0usize;
    let mut words: Vec<Token> = Vec::new();
    let mut block_mode = false;
    let mut depth = 0i32;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        match t.kind {
            TokKind::Word => {
                if words.len() < 6 {
                    words.push(t);
                    if words.len() >= 2 && word_eq(sql, &words[0], "create") {
                        block_mode = words.iter().skip(1).any(|w| {
                            word_eq(sql, w, "trigger")
                                || word_eq(sql, w, "procedure")
                                || word_eq(sql, w, "function")
                                || word_eq(sql, w, "event")
                        });
                    }
                }
                if block_mode {
                    if word_eq(sql, &t, "begin") || word_eq(sql, &t, "case") {
                        depth += 1;
                    } else if word_eq(sql, &t, "end") {
                        let next = toks[i + 1..]
                            .iter()
                            .find(|x| !matches!(x.kind, TokKind::Space | TokKind::Comment));
                        let closes_other = next.is_some_and(|x| {
                            ["if", "loop", "while", "repeat"]
                                .iter()
                                .any(|w| word_eq(sql, x, w))
                        });
                        if !closes_other {
                            depth -= 1;
                        }
                    }
                }
            }
            TokKind::Semicolon if depth <= 0 => {
                push_trimmed(sql, &toks, stmt_start, t.start, &mut out);
                stmt_start = t.end;
                words.clear();
                block_mode = false;
                depth = 0;
            }
            _ => {}
        }
        i += 1;
    }
    push_trimmed(sql, &toks, stmt_start, sql.len(), &mut out);
    out
}

fn push_trimmed(sql: &str, toks: &[Token], start: usize, end: usize, out: &mut Vec<Range<usize>>) {
    let has_code = toks.iter().any(|t| {
        t.start >= start && t.end <= end && !matches!(t.kind, TokKind::Space | TokKind::Comment)
    });
    if !has_code {
        return;
    }
    let slice = &sql[start..end];
    let lead = slice.len() - slice.trim_start().len();
    let trail = slice.len() - slice.trim_end().len();
    out.push(start + lead..end - trail);
}

/// Statement at a byte cursor. Leading indentation belongs to the following
/// statement on that line; a delimiter belongs to the preceding statement.
pub fn statement_at(sql: &str, driver: Driver, cursor: usize) -> Option<Range<usize>> {
    let cursor = cursor.min(sql.len());
    let stmts = split_statements(sql, driver);
    if let Some(r) = stmts.iter().find(|r| cursor >= r.start && cursor < r.end) {
        return Some(r.clone());
    }
    if let Some(r) = stmts
        .iter()
        .find(|r| cursor == r.end && sql.as_bytes().get(cursor) == Some(&b';'))
    {
        return Some(r.clone());
    }
    // Do not let an earlier delimiter capture the first character of the next query.
    let line_end = sql.as_bytes()[cursor..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(sql.len(), |n| cursor + n);
    if let Some(r) = stmts
        .iter()
        .find(|r| r.start >= cursor && r.start <= line_end)
    {
        return Some(r.clone());
    }
    stmts
        .iter()
        .rev()
        .find(|r| r.end <= cursor)
        .or_else(|| stmts.first())
        .cloned()
}

#[cfg(test)]
mod caret_execution_tests {
    use super::*;
    #[test]
    fn adjacent_statements_and_indentation_belong_to_the_current_query() {
        for driver in [Driver::Sqlite, Driver::Postgres, Driver::MySql] {
            let sql = "SELECT 1;SELECT 2;\n    SELECT '한글';";
            assert_eq!(&sql[statement_at(sql, driver, 8).unwrap()], "SELECT 1");
            assert_eq!(&sql[statement_at(sql, driver, 9).unwrap()], "SELECT 2");
            let indent = sql.find("    ").unwrap();
            assert_eq!(
                &sql[statement_at(sql, driver, indent).unwrap()],
                "SELECT '한글'"
            );
        }
    }
}

/// 주석을 건너뛴 첫 키워드(대문자).
pub fn first_keyword(sql: &str, driver: Driver) -> String {
    tokenize(sql, driver)
        .iter()
        .find(|t| !matches!(t.kind, TokKind::Space | TokKind::Comment))
        .filter(|t| t.kind == TokKind::Word)
        .map(|t| sql[t.start..t.end].to_ascii_uppercase())
        .unwrap_or_default()
}

/// 결과를 돌려줄 가능성이 있는 문장인지(SELECT/WITH/SHOW/EXPLAIN 등).
pub fn returns_rows(sql: &str, driver: Driver) -> bool {
    matches!(
        first_keyword(sql, driver).as_str(),
        "SELECT"
            | "WITH"
            | "SHOW"
            | "EXPLAIN"
            | "VALUES"
            | "TABLE"
            | "PRAGMA"
            | "DESCRIBE"
            | "DESC"
    ) || contains_returning(sql, driver)
}

fn contains_returning(sql: &str, driver: Driver) -> bool {
    tokenize(sql, driver)
        .iter()
        .any(|t| word_eq(sql, t, "returning"))
}

/// 최상위 SELECT 에 LIMIT 가 없으면 `LIMIT n` 을 붙인 문장을 돌려준다.
pub fn apply_auto_limit(stmt: &str, driver: Driver, n: usize) -> Option<String> {
    if first_keyword(stmt, driver) != "SELECT" {
        return None;
    }
    let toks = tokenize(stmt, driver);
    let mut depth = 0i32;
    for t in &toks {
        match t.kind {
            TokKind::Punct => match &stmt[t.start..t.end] {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            },
            TokKind::Word if depth == 0 => {
                let w = stmt[t.start..t.end].to_ascii_uppercase();
                if matches!(
                    w.as_str(),
                    "LIMIT" | "FETCH" | "INTO" | "FOR" | "OFFSET" | "TOP"
                ) {
                    return None;
                }
            }
            _ => {}
        }
    }
    let body = stmt.trim_end().trim_end_matches(';').trim_end();
    // 끝에 한 줄 주석이 있으면 줄을 바꿔서 붙인다.
    let last_is_line_comment = toks
        .iter()
        .rev()
        .find(|t| t.kind != TokKind::Space)
        .is_some_and(|t| t.kind == TokKind::Comment && !stmt[t.start..t.end].starts_with("/*"));
    let sep = if last_is_line_comment { "\n" } else { " " };
    Some(format!("{body}{sep}LIMIT {n}"))
}

/// 하이라이트용 SQL 키워드 목록(대문자).
pub const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "ATOMIC",
    "AUTOINCREMENT",
    "AUTO_INCREMENT",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BOOLEAN",
    "BY",
    "CASCADE",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "DATABASE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "ENUM",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FETCH",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LATERAL",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NATURAL",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRAGMA",
    "PRIMARY",
    "PROCEDURE",
    "RECURSIVE",
    "REFERENCES",
    "RENAME",
    "REPLACE",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SCHEMA",
    "SELECT",
    "SEQUENCE",
    "SET",
    "SHOW",
    "TABLE",
    "TEMP",
    "TEMPORARY",
    "TEXT",
    "THEN",
    "TO",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UNSIGNED",
    "UPDATE",
    "USE",
    "USING",
    "VALUES",
    "VARCHAR",
    "VIEW",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
    "WITHOUT",
];

/// 단어가 SQL 키워드인지(대소문자 무관).
pub fn is_keyword(word: &str) -> bool {
    if word.len() > 17 {
        return false;
    }
    let mut buf = [0u8; 17];
    let up = &mut buf[..word.len()];
    up.copy_from_slice(word.as_bytes());
    up.make_ascii_uppercase();
    let Ok(up) = std::str::from_utf8(up) else {
        return false;
    };
    KEYWORDS.binary_search(&up).is_ok()
}
