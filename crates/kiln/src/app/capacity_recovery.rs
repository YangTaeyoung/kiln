//! Bounded recovery from Codex's structured terminal capacity error.
//! Read-only transcript observation; never infer errors from rendered user text.
use kiln_common::paths::Canonical;
use super::{KilnApp, Pane, PaneKind, agent_runtime, layout::PaneId};
use anyhow::{Context, Result, ensure};
use kiln_accounts::Tool;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) const CONTINUE: &str = "Continue the interrupted task from the existing conversation after the model capacity error. Check the current state before repeating any action that may already have completed.";
const DELAYS: [u64; 5] = [15, 30, 60, 120, 240];
const TAIL: u64 = 2 * 1024 * 1024;
#[derive(Clone, Debug)]
pub(super) struct Failure {
    pub turn: String,
    at: SystemTime,
    checkpoint: Option<(PathBuf, u64, SystemTime)>,
}
impl Failure {
    fn still_current(&self) -> bool {
        self.checkpoint
            .as_ref()
            .is_some_and(|(path, len, modified)| {
                fs::symlink_metadata(path).is_ok_and(|m| {
                    m.is_file() && m.len() == *len && m.modified().ok() == Some(*modified)
                })
            })
    }
}

fn parse_tail(bytes: &[u8]) -> Option<Failure> {
    if !bytes.ends_with(b"\n") {
        return None;
    }
    let mut failure = None;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            failure = None;
            continue;
        };
        if row["type"] != "event_msg" {
            continue;
        }
        let payload = &row["payload"];
        match payload["type"].as_str() {
            Some("task_started" | "turn_started" | "user_message" | "turn_aborted") => {
                failure = None
            }
            Some("task_complete" | "turn_complete") => {
                failure = None;
                if payload["error"]["codex_error_info"] != "server_overloaded" {
                    continue;
                }
                let turn = payload["turn_id"].as_str()?;
                if turn.is_empty() || turn.len() > 100 {
                    return None;
                }
                let stamp = chrono::DateTime::parse_from_rfc3339(row["timestamp"].as_str()?)
                    .ok()?
                    .timestamp_millis();
                if stamp < 0 {
                    return None;
                }
                failure = Some(Failure {
                    turn: turn.into(),
                    at: UNIX_EPOCH + Duration::from_millis(stamp as u64),
                    checkpoint: None,
                });
            }
            _ => {}
        }
    }
    failure
}

