//! Ordered PTY input with a bounded upgrade checkpoint. An empty channel alone
//! does not establish delivery: its worker may still be blocked writing a frame.

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const INPUT_QUIET_PERIOD: Duration = Duration::from_millis(500);

enum Message {
    Data(Vec<u8>),
    Delivered(Sender<()>),
}

pub(crate) struct InputQueue {
    sender: Sender<Message>,
    ingress: Mutex<Ingress>,
    pending: Arc<AtomicUsize>,
}

struct Ingress {
    accepting: bool,
    revision: u64,
    last_input: Instant,
    replies: Vec<Vec<u8>>,
}

impl InputQueue {
    pub(crate) fn new(mut writer: impl Write + Send + 'static, id: u64) -> io::Result<Self> {
        let (sender, receiver) = unbounded();
        let pending = Arc::new(AtomicUsize::new(0));
        let writes = pending.clone();
        std::thread::Builder::new()
            .name(format!("pty-w-{id}"))
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Data(data) => {
                            if writer.write_all(&data).is_err() {
                                break;
                            }
                            let _ = writer.flush();
                            writes.fetch_sub(1, Ordering::Release);
                        }
                        Message::Delivered(reply) => {
                            let _ = reply.send(());
                        }
                    }
                }
            })?;
        Ok(Self {
            sender,
            pending,
            ingress: Mutex::new(Ingress {
                accepting: true,
                revision: 0,
                last_input: Instant::now(),
                replies: Vec::new(),
            }),
        })
    }

    pub(crate) fn send(&self, data: Vec<u8>) -> io::Result<()> {
        let mut ingress = self.ingress.lock();
        if !ingress.accepting {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal is upgrading",
            ));
        }
        ingress.revision = ingress.revision.wrapping_add(1);
        ingress.last_input = Instant::now();
        self.enqueue(data)
    }

    fn enqueue(&self, data: Vec<u8>) -> io::Result<()> {
        self.pending.fetch_add(1, Ordering::Release);
        self.sender.send(Message::Data(data)).map_err(|_| {
            self.pending.fetch_sub(1, Ordering::Release);
            io::Error::new(io::ErrorKind::BrokenPipe, "terminal input worker closed")
        })
    }
    /// Observe input and screen under the same ingress lock used to accept all
    /// terminal keystrokes. In-flight or recently accepted input cannot prove idle.
    pub(crate) fn observe<T>(&self, read: impl FnOnce() -> T) -> (Option<u64>, T) {
        let ingress = self.ingress.lock();
        let ready = ingress.accepting
            && self.pending.load(Ordering::Acquire) == 0
            && ingress.last_input.elapsed() >= INPUT_QUIET_PERIOD;
        (ready.then_some(ingress.revision), read())
    }
    /// New work typed after observation wins. The control bytes are never queued
    /// behind input the native process has not consumed/rendered yet.
    pub(crate) fn send_if_unchanged(
        &self,
        revision: u64,
        unchanged: impl FnOnce() -> bool,
        data: Vec<u8>,
    ) -> io::Result<bool> {
        let mut ingress = self.ingress.lock();
        if !ingress.accepting
            || ingress.revision != revision
            || self.pending.load(Ordering::Acquire) != 0
            || ingress.last_input.elapsed() < INPUT_QUIET_PERIOD
            || !unchanged()
        {
            return Ok(false);
        }
        ingress.revision = ingress.revision.wrapping_add(1);
        ingress.last_input = Instant::now();
        self.enqueue(data)?;
        Ok(true)
    }

    /// Readers still consume output until Detach. Preserve their terminal
    /// responses in the upgrade checkpoint instead of rejecting or blocking them.
    pub(crate) fn respond(&self, data: Vec<u8>) -> io::Result<()> {
        let mut ingress = self.ingress.lock();
        if !ingress.accepting {
            ingress.replies.push(data);
            return Ok(());
        }
        self.enqueue(data)
    }

    pub(crate) fn pending_replies(&self) -> Vec<Vec<u8>> {
        self.ingress.lock().replies.clone()
    }

    /// Stop ingress atomically with the barrier, before pausing any host writer.
    /// The reply follows every previously accepted write, including in-flight IO.
    pub(crate) fn pause(&self) -> io::Result<Receiver<()>> {
        let mut ingress = self.ingress.lock();
        ingress.accepting = false;
        let (reply, delivered) = bounded(1);
        self.sender.send(Message::Delivered(reply)).map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "terminal input worker closed")
        })?;
        Ok(delivered)
    }

    pub(crate) fn resume(&self) {
        let mut ingress = self.ingress.lock();
        for data in ingress.replies.drain(..) {
            let _ = self.enqueue(data);
        }
        ingress.accepting = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    struct HeldWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        entered: Sender<()>,
        release: Receiver<()>,
        held: bool,
    }
    impl Write for HeldWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.held {
                self.held = true;
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            self.bytes.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn upgrade_waits_for_in_flight_input_and_cancel_preserves_order_exactly_once() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (entered, started) = bounded(1);
        let (release, held) = bounded(1);
        let queue = InputQueue::new(
            HeldWriter {
                bytes: bytes.clone(),
                entered,
                release: held,
                held: false,
            },
            1,
        )
        .unwrap();
        queue.send(b"before".to_vec()).unwrap();
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        queue.send(b"queued".to_vec()).unwrap();
        let delivered = queue.pause().unwrap();
        assert_eq!(
            queue.send(b"late".to_vec()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        queue.respond(b"reply".to_vec()).unwrap();
        assert_eq!(queue.pending_replies(), vec![b"reply".to_vec()]);
        assert_eq!(
            delivered.recv_timeout(Duration::from_millis(20)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        );
        // Cancelling an upgrade reopens ingress without dropping the held input.
        queue.resume();
        queue.send(b"after".to_vec()).unwrap();
        let second = queue.pause().unwrap();
        release.send(()).unwrap();
        delivered.recv_timeout(Duration::from_secs(1)).unwrap();
        second.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(&*bytes.lock(), b"beforequeuedreplyafter");
        assert!(queue.pending_replies().is_empty());
    }

    #[test]
    fn new_work_after_a_snapshot_and_in_flight_input_reject_control_bytes() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (entered, started) = bounded(1);
        let (release, held) = bounded(1);
        let queue = InputQueue::new(
            HeldWriter {
                bytes: bytes.clone(),
                entered,
                release: held,
                held: false,
            },
            3,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(510));
        let (revision, _) = queue.observe(|| "idle native prompt");
        let revision = revision.unwrap();
        queue.send(b"start real work".to_vec()).unwrap();
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            queue.observe(|| ()).0.is_none(),
            "In-flight input cannot prove idle"
        );
        assert!(
            !queue
                .send_if_unchanged(revision, || true, b"must not run".to_vec())
                .unwrap()
        );
        release.send(()).unwrap();
        queue
            .pause()
            .unwrap()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        queue.resume();
        std::thread::sleep(Duration::from_millis(510));
        assert!(
            !queue
                .send_if_unchanged(revision, || true, b"stale input".to_vec())
                .unwrap()
        );
        let fresh = queue.observe(|| ()).0.unwrap();
        assert!(
            !queue
                .send_if_unchanged(fresh, || false, b"changed screen".to_vec())
                .unwrap()
        );
        assert!(
            queue
                .send_if_unchanged(fresh, || true, b"fresh control".to_vec())
                .unwrap()
        );
        queue
            .pause()
            .unwrap()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(&*bytes.lock(), b"start real workfresh control");
    }

    #[test]
    fn failed_worker_never_acknowledges_delivery() {
        struct FailedWriter;
        impl Write for FailedWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("owned fixture failure"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let queue = InputQueue::new(FailedWriter, 2).unwrap();
        queue.send(b"must not be acknowledged".to_vec()).unwrap();
        match queue.pause() {
            Ok(delivered) => assert_eq!(
                delivered.recv_timeout(Duration::from_secs(1)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            ),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::BrokenPipe),
        }
    }
}
