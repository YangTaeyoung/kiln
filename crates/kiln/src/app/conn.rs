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
    pub images: Vec<ImagePlacement>,
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
        self.images = f.images;
    }
}

#[derive(Debug, Clone)]
pub enum ConnEvent {
    Activity { session: SessionId, activity: AgentActivity },
    Notification { session: SessionId, title: String, body: String },
    Exited { session: SessionId, code: Option<i32> },
    Created { req: u32, session: SessionId },
    Error { req: u32, message: String },
    Connected,
    Upgrading,
    SearchResult { found: bool },
    /// `read_text` 요청에 대한 세션 화면 텍스트.
    SessionText { session: SessionId, text: String },
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
    pub telemetry: HashMap<SessionId, SessionTelemetry>,
    pub command_outputs: HashMap<(SessionId, u64), (String, bool)>,
    /// 붙어 있는 세션과 요청한 크기.
    attached: HashMap<SessionId, (u16, u16)>,
    /// Explicitly closed sessions stay hidden from late PTY exit/update events.
    closing_sessions: std::collections::HashSet<SessionId>,
    pub events: Vec<ConnEvent>,
    ctx: egui::Context,
    socket: String,
    exe: std::path::PathBuf,
    last_attempt: Option<Instant>,
    pub daemon_pid: u32,
    pub daemon_build: String,
    upgrade_requested: bool,
    pub sessions_listed: bool,
    /// (세션, 이미지 id) → 텍스처.
    pub textures: HashMap<(SessionId, u32), egui::TextureHandle>,
    /// 응답이 오면 클립보드로 복사할 ReadRange 요청.
    pending_copy: std::collections::HashSet<u32>,
    pending_text: HashMap<u32, SessionId>,
    cell_px: (u16, u16),
    terminal_focus_candidate: Option<(SessionId, egui::ViewportId, egui::Id)>,
    reported_terminal_focus: Option<SessionId>,
}

