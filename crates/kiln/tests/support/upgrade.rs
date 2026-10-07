use crossbeam_channel::RecvTimeoutError;
use kiln_daemon::client::Client;
use kiln_proto::ServerMsg;
use std::time::{Duration, Instant};

/// A same-PID/same-binary Hello can still come from the pre-upgrade daemon.
/// Observe the original connection closing before testing the restored owner.
pub fn disconnected(client: &Client, req: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match client.rx.recv_timeout(Duration::from_millis(100)) {
            Err(RecvTimeoutError::Disconnected) => return,
            Ok(ServerMsg::Error {
                req: failed,
                message,
            }) if failed == req => {
                panic!("upgrade {req} failed before disconnect: {message}");
            }
            Err(RecvTimeoutError::Timeout) | Ok(_) => {}
        }
        assert!(
            Instant::now() < deadline,
            "original connection did not close for upgrade {req}"
        );
    }
}
