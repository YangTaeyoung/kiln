//! 데몬 서버: 세션(PTY + 에뮬레이터) 소유, 클라이언트 연결 처리, 화면 프레임 전송.

#[cfg(unix)]
use crate::emu::Dump;
use crate::emu::Emu;
use crate::images::{Decoded, ImageScanner, ImageState, Segment};
use crate::osc::{OscEvent, OscScanner};
use crate::pty::{Pty, ReadResult};
use crate::ptyhost::{AnyPty, AnyReader, HostPty, HostRecord};
use crate::transport::{Conn, Listener};
use crate::{build_id, procinfo};
use alacritty_terminal::event::Event;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use kiln_proto::*;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Session {
    pub id: SessionId,
    pty: AnyPty,
    emu: Mutex<Emu>,
    input: Sender<Vec<u8>>,
    info: Mutex<SessionInfo>,
    generation: AtomicU64,
    images: Mutex<SessionImages>,
}

/// 세션이 표시 중인 이미지. 오래된 것부터 버린다.
#[derive(Default)]
struct SessionImages {
    map: HashMap<u32, Arc<Decoded>>,
    order: std::collections::VecDeque<u32>,
    state: ImageState,
    next: u32,
}

const MAX_IMAGES: usize = 64;

impl SessionImages {
    fn insert(&mut self, d: Decoded) -> u32 {
        self.next += 1;
        let id = self.next;
        self.map.insert(id, Arc::new(d));
        self.order.push_back(id);
        while self.order.len() > MAX_IMAGES {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        id
    }

    fn max_rows(&self) -> usize {
        self.map.values().map(|d| d.rows as usize).max().unwrap_or(0)
    }
}

struct Client {
    id: u64,
    out: Sender<ServerMsg>,
    wake: Sender<()>,
    attached: Mutex<HashMap<SessionId, AttachState>>,
}

#[derive(Default)]
struct AttachState {
    hashes: Vec<u64>,
    cols: u16,
    generation: u64,
    cursor: Option<Cursor>,
    mode: u32,
    offset: u32,
    full: bool,
    images: Vec<ImagePlacement>,
    images_sent: std::collections::HashSet<u32>,
}

pub struct Daemon {
    sessions: RwLock<HashMap<SessionId, Arc<Session>>>,
    clients: Mutex<Vec<Arc<Client>>>,
    next_session: AtomicU64,
    next_client: AtomicU64,
    upgrading: AtomicBool,
    readers_running: AtomicUsize,
    socket: String,
    cell_w: AtomicU32,
    cell_h: AtomicU32,
}

/// 업그레이드 상태 파일 v1(접두 없음).
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct UpgradeState {
    next_session: u64,
    listener_fd: i32,
    sessions: Vec<SavedSession>,
}

/// 업그레이드 상태 파일 v2: `STATE_MAGIC_V2` 접두 + 이 구조체.
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct UpgradeStateV2 {
    base: UpgradeState,
    images: Vec<SavedImages>,
}

#[derive(Serialize, Deserialize, Default)]
struct SavedImages {
    session: SessionId,
    next: u32,
    images: Vec<(u32, Decoded)>,
}

#[cfg(unix)]
const STATE_MAGIC_V2: &[u8] = b"KILNUP2\n";

/// 호스트 방식 업그레이드 상태 파일 접두.
const STATE_MAGIC_HOSTED: &[u8] = b"KILNHS1\n";

#[derive(Serialize, Deserialize)]
struct SavedHosted {
    info: SessionInfo,
    endpoint: String,
    dump: crate::emu::Dump,
}

#[derive(Serialize, Deserialize)]
struct HostedState {
    next_session: u64,
    sessions: Vec<SavedHosted>,
    images: Vec<SavedImages>,
}

impl SessionImages {
    fn save(&self, session: SessionId) -> SavedImages {
        SavedImages { session, next: self.next, images: self.order.iter().filter_map(|id| self.map.get(id).map(|d| (*id, (**d).clone()))).collect() }
    }

