//! 연결 설정·풀·비동기 런타임·쿼리 기록을 소유하는 공유 관리자.

use crate::config::{Secrets, StorePaths};
use crate::driver::{
    self, ChangeError, DbError, DbPool, DbResult, ResultSet, SessionConn, StmtOutcome,
};
use crate::edit::{ChangeSet, TableRef, build_changes, count_sql, page_sql};
use crate::export::{ExportFormat, ExportWriter};
use crate::meta::{self, ColumnDef, TableDetails, TableInfo, TableKind};
use crate::{ConnConfig, ConnId, Driver};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// 연결 상태.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// 콘솔 쿼리 기록 항목.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub sql: String,
    /// UNIX 초.
    pub at: i64,
    pub elapsed_ms: u64,
    pub ok: bool,
}

const HISTORY_CAP: usize = 300;

#[derive(Default, Serialize, Deserialize)]
struct ConfigFile {
    connections: Vec<ConnConfig>,
}

#[derive(Default)]
struct Live {
    status: Option<ConnStatus>,
    pool: Option<Arc<DbPool>>,
    lock: Arc<tokio::sync::Mutex<()>>,
}

struct Inner {
    rt: Option<tokio::runtime::Runtime>,
    paths: Option<StorePaths>,
    configs: RwLock<Vec<ConnConfig>>,
    secrets: Secrets,
    live: Mutex<HashMap<ConnId, Live>>,
    history: Mutex<HashMap<ConnId, Vec<HistoryEntry>>>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
    revision: AtomicU64,
    connection_epochs: Mutex<HashMap<ConnId, u64>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// 패널과 탭이 공유하는 DB 관리자. 복제해도 같은 상태를 가리킨다.
#[derive(Clone)]
pub struct DbManager {
    inner: Arc<Inner>,
}

/// 백그라운드 작업 핸들. 완료되면 등록된 egui 컨텍스트에 리페인트를 요청한다.
pub struct Job<T> {
    rx: tokio::sync::oneshot::Receiver<T>,
    abort: tokio::task::AbortHandle,
    started: Instant,
    done: bool,
}

impl<T> Job<T> {
    /// 결과가 도착했으면 꺼낸다(한 번만).
    pub fn poll(&mut self) -> Option<T> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(v) => {
                self.done = true;
                Some(v)
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.done = true;
                None
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => None,
        }
    }

    pub fn is_running(&self) -> bool {
        !self.done && !self.abort.is_finished()
    }

    pub fn abort(&mut self) {
        self.abort.abort();
        self.done = true;
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// 완료까지 블로킹한다(테스트용).
    pub fn wait(self) -> Option<T> {
        if self.done {
            return None;
        }
        self.rx.blocking_recv().ok()
    }
}

impl DbManager {
    /// 설정 디렉토리의 `db_connections.json` 과 OS 키체인을 쓰는 관리자.
    /// `KILN_DB_NO_KEYCHAIN` 환경변수가 있으면 키체인 대신 메모리를 쓴다.
    pub fn load() -> DbManager {
        let use_keychain = std::env::var_os("KILN_DB_NO_KEYCHAIN").is_none();
        DbManager::with_store(
            Some(kiln_common::paths::config_file("db_connections.json")),
            use_keychain,
        )
    }

    /// 저장하지 않고 비밀번호도 메모리에만 두는 관리자.
    pub fn in_memory() -> DbManager {
        DbManager::with_store(None, false)
    }

