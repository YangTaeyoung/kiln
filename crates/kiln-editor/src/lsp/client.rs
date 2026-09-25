//! 언어 서버 프로세스 하나: 감독 스레드(실행·초기화·쓰기·재시작)와 읽기 스레드.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};

use super::jsonrpc::{self, Incoming};
use super::{ServerKey, Shared, proto};

/// 응답을 받을 콜백.
pub(crate) type Callback = Box<dyn FnOnce(Result<Value, String>) + Send>;

const MAX_RESTARTS: u32 = 3;
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const STABLE_UPTIME: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Starting,
    Ready,
    Restarting,
    Failed(String),
    Stopped,
}

/// 서버가 알린 기능 중 클라이언트가 쓰는 것.
#[derive(Clone, Debug)]
pub(crate) struct Caps {
    /// 0 없음, 1 전체, 2 증분.
    pub sync_kind: u8,
    pub save: bool,
    pub save_include_text: bool,
    pub completion_triggers: Vec<String>,
    pub signature_triggers: Vec<String>,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            sync_kind: 2,
            save: true,
            save_include_text: false,
            completion_triggers: Vec::new(),
            signature_triggers: Vec::new(),
        }
    }
}

impl Caps {
    fn parse(v: &Value) -> Self {
        let mut c = Caps { save: false, ..Default::default() };
        match v.get("textDocumentSync") {
            Some(Value::Number(n)) => {
                c.sync_kind = n.as_u64().unwrap_or(2) as u8;
                c.save = true;
            }
            Some(Value::Object(o)) => {
                c.sync_kind = o.get("change").and_then(Value::as_u64).unwrap_or(0) as u8;
                match o.get("save") {
                    Some(Value::Bool(b)) => c.save = *b,
                    Some(Value::Object(s)) => {
                        c.save = true;
                        c.save_include_text = s.get("includeText").and_then(Value::as_bool).unwrap_or(false);
                    }
                    _ => {}
                }
            }
            _ => c.sync_kind = 0,
        }
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        c.completion_triggers = strings(v.get("completionProvider").and_then(|p| p.get("triggerCharacters")));
        c.signature_triggers = strings(v.get("signatureHelpProvider").and_then(|p| p.get("triggerCharacters")));
        c
    }
}

#[derive(Clone, Debug, Default)]
struct Progress {
    title: String,
    message: Option<String>,
    percentage: Option<u32>,
}

/// 감독 스레드가 받는 명령.
enum Cmd {
    Request { method: String, params: Value, cb: Callback },
    /// 문서 동기화 알림. `generation` 이 현재 프로세스와 다르면 버린다.
    DocNotify { generation: u64, method: String, params: Value },
    /// 서버 요청에 대한 응답 등 그대로 쓸 메시지.
    Raw(Value),
    ReaderExited(u64),
    Shutdown,
}

/// 감독 스레드와 읽기 스레드가 공유하는 서버 상태.
struct State {
    status: Mutex<Status>,
    ready: AtomicBool,
    generation: AtomicU64,
    caps: Mutex<Caps>,
    progress: Mutex<BTreeMap<String, Progress>>,
    pending: Mutex<HashMap<i64, Callback>>,
    next_id: AtomicI64,
}

impl State {
    fn new() -> Self {
        Self {
            status: Mutex::new(Status::Starting),
            ready: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            caps: Mutex::new(Caps::default()),
            progress: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
        }
    }

    fn fail_pending(&self, msg: &str) {
        let list: Vec<Callback> = self.pending.lock().drain().map(|(_, cb)| cb).collect();
        for cb in list {
            cb(Err(msg.to_owned()));
        }
    }
}

/// 실행 중인 언어 서버 하나에 대한 손잡이.
pub(crate) struct ServerHandle {
    pub name: String,
    tx: Sender<Cmd>,
    state: Arc<State>,
}

