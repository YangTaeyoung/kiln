//! One Kiln window per daemon on Windows: a second start hands its folder to the running
//! GUI and brings that window forward.
use std::path::Path;
use std::time::{Duration, Instant};

/// Whether a Kiln GUI already holds the lock named `name`. The first holder in a process keeps
/// it until the process ends.
fn another_runs(name: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: the first handle stays open for the life of the process; a duplicate is closed.
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if mutex.is_null() {
            return false;
        }
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        if existed {
            CloseHandle(mutex);
        }
        existed
    }
}

fn has_update_backup(executable: &Path) -> bool {
    let Some(dir) = executable.parent() else {
        return false;
    };
    if !executable
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("Kiln.exe"))
        || !dir.join("bin").join("kiln.exe").is_file()
    {
        return false;
    }
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let matches = name
                .to_str()
                .is_some_and(|name| name == "Kiln.exe.old" || name.starts_with("Kiln.exe.old-"));
            matches && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
    })
}

fn claim_after_update(name: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !another_runs(name) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// When another Kiln GUI runs for this daemon, asks it to open `folder` (if any), brings it
/// forward and returns true; otherwise takes the lock and returns false.
pub fn hand_off(folder: Option<&Path>) -> bool {
    let socket = kiln_proto::socket_name();
    let hash = socket.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    let name = format!("Local\\kiln-gui-{hash:016x}");
    if !another_runs(&name) {
        return false;
    }
    let exe = std::env::current_exe().ok();
    // The updater starts the replacement before the old GUI releases its
    // process-lifetime marker and leaves the renamed executable beside it.
    if exe.as_deref().is_some_and(has_update_backup) && claim_after_update(&name) {
        return false;
    }
    if let Some(folder) = folder.and_then(|f| std::path::absolute(f).ok()) {
        let _ = crate::native_actions::request_folder(&folder);
    }
    if let Some(window) = crate::tray_windows::gui_window(exe.as_deref()) {
        crate::tray_windows::activate(window);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_start_finds_the_first() {
        let name = format!("Local\\kiln-gui-test-{}", std::process::id());
        assert!(!super::another_runs(&name));
        assert!(super::another_runs(&name));
    }

    #[test]
    fn updater_relaunch_waits_for_the_exiting_gui() {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = format!("Local\\kiln-gui-update-test-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        assert!(!handle.is_null());
        // SAFETY: CreateMutexW returned a valid, uniquely owned handle.
        let previous = unsafe { OwnedHandle::from_raw_handle(handle) };
        assert!(another_runs(&name));
        let exiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(previous);
        });
        assert!(claim_after_update(&name));
        exiting.join().unwrap();
        assert!(another_runs(&name));
    }

    #[test]
    fn only_the_installed_gui_looks_for_its_updater_backup() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("Kiln.exe");
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin").join("kiln.exe"), "").unwrap();
        std::fs::write(dir.path().join("LICENSE.txt.old"), "").unwrap();
        assert!(!has_update_backup(&executable));
        std::fs::write(dir.path().join("Kiln.exe.old-1"), "").unwrap();
        assert!(has_update_backup(&executable));
        assert!(!has_update_backup(&dir.path().join("kiln-app.exe")));
    }
}
