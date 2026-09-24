//! GUI 쪽 데몬 연결: 재연결, 자동 데몬 업그레이드, 세션 화면 캐시.

use kiln_daemon::client::Client;
use kiln_proto::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 클라이언트가 보관하는 세션 화면.
#[derive(Default)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Line>,
    pub cursor: Option<Cursor>,
    pub mode: u32,
    pub display_offset: u32,
    pub history: u32,
    /// 프레임을 받을 때마다 증가한다.
    pub version: u64,
    pub row_versions: Vec<u64>,
}

impl Screen {
    pub fn apply(&mut self, f: Frame) {
        self.version += 1;
        if f.full || f.rows != self.rows || f.cols != self.cols {
            self.lines = vec![Line::default(); f.rows as usize];
            self.row_versions = vec![self.version; f.rows as usize];
        }
        self.cols = f.cols;
        self.rows = f.rows;
        for (r, l) in f.lines {
            if let Some(slot) = self.lines.get_mut(r as usize) {
                *slot = l;
                self.row_versions[r as usize] = self.version;
            }
        }
        self.cursor = f.cursor;
        self.mode = f.mode;
        self.display_offset = f.display_offset;
        self.history = f.history;
    }
}

#[derive(Debug, Clone)]
pub enum ConnEvent {
    Notification { session: SessionId, title: String, body: String },
    Exited { session: SessionId, code: Option<i32> },
    Created { req: u32, session: SessionId },
    Error(String),
    Connected,
    Upgrading,
    SearchResult { found: bool },
}

pub enum State {
    Disconnected { since: Instant, last_error: String },
    Connected,
}

pub struct Conn {
    client: Option<Client>,
    pub state: State,
    pub screens: HashMap<SessionId, Screen>,
    pub infos: HashMap<SessionId, SessionInfo>,
    /// 붙어 있는 세션과 요청한 크기.
    attached: HashMap<SessionId, (u16, u16)>,
    pub events: Vec<ConnEvent>,
    ctx: egui::Context,
    socket: String,
    exe: std::path::PathBuf,
    last_attempt: Option<Instant>,
    pub daemon_pid: u32,
    pub daemon_build: String,
    upgrade_requested: bool,
    pub sessions_listed: bool,
}

impl Conn {
    pub fn new(ctx: egui::Context) -> Self {
        let mut c = Conn {
            client: None,
            state: State::Disconnected { since: Instant::now(), last_error: String::new() },
            screens: HashMap::new(),
            infos: HashMap::new(),
            attached: HashMap::new(),
            events: Vec::new(),
            ctx,
            socket: socket_name(),
            exe: daemon_exe(),
            last_attempt: None,
            daemon_pid: 0,
            daemon_build: String::new(),
            upgrade_requested: false,
            sessions_listed: false,
        };
        c.try_connect();
        c
    }

    /// 데몬 없이 화면 캐시만 쓰는 연결(렌더링 벤치마크·테스트용).
    #[doc(hidden)]
    pub fn offline(ctx: egui::Context) -> Self {
        Conn {
            client: None,
            state: State::Disconnected { since: Instant::now(), last_error: String::new() },
            screens: HashMap::new(),
            infos: HashMap::new(),
            attached: HashMap::new(),
            events: Vec::new(),
            ctx,
            socket: String::new(),
            exe: Default::default(),
            last_attempt: Some(Instant::now() + Duration::from_secs(3600)),
            daemon_pid: 0,
            daemon_build: String::new(),
            upgrade_requested: false,
            sessions_listed: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected)
    }

    fn try_connect(&mut self) {
        self.last_attempt = Some(Instant::now());
        let ctx = self.ctx.clone();
        let notify: kiln_daemon::client::Notify = Arc::new(move || ctx.request_repaint());
        match Client::connect_or_spawn(&self.socket, &self.exe, Some(notify)) {
            Ok(c) => {
                self.daemon_pid = c.server_pid;
                self.daemon_build = c.server_build.clone();
                // 실행 파일이 데몬보다 새로우면 데몬을 교체한다(세션은 유지된다).
                if c.server_build != kiln_daemon::exe_build_id(&self.exe) && c.can_upgrade && !self.upgrade_requested && std::env::var_os("KILN_NO_AUTO_UPGRADE").is_none() {
                    self.upgrade_requested = true;
                    c.send(ClientMsg::Upgrade { req: c.next_req(), exe: self.exe.to_string_lossy().into_owned() });
                    self.events.push(ConnEvent::Upgrading);
                    self.state = State::Disconnected { since: Instant::now(), last_error: "upgrading daemon".into() };
                    return;
                }
                c.send(ClientMsg::ListSessions { req: c.next_req() });
                for (sid, (cols, rows)) in &self.attached {
                    c.send(ClientMsg::Attach { session: *sid, cols: *cols, rows: *rows });
                }
                self.client = Some(c);
                self.state = State::Connected;
                self.events.push(ConnEvent::Connected);
            }
            Err(e) => {
                self.state = State::Disconnected { since: Instant::now(), last_error: e.to_string() };
            }
        }
    }

