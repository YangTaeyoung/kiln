//! 드라이버별 연결·실행·디코딩. 공통 로직은 매크로로 드라이버마다 구체 타입 함수로 만든다.

use crate::config::{ConnConfig, Driver, SslMode};
use crate::value::{DISPLAY_MAX_CHARS, TypeClass, Value};
use futures_util::StreamExt;
use sqlx::decode::Decode;
use sqlx::error::BoxDynError;
use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
use sqlx::pool::PoolOptions;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Arguments, Column, Row, TypeInfo, ValueRef};
use sqlx::{MySql, MySqlConnection, PgConnection, Postgres, Sqlite, SqliteConnection};
use std::time::Duration;

/// 결과 컬럼 메타데이터.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub class: TypeClass,
}

impl ColumnInfo {
    pub fn new(name: impl Into<String>, type_name: impl Into<String>) -> ColumnInfo {
        let type_name = type_name.into();
        ColumnInfo {
            name: name.into(),
            class: TypeClass::from_type_name(&type_name),
            type_name,
        }
    }
}

/// 쿼리 결과. 표시 문자열은 결과가 도착할 때 한 번 만들어 둔다.
#[derive(Clone, Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Value>>,
    pub display: Vec<Vec<Box<str>>>,
}

impl ResultSet {
    pub fn new(columns: Vec<ColumnInfo>, rows: Vec<Vec<Value>>) -> ResultSet {
        let display = rows.iter().map(|r| display_row(r)).collect();
        ResultSet {
            columns,
            rows,
            display,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 선언 타입(컬럼 이름, 타입)을 반영해 컬럼 분류와 값, 표시 문자열을 고친다.
    pub fn apply_declared_types(&mut self, decl: &[(String, String)]) {
        for (ci, col) in self.columns.iter_mut().enumerate() {
            let Some((_, ty)) = decl.iter().find(|(n, _)| *n == col.name) else {
                continue;
            };
            let class = TypeClass::from_type_name(ty);
            col.type_name = ty.clone();
            if class == col.class {
                continue;
            }
            col.class = class;
            for (ri, row) in self.rows.iter_mut().enumerate() {
                let v = &mut row[ci];
                let nv = match (&*v, class) {
                    (Value::Int(i), TypeClass::Bool) if *i == 0 || *i == 1 => {
                        Some(Value::Bool(*i == 1))
                    }
                    (
                        Value::Text(s) | Value::Decimal(s) | Value::Other(s),
                        TypeClass::Json
                        | TypeClass::Date
                        | TypeClass::Time
                        | TypeClass::DateTime
                        | TypeClass::Timestamptz
                        | TypeClass::Uuid,
                    ) => Some(Value::from_text(s, class)),
                    _ => None,
                };
                if let Some(nv) = nv {
                    *v = nv;
                    self.display[ri][ci] = v.display(DISPLAY_MAX_CHARS).into_boxed_str();
                }
            }
        }
    }

    /// 첫 행 첫 열을 문자열로.
    pub fn scalar_string(&self) -> Option<String> {
        self.rows
            .first()
            .and_then(|r| r.first())
            .and_then(value_string)
    }
}

/// 행의 표시 문자열을 만든다.
pub fn display_row(r: &[Value]) -> Vec<Box<str>> {
    r.iter()
        .map(|v| v.display(DISPLAY_MAX_CHARS).into_boxed_str())
        .collect()
}

/// 카탈로그 조회용: 바이트도 UTF-8 로 해석해 문자열로 만든다.
pub fn value_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        other => other.to_text(),
    }
}

/// 스트리밍 행 콜백.
pub(crate) type RowSink<'a> =
    dyn FnMut(&[ColumnInfo], Vec<Value>) -> Result<(), String> + Send + 'a;

/// 한 문장 실행 결과.
#[derive(Clone, Debug, Default)]
pub struct StmtOutcome {
    pub result: ResultSet,
    /// 결과 집합(컬럼)이 있었는지.
    pub has_rows: bool,
    pub affected: u64,
    /// `max_rows` 에서 끊겼는지.
    pub truncated: bool,
}

