//! Conversation scans run in a separate `kiln sync-worker` process, so transcript
//! parsing never grows the GUI process. Requests and replies are JSON lines.
use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};

/// After a scan whose peak resident size exceeded this, the worker saves its
/// file-stamped results and exits; the next scan starts a new worker that loads them.
/// A worker whose input closes also saves them for the next app launch.
const RECYCLE_PEAK: u64 = 512 * 1024 * 1024;

/// Native metadata used by worker scans (Codex `/resume` registration in the app).
pub(super) trait NativeMetadata: Default {
    fn begin_scan(&mut self);
    fn apply(&mut self, source: &Source, gate: &Gate, request: Metadata<'_>) -> Result<Option<String>>;
}
impl NativeMetadata for codex_index::Registry {
    fn begin_scan(&mut self) {
        codex_index::Registry::begin_scan(self);
    }
    fn apply(&mut self, source: &Source, gate: &Gate, request: Metadata<'_>) -> Result<Option<String>> {
        codex_index::Registry::apply(self, source, gate, request)
    }
}

#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Scan {
        id: u64,
        pair: Pair,
        workspaces: Vec<PathBuf>,
        #[serde(default)]
        limits: BTreeMap<PathBuf, usize>,
        quiet_ms: u64,
        expedite: bool,
        reset: bool,
    },
    Cancel {
        id: u64,
    },
}
#[derive(Serialize, Deserialize)]
pub(super) enum Reply {
    Progress { id: u64, status: Status },
    Scanned {
        id: u64,
        result: Result<Status, String>,
        /// The worker saved its results and exits; later scans need a new worker.
        #[serde(default)]
        exiting: bool,
    },
}

pub(super) fn cache_path(config: &Path) -> PathBuf {
    config.join("conversation-sync/worker-cache.json")
}

fn write_line(out: &mut impl Write, value: &impl Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Peak resident size of this process in bytes.
fn peak_resident() -> u64 {
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 {
            let peak = usage.ru_maxrss.max(0) as u64;
            return if cfg!(target_os = "macos") { peak } else { peak * 1024 };
        }
    }
    0
}

/// Entry point of `kiln sync-worker`; returns when the GUI closes its input.
pub fn serve() -> Result<()> {
    serve_with::<codex_index::Registry>(
        BufReader::new(std::io::stdin()),
        std::io::stdout(),
        &kiln_common::paths::config_dir(),
        RECYCLE_PEAK,
        discovery::Scope::new,
    )
}

#[derive(Default)]
struct Shared {
    current: Option<(u64, Gate)>,
    /// Highest scan id the GUI has cancelled.
    cancelled: u64,
    /// The GUI closed its input; queued scans are cancelled.
    closed: bool,
}