    /// 지정 설정 파일을 쓰는 관리자. 기록 파일은 같은 디렉토리의 `db_history.json`.
    pub fn with_store(config_path: Option<PathBuf>, use_keychain: bool) -> DbManager {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("kiln-db")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let paths = config_path.map(|p| StorePaths {
            history: p.with_file_name("db_history.json"),
            connections: p,
        });
        let configs = paths
            .as_ref()
            .map(|p| kiln_common::store::load_json::<ConfigFile>(&p.connections).connections)
            .unwrap_or_default();
        let history: HashMap<String, Vec<HistoryEntry>> = paths
            .as_ref()
            .map(|p| kiln_common::store::load_json(&p.history))
            .unwrap_or_default();
        let history = history
            .into_iter()
            .filter_map(|(k, v)| k.parse().ok().map(|id| (ConnId(id), v)))
            .collect();
        DbManager {
            inner: Arc::new(Inner {
                rt: Some(rt),
                paths,
                configs: RwLock::new(configs),
                secrets: Secrets::new(use_keychain),
                live: Mutex::new(HashMap::new()),
                history: Mutex::new(history),
                ctx: Arc::new(Mutex::new(None)),
                revision: AtomicU64::new(1),
                connection_epochs: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn rt(&self) -> &tokio::runtime::Runtime {
        self.inner.rt.as_ref().expect("runtime alive")
    }

    /// 작업 완료 시 리페인트할 egui 컨텍스트를 등록한다.
    pub fn set_ctx(&self, ctx: &egui::Context) {
        let mut g = self.inner.ctx.lock();
        if g.is_none() {
            *g = Some(ctx.clone());
        }
    }

    fn repaint(&self) {
        if let Some(c) = self.inner.ctx.lock().as_ref() {
            c.request_repaint();
        }
    }

    /// 설정·상태가 바뀔 때마다 증가하는 번호.
    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.inner.revision.fetch_add(1, Ordering::Relaxed);
        self.repaint();
    }

    /// 퓨처를 런타임에서 실행하고 작업 핸들을 돌려준다.
    pub fn spawn<T: Send + 'static>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
    ) -> Job<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ctx = self.inner.ctx.clone();
        let handle = self.rt().spawn(async move {
            let v = fut.await;
            let _ = tx.send(v);
            if let Some(c) = ctx.lock().as_ref() {
                c.request_repaint();
            }
        });
        Job {
            rx,
            abort: handle.abort_handle(),
            started: Instant::now(),
            done: false,
        }
    }

    /// 퓨처를 현재 스레드에서 완료까지 실행한다(테스트·동기 호출용).
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.rt().block_on(fut)
    }

    // ---------- 설정 CRUD ----------

    pub fn connections(&self) -> Vec<ConnConfig> {
        self.inner.configs.read().clone()
    }

    pub fn get(&self, id: ConnId) -> Option<ConnConfig> {
        self.inner
            .configs
            .read()
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// 키체인을 쓸 수 없을 때의 경고.
    pub fn keychain_warning(&self) -> Option<String> {
        self.inner.secrets.warning()
    }

    /// 연결을 추가한다. `id` 가 0 이면 새 ID 를 만든다.
    pub fn add(&self, mut cfg: ConnConfig, password: Option<String>) -> ConnId {
        if cfg.id.0 == 0 {
            cfg.id = ConnId::new_unique();
        }
        let id = cfg.id;
        self.store_password(&mut cfg, password);
        self.inner.configs.write().push(cfg);
        self.save();
        self.bump();
        id
    }

    /// 설정을 바꾼다. `password` 가 `Some` 이면 비밀번호도 바꾼다. 열린 풀은 닫는다.
    pub fn update(&self, mut cfg: ConnConfig, password: Option<String>) {
        let id = cfg.id;
        let old_pw = self.password(id);
        self.store_password(&mut cfg, password.or(old_pw));
        {
            let mut g = self.inner.configs.write();
            match g.iter_mut().find(|c| c.id == id) {
                Some(c) => *c = cfg,
                None => g.push(cfg),
            }
        }
        self.disconnect(id);
        self.save();
        self.bump();
    }

    pub fn remove(&self, id: ConnId) {
        self.disconnect(id);
        self.inner.configs.write().retain(|c| c.id != id);
        self.inner.secrets.delete(id);
        self.inner.history.lock().remove(&id);
        self.save();
        self.bump();
    }

    /// URL 로 연결을 만들어 추가한다.
    pub fn import_url(&self, url: &str) -> Result<ConnId, String> {
        let (cfg, pw) = ConnConfig::from_url(url)?;
        Ok(self.add(cfg, pw))
    }

    fn store_password(&self, cfg: &mut ConnConfig, password: Option<String>) {
        if cfg.save_password_in_file {
            if password.is_some() {
                cfg.password_in_file = password;
            }
            self.inner.secrets.delete(cfg.id);
        } else {
            cfg.password_in_file = None;
            match password {
                Some(p) if !p.is_empty() => self.inner.secrets.set(cfg.id, &p),
                Some(_) => self.inner.secrets.delete(cfg.id),
                None => {}
            }
        }
    }

    /// 저장된 비밀번호.
    pub fn password(&self, id: ConnId) -> Option<String> {
        let cfg = self.get(id)?;
        if cfg.save_password_in_file {
            cfg.password_in_file
        } else {
            self.inner.secrets.get(id)
        }
    }

    fn save(&self) {
        let Some(paths) = &self.inner.paths else {
            return;
        };
        let file = ConfigFile {
            connections: self
                .inner
                .configs
                .read()
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    if !c.save_password_in_file {
                        c.password_in_file = None;
                    }
                    c
                })
                .collect(),
        };
        if let Err(e) = kiln_common::store::save_json(&paths.connections, &file) {
            log::error!("failed to save db connections: {e}");
        }
    }