impl Conn {
    pub fn new(ctx: egui::Context) -> Self {
        let mut c = Conn {
            client: None,
            state: State::Disconnected { since: Instant::now(), last_error: String::new() },
            screens: HashMap::new(),
            infos: HashMap::new(),
            telemetry: HashMap::new(),
            command_outputs: HashMap::new(),
            attached: HashMap::new(),
            closing_sessions: Default::default(),
            events: Vec::new(),
            ctx,
            socket: socket_name(),
            exe: daemon_exe(),
            last_attempt: None,
            daemon_pid: 0,
            daemon_build: String::new(),
            upgrade_requested: false,
            sessions_listed: false,
            textures: HashMap::new(),
            pending_copy: Default::default(),
            pending_text: HashMap::new(),
            cell_px: (0, 0),
            terminal_focus_candidate: None,
            reported_terminal_focus: None,
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
            telemetry: HashMap::new(),
            command_outputs: HashMap::new(),
            attached: HashMap::new(),
            closing_sessions: Default::default(),
            events: Vec::new(),
            ctx,
            socket: String::new(),
            exe: Default::default(),
            last_attempt: Some(Instant::now() + Duration::from_secs(3600)),
            daemon_pid: 0,
            daemon_build: String::new(),
            upgrade_requested: false,
            sessions_listed: false,
            textures: HashMap::new(),
            pending_copy: Default::default(),
            pending_text: HashMap::new(),
            cell_px: (0, 0),
            terminal_focus_candidate: None,
            reported_terminal_focus: None,
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
                if self.daemon_pid != 0 && self.daemon_pid != c.server_pid { self.closing_sessions.clear(); }
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
                if self.cell_px != (0, 0) {
                    c.send(ClientMsg::CellSize { width: self.cell_px.0, height: self.cell_px.1 });
                }
                // 재연결 후 이미지를 다시 받는다.
                self.textures.clear();
                // Command ids are local to the daemon lifetime, never reuse stale output after reconnect.
                self.command_outputs.clear();
                for (sid, (cols, rows)) in &self.attached {
                    c.send(ClientMsg::Attach { session: *sid, cols: *cols, rows: *rows });
                }
                for session in &self.closing_sessions { c.send(ClientMsg::Kill { session:*session }); }
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
            self.reported_terminal_focus = None;
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
                if self.closing_sessions.contains(&f.session) { return; }
                self.screens.entry(f.session).or_default().apply(f);
            }
            ServerMsg::Sessions { sessions, .. } => {
                self.infos = sessions.into_iter().filter(|s|!self.closing_sessions.contains(&s.id)).map(|s| (s.id, s)).collect();
                self.sessions_listed = true;
            }
            ServerMsg::SessionUpdated(i) => {
                if self.closing_sessions.contains(&i.id) { return; }
                self.infos.insert(i.id, i);
            }
            ServerMsg::SessionExited { session, code } => {
                if self.closing_sessions.contains(&session) { return; }
                if let Some(i) = self.infos.get_mut(&session) {
                    i.exited = Some(code.unwrap_or(-1));
                }
                if code.is_none() {
                    self.closing_sessions.insert(session);
                    self.infos.remove(&session);
                }
                self.events.push(ConnEvent::Exited { session, code });
            }
            ServerMsg::Notification { session, title, body } => {
                if !self.closing_sessions.contains(&session) { self.events.push(ConnEvent::Notification { session, title, body }); }
            }
            ServerMsg::Clipboard { text, .. } => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
            ServerMsg::Created { req, session } => self.events.push(ConnEvent::Created { req, session }),
            ServerMsg::Error { req, message } => self.events.push(ConnEvent::Error { req, message }),
            ServerMsg::Upgrading => self.events.push(ConnEvent::Upgrading),
            ServerMsg::SearchResult { found, .. } => self.events.push(ConnEvent::SearchResult { found }),
            ServerMsg::Text { req, text } => {
                if self.pending_copy.remove(&req) {
                    self.ctx.copy_text(text);
                } else if let Some(session) = self.pending_text.remove(&req) {
                    self.events.push(ConnEvent::SessionText { session, text });
                }
            }
            ServerMsg::SessionTelemetry { session, telemetry } => {
                if self.closing_sessions.contains(&session) { return; }
                let old = self.telemetry.get(&session).map(|s|s.activity).unwrap_or_default();
                if old != telemetry.activity { self.events.push(ConnEvent::Activity {session,activity:telemetry.activity}); }
                self.command_outputs.retain(|(sid,id),_| *sid != session || telemetry.commands.iter().any(|c| c.id == *id && c.output_available));
                self.telemetry.insert(session, telemetry);
            },
            ServerMsg::CommandOutput { session, command, text, truncated, .. } => {
                if self.command_outputs.len() >= 40 { self.command_outputs.clear(); }
                self.command_outputs.insert((session, command), (text, truncated));
            },
            ServerMsg::Image { session, id, width, height, rgba } => {
                if rgba.len() == (width * height * 4) as usize {
                    let img = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
                    let tex = self.ctx.load_texture(format!("kiln-img-{session}-{id}"), img, egui::TextureOptions::LINEAR);
                    self.textures.insert((session, id), tex);
                }
            }
            ServerMsg::Hello { .. } | ServerMsg::Pong { .. } => {}
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

    /// Only canvases actually owning keyboard focus may claim the frame.
    pub fn begin_terminal_focus_frame(&mut self) { self.terminal_focus_candidate = None; }

    pub fn note_terminal_focus(&mut self, session: SessionId, ctx: &egui::Context, widget: egui::Id) {
        self.terminal_focus_candidate = Some((session, ctx.viewport_id(), widget));
    }

    /// Current keyboard owner, independent of a CLI opting into focus reports.
    /// Missing child-window focus information is intentionally treated as unread.
    pub fn observed_terminal(&self, ctx:&egui::Context) -> Option<(SessionId,egui::ViewportId)> {
        let (session,viewport,widget)=self.terminal_focus_candidate?;
        let focused=if viewport==ctx.viewport_id() {
            ctx.input(|i|i.focused) && ctx.memory(|m|m.focused())==Some(widget) && !egui::Popup::is_any_open(ctx)
        } else {
            ctx.input(|i|i.raw.viewports.get(&viewport).and_then(|v|v.focused)==Some(true))
        };
        focused.then_some((session,viewport))
    }

    pub fn finish_terminal_focus_frame(&mut self, ctx: &egui::Context) {
        if !self.is_connected() { self.reported_terminal_focus = None; return; }
        let next = self.terminal_focus_candidate.filter(|(_, viewport, widget)| {
            // Root overlays can take focus after the terminal was drawn. An immediate
            // child viewport has already validated its own input and focus state.
            *viewport != ctx.viewport_id() || (ctx.input(|i| i.focused)
                && ctx.memory(|m| m.focused()) == Some(*widget) && !egui::Popup::is_any_open(ctx))
        }).map(|(session, _, _)| session);
        for (session, focused) in self.terminal_focus_changes(next) {
            self.input(session, if focused { b"\x1b[I" } else { b"\x1b[O" }.to_vec());
        }
    }

    fn terminal_focus_changes(&mut self, next: Option<SessionId>) -> Vec<(SessionId, bool)> {
        let enabled = |session| self.screens.get(&session).is_some_and(|screen| screen.mode & mode::FOCUS_EVENTS != 0);
        let next = next.filter(|session| enabled(*session));
        if next == self.reported_terminal_focus { return Vec::new(); }
        let mut events = Vec::with_capacity(2);
        if let Some(previous) = self.reported_terminal_focus.filter(|session| enabled(*session)) { events.push((previous, false)); }
        if let Some(session) = next { events.push((session, true)); }
        self.reported_terminal_focus = next;
        events
    }

    /// 그리드 구간 텍스트를 요청하고, 응답이 오면 클립보드에 복사한다.
    pub fn copy_range(&mut self, session: SessionId, start: (i32, u16), end: (i32, u16)) {
        let req = self.next_req();
        self.pending_copy.insert(req);
        self.send(ClientMsg::ReadRange { req, session, start, end });
    }

    pub fn read_command_output(&mut self, session: SessionId, command: u64) {
        let req = self.next_req();
        self.send(ClientMsg::ReadCommandOutput { req, session, command });
    }

    /// 세션의 보이는 화면 텍스트를 요청한다. 응답은 `ConnEvent::SessionText` 로 온다.
    pub fn read_text(&mut self, session: SessionId) {
        if self.client.is_none() {
            return;
        }
        let req = self.next_req();
        self.pending_text.insert(req, session);
        self.send(ClientMsg::ReadText { req, session, history: 0 });
    }

    /// 셀 픽셀 크기가 바뀌면 데몬에 알린다(이미지 크기 계산용).
    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        if self.cell_px != (w, h) {
            self.cell_px = (w, h);
            self.send(ClientMsg::CellSize { width: w, height: h });
        }
    }