    fn load(&mut self, saved: SavedImages) {
        self.next = saved.next;
        for (id, d) in saved.images {
            self.map.insert(id, Arc::new(d));
            self.order.push_back(id);
        }
    }
}

#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct SavedSession {
    info: SessionInfo,
    fd: i32,
    pid: i32,
    dump: Dump,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Daemon {
    fn new(socket: String) -> Arc<Self> {
        Arc::new(Daemon {
            sessions: RwLock::new(HashMap::new()),
            clients: Mutex::new(Vec::new()),
            next_session: AtomicU64::new(1),
            next_client: AtomicU64::new(1),
            upgrading: AtomicBool::new(false),
            readers_running: AtomicUsize::new(0),
            socket,
            cell_w: AtomicU32::new(8),
            cell_h: AtomicU32::new(17),
        })
    }

    fn broadcast(&self, msg: ServerMsg) {
        for c in self.clients.lock().iter() {
            let _ = c.out.try_send(msg.clone());
        }
    }

    fn wake_attached(&self, sid: SessionId) {
        for c in self.clients.lock().iter() {
            if c.attached.lock().contains_key(&sid) {
                let _ = c.wake.try_send(());
            }
        }
    }

    fn session(&self, sid: SessionId) -> Option<Arc<Session>> {
        self.sessions.read().get(&sid).cloned()
    }

    fn list(&self) -> Vec<SessionInfo> {
        let mut v: Vec<SessionInfo> = self.sessions.read().values().map(|s| s.info.lock().clone()).collect();
        v.sort_by_key(|i| i.id);
        v
    }

    fn create(self: &Arc<Self>, mut spec: SpawnSpec) -> std::io::Result<SessionId> {
        let id = self.next_session.fetch_add(1, Ordering::SeqCst);
        if spec.cols == 0 {
            spec.cols = 80;
        }
        if spec.rows == 0 {
            spec.rows = 24;
        }
        let pty = if crate::ptyhost::host_mode() {
            let exe = std::env::current_exe()?;
            AnyPty::Host(HostPty::spawn(&spec, id, &exe, &self.socket)?)
        } else {
            AnyPty::Native(Pty::spawn(&spec, id)?)
        };
        let info = SessionInfo {
            id,
            pid: pty.pid(),
            name: spec.name.clone(),
            title: String::new(),
            cwd: spec.cwd.clone(),
            fg_process: None,
            workspace: spec.workspace.clone(),
            cols: spec.cols,
            rows: spec.rows,
            exited: None,
            created_unix: now_unix(),
            attention: false,
            last_notification: None,
        };
        self.install(id, pty, Emu::new(spec.cols, spec.rows), info)?;
        self.save_registry();
        Ok(id)
    }

    fn install(self: &Arc<Self>, id: SessionId, pty: AnyPty, emu: Emu, info: SessionInfo) -> std::io::Result<()> {
        let (tx, rx) = unbounded::<Vec<u8>>();
        let mut writer = pty.writer()?;
        let reader = pty.reader()?;
        let sess = Arc::new(Session { id, pty, emu: Mutex::new(emu), input: tx, info: Mutex::new(info), generation: AtomicU64::new(1), images: Mutex::new(SessionImages::default()) });
        self.sessions.write().insert(id, sess.clone());
        std::thread::Builder::new().name(format!("pty-w-{id}")).spawn(move || {
            while let Ok(data) = rx.recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        })?;
        if sess.info.lock().exited.is_none() {
            let d = self.clone();
            self.readers_running.fetch_add(1, Ordering::SeqCst);
            std::thread::Builder::new().name(format!("pty-r-{id}")).spawn(move || d.read_loop(sess, reader))?;
        }
        self.broadcast(ServerMsg::SessionUpdated(self.session(id).map(|s| s.info.lock().clone()).unwrap_or_default()));
        Ok(())
    }

    fn read_loop(self: Arc<Self>, sess: Arc<Session>, mut reader: AnyReader) {
        let mut buf = vec![0u8; 64 * 1024];
        let mut osc = OscScanner::default();
        let mut osc_events = Vec::new();
        let mut img_scan = ImageScanner::default();
        let mut eof = false;
        loop {
            if self.upgrading.load(Ordering::SeqCst) {
                break;
            }
            match reader.read_timeout(&mut buf, 100) {
                Ok(ReadResult::Data(n)) => {
                    let data = &buf[..n];
                    osc.feed(data, &mut osc_events);
                    let mut events = Vec::new();
                    for seg in img_scan.feed(data) {
                        match seg {
                            Segment::Bytes(a, b) => events.extend(sess.emu.lock().advance(&data[a..b])),
                            Segment::Image(cmd) => self.handle_image(&sess, cmd),
                        }
                    }
                    sess.generation.fetch_add(1, Ordering::SeqCst);
                    self.handle_events(&sess, events, &mut osc_events);
                    self.wake_attached(sess.id);
                }
                Ok(ReadResult::Timeout) => continue,
                Ok(ReadResult::Detached) => break,
                Ok(ReadResult::Eof) | Err(_) => {
                    eof = true;
                    break;
                }
            }
        }
        self.readers_running.fetch_sub(1, Ordering::SeqCst);
        if !eof {
            return;
        }
        let mut code = None;
        for _ in 0..50 {
            code = sess.pty.try_wait();
            if code.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let code = code.unwrap_or(-1);
        let info = {
            let mut i = sess.info.lock();
            i.exited = Some(code);
            i.clone()
        };
        sess.generation.fetch_add(1, Ordering::SeqCst);
        self.broadcast(ServerMsg::SessionUpdated(info));
        self.broadcast(ServerMsg::SessionExited { session: sess.id, code: Some(code) });
        self.wake_attached(sess.id);
        self.save_registry();
    }

    /// 호스트 방식 세션 목록을 파일에 남긴다(데몬이 비정상 종료되면 다음 데몬이 입양한다).
    fn save_registry(&self) {
        if !crate::ptyhost::host_mode() {
            return;
        }
        let records: Vec<HostRecord> = self
            .sessions
            .read()
            .values()
            .filter_map(|s| {
                let h = s.pty.host()?;
                let i = s.info.lock();
                if i.exited.is_some() {
                    return None;
                }
                Some(HostRecord { session: s.id, endpoint: h.endpoint.clone(), name: i.name.clone(), workspace: i.workspace.clone(), cwd: i.cwd.clone(), created_unix: i.created_unix })
            })
            .collect();
        crate::ptyhost::save_registry(&self.socket, &records);
    }

    /// 레지스트리에 남은 호스트에 다시 붙는다. 최근 출력을 재생해 화면을 되살린다.
    fn adopt_orphans(self: &Arc<Self>) {
        let mut max_id = 0;
        for r in crate::ptyhost::load_registry(&self.socket) {
            let Ok(h) = HostPty::attach(&r.endpoint, true) else { continue };
            max_id = max_id.max(r.session);
            let info = SessionInfo {
                id: r.session,
                pid: h.pid(),
                name: r.name,
                title: String::new(),
                cwd: r.cwd,
                fg_process: None,
                workspace: r.workspace,
                cols: 120,
                rows: 32,
                exited: None,
                created_unix: r.created_unix,
                attention: false,
                last_notification: None,
            };
            // 크기를 한 번 바꿨다 되돌려 전체 화면 앱이 다시 그리게 한다(SIGWINCH).
            let _ = h.resize(119, 32);
            let _ = h.resize(120, 32);
            log::info!("adopted session {} from {}", r.session, r.endpoint);
            let _ = self.install(r.session, AnyPty::Host(h), Emu::new(120, 32), info);
        }
        if max_id > 0 {
            self.next_session.fetch_max(max_id + 1, Ordering::SeqCst);
        }
        self.save_registry();
    }

    fn geometry(&self, sess: &Session) -> crate::images::Geometry {
        let (cols, rows) = sess.emu.lock().size();
        crate::images::Geometry { cell_w: self.cell_w.load(Ordering::Relaxed), cell_h: self.cell_h.load(Ordering::Relaxed), cols, rows }
    }

    fn handle_image(&self, sess: &Session, cmd: crate::images::ImageCmd) {
        use crate::images::Action;
        let g = self.geometry(sess);
        let action = sess.images.lock().state.handle(cmd, &g);
        let show = |d: Decoded| {
            let (cols, rows, cursor) = (d.cols, d.rows, d.cursor);
            let id = sess.images.lock().insert(d);
            sess.emu.lock().place_image(id, cols, rows, cursor);
        };
        match action {
            Action::None => {}
            Action::Show(d) => show(d),
            Action::Reply(r) => {
                let _ = sess.input.send(r);
            }
            Action::ShowAndReply(d, r) => {
                show(d);
                let _ = sess.input.send(r);
            }
            Action::DeleteAll => {
                let mut im = sess.images.lock();
                im.map.clear();
                im.order.clear();
            }
        }
    }

    fn handle_events(&self, sess: &Session, events: Vec<Event>, osc: &mut Vec<OscEvent>) {
        let mut updated = false;
        for e in events {
            match e {
                Event::PtyWrite(s) => {
                    let _ = sess.input.send(s.into_bytes());
                }
                Event::Title(t) => {
                    sess.info.lock().title = t;
                    updated = true;
                }
                Event::ResetTitle => {
                    sess.info.lock().title.clear();
                    updated = true;
                }
                Event::Bell => {
                    let mut i = sess.info.lock();
                    if !i.attention {
                        i.attention = true;
                        updated = true;
                    }
                }
                Event::ClipboardStore(_, text) => {
                    self.broadcast(ServerMsg::Clipboard { session: sess.id, text });
                }
                Event::TextAreaSizeRequest(f) => {
                    let (cols, rows) = sess.emu.lock().size();
                    let (cw, ch) = (self.cell_w.load(Ordering::Relaxed) as u16, self.cell_h.load(Ordering::Relaxed) as u16);
                    let ws = alacritty_terminal::event::WindowSize { num_lines: rows, num_cols: cols, cell_width: cw, cell_height: ch };
                    let _ = sess.input.send(f(ws).into_bytes());
                }
                Event::ColorRequest(idx, f) => {
                    let rgb = match idx {
                        256 => alacritty_terminal::vte::ansi::Rgb { r: 0xdc, g: 0xde, b: 0xe6 },
                        257 => alacritty_terminal::vte::ansi::Rgb { r: 0x16, g: 0x17, b: 0x1c },
                        _ => alacritty_terminal::vte::ansi::Rgb { r: 0x80, g: 0x80, b: 0x80 },
                    };
                    let _ = sess.input.send(f(rgb).into_bytes());
                }
                _ => {}
            }
        }
        for e in osc.drain(..) {
            match e {
                OscEvent::Notify { title, body } => {
                    {
                        let mut i = sess.info.lock();
                        i.attention = true;
                        i.last_notification = Some(if title.is_empty() { body.clone() } else { format!("{title}: {body}") });
                    }
                    updated = true;
                    self.broadcast(ServerMsg::Notification { session: sess.id, title, body });
                }
                OscEvent::Cwd(p) => {
                    let mut i = sess.info.lock();
                    if i.cwd.as_deref() != Some(&p) {
                        i.cwd = Some(p);
                        updated = true;
                    }
                }
            }
        }
        if updated {
            self.broadcast(ServerMsg::SessionUpdated(sess.info.lock().clone()));
        }
    }

    /// 1초마다 포그라운드 프로세스 이름과 작업 디렉토리를 갱신한다.
    fn monitor_loop(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            if self.upgrading.load(Ordering::SeqCst) {
                return;
            }
            let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
            for s in sessions {
                if s.info.lock().exited.is_some() {
                    continue;
                }
                let fg = s.pty.fg_pid();
                let name = fg.and_then(procinfo::display_name);
                let cwd = fg.and_then(procinfo::cwd).or_else(|| procinfo::cwd(s.pty.pid()));
                let mut changed = false;
                {
                    let mut i = s.info.lock();
                    if name.is_some() && i.fg_process != name {
                        i.fg_process = name;
                        changed = true;
                    }
                    if cwd.is_some() && i.cwd != cwd {
                        i.cwd = cwd;
                        changed = true;
                    }
                }
                if changed {
                    self.broadcast(ServerMsg::SessionUpdated(s.info.lock().clone()));
                }
            }
        }
    }

    fn handle_client(self: Arc<Self>, conn: Conn) {
        let Conn { mut reader, mut writer } = conn;
        let (out_tx, out_rx) = bounded::<ServerMsg>(1024);
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let client = Arc::new(Client {
            id: self.next_client.fetch_add(1, Ordering::SeqCst),
            out: out_tx,
            wake: wake_tx,
            attached: Mutex::new(HashMap::new()),
        });
        self.clients.lock().push(client.clone());
        let writer_thread = std::thread::spawn(move || {
            let mut buf = Vec::with_capacity(64 * 1024);
            while let Ok(msg) = out_rx.recv() {
                buf.clear();
                buf.extend_from_slice(&encode(&msg));
                while let Ok(more) = out_rx.try_recv() {
                    buf.extend_from_slice(&encode(&more));
                    if buf.len() > 1 << 20 {
                        break;
                    }
                }
                if writer.write_all(&buf).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        });
        let d = self.clone();
        let c2 = client.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let alive2 = alive.clone();
        std::thread::spawn(move || d.push_loop(c2, wake_rx, alive2));

        while let Ok(Some(msg)) = read_msg::<_, ClientMsg>(&mut reader) {
            if !self.clone().handle_msg(&client, msg) {
                break;
            }
        }
        alive.store(false, Ordering::SeqCst);
        let _ = client.wake.try_send(());
        self.clients.lock().retain(|c| c.id != client.id);
        drop(client);
        let _ = writer_thread;
    }

    /// 메시지 하나를 처리한다. false 면 연결을 닫는다.
    fn handle_msg(self: Arc<Self>, client: &Arc<Client>, msg: ClientMsg) -> bool {
        let reply = |m: ServerMsg| {
            let _ = client.out.send(m);
        };
        match msg {
            ClientMsg::Hello { .. } => reply(ServerMsg::Hello {
                proto: PROTO_VERSION,
                build: build_id().to_string(),
                pid: std::process::id(),
                can_upgrade: true,
            }),
            ClientMsg::Ping { req } => reply(ServerMsg::Pong { req }),
            ClientMsg::ListSessions { req } => reply(ServerMsg::Sessions { req, sessions: self.list() }),
            ClientMsg::Create { req, spec } => match self.create(spec) {
                Ok(session) => reply(ServerMsg::Created { req, session }),
                Err(e) => reply(ServerMsg::Error { req, message: format!("spawn failed: {e}") }),
            },
            ClientMsg::Attach { session, cols, rows } => {
                if let Some(s) = self.session(session) {
                    if cols > 0 && rows > 0 {
                        self.resize(&s, cols, rows);
                    }
                    client.attached.lock().insert(session, AttachState { full: true, ..Default::default() });
                    let _ = client.wake.try_send(());
                } else {
                    reply(ServerMsg::Error { req: 0, message: format!("no session {session}") });
                }
            }
            ClientMsg::Detach { session } => {
                client.attached.lock().remove(&session);
            }
            ClientMsg::Input { session, data } => {
                if let Some(s) = self.session(session) {
                    {
                        let mut emu = s.emu.lock();
                        if emu.display_offset() != 0 {
                            emu.scroll(ScrollTo::Bottom);
                            s.generation.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    let _ = s.input.send(data);
                    self.wake_attached(session);
                }
            }
            ClientMsg::Resize { session, cols, rows } => {
                if let Some(s) = self.session(session) {
                    self.resize(&s, cols, rows);
                    self.wake_attached(session);
                }
            }
            ClientMsg::Scroll { session, scroll } => {
                if let Some(s) = self.session(session) {
                    s.emu.lock().scroll(scroll);
                    s.generation.fetch_add(1, Ordering::SeqCst);
                    self.wake_attached(session);
                }
            }
            ClientMsg::Search { req, session, query, backward } => {
                let found = self.session(session).map(|s| {
                    let f = s.emu.lock().search(&query, backward);
                    s.generation.fetch_add(1, Ordering::SeqCst);
                    f
                });
                self.wake_attached(session);
                reply(ServerMsg::SearchResult { req, found: found.unwrap_or(false) });
            }
            ClientMsg::Kill { session } => {
                let removed = self.sessions.write().remove(&session);
                self.save_registry();
                if let Some(s) = removed {
                    if s.info.lock().exited.is_none() {
                        s.pty.kill();
                        let pty_session = s.clone();
                        std::thread::spawn(move || {
                            for _ in 0..50 {
                                if pty_session.pty.try_wait().is_some() {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        });
                    }
                    self.broadcast(ServerMsg::SessionExited { session, code: None });
                }
            }
            ClientMsg::Rename { session, name } => {
                if let Some(s) = self.session(session) {
                    s.info.lock().name = if name.is_empty() { None } else { Some(name) };
                    self.broadcast(ServerMsg::SessionUpdated(s.info.lock().clone()));
                }
            }
            ClientMsg::ClearAttention { session } => {
                if let Some(s) = self.session(session) {
                    let changed = {
                        let mut i = s.info.lock();
                        let c = i.attention;
                        i.attention = false;
                        c
                    };
                    if changed {
                        self.broadcast(ServerMsg::SessionUpdated(s.info.lock().clone()));
                    }
                }
            }
            ClientMsg::ReadRange { req, session, start, end } => match self.session(session) {
                Some(s) => reply(ServerMsg::Text { req, text: s.emu.lock().read_range(start, end) }),
                None => reply(ServerMsg::Error { req, message: format!("no session {session}") }),
            },
            ClientMsg::CellSize { width, height } => {
                self.cell_w.store(width.max(1) as u32, Ordering::Relaxed);
                self.cell_h.store(height.max(1) as u32, Ordering::Relaxed);
            }
            ClientMsg::ReadText { req, session, history } => match self.session(session) {
                Some(s) => reply(ServerMsg::Text { req, text: s.emu.lock().text(history as usize) }),
                None => reply(ServerMsg::Error { req, message: format!("no session {session}") }),
            },
            ClientMsg::Upgrade { req, exe } => {
                if let Err(e) = self.clone().upgrade(&exe) {
                    reply(ServerMsg::Error { req, message: format!("upgrade failed: {e}") });
                }
            }
            ClientMsg::Shutdown => {
                for s in self.sessions.read().values() {
                    s.pty.kill();
                }
                let _ = std::fs::remove_file(&self.socket);
                std::process::exit(0);
            }
        }
        true
    }

    fn resize(&self, s: &Session, cols: u16, rows: u16) {
        let mut emu = s.emu.lock();
        if emu.size() != (cols, rows) {
            emu.resize(cols, rows);
            let _ = s.pty.resize(cols, rows);
            let mut i = s.info.lock();
            i.cols = cols;
            i.rows = rows;
            s.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 깨어날 때마다 붙어 있는 세션의 변경된 줄만 보낸다. 최소 간격 4ms 로 합친다.
    fn push_loop(self: Arc<Self>, client: Arc<Client>, wake: Receiver<()>, alive: Arc<AtomicBool>) {
        let min_interval = Duration::from_millis(4);
        let mut last = Instant::now() - min_interval;
        while wake.recv().is_ok() {
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let since = last.elapsed();
            if since < min_interval {
                std::thread::sleep(min_interval - since);
            }
            last = Instant::now();
            let ids: Vec<SessionId> = client.attached.lock().keys().copied().collect();
            for sid in ids {
                let Some(sess) = self.session(sid) else { continue };
                let (frame, images) = {
                    let mut attached = client.attached.lock();
                    let Some(st) = attached.get_mut(&sid) else { continue };
                    build_frame(&sess, st)
                };
                for m in images {
                    if client.out.send(m).is_err() {
                        return;
                    }
                }
                if let Some(f) = frame
                    && client.out.send(ServerMsg::Frame(f)).is_err()
                {
                    return;
                }
            }
        }
    }

    #[cfg(unix)]
    fn upgrade(self: Arc<Self>, exe: &str) -> anyhow::Result<()> {
        if self.sessions.read().values().any(|s| s.pty.host().is_some()) || crate::ptyhost::host_mode() {
            return self.upgrade_hosted(exe);
        }
        anyhow::ensure!(std::path::Path::new(exe).exists(), "{exe} not found");
        let listener_fd = LISTENER_FD.load(Ordering::SeqCst);
        anyhow::ensure!(listener_fd >= 0, "listener fd unknown");
        self.broadcast(ServerMsg::Upgrading);
        self.upgrading.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.readers_running.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut saved = Vec::new();
        let mut images = Vec::new();
        let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
        for s in &sessions {
            let dump = s.emu.lock().dump();
            let fd = s.pty.native().map(|p| p.raw_fd()).unwrap_or(-1);
            crate::pty::set_cloexec(fd, false)?;
            saved.push(SavedSession { info: s.info.lock().clone(), fd, pid: s.pty.pid() as i32, dump });
            images.push(s.images.lock().save(s.id));
        }
        crate::pty::set_cloexec(listener_fd, false)?;
        let n = saved.len();
        let state = UpgradeStateV2 { base: UpgradeState { next_session: self.next_session.load(Ordering::SeqCst), listener_fd, sessions: saved }, images };
        let path = std::path::Path::new(&self.socket).with_file_name(format!("upgrade-{}.state", std::process::id()));
        let mut bytes = STATE_MAGIC_V2.to_vec();
        bytes.extend(postcard::to_stdvec(&state)?);
        std::fs::write(&path, bytes)?;
        // 클라이언트 연결을 닫아 재연결을 유도한다.
        self.clients.lock().clear();
        log::info!("exec {exe} for upgrade with {n} sessions");
        let err = exec(exe, &["daemon", "--foreground", "--restore", &path.to_string_lossy()]);
        // exec 실패: 원래 상태로 되돌린다.
        let _ = std::fs::remove_file(&path);
        self.upgrading.store(false, Ordering::SeqCst);
        for s in sessions.iter().filter(|s| s.info.lock().exited.is_none()) {
            if let Some(p) = s.pty.native() {
                let _ = crate::pty::set_cloexec(p.raw_fd(), true);
            }
            let reader = s.pty.reader()?;
            let d = self.clone();
            let s2 = s.clone();
            self.readers_running.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || d.read_loop(s2, reader));
        }
        Err(anyhow::anyhow!("exec failed: {err}"))
    }

    #[cfg(windows)]
    fn upgrade(self: Arc<Self>, exe: &str) -> anyhow::Result<()> {
        self.upgrade_hosted(exe)
    }

    /// 호스트 방식 업그레이드: 호스트에서 떨어진 뒤 화면 상태를 저장하고, 새 데몬을 띄우고 종료한다.
    /// 새 데몬은 이 프로세스가 끝나기를 기다렸다가 같은 이름으로 리슨하고 호스트에 다시 붙는다.
    fn upgrade_hosted(self: Arc<Self>, exe: &str) -> anyhow::Result<()> {
        anyhow::ensure!(std::path::Path::new(exe).exists(), "{exe} not found");
        #[cfg(windows)]
        let exe_run = crate::client::daemon_copy(std::path::Path::new(exe)).unwrap_or_else(|| exe.into());
        #[cfg(unix)]
        let exe_run = std::path::PathBuf::from(exe);
        self.broadcast(ServerMsg::Upgrading);
        self.upgrading.store(true, Ordering::SeqCst);
        let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
        for s in &sessions {
            if let Some(h) = s.pty.host() {
                h.detach();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.readers_running.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut saved = Vec::new();
        let mut images = Vec::new();
        for s in &sessions {
            let Some(h) = s.pty.host() else { continue };
            saved.push(SavedHosted { info: s.info.lock().clone(), endpoint: h.endpoint.clone(), dump: s.emu.lock().dump() });
            images.push(s.images.lock().save(s.id));
        }
        let state = HostedState { next_session: self.next_session.load(Ordering::SeqCst), sessions: saved, images };
        let marker = crate::ptyhost::handover_marker(&self.socket);
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, std::process::id().to_string());
        let path = std::env::temp_dir().join(format!("kiln-upgrade-{}.state", std::process::id()));
        let mut bytes = STATE_MAGIC_HOSTED.to_vec();
        bytes.extend(postcard::to_stdvec(&state)?);
        std::fs::write(&path, bytes)?;
        let mut cmd = std::process::Command::new(&exe_run);
        cmd.args(["daemon", "--socket", &self.socket, "--restore", &path.to_string_lossy(), "--wait-pid", &std::process::id().to_string()]);
        cmd.env("KILN_SOCKET", &self.socket);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::client::daemon_log_path(&self.socket)) {
            cmd.stderr(f);
        }
        let spawned = crate::client::spawn_detached(&mut cmd);
        log::info!("upgrade: handing {} sessions to {} ({:?})", state.sessions.len(), exe_run.display(), spawned.as_ref().err());
        // 새 데몬을 못 띄워도 호스트는 살아 있고 레지스트리가 남아 있으므로 다음 데몬이 입양한다.
        self.clients.lock().clear();
        std::thread::sleep(Duration::from_millis(100));
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket);
        std::process::exit(if spawned.is_ok() { 0 } else { 1 });
    }
}

#[cfg(unix)]
fn exec(exe: &str, args: &[&str]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(exe).args(args).exec()
}

#[cfg(unix)]
static LISTENER_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

fn line_hash(l: &Line) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    l.hash(&mut h);
    h.finish()
}

/// 바뀐 줄만 담은 프레임과, 프레임이 처음 참조하는 이미지 데이터 메시지.
fn build_frame(sess: &Session, st: &mut AttachState) -> (Option<Frame>, Vec<ServerMsg>) {
    let generation = sess.generation.load(Ordering::SeqCst);
    if !st.full && generation == st.generation {
        return (None, Vec::new());
    }
    let emu = sess.emu.lock();
    let (cols, rows) = emu.size();
    let mut full = st.full;
    if st.hashes.len() != rows as usize || st.cols != cols {
        full = true;
        st.hashes = vec![0; rows as usize];
        st.cols = cols;
    }
    let mut lines = Vec::new();
    for r in 0..rows as usize {
        let l = emu.visible_line(r);
        let h = line_hash(&l);
        if full || st.hashes[r] != h {
            st.hashes[r] = h;
            lines.push((r as u16, l));
        }
    }
    let cursor = emu.cursor();
    let mode = emu.mode_bits();
    let offset = emu.display_offset() as u32;
    let history = emu.history() as u32;
    let mut image_msgs = Vec::new();
    let mut images = Vec::new();
    {
        let im = sess.images.lock();
        if !im.map.is_empty() {
            for (id, row, col) in emu.image_markers(im.max_rows()) {
                let Some(d) = im.map.get(&id) else { continue };
                if row + (d.rows as i32) <= 0 || row >= rows as i32 {
                    continue;
                }
                if st.images_sent.insert(id) {
                    image_msgs.push(ServerMsg::Image { session: sess.id, id, width: d.width, height: d.height, rgba: d.rgba.clone() });
                }
                images.push(ImagePlacement { id, row, col, cols: d.cols, rows: d.rows });
            }
        }
    }
    drop(emu);
    st.generation = generation;
    if !full && lines.is_empty() && cursor == st.cursor && mode == st.mode && offset == st.offset && images == st.images {
        return (None, image_msgs);
    }
    st.full = false;
    st.cursor = cursor;
    st.mode = mode;
    st.offset = offset;
    st.images = images.clone();
    (Some(Frame { session: sess.id, cols, rows, full, lines, cursor, mode, display_offset: offset, history, images }), image_msgs)
}

pub struct RunOptions {
    pub socket: String,
    pub restore: Option<String>,
    /// 이 프로세스가 끝날 때까지 기다린 뒤 시작한다(호스트 방식 업그레이드).
    pub wait_pid: Option<u32>,
}

fn wait_for_exit(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    #[cfg(unix)]
    {
        // SAFETY: 시그널 0 은 존재 여부만 확인한다.
        while unsafe { libc::kill(pid as i32, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        // SAFETY: 열린 프로세스 핸들로 기다린 뒤 닫는다.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !h.is_null() {
                WaitForSingleObject(h, timeout.as_millis() as u32);
                CloseHandle(h);
            }
        }
        let _ = deadline;
    }
}

/// 같은 이름의 데몬이 하나만 돌도록 배타적으로 연 잠금 파일을 쥔다(네임드 파이프는 중복 리슨을 막지 않는다).
#[cfg(windows)]
fn acquire_instance_lock(socket: &str) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let path = crate::ptyhost::registry_path(socket).with_extension("lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match std::fs::OpenOptions::new().create(true).write(true).share_mode(0).open(&path) {
            Ok(f) => return Ok(f),
            Err(e) if Instant::now() > deadline => anyhow::bail!("another daemon is running ({e})"),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn restore_hosted(daemon: &Arc<Daemon>, bytes: &[u8]) -> anyhow::Result<()> {
    let state: HostedState = postcard::from_bytes(bytes)?;
    daemon.next_session.store(state.next_session, Ordering::SeqCst);
    let mut images = state.images;
    for s in state.sessions {
        let id = s.info.id;
        match HostPty::attach(&s.endpoint, false) {
            Ok(h) => {
                daemon.install(id, AnyPty::Host(h), Emu::restore(s.dump), s.info)?;
                if let (Some(sess), Some(pos)) = (daemon.session(id), images.iter().position(|i| i.session == id)) {
                    sess.images.lock().load(images.swap_remove(pos));
                }
            }
            Err(e) => log::warn!("session {id}: host {} unreachable: {e}", s.endpoint),
        }
    }
    daemon.save_registry();
    Ok(())
}

/// 데몬을 실행한다. 반환하지 않는다(에러 시 반환).
pub fn run(opts: RunOptions) -> anyhow::Result<()> {
    if let Some(pid) = opts.wait_pid {
        wait_for_exit(pid, Duration::from_secs(10));
    }
    #[cfg(windows)]
    let _instance_lock = acquire_instance_lock(&opts.socket)?;
    let daemon = Daemon::new(opts.socket.clone());
    let state = match &opts.restore {
        Some(path) => {
            let b = std::fs::read(path)?;
            let _ = std::fs::remove_file(path);
            Some(b)
        }
        None => None,
    };
    let listener = match state {
        Some(b) if b.starts_with(STATE_MAGIC_HOSTED) => {
            let bound = Listener::bind(&opts.socket);
            let _ = std::fs::remove_file(crate::ptyhost::handover_marker(&opts.socket));
            let l = bound?;
            restore_hosted(&daemon, &b[STATE_MAGIC_HOSTED.len()..])?;
            l
        }
        #[cfg(unix)]
        Some(b) => restore(&daemon, &b)?,
        #[cfg(windows)]
        Some(_) => anyhow::bail!("unknown upgrade state"),
        None => {
            // 교체 중이면 새 데몬이 자리를 잡을 때까지 기다린다(그 데몬이 리슨하면 bind 가 실패한다).
            let deadline = Instant::now() + Duration::from_secs(15);
            while crate::ptyhost::handover_in_progress(&opts.socket) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            let l = Listener::bind(&opts.socket)?;
            if crate::ptyhost::host_mode() {
                daemon.adopt_orphans();
            }
            l
        }
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        LISTENER_FD.store(listener.0.as_raw_fd(), Ordering::SeqCst);
        // SAFETY: SIGPIPE 무시(소켓 쓰기 실패를 에러로 받는다).
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }
    }
    let d = daemon.clone();
    std::thread::spawn(move || d.monitor_loop());
    log::info!("kiln daemon {} listening on {}", build_id(), opts.socket);
    loop {
        match listener.accept() {
            Ok(conn) => {
                let d = daemon.clone();
                std::thread::spawn(move || d.handle_client(conn));
            }
            Err(e) => {
                if daemon.upgrading.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                log::warn!("accept error: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[cfg(unix)]
fn restore(daemon: &Arc<Daemon>, bytes: &[u8]) -> anyhow::Result<Listener> {
    use std::os::fd::FromRawFd;
    let (state, mut images) = match bytes.strip_prefix(STATE_MAGIC_V2) {
        Some(rest) => {
            let v2: UpgradeStateV2 = postcard::from_bytes(rest)?;
            (v2.base, v2.images)
        }
        None => (postcard::from_bytes::<UpgradeState>(&bytes)?, Vec::new()),
    };
    daemon.next_session.store(state.next_session, Ordering::SeqCst);
    for s in state.sessions {
        let pty = AnyPty::Native(Pty::from_raw(s.fd, s.pid)?);
        let emu = Emu::restore(s.dump);
        let id = s.info.id;
        daemon.install(id, pty, emu, s.info)?;
        if let (Some(sess), Some(pos)) = (daemon.session(id), images.iter().position(|i| i.session == id)) {
            sess.images.lock().load(images.swap_remove(pos));
        }
    }
    crate::pty::set_cloexec(state.listener_fd, true)?;
    // SAFETY: 이전 프로세스에서 상속한 리스닝 소켓 fd.
    let l = unsafe { std::os::unix::net::UnixListener::from_raw_fd(state.listener_fd) };
    Ok(Listener(l))
}
