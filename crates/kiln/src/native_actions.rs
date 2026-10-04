//! One-shot settings requests from the menu-bar companion to an existing or new GUI.
//! Requests contain only a timestamp, expire quickly, and use the isolated config path.
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_AGE: Duration = Duration::from_secs(60);

pub fn request_settings() -> io::Result<()> {
    write_request(
        &kiln_common::paths::config_file("open-settings"),
        SystemTime::now(),
    )
}

pub fn request_settings_and_launch(launch: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    request_and_launch(
        &kiln_common::paths::config_file("open-settings"),
        SystemTime::now(),
        launch,
    )
}

pub fn take_settings_request() -> bool {
    take_request(
        &kiln_common::paths::config_file("open-settings"),
        SystemTime::now(),
    )
}

pub fn cancel_settings_request() {
    let _ = fs::remove_file(kiln_common::paths::config_file("open-settings"));
}

fn unique_suffix() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn write_request(path: &Path, now: SystemTime) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", unique_suffix()));
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis();
    let result = fs::write(&temp, timestamp.to_string()).and_then(|_| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn request_and_launch(
    path: &Path,
    now: SystemTime,
    launch: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    write_request(path, now)?;
    if let Err(error) = launch() {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn take_request(path: &Path, now: SystemTime) -> bool {
    // Rename claims the exact published request before reading. Concurrent GUI
    // instances cannot both consume it, and a subsequent click is not deleted.
    let claimed = path.with_extension(format!("{}.claimed", unique_suffix()));
    if fs::rename(path, &claimed).is_err() {
        return false;
    }
    let request = fs::read_to_string(&claimed).ok();
    let _ = fs::remove_file(&claimed);
    request
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|ms| UNIX_EPOCH.checked_add(Duration::from_millis(ms)))
        .and_then(|sent| now.duration_since(sent).ok())
        .is_some_and(|age| age <= MAX_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_survives_launch_but_is_consumed_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/open-settings");
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        write_request(&path, now).unwrap();
        assert!(take_request(&path, now + Duration::from_secs(20)));
        assert!(!take_request(&path, now + Duration::from_secs(21)));
        write_request(&path, now + Duration::from_secs(22)).unwrap();
        assert!(take_request(&path, now + Duration::from_secs(22)));
    }
    #[test]
    fn stale_future_and_malformed_requests_do_not_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open-settings");
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        for sent in [now - Duration::from_secs(61), now + Duration::from_secs(1)] {
            write_request(&path, sent).unwrap();
            assert!(!take_request(&path, now));
            assert!(!path.exists());
        }
        fs::write(&path, "not a command").unwrap();
        assert!(!take_request(&path, now));
        assert!(!path.exists());
    }
    #[test]
    fn failed_launch_returns_error_and_removes_pending_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open-settings");
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let error =
            request_and_launch(&path, now, || Err(io::Error::other("launch failed"))).unwrap_err();
        assert_eq!(error.to_string(), "launch failed");
        assert!(!take_request(&path, now));
    }
    #[test]
    fn simultaneous_consumers_claim_request_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open-settings");
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        write_request(&path, now).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    take_request(&path, now)
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|&claimed| claimed)
                .count(),
            1
        );
    }
}
