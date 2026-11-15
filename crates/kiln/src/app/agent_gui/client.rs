use super::{Command, Descriptor, Envelope, Event, QueuedCommand, Transcript, journal};
use anyhow::{Result, bail};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
/// A bounded local mailbox and journal reader. Dropping this never terminates the host.
pub struct Client {
    commands: crossbeam_channel::Sender<QueuedCommand>,
    events: crossbeam_channel::Receiver<Vec<Envelope>>,
    generation: Arc<Mutex<String>>,
    error: Arc<Mutex<Option<String>>>,
    unavailable: Arc<Mutex<Option<String>>>,
    local_pending: Arc<Mutex<BTreeSet<String>>>,
    stop: Arc<AtomicBool>,
}
impl Client {
    pub fn connect(descriptor: Descriptor) -> Self {
        Self::at_directory(journal::directory(&descriptor.id))
    }
    pub(super) fn at_directory(directory: Result<std::path::PathBuf>) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<QueuedCommand>(32);
        let (etx, erx) = crossbeam_channel::bounded(8);
        let generation = Arc::new(Mutex::new(String::new()));
        let error = Arc::new(Mutex::new(None));
        let unavailable = Arc::new(Mutex::new(None));
        let local_pending = Arc::new(Mutex::new(BTreeSet::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (e, u, local, s) = (
            error.clone(),
            unavailable.clone(),
            local_pending.clone(),
            stop.clone(),
        );
        std::thread::spawn(move || {
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
                    if u.lock().unwrap().is_some() {
                        continue;
                    }
                    if let Err(err) = journal::enqueue(&dir, &queued) {
                        *e.lock().unwrap() = Some(err.to_string());
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
        }
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
        if let Some(reason) = self.unavailable.lock().unwrap().clone() {
            bail!(reason);
        }
        if self.generation.lock().unwrap().is_empty() {
            bail!("agent is still connecting");
        }
        let id = journal::unique();
        let queued = QueuedCommand {
            id: id.clone(),
            generation: self.generation.lock().unwrap().clone(),
            command,
        };
        self.local_pending.lock().unwrap().insert(id.clone());
        if self.commands.try_send(queued).is_err() {
            self.local_pending.lock().unwrap().remove(&id);
            bail!("agent command queue is unavailable");
        }
        Ok(id)
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
