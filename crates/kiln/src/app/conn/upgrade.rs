//! One automatic upgrade per daemon/target; retry explicitly after a failure.
use super::*;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Identity {
    pid: u32,
    server: String,
    target: String,
}

struct Pending {
    req: u32,
    started: Instant,
}

#[derive(Default)]
pub(super) struct Attempts {
    attempted: std::collections::HashSet<Identity>,
    pending: Option<Pending>,
    requests: std::collections::HashSet<u32>,
    announced: bool,
}

impl Attempts {
    pub fn request(&mut self, pid: u32, server: &str, target: &str, manual: bool) -> bool {
        if self.pending.is_some() {
            return false;
        }
        if !manual && (server == target || kiln_daemon::client::is_downgrade(server, target)) {
            return false;
        }
        let fresh = self.attempted.insert(Identity {
            pid,
            server: server.into(),
            target: target.into(),
        });
        manual || fresh
    }

    pub fn sent(&mut self, req: u32, now: Instant) {
        self.requests.insert(req);
        self.pending = Some(Pending { req, started: now });
        self.announced = false;
    }

    pub fn acknowledge(&mut self) -> bool {
        if self.announced {
            return false;
        }
        self.announced = true;
        true
    }

    /// None belongs to another request. Repeated/late upgrade errors are consumed
    /// without failing a newer pending request or showing the same error twice.
    pub fn error(&mut self, req: u32) -> Option<bool> {
        if !self.requests.contains(&req) {
            return None;
        }
        let current = self.pending.as_ref().is_some_and(|p| p.req == req);
        if current {
            self.pending = None;
        }
        Some(current)
    }

