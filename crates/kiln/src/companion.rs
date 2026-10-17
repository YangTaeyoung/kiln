//! Shared by the menu-bar and notification-area companions: the session summary, the
//! unread badge record and stopping reviewed background sessions.
use kiln_common::i18n::tr;
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg, ServerMsg, SessionId, SessionInfo};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Connecting,
    Connected { running: usize, attention: usize },
    Disconnected,
}
impl Status {
    /// The badge shown next to the app symbol: a dot for attention or unread notifications.
    pub(crate) fn title(&self) -> String {
        badge_title(self.attention(), persisted_unread())
    }
    pub(crate) fn attention(&self) -> bool {
        matches!(self, Self::Connected { attention, .. } if *attention > 0)
    }
    pub(crate) fn running(&self) -> usize {
        match self {
            Self::Connected { running, .. } => *running,
            _ => 0,
        }
    }
    pub(crate) fn description(&self) -> String {
        match self {
            Self::Connecting => tr("Kiln에 연결하는 중…").into(),
            Self::Connected { running: 0, attention: 0 } => tr("Kiln 실행 중 · 활성 세션 없음").into(),
            Self::Connected { running: 0, attention } => kiln_common::trf!("활성 세션 없음 · 확인 필요 {attention}개"),
            Self::Connected { running, attention: 0 } => kiln_common::trf!("백그라운드 세션 {running}개 실행 중"),
            Self::Connected { running, attention } => kiln_common::trf!("세션 {running}개 실행 중 · 확인 필요 {attention}개"),
            Self::Disconnected => tr("Kiln 연결이 끊겼습니다").into(),
        }
    }
}

pub(crate) fn badge_title(attention: bool, unread: usize) -> String {
    if attention || unread > 0 { "●".into() } else { String::new() }
}

/// A tiny shared badge record contains only the unread count, never notification text.
pub fn publish_unread(count: usize) {
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    if LAST.load(Ordering::Relaxed) != count && write_unread(&kiln_common::paths::config_file("menu-bar-unread"), count).is_ok() {
        LAST.store(count, Ordering::Relaxed);
    }
}

pub(crate) fn persisted_unread() -> usize {
    read_unread(&kiln_common::paths::config_file("menu-bar-unread"))
}

fn write_unread(path: &Path, count: usize) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, count.to_string())?;
    std::fs::rename(&temp, path)
}

