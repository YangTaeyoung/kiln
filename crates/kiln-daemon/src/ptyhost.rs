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
                    Ok(ReadResult::Eof) | Ok(ReadResult::Detached) | Ok(ReadResult::Disconnected) | Err(_) => break,
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
    shutdown: Mutex<Option<transport::Shutdown>>,
    input_ready: Arc<AtomicBool>,
    input_closed: Arc<AtomicBool>,
    framing_valid: Arc<AtomicBool>,
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
        #[cfg(unix)]
        let (Conn { mut reader, mut writer }, shutdown, handshake) = {
            let stream = std::os::unix::net::UnixStream::connect(endpoint)?;
            // Recovery must not wait forever for Hello. Only the initial
            // connection handshake uses a read deadline; framed output decoding
            // must remain blocking so a quiet/partial frame is not discarded.
            // Writes retain a deadline so control can recover a backed-up peer.
            stream.set_read_timeout(Some(Duration::from_secs(1)))?;
            stream.set_write_timeout(Some(Duration::from_secs(1)))?;
            let handshake = stream.try_clone()?;
            let cancel = stream.try_clone()?;
            let shutdown: transport::Shutdown = Arc::new(move || { let _ = cancel.shutdown(std::net::Shutdown::Both); });
            (transport::split(stream)?, Some(shutdown), handshake)
        };
        #[cfg(not(unix))]
        let (Conn { mut reader, mut writer }, shutdown) = transport::connect_with_shutdown(endpoint)?;
        write_msg(&mut writer, &ToHost::Attach { replay })?;
        let hello: FromHost = read_msg(&mut reader)?.ok_or_else(|| io::Error::other("host closed"))?;
        let FromHost::Hello { child_pid, host_pid, exited, .. } = hello else {
            return Err(io::Error::other("unexpected host hello"));
        };
        #[cfg(unix)]
        {
            handshake.set_read_timeout(None)?;
        }
        Ok(HostPty {
            endpoint: endpoint.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            reader: Mutex::new(Some(reader)),
            shutdown: Mutex::new(shutdown),
            input_ready: Arc::new(AtomicBool::new(true)),
            input_closed: Arc::new(AtomicBool::new(exited.is_some())),
            framing_valid: Arc::new(AtomicBool::new(true)),
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
            reader: Mutex::new(Some(Box::new(io::empty()))), shutdown: Mutex::new(None),
            input_ready: Arc::new(AtomicBool::new(false)), input_closed: Arc::new(AtomicBool::new(true)),
            framing_valid: Arc::new(AtomicBool::new(false)),
            child_pid, host_pid: 0, exit: Arc::new(Mutex::new(Some(code))),
            fg: Arc::new(AtomicU32::new(0)) }
    }

    fn send(&self, m: &ToHost) -> io::Result<()> {
        let mut writer = self.writer.lock();
        // Windows termination closes input first. Kill remains an ordered
        // control message on an intact stream, including paused input; it must
        // never be appended to a damaged partial frame.
        let terminating = matches!(m, ToHost::Kill);
        if self.input_closed.load(Ordering::SeqCst) && !terminating {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed"));
        }
        if !self.framing_valid.load(Ordering::SeqCst)
            || (!self.input_ready.load(Ordering::SeqCst) && !terminating) {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "terminal transport is reconnecting"));
        }
        if let Err(error) = write_msg(&mut *writer, m) {
            // A partial control frame also requires a fresh connection. Never
            // append another message to an incomplete frame on this stream.
            self.input_ready.store(false, Ordering::SeqCst);
            self.framing_valid.store(false, Ordering::SeqCst);
            return Err(error);
        }
        Ok(())
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
        Ok(HostReader { messages, decoder: Some(decoder), shutdown: self.shutdown.lock().clone(), exit: self.exit.clone(), fg: self.fg.clone(), input_ready: self.input_ready.clone(), input_closed: self.input_closed.clone(), framing_valid: self.framing_valid.clone(), pending: Vec::new(), pos: 0 })
    }

    /// Reattach the same host after interruption, retaining existing input
    /// workers and exit/foreground state. Replay only detached pending output;
    /// recent history would duplicate the retained emulator. An incomplete
    /// in-flight output frame lacks a wire acknowledgement and cannot be
    /// recovered, so the daemon requests a redraw without restarting the job.
    pub fn reconnect_reader(&self) -> io::Result<HostReader> {
        let replacement = Self::attach(&self.endpoint, false)?;
        if replacement.child_pid != self.child_pid || replacement.host_pid != self.host_pid {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "terminal host identity changed; original child was not replaced"));
        }
        let mut reader = replacement.reader()?;
        reader.exit = self.exit.clone();
        reader.fg = self.fg.clone();
        reader.input_ready = self.input_ready.clone();
        reader.input_closed = self.input_closed.clone();
        reader.framing_valid = self.framing_valid.clone();
        {
            let mut writer = self.writer.lock();
            std::mem::swap(&mut *writer, &mut *replacement.writer.lock());
            *self.shutdown.lock() = replacement.shutdown.lock().clone();
            *self.exit.lock() = *replacement.exit.lock();
            if self.exit.lock().is_some() { self.input_closed.store(true, Ordering::SeqCst); }
            // Publish readiness with the replacement writer. A delayed Detach
            // cannot clear it and then be overwritten by this worker.
            self.framing_valid.store(true, Ordering::SeqCst);
            self.input_ready.store(true, Ordering::SeqCst);
        }
        Ok(reader)
    }

    pub fn writer(&self) -> io::Result<Box<dyn Write + Send>> {
        Ok(Box::new(HostWriter { inner: self.writer.clone(), ready: self.input_ready.clone(), closed: self.input_closed.clone(), framing_valid: self.framing_valid.clone() }))
    }

    /// Transport readiness is distinct from child exit and output activity.
    pub fn is_connected(&self) -> bool { self.input_ready.load(Ordering::SeqCst) }

    /// Interrupt only this attachment to release a stalled decoder/writer.
    /// Windows has no shutdown handle in its current named-pipe transport;
    /// callers must not substitute a child signal or a process restart.
    pub fn interrupt_transport(&self) {
        self.input_ready.store(false, Ordering::SeqCst);
        self.framing_valid.store(false, Ordering::SeqCst);
        if let Some(shutdown) = self.shutdown.lock().as_ref() { shutdown(); }
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
                match hello {
                    Some(FromHost::Hello { child_pid, host_pid, .. })
                        if child_pid == self.child_pid && host_pid == self.host_pid => {}
                    Some(FromHost::Hello { .. }) => return Err(io::Error::new(io::ErrorKind::InvalidData, "terminal host identity changed; refusing termination")),
                    _ => return Err(io::Error::other("host closed before termination")),
                }
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
        if !self.framing_valid.load(Ordering::SeqCst) {self.interrupt_transport();return;}
        if write_msg(&mut *writer, &ToHost::Detach).is_err() {self.framing_valid.store(false, Ordering::SeqCst);self.interrupt_transport();}
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
    framing_valid: Arc<AtomicBool>,
}

