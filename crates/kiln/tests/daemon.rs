//! 실제 `kiln` 바이너리로 데몬을 띄워 세션 영속성과 무중단 업그레이드를 검증한다.

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SpawnSpec};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Daemon {
    socket: String,
    dir: PathBuf,
}

impl Daemon {
    fn start(tag: &str) -> Daemon {
        let dir = PathBuf::from(format!("/tmp/kt-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("d.sock").to_string_lossy().into_owned();
        Daemon { socket, dir }
    }

    fn client(&self) -> Client {
        Client::connect_or_spawn(&self.socket, &exe(), None).expect("connect")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(c) = Client::connect(&self.socket, None) {
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
    match c.request(|req| ClientMsg::Create { req, spec }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Created { session, .. } => session,
        m => panic!("{m:?}"),
    }
}

fn read(c: &Client, s: SessionId) -> String {
    match c.request(|req| ClientMsg::ReadText { req, session: s, history: 1000 }, Duration::from_secs(5)).unwrap() {
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
        if Instant::now() > deadline {
            panic!("timeout waiting for {needle:?}; screen:\n{t}");
        }
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

#[test]
fn session_survives_client_disconnect() {
    let d = Daemon::start("persist");
    let c = d.client();
    let s = create(&c, "/bin/sh", &[]);
    type_line(&c, s, "echo hello-$((40+2))");
    wait_for(&c, s, "hello-42");
    drop(c);

    // GUI 가 종료된 뒤 새 클라이언트가 붙는 상황.
    std::thread::sleep(Duration::from_millis(200));
    let c2 = d.client();
    let list = match c2.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Sessions { sessions, .. } => sessions,
        m => panic!("{m:?}"),
    };
    assert!(list.iter().any(|i| i.id == s && i.exited.is_none()));
    assert!(read(&c2, s).contains("hello-42"));
    type_line(&c2, s, "echo again-$((1+1))");
    wait_for(&c2, s, "again-2");
}

#[cfg(unix)]
#[test]
fn hot_upgrade_keeps_processes_screen_and_io() {
    let d = Daemon::start("upgrade");
    let c = d.client();
    let daemon_pid = c.server_pid;
    let s = create(&c, "/bin/sh", &[]);
    type_line(&c, s, "echo shellpid=$$");
    let text = wait_for(&c, s, "shellpid=");
    let shell_pid: u32 = text.lines().filter_map(|l| l.trim().strip_prefix("shellpid=")).filter_map(|v| v.trim().parse().ok()).next_back().expect("pid");
    // 장시간 실행 중인 자식 프로세스(에이전트 역할).
    type_line(&c, s, "sleep 300 & echo bg=$!");
    let text = wait_for(&c, s, "bg=");
    let bg_pid: u32 = text.lines().filter_map(|l| l.trim().strip_prefix("bg=")).filter_map(|v| v.trim().parse().ok()).next_back().expect("bg pid");

    c.send(ClientMsg::Upgrade { req: 99, exe: exe().to_string_lossy().into_owned() });
    // 업그레이드 후 재연결.
    let deadline = Instant::now() + Duration::from_secs(10);
    let c2 = loop {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(c2) = Client::connect(&d.socket, None) {
            if let Ok(ServerMsg::Sessions { sessions, .. }) = c2.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(2)) {
                if sessions.iter().any(|i| i.id == s) {
                    break c2;
                }
            }
        }
        assert!(Instant::now() < deadline, "daemon did not come back");
    };
    assert_eq!(c2.server_pid, daemon_pid, "exec 는 같은 pid 를 유지한다");
    assert!(alive(shell_pid), "셸이 살아 있어야 한다");
    assert!(alive(bg_pid), "백그라운드 작업이 살아 있어야 한다");
    let screen = read(&c2, s);
    assert!(screen.contains("shellpid="), "화면이 복원되어야 한다:\n{screen}");
    type_line(&c2, s, "echo after-$((6*7))");
    wait_for(&c2, s, "after-42");
    // 새 세션도 만들 수 있다.
    let s2 = create(&c2, "/bin/sh", &[]);
    assert!(s2 > s);
    type_line(&c2, s, "kill %1");
}

#[test]
fn osc_notification_sets_attention_and_is_broadcast() {
    let d = Daemon::start("notify");
    let c = d.client();
    let s = create(&c, "/bin/sh", &["-c", "sleep 0.3; printf '\\033]777;notify;Claude;needs input\\007'; sleep 5"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut got = false;
    while Instant::now() < deadline && !got {
        if let Ok(m) = c.rx.recv_timeout(Duration::from_millis(200)) {
            if let ServerMsg::Notification { session, title, body } = m {
                assert_eq!(session, s);
                assert_eq!(title, "Claude");
                assert_eq!(body, "needs input");
                got = true;
            }
        }
    }
    assert!(got, "notification not received");
    let list = match c.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(5)).unwrap() {
        ServerMsg::Sessions { sessions, .. } => sessions,
        m => panic!("{m:?}"),
    };
    let info = list.iter().find(|i| i.id == s).unwrap();
    assert!(info.attention);
    assert_eq!(info.last_notification.as_deref(), Some("Claude: needs input"));
}

#[test]
fn frames_are_incremental_after_first_full_frame() {
    let d = Daemon::start("frames");
    let c = d.client();
    let s = create(&c, "/bin/sh", &[]);
    c.send(ClientMsg::Attach { session: s, cols: 80, rows: 24 });
    let mut first_full = None;
    let mut partial_seen = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    type_line(&c, s, "echo x");
    while Instant::now() < deadline && !partial_seen {
        if let Ok(ServerMsg::Frame(f)) = c.rx.recv_timeout(Duration::from_millis(200)) {
            if first_full.is_none() {
                assert!(f.full);
                assert_eq!(f.lines.len(), 24);
                first_full = Some(());
            } else if !f.full {
                assert!(f.lines.len() < 24);
                partial_seen = true;
            }
        }
    }
    assert!(first_full.is_some() && partial_seen);
}

#[test]
fn exit_code_is_reported() {
    let d = Daemon::start("exit");
    let c = d.client();
    let s = create(&c, "/bin/sh", &["-c", "exit 7"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(ServerMsg::SessionExited { session, code }) = c.rx.recv_timeout(Duration::from_millis(200)) {
            if session == s {
                assert_eq!(code, Some(7));
                break;
            }
        }
        assert!(Instant::now() < deadline);
    }
}