/// 사용자에게 보여줄 DB 오류.
#[derive(Clone, Debug, PartialEq)]
pub struct DbError {
    pub message: String,
    pub code: Option<String>,
    /// 문장 안 위치(1부터 시작하는 문자 오프셋).
    pub position: Option<usize>,
}

impl DbError {
    pub fn msg(m: impl Into<String>) -> DbError {
        DbError {
            message: m.into(),
            code: None,
            position: None,
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(c) => write!(f, "[{c}] {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for DbError {}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> DbError {
        match &e {
            sqlx::Error::Database(d)
                if d.try_downcast_ref::<sqlx::sqlite::SqliteError>().is_some() =>
            {
                DbError::msg(d.message().to_string())
            }
            sqlx::Error::Database(d) => {
                let position = d
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .and_then(|pe| match pe.position() {
                        Some(sqlx::postgres::PgErrorPosition::Original(p)) => Some(p),
                        _ => None,
                    });
                DbError {
                    message: d.message().to_string(),
                    code: d.code().map(|c| c.into_owned()),
                    position,
                }
            }
            sqlx::Error::PoolTimedOut => DbError::msg("connection timed out"),
            other => DbError::msg(other.to_string()),
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;

/// 드라이버별 디코딩/바인딩 규칙.
pub(crate) trait DbKind: sqlx::Database {
    fn decode_value(row: &Self::Row, i: usize, class: TypeClass) -> Value;
    fn rows_affected(r: &Self::QueryResult) -> u64;
    fn add_arg(args: &mut Self::Arguments, v: &Value) -> Result<(), BoxDynError>;
}

impl DbKind for Postgres {
    fn decode_value(row: &Self::Row, i: usize, class: TypeClass) -> Value {
        let Ok(raw) = row.try_get_raw(i) else {
            return Value::Null;
        };
        if raw.is_null() {
            return Value::Null;
        }
        match raw.format() {
            sqlx::postgres::PgValueFormat::Text => match raw.as_str() {
                Ok(s) => Value::from_text(s, class),
                Err(_) => raw
                    .as_bytes()
                    .map(|b| Value::Bytes(b.to_vec()))
                    .unwrap_or(Value::Null),
            },
            sqlx::postgres::PgValueFormat::Binary => raw
                .as_bytes()
                .map(|b| Value::Bytes(b.to_vec()))
                .unwrap_or(Value::Null),
        }
    }

    fn rows_affected(r: &Self::QueryResult) -> u64 {
        r.rows_affected()
    }

    fn add_arg(args: &mut Self::Arguments, v: &Value) -> Result<(), BoxDynError> {
        let s: Option<String> = match v {
            Value::Null => None,
            Value::Bool(b) => Some(if *b { "true".into() } else { "false".into() }),
            Value::Bytes(b) => Some(format!("\\x{}", hex::encode(b))),
            other => other.to_text(),
        };
        args.add(s)
    }
}

impl DbKind for MySql {
    fn decode_value(row: &Self::Row, i: usize, class: TypeClass) -> Value {
        let Ok(raw) = row.try_get_raw(i) else {
            return Value::Null;
        };
        if raw.is_null() {
            return Value::Null;
        }
        if class == TypeClass::Bit {
            return <u64 as Decode<MySql>>::decode(raw)
                .map(Value::UInt)
                .unwrap_or(Value::Null);
        }
        match <&[u8] as Decode<MySql>>::decode(raw) {
            Ok(b) => Value::from_bytes(b, class),
            Err(_) => Value::Null,
        }
    }

    fn rows_affected(r: &Self::QueryResult) -> u64 {
        r.rows_affected()
    }

    fn add_arg(args: &mut Self::Arguments, v: &Value) -> Result<(), BoxDynError> {
        match v {
            Value::Null => args.add(None::<String>),
            Value::Bool(b) => args.add(*b as i64),
            Value::Int(i) => args.add(*i),
            Value::UInt(u) => args.add(*u),
            Value::Float(f) => args.add(*f),
            Value::Bytes(b) => args.add(b.clone()),
            other => args.add(other.to_text().unwrap_or_default()),
        }
    }
}

impl DbKind for Sqlite {
    fn decode_value(row: &Self::Row, i: usize, class: TypeClass) -> Value {
        let Ok(raw) = row.try_get_raw(i) else {
            return Value::Null;
        };
        if raw.is_null() {
            return Value::Null;
        }
        let storage = raw.type_info().name().to_ascii_uppercase();
        match storage.as_str() {
            "INTEGER" | "BOOLEAN" | "INT8" => match <i64 as Decode<Sqlite>>::decode(raw) {
                Ok(v) if class == TypeClass::Bool && (v == 0 || v == 1) => Value::Bool(v == 1),
                Ok(v) => Value::Int(v),
                Err(_) => Value::Null,
            },
            "REAL" | "NUMERIC" => match <f64 as Decode<Sqlite>>::decode(raw) {
                Ok(v) => Value::Float(v),
                Err(_) => Value::Null,
            },
            "BLOB" => match <Vec<u8> as Decode<Sqlite>>::decode(raw) {
                Ok(b) => Value::Bytes(b),
                Err(_) => Value::Null,
            },
            _ => match <String as Decode<Sqlite>>::decode(raw) {
                Ok(s) => match class {
                    TypeClass::Json
                    | TypeClass::Date
                    | TypeClass::Time
                    | TypeClass::DateTime
                    | TypeClass::Timestamptz
                    | TypeClass::Uuid
                    | TypeClass::Decimal => Value::from_text(&s, class),
                    _ => Value::Text(s),
                },
                Err(_) => Value::Null,
            },
        }
    }

    fn rows_affected(r: &Self::QueryResult) -> u64 {
        r.rows_affected()
    }

    fn add_arg(args: &mut Self::Arguments, v: &Value) -> Result<(), BoxDynError> {
        match v {
            Value::Null => args.add(None::<String>),
            Value::Bool(b) => args.add(*b as i64),
            Value::Int(i) => args.add(*i),
            Value::UInt(u) => match i64::try_from(*u) {
                Ok(i) => args.add(i),
                Err(_) => args.add(u.to_string()),
            },
            Value::Float(f) => args.add(*f),
            Value::Bytes(b) => args.add(b.clone()),
            other => args.add(other.to_text().unwrap_or_default()),
        }
    }
}

/// 트랜잭션 안에서 실행할 변경 문장 하나.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeStmt {
    pub sql: String,
    pub args: Vec<Value>,
    /// 영향받은 행이 정확히 1 이어야 하는지.
    pub expect_one: bool,
}

/// 변경 묶음 실행 실패 정보.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeError {
    /// 실패한 문장 인덱스.
    pub index: usize,
    pub error: DbError,
}

macro_rules! driver_ops {
    ($m:ident, $DB:ty, $Conn:ty, $drv:expr) => {
        pub(crate) mod $m {
            use super::*;

            fn columns_of(row: &<$DB as sqlx::Database>::Row) -> Vec<ColumnInfo> {
                row.columns()
                    .iter()
                    .map(|c| ColumnInfo::new(c.name(), c.type_info().name()))
                    .collect()
            }

            fn decode_row(row: &<$DB as sqlx::Database>::Row, cols: &[ColumnInfo]) -> Vec<Value> {
                (0..cols.len())
                    .map(|i| <$DB as DbKind>::decode_value(row, i, cols[i].class))
                    .collect()
            }

            /// 문장 하나를 텍스트 프로토콜로 실행하고 결과를 모은다.
            pub async fn run_sql(
                conn: &mut $Conn,
                sql: &str,
                max_rows: Option<usize>,
            ) -> DbResult<StmtOutcome> {
                let mut out = StmtOutcome::default();
                let mut cols: Option<Vec<ColumnInfo>> = None;
                let mut rows = Vec::new();
                {
                    let mut stream =
                        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).fetch_many(&mut *conn);
                    while let Some(item) = stream.next().await {
                        match item? {
                            sqlx::Either::Left(r) => {
                                out.affected += <$DB as DbKind>::rows_affected(&r);
                            }
                            sqlx::Either::Right(row) => {
                                let c = cols.get_or_insert_with(|| columns_of(&row));
                                if max_rows.is_some_and(|m| rows.len() >= m) {
                                    out.truncated = true;
                                    break;
                                }
                                rows.push(decode_row(&row, c));
                            }
                        }
                    }
                }
                if out.truncated {
                    // 끊은 스트림의 남은 응답을 소비해 연결을 재사용 가능 상태로 만든다.
                    let _ = sqlx::Connection::ping(&mut *conn).await;
                }
                if cols.is_none() && crate::sql::returns_rows(sql, $drv) {
                    use sqlx::{Executor as _, Statement as _};
                    if let Ok(d) = (&mut *conn)
                        .prepare(sqlx::SqlSafeStr::into_sql_str(sqlx::AssertSqlSafe(
                            sql.to_string(),
                        )))
                        .await
                    {
                        let c: Vec<ColumnInfo> = d
                            .columns()
                            .iter()
                            .map(|c| ColumnInfo::new(c.name(), c.type_info().name()))
                            .collect();
                        if !c.is_empty() {
                            cols = Some(c);
                        }
                    }
                }
                out.has_rows = cols.is_some();
                if out.has_rows {
                    out.affected = rows.len() as u64;
                }
                out.result = ResultSet::new(cols.unwrap_or_default(), rows);
                Ok(out)
            }

            /// 결과를 행 단위로 콜백에 흘려 보낸다. 콜백 오류는 중단 사유가 된다.
            pub async fn stream_sql(
                conn: &mut $Conn,
                sql: &str,
                sink: &mut RowSink<'_>,
            ) -> DbResult<u64> {
                let mut cols: Option<Vec<ColumnInfo>> = None;
                let mut n = 0u64;
                let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).fetch(&mut *conn);
                while let Some(row) = stream.next().await {
                    let row = row?;
                    let c = cols.get_or_insert_with(|| columns_of(&row));
                    let vals = decode_row(&row, c);
                    sink(c, vals).map_err(DbError::msg)?;
                    n += 1;
                }
                Ok(n)
            }

            /// 변경 문장들을 한 트랜잭션에서 실행한다. 하나라도 실패하면 롤백한다.
            pub async fn exec_changes(
                conn: &mut $Conn,
                stmts: &[ChangeStmt],
            ) -> Result<u64, ChangeError> {
                use sqlx::Connection;
                let mut tx = conn.begin().await.map_err(|e| ChangeError {
                    index: 0,
                    error: e.into(),
                })?;
                let mut total = 0;
                for (index, st) in stmts.iter().enumerate() {
                    let mut args = <$DB as sqlx::Database>::Arguments::default();
                    for v in &st.args {
                        <$DB as DbKind>::add_arg(&mut args, v).map_err(|e| ChangeError {
                            index,
                            error: DbError::msg(e.to_string()),
                        })?;
                    }
                    let res = sqlx::query_with(sqlx::AssertSqlSafe(st.sql.clone()), args)
                        .persistent(false)
                        .execute(&mut *tx)
                        .await
                        .map_err(|e| ChangeError {
                            index,
                            error: e.into(),
                        })?;
                    let n = <$DB as DbKind>::rows_affected(&res);
                    if st.expect_one && n != 1 {
                        return Err(ChangeError {
                            index,
                            error: DbError::msg(format!(
                                "expected 1 affected row, got {n} (row changed or deleted concurrently?)"
                            )),
                        });
                    }
                    total += n;
                }
                tx.commit().await.map_err(|e| ChangeError {
                    index: stmts.len(),
                    error: e.into(),
                })?;
                Ok(total)
            }
        }
    };
}

driver_ops!(pg, Postgres, PgConnection, Driver::Postgres);
driver_ops!(my, MySql, MySqlConnection, Driver::MySql);
driver_ops!(lite, Sqlite, SqliteConnection, Driver::Sqlite);

/// 드라이버별 풀. 복제 비용이 작다.
#[derive(Clone, Debug)]
pub(crate) enum DbPool {
    Pg(sqlx::PgPool),
    My(sqlx::MySqlPool),
    Lite(sqlx::SqlitePool),
}

/// 콘솔 세션이 붙잡는 단일 연결.
pub(crate) enum SessionConn {
    Pg(PgConnection),
    My(MySqlConnection),
    Lite(SqliteConnection),
}

impl DbPool {
    pub(crate) async fn run_sql(
        &self,
        sql: &str,
        max_rows: Option<usize>,
    ) -> DbResult<StmtOutcome> {
        match self {
            DbPool::Pg(p) => pg::run_sql(&mut *p.acquire().await?, sql, max_rows).await,
            DbPool::My(p) => my::run_sql(&mut *p.acquire().await?, sql, max_rows).await,
            DbPool::Lite(p) => lite::run_sql(&mut *p.acquire().await?, sql, max_rows).await,
        }
    }

