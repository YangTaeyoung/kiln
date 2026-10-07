//! pty-host: 세션 하나의 PTY 와 자식 프로세스를 소유하는 작은 프로세스.
//!
//! 데몬은 호스트에 로컬 소켓으로 붙어 입출력을 중계한다. 데몬이 떨어져 있는 동안 호스트는 출력을
//! 버퍼에 모아 두고, 새 데몬이 붙으면 넘겨준다. 그래서 데몬을 교체하거나 데몬이 죽어도 셸과
//! 에이전트는 계속 돈다. macOS와 Windows는 기본으로 이 방식을 쓴다. 다른 Unix는 `KILN_PTY_HOST=1`일 때 쓴다.

use crate::pty::{Pty, PtyReader, ReadResult};
use crate::transport::{self, Conn};
use kiln_proto::{SpawnSpec, read_msg, write_msg};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// 호스트 프로토콜 버전. 오래된 호스트와도 대화해야 하므로 변형은 뒤에만 추가한다.
pub const HOST_PROTO: u32 = 1;
const HISTORY_CAP: usize = 1 << 20;
const PENDING_CAP: usize = 16 << 20;
const CHUNK: usize = 32 * 1024;

#[derive(Serialize, Deserialize, Debug)]
pub enum ToHost {
    /// 붙기. `replay` 면 최근 출력 기록 전체를, 아니면 떨어져 있던 동안의 출력만 받는다.
    Attach { replay: bool },
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Kill,
    /// 연결을 끊되 세션은 유지한다(데몬 교체 전).
    Detach,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum FromHost {
    Hello { host_proto: u32, child_pid: u32, host_pid: u32, exited: Option<i32> },
    Data(Vec<u8>),
    Exit(i32),
    Detached,
    FgPid(u32),
}

/// 호스트 엔드포인트 이름(Unix: 소켓 경로, Windows: 네임드 파이프 이름).
pub fn endpoint_for(daemon_socket: &str, session: u64) -> String {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0) ^ (std::process::id() as u64) << 20;
    #[cfg(unix)]
    {
        std::path::Path::new(daemon_socket).with_file_name(format!("pty-{session}-{:x}.sock", nonce & 0xffff_ffff)).to_string_lossy().into_owned()
    }
    #[cfg(windows)]
    {
        format!("{daemon_socket}-pty-{session}-{:x}", nonce & 0xffff_ffff)
    }
}

// ---------------------------------------------------------------- 호스트 프로세스

struct Shared {
    writer: Option<Box<dyn Write + Send>>,
    pending: VecDeque<u8>,
    history: VecDeque<u8>,
    exit: Option<i32>,
    exit_delivered: bool,
    attachment: u64,
}

impl Shared {
    fn clear_attachment(&mut self, attachment: u64) {
        // A replaced daemon's late EOF must not discard its successor's writer.
        if self.attachment == attachment { self.writer = None; }
    }
}

fn send(w: &mut Option<Box<dyn Write + Send>>, m: &FromHost) {
    if let Some(wr) = w {
        if write_msg(wr, m).is_err() {
            *w = None;
        }
    }
}

fn send_bytes(w: &mut Option<Box<dyn Write + Send>>, data: &[u8]) {
    for c in data.chunks(CHUNK) {
        send(w, &FromHost::Data(c.to_vec()));
    }
}