pub(super) fn failure(record: &agent_runtime::Record) -> Result<Option<Failure>> {
    let config = kiln_common::paths::config_dir();
    failure_in(record, &config.join("conversation-sync/tracked"))
}
fn failure_in(record: &agent_runtime::Record, tracked: &Path) -> Result<Option<Failure>> {
    ensure!(record.tool == Tool::Codex, "not Codex");
    let id = record
        .conversation
        .as_deref()
        .context("unconfirmed conversation")?;
    let root = record
        .spec
        .env
        .iter()
        .find(|(k, _)| k == "CODEX_HOME")
        .context("unpinned namespace")?;
    let root = Path::new(&root.1).canonical()?;
    let cwd = Path::new(record.spec.cwd.as_deref().context("missing cwd")?).canonical()?;
    let entries = match fs::read_dir(tracked) {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };
    for entry in entries.take(10_000) {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.metadata()?.len() > 32 * 1024 {
            continue;
        }
        let Ok(source) = serde_json::from_slice::<Value>(&fs::read(entry.path())?) else {
            continue;
        };
        if source["id"] != id
            || source["tool"] != "codex"
            || source["root"].as_str() != root.to_str()
        {
            continue;
        }
        let Some(path) = source["path"].as_str().map(PathBuf::from) else {
            continue;
        };
        // Check every component; canonicalization alone would permit an account-escaping link.
        let relative = path
            .strip_prefix(&root)
            .context("transcript outside account")?;
        let mut checked = root.clone();
        for part in relative.components() {
            ensure!(
                matches!(part, std::path::Component::Normal(_)),
                "unsafe transcript path"
            );
            checked.push(part);
            ensure!(
                !fs::symlink_metadata(&checked)?.file_type().is_symlink(),
                "linked transcript"
            );
        }
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "non-file transcript"
        );
        let mut file = fs::File::open(&path)?;
        let before = file.metadata()?;
        let mut first = String::new();
        BufReader::new((&mut file).take(128 * 1024)).read_line(&mut first)?;
        let header: Value = serde_json::from_str(&first)?;
        ensure!(
            header["type"] == "session_meta" && header["payload"]["id"] == id,
            "wrong transcript identity"
        );
        ensure!(
            Path::new(
                header["payload"]["cwd"]
                    .as_str()
                    .context("missing transcript cwd")?
            )
            .canonical()?
                == cwd,
            "wrong workspace"
        );
        let start = before.len().saturating_sub(TAIL);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        (&mut file).take(TAIL + 1).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        ensure!(
            before.len() == after.len() && before.modified()? == after.modified()?,
            "transcript still writing"
        );
        ensure!(bytes.ends_with(b"\n"), "incomplete transcript record");
        let bytes = if start > 0 {
            bytes
                .iter()
                .position(|b| *b == b'\n')
                .map(|i| &bytes[i + 1..])
                .unwrap_or(&[])
        } else {
            &bytes
        };
        let mut failure = parse_tail(bytes);
        if let Some(failure) = &mut failure {
            failure.checkpoint = Some((path, after.len(), after.modified()?));
        }
        return Ok(failure);
    }
    Ok(None)
}
struct Observation {
    conversation: Option<String>,
    pane: PaneId,
    path: PathBuf,
    generation: String,
    failure: Option<Failure>,
}
struct Job {
    turn: String,
    attempts: usize,
    due: Option<Instant>,
    paused: bool,
    failed_at: SystemTime,
    automatic_input: Option<SystemTime>,
}
impl Job {
    fn new() -> Self {
        Self {
            turn: String::new(),
            attempts: 0,
            due: None,
            paused: false,
            failed_at: UNIX_EPOCH,
            automatic_input: None,
        }
    }
    fn observe(
        &mut self,
        failure: &Failure,
        input_at: SystemTime,
        exited: bool,
        observed_since: SystemTime,
        now: Instant,
    ) -> bool {
        if self.turn != failure.turn {
            if input_at != observed_since
                && input_at > self.failed_at
                && Some(input_at) != self.automatic_input
            {
                self.attempts = 0;
            }
            self.turn = failure.turn.clone();
            self.failed_at = failure.at;
            self.paused = self.attempts >= DELAYS.len();
            self.due = DELAYS
                .get(self.attempts)
                .map(|delay| now + Duration::from_secs(*delay));
        }
        if self.due.is_none() {
            return false;
        }
        if input_at > failure.at && (!exited || input_at > observed_since) {
            self.due = None;
            self.paused = true;
        }
        !self.paused && self.due.is_some_and(|due| due <= now)
    }
}
#[derive(Default)]
pub(super) struct Recovery {
    last_poll: Option<Instant>,
    pending: Option<Receiver<Vec<Observation>>>,
    jobs: HashMap<PaneId, Job>,
}
impl Recovery {
    pub fn label(&self, pane: PaneId) -> Option<String> {
        let job = self.jobs.get(&pane)?;
        if job.paused {
            return Some(kiln_common::i18n::tr("혼잡 자동 재시도 중지됨").into());
        }
        let due = job.due?;
        let seconds = due.saturating_duration_since(Instant::now()).as_secs();
        Some(kiln_common::trf!(
            "모델 혼잡 · {seconds}초 후 재시도 ({attempt}/5)",
            attempt = job.attempts + 1
        ))
    }
    pub fn cancel(&mut self, pane: PaneId) {
        if let Some(job) = self.jobs.get_mut(&pane) {
            job.due = None;
            job.paused = true;
        }
    }
}
impl KilnApp {
    pub(super) fn tick_capacity_recovery(&mut self, ctx: &egui::Context) {
        self.capacity_recovery
            .jobs
            .retain(|pane, _| self.panes.contains_key(pane));
        if !self.conn.is_connected() {
            return;
        }
        if let Some(rows) = self
            .capacity_recovery
            .pending
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.capacity_recovery.pending = None;
            for row in rows {
                let Some(session) = self.panes.get(&row.pane).and_then(Pane::session) else {
                    continue;
                };
                if self.cli_restarts.contains_key(&row.pane)
                    || self.cli_confirmations.contains_key(&row.pane)
                {
                    continue;
                }
                let Some(path) = self
                    .launch_specs
                    .get(&row.pane)
                    .and_then(agent_runtime::record_path)
                else {
                    continue;
                };
                if path != row.path {
                    continue;
                }
                let Ok(record) = agent_runtime::read(&path) else {
                    continue;
                };
                if super::conversation_sync::handover::blocked(&kiln_common::paths::config_dir(), &path, &record) {
                    self.capacity_recovery.cancel(row.pane);
                    continue;
                }
                if record.generation != row.generation
                    || record.conversation != row.conversation
                    || !record.confirmed
                {
                    continue;
                }
                let Some(failure) = row.failure else {
                    if let Some(job) = self.capacity_recovery.jobs.get_mut(&row.pane) {
                        job.due = None;
                        job.paused = false;
                    }
                    continue;
                };
                let Ok(age) = failure.at.elapsed() else {
                    continue;
                };
                if age > Duration::from_secs(600) {
                    continue;
                }
                let Some(info) = self.conn.infos.get(&session) else {
                    continue;
                };
                let exited = info.exited.is_some();
                if !exited && info.fg_process.as_deref() != Some("codex") {
                    continue;
                }
                let input_at = self
                    .conn
                    .input_times
                    .borrow()
                    .get(&session)
                    .copied()
                    .unwrap_or(self.conn.observed_since);
                let job = self
                    .capacity_recovery
                    .jobs
                    .entry(row.pane)
                    .or_insert_with(Job::new);
                if !job.observe(
                    &failure,
                    input_at,
                    exited,
                    self.conn.observed_since,
                    Instant::now(),
                ) {
                    continue;
                }
                if !failure.still_current() {
                    continue;
                }
                if self
                    .rotator
                    .mgr
                    .validate_launch_environment(record.tool, &record.spec.env)
                    .is_err()
                {
                    job.due = None;
                    job.paused = true;
                    continue;
                }
                job.due = None;
                job.attempts += 1;
                if exited {
                    // Only replace an already exited child, retaining the exact account/cwd/UUID.
                    let Ok(mut spec) = agent_runtime::renew(&path) else {
                        self.capacity_recovery.cancel(row.pane);
                        continue;
                    };
                    spec.env
                        .push(("KILN_CAPACITY_RETRY_TURN".into(), failure.turn));
                    if !self.conn.kill(session) {
                        self.capacity_recovery.cancel(row.pane);
                        continue;
                    }
                    if let Some(pane) = self.panes.get_mut(&row.pane) {
                        pane.kind = PaneKind::Term {
                            session: None,
                            pending: None,
                            view: None,
                        };
                    }
                    self.launch_specs.insert(row.pane, spec);
                    self.cli_confirmations.insert(row.pane, Instant::now());
                    self.spawn_for_pane(row.pane);
                } else {
                    // The native TUI is idle after a terminal error; no signal or model switch.
                    // Bracketed paste prevents an accidental multiline command sequence.
                    self.conn.input(
                        session,
                        format!("\x1b[200~{CONTINUE}\x1b[201~\r").into_bytes(),
                    );
                    if let Some(job) = self.capacity_recovery.jobs.get_mut(&row.pane) {
                        job.automatic_input = self.conn.input_times.borrow().get(&session).copied();
                    }
                }
            }
        }
        if self.capacity_recovery.pending.is_none()
            && self
                .capacity_recovery
                .last_poll
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
        {
            let paths = self
                .launch_specs
                .iter()
                .filter_map(|(pane, spec)| {
                    agent_runtime::record_path(spec).map(|path| (*pane, path))
                })
                .collect::<Vec<_>>();
            let (tx, rx) = mpsc::channel();
            self.capacity_recovery.pending = Some(rx);
            self.capacity_recovery.last_poll = Some(Instant::now());
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let rows = paths
                    .into_iter()
                    .filter_map(|(pane, path)| {
                        let record = agent_runtime::read(&path).ok()?;
                        if record.tool != Tool::Codex || !record.confirmed {
                            return None;
                        }
                        let failure = failure(&record).ok()?;
                        Some(Observation {
                            conversation: record.conversation,
                            pane,
                            path,
                            generation: record.generation,
                            failure,
                        })
                    })
                    .collect();
                let _ = tx.send(rows);
                ctx.request_repaint();
            });
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: &str, error: Value) -> String {
        serde_json::json!({"type":"event_msg","timestamp":"2026-10-13T12:00:00Z","payload":{"type":kind,"turn_id":"turn-1","error":error}}).to_string()+"\n"
    }
    #[test]
    fn only_terminal_structured_capacity_errors_trigger_recovery() {
        let fail = event(
            "task_complete",
            serde_json::json!({"codex_error_info":"server_overloaded"}),
        );
        assert_eq!(parse_tail(fail.as_bytes()).unwrap().turn, "turn-1");
        for kind in ["user_message", "agent_message", "error", "stream_error"] {
            assert!(
                parse_tail(
                    event(
                        kind,
                        serde_json::json!({"codex_error_info":"server_overloaded"})
                    )
                    .as_bytes()
                )
                .is_none()
            );
        }
        for code in [
            "usage_limit_exceeded",
            "http_connection_failed",
            "unauthorized",
        ] {
            assert!(
                parse_tail(
                    event(
                        "task_complete",
                        serde_json::json!({"codex_error_info":code})
                    )
                    .as_bytes()
                )
                .is_none()
            );
        }
        for kind in [
            "task_started",
            "turn_started",
            "user_message",
            "turn_aborted",
            "task_complete",
        ] {
            assert!(parse_tail(format!("{fail}{}", event(kind, Value::Null)).as_bytes()).is_none());
        }
        assert!(parse_tail(fail.trim_end().as_bytes()).is_none());
    }
    #[test]
    fn backoff_deduplicates_and_user_input_cancels_without_replaying() {
        let base = UNIX_EPOCH + Duration::from_secs(1000);
        let now = Instant::now();
        let f = Failure {
            turn: "one".into(),
            at: base,
            checkpoint: None,
        };
        let mut j = Job::new();
        assert!(!j.observe(
            &f,
            base - Duration::from_secs(1),
            false,
            base - Duration::from_secs(20),
            now
        ));
        assert!(j.observe(
            &f,
            base - Duration::from_secs(1),
            false,
            base - Duration::from_secs(20),
            now + Duration::from_secs(15)
        ));
        j.due = None;
        j.attempts += 1;
        j.automatic_input = Some(base + Duration::from_secs(15));
        assert!(!j.observe(
            &f,
            base + Duration::from_secs(15),
            false,
            base - Duration::from_secs(20),
            now + Duration::from_secs(16)
        ));
        let f2 = Failure {
            turn: "two".into(),
            at: base + Duration::from_secs(17),
            checkpoint: None,
        };
        assert!(!j.observe(
            &f2,
            base + Duration::from_secs(15),
            false,
            base - Duration::from_secs(20),
            now + Duration::from_secs(17)
        ));
        assert_eq!(j.attempts, 1);
        assert_eq!(j.due, Some(now + Duration::from_secs(47)));
        assert!(!j.observe(
            &f2,
            base + Duration::from_secs(18),
            false,
            base - Duration::from_secs(20),
            now + Duration::from_secs(48)
        ));
        assert!(j.paused);
        assert!(j.due.is_none());
    }
    #[test]
    fn restoring_an_exited_failure_keeps_the_five_retry_budget() {
        let observed = UNIX_EPOCH + Duration::from_secs(1000);
        let now = Instant::now();
        let mut job = Job::new();
        for attempt in 0..=DELAYS.len() {
            let failure = Failure {
                turn: format!("turn-{attempt}"),
                at: if attempt == 0 {
                    observed - Duration::from_secs(30)
                } else {
                    observed + Duration::from_secs(attempt as u64 * 300)
                },
                checkpoint: None,
            };
            let tick = now + Duration::from_secs(attempt as u64 * 300);
            assert!(!job.observe(&failure, observed, true, observed, tick));
            assert_eq!(job.attempts, attempt);
            if let Some(delay) = DELAYS.get(attempt) {
                assert!(job.observe(
                    &failure,
                    observed,
                    true,
                    observed,
                    tick + Duration::from_secs(*delay)
                ));
                job.attempts += 1;
                job.due = None;
            } else {
                assert!(job.paused);
                assert!(job.due.is_none());
            }
        }
        let manual = observed + Duration::from_secs(2000);
        let next = Failure {
            turn: "manual-new-task".into(),
            at: manual + Duration::from_secs(1),
            checkpoint: None,
        };
        assert!(!job.observe(&next, manual, false, observed, now));
        assert_eq!(job.attempts, 0);
        assert!(job.due.is_some());
    }
    #[test]
    fn tracked_identity_and_account_scope_are_required() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonical().unwrap();
        let tracked = root.join("tracked");
        fs::create_dir(&tracked).unwrap();
        let path = root.join("rollout.jsonl");
        let id = "12345678-1234-1234-1234-123456789abc";
        let record = agent_runtime::Record {
            tool: Tool::Codex,
            spec: kiln_proto::SpawnSpec {
                cwd: Some(root.to_string_lossy().into()),
                env: vec![("CODEX_HOME".into(), root.to_string_lossy().into())],
                ..Default::default()
            },
            conversation: Some(id.into()),
            generation: "fixture".into(),
            version: "0.162.1".into(),
            confirmed: true,
            request_started: false,
        };
        let header = serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":root}})
            .to_string()
            + "\n";
        let contents = header
            + &event(
                "task_complete",
                serde_json::json!({"codex_error_info":"server_overloaded"}),
            );
        fs::write(&path, &contents).unwrap();
        fs::write(
            tracked.join("one.json"),
            serde_json::json!({"id":id,"tool":"codex","path":path,"root":root}).to_string(),
        )
        .unwrap();
        let observed = failure_in(&record, &tracked).unwrap().unwrap();
        assert!(observed.still_current());
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        use std::io::Write;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(event("task_started", Value::Null).as_bytes())
            .unwrap();
        assert!(!observed.still_current());
        assert!(failure_in(&record, &tracked).unwrap().is_none());
        let mut wrong = record.clone();
        wrong.conversation = Some("another".into());
        assert!(failure_in(&wrong, &tracked).unwrap().is_none());
        #[cfg(unix)]
        {
            fs::rename(&path, root.join("original.jsonl")).unwrap();
            std::os::unix::fs::symlink(root.join("original.jsonl"), &path).unwrap();
            assert!(failure_in(&record, &tracked).is_err());
        }
    }
    #[test]
    fn cancelled_job_never_has_a_deadline_and_budget_is_bounded() {
        let mut r = Recovery::default();
        r.jobs.insert(
            1,
            Job {
                turn: "t".into(),
                due: Some(Instant::now()),
                ..Job::new()
            },
        );
        r.cancel(1);
        assert!(r.jobs[&1].due.is_none());
        assert!(r.jobs[&1].paused);
        assert_eq!(DELAYS.iter().sum::<u64>(), 465);
        assert_eq!(DELAYS.get(5), None);
    }
}
