//! Owner-private append journal and atomic command mailbox; no credential material.
use super::{Envelope, Event, QueuedCommand, Transcript};
use anyhow::{Context, Result, ensure};
#[cfg(unix)]
mod retained;
#[cfg(unix)]
pub(super) use retained::RetainedStorage;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub(super) const MAX_RECORD: usize = 2 * 1024 * 1024;
pub(super) const MAX_JOURNAL: u64 = 256 * 1024 * 1024;
const TERMINAL_RESERVE: u64 = 64 * 1024;
#[derive(serde::Serialize, serde::Deserialize)]
struct Failure {
    reason: String,
}
/// Independent of journal append availability. A permanent marker seals this ID read-only.
pub(super) fn failure_reason(dir: &Path) -> Option<String> {
    let marker = dir.join("failure.json");
    if fs::symlink_metadata(&marker)
        .ok()
        .is_some_and(|m| m.file_type().is_file())
    {
        return Some(read::<Failure>(&marker).map(|f|f.reason).unwrap_or_else(|_|"Conversation storage failure marker is unreadable; retained history is read-only".into()));
    }
    if fs::metadata(dir.join("events.jsonl"))
        .ok()
        .is_some_and(|m| m.len() >= MAX_JOURNAL - TERMINAL_RESERVE)
    {
        return Some("Conversation journal is exhausted; retained history is read-only. Pending submissions are not replayed; open a new conversation.".into());
    }
    None
}
pub(super) fn worker_absent(dir: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("host.lock"))
    else {
        return false;
    };
    file.try_lock().is_ok()
}
pub(super) fn pending_ids(dir: &Path) -> Vec<String> {
    fs::read_dir(dir.join("commands"))
        .into_iter()
        .flatten()
        .flatten()
        .take(256)
        .filter_map(|entry| read::<QueuedCommand>(&entry.path()).ok().map(|q| q.id))
        .collect()
}
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) fn unique() -> String {
    let mut bytes = [0; 16];
    use ring::rand::{SecureRandom, SystemRandom};
    SystemRandom::new().fill(&mut bytes).expect("OS randomness");
    format!(
        "{:039}-{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub(super) fn directory(id: &str) -> Result<PathBuf> {
    ensure!(valid_id(id), "invalid agent ID");
    Ok(std::path::absolute(kiln_common::paths::config_dir())?
        .join("agent-gui")
        .join(id))
}
pub(super) fn private_dir(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            fs::symlink_metadata(path)?.file_type().is_dir(),
            "invalid private directory"
        );
    }
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(super) fn open(path: &Path, append: bool) -> Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let f = o.open(path)?;
    ensure!(f.metadata()?.is_file(), "invalid private file");
    Ok(f)
}
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(super) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_observed(path, bytes, |_| Ok(())).map_err(|failure| failure.error)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublishPoint {
    BeforeRename,
    RenameAttempt,
    AfterRename,
}
#[derive(Debug)]
pub(super) struct PublicationError {
    pub uncertain: bool,
    error: anyhow::Error,
}
impl std::fmt::Display for PublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}
impl std::error::Error for PublicationError {}
fn atomic_observed(
    path: &Path,
    bytes: &[u8],
    mut observe: impl FnMut(PublishPoint) -> Result<()>,
) -> std::result::Result<(), PublicationError> {
    let tmp = path.with_extension(format!("{}.tmp", unique()));
    let mut uncertain = false;
    let result = (|| {
        ensure!(bytes.len() <= MAX_RECORD, "oversized agent record");
        let mut f = open(&tmp, false)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        observe(PublishPoint::BeforeRename)?;
        uncertain = true;
        observe(PublishPoint::RenameAttempt)?;
        fs::rename(&tmp, path)?;
        observe(PublishPoint::AfterRename)?;
        sync_directory(path.parent().context("missing private parent")?)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|error| PublicationError { uncertain, error })
}
pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "invalid agent record"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_RECORD, "oversized agent record");
    Ok(serde_json::from_slice(&bytes)?)
}
pub(super) struct Journal {
    file: File,
    limit: u64,
    failure: Option<String>,
    dir: PathBuf,
    pub generation: String,
    pub transcript: Transcript,
}
impl Journal {
    pub fn open(dir: &Path, generation: String) -> Result<Self> {
        // Windows append handles cannot set the file length. Recover with a
        // normal write handle, then reopen for append-only journal writes.
        let file = open(&dir.join("events.jsonl"), false)?;
        ensure!(
            file.metadata()?.len() <= MAX_JOURNAL,
            "agent journal is full"
        );
        let mut transcript = Transcript::default();
        let mut offset = 0;
        loop {
            let batch = tail(&dir.join("events.jsonl"), &mut offset)?;
            if batch.is_empty() {
                break;
            }
            for event in batch {
                transcript.apply(&event);
            }
        }
        // Drop an incomplete final append after a crash, before writing a new generation.
        file.set_len(offset)?;
        drop(file);
        let file = open(&dir.join("events.jsonl"), true)?;
        let failure = failure_reason(dir);
        if let Some(reason) = &failure {
            transcript.apply(&Envelope {
                sequence: 0,
                generation: transcript.generation.clone(),
                event: Event::RuntimeUnavailable {
                    reason: reason.clone(),
                    rejected: pending_ids(dir),
                },
            });
        }
        Ok(Self {
            file,
            limit: MAX_JOURNAL,
            failure,
            dir: dir.into(),
            generation,
            transcript,
        })
    }
    pub fn push(&mut self, event: Event) -> Result<Envelope> {
        ensure!(
            self.failure.is_none(),
            "Conversation storage is unavailable; retained history is read-only. Pending commands will not be replayed; open a new conversation."
        );
        let result = self.append(event, self.limit.saturating_sub(TERMINAL_RESERVE));
        if let Err(error) = &result {
            self.seal(&format!("Conversation storage is unavailable: {error}. Retained history is read-only; pending submissions are not replayed. Open a new conversation."));
        }
        result
    }
    fn append(&mut self, event: Event, budget: u64) -> Result<Envelope> {
        let envelope = Envelope {
            sequence: self.transcript.sequence + 1,
            generation: self.generation.clone(),
            event,
        };
        let mut bytes = serde_json::to_vec(&envelope)?;
        ensure!(bytes.len() <= MAX_RECORD, "oversized agent event");
        bytes.push(b'\n');
        ensure!(
            self.file
                .metadata()?
                .len()
                .saturating_add(bytes.len() as u64)
                <= budget,
            "agent journal ordinary event budget is exhausted"
        );
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.transcript.apply(&envelope);
        Ok(envelope)
    }
    fn seal(&mut self, reason: &str) {
        let reason = reason.chars().take(1024).collect::<String>();
        self.failure = Some(reason.clone());
        if let Ok(bytes) = serde_json::to_vec(&Failure {
            reason: reason.clone(),
        }) {
            let _ = atomic(&self.dir.join("failure.json"), &bytes);
        }
        // Small bounded terminal records use a separate reserve. Physical storage failure may
        // prevent both writes; clients also observe failure.json and the OS host lock directly.
        let _ = self.append(Event::Error(reason.clone()), self.limit);
        let _ = self.append(Event::Lifecycle(super::Lifecycle::Failed), self.limit);
        let _ = self.flush();
        self.transcript.apply(&Envelope {
            sequence: 0,
            generation: self.generation.clone(),
            event: Event::RuntimeUnavailable {
                reason,
                rejected: pending_ids(&self.dir),
            },
        });
    }
    pub fn flush(&self) -> Result<()> {
        Ok(self.file.sync_all()?)
    }
}
pub(super) fn tail(path: &Path, offset: &mut u64) -> Result<Vec<Envelope>> {
    let mut f = File::open(path)?;
    tail_file(&mut f, offset)
}
fn tail_file(f: &mut File, offset: &mut u64) -> Result<Vec<Envelope>> {
    ensure!(
        f.metadata()?.len() <= MAX_JOURNAL,
        "oversized agent journal"
    );
    f.seek(SeekFrom::Start(*offset))?;
    let mut r = BufReader::new(f);
    let mut out = Vec::new();
    for _ in 0..128 {
        let mut bytes = Vec::new();
        let n = r
            .by_ref()
            .take((MAX_RECORD + 2) as u64)
            .read_until(b'\n', &mut bytes)?;
        if n == 0 {
            break;
        }
        ensure!(n <= MAX_RECORD + 1, "oversized agent event");
        if bytes.last() != Some(&b'\n') {
            break;
        }
        let event = serde_json::from_slice(&bytes).context("invalid agent journal event")?;
        *offset += n as u64;
        out.push(event);
    }
    Ok(out)
}
pub(super) fn enqueue(
    dir: &Path,
    command: &QueuedCommand,
) -> std::result::Result<(), PublicationError> {
    enqueue_observed(dir, command, |_| Ok(()))
}
pub(super) fn enqueue_observed(
    dir: &Path,
    command: &QueuedCommand,
    observe: impl FnMut(PublishPoint) -> Result<()>,
) -> std::result::Result<(), PublicationError> {
    let bytes=(||->Result<Vec<u8>> {
    ensure!(valid_id(&command.id), "invalid command ID");
    ensure!(
        failure_reason(dir).is_none(),
        "Conversation storage is unavailable; queued commands are rejected and will not be replayed. Open a new conversation."
    );
    let queue = dir.join("commands");
    private_dir(&queue)?;
    ensure!(
        fs::read_dir(&queue)?.take(257).count() < 256,
        "agent command queue is full"
    );
        Ok(serde_json::to_vec(command)?)
    })().map_err(|error|PublicationError {uncertain:false,error})?;
    atomic_observed(
        &dir.join("commands").join(format!("{}.json", command.id)),
        &bytes,
        observe,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enqueue_faults_preserve_publication_boundary_and_cleanup_temporary_files() {
        for point in [PublishPoint::BeforeRename, PublishPoint::AfterRename] {
            let root = tempfile::tempdir().unwrap();
            let command = QueuedCommand {
                id: "fault-command".into(),
                generation: "generation".into(),
                command: super::super::Command::Interrupt,
            };
            let failure = enqueue_observed(root.path(), &command, |at| {
                if at == point {
                    anyhow::bail!("fixture I/O failure");
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(failure.uncertain, point == PublishPoint::AfterRename);
            let files = std::fs::read_dir(root.path().join("commands"))
                .unwrap()
                .map(|file| file.unwrap().path())
                .collect::<Vec<_>>();
            if point == PublishPoint::AfterRename {
                assert_eq!(files.len(), 1);
                let published: QueuedCommand = read(&files[0]).unwrap();
                assert_eq!(published.id, command.id);
            } else {
                assert!(files.is_empty());
            }
            assert!(
                files
                    .iter()
                    .all(|file| file.extension().is_some_and(|ext| ext == "json"))
            );
        }
    }
    #[test]
    fn replay_recovers_native_tool_identity_and_incomplete_tail() {
        let d = tempfile::tempdir().unwrap();
        let mut j = Journal::open(d.path(), "a".into()).unwrap();
        let path = d.path().join("events.jsonl");
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Tool(crate::app::agent_gui::ToolCall {
            id: "tool".into(),
            output: "result".into(),
            ..Default::default()
        }))
        .unwrap();
        drop(j);
        let retained = fs::read(&path).unwrap();
        open(&path, true)
            .unwrap()
            .write_all(b"{\"partial\"")
            .unwrap();
        let mut j = Journal::open(d.path(), "b".into()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), retained);
        // Recovery must retain an append handle, even if its cursor is moved.
        j.file.seek(SeekFrom::Start(0)).unwrap();
        j.push(Event::Generation).unwrap();
        assert_eq!(j.transcript.provider_session_id.as_deref(), Some("native"));
        assert_eq!(j.transcript.entries.len(), 1);
        let mut offset = 0;
        let replay = tail(&path, &mut offset).unwrap();
        assert_eq!(replay.len(), 4);
        assert_eq!(replay[3].generation, "b");
        assert!(fs::read(&path).unwrap().starts_with(&retained));
        drop(j);
        let retained = fs::read(&path).unwrap();
        // A complete existing journal also remains intact on the next restart.
        let mut j = Journal::open(d.path(), "c".into()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), retained);
        j.file.seek(SeekFrom::Start(0)).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native-next".into())).unwrap();
        drop(j);
        let mut offset = 0;
        let replay = tail(&path, &mut offset).unwrap();
        assert_eq!(replay.len(), 6);
        assert_eq!(
            replay.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            (1..=6).collect::<Vec<_>>()
        );
        assert_eq!(replay[4].generation, "c");
        assert!(matches!(&replay[5].event, Event::Session(id) if id == "native-next"));
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(&retained));
        assert_eq!(offset, bytes.len() as u64);
    }
    #[test]
    fn ids_never_escape_private_directory() {
        for id in ["", "../x", "a/b", ".", "a\\b"] {
            assert!(!valid_id(id));
        }
        assert!(valid_id(&unique()));
    }
}