    // ---------- 연결 ----------

    pub fn status(&self, id: ConnId) -> ConnStatus {
        self.inner
            .live
            .lock()
            .get(&id)
            .and_then(|l| l.status.clone())
            .unwrap_or(ConnStatus::Disconnected)
    }

    fn set_status(&self, id: ConnId, st: ConnStatus) {
        self.inner.live.lock().entry(id).or_default().status = Some(st);
        self.bump();
    }

    pub fn driver(&self, id: ConnId) -> Option<Driver> {
        self.get(id).map(|c| c.driver)
    }

    /// 풀을 돌려준다. 없으면 접속한다(동시 호출은 한 번만 접속).
    pub(crate) async fn pool(&self, id: ConnId) -> DbResult<Arc<DbPool>> {
        let lock = {
            let mut g = self.inner.live.lock();
            let l = g.entry(id).or_default();
            if let Some(p) = &l.pool {
                return Ok(p.clone());
            }
            l.lock.clone()
        };
        let _guard = lock.lock().await;
        if let Some(p) = self.inner.live.lock().get(&id).and_then(|l| l.pool.clone()) {
            return Ok(p);
        }
        let cfg = self
            .get(id)
            .ok_or_else(|| DbError::msg(kiln_common::i18n::tr("연결을 찾을 수 없습니다")))?;
        self.set_status(id, ConnStatus::Connecting);
        let pw = self.password(id);
        match driver::connect_pool(&cfg, pw.as_deref()).await {
            Ok(p) => {
                let p = Arc::new(p);
                {
                    let mut g = self.inner.live.lock();
                    let l = g.entry(id).or_default();
                    l.pool = Some(p.clone());
                    l.status = Some(ConnStatus::Connected);
                }
                self.bump();
                Ok(p)
            }
            Err(e) => {
                self.set_status(id, ConnStatus::Failed(e.to_string()));
                Err(e)
            }
        }
    }

    /// 접속한다.
    pub async fn connect(&self, id: ConnId) -> DbResult<()> {
        self.pool(id).await.map(|_| ())
    }

    /// 풀을 닫고 상태를 초기화한다.
    pub fn disconnect(&self, id: ConnId) {
        *self.inner.connection_epochs.lock().entry(id).or_default() += 1;
        let pool = {
            let mut g = self.inner.live.lock();
            g.remove(&id).and_then(|l| l.pool)
        };
        if let Some(p) = pool {
            self.rt().spawn(async move { p.close().await });
        }
        self.bump();
    }

