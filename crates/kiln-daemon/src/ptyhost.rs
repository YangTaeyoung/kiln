//! pty-host: 세션 하나의 PTY 와 자식 프로세스를 소유하는 작은 프로세스.
//!
//! 데몬은 호스트에 로컬 소켓으로 붙어 입출력을 중계한다. 데몬이 떨어져 있는 동안 호스트는 출력을
//! 버퍼에 모아 두고, 새 데몬이 붙으면 넘겨준다. 그래서 데몬을 교체하거나 데몬이 죽어도 셸과
//! 에이전트는 계속 돈다. Windows 는 항상 이 방식을 쓰고, Unix 는 `KILN_PTY_HOST=1` 일 때 쓴다.

use crate::pty::{Pty, PtyReader, ReadResult};
use crate::transport::{self, Conn};
use kiln_proto::{SpawnSpec, read_msg, write_msg};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
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
    let shared = Arc::new(Mutex::new(Shared { writer: None, pending: VecDeque::new(), history: VecDeque::new(), exit: None, exit_delivered: false }));
    let pty_writer = Arc::new(Mutex::new(pty.writer()?));

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
        {
            let mut s = shared.lock();
            let hello = FromHost::Hello { host_proto: HOST_PROTO, child_pid, host_pid: std::process::id(), exited: s.exit };
            if write_msg(&mut writer, &hello).is_err() {
                continue;
            }
            let backlog: Vec<u8> = if replay { s.history.iter().copied().collect() } else { s.pending.iter().copied().collect() };
            s.pending.clear();
            let mut w: Option<Box<dyn Write + Send>> = Some(writer);
            send_bytes(&mut w, &backlog);
            if let Some(code) = s.exit {
                send(&mut w, &FromHost::Exit(code));
                s.exit_delivered = w.is_some();
            }
            s.writer = w;
        }
        let shared2 = shared.clone();
        let pty = pty.clone();
        let pty_writer = pty_writer.clone();
        std::thread::spawn(move || {
            while let Ok(Some(m)) = read_msg::<_, ToHost>(&mut reader) {
                match m {
                    ToHost::Input(d) => {
                        let mut w = pty_writer.lock();
                        let _ = w.write_all(&d);
                        let _ = w.flush();
                    }
                    ToHost::Resize { cols, rows } => {
                        let _ = pty.resize(cols, rows);
                    }
                    ToHost::Kill => pty.kill(),
                    ToHost::Detach => {
                        let mut s = shared2.lock();
                        send(&mut s.writer, &FromHost::Detached);
                        s.writer = None;
                        return;
                    }
                    ToHost::Attach { .. } => {}
                }
            }
            // 연결이 끊기면 이후 출력은 버퍼에 쌓는다.
            shared2.lock().writer = None;
        });
    }
}

// ---------------------------------------------------------------- 데몬 쪽 핸들

pub struct HostPty {
    pub endpoint: String,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader: Mutex<Option<Box<dyn Read + Send>>>,
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
        let Conn { mut reader, mut writer } = transport::connect(endpoint)?;
        write_msg(&mut writer, &ToHost::Attach { replay })?;
        let hello: FromHost = read_msg(&mut reader)?.ok_or_else(|| io::Error::other("host closed"))?;
        let FromHost::Hello { child_pid, host_pid, exited, .. } = hello else {
            return Err(io::Error::other("unexpected host hello"));
        };
        Ok(HostPty {
            endpoint: endpoint.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Mutex::new(Some(reader)),
            child_pid,
            host_pid,
            exit: Arc::new(Mutex::new(exited)),
            fg: Arc::new(AtomicU32::new(0)),
        })
    }

    fn send(&self, m: &ToHost) -> io::Result<()> {
        write_msg(&mut *self.writer.lock(), m)
    }

    pub fn reader(&self) -> io::Result<HostReader> {
        let inner = self.reader.lock().take().ok_or_else(|| io::Error::other("reader already taken"))?;
        Ok(HostReader { inner, exit: self.exit.clone(), fg: self.fg.clone(), pending: Vec::new(), pos: 0 })
    }

    pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(HostWriter { inner: self.writer.clone() }))
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
        let _ = self.send(&ToHost::Kill);
    }

    pub fn detach(&self) {
        let _ = self.send(&ToHost::Detach);
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
}

impl Write for HostWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write_msg(&mut *self.inner.lock(), &ToHost::Input(buf.to_vec()))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct HostReader {
    inner: Box<dyn Read + Send>,
    exit: Arc<Mutex<Option<i32>>>,
    fg: Arc<AtomicU32>,
    pending: Vec<u8>,
    pos: usize,
}

impl HostReader {
    /// 호스트 메시지를 읽어 PTY 읽기처럼 돌려준다. 호스트 연결은 블로킹이라 시간 제한을 쓰지 않는다.
    pub fn read_timeout(&mut self, buf: &mut [u8], _timeout_ms: i32) -> io::Result<ReadResult> {
        loop {
            if self.pos < self.pending.len() {
                let n = (self.pending.len() - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(ReadResult::Data(n));
            }
            match read_msg::<_, FromHost>(&mut self.inner) {
                Ok(Some(FromHost::Data(d))) => {
                    self.pending = d;
                    self.pos = 0;
                }
                Ok(Some(FromHost::Exit(c))) => {
                    *self.exit.lock() = Some(c);
                    return Ok(ReadResult::Eof);
                }
                Ok(Some(FromHost::Detached)) => return Ok(ReadResult::Detached),
                Ok(Some(FromHost::FgPid(p))) => self.fg.store(p, Ordering::Relaxed),
                Ok(Some(FromHost::Hello { .. })) => {}
                Ok(None) | Err(_) => {
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

/// 호스트 방식을 쓸지. Windows 는 항상, Unix 는 `KILN_PTY_HOST=1` 일 때.
pub fn host_mode() -> bool {
    cfg!(windows) || std::env::var("KILN_PTY_HOST").is_ok_and(|v| v == "1")
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
