//! 플랫폼별 의사 터미널(PTY) 생성과 입출력.

use kiln_proto::SpawnSpec;
use std::io;

pub enum ReadResult {
    Data(usize),
    Timeout,
    Eof,
    /// 호스트가 데몬 교체를 위해 연결을 끊었다(세션은 살아 있다).
    Detached,
}

pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into())
    }
    #[cfg(windows)]
    {
        if which_exists("pwsh.exe") { "pwsh.exe".into() } else { "powershell.exe".into() }
    }
}

#[cfg(windows)]
fn which_exists(exe: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).exists()))
        .unwrap_or(false)
}

fn base_env(session: u64) -> Vec<(String, String)> {
    vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "kiln".into()),
        ("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
        ("KILN_SESSION".into(), session.to_string()),
    ]
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    pub struct Pty {
        master: OwnedFd,
        pid: i32,
    }

    fn winsize(cols: u16, rows: u16) -> libc::winsize {
        libc::winsize { ws_row: rows.max(1), ws_col: cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
    }

    impl Pty {
        pub fn spawn(spec: &SpawnSpec, session: u64) -> io::Result<Pty> {
            let mut master: RawFd = -1;
            let mut slave: RawFd = -1;
            let mut ws = winsize(spec.cols, spec.rows);
            // SAFETY: 출력 포인터는 유효한 지역 변수를 가리킨다.
            let rc = unsafe {
                libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws)
            };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openpty 가 방금 연 fd 의 소유권을 가져온다.
            let master = unsafe { OwnedFd::from_raw_fd(master) };
            let slave = unsafe { OwnedFd::from_raw_fd(slave) };
            set_cloexec(master.as_raw_fd(), true)?;

            let (program, login) = match &spec.program {
                Some(p) if !p.is_empty() => (p.clone(), false),
                _ => (default_shell(), true),
            };
            let mut cmd = Command::new(&program);
            if login {
                let base = std::path::Path::new(&program)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "sh".into());
                cmd.arg0(format!("-{base}"));
            }
            cmd.args(&spec.args);
            let cwd = spec
                .cwd
                .clone()
                .filter(|c| std::path::Path::new(c).is_dir())
                .or_else(|| std::env::var("HOME").ok());
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
            for (k, v) in base_env(session) {
                cmd.env(k, v);
            }
            for (k, v) in &spec.env {
                cmd.env(k, v);
            }
            cmd.stdin(Stdio::from(slave.try_clone()?));
            cmd.stdout(Stdio::from(slave.try_clone()?));
            cmd.stderr(Stdio::from(slave));
            // SAFETY: fork 이후 async-signal-safe 함수만 호출한다.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    for sig in [libc::SIGCHLD, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGALRM, libc::SIGPIPE] {
                        libc::signal(sig, libc::SIG_DFL);
                    }
                    let empty: libc::sigset_t = std::mem::zeroed();
                    libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
                    Ok(())
                });
            }
            let child = cmd.spawn()?;
            let pid = child.id() as i32;
            // Child 핸들은 버리고 pid 로 waitpid 한다(업그레이드 후에도 같은 방식).
            std::mem::forget(child);
            Ok(Pty { master, pid })
        }

        /// 업그레이드로 상속된 fd 와 pid 로 복원한다.
        pub fn from_raw(fd: RawFd, pid: i32) -> io::Result<Pty> {
            // SAFETY: 이전 데몬이 넘긴 열린 fd 의 소유권을 가져온다.
            let master = unsafe { OwnedFd::from_raw_fd(fd) };
            set_cloexec(fd, true)?;
            Ok(Pty { master, pid })
        }

        pub fn raw_fd(&self) -> RawFd {
            self.master.as_raw_fd()
        }

        pub fn reader(&self) -> io::Result<PtyReader> {
            Ok(PtyReader { fd: self.master.try_clone()? })
        }

        pub fn writer(&self) -> io::Result<Box<dyn io::Write + Send>> {
            Ok(Box::new(std::fs::File::from(self.master.try_clone()?)))
        }

        pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
            let ws = winsize(cols, rows);
            // SAFETY: 유효한 fd 와 winsize 포인터.
            let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
            if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
        }

        pub fn pid(&self) -> u32 {
            self.pid as u32
        }

        /// 종료했으면 종료 코드(시그널 종료는 128+시그널)를 돌려준다.
        pub fn try_wait(&self) -> Option<i32> {
            let mut status = 0;
            // SAFETY: WNOHANG 으로 자식 상태만 조회한다.
            let rc = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if rc == self.pid {
                if libc::WIFEXITED(status) {
                    Some(libc::WEXITSTATUS(status))
                } else if libc::WIFSIGNALED(status) {
                    Some(128 + libc::WTERMSIG(status))
                } else {
                    None
                }
            } else if rc < 0 {
                Some(-1)
            } else {
                None
            }
        }

        pub fn kill(&self) {
            // SAFETY: 세션 리더의 프로세스 그룹 전체에 SIGHUP 을 보낸다.
            unsafe {
                libc::kill(-self.pid, libc::SIGHUP);
                libc::kill(self.pid, libc::SIGHUP);
            }
        }

        pub fn fg_pid(&self) -> Option<u32> {
            // SAFETY: 유효한 fd.
            let pg = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
            if pg > 0 { Some(pg as u32) } else { None }
        }
    }

    pub fn set_cloexec(fd: RawFd, on: bool) -> io::Result<()> {
        // SAFETY: fcntl 로 fd 플래그만 바꾼다.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            let nf = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
            if libc::fcntl(fd, libc::F_SETFD, nf) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub struct PtyReader {
        fd: OwnedFd,
    }

    impl PtyReader {
        pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<ReadResult> {
            let mut pfd = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: 단일 pollfd.
            let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if rc < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    return Ok(ReadResult::Timeout);
                }
                return Err(e);
            }
            if rc == 0 {
                return Ok(ReadResult::Timeout);
            }
            // SAFETY: buf 는 유효한 가변 슬라이스.
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                Ok(ReadResult::Data(n as usize))
            } else if n == 0 {
                Ok(ReadResult::Eof)
            } else {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    // 슬레이브가 모두 닫히면 Linux 는 EIO 를 돌려준다.
                    Some(libc::EIO) => Ok(ReadResult::Eof),
                    Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(ReadResult::Timeout),
                    _ => Err(e),
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use parking_lot::Mutex;
    use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
    use std::io::Read;

    pub struct Pty {
        master: Mutex<Box<dyn MasterPty + Send>>,
        child: Mutex<Box<dyn Child + Send + Sync>>,
        writer: Mutex<Option<Box<dyn io::Write + Send>>>,
        pid: u32,
    }

    fn size(cols: u16, rows: u16) -> PtySize {
        PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 }
    }

    impl Pty {
        pub fn spawn(spec: &SpawnSpec, session: u64) -> io::Result<Pty> {
            let sys = native_pty_system();
            let pair = sys.openpty(size(spec.cols, spec.rows)).map_err(io::Error::other)?;
            let program = spec.program.clone().filter(|p| !p.is_empty()).unwrap_or_else(default_shell);
            let mut cmd = CommandBuilder::new(program);
            cmd.args(&spec.args);
            let cwd = spec.cwd.clone().filter(|c| std::path::Path::new(c).is_dir())
                .or_else(|| std::env::var("USERPROFILE").ok());
            if let Some(cwd) = cwd {
                cmd.cwd(cwd);
            }
            for (k, v) in base_env(session) {
                cmd.env(k, v);
            }
            for (k, v) in &spec.env {
                cmd.env(k, v);
            }
            let child = pair.slave.spawn_command(cmd).map_err(io::Error::other)?;
            let pid = child.process_id().unwrap_or(0);
            let writer = pair.master.take_writer().map_err(io::Error::other)?;
            Ok(Pty { master: Mutex::new(pair.master), child: Mutex::new(child), writer: Mutex::new(Some(writer)), pid })
        }

        pub fn reader(&self) -> io::Result<PtyReader> {
            Ok(PtyReader { inner: self.master.lock().try_clone_reader().map_err(io::Error::other)? })
        }

        pub fn writer(&self) -> io::Result<Box<dyn io::Write + Send>> {
            self.writer.lock().take().ok_or_else(|| io::Error::other("writer taken"))
        }

        pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
            self.master.lock().resize(size(cols, rows)).map_err(io::Error::other)
        }

        pub fn pid(&self) -> u32 {
            self.pid
        }

        pub fn try_wait(&self) -> Option<i32> {
            match self.child.lock().try_wait() {
                Ok(Some(st)) => Some(st.exit_code() as i32),
                Ok(None) => None,
                Err(_) => Some(-1),
            }
        }

        pub fn kill(&self) {
            let _ = self.child.lock().kill();
        }

        pub fn fg_pid(&self) -> Option<u32> {
            None
        }
    }

    pub struct PtyReader {
        inner: Box<dyn Read + Send>,
    }

    impl PtyReader {
        pub fn read_timeout(&mut self, buf: &mut [u8], _timeout_ms: i32) -> io::Result<ReadResult> {
            match self.inner.read(buf) {
                Ok(0) => Ok(ReadResult::Eof),
                Ok(n) => Ok(ReadResult::Data(n)),
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(ReadResult::Eof),
                Err(e) => Err(e),
            }
        }
    }
}

pub use imp::*;