    /// Snapshot only the existing pool. Read-only assistance must never reconnect.
    pub(crate) fn connected_pool(&self, id: ConnId) -> Option<Arc<DbPool>> {
        self.inner.live.lock().get(&id).and_then(|l| l.pool.clone())
    }

    pub(crate) fn schema_changed(&self, id: ConnId) {
        *self.inner.connection_epochs.lock().entry(id).or_default() += 1;
        self.bump();
    }

    /// Invalidates asynchronous metadata when this connection is replaced or closed.
    pub(crate) fn connection_epoch(&self, id: ConnId) -> u64 {
        self.inner
            .connection_epochs
            .lock()
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    /// 저장하지 않은 설정으로 접속을 시험하고 서버 버전을 돌려준다.
    pub async fn test_connection(
        &self,
        cfg: ConnConfig,
        password: Option<String>,
    ) -> DbResult<String> {
        driver::test_connection(&cfg, password.as_deref()).await
    }

    // ---------- 탐색 ----------

    pub async fn list_schemas(&self, id: ConnId) -> DbResult<Vec<String>> {
        let d = self.driver_or_err(id)?;
        meta::list_schemas(&*self.pool(id).await?, d).await
    }

    pub async fn list_tables(&self, id: ConnId, schema: &str) -> DbResult<Vec<TableInfo>> {
        let d = self.driver_or_err(id)?;
        meta::list_tables(&*self.pool(id).await?, d, schema).await
    }

    pub async fn table_details(&self, id: ConnId, t: &TableRef) -> DbResult<TableDetails> {
        let d = self.driver_or_err(id)?;
        meta::table_details(&*self.pool(id).await?, d, t.schema.as_deref(), &t.table).await
    }

    pub async fn table_ddl(&self, id: ConnId, t: &TableRef) -> DbResult<String> {
        let d = self.driver_or_err(id)?;
        meta::table_ddl(&*self.pool(id).await?, d, t.schema.as_deref(), &t.table).await
    }

    fn driver_or_err(&self, id: ConnId) -> DbResult<Driver> {
        self.driver(id)
            .ok_or_else(|| DbError::msg(kiln_common::i18n::tr("연결을 찾을 수 없습니다")))
    }

    // ---------- 데이터 ----------

    /// 풀 연결에서 문장 하나를 실행한다.
    pub async fn query(
        &self,
        id: ConnId,
        sql: &str,
        max_rows: Option<usize>,
    ) -> DbResult<StmtOutcome> {
        self.pool(id).await?.run_sql(sql, max_rows).await
    }

    /// 테이블 한 페이지를 읽는다.
    pub async fn fetch_page(
        &self,
        id: ConnId,
        t: &TableRef,
        filter: &str,
        order: &str,
        limit: usize,
        offset: usize,
    ) -> DbResult<ResultSet> {
        let d = self.driver_or_err(id)?;
        let sql = page_sql(d, t, filter, order, limit, offset);
        let mut rs = self.query(id, &sql, None).await?.result;
        if d == Driver::Sqlite {
            let decl = self
                .query(
                    id,
                    &format!(
                        "SELECT name, type FROM pragma_table_info({}, {})",
                        crate::sql::quote_literal(d, &t.table),
                        crate::sql::quote_literal(d, t.schema.as_deref().unwrap_or("main"))
                    ),
                    None,
                )
                .await?
                .result;
            let decl: Vec<(String, String)> = decl
                .rows
                .iter()
                .map(|r| {
                    (
                        driver::value_string(&r[0]).unwrap_or_default(),
                        driver::value_string(&r[1]).unwrap_or_default(),
                    )
                })
                .collect();
            rs.apply_declared_types(&decl);
        }
        Ok(rs)
    }

    pub(crate) async fn fetch_original_rows(
        &self,
        id: ConnId,
        t: &TableRef,
        columns: &[ColumnDef],
        keys: &[Vec<(usize, crate::Value)>],
    ) -> DbResult<ResultSet> {
        let driver = self.driver_or_err(id)?;
        let pool = self.pool(id).await?;
        let mut all = ResultSet::default();
        let empty = vec![Vec::new()];
        for key in if keys.is_empty() { &empty[..] } else { keys } {
            let statement = crate::edit::select_key(driver, t, columns, key);
            let result = pool.query_bound(&statement.sql, &statement.args).await?;
            all.columns = result.columns;
            all.rows.extend(result.rows);
        }
        Ok(ResultSet::new(all.columns, all.rows))
    }

    /// 필터가 적용된 전체 행 수.
    pub async fn count_rows(&self, id: ConnId, t: &TableRef, filter: &str) -> DbResult<i64> {
        let d = self.driver_or_err(id)?;
        let rs = self.query(id, &count_sql(d, t, filter), None).await?.result;
        rs.scalar_string()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                DbError::msg(kiln_common::i18n::tr("COUNT(*)가 값을 반환하지 않았습니다"))
            })
    }

    /// 변경 묶음을 한 트랜잭션으로 제출한다. 반환값은 영향받은 행 수.
    pub async fn submit_changes(
        &self,
        id: ConnId,
        t: &TableRef,
        columns: &[ColumnDef],
        changes: &ChangeSet,
    ) -> Result<u64, ChangeError> {
        let conv = |e: DbError| ChangeError { index: 0, error: e };
        let d = self.driver_or_err(id).map_err(conv)?;
        let stmts = build_changes(d, t, columns, changes);
        self.pool(id)
            .await
            .map_err(conv)?
            .exec_changes(&stmts)
            .await
    }

    /// 쿼리 결과 전체를 파일로 스트리밍한다. 반환값은 행 수.
    pub async fn export_query(
        &self,
        id: ConnId,
        sql: &str,
        path: &Path,
        fmt: ExportFormat,
    ) -> DbResult<u64> {
        let pool = self.pool(id).await?;
        let mut w = ExportWriter::create(path, fmt).map_err(|e| DbError::msg(e.to_string()))?;
        let mut sink = |cols: &[crate::ColumnInfo], row: Vec<crate::Value>| {
            w.write_row(cols, &row).map_err(|e| e.to_string())
        };
        pool.stream_sql(sql, &mut sink).await?;
        w.finish().map_err(|e| DbError::msg(e.to_string()))
    }

    /// 테이블의 모든 행을 지운다(pg/mysql TRUNCATE, sqlite DELETE).
    pub async fn truncate_table(&self, id: ConnId, t: &TableRef) -> DbResult<()> {
        let d = self.driver_or_err(id)?;
        let sql = match d {
            Driver::Sqlite => format!("DELETE FROM {}", t.sql_name(d)),
            _ => format!("TRUNCATE TABLE {}", t.sql_name(d)),
        };
        self.query(id, &sql, None).await.map(|_| ())
    }

    /// 테이블이나 뷰를 삭제한다.
    pub async fn drop_table(&self, id: ConnId, t: &TableRef, kind: TableKind) -> DbResult<()> {
        let d = self.driver_or_err(id)?;
        let what = match kind {
            TableKind::Table => "TABLE",
            TableKind::View => "VIEW",
            TableKind::MaterializedView => "MATERIALIZED VIEW",
        };
        self.query(id, &format!("DROP {what} {}", t.sql_name(d)), None)
            .await
            .map(|_| ())
    }

    // ---------- 콘솔 세션 ----------

    /// 콘솔 전용 연결을 연다.
    pub async fn open_session(&self, id: ConnId) -> DbResult<ConsoleSession> {
        let pool = self.pool(id).await?;
        let mut conn = pool.detach_session().await?;
        let backend = conn.backend_id().await;
        Ok(ConsoleSession {
            conn: Arc::new(tokio::sync::Mutex::new(Some(conn))),
            backend: Arc::new(Mutex::new(backend)),
            pool,
            driver: self.driver_or_err(id)?,
            identity: NEXT_CONSOLE_ID.fetch_add(1, Ordering::Relaxed),
            generation: Arc::new(AtomicU64::new(1)),
        })
    }

    // ---------- 기록 ----------

    pub fn history(&self, id: ConnId) -> Vec<HistoryEntry> {
        self.inner
            .history
            .lock()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn push_history(&self, id: ConnId, entry: HistoryEntry) {
        {
            let mut g = self.inner.history.lock();
            let v = g.entry(id).or_default();
            v.retain(|e| e.sql != entry.sql);
            v.insert(0, entry);
            v.truncate(HISTORY_CAP);
        }
        if let Some(paths) = &self.inner.paths {
            let snapshot: HashMap<String, Vec<HistoryEntry>> = self
                .inner
                .history
                .lock()
                .iter()
                .map(|(k, v)| (k.0.to_string(), v.clone()))
                .collect();
            if let Err(e) = kiln_common::store::save_json(&paths.history, &snapshot) {
                log::error!("failed to save db history: {e}");
            }
        }
    }
}