    pub fn kill(&mut self, sid: SessionId) -> bool {
        if !self.is_connected() { return false; }
        self.closing_sessions.insert(sid);
        self.detach(sid);
        self.send(ClientMsg::Kill { session: sid });
        self.screens.remove(&sid);
        self.infos.remove(&sid);
        self.textures.retain(|(s, _), _| *s != sid);
        true
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

#[cfg(test)]
mod terminal_focus_tests {
    use super::*;
    #[test]
    fn observation_requires_current_focus_and_known_child_window_focus() {
        let ctx=egui::Context::default();
        let mut conn=Conn::offline(ctx.clone());
        let widget=egui::Id::new("terminal");
        let mut output=ctx.run_ui(egui::RawInput::default(),|ui| {
            let ctx=ui.ctx();
            ctx.memory_mut(|m|m.request_focus(widget));
            conn.note_terminal_focus(7,ctx,widget);
            // No screen or FOCUS_EVENTS mode is needed to observe a terminal.
            assert_eq!(conn.observed_terminal(ctx),Some((7,egui::ViewportId::ROOT)));
            ctx.memory_mut(|m|m.request_focus(egui::Id::new("search")));
            assert_eq!(conn.observed_terminal(ctx),None);
        });output.textures_delta.clear();
        let child=egui::ViewportId::from_hash_of("quick-terminal");
        conn.terminal_focus_candidate=Some((7,child,widget));
        for focused in [None,Some(false),Some(true)] {
            let mut input=egui::RawInput::default();
            input.viewports.entry(child).or_default().focused=focused;
            let mut output=ctx.run_ui(input,|ui| {
                let ctx=ui.ctx();
                assert_eq!(conn.observed_terminal(ctx),focused.filter(|v|*v).map(|_|(7,child)));
            });output.textures_delta.clear();
        }
        conn.begin_terminal_focus_frame();
        assert_eq!(conn.observed_terminal(&ctx),None);
    }

    #[test]
    fn explicit_close_ignores_late_exit_updates_and_list_snapshots() {
        let mut conn=Conn::offline(egui::Context::default());
        conn.state=State::Connected;
        let info=SessionInfo { id:42, ..Default::default() };
        conn.infos.insert(42,info.clone());
        assert!(conn.kill(42));
        for message in [
            ServerMsg::SessionExited { session:42,code:None },
            ServerMsg::SessionUpdated(SessionInfo { exited:Some(1),..info.clone() }),
            ServerMsg::SessionExited { session:42,code:Some(1) },
            ServerMsg::Sessions { req:3,sessions:vec![info] },
            ServerMsg::Notification { session:42,title:"late".into(),body:String::new() },
        ] { conn.handle(message); }
        assert!(!conn.exists(42));
        assert!(conn.events.is_empty(),"intentional close must not report a failure or notification");
    }

    #[test]
    fn disconnected_kill_preserves_local_session_and_error_preserves_request_id() {
        let mut conn=Conn::offline(egui::Context::default());
        conn.infos.insert(7,SessionInfo { id:7,..Default::default() });
        assert!(!conn.kill(7));assert!(conn.exists(7));
        conn.handle(ServerMsg::Error { req:19,message:"spawn failed".into() });
        assert!(matches!(conn.events.last(),Some(ConnEvent::Error { req:19,message }) if message=="spawn failed"));
    }

    #[test]
    fn external_kill_tombstones_the_session_before_late_watcher_updates() {
        let mut conn=Conn::offline(egui::Context::default());
        conn.infos.insert(9,SessionInfo { id:9,..Default::default() });
        conn.handle(ServerMsg::SessionExited { session:9,code:None });
        conn.handle(ServerMsg::SessionUpdated(SessionInfo { id:9,exited:Some(1),..Default::default() }));
        conn.handle(ServerMsg::SessionExited { session:9,code:Some(1) });
        assert!(!conn.exists(9));
        assert_eq!(conn.events.len(),1);
        assert!(matches!(conn.events[0],ConnEvent::Exited { session:9,code:None }));
    }
    #[test]
    fn focus_reports_follow_split_tab_and_search_transitions_without_duplicates() {
        let mut conn = Conn::offline(egui::Context::default());
        for id in [1, 2] { conn.screens.insert(id, Screen { mode: mode::FOCUS_EVENTS, ..Default::default() }); }
        assert_eq!(conn.terminal_focus_changes(Some(1)), vec![(1, true)]);
        assert!(conn.terminal_focus_changes(Some(1)).is_empty());
        assert_eq!(conn.terminal_focus_changes(Some(2)), vec![(1, false), (2, true)]);
        // Search, editor, hidden tab, or an unfocused application has no canvas claimant.
        assert_eq!(conn.terminal_focus_changes(None), vec![(2, false)]);
        assert!(conn.terminal_focus_changes(None).is_empty());
        conn.screens.get_mut(&1).unwrap().mode = 0;
        assert!(conn.terminal_focus_changes(Some(1)).is_empty());
        // An already-focused CLI can enable reporting later.
        conn.screens.get_mut(&1).unwrap().mode = mode::FOCUS_EVENTS;
        assert_eq!(conn.terminal_focus_changes(Some(1)), vec![(1, true)]);
    }
}