impl ServerHandle {
    /// 감독 스레드를 띄운다. 프로세스 실행과 초기화는 그 스레드에서 한다.
    pub fn spawn(key: ServerKey, exe: PathBuf, args: Vec<String>, shared: Arc<Shared>) -> Arc<Self> {
        let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| key.1.clone());
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(State::new());
        let sup = Supervisor {
            key,
            exe,
            args,
            name: name.clone(),
            state: state.clone(),
            shared,
            tx: tx.clone(),
            rx,
            backlog: VecDeque::new(),
            generation: 0,
        };
        let spawned = std::thread::Builder::new().name(format!("lsp-{name}")).spawn(move || sup.run());
        if let Err(e) = spawned {
            *state.status.lock() = Status::Failed(e.to_string());
        }
        Arc::new(Self { name, tx, state })
    }

    /// 스레드 없이 명령을 버리는 손잡이(테스트용).
    #[cfg(test)]
    pub fn detached(key: ServerKey, _shared: Arc<Shared>) -> Arc<Self> {
        let (tx, _rx) = mpsc::channel();
        Arc::new(Self { name: key.1, tx, state: Arc::new(State::new()) })
    }

    pub fn status(&self) -> Status {
        self.state.status.lock().clone()
    }

    pub fn is_ready(&self) -> bool {
        self.state.ready.load(Ordering::Acquire)
    }

    pub fn caps(&self) -> Caps {
        self.state.caps.lock().clone()
    }

    pub fn request(&self, method: &str, params: Value, cb: Callback) {
        if let Status::Failed(msg) = self.status() {
            cb(Err(format!("{}: {msg}", self.name)));
            return;
        }
        if let Err(mpsc::SendError(Cmd::Request { cb, .. })) =
            self.tx.send(Cmd::Request { method: method.to_owned(), params, cb })
        {
            cb(Err("언어 서버가 종료되었습니다".to_owned()));
        }
    }

    /// 문서 동기화 알림을 보낸다. 서버가 아직 준비되지 않았으면 보내지 않는다
    /// (준비될 때 열린 문서 사본으로 didOpen 을 다시 보낸다). 문서 목록 잠금을 쥔 채로 부른다.
    pub fn doc_notify(&self, method: &str, params: Value) {
        if !self.is_ready() {
            return;
        }
        let generation = self.state.generation.load(Ordering::Acquire);
        let _ = self.tx.send(Cmd::DocNotify { generation, method: method.to_owned(), params });
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }

    /// 상태 표시줄용 문구. 종료된 서버는 `None`.
    pub fn status_text(&self) -> Option<String> {
        let name = &self.name;
        Some(match self.status() {
            Status::Starting => format!("{name}: 시작 중"),
            Status::Restarting => format!("{name}: 중단됨 — 다시 시작 중"),
            Status::Failed(_) => format!("{name}: 실행 실패"),
            Status::Stopped => return None,
            Status::Ready => {
                let progress = self.state.progress.lock();
                match progress.values().next() {
                    Some(p) => {
                        let mut s = format!("{name}: {}", super::progress_title(&p.title));
                        if let Some(pct) = p.percentage {
                            s.push_str(&format!(" {pct}%"));
                        } else if let Some(m) = p.message.as_deref().filter(|m| !m.is_empty() && m.chars().count() <= 40) {
                            s.push_str(&format!(" ({m})"));
                        }
                        s
                    }
                    None => name.clone(),
                }
            }
        })
    }
}

/// 한 번 실행된 프로세스.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    started: Instant,
}

enum Exit {
    Crashed,
    Shutdown,
}

struct Supervisor {
    key: ServerKey,
    exe: PathBuf,
    args: Vec<String>,
    name: String,
    state: Arc<State>,
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    rx: Receiver<Cmd>,
    backlog: VecDeque<Cmd>,
    generation: u64,
}

