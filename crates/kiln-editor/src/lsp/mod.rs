//! 언어 서버(LSP) 클라이언트. 서버마다 백그라운드 스레드에서 stdio 로 JSON-RPC 를 주고받고,
//! UI 스레드에는 막히지 않는 호출과 [`Pending`] 결과 슬롯만 내놓는다.

mod client;
mod config;
mod edit;
pub mod jsonrpc;
mod panel;
pub mod position;
mod proto;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};

use client::{ServerHandle, Status};
pub use config::{LspConfig, ServerSpec};
pub use edit::{apply_edits_to_file, apply_text_edits, snippet_to_plain};
pub use panel::{diagnostics_ui, paint_severity, severity_color};
pub use proto::{path_to_uri, uri_to_path};

/// LSP 위치. `character` 는 UTF-16 코드 단위 열.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// 파일별 편집 목록. `changes` 와 `documentChanges` 를 합친 형태.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceEdit {
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
}

/// 문서 변경 하나. `range` 가 `None` 이면 전체 텍스트 교체.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// 자동 완성 항목. 스니펫은 일반 텍스트로 바뀌어 있다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: Option<u32>,
    pub filter_text: String,
    pub sort_text: String,
    pub insert_text: String,
    /// `textEdit`(InsertReplace 면 insert 범위).
    pub edit: Option<TextEdit>,
    pub additional_edits: Vec<TextEdit>,
    /// 스니펫의 첫 탭 정지 위치(삽입 텍스트 안 바이트 오프셋).
    pub cursor_offset: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionList {
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
}

/// 함수 서명 도움말. `active_param` 은 `label` 안 바이트 범위.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    pub active_param: Option<(usize, usize)>,
    pub doc: Option<String>,
}

/// 요청 응답 기본 대기 시간.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 막히지 않는 요청 결과 슬롯.
pub struct Pending<T> {
    rx: Option<Receiver<Result<T, String>>>,
    deadline: Instant,
}

impl<T> Pending<T> {
    fn channel() -> (mpsc::Sender<Result<T, String>>, Self) {
        let (tx, rx) = mpsc::channel();
        (tx, Self { rx: Some(rx), deadline: Instant::now() + REQUEST_TIMEOUT })
    }

    /// 곧바로 실패한 결과.
    pub fn failed(msg: impl Into<String>) -> Self {
        let (tx, p) = Self::channel();
        let _ = tx.send(Err(msg.into()));
        p
    }

    /// 끝났으면 결과를 한 번만 돌려준다. 시간 초과·서버 종료는 `Err`.
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        let rx = self.rx.as_ref()?;
        let out = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) if Instant::now() < self.deadline => return None,
            Err(TryRecvError::Empty) => Err("응답 시간 초과".to_owned()),
            Err(TryRecvError::Disconnected) => Err("언어 서버 연결이 끊겼습니다".to_owned()),
        };
        self.rx = None;
        Some(out)
    }

    /// 결과를 기다린다(테스트용).
    pub fn wait(self, timeout: Duration) -> Option<Result<T, String>> {
        let rx = self.rx?;
        match rx.recv_timeout(timeout) {
            Ok(r) => Some(r),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err("언어 서버 연결이 끊겼습니다".to_owned())),
        }
    }

    /// 아직 결과를 기다리는 중인지.
    pub fn is_pending(&self) -> bool {
        self.rx.is_some()
    }
}

/// 서버 식별자: (작업 공간 루트, 서버 계열).
pub(crate) type ServerKey = (PathBuf, String);

pub(crate) struct Doc {
    pub mirror: edit::Mirror,
    pub version: i32,
    pub language: String,
    pub server: Arc<ServerHandle>,
}

/// 백그라운드 스레드와 공유하는 상태.
#[derive(Default)]
pub(crate) struct Shared {
    pub docs: Mutex<HashMap<PathBuf, Doc>>,
    pub diagnostics: Mutex<HashMap<PathBuf, Vec<Diagnostic>>>,
    pub diag_version: AtomicU64,
    pub pending_edits: Mutex<HashMap<PathBuf, Vec<TextEdit>>>,
    pub ctx: Mutex<Option<egui::Context>>,
    /// 서버를 찾지 못한 언어 ID.
    pub missing: Mutex<BTreeSet<String>>,
}

