//! Adversarial hosted-PTY handover tests. Every process/socket belongs to a fixture.
//! No global environment or installed/user daemon is accessed.
#![cfg(unix)]

use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SessionInfo, SpawnSpec};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    daemon: Child,
    socket: String,
    dir: tempfile::TempDir,
    sessions: Vec<SessionId>,
    children: Vec<u32>,
}

impl Fixture {
    fn start() -> (Self, Client) {
        let dir = tempfile::Builder::new().prefix("kiln-host-failure-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("daemon.sock").to_string_lossy().into_owned();
        let daemon = Command::new(exe())
            .args(["daemon", "--foreground", "--socket", &socket])
            .env("KILN_PTY_HOST", "1").env("KILN_SOCKET", &socket)
            .env("KILN_CONFIG_DIR", dir.path().join("config"))
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .env("RUST_LOG", "info").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
            .spawn().unwrap();
        let fixture = Self { daemon, socket, dir, sessions: Vec::new(), children: Vec::new() };
        let client = fixture.connect(Duration::from_secs(8));
        assert_eq!(client.server_pid, fixture.daemon.id());
        (fixture, client)
    }

    fn connect(&self, timeout: Duration) -> Client {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(client) = Client::connect(&self.socket, None) { return client; }
            assert!(Instant::now() < deadline, "fixture daemon did not become available");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn spawn(&mut self, client: &Client, script: Option<&str>) -> SessionId {
        let args = script.map(|s| vec!["-c".into(), s.into()]).unwrap_or_else(|| vec!["-i".into()]);
        let result = client.request(|req| ClientMsg::Create { req, spec: SpawnSpec {
            program: Some("/bin/sh".into()), args,
            cwd: Some(self.dir.path().to_string_lossy().into_owned()),
            env: vec![("PS1".into(), "fixture> ".into()), ("ENV".into(), "/dev/null".into())],
            cols: 100, rows: 24, ..Default::default()
        } }, Duration::from_secs(8)).unwrap();
        let ServerMsg::Created { session, .. } = result else { panic!("unexpected create response: {result:?}") };
        self.sessions.push(session);
        self.children.push(info(client, session).pid);
        session
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() { eprintln!("FIXTURE LOG: {}", std::fs::read_to_string(kiln_daemon::client::daemon_log_path(&self.socket)).unwrap_or_default()); }
        // This socket is under our unique TempDir. It can point to our successor
        // after an upgrade, so shutdown through it before reaping the first PID.
        if let Ok(client) = Client::connect(&self.socket, None) {
            for &session in &self.sessions { client.send(ClientMsg::Kill { session }); }
            client.send(ClientMsg::Shutdown);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Path::new(&self.socket).exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // A failing control-path regression must not strand its owned shell.
        // Verify the unique private cwd before signaling a recorded Create PID,
        // rather than trusting a PID that might have been reused after exit.
        let owned_root = self.dir.path().canonicalize().unwrap();
        for &pid in &self.children {
            if kiln_daemon::procinfo::cwd(pid).is_some_and(|cwd| Path::new(&cwd).starts_with(&owned_root)) {
                unsafe { libc::kill(pid as i32, libc::SIGHUP); }
            }
        }
        // Child::kill addresses only the process we spawned, never an arbitrary PID.
        if self.daemon.try_wait().ok().flatten().is_none() { let _ = self.daemon.kill(); }
        let _ = self.daemon.wait();
    }
}

struct OwnedStateDirectory(PathBuf);
impl OwnedStateDirectory {
    fn obstruct(pid: u32) -> Self {
        let path = std::env::temp_dir().join(format!("kiln-upgrade-{pid}.state"));
        assert!(!path.exists(), "refuse to touch pre-existing upgrade state: {}", path.display());
        // create_dir is atomic and refuses an existing target; never remove before creating.
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for OwnedStateDirectory {
    fn drop(&mut self) { let _ = std::fs::remove_dir(&self.0); }
}

/// Real protocol proxy: hold the first reconnect's Hello after the actual host
/// has accepted Attach. Existing pre-proxy connections remain untouched.
struct AttachBarrier {
    original: PathBuf,
    backend: PathBuf,
    started: mpsc::Receiver<Result<(), String>>,
    release: Option<mpsc::Sender<()>>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<UnixStream>>>,
    accept: Option<std::thread::JoinHandle<()>>,
}

impl AttachBarrier {
    fn install(fixture: &Fixture, session: SessionId) -> Self {
        use std::os::unix::fs::FileTypeExt;
        let prefix = format!("pty-{session}-");
        let paths: Vec<_> = std::fs::read_dir(fixture.dir.path()).unwrap().map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix) && entry.file_type().unwrap().is_socket())
            .map(|entry| entry.path()).collect();
        assert_eq!(paths.len(), 1, "expected one owned host endpoint");
        let original = paths[0].clone();
        let backend = fixture.dir.path().join("held-host.sock");
        assert!(!backend.exists(), "refuse to replace an existing proxy backend");
        std::fs::rename(&original, &backend).unwrap();
        let listener = match UnixListener::bind(&original) {
            Ok(listener) => listener,
            Err(error) => { std::fs::rename(&backend, &original).unwrap(); panic!("proxy bind: {error}"); }
        };
        listener.set_nonblocking(true).unwrap();
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::<UnixStream>::new()));
        let accept_stop = stop.clone();
        let accepted_sockets = sockets.clone();
        let backend_path = backend.clone();
        let accept = std::thread::spawn(move || {
            let mut workers = Vec::new();
            let mut first = true;
            while !accept_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((client, _)) => {
                        // macOS accepted descriptors can inherit O_NONBLOCK.
                        // Only the listener polls; the protocol relay must block.
                        client.set_nonblocking(false).unwrap();
                        let hold = first; first = false;
                        let backend_path = backend_path.clone();
                        let sockets = accepted_sockets.clone();
                        let worker_stop = accept_stop.clone();
                        let started_tx = started_tx.clone();
                        let release_rx = release_rx.clone();
                        workers.push(std::thread::spawn(move || {
                            let result = (|| -> std::io::Result<()> {
                                let mut client = client;
                                let mut backend = UnixStream::connect(backend_path)?;
                                for stream in [&client, &backend] {
                                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                                    let mut owned = sockets.lock().unwrap();
                                    owned.push(stream.try_clone()?);
                                    if worker_stop.load(Ordering::SeqCst) {
                                        let _ = stream.shutdown(std::net::Shutdown::Both);
                                        return Err(std::io::Error::other("proxy cleanup started"));
                                    }
                                }
                                let attach: kiln_daemon::ptyhost::ToHost = kiln_proto::read_msg(&mut client)?
                                    .ok_or_else(|| std::io::Error::other("proxy client closed before Attach"))?;
                                if !matches!(attach, kiln_daemon::ptyhost::ToHost::Attach { .. }) {
                                    return Err(std::io::Error::other("proxy expected Attach"));
                                }
                                kiln_proto::write_msg(&mut backend, &attach)?;
                                let hello: kiln_daemon::ptyhost::FromHost = kiln_proto::read_msg(&mut backend)?
                                    .ok_or_else(|| std::io::Error::other("actual host closed before Hello"))?;
                                if !matches!(hello, kiln_daemon::ptyhost::FromHost::Hello { .. }) {
                                    return Err(std::io::Error::other("proxy expected actual host Hello"));
                                }
                                if hold {
                                    let _ = started_tx.send(Ok(()));
                                    release_rx.lock().unwrap().recv_timeout(Duration::from_secs(15))
                                        .map_err(|e| std::io::Error::other(format!("Attach barrier unreleased: {e}")))?;
                                }
                                kiln_proto::write_msg(&mut client, &hello)?;
                                for stream in [&client, &backend] {
                                    stream.set_read_timeout(None)?; stream.set_write_timeout(None)?;
                                }
                                let mut upstream_client = client.try_clone()?;
                                let mut upstream_backend = backend.try_clone()?;
                                let upstream = std::thread::spawn(move || {
                                    let _ = std::io::copy(&mut upstream_client, &mut upstream_backend);
                                    // Preserve bytes buffered in the other direction.
                                    // Closing both sides here can discard a queued Kill.
                                    let _ = upstream_backend.shutdown(std::net::Shutdown::Write);
                                });
                                let _ = std::io::copy(&mut backend, &mut client);
                                let _ = client.shutdown(std::net::Shutdown::Write);
                                let _ = upstream.join();
                                let _ = backend.shutdown(std::net::Shutdown::Both);
                                let _ = client.shutdown(std::net::Shutdown::Both);
                                Ok(())
                            })();
                            if hold { if let Err(error) = result { let _ = started_tx.send(Err(error.to_string())); } }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(error) => { let _ = started_tx.send(Err(format!("proxy accept: {error}"))); break; }
                }
            }
            for stream in accepted_sockets.lock().unwrap().iter() { let _ = stream.shutdown(std::net::Shutdown::Both); }
            for worker in workers { let _ = worker.join(); }
        });
        Self { original, backend, started, release: Some(release), stop, sockets, accept: Some(accept) }
    }

    fn wait_started(&self) {
        self.started.recv_timeout(Duration::from_secs(5)).expect("actual host did not accept rollback Attach")
            .expect("actual host/proxy handshake failed");
    }

    fn release(&mut self) {
        if let Some(release) = self.release.take() { let _ = release.send(()); }
    }
}

impl Drop for AttachBarrier {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        for stream in self.sockets.lock().unwrap().iter() { let _ = stream.shutdown(std::net::Shutdown::Both); }
        if let Some(accept) = self.accept.take() { let _ = accept.join(); }
        // Restore only the endpoint renamed from this fixture, so cleanup can
        // still reach its actual host. Never overwrite an unrelated path.
        let _ = std::fs::remove_file(&self.original);
        if self.backend.exists() { let _ = std::fs::rename(&self.backend, &self.original); }
    }
}

fn exe() -> PathBuf { PathBuf::from(env!("CARGO_BIN_EXE_kiln")) }
fn input(client: &Client, session: SessionId, command: &str) {
    client.send(ClientMsg::Input { session, data: format!("{command}\r").into_bytes() });
}
fn info(client: &Client, session: SessionId) -> SessionInfo {
    let response = client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
    let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected list: {response:?}") };
    sessions.into_iter().find(|s| s.id == session).expect("owned session disappeared")
}
fn wait_info(client: &Client, session: SessionId, predicate: impl Fn(&SessionInfo) -> bool) -> SessionInfo {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = info(client, session);
        if predicate(&value) { return value; }
        assert!(Instant::now() < deadline, "session monitor did not recover: {value:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn wait_text(client: &Client, session: SessionId, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = client.request(|req| ClientMsg::ReadText { req, session, history: 200 }, Duration::from_secs(1)).unwrap();
        let ServerMsg::Text { text, .. } = response else { panic!("unexpected text: {response:?}") };
        if text.contains(needle) { return text; }
        assert!(Instant::now() < deadline, "input/output stopped; expected {needle:?}: {text}");
        std::thread::sleep(Duration::from_millis(30));
    }
}
fn failed_upgrade(client: &Client, executable: &Path) {
    let start = Instant::now();
    let result = client.request(|req| ClientMsg::Upgrade { req, exe: executable.to_string_lossy().into_owned() }, Duration::from_secs(5));
    let error = result.expect_err("obstructed upgrade unexpectedly succeeded").to_string();
    assert!(error.contains("upgrade failed:"), "expected a daemon rollback error, got {error}");
    assert!(start.elapsed() < Duration::from_secs(5), "rollback was not bounded");
}

fn assert_resumed_monitor_and_input(fixture: &Fixture, client: &Client, session: SessionId, shell: u32) {
    // Wait beyond the monitor period before changing state; stale pre-upgrade
    // SessionInfo cannot satisfy the subsequent foreground/cwd checks.
    std::thread::sleep(Duration::from_millis(1200));
    let after = fixture.dir.path().join("after-rollback");
    std::fs::create_dir(&after).unwrap();
    input(client, session, &format!("cd '{}'; printf '\\nROLLBACK_%s_OK\\n' INPUT; sleep 4", after.display()));
    wait_text(client, session, "ROLLBACK_INPUT_OK");
    let expected_cwd = after.canonicalize().unwrap().to_string_lossy().into_owned();
    let running = wait_info(client, session, |s| s.fg_process.as_deref() == Some("sleep") && s.cwd.as_deref() == Some(expected_cwd.as_str()));
    assert_eq!(running.pid, shell, "rollback replaced the shell");
    wait_info(client, session, |s| s.fg_process.as_deref() == Some("sh"));
    input(client, session, "printf '\\nSECOND_%s_OK\\n' INPUT");
    wait_text(client, session, "SECOND_INPUT_OK");
}

#[test]
fn hosted_state_write_failure_restores_input_and_live_monitoring() {
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, None);
    let before = wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sh"));
    let obstruction = OwnedStateDirectory::obstruct(client.server_pid);
    failed_upgrade(&client, &exe());
    assert!(obstruction.0.is_dir(), "rollback must not remove an unrelated directory as a state file");
    assert_eq!(fixture.connect(Duration::from_secs(1)).server_pid, client.server_pid);
    assert_resumed_monitor_and_input(&fixture, &client, session, before.pid);
}

#[test]
fn hosted_invalid_executable_rollback_keeps_existing_reader_and_writer() {
    use std::os::unix::fs::PermissionsExt;
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, None);
    let before = wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sh"));
    // An existing non-executable file passes the path-exists preflight but
    // fails process creation after hosts have detached and state was written.
    let invalid = fixture.dir.path().join("invalid-executable");
    std::fs::write(&invalid, b"not an executable image\n").unwrap();
    std::fs::set_permissions(&invalid, std::fs::Permissions::from_mode(0o600)).unwrap();
    failed_upgrade(&client, &invalid);
    assert_eq!(fixture.connect(Duration::from_secs(1)).server_pid, client.server_pid);
    assert_resumed_monitor_and_input(&fixture, &client, session, before.pid);
}