impl Supervisor {
    fn run(mut self) {
        let mut restarts = 0u32;
        loop {
            let mut proc = match self.start() {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("{} 실행 실패: {e}", self.name);
                    self.fail_forever(e);
                    return;
                }
            };
            let exit = match self.initialize(&mut proc) {
                Ok(true) => self.serve(&mut proc),
                Ok(false) => Exit::Shutdown,
                Err(e) => {
                    log::warn!("{} 초기화 실패: {e}", self.name);
                    Exit::Crashed
                }
            };
            match exit {
                Exit::Shutdown => {
                    self.stop(proc);
                    return;
                }
                Exit::Crashed => {
                    self.set_not_ready();
                    self.state.fail_pending("언어 서버가 중단되었습니다");
                    self.state.progress.lock().clear();
                    let _ = proc.child.kill();
                    let _ = proc.child.wait();
                    if proc.started.elapsed() > STABLE_UPTIME {
                        restarts = 0;
                    }
                    restarts += 1;
                    if restarts > MAX_RESTARTS {
                        self.fail_forever("여러 번 중단되어 다시 시작하지 않습니다".to_owned());
                        return;
                    }
                    *self.state.status.lock() = Status::Restarting;
                    self.shared.repaint();
                    if self.sleep_or_shutdown(Duration::from_millis(300 * u64::from(restarts))) {
                        *self.state.status.lock() = Status::Stopped;
                        return;
                    }
                }
            }
        }
    }

    fn start(&mut self) -> Result<Proc, String> {
        let mut child = Command::new(&self.exe)
            .args(&self.args)
            .current_dir(&self.key.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", self.exe.display()))?;
        let stdin = child.stdin.take().ok_or("stdin 없음")?;
        let stdout = child.stdout.take().ok_or("stdout 없음")?;
        self.generation += 1;
        let reader = Reader {
            generation: self.generation,
            state: self.state.clone(),
            shared: self.shared.clone(),
            tx: self.tx.clone(),
        };
        std::thread::Builder::new()
            .name(format!("lsp-{}-read", self.name))
            .spawn(move || reader.run(stdout))
            .map_err(|e| e.to_string())?;
        Ok(Proc { child, stdin, started: Instant::now() })
    }

    fn write(&self, proc: &mut Proc, msg: &Value) -> bool {
        jsonrpc::write_message(&mut proc.stdin, msg).is_ok()
    }

    fn send_request(&self, proc: &mut Proc, method: &str, params: Value, cb: Callback) -> bool {
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed);
        self.state.pending.lock().insert(id, cb);
        let ok = self.write(proc, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if !ok && let Some(cb) = self.state.pending.lock().remove(&id) {
            cb(Err("언어 서버에 쓸 수 없습니다".to_owned()));
        }
        ok
    }

    fn notify(&self, proc: &mut Proc, method: &str, params: Value) -> bool {
        self.write(proc, &json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// 초기화 요청을 보내고 응답을 기다린다. 그동안 온 요청은 쌓아 둔다.
    /// 종료 명령을 받으면 `Ok(false)`.
    fn initialize(&mut self, proc: &mut Proc) -> Result<bool, String> {
        *self.state.status.lock() = if self.generation == 1 { Status::Starting } else { Status::Restarting };
        self.shared.repaint();
        let root_uri = proto::path_to_uri(&self.key.0);
        let root_name = self.key.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "kiln"},
            "rootUri": root_uri,
            "rootPath": self.key.0.to_string_lossy(),
            "workspaceFolders": [{"uri": root_uri, "name": root_name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "window": {"workDoneProgress": true},
                "workspace": {
                    "applyEdit": true,
                    "workspaceEdit": {"documentChanges": true},
                    "configuration": true,
                    "workspaceFolders": true,
                },
                "textDocument": {
                    "synchronization": {"didSave": true, "dynamicRegistration": false},
                    "hover": {"contentFormat": ["markdown", "plaintext"]},
                    "completion": {
                        "completionItem": {
                            "snippetSupport": false,
                            "insertReplaceSupport": true,
                            "labelDetailsSupport": true,
                            "documentationFormat": ["markdown", "plaintext"],
                        },
                        "contextSupport": true,
                    },
                    "signatureHelp": {
                        "signatureInformation": {
                            "documentationFormat": ["markdown", "plaintext"],
                            "parameterInformation": {"labelOffsetSupport": true},
                            "activeParameterSupport": true,
                        }
                    },
                    "definition": {"linkSupport": true},
                    "references": {},
                    "rename": {"prepareSupport": false},
                    "formatting": {},
                    "publishDiagnostics": {"relatedInformation": false},
                },
            },
        });
        let (init_tx, init_rx) = mpsc::channel();
        let cb: Callback = Box::new(move |r| {
            let _ = init_tx.send(r);
        });
        if !self.send_request(proc, "initialize", params, cb) {
            return Err("initialize 를 보낼 수 없습니다".into());
        }
        let deadline = Instant::now() + INIT_TIMEOUT;
        let result = loop {
            if let Ok(r) = init_rx.try_recv() {
                break r?;
            }
            if Instant::now() > deadline {
                return Err("initialize 응답 시간 초과".into());
            }
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Cmd::Raw(v)) => {
                    self.write(proc, &v);
                }
                Ok(Cmd::ReaderExited(g)) if g == self.generation => return Err("초기화 중 종료".into()),
                Ok(Cmd::ReaderExited(_)) => {}
                Ok(Cmd::Shutdown) => return Ok(false),
                Ok(Cmd::DocNotify { .. }) => {}
                Ok(cmd) => self.backlog.push_back(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(false),
            }
        };
        *self.state.caps.lock() = Caps::parse(result.get("capabilities").unwrap_or(&Value::Null));
        if !self.notify(proc, "initialized", json!({})) {
            return Err("initialized 를 보낼 수 없습니다".into());
        }
        // 열린 문서를 모두 열고 준비 상태로 바꾼다. 문서 목록 잠금 안에서 해 순서를 보장한다.
        let docs = self.shared.docs.lock();
        for (path, doc) in docs.iter() {
            if !Arc::ptr_eq(&doc.server.state, &self.state) {
                continue;
            }
            let params = super::did_open_params(path, doc);
            if !self.notify(proc, "textDocument/didOpen", params) {
                return Err("didOpen 을 보낼 수 없습니다".into());
            }
        }
        self.state.generation.store(self.generation, Ordering::Release);
        self.state.ready.store(true, Ordering::Release);
        *self.state.status.lock() = Status::Ready;
        drop(docs);
        self.shared.repaint();
        Ok(true)
    }

    fn set_not_ready(&self) {
        let _docs = self.shared.docs.lock();
        self.state.ready.store(false, Ordering::Release);
    }

    fn serve(&mut self, proc: &mut Proc) -> Exit {
        loop {
            let cmd = match self.backlog.pop_front() {
                Some(c) => c,
                None => match self.rx.recv() {
                    Ok(c) => c,
                    Err(_) => return Exit::Shutdown,
                },
            };
            let ok = match cmd {
                Cmd::Request { method, params, cb } => self.send_request(proc, &method, params, cb),
                Cmd::DocNotify { generation, method, params } => {
                    generation != self.generation || self.notify(proc, &method, params)
                }
                Cmd::Raw(v) => self.write(proc, &v),
                Cmd::ReaderExited(g) if g == self.generation => return Exit::Crashed,
                Cmd::ReaderExited(_) => true,
                Cmd::Shutdown => return Exit::Shutdown,
            };
            if !ok {
                return Exit::Crashed;
            }
        }
    }

    /// shutdown 요청과 exit 알림을 보내고 잠시 기다린 뒤 프로세스를 끝낸다.
    fn stop(&mut self, mut proc: Proc) {
        self.set_not_ready();
        let (tx, rx) = mpsc::channel();
        let cb: Callback = Box::new(move |r| {
            let _ = tx.send(r);
        });
        if self.send_request(&mut proc, "shutdown", Value::Null, cb) {
            let _ = rx.recv_timeout(Duration::from_secs(1));
            self.notify(&mut proc, "exit", Value::Null);
        }
        drop(proc.stdin);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if matches!(proc.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = proc.child.kill();
        let _ = proc.child.wait();
        self.state.fail_pending("언어 서버가 종료되었습니다");
        self.state.progress.lock().clear();
        *self.state.status.lock() = Status::Stopped;
        self.shared.repaint();
    }

    /// 지정 시간 동안 명령을 처리하며 기다린다. 종료 명령이 오면 `true`.
    fn sleep_or_shutdown(&mut self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            match self.rx.recv_timeout(left) {
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return true,
                Ok(Cmd::Request { method, params, cb }) => self.backlog.push_back(Cmd::Request { method, params, cb }),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return false,
            }
        }
    }

    /// 실행에 실패한 서버: 이후 요청은 모두 즉시 실패시킨다.
    fn fail_forever(&mut self, msg: String) {
        self.set_not_ready();
        *self.state.status.lock() = Status::Failed(msg.clone());
        self.state.fail_pending(&msg);
        self.shared.repaint();
        for cmd in self.backlog.drain(..) {
            if let Cmd::Request { cb, .. } = cmd {
                cb(Err(msg.clone()));
            }
        }
        while let Ok(cmd) = self.rx.recv() {
            match cmd {
                Cmd::Request { cb, .. } => cb(Err(msg.clone())),
                Cmd::Shutdown => return,
                _ => {}
            }
        }
    }
}

