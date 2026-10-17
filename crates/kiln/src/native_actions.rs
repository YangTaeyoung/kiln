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
    write_payload(path, now, "")
}

/// Publishes a request: its timestamp, then `payload` on the next line when there is one.
fn write_payload(path: &Path, now: SystemTime, payload: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", unique_suffix()));
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis();
    let text = if payload.is_empty() { timestamp.to_string() } else { format!("{timestamp}\n{payload}") };
    let result = fs::write(&temp, text).and_then(|_| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn write_folder_request(path: &Path, now: SystemTime, folder: &Path) -> io::Result<()> {
    let folder = folder.to_str().ok_or_else(|| io::Error::other("folder is not Unicode"))?;
    write_payload(path, now, folder)
}

fn take_folder_request_at(path: &Path, now: SystemTime) -> Option<std::path::PathBuf> {
    take_payload(path, now).filter(|folder| !folder.is_empty()).map(std::path::PathBuf::from)
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

/// Claims the published request and returns its text; concurrent consumers cannot both
/// claim it, and a later click is not deleted.
#[cfg(not(windows))]
fn claim(path: &Path) -> Option<String> {
    // Rename moves the exact published request out of the way before reading.
    let claimed = path.with_extension(format!("{}.claimed", unique_suffix()));
    fs::rename(path, &claimed).ok()?;
    let request = fs::read_to_string(&claimed).ok();
    let _ = fs::remove_file(&claimed);
    request
}

#[cfg(windows)]
fn claim(path: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    // A handle with no sharing is the only one open; the file goes when it closes.
    const DELETE: u32 = 0x0001_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    let mut file = fs::OpenOptions::new()
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(path)
        .ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

fn take_request(path: &Path, now: SystemTime) -> bool {
    take_payload(path, now).is_some()
}

/// The payload of a fresh request, claimed once; stale, future and malformed requests are
/// dropped.
fn take_payload(path: &Path, now: SystemTime) -> Option<String> {
    let text = claim(path)?;
    let (stamp, payload) = text.split_once('\n').unwrap_or((text.as_str(), ""));
    let sent = UNIX_EPOCH.checked_add(Duration::from_millis(stamp.parse::<u64>().ok()?))?;
    let age = now.duration_since(sent).ok()?;
    (age <= MAX_AGE).then(|| payload.to_owned())
}

/// Asks the running GUI to open `folder`.
pub fn request_folder(folder: &Path) -> io::Result<()> {
    write_folder_request(&kiln_common::paths::config_file("open-folder"), SystemTime::now(), folder)
}

/// The folder a later start asked the running GUI to open, once.
pub fn take_folder_request() -> Option<std::path::PathBuf> {
    take_folder_request_at(&kiln_common::paths::config_file("open-folder"), SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_folder_request_carries_its_path_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("open-folder");
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let folder = Path::new("C:/작업 공간/kiln");
        write_folder_request(&path, now, folder).unwrap();
        assert_eq!(take_folder_request_at(&path, now + Duration::from_secs(5)).as_deref(), Some(folder));
        assert_eq!(take_folder_request_at(&path, now + Duration::from_secs(6)), None);
        write_folder_request(&path, now, folder).unwrap();
        assert_eq!(take_folder_request_at(&path, now + Duration::from_secs(61)), None);
    }
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
