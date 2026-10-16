//! pty-host 방식(Windows 기본, Unix 는 KILN_PTY_HOST=1) 의 업그레이드와 비정상 종료 복구를 검증한다.
//! 이 파일의 테스트는 모두 호스트 방식으로 돈다.

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SpawnSpec};
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

#[path = "support/upgrade.rs"]
mod upgrade;

static INIT: Once = Once::new();

fn init() {
    // SAFETY: 테스트 시작 시 한 번 설정한다.
    INIT.call_once(|| unsafe { std::env::set_var("KILN_PTY_HOST", "1") });
}

struct Daemon {
    socket: String,
    dir: PathBuf,
}

impl Daemon {
    fn start(tag: &str) -> Daemon {
        init();
        #[cfg(unix)]
        let dir = PathBuf::from(format!("/tmp/kh-{}-{tag}", std::process::id()));
        #[cfg(windows)]
        let dir = std::env::temp_dir().join(format!("kh-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        let socket = dir.join("d.sock").to_string_lossy().into_owned();
        #[cfg(windows)]
        let socket = format!("kiln-hosted-test-{}-{tag}", std::process::id());
        Daemon { socket, dir }
    }

    fn client(&self) -> Client {
        Client::connect_or_spawn(&self.socket, &exe(), None).expect("connect")
    }

    fn reconnect(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(c) = Client::connect_or_spawn(&self.socket, &exe(), None) {
                if let Ok(ServerMsg::Sessions { .. }) = c.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(2)) {
                    return c;
                }
            }
            assert!(Instant::now() < deadline, "daemon did not come back");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(c) = Client::connect(&self.socket, None) {
            if let Ok(ServerMsg::Sessions { sessions, .. }) = c.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(2)) {
                for s in sessions {
                    c.send(ClientMsg::Kill { session: s.id });
                }
            }
            std::thread::sleep(Duration::from_millis(200));
            c.send(ClientMsg::Shutdown);
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The kiln executable; `KILN_TEST_EXE` names it when the tests run on another machine.
fn exe() -> PathBuf {
    std::env::var_os("KILN_TEST_EXE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_kiln")))
}

/// The platform shell and the commands the tests type into it.
#[cfg(unix)]
mod sh {
    pub const PROGRAM: &str = "/bin/sh";
    pub const ARGS: &[&str] = &[];
    pub const PRINT_PID: &str = "echo shellpid=$$";
    pub const EXIT_3_LATER: &[&str] = &["-c", "sleep 0.2; exit 3"];
    /// Prints `head` and `tail` joined, without the joined text in the typed line.
    pub fn echo_joined(head: &str, tail: &str) -> String {
        format!("echo {head}''{tail}")
    }
    /// Prints the joined text 300 ms later while the shell keeps reading input.
    pub fn echo_joined_later(head: &str, tail: &str) -> String {
        format!("(sleep 0.3; {}) &", echo_joined(head, tail))
    }
    pub const PRINT_42: &str = "echo after-$((6*7))";
}
#[cfg(windows)]
mod sh {
    pub const PROGRAM: &str = "powershell.exe";
    pub const ARGS: &[&str] = &["-NoLogo", "-NoProfile"];
    pub const PRINT_PID: &str = "echo \"shellpid=$PID\"";
    pub const EXIT_3_LATER: &[&str] = &["-NoLogo", "-NoProfile", "-Command", "Start-Sleep -Milliseconds 200; exit 3"];
    /// Prints `head` and `tail` joined, without the joined text in the typed line.
    pub fn echo_joined(head: &str, tail: &str) -> String {
        format!("echo \"{head}$('{tail}')\"")
    }
    /// Prints the joined text 300 ms later; the shell reads the next input afterwards.
    pub fn echo_joined_later(head: &str, tail: &str) -> String {
        format!("Start-Sleep -Milliseconds 300; {}", echo_joined(head, tail))
    }
    pub const PRINT_42: &str = "echo \"after-$(6*7)\"";
}

fn create_shell(c: &Client) -> SessionId {
    create(c, sh::PROGRAM, sh::ARGS)
}

fn create(c: &Client, program: &str, args: &[&str]) -> SessionId {
    let spec = SpawnSpec {
        program: Some(program.into()),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        cols: 80,
        rows: 24,
        env: vec![("PS1".into(), "$ ".into())],
        ..Default::default()
    };
    match c.request(|req| ClientMsg::Create { req, spec }, Duration::from_secs(10)).unwrap() {
        ServerMsg::Created { session, .. } => session,
        m => panic!("{m:?}"),
    }
}

fn read(c: &Client, s: SessionId) -> String {
    match c.request(|req| ClientMsg::ReadText { req, session: s, history: 2000 }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Text { text, .. } => text,
        m => panic!("{m:?}"),
    }
}

fn wait_for(c: &Client, s: SessionId, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let t = read(c, s);
        if t.contains(needle) {
            return t;
        }
        assert!(Instant::now() < deadline, "timeout waiting for {needle:?}; screen:\n{t}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn type_line(c: &Client, s: SessionId, line: &str) {
    c.send(ClientMsg::Input { session: s, data: format!("{line}\r").into_bytes() });
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: 시그널 0 은 존재 여부만 확인한다.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is checked before use and closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let queried = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        queried && code == STILL_ACTIVE as u32
    }
}

/// Ends a process the test started, without letting it clean up.
fn kill_hard(pid: u32) {
    #[cfg(unix)]
    // SAFETY: 테스트가 띄운 데몬을 강제 종료한다.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    // SAFETY: the handle is checked before use and closed once.
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!handle.is_null(), "cannot open process {pid}");
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}