impl Shared {
    pub fn repaint(&self) {
        if let Some(ctx) = self.ctx.lock().as_ref() {
            ctx.request_repaint();
        }
    }

    /// 열린 문서의 편집은 편집기 대기열에 넣고, 나머지는 디스크 파일에 쓴다.
    pub fn apply_workspace_edit(&self, edit: &WorkspaceEdit) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        for (path, edits) in &edit.changes {
            let open = self.docs.lock().contains_key(path);
            if open {
                self.pending_edits.lock().entry(path.clone()).or_default().extend(edits.iter().cloned());
            } else if let Err(e) = edit::apply_edits_to_file(path, edits) {
                errors.push(format!("{e:#}"));
            }
        }
        self.repaint();
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("\n"));
        Ok(())
    }

    pub fn publish_diagnostics(&self, path: PathBuf, diags: Vec<Diagnostic>) {
        {
            let mut map = self.diagnostics.lock();
            if diags.is_empty() {
                map.remove(&path);
            } else {
                map.insert(path, diags);
            }
        }
        self.diag_version.fetch_add(1, Ordering::Relaxed);
        self.repaint();
    }
}

struct Inner {
    root: PathBuf,
    config: LspConfig,
    shared: Arc<Shared>,
    servers: Mutex<HashMap<ServerKey, Arc<ServerHandle>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        shutdown_servers(&self.servers, &self.shared);
    }
}

fn shutdown_servers(servers: &Mutex<HashMap<ServerKey, Arc<ServerHandle>>>, shared: &Shared) {
    let list: Vec<_> = servers.lock().drain().map(|(_, s)| s).collect();
    shared.docs.lock().clear();
    for s in list {
        s.shutdown();
    }
}

/// 작업 공간 하나의 언어 서버들. 복제하면 같은 서버들을 공유한다. 마지막 복제본이 사라지면 서버를 끈다.
#[derive(Clone)]
pub struct LspManager {
    inner: Arc<Inner>,
}

impl LspManager {
    /// `lsp.json` 설정과 내장 기본값으로 만든다.
    pub fn new(root: PathBuf) -> Self {
        Self::with_config(root, LspConfig::load())
    }

    pub fn with_config(root: PathBuf, config: LspConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                config,
                shared: Arc::new(Shared::default()),
                servers: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.inner.root.clone()
    }

    /// 서버 메시지가 도착하면 이 컨텍스트에 다시 그리기를 요청한다.
    pub fn set_repaint_ctx(&self, ctx: &egui::Context) {
        let mut slot = self.inner.shared.ctx.lock();
        if slot.is_none() {
            *slot = Some(ctx.clone());
        }
    }

    /// 내장 확장자 표로 언어 ID 를 고른다.
    pub fn language_id(path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        config::builtin_language(&ext).map(str::to_owned)
    }

    /// 설정의 `extensions` 까지 고려한 언어 ID.
    pub fn language_for(&self, path: &Path) -> Option<String> {
        self.inner.config.language_for(path)
    }

    /// 경로에 맞는 서버를 찾거나 띄운다. 서버가 없거나 꺼져 있으면 `None`.
    fn server_for(&self, path: &Path, language: &str) -> Option<Arc<ServerHandle>> {
        let (family, spec) = self.inner.config.spec_for(language)?;
        if !spec.enabled {
            return None;
        }
        let key: ServerKey = (config::workspace_root(&self.inner.root, path), family.clone());
        if let Some(s) = self.inner.servers.lock().get(&key) {
            return (!matches!(s.status(), Status::Failed(_))).then(|| s.clone());
        }
        let candidates = std::iter::once(spec).chain(config::fallbacks(&family));
        let Some((exe, spec)) = candidates.filter(|s| s.enabled).find_map(|s| config::find_executable(&s.command).map(|p| (p, s)))
        else {
            self.inner.shared.missing.lock().insert(language.to_owned());
            return None;
        };
        self.inner.shared.missing.lock().remove(language);
        let handle = ServerHandle::spawn(key.clone(), exe, spec.args.clone(), self.inner.shared.clone());
        let mut servers = self.inner.servers.lock();
        Some(servers.entry(key).or_insert(handle).clone())
    }