pub(super) fn serve_with<M: NativeMetadata>(
    input: impl BufRead + Send + 'static,
    output: impl Write,
    config: &Path,
    recycle_peak: u64,
    scope: impl Fn(&Pair, Vec<PathBuf>) -> Result<discovery::Scope>,
) -> Result<()> {
    let (requests_tx, requests) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let reader = shared.clone();
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str(&line) {
                Ok(Request::Cancel { id }) => {
                    let mut shared = reader.lock().unwrap();
                    shared.cancelled = shared.cancelled.max(id);
                    if let Some((scan, gate)) = &shared.current
                        && *scan == id
                    {
                        gate.store(false, Ordering::Release);
                    }
                }
                Ok(request) => {
                    if requests_tx.send(request).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // A closed input means the GUI is gone; the running scan stops at its next check.
        let mut shared = reader.lock().unwrap();
        shared.closed = true;
        if let Some((_, gate)) = &shared.current {
            gate.store(false, Ordering::Release);
        }
    });
    let saved = cache_path(config);
    let mut cache: Cache = bounded(&saved, 256 * 1024 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let _ = fs::remove_file(&saved);
    let mut registry = M::default();
    let output = std::cell::RefCell::new(output);
    for request in requests {
        let Request::Scan { id, pair, workspaces, limits, quiet_ms, expedite, reset } = request else {
            continue;
        };
        if reset {
            cache = Cache::new();
            registry = M::default();
        }
        if expedite {
            cache.expedite();
        }
        cache.quiet = Duration::from_millis(quiet_ms);
        let gate: Gate = Arc::new(AtomicBool::new(true));
        {
            let mut shared = shared.lock().unwrap();
            if shared.cancelled >= id || shared.closed {
                gate.store(false, Ordering::Release);
            }
            shared.current = Some((id, gate.clone()));
        }
        registry.begin_scan();
        let last_progress = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
        let result = scope(&pair, workspaces)
            .and_then(|mut scope| {
                scope.limits=limits;
                scan_with_metadata(
                    config,
                    &pair,
                    &gate,
                    &mut cache,
                    Some(&scope),
                    |source, request| registry.apply(source, &gate, request),
                    |status| {
                        if last_progress.get().elapsed() >= Duration::from_millis(100) {
                            last_progress.set(Instant::now());
                            let progress = Reply::Progress { id, status: status.clone() };
                            let _ = write_line(&mut *output.borrow_mut(), &progress);
                        }
                    },
                )
            })
            .map_err(|e| e.to_string());
        shared.lock().unwrap().current = None;
        let exiting = peak_resident() > recycle_peak;
        if exiting {
            private_dir(saved.parent().context("Missing sync directory")?)?;
            replace(&saved, &serde_json::to_vec(&cache)?)?;
        }
        write_line(&mut *output.borrow_mut(), &Reply::Scanned { id, result, exiting })?;
        if exiting {
            return Ok(());
        }
    }
    private_dir(saved.parent().context("Missing sync directory")?)?;
    replace(&saved, &serde_json::to_vec(&cache)?)?;
    Ok(())
}

/// GUI side of a running `kiln sync-worker`. Replies arrive on the manager's event channel.
pub(super) struct Worker {
    child: Option<Child>,
    input: ChildStdin,
    pending: Arc<Mutex<HashMap<u64, Gate>>>,
    /// Set when the worker announced its exit; no further scans are sent to it.
    exiting: Arc<AtomicBool>,
}
impl Worker {
    pub fn start(exe: &Path, events: mpsc::SyncSender<Event>, ctx: egui::Context) -> Result<Self> {
        let mut child = kiln_common::process::hide_window(&mut Command::new(exe))
            .arg("sync-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Cannot start the conversation sync worker")?;
        let input = child.stdin.take().context("Sync worker input unavailable")?;
        let output = child.stdout.take().context("Sync worker output unavailable")?;
        let pending: Arc<Mutex<HashMap<u64, Gate>>> = Arc::default();
        let replies = pending.clone();
        let exiting = Arc::new(AtomicBool::new(false));
        let announced = exiting.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(reply) = serde_json::from_str::<Reply>(&line) else { continue };
                match reply {
                    Reply::Progress { id, status } => {
                        let gate = replies.lock().unwrap().get(&id).cloned();
                        if let Some(gate) = gate {
                            let _ = events.try_send(Event::Progress(gate, status));
                        }
                    }
                    Reply::Scanned { id, result, exiting } => {
                        if exiting {
                            announced.store(true, Ordering::Release);
                        }
                        let gate = replies.lock().unwrap().remove(&id);
                        if let Some(gate) = gate {
                            let _ = events.send(Event::Scanned(gate, result));
                        }
                    }
                }
                ctx.request_repaint();
            }
            let stopped: Vec<_> = replies.lock().unwrap().drain().map(|(_, gate)| gate).collect();
            for gate in stopped {
                let _ = events.send(Event::Scanned(
                    gate,
                    Err("Conversation sync worker stopped; retrying".into()),
                ));
            }
            ctx.request_repaint();
        });
        Ok(Self { child: Some(child), input, pending, exiting })
    }
    pub fn alive(&mut self) -> bool {
        !self.exiting.load(Ordering::Acquire)
            && self.child.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn scan(
        &mut self,
        id: u64,
        gate: Gate,
        pair: &Pair,
        workspaces: &[PathBuf],
        limits: &BTreeMap<PathBuf, usize>,
        quiet: Duration,
        expedite: bool,
        reset: bool,
    ) -> Result<()> {
        self.pending.lock().unwrap().insert(id, gate);
        let request = Request::Scan {
            id,
            pair: pair.clone(),
            workspaces: workspaces.to_vec(),
            limits: limits.clone(),
            quiet_ms: quiet.as_millis() as u64,
            expedite,
            reset,
        };
        if let Err(error) = write_line(&mut self.input, &request) {
            self.pending.lock().unwrap().remove(&id);
            return Err(error.into());
        }
        Ok(())
    }
    pub fn cancel(&mut self, id: u64) {
        let _ = write_line(&mut self.input, &Request::Cancel { id });
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // The worker sees its input close, stops the running scan and exits; it is reaped off the UI thread.
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
