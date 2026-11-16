use super::{Command, Descriptor, Envelope, Event, QueuedCommand, Transcript, journal};
use anyhow::{Result, bail};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};
type LocalMailbox = Mutex<BTreeSet<String>>;
// A reconnect in the same GUI process must not infer unknown delivery while a
// dropped client's background worker can still enqueue its local command.
#[derive(Default)]
struct WriterRegistry {
    revision: u64,
    mailboxes: Vec<Weak<LocalMailbox>>,
}
static LOCAL_MAILBOXES: OnceLock<Mutex<WriterRegistry>> = OnceLock::new();
fn writers() -> &'static Mutex<WriterRegistry> {
    LOCAL_MAILBOXES.get_or_init(Default::default)
}
fn register_mailbox(local: &Arc<LocalMailbox>) {
    let mut registry = writers().lock().unwrap();
    registry.mailboxes.retain(|weak| weak.strong_count() > 0);
    registry.mailboxes.push(Arc::downgrade(local));
}
fn queued_locally_in(registry: &mut WriterRegistry, id: &str) -> bool {
    registry.mailboxes.retain(|weak| weak.strong_count() > 0);
    registry
        .mailboxes
        .iter()
        .filter_map(Weak::upgrade)
        .any(|local| local.lock().unwrap().contains(id))
}
#[cfg(test)]
fn queued_locally(id: &str) -> bool {
    queued_locally_in(&mut writers().lock().unwrap(), id)
}
// The revision is advanced both before I/O and when the write finishes, before
// the worker can release its local mailbox. No writer mutex is held during I/O.
struct EnqueuePublication;
impl EnqueuePublication {
    fn begin() -> Self {
        writers().lock().unwrap().revision += 1;
        Self
    }
}
impl Drop for EnqueuePublication {
    fn drop(&mut self) {
        writers().lock().unwrap().revision += 1;
    }
}
#[derive(Clone)]
struct ReplayCheckpoint {
    generation: String,
    sequence: u64,
    queued: BTreeSet<String>,
    writer_revision: u64,
}
#[derive(Default)]
struct WorkerHooks {
    #[cfg(test)]
    enqueue_fault: Option<journal::PublishPoint>,
    #[cfg(test)]
    before_failure: Option<(
        crossbeam_channel::Sender<()>,
        crossbeam_channel::Receiver<()>,
    )>,
    #[cfg(test)]
    inside_enqueue: Option<(
        crossbeam_channel::Sender<()>,
        crossbeam_channel::Receiver<()>,
    )>,
    #[cfg(test)]
    after_checkpoint: Option<(
        crossbeam_channel::Sender<()>,
        crossbeam_channel::Receiver<()>,
    )>,
}
impl WorkerHooks {
    #[cfg(test)]
    fn publication(&mut self, point: journal::PublishPoint) -> Result<()> {
        if self.enqueue_fault == Some(point) {
            self.enqueue_fault = None;
            bail!("fixture publication failure");
        }
        let _ = point;
        Ok(())
    }
    fn before_failure(&mut self) {
        #[cfg(test)]
        if let Some((reached, proceed)) = self.before_failure.take() {
            let _ = reached.send(());
            let _ = proceed.recv();
        }
    }
    fn inside_enqueue(&mut self) {
        #[cfg(test)]
        if let Some((reached, proceed)) = self.inside_enqueue.take() {
            let _ = reached.send(());
            let _ = proceed.recv();
        }
    }
    fn after_checkpoint(&mut self) {
        #[cfg(test)]
        if let Some((reached, proceed)) = self.after_checkpoint.take() {
            let _ = reached.send(());
            let _ = proceed.recv();
        }
    }
}
fn enqueue_command(
    dir: &std::path::Path,
    command: &QueuedCommand,
    hooks: &mut WorkerHooks,
) -> std::result::Result<(), journal::PublicationError> {
    let _publication = EnqueuePublication::begin();
    hooks.inside_enqueue();
    #[cfg(test)]
    {
        journal::enqueue_observed(dir, command, |point| hooks.publication(point))
    }
    #[cfg(not(test))]
    {
        journal::enqueue(dir, command)
    }
}
// Match host::mailbox: only atomically published .json commands can execute.
// An unreadable or oversized mailbox cannot prove that a command is absent.
fn published_command_ids(dir: &std::path::Path) -> Result<BTreeSet<String>> {
    let entries = match std::fs::read_dir(dir.join("commands")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    let paths = entries
        .take(257)
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    anyhow::ensure!(paths.len() <= 256, "agent command queue is full");
    paths
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| journal::read::<QueuedCommand>(&path).map(|command| command.id))
        .collect()
}
/// A bounded local mailbox and journal reader. Dropping this never terminates the host.
pub struct Client {
    commands: crossbeam_channel::Sender<QueuedCommand>,
    events: crossbeam_channel::Receiver<Vec<Envelope>>,
    generation: Arc<Mutex<String>>,
    error: Arc<Mutex<Option<String>>>,
    unavailable: Arc<Mutex<Option<String>>>,
    local_pending: Arc<Mutex<BTreeSet<String>>>,
    stop: Arc<AtomicBool>,
    replay: Arc<Mutex<Option<ReplayCheckpoint>>>,
}
impl Client {
    pub fn connect(descriptor: Descriptor) -> Self {
        Self::at_directory(journal::directory(&descriptor.id))
    }
    pub(super) fn at_directory(directory: Result<std::path::PathBuf>) -> Self {
        Self::at_directory_gated(directory, None, WorkerHooks::default())
    }
    #[cfg(test)]
    pub(super) fn paused(
        directory: std::path::PathBuf,
        generation: &str,
    ) -> (Self, crossbeam_channel::Sender<()>) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let client = Self::at_directory_gated(Ok(directory), Some(rx), WorkerHooks::default());
        *client.generation.lock().unwrap() = generation.into();
        (client, tx)
    }
    #[cfg(test)]
    pub(super) fn faulted_enqueue(
        directory: std::path::PathBuf,
        generation: &str,
        point: journal::PublishPoint,
    ) -> (Self, crossbeam_channel::Sender<()>) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let client = Self::at_directory_gated(
            Ok(directory),
            Some(rx),
            WorkerHooks {
                enqueue_fault: Some(point),
                ..Default::default()
            },
        );
        *client.generation.lock().unwrap() = generation.into();
        (client, tx)
    }
    fn at_directory_gated(
        directory: Result<std::path::PathBuf>,
        start: Option<crossbeam_channel::Receiver<()>>,
        mut hooks: WorkerHooks,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<QueuedCommand>(32);
        let (etx, erx) = crossbeam_channel::bounded(8);
        let generation = Arc::new(Mutex::new(String::new()));
        let error = Arc::new(Mutex::new(None));
        let unavailable = Arc::new(Mutex::new(None));
        let local_pending = Arc::new(Mutex::new(BTreeSet::new()));
        register_mailbox(&local_pending);
        let replay = Arc::new(Mutex::new(None));
        let checkpoint = replay.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (e, u, local, s) = (
            error.clone(),
            unavailable.clone(),
            local_pending.clone(),
            stop.clone(),
        );
        std::thread::spawn(move || {
            if let Some(start) = start {
                let _ = start.recv();
            }
            let dir = match directory {
                Ok(d) => d,
                Err(err) => {
                    *e.lock().unwrap() = Some(err.to_string());
                    return;
                }
            };
            let mut offset = 0;
            let mut pending = None;
            let mut pending_outcomes = false;
            let mut observed = Transcript::default();
            let mut reported = None;
            while !s.load(Ordering::Relaxed) {
                if let Some(batch) = pending.take() {
                    match etx.try_send(batch) {
                        Ok(()) => {
                            pending_outcomes = false;
                        }
                        Err(crossbeam_channel::TrySendError::Full(batch)) => pending = Some(batch),
                        Err(_) => break,
                    }
                }
                let mut reason = journal::failure_reason(&dir);
                if reason.is_none() && journal::worker_absent(&dir) {
                    reason=Some("Agent worker is unavailable. Pending submissions are unconfirmed and will not be replayed.".into());
                }
                let mut at_end = false;
                if pending.is_none() {
                    match journal::tail(&dir.join("events.jsonl"), &mut offset) {
                        Ok(batch) if !batch.is_empty() => {
                            *checkpoint.lock().unwrap() = None;
                            for event in &batch {
                                if matches!(event.event, Event::Generation) && reason.is_none() {
                                    *u.lock().unwrap() = None;
                                    reported = None;
                                }
                                if let Event::CommandState { id, .. } = &event.event {
                                    local.lock().unwrap().remove(id);
                                }
                                observed.apply(event);
                            }
                            pending = Some(batch);
                        }
                        Ok(_) => at_end = true,
                        Err(err) => {
                            if dir.join("events.jsonl").exists() {
                                reason = Some(format!(
                                    "Conversation history storage is unavailable: {err}"
                                ));
                                at_end = true;
                            }
                        }
                    }
                }
                if let Some(reason) = reason {
                    *e.lock().unwrap() = Some(reason.clone());
                    hooks.before_failure();
                    *u.lock().unwrap() = Some(reason.clone());
                    if at_end && pending.is_none() && reported.as_ref() != Some(&reason) {
                        let mut rejected = journal::pending_ids(&dir)
                            .into_iter()
                            .collect::<BTreeSet<_>>();
                        rejected.extend(local.lock().unwrap().iter().cloned());
                        pending = Some(vec![Envelope {
                            sequence: 0,
                            generation: observed.generation.clone(),
                            event: Event::RuntimeUnavailable {
                                reason: reason.clone(),
                                rejected: rejected.into_iter().collect(),
                            },
                        }]);
                        reported = Some(reason);
                    }
                }
                // An undelivered outcome batch pauses further command consumption.
                if !pending_outcomes {
                    for queued in rx.try_iter().take(32) {
                        *checkpoint.lock().unwrap() = None;
                        if s.load(Ordering::Relaxed) {
                            break;
                        }
                        if u.lock().unwrap().is_some() {
                            continue;
                        }
                        if let Err(err) = enqueue_command(&dir, &queued, &mut hooks) {
                            *e.lock().unwrap() = Some(err.to_string());
                            local.lock().unwrap().remove(&queued.id);
                            let outcome = Envelope {
                                sequence: 0,
                                generation: queued.generation,
                                event: Event::EnqueueFailed {
                                    id: queued.id,
                                    uncertain: err.uncertain,
                                },
                            };
                            observed.apply(&outcome);
                            pending.get_or_insert_with(Vec::new).push(outcome);
                            pending_outcomes = true;
                        }
                    }
                }
                if at_end
                    && pending.is_none()
                    && u.lock().unwrap().is_none()
                    && !observed.generation.is_empty()
                {
                    let revision = writers().lock().unwrap().revision;
                    let queued = published_command_ids(&dir);
                    // Cover host outcome/removal ordering as well as concurrent
                    // local publications. Publishing and querying use writer ->
                    // checkpoint lock order; an expired mailbox cannot validate
                    // a checkpoint from before its final possible publication.
                    let journal_stable = std::fs::metadata(dir.join("events.jsonl"))
                        .is_ok_and(|meta| meta.len() == offset);
                    let checkpointed = {
                        let registry = writers().lock().unwrap();
                        let mut checkpoint = checkpoint.lock().unwrap();
                        if let Ok(queued) = queued
                            && journal_stable
                            && registry.revision == revision
                        {
                            *checkpoint = Some(ReplayCheckpoint {
                                generation: observed.generation.clone(),
                                sequence: observed.sequence,
                                queued,
                                writer_revision: revision,
                            });
                            true
                        } else {
                            *checkpoint = None;
                            false
                        }
                    };
                    if checkpointed {
                        hooks.after_checkpoint();
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        });
        Self {
            commands: tx,
            events: erx,
            generation,
            error,
            unavailable,
            local_pending,
            stop,
            replay,
        }
    }
    #[cfg(test)]
    pub(super) fn fixture_mailbox(
        events: Vec<Envelope>,
        generation: &str,
    ) -> (Self, crossbeam_channel::Receiver<QueuedCommand>) {
        let (commands, rx) = crossbeam_channel::bounded(32);
        let (tx, events_rx) = crossbeam_channel::bounded(8);
        tx.send(events).unwrap();
        let local_pending = Arc::new(Mutex::new(BTreeSet::new()));
        register_mailbox(&local_pending);
        (
            Self {
                commands,
                events: events_rx,
                generation: Arc::new(Mutex::new(generation.into())),
                error: Arc::new(Mutex::new(None)),
                unavailable: Arc::new(Mutex::new(None)),
                local_pending,
                stop: Arc::new(AtomicBool::new(false)),
                replay: Arc::new(Mutex::new(None)),
            },
            rx,
        )
    }
    pub fn drain(&mut self) -> Vec<Envelope> {
        let events: Vec<Envelope> = self.events.try_iter().take(8).flatten().collect();
        for envelope in &events {
            if matches!(envelope.event, Event::Generation) {
                *self.generation.lock().unwrap() = envelope.generation.clone();
            }
        }
        events
    }
    /// Returns the locally queued ID for pending-draft reconciliation, not provider acceptance.
    pub fn send(&self, command: Command) -> Result<String> {
        let generation = self.generation.lock().unwrap().clone();
        self.send_for_generation(&generation, command)
    }
    /// Bind every view control to the generation actually reduced by that view.
    /// A drained but unapplied Generation must never retarget an old approval.
    pub fn send_for_generation(&self, generation: &str, command: Command) -> Result<String> {
        self.send_with_registration(generation, command, || {})
    }
    fn send_with_registration(
        &self,
        generation: &str,
        command: Command,
        before_registration: impl FnOnce(),
    ) -> Result<String> {
        let unavailable = self.unavailable.lock().unwrap();
        if let Some(reason) = unavailable.as_ref() {
            bail!(reason.clone());
        }
        before_registration();
        if generation.is_empty() {
            bail!("agent is still connecting");
        }
        let id = journal::unique();
        let queued = QueuedCommand {
            id: id.clone(),
            generation: generation.into(),
            command,
        };
        self.local_pending.lock().unwrap().insert(id.clone());
        if self.commands.try_send(queued).is_err() {
            self.local_pending.lock().unwrap().remove(&id);
            bail!("agent command queue is unavailable");
        }
        drop(unavailable);
        Ok(id)
    }
    /// Missing is unknown (never rejected) only after background replay and queue
    /// inspection are consistent with this applied transcript watermark. Local
    /// mailboxes, including a dropped worker still finishing enqueue, take priority.
    pub fn submission_is_unknown(&self, id: &str, generation: &str, applied_sequence: u64) -> bool {
        let mut registry = writers().lock().unwrap();
        let replay = self.replay.lock().unwrap();
        replay.as_ref().is_some_and(|r| {
            r.writer_revision == registry.revision
                && r.generation == generation
                && r.sequence <= applied_sequence
                && !r.queued.contains(id)
        }) && !queued_locally_in(&mut registry, id)
    }
    pub fn invalidate_replay(&self) { *self.replay.lock().unwrap() = None; }
    /// Current background replay AND mailbox inspection, not only an old Ready event.
    pub fn replay_current(&self, generation: &str, sequence: u64) -> bool {
        let mut registry = writers().lock().unwrap();
        registry.mailboxes.retain(|weak| weak.strong_count() > 0);
        let no_local = registry.mailboxes.iter().filter_map(Weak::upgrade)
            .all(|local| local.lock().unwrap().is_empty());
        self.replay.lock().unwrap().as_ref().is_some_and(|r| {
            r.generation == generation && r.sequence <= sequence
                && r.writer_revision == registry.revision && r.queued.is_empty() && no_local
        })
    }
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap().take()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_gui::{Decision, Event};
    #[test]
    fn failure_transition_and_command_registration_share_accounting_boundary() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(root.path(), "generation".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        let (start, starting) = crossbeam_channel::bounded(1);
        let (failing, failure) = crossbeam_channel::bounded(1);
        let (fail, failed) = crossbeam_channel::bounded(1);
        let client = Client::at_directory_gated(
            Ok(root.path().into()),
            Some(starting),
            WorkerHooks {
                before_failure: Some((failing, failed)),
                ..Default::default()
            },
        );
        *client.generation.lock().unwrap() = "generation".into();
        let client = Arc::new(client);
        let (checked, check) = crossbeam_channel::bounded(1);
        let (resume, resumed) = crossbeam_channel::bounded(1);
        let sender = client.clone();
        let sending = std::thread::spawn(move || {
            sender
                .send_with_registration("generation", Command::Interrupt, || {
                    checked.send(()).unwrap();
                    resumed
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                })
                .unwrap()
        });
        check
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        journal::atomic(
            &root.path().join("failure.json"),
            br#"{"reason":"fixture failure"}"#,
        )
        .unwrap();
        start.send(()).unwrap();
        failure
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let excluded = client.unavailable.try_lock().is_err();
        fail.send(()).unwrap();
        resume.send(()).unwrap();
        let id = sending.join().unwrap();
        let mut client = Arc::try_unwrap(client).ok().unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut transcript = Transcript::default();
        loop {
            for event in client.drain() {
                transcript.apply(&event);
            }
            if transcript.commands.get(&id) == Some(&super::super::CommandPhase::Rejected) {
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(
            excluded,
            "failure must wait for registered command identity"
        );
        assert!(client.send(Command::Interrupt).is_err());
        assert!(!root.path().join("commands").exists());
    }
    #[test]
    fn approval_generation_comes_from_delivered_view_state_not_unseen_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(dir.path(), "viewed".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        let mut client = Client::at_directory(Ok(dir.path().into()));
        let started = std::time::Instant::now();
        while client.drain().is_empty() {
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        journal.generation = "unseen".into();
        journal.push(Event::Generation).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        client
            .send(Command::Respond {
                request_id: "old-request".into(),
                decision: Decision::Allow,
                answers: Default::default(),
            })
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            if let Ok(files) = std::fs::read_dir(dir.path().join("commands"))
                && let Some(file) = files
                    .flatten()
                    .find(|f| f.path().extension().is_some_and(|e| e == "json"))
            {
                let q: QueuedCommand = journal::read(&file.path()).unwrap();
                assert_eq!(q.generation, "viewed");
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn explicit_generation_survives_drain_while_legacy_send_remains_compatible() {
        let (mut client, rx) = Client::fixture_mailbox(
            vec![Envelope {
                sequence: 2,
                generation: "G2".into(),
                event: Event::Generation,
            }],
            "G1",
        );
        client.drain();
        client
            .send_for_generation("G1", Command::Interrupt)
            .unwrap();
        assert_eq!(rx.recv().unwrap().generation, "G1");
        client.send(Command::Interrupt).unwrap();
        assert_eq!(rx.recv().unwrap().generation, "G2");
        assert!(client.send_for_generation("", Command::Interrupt).is_err());
    }
    #[test]
    fn replay_checkpoint_requires_applied_watermark_and_absent_durable_mailbox() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(root.path(), "generation".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        for _ in 0..260 {
            journal
                .push(Event::Lifecycle(super::super::Lifecycle::Ready))
                .unwrap();
        }
        let q = QueuedCommand {
            id: "still-queued".into(),
            generation: "generation".into(),
            command: Command::Interrupt,
        };
        journal::enqueue(root.path(), &q).unwrap();
        let mut client = Client::at_directory(Ok(root.path().into()));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut applied = 0;
        loop {
            for event in client.drain() {
                applied = applied.max(event.sequence);
            }
            assert!(
                !client.submission_is_unknown("missing", "generation", 1),
                "an old applied watermark cannot prove replay completion"
            );
            assert!(
                !client.submission_is_unknown("still-queued", "generation", applied),
                "the durable mailbox can still execute"
            );
            if applied == journal.transcript.sequence
                && client.submission_is_unknown("missing", "generation", applied)
            {
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!client.submission_is_unknown("missing", "other-generation", applied));
    }
}

#[cfg(test)]
mod round2_tests {
    use super::*;
    #[test]
    fn review_round2_i2_completed_old_publication_invalidates_paused_new_reader_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(root.path(), "G1".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        journal
            .push(Event::Lifecycle(super::super::Lifecycle::Ready))
            .unwrap();
        let (a_reached_tx, a_reached_rx) = crossbeam_channel::bounded(1);
        let (a_release_tx, a_release_rx) = crossbeam_channel::bounded(1);
        let old = Client::at_directory_gated(
            Ok(root.path().into()),
            None,
            WorkerHooks {
                inside_enqueue: Some((a_reached_tx, a_release_rx)),
                ..Default::default()
            },
        );
        let id = old
            .send_for_generation(
                "G1",
                Command::Submit {
                    text: "original request".into(),
                    attachments: vec![],
                    model: None,
                },
            )
            .unwrap();
        a_reached_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (b_reached_tx, b_reached_rx) = crossbeam_channel::bounded(1);
        let (b_release_tx, b_release_rx) = crossbeam_channel::bounded(1);
        let mut restored = Client::at_directory_gated(
            Ok(root.path().into()),
            None,
            WorkerHooks {
                after_checkpoint: Some((b_reached_tx, b_release_rx)),
                ..Default::default()
            },
        );
        b_reached_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let applied = restored.drain().iter().map(|e| e.sequence).max().unwrap();
        assert_eq!(applied, 2);
        assert!(
            restored
                .replay
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .queued
                .is_empty()
        );
        assert!(!restored.submission_is_unknown(&id, "G1", applied));
        drop(old);
        a_release_tx.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while queued_locally(&id) {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(journal::pending_ids(root.path()).contains(&id));
        assert!(
            !restored.submission_is_unknown(&id, "G1", applied),
            "a stale empty checkpoint must not outlive the old writer's successful publication while the new reader is paused"
        );
        b_release_tx.send(()).unwrap();
        while !restored
            .replay
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|r| r.queued.contains(&id))
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!restored.submission_is_unknown(&id, "G1", applied));
        while !restored.submission_is_unknown("genuinely-missing", "G1", applied) {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod round2_publication_tests {
    use super::*;
    #[test]
    fn review_round2_unpublished_atomic_temp_file_is_not_an_executable_mailbox() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(root.path(), "G1".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        journal
            .push(Event::Lifecycle(super::super::Lifecycle::Ready))
            .unwrap();
        let queue = root.path().join("commands");
        journal::private_dir(&queue).unwrap();
        let q = QueuedCommand {
            id: "unpublished".into(),
            generation: "G1".into(),
            command: Command::Interrupt,
        };
        journal::atomic(&queue.join("orphan.tmp"), &serde_json::to_vec(&q).unwrap()).unwrap();
        let mut client = Client::at_directory(Ok(root.path().into()));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut applied = 0;
        while !client.submission_is_unknown("unpublished", "G1", applied) {
            for e in client.drain() {
                applied = applied.max(e.sequence);
            }
            assert!(
                std::time::Instant::now() < until,
                "host only consumes .json; an orphan atomic temp must not strand a restored draft"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
