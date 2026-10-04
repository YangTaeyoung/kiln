//! 테이블 뷰용 SQL 생성: 페이지 조회, 개수, 변경 묶음(UPDATE/INSERT/DELETE).

use crate::driver::ChangeStmt;
use crate::meta::ColumnDef;
use crate::sql::{qualified, quote_ident};
use crate::{Driver, Value};

/// 테이블 참조.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TableRef {
    pub schema: Option<String>,
    pub table: String,
}

impl TableRef {
    pub fn new(schema: Option<String>, table: impl Into<String>) -> TableRef {
        TableRef {
            schema,
            table: table.into(),
        }
    }

    pub fn sql_name(&self, driver: Driver) -> String {
        qualified(driver, self.schema.as_deref(), &self.table)
    }
}

fn where_clause(filter: &str) -> String {
    let f = filter.trim().trim_end_matches(';').trim();
    if f.is_empty() {
        String::new()
    } else {
        format!(" WHERE {f}")
    }
}

fn order_clause(order: &str) -> String {
    let o = order.trim().trim_end_matches(';').trim();
    if o.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {o}")
    }
}

/// 필터·정렬이 붙은 전체 조회 문장(LIMIT 없음).
pub fn select_sql(driver: Driver, t: &TableRef, filter: &str, order: &str) -> String {
    format!(
        "SELECT * FROM {}{}{}",
        t.sql_name(driver),
        where_clause(filter),
        order_clause(order)
    )
}

/// 한 페이지 조회 문장.
pub fn page_sql(
    driver: Driver,
    t: &TableRef,
    filter: &str,
    order: &str,
    limit: usize,
    offset: usize,
) -> String {
    format!(
        "{} LIMIT {limit} OFFSET {offset}",
        select_sql(driver, t, filter, order)
    )
}

/// 필터가 적용된 전체 행 수 문장.
pub fn count_sql(driver: Driver, t: &TableRef, filter: &str) -> String {
    format!(
        "SELECT COUNT(*) FROM {}{}",
        t.sql_name(driver),
        where_clause(filter)
    )
}

/// 정렬 상태를 ORDER BY 식으로 만든다.
pub fn order_for(driver: Driver, column: &str, desc: bool) -> String {
    format!(
        "{} {}",
        quote_ident(driver, column),
        if desc { "DESC" } else { "ASC" }
    )
}

/// 기존 행 한 개의 변경.
#[derive(Clone, Debug, PartialEq)]
pub struct RowUpdate {
    /// PK 컬럼 인덱스와 원래 값.
    pub key: Vec<(usize, Value)>,
    /// 바뀐 컬럼 인덱스와 새 값.
    pub sets: Vec<(usize, Value)>,
}

/// 새 행. `None` 은 DEFAULT 로 둔다.
#[derive(Clone, Debug, PartialEq)]
pub struct RowInsert {
    pub values: Vec<Option<Value>>,
}

/// 제출할 변경 묶음.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    pub deletes: Vec<Vec<(usize, Value)>>,
    pub updates: Vec<RowUpdate>,
    pub inserts: Vec<RowInsert>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.deletes.is_empty() && self.updates.is_empty() && self.inserts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.deletes.len() + self.updates.len() + self.inserts.len()
    }
}

struct Params {
    driver: Driver,
    n: usize,
    args: Vec<Value>,
}

impl Params {
    fn push(&mut self, v: Value, col: &ColumnDef) -> String {
        self.n += 1;
        self.args.push(v);
        match self.driver {
            Driver::Postgres => {
                if col.cast_type.is_empty() {
                    format!("${}", self.n)
                } else {
                    format!("${}::{}", self.n, col.cast_type)
                }
            }
            _ => "?".to_string(),
        }
    }
}

fn key_where(driver: Driver, cols: &[ColumnDef], key: &[(usize, Value)], p: &mut Params) -> String {
    key.iter()
        .map(|(i, v)| {
            let c = &cols[*i];
            if v.is_null() {
                format!("{} IS NULL", quote_ident(driver, &c.name))
            } else {
                format!(
                    "{} = {}",
                    quote_ident(driver, &c.name),
                    p.push(v.clone(), c)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Read the original row using bound primary-key values, independent of page order.
pub(crate) fn select_key(driver:Driver,t:&TableRef,cols:&[ColumnDef],key:&[(usize,Value)])->ChangeStmt{
    let mut params=Params{driver,n:0,args:Vec::new()};
    let predicate=key_where(driver,cols,key,&mut params);
    ChangeStmt{sql:format!("SELECT * FROM {} WHERE {} LIMIT 2",t.sql_name(driver),if predicate.is_empty(){"1=0"}else{&predicate}),args:params.args,expect_one:false}
}

/// 변경 묶음을 파라미터 바인딩 문장 목록으로 만든다(DELETE → UPDATE → INSERT 순).
pub fn build_changes(
    driver: Driver,
    t: &TableRef,
    cols: &[ColumnDef],
    cs: &ChangeSet,
) -> Vec<ChangeStmt> {
    let name = t.sql_name(driver);
    let mut out = Vec::new();
    for key in &cs.deletes {
        let mut p = Params {
            driver,
            n: 0,
            args: Vec::new(),
        };
        let w = key_where(driver, cols, key, &mut p);
        out.push(ChangeStmt {
            sql: format!("DELETE FROM {name} WHERE {w}"),
            args: p.args,
            expect_one: true,
        });
    }
    for u in &cs.updates {
        if u.sets.is_empty() {
            continue;
        }
        let mut p = Params {
            driver,
            n: 0,
            args: Vec::new(),
        };
        let sets = u
            .sets
            .iter()
            .map(|(i, v)| {
                let c = &cols[*i];
                format!(
                    "{} = {}",
                    quote_ident(driver, &c.name),
                    p.push(v.clone(), c)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let w = key_where(driver, cols, &u.key, &mut p);
        out.push(ChangeStmt {
            sql: format!("UPDATE {name} SET {sets} WHERE {w}"),
            args: p.args,
            expect_one: true,
        });
    }
    for ins in &cs.inserts {
        let mut p = Params {
            driver,
            n: 0,
            args: Vec::new(),
        };
        let mut names = Vec::new();
        let mut vals = Vec::new();
        for (i, v) in ins.values.iter().enumerate() {
            if let (Some(v), Some(c)) = (v, cols.get(i)) {
                names.push(quote_ident(driver, &c.name));
                vals.push(p.push(v.clone(), c));
            }
        }
        let sql = if names.is_empty() {
            match driver {
                Driver::MySql | Driver::MariaDb => format!("INSERT INTO {name} () VALUES ()"),
                _ => format!("INSERT INTO {name} DEFAULT VALUES"),
            }
        } else {
            format!(
                "INSERT INTO {name} ({}) VALUES ({})",
                names.join(", "),
                vals.join(", ")
            )
        };
        out.push(ChangeStmt {
            sql,
            args: p.args,
            expect_one: true,
        });
    }
    out
}
