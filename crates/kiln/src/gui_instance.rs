//! One Kiln window per daemon on Windows: a second start hands its folder to the running
//! GUI and brings that window forward.
use std::path::Path;

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

/// When another Kiln GUI runs for this daemon, asks it to open `folder` (if any), brings it
/// forward and returns true; otherwise takes the lock and returns false.
pub fn hand_off(folder: Option<&Path>) -> bool {
    let socket = kiln_proto::socket_name();
    let hash = socket.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    if !another_runs(&format!("Local\\kiln-gui-{hash:016x}")) {
        return false;
    }
    if let Some(folder) = folder.and_then(|f| std::path::absolute(f).ok()) {
        let _ = crate::native_actions::request_folder(&folder);
    }
    if let Some(window) = crate::tray_windows::gui_window(std::env::current_exe().ok().as_deref()) {
        crate::tray_windows::activate(window);
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_second_start_finds_the_first() {
        let name = format!("Local\\kiln-gui-test-{}", std::process::id());
        assert!(!super::another_runs(&name));
        assert!(super::another_runs(&name));
    }
}