    pub(crate) async fn query(&self, sql: &str) -> DbResult<ResultSet> {
        Ok(self.run_sql(sql, None).await?.result)
    }

    pub(crate) async fn stream_sql(&self, sql: &str, sink: &mut RowSink<'_>) -> DbResult<u64> {
        match self {
            DbPool::Pg(p) => pg::stream_sql(&mut *p.acquire().await?, sql, sink).await,
            DbPool::My(p) => my::stream_sql(&mut *p.acquire().await?, sql, sink).await,
            DbPool::Lite(p) => lite::stream_sql(&mut *p.acquire().await?, sql, sink).await,
        }
    }

    pub(crate) async fn exec_changes(&self, stmts: &[ChangeStmt]) -> Result<u64, ChangeError> {
        let conv = |e: sqlx::Error| ChangeError {
            index: 0,
            error: e.into(),
        };
        match self {
            DbPool::Pg(p) => pg::exec_changes(&mut *p.acquire().await.map_err(conv)?, stmts).await,
            DbPool::My(p) => my::exec_changes(&mut *p.acquire().await.map_err(conv)?, stmts).await,
            DbPool::Lite(p) => {
                lite::exec_changes(&mut *p.acquire().await.map_err(conv)?, stmts).await
            }
        }
    }

    /// 풀에서 연결 하나를 떼어 세션 전용으로 만든다.
    pub(crate) async fn detach_session(&self) -> DbResult<SessionConn> {
        Ok(match self {
            DbPool::Pg(p) => SessionConn::Pg(p.acquire().await?.detach()),
            DbPool::My(p) => SessionConn::My(p.acquire().await?.detach()),
            DbPool::Lite(p) => SessionConn::Lite(p.acquire().await?.detach()),
        })
    }

