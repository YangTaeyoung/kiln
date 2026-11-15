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
static LOCAL_MAILBOXES: OnceLock<Mutex<Vec<Weak<LocalMailbox>>>> = OnceLock::new();
fn register_mailbox(local: &Arc<LocalMailbox>) {
    let mut mailboxes = LOCAL_MAILBOXES
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    mailboxes.retain(|weak| weak.strong_count() > 0);
    mailboxes.push(Arc::downgrade(local));
}
fn queued_locally(id: &str) -> bool {
    let mut mailboxes = LOCAL_MAILBOXES
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    mailboxes.retain(|weak| weak.strong_count() > 0);
    mailboxes
        .iter()
        .filter_map(Weak::upgrade)
        .any(|local| local.lock().unwrap().contains(id))
}
#[derive(Clone)]
struct ReplayCheckpoint {
    generation: String,
    sequence: u64,
    queued: BTreeSet<String>,
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
        Self::at_directory_gated(directory, None)
    }
    #[cfg(test)]
    pub(super) fn paused(
        directory: std::path::PathBuf,
        generation: &str,
    ) -> (Self, crossbeam_channel::Sender<()>) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let client = Self::at_directory_gated(Ok(directory), Some(rx));
        *client.generation.lock().unwrap() = generation.into();
        (client, tx)
    }
    fn at_directory_gated(
        directory: Result<std::path::PathBuf>,
        start: Option<crossbeam_channel::Receiver<()>>,
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
            let mut observed = Transcript::default();
            let mut reported = None;
            while !s.load(Ordering::Relaxed) {
                if let Some(batch) = pending.take() {
                    match etx.try_send(batch) {
                        Ok(()) => {}
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
                for queued in rx.try_iter().take(32) {
                    *checkpoint.lock().unwrap() = None;
                    if s.load(Ordering::Relaxed) {
                        break;
                    }
                    if u.lock().unwrap().is_some() {
                        continue;
                    }
                    if let Err(err) = journal::enqueue(&dir, &queued) {
                        *e.lock().unwrap() = Some(err.to_string());
                    }
                }
                if at_end
                    && pending.is_none()
                    && u.lock().unwrap().is_none()
                    && !observed.generation.is_empty()
                {
                    let queued = journal::pending_ids(&dir).into_iter().collect();
                    // Queue removal follows a durable host command outcome. Recheck
                    // the journal length after queue inspection to close that race.
                    if std::fs::metadata(dir.join("events.jsonl"))
                        .is_ok_and(|meta| meta.len() == offset)
                    {
                        *checkpoint.lock().unwrap() = Some(ReplayCheckpoint {
                            generation: observed.generation.clone(),
                            sequence: observed.sequence,
                            queued,
                        });
                    } else {
                        *checkpoint.lock().unwrap() = None;
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
        if let Some(reason) = self.unavailable.lock().unwrap().clone() {
            bail!(reason);
        }
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
        Ok(id)
    }
    /// Missing is unknown (never rejected) only after background replay and queue
    /// inspection are consistent with this applied transcript watermark. Local
    /// mailboxes, including a dropped worker still finishing enqueue, take priority.
    pub fn submission_is_unknown(&self, id: &str, generation: &str, applied_sequence: u64) -> bool {
        let replay = self.replay.lock().unwrap();
        replay.as_ref().is_some_and(|r| {
            r.generation == generation && r.sequence <= applied_sequence && !r.queued.contains(id)
        }) && !queued_locally(id)
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