/// 서버 stdout 을 읽어 응답·알림·서버 요청을 처리한다.
struct Reader {
    generation: u64,
    state: Arc<State>,
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
}

impl Reader {
    fn run(self, stdout: impl std::io::Read) {
        let mut r = BufReader::new(stdout);
        while let Ok(Some(msg)) = jsonrpc::read_message(&mut r) {
            self.handle(msg);
        }
        let _ = self.tx.send(Cmd::ReaderExited(self.generation));
    }

    fn reply(&self, id: Value, result: Result<Value, (i64, &str)>) {
        let msg = match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err((code, m)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}}),
        };
        let _ = self.tx.send(Cmd::Raw(msg));
    }

    fn handle(&self, msg: Value) {
        match jsonrpc::classify(msg) {
            Incoming::Response { id, result } => {
                let cb = id.as_i64().and_then(|id| self.state.pending.lock().remove(&id));
                if let Some(cb) = cb {
                    cb(result);
                }
            }
            Incoming::Request { id, method, params } => match method.as_str() {
                "window/workDoneProgress/create" | "client/registerCapability" | "client/unregisterCapability" => {
                    self.reply(id, Ok(Value::Null))
                }
                "workspace/configuration" => {
                    let n = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
                    self.reply(id, Ok(Value::Array(vec![Value::Null; n])))
                }
                "workspace/workspaceFolders" => self.reply(id, Ok(Value::Null)),
                "workspace/applyEdit" => {
                    let edit = proto::parse_workspace_edit(params.get("edit").unwrap_or(&Value::Null));
                    let res = match self.shared.apply_workspace_edit(&edit) {
                        Ok(()) => json!({"applied": true}),
                        Err(e) => json!({"applied": false, "failureReason": format!("{e:#}")}),
                    };
                    self.reply(id, Ok(res))
                }
                "window/showMessageRequest" => self.reply(id, Ok(Value::Null)),
                _ => self.reply(id, Err((-32601, "지원하지 않는 메서드"))),
            },
            Incoming::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let Some(path) = params.get("uri").and_then(Value::as_str).and_then(proto::uri_to_path) else {
                        return;
                    };
                    let diags = params
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(proto::parse_diagnostic).collect())
                        .unwrap_or_default();
                    self.shared.publish_diagnostics(path, diags);
                }
                "$/progress" => {
                    self.progress(&params);
                    self.shared.repaint();
                }
                _ => {}
            },
            Incoming::Invalid => {}
        }
    }

    fn progress(&self, params: &Value) {
        let token = match params.get("token") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => return,
        };
        let Some(value) = params.get("value") else { return };
        let mut map = self.state.progress.lock();
        let pct = value.get("percentage").and_then(Value::as_u64).map(|p| p.min(100) as u32);
        let msg = value.get("message").and_then(Value::as_str).map(str::to_owned);
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                let title = value.get("title").and_then(Value::as_str).unwrap_or("").to_owned();
                map.insert(token, Progress { title, message: msg, percentage: pct });
            }
            Some("report") => {
                if let Some(p) = map.get_mut(&token) {
                    if pct.is_some() {
                        p.percentage = pct;
                    }
                    if msg.is_some() {
                        p.message = msg;
                    }
                }
            }
            Some("end") => {
                map.remove(&token);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_parse_sync_kinds_and_triggers() {
        let c = Caps::parse(&json!({"textDocumentSync": 1, "completionProvider": {"triggerCharacters": [".", ":"]}}));
        assert_eq!((c.sync_kind, c.save), (1, true));
        assert_eq!(c.completion_triggers, vec![".", ":"]);
        let c = Caps::parse(&json!({"textDocumentSync": {"change": 2, "save": {"includeText": true}}}));
        assert_eq!((c.sync_kind, c.save, c.save_include_text), (2, true, true));
        let c = Caps::parse(&json!({}));
        assert_eq!(c.sync_kind, 0);
    }
}
