//! Ordered PTY input with a bounded upgrade checkpoint. An empty channel alone
//! does not establish delivery: its worker may still be blocked writing a frame.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use parking_lot::Mutex;
use std::io::{self, Write};

enum Message {
    Data(Vec<u8>),
    Delivered(Sender<()>),
}

pub(crate) struct InputQueue {
    sender: Sender<Message>,
    ingress: Mutex<Ingress>,
}

struct Ingress {
    accepting: bool,
    replies: Vec<Vec<u8>>,
}

impl InputQueue {
    pub(crate) fn new(mut writer: impl Write + Send + 'static, id: u64) -> io::Result<Self> {
        let (sender, receiver) = unbounded();
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
                        }
                        Message::Delivered(reply) => {
                            let _ = reply.send(());
                        }
                    }
                }
            })?;
        Ok(Self {
            sender,
            ingress: Mutex::new(Ingress {
                accepting: true,
                replies: Vec::new(),
            }),
        })
    }

    pub(crate) fn send(&self, data: Vec<u8>) -> io::Result<()> {
        let ingress = self.ingress.lock();
        if !ingress.accepting {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal is upgrading",
            ));
        }
        self.sender
            .send(Message::Data(data))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "terminal input worker closed"))
    }

    /// Readers still consume output until Detach. Preserve their terminal
    /// responses in the upgrade checkpoint instead of rejecting or blocking them.
    pub(crate) fn respond(&self, data: Vec<u8>) -> io::Result<()> {
        let mut ingress = self.ingress.lock();
        if !ingress.accepting {
            ingress.replies.push(data);
            return Ok(());
        }
        self.sender
            .send(Message::Data(data))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "terminal input worker closed"))
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
            let _ = self.sender.send(Message::Data(data));
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
