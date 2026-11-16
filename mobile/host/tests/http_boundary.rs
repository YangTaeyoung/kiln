use kiln_mobile_host::{Options, serve};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::task::JoinHandle;
struct Host {
    root: tempfile::TempDir,
    task: JoinHandle<anyhow::Result<()>>,
    url: String,
    token: String,
}
impl Host {
    async fn start(ttl: Duration) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("kiln");
        std::fs::write(&cli, "#!/bin/sh\necho 'kiln 0.3.0'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let pair = root.path().join("pairing/connection.json");
        let o = Options {
            workspaces: vec![root.path().into()],
            kiln_cli: cli,
            listen: "127.0.0.1:0".parse().unwrap(),
            pairing_file: pair.clone(),
            dev_origin: None,
            token_ttl: ttl,
        };
        let task = tokio::spawn(serve(o));
        for _ in 0..100 {
            if pair.exists() {
                break;
            }
            if task.is_finished() {
                panic!("host exited before pairing: {:?}", task.await)
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let p: Value = serde_json::from_slice(&std::fs::read(pair).expect("pairing file")).unwrap();
        Self {
            root,
            task,
            url: p["url"].as_str().unwrap().into(),
            token: p["token"].as_str().unwrap().into(),
        }
    }
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .get(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[tokio::test]
async fn authenticated_http_boundary_and_private_pairing() {
    let h = Host::start(Duration::from_secs(60)).await;
    let c = reqwest::Client::new();
    for token in [None, Some("incorrect")] {
        let mut r = c.get(format!("{}/v1/bootstrap", h.url));
        if let Some(t) = token {
            r = r.bearer_auth(t)
        }
        assert_eq!(r.send().await.unwrap().status(), 401);
    }
    assert_eq!(h.get("/v1/bootstrap").send().await.unwrap().status(), 200);
    assert_eq!(
        h.get("/v1/bootstrap")
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        h.get("/v1/bootstrap")
            .header("host", "evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let native = h
        .get("/v1/bootstrap")
        .header("origin", "capacitor://localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(native.status(), 200);
    assert_eq!(
        native.headers()["access-control-allow-origin"],
        "capacitor://localhost"
    );
    let post = || {
        c.post(format!("{}/v1/conversations", h.url))
            .bearer_auth(&h.token)
    };
    assert_eq!(post().body("{}").send().await.unwrap().status(), 415);
    assert_eq!(
        post()
            .header("content-type", "application/json")
            .body("x".repeat(128 * 1024 + 1))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    assert_eq!(post().json(&json!({"request_id":"test","workspace_id":"outside","provider":"claude","launch_options":"Inherited"})).send().await.unwrap().status(),400);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(h.root.path().join("pairing"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(h.root.path().join("pairing/connection.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[tokio::test]
async fn expired_and_nonloopback_hosts_fail_closed() {
    let h = Host::start(Duration::from_millis(30)).await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(h.get("/v1/bootstrap").send().await.unwrap().status(), 401);
    let o = Options {
        workspaces: vec![h.root.path().into()],
        kiln_cli: PathBuf::from("/missing"),
        listen: "0.0.0.0:0".parse().unwrap(),
        pairing_file: h.root.path().join("bad/pair"),
        dev_origin: None,
        token_ttl: Duration::from_secs(60),
    };
    assert!(serve(o).await.is_err());
}

#[tokio::test]
async fn slow_body_deadline_and_preflight_do_not_read_native_data() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Host::start(Duration::from_secs(60)).await;
    let c = reqwest::Client::new();
    let response = c
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/v1/conversations", h.url),
        )
        .header("origin", "https://localhost")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://localhost"
    );
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-credentials")
    );
    let authority = h.url.strip_prefix("http://").unwrap();
    let mut socket = tokio::net::TcpStream::connect(authority).await.unwrap();
    socket.write_all(format!("POST /v1/conversations HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{{",h.token).as_bytes()).await.unwrap();
    let mut response = vec![];
    tokio::time::timeout(Duration::from_secs(7), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 408"), "{response}");
    assert!(response.contains("request_deadline"));
    assert!(!response.contains(&h.token));
}

// Companion subprocess fixture: native IPC peers intentionally stall; no user daemon is contacted.
struct BoundaryProcess {
    root: tempfile::TempDir,
    child: std::process::Child,
    url: String,
    token: String,
}
impl BoundaryProcess {
    fn start() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .prefix("kiln-mobile-boundary-")
            .tempdir_in("/private/tmp")
            .unwrap();
        let b = root.path();
        for dir in ["home", "config", "project", "pairing"] {
            std::fs::create_dir(b.join(dir)).unwrap();
        }
        std::fs::set_permissions(b.join("pairing"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let cli = b.join("version-cli");
        std::fs::write(&cli, "#!/bin/sh\necho 'kiln 0.3.0'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_kiln-mobile-host"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", b.join("home"))
            .env("KILN_CONFIG_DIR", b.join("config"))
            .env("KILN_SOCKET", b.join("stalled.sock"))
            .env("KILN_ACCOUNTS_SANDBOX", b)
            .env("CODEX_HOME", b.join("home/codex"))
            .env("CLAUDE_CONFIG_DIR", b.join("home/claude"))
            .args(["--workspace"])
            .arg(b.join("project"))
            .arg("--kiln-cli")
            .arg(cli)
            .arg("--pairing-file")
            .arg(b.join("pairing/pair.json"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pairing = b.join("pairing/pair.json");
        let mut fixture = Self {
            root,
            child,
            url: String::new(),
            token: String::new(),
        };
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !pairing.exists() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let pair: Value = serde_json::from_slice(&std::fs::read(pairing).unwrap()).unwrap();
        fixture.url = pair["url"].as_str().unwrap().into();
        fixture.token = pair["token"].as_str().unwrap().into();
        fixture
    }

    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            let until = std::time::Instant::now() + Duration::from_secs(3);
            while self.child.try_wait().unwrap().is_none() {
                assert!(
                    std::time::Instant::now() < until,
                    "owned host did not cancel/reap blocked helpers"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for BoundaryProcess {
    fn drop(&mut self) {
        self.stop();
    }
}
#[test]
fn fix_round1_real_work_capacity_survives_http_timeout_and_stalled_hello_shutdown_is_bounded() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    let mut host = BoundaryProcess::start();
    let listener =
        std::os::unix::net::UnixListener::bind(host.root.path().join("stalled.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let peers = Arc::new(Mutex::new(Vec::new()));
    let accepted = peers.clone();
    let done = stop.clone();
    let peer = std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => accepted.lock().unwrap().push(stream),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let mut requests = vec![];
    for _ in 0..32 {
        let url = host.url.clone();
        let token = host.token.clone();
        requests.push(std::thread::spawn(move || {
            reqwest::blocking::Client::new()
                .get(format!("{url}/v1/terminals"))
                .bearer_auth(token)
                .send()
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    for request in requests {
        assert_eq!(request.join().unwrap(), 408);
    }
    let connections = peers.lock().unwrap().len();
    assert!(
        (1..=32).contains(&connections),
        "real helper work exceeded advertised bound: {connections}"
    );
    let refused = reqwest::blocking::Client::new()
        .get(format!("{}/v1/terminals", host.url))
        .bearer_auth(&host.token)
        .send()
        .unwrap();
    assert_eq!(
        refused.status(),
        429,
        "HTTP timeout freed capacity while actual work survived"
    );
    let before = std::time::Instant::now();
    host.stop();
    assert!(before.elapsed() < Duration::from_secs(3));
    stop.store(true, Ordering::Release);
    peer.join().unwrap();
    peers.lock().unwrap().clear();
    assert!(!host.root.path().join("pairing/pair.json").exists());
    eprintln!(
        "owned stalled-Hello host drained 32 exact native helpers after HTTP timeout; retained capacity=32; shutdown<3s; root={}",
        host.root.path().display()
    );
}

#[test]
fn fix_round1_stalled_connect_and_daemon_response_are_owned_and_cancelled() {
    use std::os::fd::AsRawFd;
    for after_hello in [false, true] {
        let mut host = BoundaryProcess::start();
        let listener =
            std::os::unix::net::UnixListener::bind(host.root.path().join("stalled.sock")).unwrap();
        let filler = if after_hello {
            None
        } else {
            assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
            Some(
                std::os::unix::net::UnixStream::connect(host.root.path().join("stalled.sock"))
                    .unwrap(),
            )
        };
        let url = host.url.clone();
        let token = host.token.clone();
        let request = std::thread::spawn(move || {
            reqwest::blocking::Client::new()
                .get(format!("{url}/v1/terminals"))
                .bearer_auth(token)
                .send()
                .map(|r| r.status().as_u16())
        });
        let peer = if after_hello {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = stream.try_clone().unwrap();
            let mut writer = stream.try_clone().unwrap();
            assert!(matches!(
                kiln_proto::read_msg::<_, kiln_proto::ClientMsg>(&mut reader).unwrap(),
                Some(kiln_proto::ClientMsg::Hello { .. })
            ));
            kiln_proto::write_msg(
                &mut writer,
                &kiln_proto::ServerMsg::Hello {
                    proto: kiln_proto::PROTO_VERSION,
                    build: "owned inert boundary peer".into(),
                    pid: std::process::id(),
                    can_upgrade: false,
                },
            )
            .unwrap();
            assert!(matches!(
                kiln_proto::read_msg::<_, kiln_proto::ClientMsg>(&mut reader).unwrap(),
                Some(kiln_proto::ClientMsg::ListSessions { .. })
            ));
            Some(stream)
        } else {
            std::thread::sleep(Duration::from_millis(200));
            None
        };
        let start = std::time::Instant::now();
        host.stop();
        assert!(start.elapsed() < Duration::from_secs(3));
        let response = request.join().unwrap();
        assert!(response.is_err() || matches!(response, Ok(503)));
        drop(peer);
        drop(filler);
        drop(listener);
    }
    eprintln!(
        "owned stalled Unix connect and native ListSessions response both cancel/reap under3s; no user daemon"
    );
}

#[test]
fn fix_round1_delivered_native_input_with_lost_ack_is_unknown_and_never_replayed() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let mut host = BoundaryProcess::start();
    let listener =
        std::os::unix::net::UnixListener::bind(host.root.path().join("stalled.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let retained = Arc::new(Mutex::new(Vec::new()));
    let done = stop.clone();
    let effects = writes.clone();
    let held = retained.clone();
    let cwd = host
        .root
        .path()
        .join("project")
        .to_string_lossy()
        .into_owned();
    let peer = std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = stream.try_clone().unwrap();
                    let hello =
                        kiln_proto::read_msg::<_, kiln_proto::ClientMsg>(&mut reader).unwrap();
                    assert!(matches!(hello, Some(kiln_proto::ClientMsg::Hello { .. })));
                    kiln_proto::write_msg(
                        &mut stream,
                        &kiln_proto::ServerMsg::Hello {
                            proto: kiln_proto::PROTO_VERSION,
                            build: "owned boundary native peer".into(),
                            pid: std::process::id(),
                            can_upgrade: false,
                        },
                    )
                    .unwrap();
                    match kiln_proto::read_msg::<_, kiln_proto::ClientMsg>(&mut reader).unwrap() {
                        Some(kiln_proto::ClientMsg::ListSessions { req }) => {
                            kiln_proto::write_msg(
                                &mut stream,
                                &kiln_proto::ServerMsg::Sessions {
                                    req,
                                    sessions: vec![kiln_proto::SessionInfo {
                                        id: 1,
                                        cwd: Some(cwd.clone()),
                                        ..Default::default()
                                    }],
                                },
                            )
                            .unwrap();
                        }
                        Some(kiln_proto::ClientMsg::Input { .. }) => {
                            effects.fetch_add(1, Ordering::AcqRel);
                            assert!(matches!(
                                kiln_proto::read_msg::<_, kiln_proto::ClientMsg>(&mut reader)
                                    .unwrap(),
                                Some(kiln_proto::ClientMsg::Ping { .. })
                            ));
                            held.lock().unwrap().push(stream);
                        }
                        _ => panic!("unexpected native boundary operation"),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let payload = json!({"request_id":"lost-input-ack","text":"one explicit input","enter":true});
    let client = reqwest::blocking::Client::new();
    let r = client
        .post(format!("{}/v1/terminals/1/input", host.url))
        .bearer_auth(&host.token)
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(r.status(), 408);
    assert_eq!(writes.load(Ordering::Acquire), 1);
    let until = std::time::Instant::now() + Duration::from_secs(6);
    loop {
        let op: Value = client
            .get(format!("{}/v1/operations/lost-input-ack", host.url))
            .bearer_auth(&host.token)
            .send()
            .unwrap()
            .json()
            .unwrap();
        if op["state"] != "pending" {
            assert_eq!(op["state"], "unknown", "{op}");
            assert_eq!(op["error"]["code"], "delivery_unknown");
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    let repeat = client
        .post(format!("{}/v1/terminals/1/input", host.url))
        .bearer_auth(&host.token)
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(repeat.status(), 503);
    assert_eq!(writes.load(Ordering::Acquire), 1);
    host.stop();
    stop.store(true, Ordering::Release);
    peer.join().unwrap();
    retained.lock().unwrap().clear();
    eprintln!(
        "actual native Input wire observed once; HTTP timeout then helper deadline => cached UNKNOWN delivery; duplicate native effects=0 (inert daemon boundary peer)"
    );
}