/// `kiln pty-host` 진입점. 자식이 끝나고 종료 코드를 데몬에 넘기면 반환한다.
pub fn run_host(endpoint: &str, spec: SpawnSpec, session: u64) -> anyhow::Result<()> {
    let listener = transport::Listener::bind(endpoint)?;
    let pty = Arc::new(Pty::spawn(&spec, session)?);
    let child_pid = pty.pid();
    let shared = Arc::new(Mutex::new(Shared { writer: None, pending: VecDeque::new(), history: VecDeque::new(), exit: None, exit_delivered: false, attachment: 0 }));
    // A child may stop reading stdin. Keep control messages (detach/kill/resize)
    // independent of that blocking PTY write, as the daemon already does.
    let (input, queued) = crossbeam_channel::unbounded::<Vec<u8>>();
    let mut pty_writer = pty.writer()?;
    std::thread::spawn(move || {
        while let Ok(data) = queued.recv() {
            if pty_writer.write_all(&data).is_err() { break; }
            let _ = pty_writer.flush();
        }
    });

    // PTY 출력 → 데몬(또는 버퍼).
    {
        let shared = shared.clone();
        let pty = pty.clone();
        let mut reader = pty.reader()?;
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read_timeout(&mut buf, 200) {
                    Ok(ReadResult::Data(n)) => {
                        let data = &buf[..n];
                        let mut s = shared.lock();
                        s.history.extend(data);
                        while s.history.len() > HISTORY_CAP {
                            s.history.pop_front();
                        }
                        if s.writer.is_some() {
                            send_bytes(&mut s.writer, data);
                        } else {
                            s.pending.extend(data);
                            while s.pending.len() > PENDING_CAP {
                                s.pending.pop_front();
                            }
                        }
                    }
                    Ok(ReadResult::Timeout) => {}
                    Ok(ReadResult::Eof) | Ok(ReadResult::Detached) | Err(_) => break,
                }
            }
            let mut code = None;
            for _ in 0..100 {
                code = pty.try_wait();
                if code.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            let code = code.unwrap_or(-1);
            let mut s = shared.lock();
            s.exit = Some(code);
            if s.writer.is_some() {
                send(&mut s.writer, &FromHost::Exit(code));
                s.exit_delivered = s.writer.is_some();
            }
        });
    }

    // 포그라운드 프로세스 알림.
    {
        let shared = shared.clone();
        let pty = pty.clone();
        std::thread::spawn(move || {
            let mut last = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(700));
                let fg = pty.fg_pid().unwrap_or(0);
                let mut s = shared.lock();
                if s.exit.is_some() {
                    return;
                }
                if fg != last && s.writer.is_some() {
                    last = fg;
                    send(&mut s.writer, &FromHost::FgPid(fg));
                }
            }
        });
    }

    // 종료 코드를 넘겼으면 끝낸다. 아무도 가져가지 않으면 10분 뒤 끝낸다.
    {
        let shared = shared.clone();
        #[cfg(unix)]
        let endpoint = endpoint.to_string();
        std::thread::spawn(move || {
            let mut exited_at: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let s = shared.lock();
                if s.exit.is_some() {
                    let t = *exited_at.get_or_insert_with(Instant::now);
                    if s.exit_delivered || t.elapsed() > Duration::from_secs(600) {
                        drop(s);
                        #[cfg(unix)]
                        let _ = std::fs::remove_file(&endpoint);
                        std::thread::sleep(Duration::from_millis(100));
                        std::process::exit(0);
                    }
                }
            }
        });
    }

    loop {
        let Conn { mut reader, mut writer } = match listener.accept() {
            Ok(c) => c,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let replay = match read_msg::<_, ToHost>(&mut reader) {
            Ok(Some(ToHost::Attach { replay })) => replay,
            _ => continue,
        };
        let attachment = {
            let mut s = shared.lock();
            let hello = FromHost::Hello { host_proto: HOST_PROTO, child_pid, host_pid: std::process::id(), exited: s.exit };
            if write_msg(&mut writer, &hello).is_err() {
                continue;
            }
            s.attachment = s.attachment.wrapping_add(1);
            let backlog: Vec<u8> = if replay { s.history.iter().copied().collect() } else { s.pending.iter().copied().collect() };
            s.pending.clear();
            let mut w: Option<Box<dyn Write + Send>> = Some(writer);
            send_bytes(&mut w, &backlog);
            if let Some(code) = s.exit {
                send(&mut w, &FromHost::Exit(code));
                s.exit_delivered = w.is_some();
            }
            // A successor must see an unchanged foreground job too. The periodic
            // sender only emits changes, so each attachment needs a baseline.
            send(&mut w, &FromHost::FgPid(pty.fg_pid().unwrap_or(0)));
            s.writer = w;
            s.attachment
        };
        let shared2 = shared.clone();
        let pty = pty.clone();
        let input = input.clone();
        std::thread::spawn(move || {
            while let Ok(Some(m)) = read_msg::<_, ToHost>(&mut reader) {
                match m {
                    ToHost::Input(d) => {
                        if input.send(d).is_err() { break; }
                    }
                    ToHost::Resize { cols, rows } => {
                        let _ = pty.resize(cols, rows);
                    }
                    ToHost::Kill => pty.kill(),
                    ToHost::Detach => {
                        let mut s = shared2.lock();
                        if s.attachment == attachment {
                            send(&mut s.writer, &FromHost::Detached);
                            s.clear_attachment(attachment);
                        }
                        return;
                    }
                    ToHost::Attach { .. } => {}
                }
            }
            // 연결이 끊기면 이후 출력은 버퍼에 쌓는다.
            shared2.lock().clear_attachment(attachment);
        });
    }
}

