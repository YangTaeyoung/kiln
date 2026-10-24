//! Retry per daemon identity, not once for the lifetime of the GUI.
use super::*;
#[derive(Default)]
pub(super) struct Attempts { last: Option<(u32, String, String, Instant)> }
impl Attempts {
    pub fn request(&mut self, now: Instant, pid: u32, server: &str, target: &str) -> bool {
        if server == target || kiln_daemon::client::is_downgrade(server, target) { return false; }
        if self.last.as_ref().is_some_and(|(old_pid, old_server, old_target, at)| {
            *old_pid == pid && old_server == server && old_target == target && now.saturating_duration_since(*at) < Duration::from_secs(30)
        }) { return false; }
        self.last = Some((pid, server.into(), target.into(), now));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_replaced_daemon_is_upgraded_even_after_an_earlier_attempt() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        assert!(attempts.request(now, 10, "0.2.8-old", "0.2.11-new"));
        assert!(!attempts.request(now, 10, "0.2.8-old", "0.2.11-new"));
        assert!(attempts.request(now, 11, "0.2.8-old", "0.2.11-new"));
        assert!(!attempts.request(now, 11, "0.2.12-new", "0.2.11-new"));
    }
    #[test]
    fn failed_same_daemon_attempts_back_off_and_can_retry() {
        let mut attempts = Attempts::default();
        let now = Instant::now();
        assert!(attempts.request(now, 10, "0.2.8-old", "0.2.11-new"));
        assert!(!attempts.request(now + Duration::from_secs(10), 10, "0.2.8-old", "0.2.11-new"));
        assert!(attempts.request(now + Duration::from_secs(31), 10, "0.2.8-old", "0.2.11-new"));
        assert!(!attempts.request(now, 11, "0.2.11-new", "0.2.11-new"));
    }
}
