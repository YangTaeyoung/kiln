//! Actual published Kiln CLI + private daemon + native agent worker; inference is an inert provider fixture.
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg};
use reqwest::blocking::Client as Http;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct StartingChild(Option<Child>);
impl StartingChild {
    fn id(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }
    fn take(mut self) -> Child {
        self.0.take().unwrap()
    }
}
impl Drop for StartingChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
    }
}
struct Fixture {
    root: tempfile::TempDir,
    host: Child,
    daemon: Child,
    url: String,
    token: String,
    socket: String,
    cli: PathBuf,
}
impl Fixture {
    fn start() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .prefix("kiln-mobile-native-")
            .tempdir_in("/private/tmp")
            .unwrap();
        let b = root.path();
        for d in [
            "home", "config", "codex", "claude", "project", "bin", "pairing",
        ] {
            fs::create_dir(b.join(d)).unwrap();
        }
        fs::set_permissions(b.join("pairing"), fs::Permissions::from_mode(0o700)).unwrap();
        let cli = std::env::var_os("KILN_MOBILE_TEST_CLI")
            .map(PathBuf::from)
            .expect("set KILN_MOBILE_TEST_CLI to verified owned 0.3 CLI");
        let python = kiln_common::paths::find_executable("python3", &[]).unwrap();
        let provider=include_str!("../../../crates/kiln/tests/fixtures/agent_gui.py").replace("\"content\": \"fixture result\"", "\"content\": (\"x\" * 70000 if turn == 2 else \"fixture result\")").replace("output = \"written\" if allow else \"denied\"", "output = (\"<script>inert-output</script>\" + \"x\" * 140000) if allow else \"denied\"");
        let provider=provider.replace("import json\n", "import json\nimport threading\n").replace("        elif subtype == \"interrupt\":",r#"        elif subtype == "set_permission_mode":
            identity = value["request_id"]
            selected = value["request"]["mode"]
            def delayed_mode(identity=identity, selected=selected):
                for i in range(1600):
                    emit({"type":"assistant","message":{"id":"backlog-"+str(i),"content":[{"type":"text","text":"native backlog "+str(i)}]}})
                Path("configuration-pending").touch()
                while not Path("release-configuration").exists():
                    time.sleep(0.01)
                emit({"type":"control_response","response":{"subtype":"success","request_id":identity,"response":{"mode":selected}}})
            threading.Thread(target=delayed_mode, daemon=True).start()
        elif subtype == "interrupt":"#);
        fs::write(b.join("provider.py"), provider).unwrap();
        let bin = b.join("bin/claude");
        fs::write(&bin,format!("#!/bin/sh\nexport FIXTURE_PROVIDER=claude\nexport FIXTURE_MODE=conversation\nexec '{}' '{}' \"$@\"\n",python.display(),b.join("provider.py").display())).unwrap();
        fs::set_permissions(bin, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = b.join("d.sock").to_string_lossy().into_owned();
        let envcmd = |exe: &Path| {
            let mut c = Command::new(exe);
            c.env_clear()
                .env(
                    "PATH",
                    format!(
                        "{}:/usr/bin:/bin:/opt/homebrew/bin",
                        b.join("bin").display()
                    ),
                )
                .env("HOME", b.join("home"))
                .env("USERPROFILE", b.join("home"))
                .env("KILN_CONFIG_DIR", b.join("config"))
                .env("CODEX_HOME", b.join("codex"))
                .env("CLAUDE_CONFIG_DIR", b.join("claude"))
                .env("KILN_SOCKET", &socket)
                .env("KILN_ACCOUNTS_SANDBOX", b)
                .env("KILN_EXE", &cli)
                .env("KILN_NO_AUTO_UPGRADE", "1")
                .env("KILN_DB_NO_KEYCHAIN", "1")
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            c
        };
        let daemon = StartingChild(Some(
            envcmd(&cli)
                .args(["daemon", "--socket", &socket])
                .spawn()
                .unwrap(),
        ));
        let until = Instant::now() + Duration::from_secs(8);
        while Client::connect(&socket, None).is_err() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        let host = StartingChild(Some(
            envcmd(Path::new(env!("CARGO_BIN_EXE_kiln-mobile-host")))
                .arg("--workspace")
                .arg(b.join("project"))
                .arg("--kiln-cli")
                .arg(&cli)
                .args(["--listen", "127.0.0.1:0", "--pairing-file"])
                .arg(b.join("pairing/connection.json"))
                .spawn()
                .unwrap(),
        ));
        let until = Instant::now() + Duration::from_secs(8);
        while !b.join("pairing/connection.json").exists() {
            assert!(Instant::now() < until, "pairing");
            std::thread::sleep(Duration::from_millis(20));
        }
        let pair: Value =
            serde_json::from_slice(&fs::read(b.join("pairing/connection.json")).unwrap()).unwrap();
        eprintln!(
            "owned fixture root={} host_pid={} daemon_pid={} socket={}",
            b.display(),
            host.id(),
            daemon.id(),
            socket
        );
        Self {
            root,
            host: host.take(),
            daemon: daemon.take(),
            url: pair["url"].as_str().unwrap().into(),
            token: pair["token"].as_str().unwrap().into(),
            socket,
            cli,
        }
    }
    fn restart_host(&mut self) {
        unsafe {
            libc::kill(self.host.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.host.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(self.host.wait().unwrap().success());
        let b = self.root.path();
        self.host = Command::new(env!("CARGO_BIN_EXE_kiln-mobile-host"))
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/opt/homebrew/bin",
                    b.join("bin").display()
                ),
            )
            .env("HOME", b.join("home"))
            .env("USERPROFILE", b.join("home"))
            .env("KILN_CONFIG_DIR", b.join("config"))
            .env("KILN_SOCKET", &self.socket)
            .env("KILN_ACCOUNTS_SANDBOX", b)
            .env("KILN_EXE", &self.cli)
            .env("KILN_NO_AUTO_UPGRADE", "1")
            .env("KILN_DB_NO_KEYCHAIN", "1")
            .env("CODEX_HOME", b.join("codex"))
            .env("CLAUDE_CONFIG_DIR", b.join("claude"))
            .arg("--workspace")
            .arg(b.join("project"))
            .arg("--kiln-cli")
            .arg(&self.cli)
            .arg("--pairing-file")
            .arg(b.join("pairing/connection.json"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !b.join("pairing/connection.json").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let p: Value =
            serde_json::from_slice(&fs::read(b.join("pairing/connection.json")).unwrap()).unwrap();
        self.url = p["url"].as_str().unwrap().into();
        self.token = p["token"].as_str().unwrap().into();
    }
    fn get(&self, p: &str) -> Value {
        let r = Http::new()
            .get(format!("{}{p}", self.url))
            .bearer_auth(&self.token)
            .send()
            .unwrap();
        assert!(r.status().is_success(), "{}", r.text().unwrap());
        r.json().unwrap()
    }
    fn post(&self, p: &str, v: &Value) -> (u16, Value) {
        let r = Http::new()
            .post(format!("{}{p}", self.url))
            .bearer_auth(&self.token)
            .json(v)
            .send()
            .unwrap();
        (r.status().as_u16(), r.json().unwrap())
    }
    fn wait(&self, p: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let v = self.get(p);
            if pred(&v) {
                return v;
            }
            assert!(Instant::now() < until, "timeout snapshot {v}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

// Daemon shutdown intentionally preserves PTY owners. Fixture cleanup owns these exact endpoints.
fn cleanup_owned_pty_hosts(root: &Path, cli: &Path) {
    let inventory = Command::new("/bin/ps")
        .args(["-axo", "pid=,command="])
        .output()
        .unwrap();
    let expected = format!("{} pty-host --endpoint {}/", cli.display(), root.display());
    for line in String::from_utf8(inventory.stdout).unwrap().lines() {
        let line = line.trim();
        let Some((pid, argv)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !argv.trim().starts_with(&expected) {
            continue;
        }
        let pid: i32 = pid.parse().unwrap();
        let identity = || {
            Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "lstart=,command="])
                .output()
                .unwrap()
                .stdout
        };
        let before = identity();
        let image = Command::new("/usr/sbin/lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-Fn"])
            .output()
            .unwrap();
        if before.is_empty() {
            continue;
        }
        assert!(
            String::from_utf8_lossy(&image.stdout)
                .lines()
                .any(|line| line == format!("n{}", cli.display())),
            "owned PTY image identity changed"
        );
        assert_eq!(identity(), before, "owned PTY PID/start time changed");
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !identity().is_empty() {
            assert!(
                Instant::now() < deadline,
                "owned PTY host {pid} did not terminate"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "fixture PTY listener cleanup complete pid={pid} root={} (exact argv/start/image checked)",
            root.display()
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.host.try_wait().unwrap().is_none() {
            unsafe {
                libc::kill(self.host.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.host.try_wait().unwrap().is_none() {
                assert!(
                    Instant::now() < deadline,
                    "owned host did not safely drain its helpers"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let _ = self.host.wait();
        if let Ok(c) = Client::connect(&self.socket, None) {
            if let Ok(ServerMsg::Sessions { sessions, .. }) = c.request(
                |req| ClientMsg::ListSessions { req },
                Duration::from_secs(2),
            ) {
                for s in sessions {
                    c.send(ClientMsg::Kill { session: s.id });
                }
            }
            std::thread::sleep(Duration::from_millis(300));
            c.send(ClientMsg::Shutdown);
        }
        let until = Instant::now() + Duration::from_secs(4);
        loop {
            if self.daemon.try_wait().ok().flatten().is_some() {
                break;
            }
            if Instant::now() > until {
                let _ = self.daemon.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.daemon.wait();
        cleanup_owned_pty_hosts(self.root.path(), &self.cli);
        let root = self.root.path();
        fs::remove_dir_all(root).unwrap();
        eprintln!(
            "fixture cleanup complete root={} host_pid={} daemon_pid={} (all waited; native workers terminated only by fixture)",
            root.display(),
            self.host.id(),
            self.daemon.id()
        );
    }
}
#[test]
fn actual_http_native_commands_are_once_generation_bound_and_survive_host_shutdown() {
    let mut f = Fixture::start();
    let b = f.get("/v1/bootstrap");
    let wid = b["workspaces"][0]["id"].as_str().unwrap();
    // Slow actual native account settings read: a private FIFO delays preparation beyond the HTTP deadline.
    let fifo = f.root.path().join("config/accounts.json");
    let fifo_c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let launch = json!({"request_id":"launch-one","workspace_id":wid,"provider":"claude","launch_options":"Inherited"});
    let a = f.post("/v1/conversations", &launch);
    assert_eq!(a.0, 202, "{:?}", a);
    let dup = f.post("/v1/conversations", &launch);
    assert_eq!(a, dup);
    let mut changed = launch.clone();
    changed["provider"] = json!("codex");
    assert_eq!(f.post("/v1/conversations", &changed).0, 409);
    changed["workspace_id"] = json!("outside");
    assert_eq!(f.post("/v1/conversations", &changed).0, 409);
    assert_eq!(f.get("/v1/operations/launch-one")["state"], "pending");
    std::thread::sleep(Duration::from_millis(5200));
    assert_eq!(f.post("/v1/conversations", &launch), a);
    assert_eq!(f.get("/v1/operations/launch-one")["state"], "pending");
    {
        use std::io::Write;
        let mut writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        writer.write_all(b"{}").unwrap();
    }
    fs::remove_file(&fifo).unwrap();
    fs::write(&fifo, b"{}").unwrap();
    let op = f.wait("/v1/operations/launch-one", |v| v["state"] != "pending");
    assert_eq!(op["state"], "succeeded", "{op}");
    let id = op["result"]["conversation_id"].as_str().unwrap();
    let path = format!("/v1/conversations/{id}");
    let ready = f.wait(&path, |v| {
        v["lifecycle"] == "Ready" && v["replay_ready"] == true
    });
    let generation = ready["generation"].as_str().unwrap();
    // Immutable upload IDs never expose/accept client filesystem paths.
    let upload_path = format!("{path}/attachments");
    let upload = json!({"request_id":"image-one","filename":"pixel.png","mime":"image/png","bytes_base64":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jP1EAAAAASUVORK5CYII="});
    let image = f.post(&upload_path, &upload);
    assert_eq!(image.0, 202, "{image:?}");
    assert_eq!(f.post(&upload_path, &upload), image);
    assert!(image.1["attachment"].get("path").is_none());
    let stored = f
        .root
        .path()
        .join("config/agent-gui")
        .join(id)
        .join("attachments");
    assert_eq!(fs::read_dir(&stored).unwrap().count(), 1);
    use std::os::unix::fs::PermissionsExt;
    let image_file = fs::read_dir(&stored)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::metadata(image_file).unwrap().permissions().mode() & 0o777,
        0o400
    );
    let mut traversal = upload.clone();
    traversal["request_id"] = json!("bad-image");
    traversal["filename"] = json!("../auth.json");
    assert_eq!(f.post(&upload_path, &traversal).0, 400);
    let cmdpath = format!("{path}/commands");
    let submit = json!({"request_id":"submit-one","generation":generation,"command":{"Submit":{"text":"write once","attachments":[],"model":null}}});
    let mut stale = submit.clone();
    stale["request_id"] = json!("stale");
    stale["generation"] = json!("wrong");
    assert_eq!(f.post(&cmdpath, &stale).0, 409);
    let sent = f.post(&cmdpath, &submit);
    assert_eq!(sent.0, 202, "{sent:?}");
    assert_eq!(sent, f.post(&cmdpath, &submit));
    let waiting = f.wait(&path, |v| {
        v["requests"].as_array().is_some_and(|a| !a.is_empty())
    });
    assert_eq!(waiting["requests"][0]["respondable"], true, "{waiting}");
    let rid = waiting["requests"][0]["id"].as_str().unwrap();
    let approve = json!({"request_id":"approve-one","generation":generation,"command":{"Respond":{"request_id":rid,"decision":"Allow","answers":{}}}});
    assert_eq!(f.post(&cmdpath, &approve).0, 202);
    f.wait(&path, |v| {
        v["lifecycle"] == "Ready" && v["requests"].as_array().is_some_and(Vec::is_empty)
    });
    assert_eq!(
        fs::read(f.root.path().join("project/operation-1.txt")).unwrap(),
        b"fixture result\n"
    );
    let wire = fs::read_to_string(f.root.path().join("project/wire.jsonl")).unwrap();
    assert_eq!(
        wire.lines()
            .filter(|l| serde_json::from_str::<Value>(l).unwrap()["type"] == "user")
            .count(),
        1
    );
    assert!(!f.root.path().join("project/operation-2.txt").exists());
    let snapshot = f.get(&path);
    assert!(
        !snapshot
            .to_string()
            .contains(fs::canonicalize(f.root.path()).unwrap().to_str().unwrap()),
        "private path exposed"
    );
    assert!(snapshot.to_string().len() < 2097152);
    assert_eq!(snapshot["coverage"]["truncated"], true);
    let tools = snapshot["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.get("Tool"))
        .collect::<Vec<_>>();
    assert!(tools.iter().any(|t| {
        t["output"]
            .as_str()
            .is_some_and(|s| s.starts_with("<script>") && s.len() <= 120000)
    }));
    // All native details must fit before any Allow; the second real request carries >64KiB input.
    let second = json!({"request_id":"submit-two","generation":generation,"command":{"Submit":{"text":"oversized approval","attachments":[],"model":null}}});
    assert_eq!(f.post(&cmdpath, &second).0, 202);
    let incomplete = f.wait(&path, |v| {
        v["requests"].as_array().is_some_and(|a| !a.is_empty())
    });
    assert_eq!(incomplete["requests"][0]["respondable"], false);
    let rid = incomplete["requests"][0]["id"].as_str().unwrap();
    let hidden = json!({"request_id":"hidden-allow","generation":generation,"command":{"Respond":{"request_id":rid,"decision":"Allow","answers":{}}}});
    let refused = f.post(&cmdpath, &hidden);
    assert_eq!(refused.0, 409);
    assert_eq!(refused.1["error"]["code"], "incomplete_approval");
    assert_eq!(f.post(&cmdpath, &hidden), refused);
    assert!(!f.root.path().join("project/operation-2.txt").exists());
    // Metadata projection omits raw environment/program/auth and refuses replaced files/directories.
    let records = f.root.path().join("config/agent-gui");
    let fixture = records.join("metadata-fixture");
    fs::create_dir(&fixture).unwrap();
    let config = json!({"descriptor":{"id":"metadata-fixture","tool":"claude","cwd":fs::canonicalize(f.root.path().join("project")).unwrap(),"account_label":"Default"},"program":"sensitive-cli","environment":[["SECRET","credential-canary"]]});
    fs::write(
        fixture.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let list = f.get("/v1/conversations");
    assert!(!list.to_string().contains("credential-canary"));
    assert!(
        list["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["descriptor"]["id"] == "metadata-fixture")
    );
    fs::remove_file(fixture.join("config.json")).unwrap();
    std::os::unix::fs::symlink(&fifo, fixture.join("config.json")).unwrap();
    assert_eq!(
        Http::new()
            .get(format!("{}/v1/conversations/metadata-fixture", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap()
            .status(),
        404
    );
    fs::remove_file(fixture.join("config.json")).unwrap();
    let mut outside = config.clone();
    outside["descriptor"]["cwd"] = json!(fs::canonicalize(f.root.path().join("home")).unwrap());
    fs::write(
        fixture.join("config.json"),
        serde_json::to_vec(&outside).unwrap(),
    )
    .unwrap();
    assert_eq!(
        Http::new()
            .get(format!("{}/v1/conversations/metadata-fixture", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap()
            .status(),
        404
    );
    fs::remove_dir_all(&fixture).unwrap();
    std::os::unix::fs::symlink(f.root.path().join("home"), &fixture).unwrap();
    assert!(
        !f.get("/v1/conversations")["conversations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["descriptor"]["id"] == "metadata-fixture")
    );
    // A previously enabled cwd replaced by a symlink must not dispatch using cached native state.
    let project = f.root.path().join("project");
    let retained = f.root.path().join("project-retained");
    fs::rename(&project, &retained).unwrap();
    std::os::unix::fs::symlink(f.root.path().join("home"), &project).unwrap();
    let escaped =
        json!({"request_id":"escaped-workspace","generation":generation,"command":"Interrupt"});
    assert_eq!(f.post(&cmdpath, &escaped).0, 404);
    assert_eq!(
        Http::new()
            .get(format!("{}{path}", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap()
            .status(),
        404
    );
    fs::remove_file(&project).unwrap();
    fs::rename(&retained, &project).unwrap();
    let stop = json!({"request_id":"stop-one","generation":generation,"command":"Interrupt"});
    assert_eq!(f.post(&cmdpath, &stop).0, 202);
    // Bound active native readers to 16, including the actual worker already opened.
    for i in 0..16 {
        let fake = format!("reader-{i}");
        let dir = records.join(&fake);
        fs::create_dir(&dir).unwrap();
        let mut c = config.clone();
        c["descriptor"]["id"] = json!(fake);
        fs::write(dir.join("config.json"), serde_json::to_vec(&c).unwrap()).unwrap();
        let response = Http::new()
            .get(format!("{}/v1/conversations/{fake}", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap();
        assert_eq!(response.status().as_u16(), if i < 15 { 200 } else { 429 });
    }
    let daemon = Client::connect(&f.socket, None).unwrap();
    let sessions = match daemon
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(3),
        )
        .unwrap()
    {
        ServerMsg::Sessions { sessions, .. } => sessions,
        _ => panic!(),
    };
    assert_eq!(sessions.len(), 1);
    #[cfg(unix)]
    unsafe {
        libc::kill(f.host.id() as i32, libc::SIGTERM);
    }
    assert!(f.host.wait().unwrap().success());
    let after = match daemon
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(3),
        )
        .unwrap()
    {
        ServerMsg::Sessions { sessions, .. } => sessions,
        _ => panic!(),
    };
    assert_eq!(after[0].pid, sessions[0].pid);
    assert_eq!(after[0].exited, None);
    // Fixture alone owns daemon/workers; host shutdown must leave them reusable.
    daemon.send(ClientMsg::Kill {
        session: after[0].id,
    });
    eprintln!(
        "actual CLI/native worker roundtrip PASS; launch=1 primary prompt=1 approved file effect=1 stale dispatches=0 duplicate dispatches=0 oversized approval blocked=1; host shutdown preserves worker PID {}",
        after[0].pid
    );
}

#[test]
fn native_preparation_is_tracked_once_and_bounded_to_four() {
    let f = Fixture::start();
    let fifo = f.root.path().join("config/accounts.json");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let mut launches = vec![];
    for i in 0..5 {
        let id = format!("bounded-{i}");
        let v = json!({"request_id":id,"workspace_id":"workspace-1","provider":"claude","launch_options":"Inherited"});
        let response = f.post("/v1/conversations", &v);
        assert_eq!(response.0, 202);
        assert_eq!(f.post("/v1/conversations", &v), response);
        launches.push(id);
    }
    let refused = f.get("/v1/operations/bounded-4");
    assert_eq!(refused["state"], "failed");
    assert_eq!(refused["error"]["code"], "preparation_limit");
    for id in &launches[..4] {
        assert_eq!(f.get(&format!("/v1/operations/{id}"))["state"], "pending");
    }
    let daemon = Client::connect(&f.socket, None).unwrap();
    match daemon
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(3),
        )
        .unwrap()
    {
        ServerMsg::Sessions { sessions, .. } => assert!(sessions.is_empty()),
        _ => panic!(),
    }
    eprintln!(
        "actual native preparation bound PASS: 4 tracked blocked preparations; fifth refused; duplicate creates=0; fixture alone terminates its blocked host/daemon"
    );
}

#[test]
fn existing_terminal_input_is_explicit_once_and_allowlisted() {
    let f = Fixture::start();
    let daemon = Client::connect(&f.socket, None).unwrap();
    let create = |cwd: PathBuf| {
        match daemon.request(|req|ClientMsg::Create{req,spec:kiln_proto::SpawnSpec{program:Some("/bin/sh".into()),args:vec!["-c".into(),"while IFS= read -r line; do printf '%s\\n' \"$line\" >> terminal-effect.txt; done".into()],cwd:Some(cwd.to_string_lossy().into_owned()),cols:80,rows:24,..Default::default()}},Duration::from_secs(3)).unwrap(){ServerMsg::Created{session,..}=>session,_=>panic!()}
    };
    let allowed = create(f.root.path().join("project"));
    let outside = create(f.root.path().join("home"));
    let summaries = f.get("/v1/terminals");
    assert!(
        summaries["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == allowed.to_string())
    );
    assert!(
        !summaries["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == outside.to_string())
    );
    let input = json!({"request_id":"terminal-one","text":"explicit terminal line","enter":true});
    let path = format!("/v1/terminals/{allowed}/input");
    let first = f.post(&path, &input);
    assert_eq!(first.0, 202);
    assert_eq!(f.post(&path, &input), first);
    assert_eq!(
        f.post(
            &format!("/v1/terminals/{outside}/input"),
            &json!({"request_id":"outside-terminal","text":"must not dispatch","enter":true})
        )
        .0,
        400
    );
    let effect = f.root.path().join("project/terminal-effect.txt");
    let until = Instant::now() + Duration::from_secs(4);
    while !effect.exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(fs::read(effect).unwrap(), b"explicit terminal line\n");
    assert!(!f.root.path().join("home/terminal-effect.txt").exists());
    let text = f.get(&format!("/v1/terminals/{allowed}"));
    assert!(
        text["text"]
            .as_str()
            .unwrap()
            .contains("explicit terminal line")
    );
    assert_eq!(text["exited"], Value::Null);
    eprintln!(
        "actual existing terminal PASS: one explicit input effect; duplicate/outside input effects=0; no host Create route"
    );
}

fn stored_history(f: &Fixture, id: &str, title: &str) -> PathBuf {
    let dir = f.root.path().join("config/agent-gui").join(id);
    fs::create_dir_all(&dir).unwrap();
    let config = json!({"descriptor":{"id":id,"tool":"claude","cwd":f.root.path().join("project"),"account_label":"Default"},"program":"inert","environment":[]});
    fs::write(
        dir.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let events = [
        json!({"sequence":1,"generation":"fixture-generation","event":"Generation"}),
        json!({"sequence":2,"generation":"fixture-generation","event":{"Lifecycle":"Ready"}}),
        json!({"sequence":3,"generation":"fixture-generation","event":{"Message":{"message":{"id":"user-1","role":"User","text":title,"attachments":[]},"append":false}}}),
    ];
    fs::write(
        dir.join("events.jsonl"),
        events.iter().map(|e| format!("{e}\n")).collect::<String>(),
    )
    .unwrap();
    dir
}
#[test]
fn fix_round1_external_history_and_cached_storage_replacement_never_expose_or_dispatch() {
    let f = Fixture::start();
    let outside = f.root.path().join("home/outside.jsonl");
    let dir = stored_history(&f, "journal-link", "REVIEW_OUTSIDE_HISTORY_CANARY");
    fs::rename(dir.join("events.jsonl"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("events.jsonl")).unwrap();
    let path = "/v1/conversations/journal-link";
    let response = Http::new()
        .get(format!("{}{path}", f.url))
        .bearer_auth(&f.token)
        .send()
        .unwrap();
    let mut body = response.text().unwrap();
    for _ in 0..40 {
        if body.contains("REVIEW_OUTSIDE_HISTORY_CANARY") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        body = Http::new()
            .get(format!("{}{path}", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap()
            .text()
            .unwrap();
    }
    assert!(
        !body.contains("REVIEW_OUTSIDE_HISTORY_CANARY"),
        "outside journal exposed: {body}"
    );
    let dir = stored_history(&f, "cached-journal", "trusted initial title");
    let path = "/v1/conversations/cached-journal";
    f.wait(path, |v| v["generation"] == "fixture-generation");
    let prefix = fs::read_to_string(dir.join("events.jsonl")).unwrap();
    let canary = json!({"sequence":4,"generation":"fixture-generation","event":{"Message":{"message":{"id":"outside-4","role":"Assistant","text":"REVIEW_OUTSIDE_HISTORY_CANARY","attachments":[]},"append":false}}});
    fs::write(&outside, format!("{prefix}{canary}\n")).unwrap();
    fs::rename(dir.join("events.jsonl"), dir.join("events-original.jsonl")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("events.jsonl")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let response = Http::new()
        .get(format!("{}{path}", f.url))
        .bearer_auth(&f.token)
        .send()
        .unwrap();
    let body = response.text().unwrap();
    assert!(!body.contains("REVIEW_OUTSIDE_HISTORY_CANARY"));
    let cmd = json!({"request_id":"cached-replacement-command","generation":"fixture-generation","command":"Interrupt"});
    let r = f.post(&format!("{path}/commands"), &cmd);
    assert_ne!(
        r.0, 202,
        "cached replaced storage accepted a command: {r:?}"
    );
    assert!(!dir.join("commands").exists());
}
#[test]
fn fix_round1_metadata_fifo_is_rejected_and_shutdown_does_not_wait_for_a_writer() {
    let mut f = Fixture::start();
    let dir = f.root.path().join("config/agent-gui/fifo-config");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    let p = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    let r = Http::new()
        .get(format!("{}/v1/conversations/fifo-config", f.url))
        .bearer_auth(&f.token)
        .send()
        .unwrap();
    assert_eq!(r.status(), 404);
    assert!(start.elapsed() < Duration::from_secs(1));
    unsafe {
        libc::kill(f.host.id() as i32, libc::SIGTERM);
    }
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = f.host.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < until, "shutdown waits for FIFO writer");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn fix_round1_slow_preparation_rechecks_original_workspace_identity_before_create() {
    let f = Fixture::start();
    let fifo = f.root.path().join("config/accounts.json");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let launch = json!({"request_id":"workspace-swap","workspace_id":"workspace-1","provider":"claude","launch_options":"Inherited"});
    assert_eq!(f.post("/v1/conversations", &launch).0, 202);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(f.get("/v1/operations/workspace-swap")["state"], "pending");
    let original = f.root.path().join("project");
    let retained = f.root.path().join("original-project");
    fs::rename(&original, &retained).unwrap();
    std::os::unix::fs::symlink(f.root.path().join("home"), &original).unwrap();
    {
        use std::io::Write;
        let mut writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        writer.write_all(b"{}").unwrap();
    }
    fs::remove_file(&fifo).unwrap();
    fs::write(&fifo, b"{}").unwrap();
    let op = f.wait("/v1/operations/workspace-swap", |v| v["state"] != "pending");
    assert_eq!(op["state"], "failed", "{op}");
    let daemon = Client::connect(&f.socket, None).unwrap();
    match daemon
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(2),
        )
        .unwrap()
    {
        ServerMsg::Sessions { sessions, .. } => assert!(
            sessions.is_empty(),
            "Create dispatched into swapped workspace"
        ),
        _ => panic!(),
    }
    assert!(!f.root.path().join("home/wire.jsonl").exists());
    assert!(!retained.join("wire.jsonl").exists());
}
#[test]
fn fix_round1_list_has_real_bounded_title_lifecycle_and_does_not_allocate_readers() {
    let f = Fixture::start();
    for i in 0..20 {
        stored_history(&f, &format!("list-{i}"), &format!("Actual stored task {i}"));
    }
    let list = f.get("/v1/conversations");
    let rows = list["conversations"].as_array().unwrap();
    assert_eq!(rows.len(), 20);
    for row in rows {
        assert!(
            row["title"]
                .as_str()
                .is_some_and(|s| s.starts_with("Actual stored task")),
            "missing safe native title: {row}"
        );
        assert_eq!(row["lifecycle"], "Ready");
        assert_eq!(row["summary"]["partial"], false);
    }
    for i in 0..16 {
        let r = Http::new()
            .get(format!("{}/v1/conversations/list-{i}", f.url))
            .bearer_auth(&f.token)
            .send()
            .unwrap();
        assert_eq!(r.status(), 200, "list consumed reader capacity");
    }
}

#[test]
fn fix_round1_shutdown_cancels_blocked_native_preparation_without_create() {
    let mut f = Fixture::start();
    let fifo = f.root.path().join("config/accounts.json");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let create = json!({"request_id":"blocked-shutdown","workspace_id":"workspace-1","provider":"claude","launch_options":"Inherited"});
    assert_eq!(f.post("/v1/conversations", &create).0, 202);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(f.get("/v1/operations/blocked-shutdown")["state"], "pending");
    let start = Instant::now();
    unsafe {
        libc::kill(f.host.id() as i32, libc::SIGTERM);
    }
    loop {
        if let Some(status) = f.host.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "host waited for an unprovided native accounts FIFO writer"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let daemon = Client::connect(&f.socket, None).unwrap();
    match daemon
        .request(
            |req| ClientMsg::ListSessions { req },
            Duration::from_secs(2),
        )
        .unwrap()
    {
        ServerMsg::Sessions { sessions, .. } => assert!(sessions.is_empty()),
        _ => panic!(),
    }
    eprintln!(
        "blocked native preparation helper cancelled/reaped on shutdown <3s; daemon retained; Create=0; no FIFO writer"
    );
}

#[test]
fn fix_round1_actual_native_configuration_settles_after_reconnect_backlog_and_interrupt() {
    let mut f = Fixture::start();
    let create = json!({"request_id":"configuration-launch","workspace_id":"workspace-1","provider":"claude","launch_options":"Inherited"});
    assert_eq!(f.post("/v1/conversations", &create).0, 202);
    let created = f.wait("/v1/operations/configuration-launch", |v| {
        v["state"] != "pending"
    });
    assert_eq!(created["state"], "succeeded", "{created}");
    let id = created["result"]["conversation_id"].as_str().unwrap();
    let path = format!("/v1/conversations/{id}");
    let ready = f.wait(&path, |v| {
        v["replay_ready"] == true && v["lifecycle"] == "Ready"
    });
    let generation = ready["generation"].as_str().unwrap().to_string();
    let commands = format!("{path}/commands");
    let configure = json!({"request_id":"configuration-plan","generation":generation,"command":{"Configure":{"control":"permissionMode","value":{"Text":"plan"}}}});
    let sent = f.post(&commands, &configure);
    assert_eq!(sent.0, 202, "{sent:?}");
    f.wait(&path, |v| {
        v["controls"]["outcomes"].as_object().is_some_and(|m| {
            m.values()
                .any(|o| o["state"] == "Pending" && o["settled"] == false)
        })
    });
    let until = Instant::now() + Duration::from_secs(8);
    while !f.root.path().join("project/configuration-pending").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    f.restart_host();
    let first = f.get(&path);
    let caught_up = f.wait(&path, |v| {
        v["generation"] == generation
            && v["controls"]["outcomes"].as_object().is_some_and(|m| {
                m.values()
                    .any(|o| o["state"] == "Pending" && o["settled"] == false)
            })
    });
    let blocked = json!({"request_id":"settling-send","generation":generation,"command":{"Submit":{"text":"must remain unsent","attachments":[],"model":null}}});
    assert_eq!(f.post(&commands, &blocked).0, 409);
    assert_eq!(
        f.post(
            &commands,
            &json!({"request_id":"settling-stop","generation":generation,"command":"Interrupt"})
        )
        .0,
        202
    );
    f.wait(&path, |v| {
        v["controls"]["outcomes"].as_object().is_some_and(|m| {
            m.values()
                .any(|o| o["state"] == "Cancelled" && o["settled"] == false)
        })
    });
    assert_eq!(f.post(&commands,&json!({"request_id":"stopped-still-settling-send","generation":generation,"command":{"Submit":{"text":"still must not dispatch","attachments":[],"model":null}}})).0,409);
    fs::write(
        f.root.path().join("project/release-configuration"),
        b"release",
    )
    .unwrap();
    let settled = f.wait(&path, |v| {
        v["replay_ready"] == true
            && v["controls"]["outcomes"].as_object().is_some_and(|m| {
                m.values().any(|o| {
                    o["state"] == "Cancelled"
                        && o["settled"] == true
                        && o["effective"] == json!({"Text":"plan"})
                })
            })
    });
    assert_eq!(settled["selected_mode"], "plan");
    let applied=f.post(&commands,&json!({"request_id":"configuration-default","generation":generation,"command":{"Configure":{"control":"permissionMode","value":{"Text":"default"}}}}));
    assert_eq!(applied.0, 202, "{applied:?}");
    f.wait(&path, |v| {
        v["controls"]["outcomes"].as_object().is_some_and(|m| {
            m.values().any(|o| {
                o["state"] == "Applied"
                    && o["settled"] == true
                    && o["effective"] == json!({"Text":"default"})
            })
        })
    });
    let wire = fs::read_to_string(f.root.path().join("project/wire.jsonl")).unwrap();
    assert_eq!(
        wire.lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["type"] == "user")
            .count(),
        0,
        "incompatible Send reached provider"
    );
    eprintln!(
        "actual native configuration PASS: Pending -> reconnect/backlog -> Interrupt/Cancelled-unsettled -> Cancelled-settled effective plan -> Applied default; Submit effects=0; first_replay_ready={} retained_backlog_entries={}",
        first["replay_ready"], caught_up["coverage"]["entries_retained"]
    );
}

#[test]
fn fix_round1_partial_fixture_initialization_reaps_its_started_child() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let pid = AtomicU32::new(0);
    let initialize = || -> Result<(), &str> {
        let child = StartingChild(Some(
            Command::new("/bin/sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        ));
        pid.store(child.id(), Ordering::Release);
        Err("owned fixture initialization failed after child start")
    };
    assert!(initialize().is_err());
    let observed = Command::new("/bin/ps")
        .args(["-p", &pid.load(Ordering::Acquire).to_string(), "-o", "pid="])
        .output()
        .unwrap();
    assert!(
        observed.stdout.is_empty(),
        "partial fixture constructor leaked its exact started child"
    );
}