#[cfg(test)]
mod exhaustion_regressions {
    use super::*;
    use crate::app::agent_gui::{Command, CommandPhase, Lifecycle, Message, Role, client::Client};
    #[test]
    fn ordinary_limit_reserves_terminal_outcome_and_seals_pending_work_readably() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path(), "generation".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Lifecycle(Lifecycle::Running)).unwrap();
        j.push(Event::CommandState {
            id: "inflight".into(),
            state: CommandPhase::Dispatched,
        })
        .unwrap();
        let limit = j.file.metadata().unwrap().len() + 64 * 1024 + 1;
        j.limit = limit;
        let lock = open(&dir.path().join("host.lock"), false).unwrap();
        lock.try_lock().unwrap();
        let mut client = Client::at_directory(Ok(dir.path().into()));
        let mut view = Transcript::default();
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Running {
            for e in client.drain() {
                view.apply(&e);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        enqueue(
            dir.path(),
            &QueuedCommand {
                id: "pending".into(),
                generation: "generation".into(),
                command: Command::Submit {
                    text: "never replay".into(),
                    attachments: vec![],
                    model: None,
                },
            },
        )
        .unwrap();
        assert!(
            j.push(Event::Message {
                message: Message {
                    id: "too-large-for-ordinary-budget".into(),
                    role: Role::Assistant,
                    text: "x".repeat(8192),
                    attachments: vec![]
                },
                append: false
            })
            .is_err()
        );
        assert!(j.file.metadata().unwrap().len() < limit);
        assert!(
            j.file.metadata().unwrap().len() > limit - 64 * 1024,
            "terminal records must use the reserved budget"
        );
        let mut offset = 0;
        let durable = tail(&dir.path().join("events.jsonl"), &mut offset).unwrap();
        assert!(
            durable
                .iter()
                .any(|e| matches!(e.event, Event::Lifecycle(Lifecycle::Failed)))
        );
        assert_eq!(j.transcript.lifecycle, Lifecycle::Failed);
        drop(lock);
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Failed || !view.commands.contains_key("pending") {
            for e in client.drain() {
                view.apply(&e);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(view.commands["inflight"], CommandPhase::Unconfirmed);
        assert_eq!(view.commands["pending"], CommandPhase::Rejected);
        assert_eq!(view.provider_session_id.as_deref(), Some("native"));
        assert!(!view.errors.is_empty());
        assert!(client.send(Command::Restart).is_err());
        assert!(
            client
                .send(Command::Submit {
                    text: "retry".into(),
                    attachments: vec![],
                    model: None
                })
                .is_err()
        );
        let mut reopened = Journal::open(dir.path(), "new".into()).unwrap();
        assert!(reopened.push(Event::Generation).is_err());
        assert_eq!(
            reopened.transcript.provider_session_id.as_deref(),
            Some("native")
        );
        assert!(dir.path().join("commands/pending.json").exists());
    }
    #[test]
    fn physical_write_failure_and_unwritable_sidecar_still_reaches_client_via_worker_lock() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path(), "generation".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Lifecycle(Lifecycle::Running)).unwrap();
        j.push(Event::CommandState {
            id: "inflight".into(),
            state: CommandPhase::Dispatched,
        })
        .unwrap();
        let lock = open(&dir.path().join("host.lock"), false).unwrap();
        lock.try_lock().unwrap();
        let mut client = Client::at_directory(Ok(dir.path().into()));
        let mut view = Transcript::default();
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Running {
            for e in client.drain() {
                view.apply(&e);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        fs::create_dir(dir.path().join("failure.json")).unwrap();
        j.file = File::open(dir.path().join("events.jsonl")).unwrap();
        assert!(j.push(Event::Error("cannot be written".into())).is_err());
        drop(lock);
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Failed {
            for e in client.drain() {
                view.apply(&e);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(view.commands["inflight"], CommandPhase::Unconfirmed);
        assert_eq!(view.provider_session_id.as_deref(), Some("native"));
        assert!(
            view.errors
                .iter()
                .any(|e| e.contains("worker is unavailable"))
        );
        assert!(client.send(Command::Restart).is_err());
    }
    #[test]
    fn reconnect_delivers_out_of_band_failure_after_all_replay_batches_so_old_running_cannot_overwrite_it()
     {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path(), "generation".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        for index in 0..300 {
            j.push(Event::Message {
                message: Message {
                    id: format!("m-{index}"),
                    role: Role::Assistant,
                    text: "retained history".into(),
                    attachments: vec![],
                },
                append: false,
            })
            .unwrap();
            if index % 50 == 0 {
                j.push(Event::Lifecycle(Lifecycle::Ready)).unwrap();
            }
        }
        j.push(Event::Lifecycle(Lifecycle::Running)).unwrap();
        let last_sequence = j.transcript.sequence;
        let lock = open(&dir.path().join("host.lock"), false).unwrap();
        lock.try_lock().unwrap();
        j.file = File::open(dir.path().join("events.jsonl")).unwrap();
        assert!(j.push(Event::Error("storage failure".into())).is_err());
        let mut client = Client::at_directory(Ok(dir.path().into()));
        let mut view = Transcript::default();
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Failed {
            for event in client.drain() {
                if matches!(event.event, Event::RuntimeUnavailable { .. }) {
                    assert_eq!(view.entries.len(), 300);
                    assert_eq!(view.sequence, last_sequence);
                }
                view.apply(&event);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(3));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        for event in client.drain() {
            view.apply(&event);
        }
        assert_eq!(view.lifecycle, Lifecycle::Failed);
        assert_eq!(view.entries.len(), 300);
        assert_eq!(view.sequence, last_sequence);
        drop(lock);
    }

    #[test]
    fn failure_sidecar_reaches_client_while_worker_lock_is_still_held_and_terminal_appends_fail() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(dir.path(), "generation".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Lifecycle(Lifecycle::Running)).unwrap();
        let last_sequence = j.transcript.sequence;
        let lock = open(&dir.path().join("host.lock"), false).unwrap();
        lock.try_lock().unwrap();
        j.file = File::open(dir.path().join("events.jsonl")).unwrap();
        assert!(j.push(Event::Error("write fails".into())).is_err());
        assert!(!worker_absent(dir.path()));
        assert!(failure_reason(dir.path()).is_some());
        let mut client = Client::at_directory(Ok(dir.path().into()));
        let mut view = Transcript::default();
        let start = std::time::Instant::now();
        while view.lifecycle != Lifecycle::Failed {
            for e in client.drain() {
                view.apply(&e);
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(view.sequence, last_sequence);
        assert_eq!(view.provider_session_id.as_deref(), Some("native"));
        assert!(client.send(Command::Restart).is_err());
        drop(lock);
    }
}