/// 콘솔 탭 하나가 쓰는 전용 연결. 세션 상태(SET, BEGIN 등)가 유지된다.
static NEXT_CONSOLE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct ConsoleSession {
    conn: Arc<tokio::sync::Mutex<Option<SessionConn>>>,
    backend: Arc<Mutex<Option<i64>>>,
    pool: Arc<DbPool>,
    driver: Driver,
    identity: u64,
    generation: Arc<AtomicU64>,
}

impl ConsoleSession {
    /// 문장 하나를 실행한다. 실행 도중 작업이 중단되면 연결을 버리고, 다음 실행 때 새로 연다.
    pub async fn run(&self, sql: &str, max_rows: Option<usize>) -> DbResult<StmtOutcome> {
        let mut guard = self.conn.lock().await;
        let mut conn = match guard.take() {
            Some(c) => c,
            None => {
                let mut c = self.pool.detach_session().await?;
                self.generation.fetch_add(1, Ordering::Relaxed);
                *self.backend.lock() = c.backend_id().await;
                c
            }
        };
        let r = conn.run_sql(sql, max_rows).await;
        *guard = Some(conn);
        r
    }

    /// Executes normally, then builds a bounded read-only provenance proof on
    /// this exact session. Failed proof never changes the SELECT result.
    pub async fn run_editable(
        &self,
        sql: &str,
        max_rows: Option<usize>,
    ) -> DbResult<crate::result_edit::EditableOutcome> {
        let mut guard = self.conn.lock().await;
        let mut conn = match guard.take() {
            Some(c) => c,
            None => {
                let mut c = self.pool.detach_session().await?;
                self.generation.fetch_add(1, Ordering::Relaxed);
                *self.backend.lock() = c.backend_id().await;
                c
            }
        };
        let before = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::result_edit::capture(&mut conn, self.driver, sql),
        )
        .await
        .unwrap_or(Err(
            crate::result_edit::ResultReadOnlyReason::MetadataUnavailable,
        ));
        let result = conn.run_sql_with_origins(sql, max_rows).await;
        let result = match result {
            Ok((outcome, origins)) => {
                let editing = if outcome.has_rows {
                    match before {
                        Err(reason) => Err(reason),
                        Ok(before) => tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            crate::result_edit::prepare(
                                &mut conn,
                                self.driver,
                                sql,
                                &outcome.result,
                                self.identity,
                                self.generation.load(Ordering::Relaxed),
                                &before,
                                &origins,
                            ),
                        )
                        .await
                        .unwrap_or(Err(
                            crate::result_edit::ResultReadOnlyReason::MetadataUnavailable,
                        )),
                    }
                } else {
                    Err(crate::result_edit::ResultReadOnlyReason::UnsupportedQuery)
                };
                Ok(crate::result_edit::EditableOutcome { outcome, editing })
            }
            Err(error) => Err(error),
        };
        *guard = Some(conn);
        result
    }

    /// Commits only the verified permanent-table changes in an independent
    /// pooled transaction. Never commits the console's explicit BEGIN.
    pub async fn submit_result_edits(
        &self,
        plan: &crate::result_edit::ResultEditPlan,
        cells: &[crate::result_edit::ResultEditCell],
    ) -> Result<u64, ChangeError> {
        let invalid = || ChangeError {
            index: 0,
            error: DbError::msg(kiln_common::i18n::tr(
                "조회 세션이 변경되어 결과를 다시 실행해야 합니다",
            )),
        };
        let guard = self.conn.lock().await;
        if guard.is_none()
            || plan.session != self.identity
            || plan.generation != self.generation.load(Ordering::Relaxed)
        {
            return Err(invalid());
        }
        let statements = plan
            .changes(cells)
            .map_err(|error| ChangeError { index: 0, error })?;
        plan.submit(&self.pool, &statements).await
    }

    /// Refresh is separate from Submit: an error here means the preceding
    /// successful write remains committed. The caller must invalidate its old
    /// plan before refreshing, and report a refresh failure without resubmitting.
    pub async fn refresh_result_edit(
        &self,
        plan: &crate::result_edit::ResultEditPlan,
        max_rows: Option<usize>,
    ) -> DbResult<crate::result_edit::EditableOutcome> {
        if plan.session != self.identity
            || plan.generation != self.generation.load(Ordering::Relaxed)
        {
            return Err(DbError::msg(kiln_common::i18n::tr(
                "조회 세션이 변경되어 결과를 다시 실행해야 합니다",
            )));
        }
        self.run_editable(&plan.sql, max_rows).await
    }

    /// 세션 연결이 살아 있는지.
    pub fn is_alive(&self) -> bool {
        self.conn.try_lock().map(|g| g.is_some()).unwrap_or(true)
    }

    /// 서버 쪽 세션 ID.
    pub fn backend_id(&self) -> Option<i64> {
        *self.backend.lock()
    }

    /// 실행 중인 문장 취소를 서버에 요청한다(pg: pg_cancel_backend, mysql: KILL QUERY).
    pub async fn cancel(&self) -> DbResult<()> {
        let Some(pid) = self.backend_id() else {
            return Ok(());
        };
        let sql = match self.driver {
            Driver::Postgres => format!("SELECT pg_cancel_backend({pid})"),
            Driver::MySql | Driver::MariaDb => format!("KILL QUERY {pid}"),
            Driver::Sqlite => return Ok(()),
        };
        self.pool.run_sql(&sql, None).await.map(|_| ())
    }

    pub fn driver(&self) -> Driver {
        self.driver
    }
}