#[test]
fn kill_during_real_host_reconnect_does_not_resurrect_removed_session() {
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, None);
    let shell = wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sh")).pid;
    let _obstruction = OwnedStateDirectory::obstruct(client.server_pid);
    let mut barrier = AttachBarrier::install(&fixture, session);
    failed_upgrade(&client, &exe());
    barrier.wait_started();
    // Kill can target the detached connection; the reconnect's fresh writer
    // must also receive it after Hello is released.
    client.send(ClientMsg::Kill { session });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let response = client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
        let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected {response:?}") };
        if !sessions.iter().any(|s| s.id == session) { break; }
        assert!(Instant::now() < deadline, "Kill blocked behind the reconnect barrier");
    }
    barrier.release();
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(shell as i32, 0) == 0 } {
        if Instant::now() >= deadline {
            let state = Command::new("/bin/ps").args(["-p", &shell.to_string(), "-o", "pid,ppid,pgid,state,command"]).output().unwrap();
            panic!("removed session's owned child survived: {}", String::from_utf8_lossy(&state.stdout));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let response = client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
    let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected {response:?}") };
    assert!(!sessions.iter().any(|s| s.id == session), "reconnect resurrected a removed session");
}

#[test]
fn second_upgrade_waits_for_real_inflight_host_reconnect_and_safely_aborts() {
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, None);
    let shell = wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sh")).pid;
    let obstruction = OwnedStateDirectory::obstruct(client.server_pid);
    let mut barrier = AttachBarrier::install(&fixture, session);
    failed_upgrade(&client, &exe());
    barrier.wait_started();
    let start = Instant::now();
    let error = client.request(|req| ClientMsg::Upgrade { req, exe: exe().to_string_lossy().into_owned() }, Duration::from_secs(4))
        .expect_err("an in-flight reserved reader must prevent handover").to_string();
    assert!(error.contains("terminal host did not pause"), "expected bounded reader-drain abort, got {error}");
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(fixture.connect(Duration::from_secs(1)).server_pid, client.server_pid);
    assert!(obstruction.0.is_dir(), "second upgrade wrote a new snapshot over the obstruction");
    assert!(!kiln_daemon::ptyhost::handover_marker(&fixture.socket).exists(), "aborted upgrade left a handover marker");
    barrier.release();
    assert_resumed_monitor_and_input(&fixture, &client, session, shell);
}

