#![cfg(target_os = "macos")]

use std::os::unix::{ffi::OsStringExt, io::AsRawFd, process::CommandExt};
use std::process::Command;

unsafe extern "C" {
    fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
}

// This subprocess test exercises the real startup replacement, not a spawn mock.
// It catches inherited responsibility, changed PIDs/arguments/environment, closed
// restore FDs, replacement loops and loss of child-exit ownership.
#[test]
fn background_owner_replaces_itself_without_losing_restore_state() {
    const FIXTURE: &str = "KILN_RESPONSIBILITY_TEST";
    if std::env::var_os(FIXTURE).is_some() {
        let snapshot_path = std::path::PathBuf::from(std::env::var_os(FIXTURE).unwrap());
        if !snapshot_path.exists() {
            let keeper = Command::new("/bin/sh")
                .args(["-c", "sleep 0.4; exit 42"])
                .spawn()
                .unwrap();
            let snapshot = serde_json::json!({
                "pid": std::process::id(), "keeper": keeper.id(),
                "args": std::env::args_os().map(|s| s.into_vec()).collect::<Vec<_>>()
            });
            std::fs::write(&snapshot_path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
        }
        let before: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        kiln_daemon::macos_responsibility::ensure_own_responsibility().unwrap();
        // Calling twice must not replace again, including after native upgrades.
        kiln_daemon::macos_responsibility::ensure_own_responsibility().unwrap();
        assert_eq!(std::process::id(), before["pid"].as_u64().unwrap() as u32);
        assert_eq!(
            serde_json::to_value(
                std::env::args_os()
                    .map(|s| s.into_vec())
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            before["args"]
        );
        assert_eq!(
            std::env::var_os("KILN_TEST_BYTES").unwrap().into_vec(),
            b"a\xff=b"
        );
        let fd: i32 = std::env::var("KILN_TEST_RESTORE_FD")
            .unwrap()
            .parse()
            .unwrap();
        let mut bytes = [0; 7];
        assert_eq!(
            unsafe { libc::pread(fd, bytes.as_mut_ptr().cast(), 7, 0) },
            7
        );
        assert_eq!(&bytes, b"restore");
        // A child from the previous image must still belong to this same PID.
        let keeper = before["keeper"].as_u64().unwrap() as i32;
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(keeper, &mut status, 0) }, keeper);
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 42);
        let pid = std::process::id() as i32;
        assert_eq!(
            unsafe { responsibility_get_pid_responsible_for_pid(pid) },
            pid
        );
        let mut child = Command::new("/bin/sleep").arg("0.1").spawn().unwrap();
        assert_eq!(
            unsafe { responsibility_get_pid_responsible_for_pid(child.id() as i32) },
            pid
        );
        assert!(child.wait().unwrap().success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("restore-state"), b"restore").unwrap();
    // Use a readable descriptor; the child pre_exec explicitly preserves it.
    let file = std::fs::File::open(root.path().join("restore-state")).unwrap();
    let fd = file.as_raw_fd();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "background_owner_replaces_itself_without_losing_restore_state",
            "--nocapture",
        ])
        .env(FIXTURE, root.path().join("snapshot.json"))
        .env(
            "KILN_TEST_BYTES",
            std::ffi::OsString::from_vec(b"a\xff=b".to_vec()),
        )
        .env("KILN_TEST_RESTORE_FD", fd.to_string())
        .env("KILN_CONFIG_DIR", root.path().join("config"))
        .env("KILN_SOCKET", root.path().join("unused.sock"));
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    assert!(command.status().unwrap().success());
}