#[cfg(test)]
mod recovery_query_tests {
    use super::*;
    #[test]
    fn row_recovery_query_binds_keys_and_does_not_read_unrelated_rows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fixture.db");
        std::fs::write(&file, []).unwrap();
        let manager = DbManager::in_memory();
        let id = manager.add(
            ConnConfig {
                driver: Driver::Sqlite,
                file: file.to_string_lossy().into_owned(),
                ..Default::default()
            },
            None,
        );
        manager.block_on(manager.query(id,"CREATE TABLE items (id TEXT PRIMARY KEY, value TEXT); INSERT INTO items VALUES ('one','a'),('two','b');",None)).unwrap();
        let table = TableRef::new(None, "items");
        let details = manager.block_on(manager.table_details(id, &table)).unwrap();
        let keys = vec![vec![(0, crate::Value::Text("two".into()))]];
        let result = manager
            .block_on(manager.fetch_original_rows(id, &table, &details.columns, &keys))
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], crate::Value::Text("two".into()));
        let malicious = vec![vec![(
            0,
            crate::Value::Text("' OR 1=1; DROP TABLE items; --".into()),
        )]];
        let result = manager
            .block_on(manager.fetch_original_rows(id, &table, &details.columns, &malicious))
            .unwrap();
        assert!(result.rows.is_empty());
        assert_eq!(result.columns.len(), 2);
        assert_eq!(
            manager
                .block_on(manager.count_rows(id, &table, ""))
                .unwrap(),
            2
        );
    }
}