#[test]
fn blocked_host_input_does_not_block_upgrade_kill_or_shutdown() {
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, Some("stty -echo -icanon min 1 time 0; printf 'BLOCK_READY\\n'; exec sleep 10"));
    wait_text(&client, session, "BLOCK_READY");
    let blocked_pid = info(&client, session).pid;
    // Far larger than PTY input capacity; the child deliberately never reads it.
    client.send(ClientMsg::Input { session, data: vec![b'x'; 8 * 1024 * 1024] });
    std::thread::sleep(Duration::from_millis(150));
    let request = client.next_req();
    let start = Instant::now();
    client.send(ClientMsg::Upgrade { req: request, exe: exe().to_string_lossy().into_owned() });
    let deadline = start + Duration::from_millis(4500);
    let control = loop {
        let _ = fixture.daemon.try_wait(); // reap our foreground child after handover
        match client.rx.recv_timeout(Duration::from_millis(30)) {
            Ok(ServerMsg::Error { req, message }) if req == request => {
                assert!(message.contains("upgrade failed:"), "unexpected upgrade failure: {message}");
                break fixture.connect(Duration::from_secs(1));
            }
            _ => {}
        }
        if let Ok(next) = Client::connect(&fixture.socket, None) {
            if next.server_pid != client.server_pid { break next; }
        }
        assert!(Instant::now() < deadline, "blocked PTY input wedged upgrade/control for more than 4.5s");
    };
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(matches!(control.request(|req| ClientMsg::Ping { req }, Duration::from_secs(1)).unwrap(), ServerMsg::Pong { .. }));
    control.send(ClientMsg::Kill { session });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let response = control.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(1)).unwrap();
        let ServerMsg::Sessions { sessions, .. } = response else { panic!("unexpected {response:?}") };
        let alive = unsafe { libc::kill(blocked_pid as i32, 0) == 0 };
        if !sessions.iter().any(|s| s.id == session) && !alive { break; }
        assert!(Instant::now() < deadline, "Kill did not terminate the owned blocked-input child");
        std::thread::sleep(Duration::from_millis(20));
    }
    control.send(ClientMsg::Shutdown);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Path::new(&fixture.socket).exists() {
        assert!(Instant::now() < deadline, "shutdown unusable after blocked host input");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "macos")]