// ---------------------------------------------------------------- 데몬 쪽 핸들

pub struct HostPty {
    pub endpoint: String,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader: Mutex<Option<Box<dyn Read + Send>>>,
    shutdown: Option<transport::Shutdown>,
    input_ready: Arc<AtomicBool>,
    input_closed: Arc<AtomicBool>,
    child_pid: u32,
    pub host_pid: u32,
    exit: Arc<Mutex<Option<i32>>>,
    fg: Arc<AtomicU32>,
}

impl HostPty {
    /// 새 호스트 프로세스를 띄우고 붙는다.
    pub fn spawn(spec: &SpawnSpec, session: u64, exe: &std::path::Path, daemon_socket: &str) -> io::Result<HostPty> {
        use base64::Engine;
        let endpoint = endpoint_for(daemon_socket, session);
        let spec_b64 = base64::engine::general_purpose::STANDARD.encode(postcard::to_stdvec(spec).map_err(io::Error::other)?);
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["pty-host", "--endpoint", &endpoint, "--session", &session.to_string(), "--spec", &spec_b64]);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        // Startup identity failures must be diagnosable for hosted PTYs too.
        // Do not log the encoded spawn spec or the terminal's environment.
        if let Ok(log) = std::fs::OpenOptions::new().create(true).append(true)
            .open(crate::client::daemon_log_path(daemon_socket)) {
            cmd.stderr(log);
        }
        crate::client::spawn_detached(&mut cmd)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match HostPty::attach(&endpoint, false) {
                Ok(h) => return Ok(h),
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// 이미 떠 있는 호스트에 붙는다.
    pub fn attach(endpoint: &str, replay: bool) -> io::Result<HostPty> {
        let (Conn { mut reader, mut writer }, shutdown) = transport::connect_with_shutdown(endpoint)?;
        write_msg(&mut writer, &ToHost::Attach { replay })?;
        let hello: FromHost = read_msg(&mut reader)?.ok_or_else(|| io::Error::other("host closed"))?;
        let FromHost::Hello { child_pid, host_pid, exited, .. } = hello else {
            return Err(io::Error::other("unexpected host hello"));
        };
        Ok(HostPty {
            endpoint: endpoint.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Mutex::new(Some(reader)),
            shutdown,
            input_ready: Arc::new(AtomicBool::new(true)),
            input_closed: Arc::new(AtomicBool::new(exited.is_some())),
            child_pid,
            host_pid,
            exit: Arc::new(Mutex::new(exited)),
            fg: Arc::new(AtomicU32::new(0)),
        })
    }

    /// A completed session no longer needs a live host endpoint; retain its
    /// screen and exit status through upgrades without resurrecting a process.
    pub fn finished(endpoint: String, child_pid: u32, code: i32) -> Self {
        Self { endpoint, writer: Arc::new(Mutex::new(Box::new(io::sink()))),
            reader: Mutex::new(Some(Box::new(io::empty()))), shutdown: None,
            input_ready: Arc::new(AtomicBool::new(false)), input_closed: Arc::new(AtomicBool::new(true)),
            child_pid, host_pid: 0, exit: Arc::new(Mutex::new(Some(code))),
            fg: Arc::new(AtomicU32::new(0)) }
    }

    fn send(&self, m: &ToHost) -> io::Result<()> {
        write_msg(&mut *self.writer.lock(), m)
    }

    pub fn reader(&self) -> io::Result<HostReader> {
        let mut inner = self.reader.lock().take().ok_or_else(|| io::Error::other("reader already taken"))?;
        // A framed read cannot simply time out halfway through a message: the
        // next read would mistake its remaining payload for a new length. Keep
        // complete decoding on a dedicated reader and time out the bounded
        // delivery queue instead. This also lets the emulator expire redraws
        // while a hosted child is silent.
        let (tx, messages) = crossbeam_channel::bounded(16);
        let decoder = std::thread::Builder::new().name(format!("pty-host-r-{}", self.child_pid)).spawn(move || {
            while let Ok(Some(message)) = read_msg::<_, FromHost>(&mut inner) {
                if tx.send(message).is_err() { break; }
            }
        })?;
        Ok(HostReader { messages, decoder: Some(decoder), shutdown: self.shutdown.clone(), exit: self.exit.clone(), fg: self.fg.clone(), input_ready: self.input_ready.clone(), input_closed: self.input_closed.clone(), pending: Vec::new(), pos: 0 })
    }

    /// Resume a paused host after a failed daemon exec. Existing input writers
    /// keep their Arc; replace its connection and share the old exit/foreground
    /// state with the fresh decoder rather than abandoning those handles.
    pub fn reconnect_reader(&self) -> io::Result<HostReader> {
        let replacement = Self::attach(&self.endpoint, false)?;
        let mut reader = replacement.reader()?;
        reader.exit = self.exit.clone();
        reader.fg = self.fg.clone();
        reader.input_ready = self.input_ready.clone();
        reader.input_closed = self.input_closed.clone();
        {
            let mut writer = self.writer.lock();
            std::mem::swap(&mut *writer, &mut *replacement.writer.lock());
            *self.exit.lock() = *replacement.exit.lock();
            if self.exit.lock().is_some() { self.input_closed.store(true, Ordering::SeqCst); }
            // Publish readiness with the replacement writer. A delayed Detach
            // cannot clear it and then be overwritten by this worker.
            self.input_ready.store(true, Ordering::SeqCst);
        }
        Ok(reader)
    }

    pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(HostWriter { inner: self.writer.clone(), ready: self.input_ready.clone(), closed: self.input_closed.clone() }))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.send(&ToHost::Resize { cols, rows })
    }

    pub fn pid(&self) -> u32 {
        self.child_pid
    }

    pub fn try_wait(&self) -> Option<i32> {
        *self.exit.lock()
    }

    pub fn kill(&self) {
        self.input_closed.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        {
            // Closing is independent of a detached or input-blocked writer.
            // Attach through a bounded control connection, including when a
            // late Detach races with cancellation before recovery is reserved.
            let terminate = || -> io::Result<()> {
                let mut stream = std::os::unix::net::UnixStream::connect(&self.endpoint)?;
                stream.set_read_timeout(Some(Duration::from_secs(1)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                write_msg(&mut stream, &ToHost::Attach { replay: false })?;
                let hello = read_msg::<_, FromHost>(&mut stream)?;
                if !matches!(hello, Some(FromHost::Hello { .. })) { return Err(io::Error::other("host closed before termination")); }
                write_msg(&mut stream, &ToHost::Kill)
            };
            if let Err(e) = terminate() { log::warn!("PTY host termination failed: {e}"); }
        }
        #[cfg(windows)]
        { let _ = self.send(&ToHost::Kill); }
    }

    pub fn pause_input(&self) { self.input_ready.store(false, Ordering::SeqCst); }

    pub fn detach(&self) {
        let mut writer = self.writer.lock();
        // A delayed detach may acquire the replacement writer after rollback.
        // Stop input under that same lock so it cannot be sent behind Detach.
        self.input_ready.store(false, Ordering::SeqCst);
        let _ = write_msg(&mut *writer, &ToHost::Detach);
    }

    pub fn fg_pid(&self) -> Option<u32> {
        match self.fg.load(Ordering::Relaxed) {
            0 => None,
            p => Some(p),
        }
    }
}

struct HostWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
    ready: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}

