//! 데몬 클라이언트. GUI 와 CLI 가 공유한다.

use crate::transport::{self, Conn};
use crossbeam_channel::{Receiver, Sender, unbounded};
use kiln_proto::*;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub struct Client {
    tx: Sender<ClientMsg>,
    pub rx: Receiver<ServerMsg>,
    next_req: Arc<AtomicU32>,
    pub server_build: String,
    pub server_pid: u32,
    pub can_upgrade: bool,
}

pub type Notify = Arc<dyn Fn() + Send + Sync>;

impl Client {
    /// 연결 후 Hello 를 주고받는다. `notify` 는 서버 메시지가 도착할 때마다 호출된다.
    pub fn connect(socket: &str, notify: Option<Notify>) -> std::io::Result<Client> {
        let Conn { mut reader, mut writer } = transport::connect(socket)?;
        write_msg(&mut writer, &ClientMsg::Hello { proto: PROTO_VERSION, build: crate::build_id().into(), client: "kiln".into() })?;
        let hello: ServerMsg = read_msg(&mut reader)?.ok_or_else(|| std::io::Error::other("daemon closed"))?;
        let (server_build, server_pid, can_upgrade) = match hello {
            ServerMsg::Hello { proto, build, pid, can_upgrade } => {
                if proto != PROTO_VERSION {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{PROTO_MISMATCH}: daemon {proto}, client {PROTO_VERSION}")));
                }
                (build, pid, can_upgrade)
            }
            other => return Err(std::io::Error::other(format!("unexpected hello: {other:?}"))),
        };
        let (tx, out_rx) = unbounded::<ClientMsg>();
        let (in_tx, rx) = unbounded::<ServerMsg>();
        std::thread::Builder::new().name("kiln-client-w".into()).spawn(move || {
            let mut buf = Vec::new();
            while let Ok(m) = out_rx.recv() {
                buf.clear();
                buf.extend_from_slice(&encode(&m));
                while let Ok(more) = out_rx.try_recv() {
                    buf.extend_from_slice(&encode(&more));
                }
                if writer.write_all(&buf).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        })?;
        std::thread::Builder::new().name("kiln-client-r".into()).spawn(move || {
            while let Ok(Some(m)) = read_msg::<_, ServerMsg>(&mut reader) {
                if in_tx.send(m).is_err() {
                    break;
                }
                if let Some(n) = &notify {
                    n();
                }
            }
            drop(in_tx);
            if let Some(n) = &notify {
                n();
            }
        })?;
        Ok(Client { tx, rx, next_req: Arc::new(AtomicU32::new(1)), server_build, server_pid, can_upgrade })
    }

    /// 데몬이 없으면 `exe daemon` 을 분리 실행한 뒤 연결한다.
    /// 프로토콜 버전이 다른 데몬이면 `exe` 로 업그레이드시킨 뒤 연결한다.
    pub fn connect_or_spawn(socket: &str, exe: &Path, notify: Option<Notify>) -> anyhow::Result<Client> {
        match Client::connect(socket, notify.clone()) {
            Ok(c) => return Ok(c),
            Err(e) if is_proto_mismatch(&e) => {
                request_upgrade(socket, exe)?;
            }
            Err(_) => spawn_daemon(exe, socket)?,
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match Client::connect(socket, notify.clone()) {
                Ok(c) => return Ok(c),
                Err(e) if Instant::now() > deadline => return Err(anyhow::anyhow!("daemon did not start: {e}")),
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
    }

    pub fn send(&self, m: ClientMsg) {
        let _ = self.tx.send(m);
    }

    pub fn next_req(&self) -> u32 {
        self.next_req.fetch_add(1, Ordering::SeqCst)
    }

    /// 요청을 보내고 같은 req 번호의 응답을 기다린다. 다른 메시지는 버린다(CLI 용).
    pub fn request(&self, f: impl FnOnce(u32) -> ClientMsg, timeout: Duration) -> anyhow::Result<ServerMsg> {
        let req = self.next_req();
        self.send(f(req));
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self.rx.recv_timeout(left).map_err(|_| anyhow::anyhow!("timeout waiting for daemon"))?;
            let r = match &m {
                ServerMsg::Sessions { req, .. }
                | ServerMsg::Created { req, .. }
                | ServerMsg::Text { req, .. }
                | ServerMsg::Error { req, .. }
                | ServerMsg::Pong { req }
                | ServerMsg::SearchResult { req, .. } => Some(*req),
                _ => None,
            };
            if r == Some(req) {
                if let ServerMsg::Error { message, .. } = &m {
                    anyhow::bail!("{message}");
                }
                return Ok(m);
            }
        }
    }
}

pub const PROTO_MISMATCH: &str = "protocol mismatch";

pub fn is_proto_mismatch(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::InvalidData && e.to_string().starts_with(PROTO_MISMATCH)
}

/// 버전에 상관없이 인코딩이 고정된 Hello/Upgrade 만 써서 데몬에 업그레이드를 요청한다.
pub fn request_upgrade(socket: &str, exe: &Path) -> std::io::Result<()> {
    let Conn { mut reader, mut writer } = transport::connect(socket)?;
    write_msg(&mut writer, &ClientMsg::Hello { proto: PROTO_VERSION, build: crate::build_id().into(), client: "kiln-upgrade".into() })?;
    let hello: ServerMsg = read_msg(&mut reader)?.ok_or_else(|| std::io::Error::other("daemon closed"))?;
    match hello {
        ServerMsg::Hello { can_upgrade: true, .. } => {
            write_msg(&mut writer, &ClientMsg::Upgrade { req: 0, exe: exe.to_string_lossy().into_owned() })?;
            // 데몬이 교체되며 연결이 닫힐 때까지 기다린다.
            let mut sink = [0u8; 4096];
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match std::io::Read::read(&mut reader, &mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            Ok(())
        }
        ServerMsg::Hello { .. } => Err(std::io::Error::other("daemon cannot upgrade in place")),
        _ => Err(std::io::Error::other("unexpected hello")),
    }
}

pub fn daemon_log_path(socket: &str) -> std::path::PathBuf {
    let p = Path::new(socket);
    if p.is_absolute() {
        p.with_file_name("daemon.log")
    } else {
        std::env::temp_dir().join("kiln-daemon.log")
    }
}

/// `exe daemon` 을 현재 프로세스와 분리해 실행한다.
pub fn spawn_daemon(exe: &Path, socket: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let exe_copy = daemon_copy(exe);
    #[cfg(windows)]
    let exe: &Path = exe_copy.as_deref().unwrap_or(exe);
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon").env("KILN_SOCKET", socket);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    let log = daemon_log_path(socket);
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        Ok(f) => {
            cmd.stderr(f);
        }
        Err(_) => {
            cmd.stderr(std::process::Stdio::null());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: fork 이후 setsid 만 호출한다.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // 부모의 표준 핸들(예: 파이프)을 데몬이 물려받아 부모 쪽 파이프가 닫히지 않는 것을 막는다.
        // SAFETY: 현재 프로세스의 표준 핸들 플래그만 바꾼다.
        unsafe {
            use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
            use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
            for h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                let handle = GetStdHandle(h);
                if !handle.is_null() {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
        // 부모의 잡 객체(예: SSH 세션)와 함께 종료되지 않도록 잡에서 분리를 먼저 시도한다.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        if let Ok(child) = cmd.spawn() {
            std::mem::forget(child);
            return Ok(());
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    let child = cmd.spawn()?;
    std::mem::forget(child);
    Ok(())
}

/// Windows 는 실행 중인 exe 를 덮어쓸 수 없으므로 데몬을 빌드별 복사본에서 실행한다.
/// 설치된 kiln.exe 는 잠기지 않아 업데이트로 교체할 수 있다.
#[cfg(windows)]
fn daemon_copy(exe: &Path) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)?.join("Kiln").join("daemon");
    std::fs::create_dir_all(&base).ok()?;
    let target = base.join(format!("kiln-daemon-{}.exe", crate::exe_build_id(exe)));
    if !target.exists() {
        let tmp = target.with_extension("tmp");
        std::fs::copy(exe, &tmp).ok()?;
        std::fs::rename(&tmp, &target).ok()?;
    }
    // 사용 중이 아닌 이전 복사본은 지운다(사용 중이면 삭제가 실패한다).
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            if e.path() != target {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Some(target)
}