/// Waits for an output line `key<number>` and returns the number.
fn wait_value(c: &Client, s: SessionId, key: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = read(c, s);
        if let Some(value) = text.lines().filter_map(|l| l.trim().strip_prefix(key)).filter_map(|v| v.trim().parse().ok()).next_back() {
            return value;
        }
        assert!(Instant::now() < deadline, "timeout waiting for {key:?}; screen:\n{text}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hosted_upgrade_keeps_sessions_and_output_produced_during_handover() {
    let d = Daemon::start("upgrade");
    let c = d.client();
    let old_daemon = c.server_pid;
    let s = create_shell(&c);
    type_line(&c, s, sh::PRINT_PID);
    let shell = wait_value(&c, s, "shellpid=");
    // 교체 도중(데몬이 없는 순간)에 나올 출력.
    type_line(&c, s, &sh::echo_joined_later("during-hand", "over"));
    std::thread::sleep(Duration::from_millis(100));
    c.send(ClientMsg::Upgrade { req: 1, exe: exe().to_string_lossy().into_owned() });
    upgrade::disconnected(&c, 1);
    let c2 = d.reconnect();
    assert_ne!(c2.server_pid, old_daemon, "호스트 방식은 새 데몬 프로세스로 바뀐다");
    assert!(alive(shell));
    wait_for(&c2, s, "during-handover");
    assert!(read(&c2, s).contains("shellpid="), "화면이 복원되어야 한다");
    type_line(&c2, s, sh::PRINT_42);
    wait_for(&c2, s, "after-42");
}

#[test]
fn daemon_crash_is_recovered_by_adopting_hosts() {
    let d = Daemon::start("crash");
    let c = d.client();
    let daemon_pid = c.server_pid;
    let s = create_shell(&c);
    type_line(&c, s, sh::PRINT_PID);
    let shell = wait_value(&c, s, "shellpid=");
    // A program reads its cursor-position answer; the query in the history must not be
    // answered again after adoption.
    #[cfg(unix)]
    {
        type_line(&c, s, r#"stty -echo -icanon; printf '\033[6n'; while c=$(dd bs=1 count=1 2>/dev/null) && [ "$c" != R ]; do :; done; stty sane; echo answer-''read"#);
        wait_for(&c, s, "answer-read");
    }
    type_line(&c, s, &sh::echo_joined("before-", "crash"));
    wait_for(&c, s, "before-crash");
    drop(c);
    kill_hard(daemon_pid);
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(shell), "데몬이 죽어도 셸은 산다");
    let c2 = d.reconnect();
    assert_ne!(c2.server_pid, daemon_pid);
    let list = match c2.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Sessions { sessions, .. } => sessions,
        m => panic!("{m:?}"),
    };
    assert!(list.iter().any(|i| i.id == s && i.pid == shell), "{list:?}");
    wait_for(&c2, s, "before-crash");
    type_line(&c2, s, &sh::echo_joined("adopted-", "ok"));
    wait_for(&c2, s, "adopted-ok");
    // 새 세션 id 는 입양한 id 와 겹치지 않는다.
    assert!(create_shell(&c2) > s);
}

#[test]
fn hosted_exit_code_is_reported() {
    let d = Daemon::start("exit");
    let c = d.client();
    let s = create(&c, sh::PROGRAM, sh::EXIT_3_LATER);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(ServerMsg::SessionExited { session, code }) = c.rx.recv_timeout(Duration::from_millis(200)) {
            if session == s {
                assert_eq!(code, Some(3));
                break;
            }
        }
        assert!(Instant::now() < deadline);
    }
}

/// A killed session takes the processes its shell started with it.
#[cfg(windows)]
#[test]
fn killed_session_leaves_no_descendants() {
    let d = Daemon::start("tree");
    let c = d.client();
    let s = create_shell(&c);
    type_line(&c, s, "$p = Start-Process powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep 60' -WindowStyle Hidden -PassThru; echo \"childpid=$($p.Id)\"");
    let child = wait_value(&c, s, "childpid=");
    assert!(alive(child));
    c.send(ClientMsg::Kill { session: s });
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(child) {
        assert!(Instant::now() < deadline, "the shell's child {child} outlived its killed session");
        std::thread::sleep(Duration::from_millis(100));
    }
}