    pub(crate) async fn close(&self) {
        match self {
            DbPool::Pg(p) => p.close().await,
            DbPool::My(p) => p.close().await,
            DbPool::Lite(p) => p.close().await,
        }
    }
}

impl SessionConn {
    pub(crate) async fn run_sql(
        &mut self,
        sql: &str,
        max_rows: Option<usize>,
    ) -> DbResult<StmtOutcome> {
        match self {
            SessionConn::Pg(c) => pg::run_sql(c, sql, max_rows).await,
            SessionConn::My(c) => my::run_sql(c, sql, max_rows).await,
            SessionConn::Lite(c) => lite::run_sql(c, sql, max_rows).await,
        }
    }

    /// 서버 쪽 세션 ID(pg backend pid, mysql connection id).
    pub(crate) async fn backend_id(&mut self) -> Option<i64> {
        let sql = match self {
            SessionConn::Pg(_) => "SELECT pg_backend_pid()",
            SessionConn::My(_) => "SELECT CONNECTION_ID()",
            SessionConn::Lite(_) => return None,
        };
        let out = self.run_sql(sql, Some(1)).await.ok()?;
        out.result.scalar_string()?.parse().ok()
    }
}

/// 설정과 비밀번호로 풀을 만든다. 연결 제한 시간을 넘기면 실패한다.
pub(crate) async fn connect_pool(cfg: &ConnConfig, password: Option<&str>) -> DbResult<DbPool> {
    let timeout = Duration::from_secs(cfg.connect_timeout_secs.max(1) as u64);
    let fut = async {
        Ok::<_, DbError>(match cfg.driver {
            Driver::Postgres => DbPool::Pg(
                PoolOptions::<Postgres>::new()
                    .max_connections(6)
                    .acquire_timeout(timeout)
                    .connect_with(pg_options(cfg, password))
                    .await?,
            ),
            Driver::MySql | Driver::MariaDb => DbPool::My(
                PoolOptions::<MySql>::new()
                    .max_connections(6)
                    .acquire_timeout(timeout)
                    .connect_with(my_options(cfg, password))
                    .await?,
            ),
            Driver::Sqlite => {
                if cfg.file.trim().is_empty() {
                    return Err(DbError::msg("SQLite file path is empty"));
                }
                DbPool::Lite(
                    PoolOptions::<Sqlite>::new()
                        .max_connections(4)
                        .acquire_timeout(timeout)
                        .connect_with(lite_options(cfg))
                        .await?,
                )
            }
        })
    };
    match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(DbError::msg(format!(
            "connect timed out after {}s",
            timeout.as_secs()
        ))),
    }
}