fn read_unread(path: &Path) -> usize {
    std::fs::read_to_string(path).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Polls the daemon every two seconds and keeps `status` current; after ten failures in a
/// row it reports [`Status::Disconnected`] and returns.
pub(crate) fn watch(socket: String, status: Arc<Mutex<Status>>) {
    let mut failures = 0;
    let mut client: Option<Client> = None;
    loop {
        if client.is_none() {
            client = Client::connect(&socket, None).ok();
        }
        let snapshot = client.as_ref().and_then(|c| c.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(2)).ok());
        let next = if let Some(ServerMsg::Sessions { sessions, .. }) = snapshot {
            failures = 0;
            let running = sessions.iter().filter(|s| s.exited.is_none()).count();
            let attention = sessions.iter().filter(|s| s.attention).count();
            Status::Connected { running, attention }
        } else {
            client = None;
            failures += 1;
            if failures >= 10 { Status::Disconnected } else { Status::Connecting }
        };
        let done = next == Status::Disconnected;
        *status.lock().unwrap_or_else(|e| e.into_inner()) = next;
        if done {
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub(crate) fn list_sessions(client: &Client) -> anyhow::Result<Vec<SessionInfo>> {
    match client.request(|req| ClientMsg::ListSessions { req }, Duration::from_secs(3))? {
        ServerMsg::Sessions { sessions, .. } => Ok(sessions),
        _ => anyhow::bail!("{}", tr("세션 목록을 확인하지 못했습니다. 다시 시도하세요.")),
    }
}

/// The running sessions of the default daemon, with the connection that listed them.
pub(crate) fn running_sessions() -> anyhow::Result<(Client, Vec<SessionInfo>)> {
    // Connecting never starts, upgrades, or shuts down a daemon.
    let client = Client::connect(&kiln_proto::socket_name(), None)?;
    let sessions = list_sessions(&client)?.into_iter().filter(|s| s.exited.is_none()).collect();
    Ok((client, sessions))
}

pub(crate) fn reviewed_session_ids(reviewed: &[SessionInfo], current: &[SessionInfo]) -> Vec<SessionId> {
    reviewed
        .iter()
        .filter(|old| current.iter().any(|now| now.exited.is_none() && now.id == old.id && now.pid == old.pid && now.created_unix == old.created_unix))
        .map(|s| s.id)
        .collect()
}

pub(crate) fn stop_reviewed(client: &Client, reviewed: &[SessionInfo]) -> anyhow::Result<()> {
    // Use the original connection; never reconnect to a restarted daemon whose
    // IDs could be reused. New sessions created during confirmation are excluded.
    let ids = reviewed_session_ids(reviewed, &list_sessions(client)?);
    for &session in &ids {
        client.send(ClientMsg::Kill { session });
    }
    let remaining = list_sessions(client)?;
    anyhow::ensure!(
        !remaining.iter().any(|s| ids.contains(&s.id) && s.exited.is_none()),
        "{}",
        tr("일부 세션의 종료를 확인하지 못했습니다. 세션 목록을 확인하세요.")
    );
    Ok(())
}

/// The heading and explanation of the confirmation before stopping `sessions`.
pub(crate) fn stop_message(sessions: &[SessionInfo]) -> (String, String) {
    let names = sessions
        .iter()
        .take(8)
        .map(|s| {
            let label = s.name.as_deref().filter(|s| !s.is_empty()).unwrap_or(&s.title);
            let label: String = label.chars().filter(|c| !c.is_control()).take(80).collect();
            format!("• #{} {}", s.id, label)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let extra = if sessions.len() > 8 { kiln_common::trf!("\n외 {}개", sessions.len() - 8) } else { String::new() };
    let body = kiln_common::trf!("모든 워크스페이스의 터미널 세션 {}개를 종료합니다. 현재 창에 표시된 세션도 포함됩니다. 실행 중인 셸, 에이전트, 빌드와 서버가 중단되며 저장하지 않은 터미널 작업은 사라질 수 있습니다.\n\n{}{}\n\nKiln 창과 세션 서비스는 계속 실행됩니다.", sessions.len(), names, extra);
    (kiln_common::trf!("백그라운드 세션 {}개를 종료할까요?", sessions.len()), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(id: u64) -> SessionInfo {
        SessionInfo { id, pid: id as u32 + 100, created_unix: id + 1000, ..Default::default() }
    }
    #[test]
    fn termination_excludes_new_exited_and_reused_sessions() {
        let reviewed = vec![session(1), session(2), session(3), session(4)];
        let mut exited = session(2);
        exited.exited = Some(0);
        let mut reused = session(3);
        reused.created_unix += 1;
        let mut changed_pid = session(4);
        changed_pid.pid += 1;
        assert_eq!(reviewed_session_ids(&reviewed, &[session(1), exited, reused, changed_pid, session(5)]), vec![1]);
    }
    #[cfg(unix)]
    #[test]
    fn confirmed_stop_sends_only_reviewed_kills_and_keeps_connection_alive() {
        use kiln_proto::{PROTO_VERSION, read_msg, write_msg};
        use std::os::unix::net::UnixListener;
        let dir = tempfile::Builder::new().prefix("kiln-menu-test-").tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("d.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            assert!(matches!(read_msg::<_, ClientMsg>(&mut stream).unwrap(), Some(ClientMsg::Hello { .. })));
            write_msg(&mut stream, &ServerMsg::Hello { proto: PROTO_VERSION, build: "test".into(), pid: 1, can_upgrade: false }).unwrap();
            let mut sessions = vec![session(1), session(2)];
            let mut killed = vec![];
            loop {
                match read_msg::<_, ClientMsg>(&mut stream).unwrap().unwrap() {
                    ClientMsg::ListSessions { req } => write_msg(&mut stream, &ServerMsg::Sessions { req, sessions: sessions.clone() }).unwrap(),
                    ClientMsg::Kill { session } => {
                        killed.push(session);
                        sessions.retain(|s| s.id != session);
                    }
                    ClientMsg::Ping { req } => {
                        write_msg(&mut stream, &ServerMsg::Pong { req }).unwrap();
                        assert_eq!(killed, vec![1]);
                        assert_eq!(sessions, vec![session(2)]);
                        break;
                    }
                    unexpected => panic!("must not shut down or create sessions: {unexpected:?}"),
                }
            }
        });
        let client = Client::connect(socket.to_str().unwrap(), None).unwrap();
        // This fixture is a protocol peer only; no real daemon or PTY exists.
        stop_reviewed(&client, &[session(1)]).unwrap();
        assert!(matches!(client.request(|req| ClientMsg::Ping { req }, Duration::from_secs(3)).unwrap(), ServerMsg::Pong { .. }));
        server.join().unwrap();
    }
    #[test]
    fn summary_distinguishes_connecting_idle_running_and_attention() {
        assert_eq!(Status::Connecting.description(), "Kiln에 연결하는 중…");
        assert_eq!(Status::Connected { running: 0, attention: 0 }.description(), "Kiln 실행 중 · 활성 세션 없음");
        assert_eq!(Status::Connected { running: 0, attention: 2 }.description(), "활성 세션 없음 · 확인 필요 2개");
        assert_eq!(Status::Connected { running: 2, attention: 1 }.description(), "세션 2개 실행 중 · 확인 필요 1개");
        assert_eq!(badge_title(false, 0), "");
        assert_eq!(badge_title(true, 0), "●");
        assert_eq!(badge_title(false, 1), "●");
    }
    #[test]
    fn unread_record_round_trips_and_reads_missing_as_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menu-bar-unread");
        assert_eq!(read_unread(&path), 0);
        write_unread(&path, 3).unwrap();
        assert_eq!(read_unread(&path), 3);
    }
    #[test]
    fn stop_confirmation_lists_at_most_eight_sessions() {
        let sessions: Vec<_> = (1..=10).map(|id| SessionInfo { title: format!("task\u{7}{id}"), ..session(id) }).collect();
        let (title, body) = stop_message(&sessions);
        assert_eq!(title, "백그라운드 세션 10개를 종료할까요?");
        assert!(body.contains("• #8 task8") && !body.contains("#9 ") && body.contains("외 2개"));
    }
}