impl Write for HostWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            if self.closed.load(Ordering::SeqCst) { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")); }
            if !self.ready.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(10)); continue; }
            let mut writer = self.inner.lock();
            if self.closed.load(Ordering::SeqCst) { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")); }
            if !self.ready.load(Ordering::SeqCst) { continue; }
            write_msg(&mut *writer, &ToHost::Input(buf.to_vec()))?;
            return Ok(buf.len());
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct HostReader {
    messages: crossbeam_channel::Receiver<FromHost>,
    decoder: Option<std::thread::JoinHandle<()>>,
    shutdown: Option<transport::Shutdown>,
    exit: Arc<Mutex<Option<i32>>>,
    fg: Arc<AtomicU32>,
    input_ready: Arc<AtomicBool>,
    input_closed: Arc<AtomicBool>,
    pending: Vec<u8>,
    pos: usize,
}

impl Drop for HostReader {
    fn drop(&mut self) {
        // Release a decoder waiting for queue capacity before joining it.
        let (_, empty) = crossbeam_channel::bounded(0);
        drop(std::mem::replace(&mut self.messages, empty));
        if let Some(shutdown) = &self.shutdown {
            shutdown();
            if let Some(decoder) = self.decoder.take() { let _ = decoder.join(); }
        }
    }
}

impl HostReader {
    /// Decode complete host frames while respecting the emulator's idle deadline.
    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<ReadResult> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
        loop {
            if self.pos < self.pending.len() {
                let n = (self.pending.len() - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(ReadResult::Data(n));
            }
            match self.messages.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(FromHost::Data(d)) => {
                    self.pending = d;
                    self.pos = 0;
                }
                Ok(FromHost::Exit(c)) => {
                    *self.exit.lock() = Some(c);
                    self.input_closed.store(true, Ordering::SeqCst);
                    return Ok(ReadResult::Eof);
                }
                Ok(FromHost::Detached) => { self.input_ready.store(false, Ordering::SeqCst); return Ok(ReadResult::Detached); },
                Ok(FromHost::FgPid(p)) => self.fg.store(p, Ordering::Relaxed),
                Ok(FromHost::Hello { .. }) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => return Ok(ReadResult::Timeout),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    self.input_closed.store(true, Ordering::SeqCst);
                    // 호스트가 사라졌다: 자식도 끝난 것으로 본다.
                    let mut e = self.exit.lock();
                    if e.is_none() {
                        *e = Some(-1);
                    }
                    return Ok(ReadResult::Eof);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 레지스트리(데몬 비정상 종료 후 입양)

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HostRecord {
    pub session: u64,
    pub endpoint: String,
    pub name: Option<String>,
    pub workspace: Option<String>,
    pub cwd: Option<String>,
    pub created_unix: u64,
}

pub fn registry_path(daemon_socket: &str) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        std::path::Path::new(daemon_socket).with_file_name("hosts.json")
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("Kiln").join(format!("{}-hosts.json", daemon_socket.replace(['\\', '/', ':'], "_")))
    }
}

pub fn load_registry(daemon_socket: &str) -> Vec<HostRecord> {
    std::fs::read(registry_path(daemon_socket)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_registry(daemon_socket: &str, records: &[HostRecord]) {
    let path = registry_path(daemon_socket);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if let Ok(b) = serde_json::to_vec_pretty(records) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// ---------------------------------------------------------------- 세션 PTY(직접 소유 또는 호스트)

/// macOS/Windows default to a persistent owner per terminal. Unix fixtures may opt out.
pub fn host_mode() -> bool {
    if cfg!(windows) { return true; }
    match std::env::var("KILN_PTY_HOST").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => cfg!(target_os = "macos"),
    }
}

pub enum AnyPty {
    Native(Pty),
    Host(HostPty),
}

pub enum AnyReader {
    Native(PtyReader),
    Host(HostReader),
}

impl AnyReader {
    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<ReadResult> {
        match self {
            AnyReader::Native(r) => r.read_timeout(buf, timeout_ms),
            AnyReader::Host(r) => r.read_timeout(buf, timeout_ms),
        }
    }
}

impl AnyPty {
    pub fn reader(&self) -> io::Result<AnyReader> {
        Ok(match self {
            AnyPty::Native(p) => AnyReader::Native(p.reader()?),
            AnyPty::Host(h) => AnyReader::Host(h.reader()?),
        })
    }

    pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        match self {
            AnyPty::Native(p) => p.writer(),
            AnyPty::Host(h) => h.writer(),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        match self {
            AnyPty::Native(p) => p.resize(cols, rows),
            AnyPty::Host(h) => h.resize(cols, rows),
        }
    }

    pub fn pid(&self) -> u32 {
        match self {
            AnyPty::Native(p) => p.pid(),
            AnyPty::Host(h) => h.pid(),
        }
    }

    pub fn try_wait(&self) -> Option<i32> {
        match self {
            AnyPty::Native(p) => p.try_wait(),
            AnyPty::Host(h) => h.try_wait(),
        }
    }

    pub fn kill(&self) {
        match self {
            AnyPty::Native(p) => p.kill(),
            AnyPty::Host(h) => h.kill(),
        }
    }

    pub fn fg_pid(&self) -> Option<u32> {
        match self {
            AnyPty::Native(p) => p.fg_pid(),
            AnyPty::Host(h) => h.fg_pid(),
        }
    }

    pub fn host(&self) -> Option<&HostPty> {
        match self {
            AnyPty::Host(h) => Some(h),
            AnyPty::Native(_) => None,
        }
    }

    #[cfg(unix)]
    pub fn native(&self) -> Option<&Pty> {
        match self {
            AnyPty::Native(p) => Some(p),
            AnyPty::Host(_) => None,
        }
    }
}

/// 데몬 교체 중임을 알리는 표식 파일. 이 파일이 있는 동안 클라이언트는 새 데몬을 띄우지 않고 기다린다.
pub fn handover_marker(daemon_socket: &str) -> std::path::PathBuf {
    let reg = registry_path(daemon_socket);
    reg.with_file_name(format!("{}.upgrading", reg.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()))
}

/// 교체가 진행 중인지(표식이 15초 이내에 만들어졌는지).
pub fn handover_in_progress(daemon_socket: &str) -> bool {
    std::fs::metadata(handover_marker(daemon_socket))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e < Duration::from_secs(15))
}

#[cfg(all(test, unix))]
mod recovery_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn reader_fixture() -> (HostPty, HostReader, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        let cancel = stream.try_clone().unwrap();
        let connection = transport::split(stream).unwrap();
        let host = HostPty {
            endpoint: String::new(), writer: Arc::new(Mutex::new(connection.writer)),
            reader: Mutex::new(Some(connection.reader)),
            shutdown: Some(Arc::new(move || { let _ = cancel.shutdown(std::net::Shutdown::Both); })),
            input_ready: Arc::new(AtomicBool::new(true)), input_closed: Arc::new(AtomicBool::new(false)),
            child_pid: 0, host_pid: 0, exit: Arc::new(Mutex::new(None)), fg: Arc::new(AtomicU32::new(0)),
        };
        let reader = host.reader().unwrap();
        (host, reader, peer)
    }

    #[test]
    fn host_deadline_preserves_partial_frame_and_idle_reader_drop_joins_decoder() {
        let (_host, mut reader, mut peer) = reader_fixture();
        let frame = kiln_proto::encode(&FromHost::Data("한?".as_bytes().to_vec()));
        peer.write_all(&frame[..frame.len() - 1]).unwrap();
        let mut buf = [0; 64];
        assert!(matches!(reader.read_timeout(&mut buf, 10).unwrap(), ReadResult::Timeout));
        peer.write_all(&frame[frame.len() - 1..]).unwrap();
        let ReadResult::Data(n) = reader.read_timeout(&mut buf, 200).unwrap() else { panic!("complete frame lost") };
        assert_eq!(&buf[..n], "한?".as_bytes());
        assert!(matches!(reader.read_timeout(&mut buf, 10).unwrap(), ReadResult::Timeout));
        let start = Instant::now();
        drop(reader); // joins a decoder blocked in its next framed socket read
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(peer.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn dropping_partial_or_backpressured_host_reader_releases_decoder() {
        for partial in [true, false] {
            let (_host, reader, mut peer) = reader_fixture();
            if partial {
                let frame = kiln_proto::encode(&FromHost::Data(vec![b'x'; 20]));
                peer.write_all(&frame[..5]).unwrap();
            } else {
                // More than the bounded queue capacity, so the sender waits
                // for a receiver that is deliberately not being serviced.
                for _ in 0..64 { write_msg(&mut peer, &FromHost::Data(vec![b'x'; 20])).unwrap(); }
            }
            std::thread::sleep(Duration::from_millis(10));
            let start = Instant::now();
            drop(reader);
            assert!(start.elapsed() < Duration::from_millis(500));
        }
    }

    #[test]
    fn stale_attachment_cleanup_does_not_clear_replacement_writer() {
        let mut shared = Shared { writer: Some(Box::new(Vec::<u8>::new())), pending: VecDeque::new(), history: VecDeque::new(), exit: None, exit_delivered: false, attachment: 2 };
        shared.clear_attachment(1);
        assert!(shared.writer.is_some());
        shared.clear_attachment(2);
        assert!(shared.writer.is_none());
    }
}
