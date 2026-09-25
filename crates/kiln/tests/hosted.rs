//! pty-host 방식(Windows 기본, Unix 는 KILN_PTY_HOST=1) 의 업그레이드와 비정상 종료 복구를 검증한다.
//! 이 파일의 테스트는 모두 호스트 방식으로 돈다.

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SpawnSpec};
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

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
        let dir = PathBuf::from(format!("/tmp/kh-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Daemon { socket: dir.join("d.sock").to_string_lossy().into_owned(), dir }
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

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kiln"))
}

fn create(c: &Client, program: &str, args: &[&str]) -> SessionId {
    let spec = SpawnSpec {
        program: Some(program.into()),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: Some("/tmp".into()),
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

fn alive(pid: u32) -> bool {
    // SAFETY: 시그널 0 은 존재 여부만 확인한다.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn value(text: &str, key: &str) -> u32 {
    text.lines().filter_map(|l| l.trim().strip_prefix(key)).filter_map(|v| v.trim().parse().ok()).next_back().expect(key)
}

#[test]
fn hosted_upgrade_keeps_sessions_and_output_produced_during_handover() {
    let d = Daemon::start("upgrade");
    let c = d.client();
    let old_daemon = c.server_pid;
    let s = create(&c, "/bin/sh", &[]);
    type_line(&c, s, "echo shellpid=$$");
    let shell = value(&wait_for(&c, s, "shellpid="), "shellpid=");
    // 교체 도중(데몬이 없는 순간)에 나올 출력.
    type_line(&c, s, "(sleep 0.3; echo during-handover) &");
    std::thread::sleep(Duration::from_millis(100));
    c.send(ClientMsg::Upgrade { req: 1, exe: exe().to_string_lossy().into_owned() });
    std::thread::sleep(Duration::from_millis(200));
    let c2 = d.reconnect();
    assert_ne!(c2.server_pid, old_daemon, "호스트 방식은 새 데몬 프로세스로 바뀐다");
    assert!(alive(shell));
    wait_for(&c2, s, "during-handover");
    assert!(read(&c2, s).contains("shellpid="), "화면이 복원되어야 한다");
    type_line(&c2, s, "echo after-$((6*7))");
    wait_for(&c2, s, "after-42");
}

#[test]
fn daemon_crash_is_recovered_by_adopting_hosts() {
    let d = Daemon::start("crash");
    let c = d.client();
    let daemon_pid = c.server_pid;
    let s = create(&c, "/bin/sh", &[]);
    type_line(&c, s, "echo shellpid=$$; echo before-crash");
    let shell = value(&wait_for(&c, s, "before-crash"), "shellpid=");
    drop(c);
    // SAFETY: 테스트가 띄운 데몬을 강제 종료한다.
    unsafe { libc::kill(daemon_pid as i32, libc::SIGKILL) };
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
    type_line(&c2, s, "echo adopted-ok");
    wait_for(&c2, s, "adopted-ok");
    // 새 세션 id 는 입양한 id 와 겹치지 않는다.
    assert!(create(&c2, "/bin/sh", &[]) > s);
}

#[test]
fn hosted_exit_code_is_reported() {
    let d = Daemon::start("exit");
    let c = d.client();
    let s = create(&c, "/bin/sh", &["-c", "sleep 0.2; exit 3"]);
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
