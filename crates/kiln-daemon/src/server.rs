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
    reader_active: AtomicBool,
    cancelled: AtomicBool,
    images: Mutex<SessionImages>,
    telemetry: Mutex<crate::shell::Tracker>,
    recovery: Mutex<crate::recovery::Recovery>,
    control: Mutex<()>,
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
    session_changes: Mutex<()>,
    reader_lifecycle: Mutex<()>,
    unreachable_hosts: Mutex<Vec<HostRecord>>,
    readers_running: AtomicUsize,
    socket: String,
    cell_w: AtomicU32,
    cell_h: AtomicU32,
    palette: RwLock<TerminalPalette>,
    completions: Mutex<HashMap<SessionId,(ShellCompletion,Instant)>>,
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

/// Same-PID migration can contain retained native PTYs and new hosted PTYs.
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct UpgradeStateV3 {
    base: UpgradeState,
    hosted: Vec<SavedHosted>,
    images: Vec<SavedImages>,
}
#[cfg(unix)]
const STATE_MAGIC_V3: &[u8] = b"KILNUP3\n";

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
            session_changes: Mutex::new(()),
            reader_lifecycle: Mutex::new(()),
            unreachable_hosts: Mutex::new(Vec::new()),
            readers_running: AtomicUsize::new(0),
            socket,
            cell_w: AtomicU32::new(8),
            cell_h: AtomicU32::new(17),
            palette: RwLock::new(TerminalPalette::default()),
            completions: Mutex::new(HashMap::new()),
        })
    }

    fn completion_update(&self, session:SessionId,request:Option<ShellCompletion>){
        for c in self.clients.lock().iter(){if c.attached.lock().contains_key(&session){let _=c.out.try_send(ServerMsg::ShellCompletion{session,request:request.clone()});}}
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
        let _change = self.session_changes.lock();
        if self.upgrading.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("daemon is upgrading; retry after reconnecting"));
        }
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
        let sess = Arc::new(Session { id, pty, emu: Mutex::new(emu), input: tx, info: Mutex::new(info), generation: AtomicU64::new(1), reader_active: AtomicBool::new(false), cancelled: AtomicBool::new(false), images: Mutex::new(SessionImages::default()), telemetry: Mutex::new(crate::shell::Tracker::default()), recovery: Mutex::new(Default::default()), control: Mutex::new(()) });
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
            sess.reader_active.store(true, Ordering::SeqCst);
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
        let mut private_osc=crate::shell_completion::PrivateOscFilter::default();
        let mut img_scan = ImageScanner::default();
        let mut eof = false;
        let mut disconnected = false;
        loop {
            if self.upgrading.load(Ordering::SeqCst) && sess.pty.host().is_none() {
                break;
            }
            sess.recovery.lock().pulse(Instant::now());
            let (sync_events, timeout_ms) = {
                let mut emu = sess.emu.lock();
                (emu.expire_sync(), emu.read_timeout_ms())
            };
            if let Some(events) = sync_events {
                sess.generation.fetch_add(1, Ordering::SeqCst);
                self.handle_events(&sess, events, &mut osc_events);
                self.wake_attached(sess.id);
            }
            match reader.read_timeout(&mut buf, timeout_ms) {
                Ok(ReadResult::Data(n)) => {
                    osc.feed(&buf[..n], &mut osc_events);
                    let filtered=private_osc.feed(&buf[..n]);
                    let data=filtered.as_slice();
                    let cwd = sess.info.lock().cwd.clone();
                    let (cols, rows) = sess.emu.lock().size();
                    let changed = { let mut tracker = sess.telemetry.lock(); tracker.resize(cols, rows); tracker.feed(data, cwd.as_deref()) };
                    if changed { self.broadcast(ServerMsg::SessionTelemetry { session: sess.id, telemetry: sess.telemetry.lock().state.clone() }); }
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
                Ok(ReadResult::Disconnected) => { disconnected=true; break; }
                Ok(ReadResult::Eof) | Err(_) => {
                    eof = true;
                    break;
                }
            }
        }
        // Edit snapshots are ephemeral even when the shell cannot emit its ZLE
        // finish hook (exit, lost transport, or daemon handover).
        self.completions.lock().remove(&sess.id);
        self.completion_update(sess.id,None);
        if !eof {
            drop(reader);
            { let _readers = self.reader_lifecycle.lock();
                sess.reader_active.store(false, Ordering::SeqCst);
                self.readers_running.fetch_sub(1, Ordering::SeqCst);
            }
            if disconnected {
                sess.recovery.lock().disconnected(Instant::now());
                self.publish_health(&sess);
                self.recover_terminal(&sess, false);
                return;
            }
            // A detach can arrive after a timed-out upgrade was cancelled.
            // Recover that late reader without creating duplicate consumers.
            if !self.upgrading.load(Ordering::SeqCst) {
                sess.recovery.lock().disconnected(Instant::now());
                self.recover_terminal(&sess,false);
            }
            return;
        }
        let final_events = sess.emu.lock().finish_sync();
        if let Some(events) = final_events {
            sess.generation.fetch_add(1, Ordering::SeqCst);
            self.handle_events(&sess, events, &mut osc_events);
            self.wake_attached(sess.id);
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
        sess.telemetry.lock().exited();
        self.broadcast(ServerMsg::SessionTelemetry { session: sess.id, telemetry: sess.telemetry.lock().state.clone() });
        self.broadcast(ServerMsg::SessionExited { session: sess.id, code: Some(code) });
        self.wake_attached(sess.id);
        self.save_registry();
        drop(reader);
        { let _readers = self.reader_lifecycle.lock();
            sess.reader_active.store(false, Ordering::SeqCst);
            self.readers_running.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// 호스트 방식 세션 목록을 파일에 남긴다(데몬이 비정상 종료되면 다음 데몬이 입양한다).
    fn save_registry(&self) {
        let mut records: Vec<HostRecord> = self
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
        records.extend(self.unreachable_hosts.lock().iter().cloned());
        crate::ptyhost::save_registry(&self.socket, &records);
    }

    /// 레지스트리에 남은 호스트에 다시 붙는다. 최근 출력을 재생해 화면을 되살린다.
    fn adopt_orphans(self: &Arc<Self>) {
        let mut max_id = 0;
        for r in crate::ptyhost::load_registry(&self.socket) {
            max_id = max_id.max(r.session);
            let h = match HostPty::attach(&r.endpoint, true) {
                Ok(h) => h,
                Err(_) => { self.unreachable_hosts.lock().push(r); continue; }
            };
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
                    let [r,g,b] = sess.emu.lock().color_override(idx)
                        .or_else(|| self.palette.read().color(idx)).unwrap_or([0x80;3]);
                    let rgb = alacritty_terminal::vte::ansi::Rgb { r, g, b };
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
                OscEvent::Activity(activity) => {
                    let mut info = sess.info.lock();
                    info.attention = matches!(activity, AgentActivity::Waiting | AgentActivity::Done | AgentActivity::Failed);
                    updated = true;
                }
                OscEvent::Completion(request) => {
                    if sess.pty.fg_pid().and_then(procinfo::foreground_pid).and_then(procinfo::display_name).as_deref().is_some_and(|n|matches!(n,"zsh"|"-zsh")) {
                        self.completions.lock().insert(sess.id,(request.clone(),Instant::now()));
                        self.completion_update(sess.id,Some(request));
                    }
                }
                OscEvent::CompletionCancelled | OscEvent::CommandText(_) | OscEvent::CommandStart | OscEvent::CommandEnd(_) | OscEvent::Prompt => {
                    self.completions.lock().remove(&sess.id);
                    self.completion_update(sess.id,None);
                },
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
                continue;
            }
            let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
            for s in sessions {
                if s.info.lock().exited.is_some() {
                    continue;
                }
                if s.pty.host().is_some_and(|h|!h.is_connected()) {
                    s.recovery.lock().disconnected(Instant::now());
                }
                s.recovery.lock().inspect(Instant::now());
                self.recover_terminal(&s, false);
                let fg = s.pty.fg_pid().and_then(procinfo::foreground_pid);
                let name = fg.and_then(procinfo::display_name);
                let cwd = fg.and_then(procinfo::cwd).or_else(|| procinfo::cwd(s.pty.pid()));
                let mut changed = false;
                {
                    let mut i = s.info.lock();
                    if i.fg_process != name {
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
            ClientMsg::TerminalHealth { req, session } => {
                if let Some(s) = self.session(session) {
                    let health = if s.info.lock().exited.is_some() {Default::default()}else{s.recovery.lock().inspect(Instant::now())};
                    reply(ServerMsg::TerminalHealth {req,session,health});
                }
            }
            ClientMsg::RecoverTerminal { session } => {
                if let Some(s) = self.session(session) {self.recover_terminal(&s,true);}
            }
            ClientMsg::ListSessions { req } => {
                reply(ServerMsg::Sessions { req, sessions: self.list() });
                for sess in self.sessions.read().values() { reply(ServerMsg::SessionTelemetry { session: sess.id, telemetry: sess.telemetry.lock().state.clone() }); }
            },
            ClientMsg::ReadCommandOutput { req, session, command } => {
                match self.session(session).and_then(|s| s.telemetry.lock().output(command)) {
                    Some((text, truncated)) => reply(ServerMsg::CommandOutput { req, session, command, text, truncated }),
                    None => reply(ServerMsg::Error { req, message: "명령 출력을 더 이상 사용할 수 없습니다. 데몬 재시작 또는 기록 제한을 확인하세요.".into() }),
                }
            },
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
            ClientMsg::ApplyShellCompletion{session,revision,buffer,cursor}=>{
                if let (Some(s),Some((request,at)))=(self.session(session),self.completions.lock().remove(&session)) {
                    if request.revision==revision && at.elapsed()<std::time::Duration::from_secs(60)
                        && s.pty.fg_pid().and_then(procinfo::foreground_pid).and_then(procinfo::display_name).as_deref().is_some_and(|n|matches!(n,"zsh"|"-zsh"))
                        && crate::shell_completion::stage(&request,&buffer,cursor).is_ok(){let _=s.input.send(b"\x1b[99;1~".to_vec());}
                }
                self.completion_update(session,None);
            }
            ClientMsg::Input { session, data } => {
                if self.completions.lock().remove(&session).is_some(){self.completion_update(session,None);}
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
                let _change = self.session_changes.lock();
                if self.upgrading.load(Ordering::SeqCst) { return true; }
                let removed = self.sessions.write().remove(&session);
                self.unreachable_hosts.lock().retain(|r| r.session != session);
                self.save_registry();
                if let Some(s) = removed {
                    s.cancelled.store(true, Ordering::SeqCst);
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
            ClientMsg::SetPalette { palette } => { *self.palette.write() = palette; }
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
                let _change = self.session_changes.lock();
                if self.upgrading.load(Ordering::SeqCst) { return true; }
                for s in self.sessions.read().values() {
                    s.cancelled.store(true, Ordering::SeqCst);
                    s.pty.kill();
                }
                let _ = std::fs::remove_file(&self.socket);
                std::process::exit(0);
            }
        }
        true
    }

    fn resize(&self, s: &Session, cols: u16, rows: u16) {
        {
            let mut emu = s.emu.lock();
            if emu.size() == (cols, rows) {return;}
            emu.resize(cols, rows);
            let mut info=s.info.lock();
            (info.cols,info.rows)=emu.size();
            s.generation.fetch_add(1,Ordering::SeqCst);
        }
        // A blocked host input writer must not retain the emulator lock: its
        // output consumer needs that lock to drain the opposite socket direction.
        let _control=s.control.lock();
        let (cols,rows)={let info=s.info.lock();(info.cols,info.rows)};
        let _=s.pty.resize(cols,rows);
    }

    fn redraw_terminal(&self,s:&Session)->std::io::Result<()> {
        let _control=s.control.try_lock_for(Duration::from_secs(2)).ok_or_else(||std::io::Error::new(std::io::ErrorKind::TimedOut,"terminal resize control is busy"))?;
        if self.upgrading.load(Ordering::SeqCst) || s.cancelled.load(Ordering::SeqCst) {return Err(std::io::Error::new(std::io::ErrorKind::Interrupted,"terminal no longer available"));}
        let (cols,rows)={let info=s.info.lock();if info.exited.is_some(){return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe,"terminal exited"));}(info.cols,info.rows)};
        // Unchanged TIOCSWINSZ does not emit SIGWINCH on macOS. Briefly change
        // one column, then restore the latest requested size. No emulator lock
        // is held across either transport write; the reader stays independent.
        let temporary=if cols>2 {cols-1}else{cols+1};
        s.pty.resize(temporary,rows)?;
        let (cols,rows)={let info=s.info.lock();(info.cols,info.rows)};
        s.pty.resize(cols,rows)
    }

    fn publish_health(&self, s:&Session) {
        let (health,reader_age)={let recovery=s.recovery.lock();(recovery.health,Instant::now().saturating_duration_since(recovery.reader_seen))};
        log::info!("terminal {} connection {:?}, attempt {}, reader_active={}, reader_age_ms={}, host_ready={}",s.id,health.state,health.attempts,s.reader_active.load(Ordering::SeqCst),reader_age.as_millis(),s.pty.host().is_none_or(|h|h.is_connected()));
        self.broadcast(ServerMsg::TerminalHealth {req:0,session:s.id,health});
    }

    /// Reconnect only transport and redraw. Never signal termination, create a
    /// replacement child, resend a prompt, or replay a completed input frame.
    fn recover_terminal(self:&Arc<Self>, s:&Arc<Session>, manual:bool) {
        if self.upgrading.load(Ordering::SeqCst) || s.cancelled.load(Ordering::SeqCst) || s.info.lock().exited.is_some() {return;}
        if !s.recovery.lock().begin(Instant::now(),manual) {return;}
        self.publish_health(s);
        if !s.reader_active.load(Ordering::SeqCst) {
            self.resume_reader(s);
            return;
        }
        if let Some(h)=s.pty.host().filter(|h|!h.is_connected()) {
            h.interrupt_transport();
            s.recovery.lock().failed(Instant::now());
            self.publish_health(s);
            return;
        }
        let d=self.clone();let s=s.clone();
        std::thread::spawn(move || {
            let events=s.emu.try_lock_for(Duration::from_millis(100)).map(|mut e|e.finish_sync().unwrap_or_default());
            let responsive=Instant::now().saturating_duration_since(s.recovery.lock().reader_seen)<=Duration::from_secs(5);
            if let Some(events)=events.filter(|_|responsive) {
                d.handle_events(&s,events,&mut Vec::new());
                if d.redraw_terminal(&s).is_ok() {
                    d.refresh_screen(&s);
                    s.recovery.lock().alive(Instant::now());
                } else {s.recovery.lock().failed(Instant::now());}
            } else {s.recovery.lock().failed(Instant::now());}
            d.publish_health(&s);
        });
    }

    fn refresh_screen(&self,s:&Session) {
        s.generation.fetch_add(1,Ordering::SeqCst);
        let clients=self.clients.lock().clone();
        for c in clients {
            if let Some(mut attached)=c.attached.try_lock() {
                if let Some(st)=attached.get_mut(&s.id) {st.full=true;}
            }
            let _=c.wake.try_send(());
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
                    let result=build_frame(&sess, st);
                    if st.full || st.generation!=sess.generation.load(Ordering::SeqCst) {let _=client.wake.try_send(());}
                    result
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
        let _change = self.session_changes.lock();
        anyhow::ensure!(!self.upgrading.load(Ordering::SeqCst), "upgrade already in progress");
        // A retained native child must keep this parent, even when new sessions
        // use hosts. Routing on host_mode alone silently discarded native PTYs.
        let has_native = self.sessions.read().values().any(|s| s.pty.native().is_some());
        if !has_native && (crate::ptyhost::host_mode() || self.sessions.read().values().any(|s| s.pty.host().is_some())) {
            return self.clone().upgrade_hosted(exe);
        }
        anyhow::ensure!(std::path::Path::new(exe).exists(), "{exe} not found");
        let listener_fd = LISTENER_FD.load(Ordering::SeqCst);
        anyhow::ensure!(listener_fd >= 0, "listener fd unknown");
        self.broadcast(ServerMsg::Upgrading);
        { let _readers = self.reader_lifecycle.lock(); self.upgrading.store(true, Ordering::SeqCst); }
        let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
        if let Err(error) = self.pause_readers(&sessions) {
            self.resume_after_failed_upgrade(&sessions);
            return Err(error);
        }
        let path = std::path::Path::new(&self.socket).with_file_name(format!("upgrade-{}.state", std::process::id()));
        let attempt = (|| -> anyhow::Result<()> {
            let mut saved = Vec::new();
            let mut hosted = Vec::new();
            let mut images = Vec::new();
            for s in &sessions {
                let dump = s.emu.lock().dump();
                if let Some(native) = s.pty.native() {
                    let fd = native.raw_fd();
                    crate::pty::set_cloexec(fd, false)?;
                    saved.push(SavedSession { info: s.info.lock().clone(), fd, pid: s.pty.pid() as i32, dump });
                } else if let Some(host) = s.pty.host() {
                    hosted.push(SavedHosted { info: { let mut i = s.info.lock().clone(); i.exited = i.exited.or(s.pty.try_wait()); i }, endpoint: host.endpoint.clone(), dump });
                }
                images.push(s.images.lock().save(s.id));
            }
            crate::pty::set_cloexec(listener_fd, false)?;
            let state = UpgradeStateV3 { base: UpgradeState { next_session: self.next_session.load(Ordering::SeqCst), listener_fd, sessions: saved }, hosted, images };
            let mut bytes = STATE_MAGIC_V3.to_vec(); bytes.extend(postcard::to_stdvec(&state)?);
            std::fs::write(&path, bytes)?;
            self.clients.lock().clear();
            log::info!("exec upgrade preserving {} sessions", sessions.len());
            Err(exec(exe, &["daemon", "--foreground", "--socket", &self.socket, "--restore", &path.to_string_lossy()]).into())
        })();
        // Restore all paused resources on any save/FD/exec failure, not only exec.
        let _ = std::fs::remove_file(&path);
        let _ = crate::pty::set_cloexec(listener_fd, true);
        self.resume_after_failed_upgrade(&sessions);
        attempt
    }

    fn pause_readers(&self, sessions: &[Arc<Session>]) -> anyhow::Result<()> {
        for s in sessions {
            if let Some(host) = s.pty.host() && s.info.lock().exited.is_none() {
                host.pause_input();
                let s = s.clone();
                // A legacy host can be blocked writing to a child. Never wait
                // for its control socket while holding the lifecycle lock.
                std::thread::spawn(move || s.pty.host().unwrap().detach());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.readers_running.load(Ordering::SeqCst) > 0 {
            anyhow::ensure!(Instant::now() < deadline, "terminal host did not pause; upgrade cancelled, sessions retained");
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn resume_after_failed_upgrade(self: &Arc<Self>, sessions: &[Arc<Session>]) {
        { let _readers = self.reader_lifecycle.lock(); self.upgrading.store(false, Ordering::SeqCst); }
        for s in sessions {
            #[cfg(unix)]
            if let Some(native) = s.pty.native() { let _ = crate::pty::set_cloexec(native.raw_fd(), true); }
            self.resume_reader(s);
        }
    }

    fn resume_reader(self: &Arc<Self>, s: &Arc<Session>) {
        {
            let _readers = self.reader_lifecycle.lock();
            if self.upgrading.load(Ordering::SeqCst) || s.cancelled.load(Ordering::SeqCst) || s.info.lock().exited.is_some() || s.reader_active.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() { return; }
            // Count the reservation, including an in-flight host reconnect.
            self.readers_running.fetch_add(1, Ordering::SeqCst);
        }
        let d = self.clone(); let s = s.clone();
        // Reconnection is independent of the lifecycle lock. A broken host
        // must not prevent Ping/Kill/Shutdown or recovering other sessions.
        std::thread::spawn(move || {
            #[cfg(unix)]
            let reader = if let Some(p) = s.pty.native() {
                let _ = crate::pty::set_cloexec(p.raw_fd(), true);
                s.pty.reader()
            } else { s.pty.host().unwrap().reconnect_reader().map(crate::ptyhost::AnyReader::Host) };
            #[cfg(windows)]
            let reader = s.pty.host().unwrap().reconnect_reader().map(crate::ptyhost::AnyReader::Host);
            match reader {
                Ok(reader) => {
                    if s.cancelled.load(Ordering::SeqCst) {
                        // Kill may have targeted the detached writer. The fresh
                        // connection must carry it too, never revive a removed pane.
                        s.pty.kill();
                        drop(reader);
                        let _readers = d.reader_lifecycle.lock();
                        s.reader_active.store(false, Ordering::SeqCst);
                        d.readers_running.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    // A second upgrade may already be waiting for this reserved
                    // reader. Detach the new attachment before consuming it.
                    if d.upgrading.load(Ordering::SeqCst) {
                        let panel=s.clone();
                        std::thread::spawn(move || {if let Some(h)=panel.pty.host() {h.detach();}});
                    }
                    else {
                        s.recovery.lock().connected(Instant::now());
                        d.publish_health(&s);
                        // Consume the backlog immediately. Resize/Input share a
                        // writer and can wait for the host to finish replaying;
                        // awaiting them here deadlocks the bounded decoder queue.
                        let redraw=d.clone();let panel=s.clone();
                        std::thread::spawn(move || {
                            if redraw.redraw_terminal(&panel).is_err() {
                                panel.recovery.lock().failed(Instant::now());
                                redraw.publish_health(&panel);
                            }
                            redraw.refresh_screen(&panel);
                        });
                    }
                    d.read_loop(s, reader);
                }
                Err(e) => {
                    { let _readers = d.reader_lifecycle.lock();
                        s.reader_active.store(false, Ordering::SeqCst);
                        d.readers_running.fetch_sub(1, Ordering::SeqCst);
                    }
                    log::error!("session {}: could not resume reader after failed upgrade: {e}", s.id);
                    s.recovery.lock().failed(Instant::now());
                    d.publish_health(&s);
                }
            }
        });
    }

    #[cfg(windows)]
    fn upgrade(self: Arc<Self>, exe: &str) -> anyhow::Result<()> {
        let _change = self.session_changes.lock();
        self.clone().upgrade_hosted(exe)
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
        { let _readers = self.reader_lifecycle.lock(); self.upgrading.store(true, Ordering::SeqCst); }
        let sessions: Vec<Arc<Session>> = self.sessions.read().values().cloned().collect();
        if let Err(error) = self.pause_readers(&sessions) {
            self.resume_after_failed_upgrade(&sessions);
            return Err(error);
        }
        let marker = crate::ptyhost::handover_marker(&self.socket);
        let path = std::env::temp_dir().join(format!("kiln-upgrade-{}.state", std::process::id()));
        let attempt = (|| -> anyhow::Result<()> {
        let mut saved = Vec::new();
        let mut images = Vec::new();
        for s in &sessions {
            let Some(h) = s.pty.host() else { continue };
            saved.push(SavedHosted { info: { let mut i = s.info.lock().clone(); i.exited = i.exited.or(s.pty.try_wait()); i }, endpoint: h.endpoint.clone(), dump: s.emu.lock().dump() });
            images.push(s.images.lock().save(s.id));
        }
        let state = HostedState { next_session: self.next_session.load(Ordering::SeqCst), sessions: saved, images };
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&marker, std::process::id().to_string())?;
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
        crate::client::spawn_detached(&mut cmd)?;
        log::info!("upgrade: handing {} hosted sessions to new daemon", state.sessions.len());
            Ok(())
        })();
        if let Err(error) = attempt {
            let _ = std::fs::remove_file(&marker);
            let _ = std::fs::remove_file(&path);
            self.resume_after_failed_upgrade(&sessions);
            return Err(error);
        }
        // 새 데몬을 못 띄워도 호스트는 살아 있고 레지스트리가 남아 있으므로 다음 데몬이 입양한다.
        self.clients.lock().clear();
        std::thread::sleep(Duration::from_millis(100));
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket);
        std::process::exit(0);
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
    let Some(emu) = sess.emu.try_lock() else {return (None,Vec::new());};
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

fn retain_unrestored_records(daemon: &Arc<Daemon>, restored: &[SessionId]) {
    let mut unreachable = daemon.unreachable_hosts.lock();
    for record in crate::ptyhost::load_registry(&daemon.socket) {
        if !restored.contains(&record.session) && !unreachable.iter().any(|r| r.session == record.session) {
            daemon.next_session.fetch_max(record.session + 1, Ordering::SeqCst);
            unreachable.push(record);
        }
    }
}

fn restore_host_session(daemon: &Arc<Daemon>, mut s: SavedHosted, images: &mut Vec<SavedImages>) -> anyhow::Result<()> {
    let id = s.info.id;
    let host = if let Some(code) = s.info.exited {
        HostPty::finished(s.endpoint.clone(), s.info.pid, code)
    } else {
        match HostPty::attach(&s.endpoint, false) {
            Ok(h) => { s.info.exited = h.try_wait(); h },
            Err(e) => {
                // A vanished optional host must never destroy inherited native
                // masters or discard another running host's recovery record.
                log::warn!("session {id}: host {} unreachable: {e}; retaining recovery record", s.endpoint);
                daemon.unreachable_hosts.lock().push(HostRecord { session: id, endpoint: s.endpoint,
                    name: s.info.name, workspace: s.info.workspace, cwd: s.info.cwd,
                    created_unix: s.info.created_unix });
                return Ok(());
            }
        }
    };
    daemon.install(id, AnyPty::Host(host), Emu::restore(s.dump), s.info)?;
    if let (Some(sess), Some(pos)) = (daemon.session(id), images.iter().position(|i| i.session == id)) {
        sess.images.lock().load(images.swap_remove(pos));
    }
    Ok(())
}

fn restore_hosted(daemon: &Arc<Daemon>, bytes: &[u8]) -> anyhow::Result<()> {
    let state: HostedState = postcard::from_bytes(bytes)?;
    daemon.next_session.store(state.next_session, Ordering::SeqCst);
    let mut images = state.images;
    let restored: Vec<_> = state.sessions.iter().map(|s| s.info.id).collect();
    retain_unrestored_records(daemon, &restored);
    for s in state.sessions { restore_host_session(daemon, s, &mut images)?; }
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
    let (state, mut images, hosted) = if let Some(rest) = bytes.strip_prefix(STATE_MAGIC_V3) {
        let v3: UpgradeStateV3 = postcard::from_bytes(rest)?;
        (v3.base, v3.images, v3.hosted)
    } else if let Some(rest) = bytes.strip_prefix(STATE_MAGIC_V2) {
        let v2: UpgradeStateV2 = postcard::from_bytes(rest)?;
        (v2.base, v2.images, Vec::new())
    } else { (postcard::from_bytes::<UpgradeState>(bytes)?, Vec::new(), Vec::new()) };
    daemon.next_session.store(state.next_session, Ordering::SeqCst);
    let restored: Vec<_> = state.sessions.iter().map(|s| s.info.id).chain(hosted.iter().map(|s| s.info.id)).collect();
    retain_unrestored_records(daemon, &restored);
    for s in state.sessions {
        let pty = AnyPty::Native(Pty::from_raw(s.fd, s.pid)?);
        let emu = Emu::restore(s.dump);
        let id = s.info.id;
        daemon.install(id, pty, emu, s.info)?;
        if let (Some(sess), Some(pos)) = (daemon.session(id), images.iter().position(|i| i.session == id)) {
            sess.images.lock().load(images.swap_remove(pos));
        }
    }
    for s in hosted { restore_host_session(daemon, s, &mut images)?; }
    daemon.save_registry();
    crate::pty::set_cloexec(state.listener_fd, true)?;
    // SAFETY: 이전 프로세스에서 상속한 리스닝 소켓 fd.
    let l = unsafe { std::os::unix::net::UnixListener::from_raw_fd(state.listener_fd) };
    Ok(Listener(l))
}

#[cfg(all(test, unix))]
mod palette_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_busy_panel_cannot_block_another_panels_frame_or_health() {
        let dir=tempfile::tempdir().unwrap();
        let daemon=Daemon::new(dir.path().join("isolated.sock").to_string_lossy().into_owned());
        struct Cleanup(Arc<Daemon>);
        impl Drop for Cleanup {fn drop(&mut self){for s in self.0.sessions.read().values(){s.pty.kill();}}}
        let _cleanup=Cleanup(daemon.clone());
        let spec=SpawnSpec {program:Some("/bin/cat".into()),cols:80,rows:24,..Default::default()};
        let first=daemon.create(spec.clone()).unwrap();let second=daemon.create(spec).unwrap();
        let first=daemon.session(first).unwrap();let second=daemon.session(second).unwrap();
        let held=first.emu.lock();
        let (tx,rx)=bounded(1);let busy=first.clone();
        let worker=std::thread::spawn(move || {let mut st=AttachState {full:true,..Default::default()};let _=tx.send(build_frame(&busy,&mut st).0);});
        let result=rx.recv_timeout(Duration::from_millis(200));
        let mut st=AttachState {full:true,..Default::default()};
        assert!(build_frame(&second,&mut st).0.is_some());
        assert_eq!(second.recovery.lock().inspect(Instant::now()).state,TerminalState::Healthy);
        drop(held);worker.join().unwrap();
        assert!(matches!(result,Ok(None)),"frame publisher waited on another panel's emulator lock");
    }

    #[test]
    fn first_child_queries_use_palette_sent_before_create_and_updates_preserve_overrides() {
        assert!(!crate::ptyhost::host_mode(), "run this isolated native PTY test without KILN_PTY_HOST=1");
        let dir=tempfile::Builder::new().prefix("kiln-palette-").tempdir_in("/tmp").unwrap();
        let socket=dir.path().join("d.sock").to_string_lossy().into_owned();
        let daemon=Daemon::new(socket.clone());
        struct Cleanup(Arc<Daemon>);
        impl Drop for Cleanup {fn drop(&mut self){for s in self.0.sessions.read().values(){if s.info.lock().exited.is_none(){s.pty.kill();}}}}
        let _cleanup=Cleanup(daemon.clone());
        let listener=Listener::bind(&socket).unwrap();
        let d=daemon.clone();
        let server=std::thread::spawn(move||d.handle_client(listener.accept().unwrap()));
        let mut stream=UnixStream::connect(&socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write_msg(&mut stream,&ClientMsg::Hello{proto:PROTO_VERSION,build:"test".into(),client:"palette-test".into()}).unwrap();
        assert!(matches!(read_msg::<_,ServerMsg>(&mut stream).unwrap(),Some(ServerMsg::Hello{proto:PROTO_VERSION,..})));
        let light=TerminalPalette{fg:[28,29,33],bg:[255;3],cursor:[76,88,210],ansi:[[20,30,40];16]};
        let script=dir.path().join("probe.py");
        std::fs::write(&script,r#"import os,sys,tty,select,time,json
os.chdir(sys.argv[1])
tty.setraw(0)
def query(sequence):
    os.write(1, sequence)
    data=b''
    deadline=time.monotonic()+5
    while not data.endswith(b'\x07'):
        remaining=deadline-time.monotonic()
        if remaining<=0 or not select.select([0],[],[],remaining)[0]:
            raise RuntimeError('OSC reply timeout: '+repr(data))
        data+=os.read(0,1)
    return data.decode()
def phase(name,queries):
    values=[query(q) for q in queries]
    with open(name,'w') as f: json.dump(values,f)
queries=[b'\x1b]10;?\x07',b'\x1b]11;?\x07',b'\x1b]12;?\x07',b'\x1b]4;7;?\x07',b'\x1b]4;21;?\x07',b'\x1b]4;244;?\x07']
phase('initial.json',queries)
deadline=time.monotonic()+10
while not os.path.exists('continue'):
    if time.monotonic()>deadline: raise RuntimeError('test did not update palette')
    time.sleep(.005)
phase('updated.json',queries)
os.write(1,b'\x1b]11;rgb:11/22/33\x07\x1b]4;7;rgb:00/aa/00\x07')
phase('override.json',[queries[1],queries[3]])
os.write(1,b'\x1b]111\x07\x1b]104;7\x07')
phase('reset.json',[queries[1],queries[3]])
"#).unwrap();
        // The two messages share one FIFO stream. No sleep between palette and Create.
        write_msg(&mut stream,&ClientMsg::SetPalette{palette:light}).unwrap();
        write_msg(&mut stream,&ClientMsg::Create{req:7,spec:SpawnSpec{program:Some("/usr/bin/python3".into()),args:vec![script.to_string_lossy().into_owned(),dir.path().to_string_lossy().into_owned()],cwd:Some(dir.path().to_string_lossy().into_owned()),cols:80,rows:24,..Default::default()}}).unwrap();
        let session=loop {match read_msg::<_,ServerMsg>(&mut stream).unwrap().unwrap(){ServerMsg::Created{req:7,session}=>break session,ServerMsg::Error{message,..}=>panic!("{message}"),_=>{}}};
        let read_phase=|name:&str|{
            let deadline=Instant::now()+Duration::from_secs(10);
            loop {
                if let Ok(bytes)=std::fs::read(dir.path().join(name)) {if let Ok(values)=serde_json::from_slice::<Vec<String>>(&bytes){break values;}}
                assert!(Instant::now()<deadline,"missing {name}: {}",daemon.session(session).unwrap().emu.lock().text(0));
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let reply=|prefix:&str,[r,g,b]:[u8;3]|format!("\x1b]{prefix};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07");
        let expected=|palette:TerminalPalette|vec![reply("10",palette.fg),reply("11",palette.bg),reply("12",palette.cursor),reply("4;7",palette.ansi[7]),reply("4;21",[0,0,255]),reply("4;244",[128;3])];
        assert_eq!(read_phase("initial.json"),expected(light));
        let dark=TerminalPalette::default();
        write_msg(&mut stream,&ClientMsg::SetPalette{palette:dark}).unwrap();
        write_msg(&mut stream,&ClientMsg::Ping{req:8}).unwrap();
        loop {if matches!(read_msg::<_,ServerMsg>(&mut stream).unwrap(),Some(ServerMsg::Pong{req:8})){break;}}
        std::fs::write(dir.path().join("continue"),b"go").unwrap();
        assert_eq!(read_phase("updated.json"),expected(dark));
        assert_eq!(read_phase("override.json"),vec![reply("11",[0x11,0x22,0x33]),reply("4;7",[0,0xaa,0])]);
        assert_eq!(read_phase("reset.json"),vec![reply("11",dark.bg),reply("4;7",dark.ansi[7])]);
        stream.shutdown(std::net::Shutdown::Both).unwrap();drop(stream);server.join().unwrap();
    }
}