    /// 문서를 연다. 언어를 모르거나 서버가 없으면 `false`. 서버는 백그라운드에서 시작한다.
    pub fn open_document(&self, path: &Path, text: &str) -> bool {
        let Some(language) = self.language_for(path) else { return false };
        let Some(server) = self.server_for(path, &language) else { return false };
        let mut docs = self.inner.shared.docs.lock();
        if let Some(old) = docs.remove(path) {
            old.server.doc_notify("textDocument/didClose", json!({"textDocument": {"uri": path_to_uri(path)}}));
        }
        let doc = Doc { mirror: edit::Mirror::new(text), version: 0, language, server: server.clone() };
        server.doc_notify("textDocument/didOpen", did_open_params(path, &doc));
        docs.insert(path.to_path_buf(), doc);
        true
    }

    /// 변경을 사본에 적용하고 서버의 동기화 방식(증분·전체)에 맞춰 보낸다.
    pub fn change_document(&self, path: &Path, changes: &[TextChange]) {
        if changes.is_empty() {
            return;
        }
        let mut docs = self.inner.shared.docs.lock();
        let Some(doc) = docs.get_mut(path) else { return };
        for c in changes {
            doc.mirror.apply(c);
        }
        doc.version += 1;
        let content_changes: Vec<Value> = match doc.server.caps().sync_kind {
            0 => return,
            1 => vec![json!({"text": doc.mirror.text()})],
            _ => changes
                .iter()
                .map(|c| match c.range {
                    Some(r) => json!({"range": proto::range_json(r), "text": c.text}),
                    None => json!({"text": c.text}),
                })
                .collect(),
        };
        doc.server.doc_notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": path_to_uri(path), "version": doc.version},
                "contentChanges": content_changes,
            }),
        );
    }

    pub fn save_document(&self, path: &Path) {
        let docs = self.inner.shared.docs.lock();
        let Some(doc) = docs.get(path) else { return };
        let caps = doc.server.caps();
        if !caps.save {
            return;
        }
        let mut params = json!({"textDocument": {"uri": path_to_uri(path)}});
        if caps.save_include_text {
            params["text"] = Value::String(doc.mirror.text());
        }
        doc.server.doc_notify("textDocument/didSave", params);
    }

    pub fn close_document(&self, path: &Path) {
        if let Some(doc) = self.inner.shared.docs.lock().remove(path) {
            doc.server.doc_notify("textDocument/didClose", json!({"textDocument": {"uri": path_to_uri(path)}}));
        }
        self.inner.shared.pending_edits.lock().remove(path);
    }

    /// 서버에 보낸 문서 내용의 사본.
    pub fn document_text(&self, path: &Path) -> Option<String> {
        self.inner.shared.docs.lock().get(path).map(|d| d.mirror.text())
    }

    /// 문서 버전(열 때 0, 변경마다 1씩 증가).
    pub fn document_version(&self, path: &Path) -> Option<i32> {
        self.inner.shared.docs.lock().get(path).map(|d| d.version)
    }

    fn server_of(&self, path: &Path) -> Option<Arc<ServerHandle>> {
        self.inner.shared.docs.lock().get(path).map(|d| d.server.clone())
    }

    /// 문서의 서버가 초기화를 마쳤는지.
    pub fn is_ready(&self, path: &Path) -> bool {
        self.server_of(path).is_some_and(|s| s.is_ready())
    }

    pub fn completion_triggers(&self, path: &Path) -> Vec<String> {
        self.server_of(path).map(|s| s.caps().completion_triggers).unwrap_or_default()
    }

    pub fn signature_triggers(&self, path: &Path) -> Vec<String> {
        self.server_of(path).map(|s| s.caps().signature_triggers).unwrap_or_default()
    }

    fn request<T: Send + 'static>(
        &self,
        path: &Path,
        method: &str,
        params: Value,
        parse: impl FnOnce(Value) -> Result<T, String> + Send + 'static,
    ) -> Pending<T> {
        let Some(server) = self.server_of(path) else { return Pending::failed("이 파일에 연결된 언어 서버가 없습니다") };
        let (tx, pending) = Pending::channel();
        server.request(
            method,
            params,
            Box::new(move |r| {
                let _ = tx.send(r.and_then(parse));
            }),
        );
        pending
    }

    fn doc_pos(path: &Path, pos: Position) -> Value {
        json!({"textDocument": {"uri": path_to_uri(path)}, "position": proto::position_json(pos)})
    }

    pub fn hover(&self, path: &Path, pos: Position) -> Pending<Option<String>> {
        self.request(path, "textDocument/hover", Self::doc_pos(path, pos), |v| Ok(proto::parse_hover(&v)))
    }

    pub fn definition(&self, path: &Path, pos: Position) -> Pending<Vec<Location>> {
        self.request(path, "textDocument/definition", Self::doc_pos(path, pos), |v| Ok(proto::parse_locations(&v)))
    }

    pub fn references(&self, path: &Path, pos: Position) -> Pending<Vec<Location>> {
        let mut params = Self::doc_pos(path, pos);
        params["context"] = json!({"includeDeclaration": true});
        self.request(path, "textDocument/references", params, |v| Ok(proto::parse_locations(&v)))
    }

    pub fn completion(&self, path: &Path, pos: Position, trigger_char: Option<String>) -> Pending<CompletionList> {
        let mut params = Self::doc_pos(path, pos);
        params["context"] = match trigger_char {
            Some(c) => json!({"triggerKind": 2, "triggerCharacter": c}),
            None => json!({"triggerKind": 1}),
        };
        self.request(path, "textDocument/completion", params, |v| Ok(proto::parse_completion(&v)))
    }

    pub fn signature_help(&self, path: &Path, pos: Position) -> Pending<Option<SignatureHelp>> {
        self.request(path, "textDocument/signatureHelp", Self::doc_pos(path, pos), |v| Ok(proto::parse_signature_help(&v)))
    }

    pub fn rename(&self, path: &Path, pos: Position, new_name: &str) -> Pending<WorkspaceEdit> {
        let mut params = Self::doc_pos(path, pos);
        params["newName"] = Value::String(new_name.to_owned());
        self.request(path, "textDocument/rename", params, |v| {
            if v.is_null() { Err("이름을 바꿀 수 없는 위치입니다".to_owned()) } else { Ok(proto::parse_workspace_edit(&v)) }
        })
    }

    pub fn format(&self, path: &Path, tab_size: u32, insert_spaces: bool) -> Pending<Vec<TextEdit>> {
        let params = json!({
            "textDocument": {"uri": path_to_uri(path)},
            "options": {"tabSize": tab_size, "insertSpaces": insert_spaces, "trimTrailingWhitespace": true},
        });
        self.request(path, "textDocument/formatting", params, |v| Ok(proto::parse_text_edits(&v)))
    }

    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        self.inner.shared.diagnostics.lock().get(path).cloned().unwrap_or_default()
    }

    /// 진단이 있는 모든 파일. 경로 순으로 정렬된다.
    pub fn all_diagnostics(&self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        let mut v: Vec<_> = self
            .inner
            .shared
            .diagnostics
            .lock()
            .iter()
            .filter(|(_, d)| !d.is_empty())
            .map(|(p, d)| (p.clone(), d.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Empty diagnostics are not proof that the workspace has been checked.
    pub(crate) fn empty_diagnostics_status(&self) -> (&'static str,String,bool) {
        let servers=self.inner.servers.lock();
        let unavailable=!self.inner.shared.missing.lock().is_empty() || servers.values().any(|s|matches!(s.status(),Status::Failed(_) | Status::Stopped));
        let starting=servers.values().any(|s|matches!(s.status(),Status::Starting | Status::Restarting));
        let has_servers=!servers.is_empty();
        drop(servers);
        if unavailable { return ("진단 연결 확인 필요",format!("{}\n언어 서버 설정을 확인하세요. 전체 프로젝트를 검사한 결과가 아닙니다.",self.status_text().unwrap_or_else(||"언어 서버가 중지되었습니다".into())),false); }
        if starting { return ("언어 서버 시작 중",self.status_text().unwrap_or_else(||"진단 결과를 기다리고 있습니다.".into()),false); }
        if self.diagnostics_version()>0 { return ("수신한 진단 0건","수신한 진단에서 발견된 문제가 없습니다. 전체 프로젝트 검사 결과는 아닙니다.".into(),true); }
        if has_servers { ("진단 대기 중","언어 서버에서 아직 진단 결과를 받지 못했습니다.".into(),false) }
        else { ("아직 진단 결과 없음","코드 파일을 열면 언어 서버 연결을 시작합니다. 서버가 설치되어 있어야 진단을 받을 수 있습니다.".into(),false) }
    }

    /// 진단이 게시될 때마다 증가한다.
    pub fn diagnostics_version(&self) -> u64 {
        self.inner.shared.diag_version.load(Ordering::Relaxed)
    }

    /// 진단을 직접 넣는다(테스트·미리보기용).
    #[doc(hidden)]
    pub fn inject_diagnostics(&self, path: PathBuf, diags: Vec<Diagnostic>) {
        self.inner.shared.publish_diagnostics(path, diags);
    }

    /// 열린 문서의 편집은 대기열에 넣고([`take_pending_edits`](Self::take_pending_edits)),
    /// 나머지 파일은 디스크에서 바로 고친다.
    pub fn apply_workspace_edit(&self, edit: &WorkspaceEdit) -> anyhow::Result<()> {
        self.inner.shared.apply_workspace_edit(edit)
    }

    /// 편집기가 자기 파일에 적용할 대기 편집을 꺼낸다.
    pub fn take_pending_edits(&self, path: &Path) -> Vec<TextEdit> {
        self.inner.shared.pending_edits.lock().remove(path).unwrap_or_default()
    }

    /// 상태 표시줄 문구. 아무 문서도 열지 않았으면 `None`.
    pub fn status_text(&self) -> Option<String> {
        let mut servers: Vec<Arc<ServerHandle>> = self.inner.servers.lock().values().cloned().collect();
        servers.sort_by(|a, b| a.name.cmp(&b.name));
        let mut parts: Vec<String> = servers.iter().filter_map(|s| s.status_text()).collect();
        parts.dedup();
        for lang in self.inner.shared.missing.lock().iter() {
            parts.push(format!("{lang}: 언어 서버 없음"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// 모든 서버에 종료를 요청한다. 실제 종료는 백그라운드에서 진행된다.
    pub fn shutdown(&self) {
        shutdown_servers(&self.inner.servers, &self.inner.shared);
    }

    /// 편집기 UI 가 아닌 곳(문제 패널)에서 한 줄 내용을 얻는다: 열린 문서 사본, 없으면 디스크.
    pub(crate) fn line_text(&self, path: &Path, line: usize) -> Option<String> {
        if let Some(d) = self.inner.shared.docs.lock().get(path) {
            return d.mirror.line(line).map(str::to_owned);
        }
        let text = std::fs::read_to_string(path).ok()?;
        text.split('\n').nth(line).map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
    }
}

fn did_open_params(path: &Path, doc: &Doc) -> Value {
    json!({"textDocument": {
        "uri": path_to_uri(path),
        "languageId": doc.language,
        "version": doc.version,
        "text": doc.mirror.text(),
    }})
}

/// `$/progress` 제목을 한국어로 옮긴다. 모르는 제목은 그대로 둔다.
pub(crate) fn progress_title(title: &str) -> String {
    let l = title.to_ascii_lowercase();
    let known = [
        ("indexing", "인덱싱 중"),
        ("fetching", "가져오는 중"),
        ("loading", "불러오는 중"),
        ("building", "빌드 중"),
        ("roots scanned", "파일 검사 중"),
        ("cargo check", "검사 중"),
        ("checking", "검사 중"),
        ("flycheck", "검사 중"),
        ("background index", "인덱싱 중"),
    ];
    known.iter().find(|(k, _)| l.starts_with(k)).map_or_else(|| title.to_owned(), |(_, v)| (*v).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diagnostics_distinguish_unchecked_missing_and_received() {
        let m=LspManager::with_config(PathBuf::from("/tmp/kiln-diagnostics-fixture"),LspConfig::default());
        assert_eq!(m.empty_diagnostics_status().0,"아직 진단 결과 없음");
        assert!(!m.empty_diagnostics_status().2);
        m.inject_diagnostics(PathBuf::from("/tmp/kiln-diagnostics-fixture/a.rs"),vec![]);
        assert_eq!(m.empty_diagnostics_status().0,"수신한 진단 0건");
        m.inner.shared.missing.lock().insert("rust".into());
        assert_eq!(m.empty_diagnostics_status().0,"진단 연결 확인 필요");
        assert!(!m.empty_diagnostics_status().2);
    }

    #[test]
    fn pending_reports_once_and_fails_when_sender_dropped() {
        let (tx, mut p) = Pending::<u32>::channel();
        assert!(p.poll().is_none());
        tx.send(Ok(3)).unwrap();
        assert_eq!(p.poll(), Some(Ok(3)));
        assert_eq!(p.poll(), None);
        let (tx, mut p) = Pending::<u32>::channel();
        drop(tx);
        assert!(matches!(p.poll(), Some(Err(_))));
        assert!(matches!(Pending::<u32>::failed("x").wait(Duration::from_secs(1)), Some(Err(e)) if e == "x"));
    }

    #[test]
    fn unknown_language_and_missing_server_are_disabled() {
        let mut cfg = LspConfig::default();
        cfg.languages.insert("rust".into(), ServerSpec::new("kiln-no-such-server", &[]));
        let m = LspManager::with_config(PathBuf::from("/tmp/x"), cfg);
        assert!(!m.open_document(Path::new("/tmp/x/a.txt"), "hi"));
        assert_eq!(m.status_text(), None);
        assert!(!m.open_document(Path::new("/tmp/x/a.rs"), "fn a() {}"));
        assert_eq!(m.status_text().as_deref(), Some("rust: 언어 서버 없음"));
        assert!(m.hover(Path::new("/tmp/x/a.rs"), Position::default()).wait(Duration::from_secs(1)).unwrap().is_err());
    }

    #[test]
    fn progress_titles_translate() {
        assert_eq!(progress_title("Indexing"), "인덱싱 중");
        assert_eq!(progress_title("Roots Scanned"), "파일 검사 중");
        assert_eq!(progress_title("Custom"), "Custom");
    }

    #[test]
    fn workspace_edit_queues_open_docs_and_writes_closed_files() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.rs");
        std::fs::write(&other, "let a = 1;\n").unwrap();
        let m = LspManager::with_config(dir.path().to_path_buf(), LspConfig::default());
        let open = dir.path().join("open.rs");
        // 서버 없이 문서 등록만 흉내 낸다.
        let handle = ServerHandle::detached(("x".into(), "x".into()), m.inner.shared.clone());
        m.inner.shared.docs.lock().insert(open.clone(), Doc {
            mirror: edit::Mirror::new("a"),
            version: 0,
            language: "rust".into(),
            server: handle,
        });
        let r = Range { start: Position::new(0, 4), end: Position::new(0, 5) };
        let e = WorkspaceEdit {
            changes: vec![
                (open.clone(), vec![TextEdit { range: r, new_text: "b".into() }]),
                (other.clone(), vec![TextEdit { range: r, new_text: "b".into() }]),
            ],
        };
        m.apply_workspace_edit(&e).unwrap();
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "let b = 1;\n");
        assert_eq!(m.take_pending_edits(&open).len(), 1);
        assert!(m.take_pending_edits(&open).is_empty());
    }
}