impl Write for HostWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            if self.closed.load(Ordering::SeqCst) { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")); }
            if !self.ready.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(10)); continue; }
            let mut writer = self.inner.lock();
            if self.closed.load(Ordering::SeqCst) { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "terminal closed")); }
            if !self.ready.load(Ordering::SeqCst) { continue; }
            let frame = kiln_proto::encode(&ToHost::Input(buf.to_vec()));
            match writer.write_all(&frame) {
                Ok(()) => {
                    // Once the complete frame was written, a later flush error
                    // must not replay it and inject the user's input twice.
                    let _ = writer.flush();
                    return Ok(buf.len());
                },
                Err(_) => {
                    // Keep this Input in the existing worker while the reader
                    // reconnects the same child. A failed framed write has no
                    // complete Input for the host to decode; retry only after
                    // the replacement writer/readiness is published together.
                    self.ready.store(false, Ordering::SeqCst);
                    self.framing_valid.store(false, Ordering::SeqCst);
                }
            }
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
    framing_valid: Arc<AtomicBool>,
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
                    self.input_ready.store(false, Ordering::SeqCst);
                    self.framing_valid.store(false, Ordering::SeqCst);
                    // IPC EOF/decoding failure is not evidence that the PTY
                    // child exited. The daemon can repair this attachment;
                    // only an explicit Exit/Hello status closes user input.
                    if self.exit.lock().is_some() {
                        self.input_closed.store(true, Ordering::SeqCst);
                        return Ok(ReadResult::Eof);
                    }
                    return Ok(ReadResult::Disconnected);
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
            shutdown: Mutex::new(Some(Arc::new(move || { let _ = cancel.shutdown(std::net::Shutdown::Both); }))),
            input_ready: Arc::new(AtomicBool::new(true)), input_closed: Arc::new(AtomicBool::new(false)),
            framing_valid: Arc::new(AtomicBool::new(true)),
            child_pid: 0, host_pid: 0, exit: Arc::new(Mutex::new(None)), fg: Arc::new(AtomicU32::new(0)),
        };
        let reader = host.reader().unwrap();
        (host, reader, peer)
    }

    #[test]
    fn termination_control_survives_closed_paused_input_only_on_an_intact_stream() {
        let (host, _reader, mut peer) = reader_fixture();
        host.pause_input();
        host.input_closed.store(true, Ordering::SeqCst);
        assert_eq!(host.resize(120, 30).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        host.send(&ToHost::Kill).unwrap();
        assert!(matches!(read_msg::<_, ToHost>(&mut peer).unwrap(), Some(ToHost::Kill)));
        host.framing_valid.store(false, Ordering::SeqCst);
        assert_eq!(host.send(&ToHost::Kill).unwrap_err().kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn termination_refuses_a_replaced_host_or_child_identity() {
        for changed_child in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = dir.path().join("host.sock");
            let listener = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
            let peer = std::thread::spawn(move || {
                let (mut initial, _) = listener.accept().unwrap();
                assert!(matches!(read_msg::<_, ToHost>(&mut initial).unwrap(), Some(ToHost::Attach { .. })));
                write_msg(&mut initial, &FromHost::Hello { host_proto: HOST_PROTO, child_pid: 12, host_pid: 34, exited: None }).unwrap();
                let (mut termination, _) = listener.accept().unwrap();
                assert!(matches!(read_msg::<_, ToHost>(&mut termination).unwrap(), Some(ToHost::Attach { .. })));
                write_msg(&mut termination, &FromHost::Hello { host_proto: HOST_PROTO, child_pid: if changed_child {99} else {12}, host_pid: if changed_child {34} else {99}, exited: None }).unwrap();
                assert!(read_msg::<_, ToHost>(&mut termination).unwrap().is_none(), "replacement must not receive Kill");
            });
            let host = HostPty::attach(endpoint.to_str().unwrap(), false).unwrap();
            host.kill();
            peer.join().unwrap();
            assert_eq!(host.pid(), 12);
            assert_eq!(host.try_wait(), None);
        }
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

    #[test]
    fn host_transport_loss_preserves_child_and_drains_received_output() {
        for malformed in [false, true] {
            let (host, mut reader, mut peer) = reader_fixture();
            write_msg(&mut peer, &FromHost::Data(b"completed output".to_vec())).unwrap();
            if malformed {
                // A truncated frame must not become an invented child exit.
                peer.write_all(&[4, 0, 0, 0, 255]).unwrap();
            }
            drop(peer);
            let mut buf = [0; 64];
            let ReadResult::Data(n) = reader.read_timeout(&mut buf, 1000).unwrap() else { panic!("received output lost") };
            assert_eq!(&buf[..n], b"completed output");
            let result = reader.read_timeout(&mut buf, 1000).unwrap();
            assert_eq!(host.try_wait(), None, "transport EOF is not a child exit");
            assert!(!host.input_closed.load(Ordering::SeqCst), "same child must remain recoverable");
            assert!(!host.input_ready.load(Ordering::SeqCst), "input must wait for a replacement attachment");
            assert!(matches!(result, ReadResult::Disconnected));
        }
    }

    #[test]
    fn explicit_host_exit_remains_final_when_transport_closes() {
        let (host, mut reader, mut peer) = reader_fixture();
        write_msg(&mut peer, &FromHost::Exit(42)).unwrap();
        drop(peer);
        let mut buf = [0; 64];
        assert!(matches!(reader.read_timeout(&mut buf, 1000).unwrap(), ReadResult::Eof));
        assert_eq!(host.try_wait(), Some(42));
        assert!(host.input_closed.load(Ordering::SeqCst));
        assert!(matches!(reader.read_timeout(&mut buf, 1000).unwrap(), ReadResult::Eof));
        assert_eq!(host.try_wait(), Some(42));
    }

    #[test]
    fn host_input_waits_for_replacement_after_transport_write_failure() {
        let (host, reader, peer) = reader_fixture();
        drop(peer);
        let mut writer = host.writer().unwrap();
        let (done, completed) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || { let _ = done.send(writer.write_all(b"pending input")); });
        let deadline = Instant::now() + Duration::from_secs(1);
        while host.input_ready.load(Ordering::SeqCst) && completed.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(completed.is_empty(), "transient socket failure must not terminate the session's input worker");
        assert!(!host.input_ready.load(Ordering::SeqCst));
        let (replacement, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        {
            let mut target = host.writer.lock();
            *target = Box::new(replacement);
            host.input_ready.store(true, Ordering::SeqCst);
        }
        let ToHost::Input(data) = read_msg::<_, ToHost>(&mut peer).unwrap().unwrap() else { panic!("expected retried input") };
        assert_eq!(data, b"pending input");
        assert!(completed.recv_timeout(Duration::from_secs(1)).unwrap().is_ok());
        worker.join().unwrap();
        drop(reader);
    }

    #[test]
    fn host_input_completed_frame_is_not_retried_on_flush_failure() {
        struct FlushFailure(Arc<Mutex<Vec<u8>>>);
        impl Write for FlushFailure {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> { Err(io::Error::other("flush failed after accepted frame")) }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut writer = HostWriter { inner: Arc::new(Mutex::new(Box::new(FlushFailure(bytes.clone())))), ready: Arc::new(AtomicBool::new(true)), closed: Arc::new(AtomicBool::new(false)), framing_valid: Arc::new(AtomicBool::new(true)) };
        assert_eq!(writer.write(b"exactly once").unwrap(), 12);
        assert_eq!(*bytes.lock(), kiln_proto::encode(&ToHost::Input(b"exactly once".to_vec())));
        assert!(writer.ready.load(Ordering::SeqCst));
    }

    #[test]
    fn cancelled_host_input_does_not_wait_for_reconnect() {
        let mut writer = HostWriter { inner: Arc::new(Mutex::new(Box::new(io::sink()))), ready: Arc::new(AtomicBool::new(false)), closed: Arc::new(AtomicBool::new(true)), framing_valid: Arc::new(AtomicBool::new(false)) };
        assert_eq!(writer.write(b"must not reach child").unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn host_control_never_appends_to_a_failed_partial_input_frame() {
        struct PartialFailure(Arc<Mutex<Vec<u8>>>,Arc<AtomicU32>);
        impl Write for PartialFailure {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.1.fetch_add(1,Ordering::SeqCst);
                let mut bytes=self.0.lock();
                if bytes.is_empty(){bytes.extend_from_slice(&data[..3]);Ok(3)}
                else {Err(io::Error::new(io::ErrorKind::BrokenPipe,"partial socket frame"))}
            }
            fn flush(&mut self)->io::Result<()> {Ok(())}
        }
        let (host,mut reader,_peer)=reader_fixture();
        let bytes=Arc::new(Mutex::new(Vec::new()));
        let calls=Arc::new(AtomicU32::new(0));
        *host.writer.lock()=Box::new(PartialFailure(bytes.clone(),calls.clone()));
        let mut writer=host.writer().unwrap();let closed=host.input_closed.clone();
        let worker=std::thread::spawn(move||writer.write_all(b"held input"));
        let deadline=Instant::now()+Duration::from_secs(1);
        while host.is_connected() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(1));}
        let result=host.resize(121,32);
        host.detach();
        let mut output=[0;16];
        assert!(matches!(reader.read_timeout(&mut output,1000).unwrap(),ReadResult::Disconnected));
        let captured=bytes.lock().clone();
        closed.store(true,Ordering::SeqCst);
        assert_eq!(worker.join().unwrap().unwrap_err().kind(),io::ErrorKind::BrokenPipe);
        assert_eq!(result.unwrap_err().kind(),io::ErrorKind::NotConnected);
        assert_eq!(calls.load(Ordering::SeqCst),2,"Resize and Detach must not even attempt a write on the damaged frame");
        assert_eq!(captured,kiln_proto::encode(&ToHost::Input(b"held input".to_vec()))[..3].to_vec());
        drop(reader);
    }

    #[test]
    fn paused_input_still_sends_ordered_detach_when_framing_is_valid() {
        let (host,mut reader,mut peer)=reader_fixture();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        host.pause_input();host.detach();
        assert!(matches!(read_msg::<_,ToHost>(&mut peer).unwrap(),Some(ToHost::Detach)));
        write_msg(&mut peer,&FromHost::Detached).unwrap();
        let mut output=[0;16];
        assert!(matches!(reader.read_timeout(&mut output,1000).unwrap(),ReadResult::Detached));
        assert_eq!(host.try_wait(),None);
        assert!(!host.is_connected());assert!(host.framing_valid.load(Ordering::SeqCst));
    }

    #[test]
    fn attached_host_write_timeout_releases_a_backpressured_input_for_recovery() {
        let dir=tempfile::tempdir().unwrap();let endpoint=dir.path().join("slow.sock");
        let listener=std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
        let (release,wait)=crossbeam_channel::bounded(1);
        let peer=std::thread::spawn(move||{
            let (mut stream,_)=listener.accept().unwrap();
            assert!(matches!(read_msg::<_,ToHost>(&mut stream).unwrap(),Some(ToHost::Attach{..})));
            write_msg(&mut stream,&FromHost::Hello{host_proto:HOST_PROTO,child_pid:12,host_pid:34,exited:None}).unwrap();
            let _=wait.recv_timeout(Duration::from_secs(5)); // deliberately never read Input
        });
        let host=HostPty::attach(endpoint.to_str().unwrap(),false).unwrap();
        let mut writer=host.writer().unwrap();let closed=host.input_closed.clone();
        let worker=std::thread::spawn(move||writer.write_all(&vec![b'x';8*1024*1024]));
        let deadline=Instant::now()+Duration::from_secs(3);
        while host.is_connected() && Instant::now()<deadline{std::thread::sleep(Duration::from_millis(5));}
        let interrupted=!host.is_connected();
        closed.store(true,Ordering::SeqCst);host.interrupt_transport();
        let _=release.send(());peer.join().unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(interrupted,"socket writes must time out before recovery is stranded");
        assert_eq!(host.try_wait(),None,"a write timeout does not prove child exit");
    }

    #[test]
    fn interrupting_host_attachment_preserves_child_and_uses_replacement_socket() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = dir.path().join("repair.sock");
        let listener = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
        let peer = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                assert!(matches!(read_msg::<_, ToHost>(&mut stream).unwrap(), Some(ToHost::Attach { .. })));
                write_msg(&mut stream, &FromHost::Hello { host_proto: HOST_PROTO, child_pid: 12, host_pid: 34, exited: None }).unwrap();
                assert!(read_msg::<_, ToHost>(&mut stream).unwrap().is_none(), "only the socket must close");
            }
        });
        let host = HostPty::attach(endpoint.to_str().unwrap(), false).unwrap();
        let mut reader = host.reader().unwrap();
        let mut buf = [0; 16];
        for pass in 0..2 {
            assert!(host.is_connected());
            host.interrupt_transport();
            assert!(!host.is_connected());
            assert!(matches!(reader.read_timeout(&mut buf, 1000).unwrap(), ReadResult::Disconnected));
            assert_eq!(host.pid(), 12);
            assert_eq!(host.try_wait(), None);
            assert!(!host.input_closed.load(Ordering::SeqCst));
            if pass == 1 {drop(reader);break;}
            let replacement=host.reconnect_reader().unwrap();
            drop(reader); // stale reader Drop must not invalidate the successor
            assert!(host.is_connected());assert!(host.framing_valid.load(Ordering::SeqCst));
            reader=replacement;
        }
        peer.join().unwrap();
    }

    #[test]
    fn host_reconnect_rejects_changed_child_or_host_identity() {
        for changed_child in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = dir.path().join("identity.sock");
            let listener = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
            let peer = std::thread::spawn(move || {
                for pass in 0..2 {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    assert!(matches!(read_msg::<_, ToHost>(&mut stream).unwrap(), Some(ToHost::Attach { .. })));
                    let wrong = pass == 1;
                    write_msg(&mut stream, &FromHost::Hello { host_proto: HOST_PROTO, child_pid: if wrong && changed_child {99}else{12}, host_pid: if wrong && !changed_child {99}else{34}, exited: None }).unwrap();
                    assert!(read_msg::<_, ToHost>(&mut stream).unwrap().is_none());
                }
            });
            let host = HostPty::attach(endpoint.to_str().unwrap(), false).unwrap();
            let reader = host.reader().unwrap();
            host.interrupt_transport();
            drop(reader);
            let replacement = host.reconnect_reader();
            // On the old implementation, drop the wrong attachment before
            // asserting so the owned peer fixture still closes deterministically.
            let accepted = replacement.is_ok();
            drop(replacement);
            peer.join().unwrap();
            assert!(!accepted, "must not replace a terminal with another host or child");
            assert_eq!(host.pid(), 12);
            assert_eq!(host.host_pid, 34);
            assert_eq!(host.try_wait(), None);
            assert!(!host.is_connected());
        }
    }

    #[test]
    fn host_attach_hello_is_bounded_but_output_reader_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = dir.path().join("host.sock");
        let listener = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
        let (release, wait) = crossbeam_channel::bounded(1);
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(matches!(read_msg::<_, ToHost>(&mut stream).unwrap(), Some(ToHost::Attach { .. })));
            let _ = wait.recv_timeout(Duration::from_secs(3));
        });
        let started = Instant::now();
        assert!(HostPty::attach(endpoint.to_str().unwrap(), false).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "unresponsive host hello must not strand recovery");
        let _ = release.send(());
        peer.join().unwrap();

        let listener = std::os::unix::net::UnixListener::bind(dir.path().join("ready.sock")).unwrap();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(matches!(read_msg::<_, ToHost>(&mut stream).unwrap(), Some(ToHost::Attach { .. })));
            write_msg(&mut stream, &FromHost::Hello { host_proto: HOST_PROTO, child_pid: 12, host_pid: 34, exited: None }).unwrap();
            // A quiet, healthy terminal must survive longer than the handshake timeout.
            std::thread::sleep(Duration::from_millis(1100));
            write_msg(&mut stream, &FromHost::Data(b"still alive".to_vec())).unwrap();
        });
        let host = HostPty::attach(dir.path().join("ready.sock").to_str().unwrap(), false).unwrap();
        let mut reader = host.reader().unwrap();
        let mut buf = [0; 64];
        let ReadResult::Data(n) = reader.read_timeout(&mut buf, 2000).unwrap() else { panic!("handshake timeout leaked into output decoder") };
        assert_eq!(&buf[..n], b"still alive");
        peer.join().unwrap();
    }
}
