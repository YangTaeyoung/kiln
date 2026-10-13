//! Native metadata registration through thread/read; never starts a model turn.
use super::*;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdin, Command, Stdio},
};

#[derive(Default)]
pub(super) struct Registry {
    connections: HashMap<PathBuf, Connection>,
    failures: HashMap<PathBuf, String>,
    binary: Option<PathBuf>,
}
impl Registry {
    pub fn ensure(&mut self, source: &Source, gate: &Gate) -> Result<()> {
        if source.tool != Tool::Codex {
            return Ok(());
        }
        if let Some(error) = self.failures.get(&source.root) {
            anyhow::bail!("{error}");
        }
        let result = (|| {
            if !self.connections.contains_key(&source.root) {
                if self.binary.is_none() {
                    let binary = super::super::agent_launch::executable("codex")
                        .context("Install Codex CLI to register /resume conversations")?;
                    ensure!(
                        super::super::agent_runtime::probe(&binary)? == "0.162.1",
                        "Codex /resume registration requires CLI 0.162.1"
                    );
                    self.binary = Some(binary);
                }
                let connection = Connection::start(
                    self.binary.as_ref().unwrap(),
                    &source.root,
                    gate,
                    Duration::from_secs(10),
                )?;
                self.connections.insert(source.root.clone(), connection);
            }
            let connection = self.connections.get_mut(&source.root).unwrap();
            let result = connection.request(
                "thread/read",
                json!({"threadId":source.id,"includeTurns":false}),
                gate,
            )?;
            ensure!(
                result["thread"]["id"] == source.id && result["thread"]["cwd"] == source.cwd,
                "Codex /resume registration returned a different conversation"
            );
            ensure!(
                result["thread"]["path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p) == source.path),
                "Codex /resume registration returned a different transcript"
            );
            Ok(())
        })();
        if let Err(error) = &result {
            self.connections.remove(&source.root);
            self.failures.insert(
                source.root.clone(),
                format!("Codex /resume registration failed: {error}"),
            );
        }
        result
    }
}
struct Connection {
    child: Child,
    input: ChildStdin,
    replies: mpsc::Receiver<Result<Value, String>>,
    sequence: u64,
    timeout: Duration,
}
impl Connection {
    fn start(binary: &Path, root: &Path, gate: &Gate, timeout: Duration) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .args(["--no-daemon", "app-server"])
            .env("CODEX_HOME", root)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .context("Cannot start Codex metadata reader")?;
        let input = child.stdin.take().context("Codex input unavailable")?;
        let output = child.stdout.take().context("Codex output unavailable")?;
        let (tx, replies) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let read = reader
                    .by_ref()
                    .take(1024 * 1024)
                    .read_until(b'\n', &mut line);
                let result = match read {
                    Ok(0) => break,
                    Ok(_) if line.ends_with(b"\n") => serde_json::from_slice(&line)
                        .map_err(|_| "Invalid Codex metadata response".into()),
                    _ => Err(
                        "Codex metadata response exceeded its limit or could not be read".into(),
                    ),
                };
                let failed = result.is_err();
                if tx.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let mut connection = Self {
            child,
            input,
            replies,
            sequence: 0,
            timeout,
        };
        connection.request(
            "initialize",
            json!({"clientInfo":{"name":"kiln","version":env!("CARGO_PKG_VERSION")}}),
            gate,
        )?;
        connection.send(json!({"method":"initialized"}))?;
        Ok(connection)
    }
    fn send(&mut self, request: Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, &request)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }
    fn request(&mut self, method: &str, params: Value, gate: &Gate) -> Result<Value> {
        ensure!(gate.load(Ordering::Acquire), "Synchronization paused");
        self.sequence += 1;
        let id = self.sequence;
        self.send(json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            ensure!(gate.load(Ordering::Acquire), "Synchronization paused");
            ensure!(
                Instant::now() < deadline,
                "Codex metadata request timed out"
            );
            match self.replies.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(reply)) if reply["id"] == id => {
                    // Never expose server payloads, which can contain transcript text.
                    ensure!(
                        reply.get("error").is_none(),
                        "Codex rejected the metadata request"
                    );
                    return reply
                        .get("result")
                        .cloned()
                        .context("Missing Codex metadata result");
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => anyhow::bail!("{error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Codex metadata reader exited")
                }
            }
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        // Only the subprocess group created above belongs to this worker.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_reader_is_bounded_cancellable_and_never_starts_a_turn() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("codex-fixture");
        fs::write(&script, r#"#!/usr/bin/env python3
import json,sys,time
for line in sys.stdin:
 r=json.loads(line)
 if r['method']=='initialized': continue
 if r['method']=='hang': time.sleep(60)
 elif r['method']=='bad': print(json.dumps({'id':r['id'],'error':{'message':'PRIVATE CONTENT'}}),flush=True)
 else:
  assert r['method'] in ['initialize','thread/read']
  print(json.dumps({'id':r['id'],'result':{'thread':{'id':'fixture'}}}),flush=True)
"#).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let gate = Arc::new(AtomicBool::new(true));
        let mut reader =
            Connection::start(&script, root.path(), &gate, Duration::from_secs(3)).unwrap();
        assert_eq!(
            reader
                .request(
                    "thread/read",
                    json!({"threadId":"fixture","includeTurns":false}),
                    &gate
                )
                .unwrap()["thread"]["id"],
            "fixture"
        );
        let error = reader
            .request("bad", json!({}), &gate)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("PRIVATE CONTENT"));
        reader.timeout = Duration::from_millis(150);
        let start = Instant::now();
        assert!(
            reader
                .request("hang", json!({}), &gate)
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        drop(reader);
        assert!(start.elapsed() < Duration::from_secs(3));
        let mut reader =
            Connection::start(&script, root.path(), &gate, Duration::from_secs(10)).unwrap();
        gate.store(false, Ordering::Release);
        assert!(
            reader
                .request("thread/read", json!({}), &gate)
                .unwrap_err()
                .to_string()
                .contains("paused")
        );
    }
    #[test]
    #[ignore]
    fn native_read_registers_a_new_copy_in_an_existing_session_index() {
        let dir = PathBuf::from(
            std::env::var_os("KILN_SYNC_FIXTURE_DIR").expect("explicit fixture directory"),
        )
        .canonicalize()
        .unwrap();
        assert!(dir.join("owned-fixture").is_file());
        let root = dir.join("codex");
        assert!(root.join("state_5.sqlite").is_file());
        let binary = super::super::super::agent_launch::executable("codex").unwrap();
        assert_eq!(
            super::super::super::agent_runtime::probe(&binary).unwrap(),
            "0.162.1"
        );
        let gate = Arc::new(AtomicBool::new(true));
        let mut registry = Registry::default();
        registry.binary = Some(binary.clone());
        registry.connections.insert(
            root.clone(),
            Connection::start(&binary, &root, &gate, Duration::from_secs(10)).unwrap(),
        );
        let id = stable_id(&format!("native-index-{:?}", SystemTime::now()));
        let pair = Pair {
            claude: Endpoint {
                root: dir.join("claude"),
                label: "fixture".into(),
            },
            codex: Endpoint {
                root: root.clone(),
                label: "fixture".into(),
            },
        };
        let seed = Source {
            tool: Tool::Claude,
            root: dir.join("claude"),
            path: PathBuf::new(),
            cwd: dir.to_string_lossy().into_owned(),
            id: String::new(),
        };
        let source = destination(&pair, &seed, &id, chrono::Utc::now()).unwrap();
        let messages = vec![
            format::Message {
                images: Vec::new(),
                role: "user".into(),
                text: "Native index fixture: original user message".into(),
            },
            format::Message {
                images: Vec::new(),
                role: "assistant".into(),
                text: "Native index fixture: original assistant message".into(),
            },
        ];
        publish(
            &source.path,
            &format::render(
                Tool::Codex,
                &id,
                &source.cwd,
                &chrono::Utc::now().to_rfc3339(),
                &messages,
            )
            .unwrap(),
        )
        .unwrap();
        let params = json!({"cwd":source.cwd,"limit":100});
        // The native background scanner may race us and index this file first;
        // registration must succeed and be idempotent in either ordering.
        registry.ensure(&source, &gate).unwrap();
        let after = registry
            .connections
            .get_mut(&root)
            .unwrap()
            .request("thread/list", params, &gate)
            .unwrap();
        assert!(
            after["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["id"] == id)
        );
        assert_eq!(read_messages(&source).unwrap(), messages);
        println!("Native index registration verified: {id}");
    }
}