    pub fn expire(&mut self, now: Instant) -> Option<u32> {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| now.saturating_duration_since(p.started) >= Duration::from_secs(30))
        {
            return self.pending.take().map(|p| p.req);
        }
        None
    }

    pub fn disconnected(&mut self) {
        self.pending = None;
        self.requests.clear();
        self.announced = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_replaced_daemon_is_upgraded_even_after_an_earlier_attempt() {
        let mut attempts = Attempts::default();
        assert!(attempts.request(10, "0.2.8-old", "0.2.11-new", false));
        assert!(!attempts.request(10, "0.2.8-old", "0.2.11-new", false));
        assert!(attempts.request(11, "0.2.8-old", "0.2.11-new", false));
        assert!(!attempts.request(11, "0.2.12-new", "0.2.11-new", false));
    }
    #[test]
    fn same_daemon_does_not_retry_automatically_after_thirty_seconds() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        assert!(attempts.request(10, "0.2.8-old", "0.2.11-new", false));
        attempts.sent(7, now);
        assert_eq!(attempts.expire(now + Duration::from_secs(10)), None);
        assert_eq!(attempts.expire(now + Duration::from_secs(31)), Some(7));
        assert!(!attempts.request(10, "0.2.8-old", "0.2.11-new", false));
        assert_eq!(attempts.expire(now + Duration::from_secs(3600)), None);
        assert!(!attempts.request(10, "0.2.8-old", "0.2.11-new", false));
        assert!(!attempts.request(11, "0.2.11-new", "0.2.11-new", false));
    }

    #[test]
    fn failure_manual_retry_and_new_target_keep_request_identity_separate() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        assert!(attempts.request(10, "0.2.15-old", "0.2.16-new", false));
        attempts.sent(4, now);
        assert!(
            !attempts.request(10, "0.2.15-old", "0.2.16-new", true),
            "one pending request even for manual clicks"
        );
        assert_eq!(
            attempts.error(0),
            None,
            "an unrelated error must not finish the upgrade"
        );
        assert_eq!(attempts.error(4), Some(true));
        assert!(!attempts.request(10, "0.2.15-old", "0.2.16-new", false));
        assert!(attempts.request(10, "0.2.15-old", "0.2.16-new", true));
        attempts.sent(8, now);
        assert_eq!(
            attempts.error(4),
            Some(false),
            "late errors cannot finish the retry"
        );
        assert_eq!(attempts.error(8), Some(true));
        assert_eq!(attempts.error(8), Some(false));
        assert!(
            attempts.request(10, "0.2.15-old", "0.2.17-new", false),
            "a new target is eligible"
        );
        attempts.sent(12, now);
        attempts.disconnected();
        assert!(
            !attempts.request(10, "0.2.15-old", "0.2.16-new", false),
            "reconnection cannot re-arm the same daemon/target"
        );
        assert!(
            attempts.request(20, "0.2.16-new", "0.2.16-new", true),
            "preserve explicit same-version replacement"
        );
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use super::*;
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::mpsc::{self, Receiver};

    struct Server {
        _dir: tempfile::TempDir,
        listener: UnixListener,
        socket: String,
    }

    struct Peer {
        stream: UnixStream,
        received: Receiver<ClientMsg>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir
                .path()
                .join("daemon.sock")
                .to_string_lossy()
                .into_owned();
            Self {
                listener: UnixListener::bind(&socket).unwrap(),
                socket,
                _dir: dir,
            }
        }

        fn listen(&self, pid: u32, build: &str) -> impl FnOnce() -> Peer {
            let listener = self.listener.try_clone().unwrap();
            let build = build.to_owned();
            let (ready, stream) = mpsc::channel();
            let (messages, received) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                let (mut peer, _) = listener.accept().unwrap();
                assert!(matches!(
                    read_msg::<_, ClientMsg>(&mut peer).unwrap(),
                    Some(ClientMsg::Hello { .. })
                ));
                write_msg(
                    &mut peer,
                    &ServerMsg::Hello {
                        proto: PROTO_VERSION,
                        build,
                        pid,
                        can_upgrade: true,
                    },
                )
                .unwrap();
                ready.send(peer.try_clone().unwrap()).unwrap();
                while let Ok(Some(message)) = read_msg::<_, ClientMsg>(&mut peer) {
                    if messages.send(message).is_err() {
                        break;
                    }
                }
            });
            move || Peer {
                stream: stream.recv_timeout(Duration::from_secs(3)).unwrap(),
                received,
                thread: Some(thread),
            }
        }

        fn connect(&self) -> (Conn, Peer) {
            let ready = self.listen(101, "0.2.15-old");
            let mut conn = Conn::offline(egui::Context::default());
            conn.socket = self.socket.clone();
            conn.exe = self._dir.path().join("fixture-kiln");
            conn.target_build = "0.2.16-new".into();
            conn.screens.insert(
                7,
                Screen {
                    cols: 80,
                    rows: 24,
                    version: 42,
                    ..Default::default()
                },
            );
            conn.try_connect();
            (conn, ready())
        }
    }

    impl Peer {
        fn send(&mut self, message: ServerMsg) {
            write_msg(&mut self.stream, &message).unwrap();
        }

        fn upgrade(&self) -> u32 {
            loop {
                if let ClientMsg::Upgrade { req, .. } =
                    self.received.recv_timeout(Duration::from_secs(3)).unwrap()
                {
                    return req;
                }
            }
        }

        fn frame(&mut self) {
            self.send(ServerMsg::Frame(Frame {
                session: 7,
                cols: 80,
                rows: 24,
                full: false,
                lines: vec![],
                cursor: None,
                mode: 0,
                display_offset: 0,
                history: 0,
                images: vec![],
            }));
        }

        fn no_upgrade_before_barrier(&self, conn: &Conn) {
            let barrier = conn.next_req();
            conn.send(ClientMsg::Ping { req: barrier });
            loop {
                match self.received.recv_timeout(Duration::from_secs(3)).unwrap() {
                    ClientMsg::Upgrade { .. } => panic!("unexpected repeated upgrade request"),
                    ClientMsg::Ping { req } if req == barrier => return,
                    _ => {}
                }
            }
        }
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            let _ = self.stream.shutdown(Shutdown::Both);
            if let Some(thread) = self.thread.take() {
                thread.join().unwrap();
            }
        }
    }

    fn pump_until(conn: &mut Conn, condition: impl Fn(&Conn) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            conn.pump();
            if condition(conn) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "GUI did not handle the socket event"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn startup_rejection_keeps_connection_screen_and_server_error() {
        let server = Server::new();
        let (mut conn, mut peer) = server.connect();
        assert!(
            conn.is_connected(),
            "queuing an upgrade must retain the GUI connection"
        );
        assert!(
            !conn
                .events
                .iter()
                .any(|e| matches!(e, ConnEvent::Upgrading))
        );
        let req = peer.upgrade();
        assert_ne!(req, 0);
        peer.send(ServerMsg::Error {
            req,
            message: "upgrade failed: fixture refusal".into(),
        });
        pump_until(&mut conn, |c| {
            c.events
                .iter()
                .any(|e| matches!(e, ConnEvent::Error { req:r, .. } if *r == req))
        });
        assert!(conn.client.is_some() && conn.is_connected());
        assert_eq!(conn.screens[&7].version, 42);
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Connected))
                .count(),
            1
        );
        assert!(
            !conn
                .events
                .iter()
                .any(|e| matches!(e, ConnEvent::Upgrading))
        );
        for _ in 0..40 {
            conn.pump();
        }
        peer.no_upgrade_before_barrier(&conn);
    }

    #[test]
    fn delayed_acknowledgement_keeps_live_frames_and_emits_upgrading_once() {
        let server = Server::new();
        let (mut conn, mut peer) = server.connect();
        assert!(
            conn.is_connected(),
            "waiting for acknowledgement is still connected"
        );
        peer.upgrade();
        for _ in 0..40 {
            conn.pump();
        }
        assert!(
            !conn
                .events
                .iter()
                .any(|e| matches!(e, ConnEvent::Upgrading))
        );
        peer.send(ServerMsg::Upgrading);
        peer.send(ServerMsg::Upgrading);
        peer.frame();
        pump_until(&mut conn, |c| c.screens[&7].version == 43);
        assert!(conn.client.is_some() && conn.is_connected());
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Upgrading))
                .count(),
            1
        );
        peer.no_upgrade_before_barrier(&conn);
    }

    #[test]
    fn failure_after_acknowledgement_is_reported_once_without_disconnect() {
        let server = Server::new();
        let (mut conn, mut peer) = server.connect();
        assert!(conn.is_connected());
        let req = peer.upgrade();
        peer.send(ServerMsg::Upgrading);
        for _ in 0..2 {
            peer.send(ServerMsg::Error {
                req,
                message: "upgrade failed: fixture pause timeout".into(),
            });
        }
        peer.frame();
        pump_until(&mut conn, |c| c.screens[&7].version == 43);
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Error { req:r, .. } if *r == req))
                .count(),
            1
        );
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Upgrading))
                .count(),
            1
        );
        assert_eq!(conn.screens[&7].version, 43);
        assert!(conn.is_connected());
        peer.no_upgrade_before_barrier(&conn);
    }

    #[test]
    fn manual_retry_uses_a_fresh_request_and_ignores_the_previous_error() {
        let server = Server::new();
        let (mut conn, mut peer) = server.connect();
        let first = peer.upgrade();
        peer.send(ServerMsg::Error {
            req: first,
            message: "upgrade failed: fixture refusal".into(),
        });
        peer.frame();
        pump_until(&mut conn, |c| c.screens[&7].version == 43);
        assert!(conn.upgrade_daemon());
        let retry = peer.upgrade();
        assert_ne!(first, retry);
        assert_ne!(retry, 0);
        assert!(
            !conn.upgrade_daemon(),
            "repeated clicks must not queue another request"
        );
        peer.send(ServerMsg::Error {
            req: first,
            message: "late duplicate".into(),
        });
        peer.send(ServerMsg::Error {
            req: 9999,
            message: "unrelated request".into(),
        });
        peer.frame();
        pump_until(&mut conn, |c| c.screens[&7].version == 44);
        assert_eq!(conn.upgrades.pending.as_ref().unwrap().req, retry);
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Error { req, .. } if *req == first))
                .count(),
            1
        );
        assert!(
            conn.events
                .iter()
                .any(|e| matches!(e, ConnEvent::Error { req: 9999, .. }))
        );
        assert!(conn.is_connected());
        peer.no_upgrade_before_barrier(&conn);
    }

    #[test]
    fn incomplete_upgrade_times_out_once_and_keeps_the_same_socket_for_manual_retry() {
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let server = Server::new();
            let (mut conn, peer) = server.connect();
            let req = peer.upgrade();
            conn.upgrades.pending.as_mut().unwrap().started =
                Instant::now() - Duration::from_secs(31);
            conn.pump();
            for _ in 0..40 {
                conn.pump();
            }
            assert!(conn.is_connected());
            assert_eq!(conn.screens[&7].version, 42);
            assert_eq!(
                conn.events
                    .iter()
                    .filter(|e| matches!(e, ConnEvent::Error { req:r, .. } if *r == req))
                    .count(),
                1
            );
            assert!(
                !conn
                    .events
                    .iter()
                    .any(|e| matches!(e, ConnEvent::Upgrading))
            );
            assert!(conn.events.iter().any(|e| matches!(e, ConnEvent::Error { req:r, message }
            if *r == req && message == "데몬 교체를 완료하지 못했습니다. 설정에서 다시 시도하세요.")),
            "the Korean UI must receive a localized timeout message");
            peer.no_upgrade_before_barrier(&conn);
            assert!(conn.upgrade_daemon());
            assert_ne!(peer.upgrade(), req);
        });
    }

    #[test]
    fn early_eof_reconnecting_to_the_unchanged_daemon_does_not_retry_upgrade() {
        let server = Server::new();
        let (mut conn, peer) = server.connect();
        peer.upgrade();
        let next = server.listen(101, "0.2.15-old");
        drop(peer);
        conn.last_attempt = Some(Instant::now() - Duration::from_secs(1));
        pump_until(&mut conn, |c| {
            c.is_connected()
                && c.events
                    .iter()
                    .filter(|e| matches!(e, ConnEvent::Connected))
                    .count()
                    == 2
        });
        let peer = next();
        for _ in 0..40 {
            conn.pump();
        }
        assert_eq!(conn.daemon_pid, 101);
        assert_eq!(conn.screens[&7].version, 42);
        assert!(
            !conn
                .events
                .iter()
                .any(|e| matches!(e, ConnEvent::Upgrading))
        );
        peer.no_upgrade_before_barrier(&conn);
    }

    #[test]
    fn successful_upgrade_reconnects_only_after_real_eof() {
        let server = Server::new();
        let (mut conn, mut peer) = server.connect();
        assert!(conn.is_connected());
        peer.upgrade();
        peer.send(ServerMsg::Upgrading);
        pump_until(&mut conn, |c| {
            c.events.iter().any(|e| matches!(e, ConnEvent::Upgrading))
        });
        assert_eq!(conn.daemon_pid, 101);
        assert!(conn.is_connected());
        let next = server.listen(202, "0.2.16-new");
        drop(peer);
        conn.last_attempt = Some(Instant::now() - Duration::from_secs(1));
        pump_until(&mut conn, |c| c.daemon_pid == 202 && c.is_connected());
        let peer = next();
        assert_eq!(conn.screens[&7].version, 42);
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Upgrading))
                .count(),
            1
        );
        assert_eq!(
            conn.events
                .iter()
                .filter(|e| matches!(e, ConnEvent::Connected))
                .count(),
            2
        );
        assert!(
            !conn
                .events
                .iter()
                .any(|e| matches!(e, ConnEvent::Error { .. }))
        );
        peer.no_upgrade_before_barrier(&conn);
    }
}
