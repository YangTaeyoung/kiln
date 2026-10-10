//! Real application routing against an owned, synthetic S3 endpoint.
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln::app::{Action, KilnApp};
use kiln_remote::{ConnectionProfile, RemoteEndpoint, Secrets, ui::RemoteManager};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Dropped(std::path::PathBuf);
impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

struct Storage {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    file: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Storage {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let file = Arc::new(Mutex::new(b"{\"theme\":\"dark\"}\n".to_vec()));
        let stop = Arc::new(AtomicBool::new(false));
        let hold = Arc::new(AtomicBool::new(false));
        let holding = hold.clone();
        let (log, data, stopping) = (requests.clone(), file.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            let mut objects = std::collections::HashMap::from([
                (
                    "/fixture-assets/docs/config.json".to_owned(),
                    b"{\"theme\":\"dark\"}\n".to_vec(),
                ),
                (
                    "/fixture-assets/readme.md".to_owned(),
                    b"# Remote notes\n".to_vec(),
                ),
            ]);
            while !stopping.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") && header.len() < 65536 {
                    if stream.read_exact(&mut byte).is_err() {
                        break;
                    }
                    header.push(byte[0]);
                }
                if !header.ends_with(b"\r\n\r\n") {
                    continue;
                }
                let header = String::from_utf8(header).unwrap();
                let request = header.lines().next().unwrap().to_owned();
                log.lock().unwrap().push(request.clone());
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let method = request.split_whitespace().next().unwrap();
                let target = request.split_whitespace().nth(1).unwrap();
                let path = target.split('?').next().unwrap();
                if method == "POST" && target.contains("uploads") && target.contains("upload.txt") {
                    while holding.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                let mut status = "200 OK";
                let response = if request.contains("list-type=2") {
                    let entries = if request.contains("prefix=docs") {
                        "<Contents><Key>docs/config.json</Key><Size>17</Size><LastModified>2026-10-10T00:00:00Z</LastModified></Contents>".to_owned()
                    } else {
                        let mut entries = "<CommonPrefixes><Prefix>docs/</Prefix></CommonPrefixes><Contents><Key>readme.md</Key><Size>14</Size><LastModified>2026-10-10T00:00:00Z</LastModified></Contents>".to_owned();
                        if objects.contains_key("/fixture-assets/upload.txt") {
                            entries.push_str("<Contents><Key>upload.txt</Key><Size>7</Size><LastModified>2026-10-10T00:00:00Z</LastModified></Contents>");
                        }
                        entries
                    };
                    format!("<?xml version=\"1.0\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>fixture-assets</Name><Prefix></Prefix><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{entries}</ListBucketResult>").into_bytes()
                } else if method == "POST" && target.contains("uploads") {
                    format!("<InitiateMultipartUploadResult><Bucket>fixture-assets</Bucket><Key>{path}</Key><UploadId>fixture-upload</UploadId></InitiateMultipartUploadResult>").into_bytes()
                } else if method == "POST" && target.contains("uploadId=") {
                    b"<CompleteMultipartUploadResult><ETag>fixture</ETag></CompleteMultipartUploadResult>".to_vec()
                } else if method == "PUT" {
                    if let Some(source) = header.lines().find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("x-amz-copy-source"))
                            .map(|(_, v)| v.trim())
                    }) {
                        let decoded = decode(source);
                        let content = objects
                            .get(&format!("/{}", decoded.trim_start_matches('/')))
                            .expect("copy source must be a completed staging object")
                            .clone();
                        if path == "/fixture-assets/docs/config.json" {
                            *data.lock().unwrap() = content.clone();
                        }
                        objects.insert(path.into(), content);
                        b"<CopyObjectResult><ETag>fixture</ETag></CopyObjectResult>".to_vec()
                    } else {
                        objects.insert(path.into(), body);
                        Vec::new()
                    }
                } else if method == "DELETE" {
                    objects.remove(path);
                    Vec::new()
                } else if method == "HEAD" {
                    if !objects.contains_key(path) {
                        status = "404 Not Found";
                    }
                    Vec::new()
                } else {
                    match objects.get(path) {
                        Some(body) => body.clone(),
                        None => {
                            status = "404 Not Found";
                            Vec::new()
                        }
                    }
                };
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(&response);
            }
        });
        Self {
            endpoint,
            requests,
            file,
            stop,
            hold,
            worker: Some(worker),
        }
    }
}
fn decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            bytes.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            bytes.push(input[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).unwrap()
}
impl Drop for Storage {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
struct DaemonCleanup(std::path::PathBuf);
impl Drop for DaemonCleanup {
    fn drop(&mut self) {
        if let Ok(client) = kiln_daemon::client::Client::connect(&self.0.to_string_lossy(), None) {
            client.send(kiln_proto::ClientMsg::Shutdown);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
fn sidebar_entry<'a>(h: &'a Harness<'_, KilnApp>, name: &'a str) -> Option<egui_kittest::Node<'a>> {
    h.query_all_by_label(name).find(|n| {
        n.accesskit_node().role() == egui::accesskit::Role::Button && n.rect().left() > 1000.0
    })
}
#[track_caller]
fn pump(h: &mut Harness<'_, KilnApp>, ready: impl Fn(&Harness<'_, KilnApp>) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(Instant::now() < end, "GUI timed out: {:#?}", h.root());
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[test]
fn folders_stay_in_inspector_files_have_independent_editors_and_save_exact_object() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base.join("cfg")).unwrap();
    std::fs::create_dir_all(base.join("project")).unwrap();
    // One test in this process; never use the user's config, accounts or daemon.
    unsafe {
        std::env::set_var("KILN_SOCKET", base.join("daemon.sock"));
        std::env::set_var("KILN_CONFIG_DIR", base.join("cfg"));
        std::env::set_var("KILN_ACCOUNTS_SANDBOX", base.join("accounts"));
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
    }
    let _cleanup = DaemonCleanup(base.join("daemon.sock"));
    let server = Storage::new();
    let manager = RemoteManager::load();
    let profile = ConnectionProfile {
        id: "fixture-navigation".into(),
        name: "Release assets".into(),
        endpoint: RemoteEndpoint::S3 {
            bucket: "fixture-assets".into(),
            region: "us-east-1".into(),
            endpoint: Some(server.endpoint.clone()),
            path_style: true,
            prefix: String::new(),
            aws_profile: None,
            aws_auth: Some(kiln_remote::S3Authentication::Manual),
        },
    };
    manager
        .save(
            profile,
            Secrets {
                access_key: Some("fixture-access".into()),
                secret_key: Some("fixture-secret".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut h = Harness::builder()
        .with_step_dt(1.0 / 60.0)
        .with_size([1440.0, 900.0])
        .wgpu()
        .build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(base.join("project"))));
    pump(&mut h, |h| h.state().debug_focused_session().is_some());
    h.get_by_label("도구").click();
    h.run_steps(2);
    h.get_by_label("원격 연결").click();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Release assets")
        .click();
    pump(&mut h, |h| sidebar_entry(h, "docs").is_some());
    assert!(
        h.state().debug_tool_keys().is_empty(),
        "connecting must not open a directory card"
    );
    assert!(
        sidebar_entry(&h, "docs").unwrap().rect().left() > 900.0,
        "folder belongs in the inspector"
    );
    let shots = std::path::Path::new("/tmp/kiln-remote-navigation");
    std::fs::create_dir_all(shots).unwrap();
    h.render()
        .unwrap()
        .save(shots.join("app-folders-dark.png"))
        .unwrap();
    // Separate the folder gesture from the earlier connection click.
    h.run_steps(40);
    let folder_rect = sidebar_entry(&h, "docs").unwrap().rect();
    sidebar_entry(&h, "docs").unwrap().click();
    h.run_steps(2);
    assert_eq!(
        sidebar_entry(&h, "docs").unwrap().rect(),
        folder_rect,
        "selection must not move the row during a double-click"
    );
    sidebar_entry(&h, "docs").unwrap().click();
    pump(&mut h, |h| sidebar_entry(h, "config.json").is_some());
    assert!(
        h.state().debug_tool_keys().is_empty(),
        "folder navigation must preserve the main terminal"
    );
    sidebar_entry(&h, "config.json").unwrap().click();
    pump(&mut h, |h| {
        h.query_by_role(egui::accesskit::Role::MultilineTextInput)
            .is_some_and(|n| {
                n.accesskit_node().value().as_deref() == Some("{\"theme\":\"dark\"}\n")
            })
    });
    assert_eq!(h.state().debug_tool_keys().len(), 1);
    assert!(h.state().debug_tool_keys()[0].contains("docs/config.json"));
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .click();
    let primary = egui::Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    h.key_press_modifiers(primary, egui::Key::End);
    h.event(egui::Event::Text("// saved from Kiln\n".into()));
    h.run_steps(3);
    // Reopening keeps the draft and requires explicit credentials, not a reread.
    let edited = h
        .get_by_role(egui::accesskit::Role::MultilineTextInput)
        .accesskit_node()
        .value()
        .unwrap()
        .to_owned();
    let before_downloads = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("GET /fixture-assets/docs/config.json "))
        .count();
    let ctx = h.ctx.clone();
    h.state_mut().debug_checkpoint_restore(&ctx);
    h.run_steps(3);
    assert_eq!(
        h.get_by_role(egui::accesskit::Role::MultilineTextInput)
            .accesskit_node()
            .value()
            .as_deref(),
        Some(edited.as_str())
    );
    assert!(
        !server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("PUT ")),
        "restore cannot write remote data"
    );
    h.query_all_by_label("연결")
        .find(|n| {
            n.accesskit_node().role() == egui::accesskit::Role::Button && n.rect().left() < 1000.0
        })
        .unwrap()
        .click();
    pump(&mut h, |h| {
        h.query_by_label("원격 저장")
            .is_some_and(|n| !n.accesskit_node().is_disabled())
    });
    assert_eq!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("GET /fixture-assets/docs/config.json "))
            .count(),
        before_downloads,
        "credentials must not overwrite the restored draft"
    );
    h.get_by_label("원격 저장").click();
    pump(&mut h, |_| {
        String::from_utf8_lossy(&server.file.lock().unwrap()).contains("saved from Kiln")
    });
    pump(&mut h, |h| {
        h.query_by_label("원격 파일을 저장했습니다").is_some()
    });
    // Reconnect the restored navigator independently of its editor.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Release assets")
        .click();
    h.run_steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Release assets")
        .click();
    pump(&mut h, |h| sidebar_entry(h, "config.json").is_some());
    // Inspector remains navigable independently of the editor.
    pump(&mut h, |h| {
        h.query_by_label("상위 폴더")
            .is_some_and(|n| !n.accesskit_node().is_disabled())
    });
    h.get_by_label("상위 폴더").click();
    pump(&mut h, |h| sidebar_entry(h, "readme.md").is_some());
    let root_folder_top = sidebar_entry(&h, "docs").unwrap().rect().top();
    sidebar_entry(&h, "readme.md").unwrap().click();
    pump(&mut h, |h| {
        h.query_by_role(egui::accesskit::Role::MultilineTextInput)
            .is_some_and(|n| n.accesskit_node().value().as_deref() == Some("# Remote notes\n"))
    });
    assert_eq!(
        h.state().debug_tool_keys().len(),
        2,
        "two files must retain separate editors"
    );
    assert_eq!(
        sidebar_entry(&h, "docs").unwrap().rect().top(),
        root_folder_top,
        "file actions must not shift the navigation list"
    );
    for (name, theme) in [("dark", "kiln-dark"), ("light", "kiln-light")] {
        let ctx = h.ctx.clone();
        h.state_mut().debug_set_theme(&ctx, theme);
        h.run_steps(5);
        h.render()
            .unwrap()
            .save(shots.join(format!("app-editor-{name}.png")))
            .unwrap();
    }
    // Drop a file, hide its inspector, and keep its transfer protected without
    // preventing an unrelated editor from closing.
    let upload = base.join("upload.txt");
    std::fs::write(&upload, b"fixture").unwrap();
    server.hold.store(true, Ordering::SeqCst);
    let pointer = sidebar_entry(&h, "readme.md").unwrap().rect().center();
    h.event(egui::Event::PointerMoved(pointer));
    h.input_mut().dropped_files.push(Arc::new(Dropped(upload)));
    pump(&mut h, |_| {
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("POST ") && r.contains("upload.txt") && r.contains("uploads"))
    });
    h.get_by_label("도구 닫기").click();
    h.run_steps(3);
    assert!(!h.state().debug_sheet_open());
    let pane = h.state().debug_focused_pane_id().unwrap();
    h.state_mut()
        .debug_queue_action(Action::ClosePane(pane, false));
    h.run_steps(3);
    assert_eq!(
        h.state().debug_tool_keys().len(),
        1,
        "unrelated editor may close during sidebar upload"
    );
    h.state_mut().debug_queue_action(Action::CloseWorkspace(0));
    h.run_steps(3);
    assert_eq!(h.state().debug_workspace_count(), 1);
    assert!(
        h.state()
            .debug_toast_titles()
            .contains(&"원격 작업 진행 중".to_owned())
    );
    h.state_mut()
        .debug_queue_action(Action::QuitPreservingDrafts);
    h.run_steps(3);
    assert!(
        !h.output()
            .viewport_output
            .values()
            .flat_map(|v| &v.commands)
            .any(|c| matches!(c, egui::ViewportCommand::Close))
    );
    server.hold.store(false, Ordering::SeqCst);
    pump(&mut h, |_| {
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("DELETE ") && r.contains("upload.txt.kiln-upload"))
    });
    h.get_by_label("도구").click();
    h.run_steps(2);
    h.get_by_label("원격 연결").click();
    pump(&mut h, |h| sidebar_entry(h, "upload.txt").is_some());
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("PUT /fixture-assets/docs/config.json "))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .any(|r| r.starts_with("PUT /fixture-assets/docs/config.json "))
    );
    assert!(
        !requests
            .iter()
            .any(|r| r.starts_with("PUT /fixture-assets/readme.md "))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.starts_with("PUT /fixture-assets/upload.txt "))
            .count(),
        1
    );
    drop(requests);
}