    /// 매 프레임 호출: 서버 메시지를 처리하고 끊겼으면 재연결한다.
    pub fn pump(&mut self) {
        let mut disconnected = false;
        let mut msgs = Vec::new();
        if let Some(c) = &self.client {
            loop {
                match c.rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for m in msgs {
            self.handle(m);
        }
        if disconnected {
            self.client = None;
            self.state = State::Disconnected { since: Instant::now(), last_error: "connection lost".into() };
        }
        if self.client.is_none() {
            let due = self.last_attempt.is_none_or(|t| t.elapsed() > Duration::from_millis(150));
            if due {
                self.try_connect();
            }
            self.ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn handle(&mut self, m: ServerMsg) {
        match m {
            ServerMsg::Frame(f) => {
                self.screens.entry(f.session).or_default().apply(f);
            }
            ServerMsg::Sessions { sessions, .. } => {
                self.infos = sessions.into_iter().map(|s| (s.id, s)).collect();
                self.sessions_listed = true;
            }
            ServerMsg::SessionUpdated(i) => {
                self.infos.insert(i.id, i);
            }
            ServerMsg::SessionExited { session, code } => {
                if let Some(i) = self.infos.get_mut(&session) {
                    i.exited = Some(code.unwrap_or(-1));
                }
                if code.is_none() {
                    self.infos.remove(&session);
                }
                self.events.push(ConnEvent::Exited { session, code });
            }
            ServerMsg::Notification { session, title, body } => self.events.push(ConnEvent::Notification { session, title, body }),
            ServerMsg::Clipboard { text, .. } => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
            ServerMsg::Created { req, session } => self.events.push(ConnEvent::Created { req, session }),
            ServerMsg::Error { message, .. } => self.events.push(ConnEvent::Error(message)),
            ServerMsg::Upgrading => self.events.push(ConnEvent::Upgrading),
            ServerMsg::SearchResult { found, .. } => self.events.push(ConnEvent::SearchResult { found }),
            ServerMsg::Hello { .. } | ServerMsg::Text { .. } | ServerMsg::Pong { .. } => {}
        }
    }

    pub fn send(&self, m: ClientMsg) {
        if let Some(c) = &self.client {
            c.send(m);
        }
    }

    pub fn create(&mut self, spec: SpawnSpec) -> Option<u32> {
        let c = self.client.as_ref()?;
        let req = c.next_req();
        c.send(ClientMsg::Create { req, spec });
        Some(req)
    }

    pub fn attach(&mut self, sid: SessionId, cols: u16, rows: u16) {
        if self.attached.get(&sid) != Some(&(cols, rows)) {
            let first = !self.attached.contains_key(&sid);
            self.attached.insert(sid, (cols, rows));
            if first {
                self.send(ClientMsg::Attach { session: sid, cols, rows });
            } else {
                self.send(ClientMsg::Resize { session: sid, cols, rows });
            }
        }
    }

    pub fn detach(&mut self, sid: SessionId) {
        if self.attached.remove(&sid).is_some() {
            self.send(ClientMsg::Detach { session: sid });
        }
    }

    pub fn input(&self, sid: SessionId, data: Vec<u8>) {
        if !data.is_empty() {
            self.send(ClientMsg::Input { session: sid, data });
        }
    }

    pub fn kill(&mut self, sid: SessionId) {
        self.detach(sid);
        self.send(ClientMsg::Kill { session: sid });
        self.screens.remove(&sid);
        self.infos.remove(&sid);
    }

    pub fn is_alive(&self, sid: SessionId) -> bool {
        self.infos.get(&sid).is_some_and(|i| i.exited.is_none())
    }

    pub fn exists(&self, sid: SessionId) -> bool {
        self.infos.contains_key(&sid)
    }

    pub fn next_req(&self) -> u32 {
        self.client.as_ref().map(|c| c.next_req()).unwrap_or(0)
    }
}

/// 데몬을 실행할 파일. `KILN_EXE` 가 있으면 그 경로를 쓴다.
pub fn daemon_exe() -> std::path::PathBuf {
    std::env::var_os("KILN_EXE").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::current_exe().unwrap_or_default())
}