fn foreground_pid(shell: u32) -> u32 {
    let mut value: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&value) as i32;
    let read = unsafe { libc::proc_pidinfo(shell as i32, libc::PROC_PIDTBSDINFO, 0, (&mut value as *mut libc::proc_bsdinfo).cast(), size) };
    assert_eq!(read, size, "owned shell foreground metadata unavailable");
    kiln_daemon::procinfo::foreground_pid(value.e_tpgid).expect("foreground job disappeared")
}

#[cfg(target_os = "macos")]
#[test]
fn hosted_upgrade_keeps_same_foreground_job_after_monitor_tick() {
    let (mut fixture, client) = Fixture::start();
    let session = fixture.spawn(&client, None);
    wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sh"));
    input(&client, session, "sleep 20");
    let before = wait_info(&client, session, |s| s.fg_process.as_deref() == Some("sleep"));
    let foreground = foreground_pid(before.pid);
    client.send(ClientMsg::Upgrade { req: client.next_req(), exe: exe().to_string_lossy().into_owned() });
    let deadline = Instant::now() + Duration::from_secs(7);
    let successor = loop {
        let _ = fixture.daemon.try_wait(); // production daemons are reaped by their launcher
        if let Ok(next) = Client::connect(&fixture.socket, None) {
            if next.server_pid != client.server_pid { break next; }
        }
        assert!(Instant::now() < deadline, "hosted handover did not complete");
        std::thread::sleep(Duration::from_millis(30));
    };
    // A saved stale fg_process must survive a real new monitor tick, not merely
    // be echoed from the upgrade state while cached HostPty fg starts at zero.
    std::thread::sleep(Duration::from_millis(1300));
    let after = info(&successor, session);
    assert_eq!(after.pid, before.pid);
    assert_eq!(after.fg_process, before.fg_process);
    assert_eq!(foreground_pid(after.pid), foreground);
    assert_eq!(after.exited, None);
}