/// 단일 연결로 접속을 시험하고 서버 버전을 돌려준다.
pub(crate) async fn test_connection(cfg: &ConnConfig, password: Option<&str>) -> DbResult<String> {
    let pool = connect_pool(cfg, password).await?;
    let sql = match cfg.driver {
        Driver::Postgres => "SELECT version()",
        Driver::MySql | Driver::MariaDb => "SELECT VERSION()",
        Driver::Sqlite => "SELECT 'SQLite ' || sqlite_version()",
    };
    let v = pool.query(sql).await?.scalar_string().unwrap_or_default();
    pool.close().await;
    Ok(v)
}

fn pg_options(cfg: &ConnConfig, password: Option<&str>) -> PgConnectOptions {
    let mut o = PgConnectOptions::new_without_pgpass()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.user)
        .application_name("kiln")
        .ssl_mode(match cfg.ssl_mode {
            SslMode::Disable => PgSslMode::Disable,
            SslMode::Prefer => PgSslMode::Prefer,
            SslMode::Require => PgSslMode::Require,
            SslMode::VerifyCa => PgSslMode::VerifyCa,
            SslMode::VerifyFull => PgSslMode::VerifyFull,
        });
    if let Some(p) = password {
        o = o.password(p);
    }
    if !cfg.database.is_empty() {
        o = o.database(&cfg.database);
    }
    o
}

fn my_options(cfg: &ConnConfig, password: Option<&str>) -> MySqlConnectOptions {
    let mut o = MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.user)
        .ssl_mode(match cfg.ssl_mode {
            SslMode::Disable => MySqlSslMode::Disabled,
            SslMode::Prefer => MySqlSslMode::Preferred,
            SslMode::Require => MySqlSslMode::Required,
            SslMode::VerifyCa => MySqlSslMode::VerifyCa,
            SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
        });
    if let Some(p) = password {
        o = o.password(p);
    }
    if !cfg.database.is_empty() {
        o = o.database(&cfg.database);
    }
    o
}

fn lite_options(cfg: &ConnConfig) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(&cfg.file)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5))
}
