use super::{Command, Descriptor, Envelope, QueuedCommand, journal};
use anyhow::{Result, bail};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
/// A bounded local mailbox and journal reader. Dropping this never terminates the host.
pub struct Client {
    commands: crossbeam_channel::Sender<QueuedCommand>,
    events: crossbeam_channel::Receiver<Vec<Envelope>>,
    generation: Arc<Mutex<String>>,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}
impl Client {
    pub fn connect(descriptor: Descriptor) -> Self {
        Self::at_directory(journal::directory(&descriptor.id))
    }
    fn at_directory(directory: Result<std::path::PathBuf>) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<QueuedCommand>(32);
        let (etx, erx) = crossbeam_channel::bounded(8);
        let generation = Arc::new(Mutex::new(String::new()));
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (e, s) = (error.clone(), stop.clone());
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
            while !s.load(Ordering::Relaxed) {
                if let Some(batch) = pending.take() {
                    match etx.try_send(batch) {
                        Ok(()) => {}
                        Err(crossbeam_channel::TrySendError::Full(batch)) => pending = Some(batch),
                        Err(_) => break,
                    }
                }
                if pending.is_none() {
                    match journal::tail(&dir.join("events.jsonl"), &mut offset) {
                        Ok(batch) if !batch.is_empty() => {
                            pending = Some(batch);
                        }
                        Ok(_) => {}
                        Err(err) => {
                            if dir.join("events.jsonl").exists() {
                                *e.lock().unwrap() = Some(err.to_string());
                            }
                        }
                    }
                }
                for queued in rx.try_iter().take(32) {
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
            stop,
        }
    }
    pub fn drain(&mut self) -> Vec<Envelope> {
        let events: Vec<Envelope> = self.events.try_iter().take(8).flatten().collect();
        for envelope in &events {
            if matches!(envelope.event, super::Event::Generation) {
                *self.generation.lock().unwrap() = envelope.generation.clone();
            }
        }
        events
    }
    /// Success means locally queued; transcript CommandPhase tracks dispatch/acceptance.
    pub fn send(&self, command: Command) -> Result<()> {
        if self.generation.lock().unwrap().is_empty() {
            bail!("agent is still connecting");
        }
        let queued = QueuedCommand {
            id: journal::unique(),
            generation: self.generation.lock().unwrap().clone(),
            command,
        };
        self.commands
            .try_send(queued)
            .map_err(|_| anyhow::anyhow!("agent command queue is unavailable"))
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
