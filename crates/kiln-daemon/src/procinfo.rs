//! 프로세스 이름과 작업 디렉토리 조회.

#[cfg(windows)]
#[path = "procinfo_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::{cmdline, cwd, foreground_of, is_window_application, name, open_files, parent_of};

/// Resolve the actual foreground job through Kiro's nested PTY. tcgetpgrp
/// reports the outer bridge's process group, not its inner shell's job.
/// This is read-only metadata: never use this PID for PTY ownership or signals.
pub fn foreground_pid(group: u32) -> Option<u32> {
    #[cfg(target_os = "macos")]
    { mac_foreground::resolve(group) }
    #[cfg(not(target_os = "macos"))]
    { Some(group) }
}

#[cfg(target_os = "macos")]
mod mac_foreground {
    // Apple's definitions: bsd/sys/proc_info.h and
    // libsyscall/wrappers/libproc/libproc.h in apple-oss-distributions/xnu.
    const PGRP: u32 = 2;
    const PPID: u32 = 6;

    fn info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let mut value: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&value) as i32;
        // SAFETY: correctly sized output struct, read-only kernel query.
        let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, (&mut value as *mut libc::proc_bsdinfo).cast(), size) };
        (n == size && value.pbi_pid == pid && value.pbi_status != 5).then_some(value)
    }

    fn list(kind: u32, id: u32) -> Vec<u32> {
        // Bounded and retried for process creation between sizing and reading.
        for capacity in [64, 1024] {
            let mut pids = vec![0u32; capacity];
            let size = (capacity * std::mem::size_of::<u32>()) as i32;
            // SAFETY: buffer size matches the allocated PID array.
            let bytes = unsafe { libc::proc_listpids(kind, id, pids.as_mut_ptr().cast(), size) };
            if bytes <= 0 { return Vec::new(); }
            if bytes >= size { continue; }
            pids.truncate(bytes as usize / std::mem::size_of::<u32>());
            pids.retain(|pid| *pid > 0);
            return pids;
        }
        Vec::new()
    }

    fn is_agent(pid: u32) -> bool {
        matches!(super::display_name(pid).as_deref(), Some("codex" | "claude" | "agy"))
    }

    fn is_shell(pid: u32) -> bool {
        matches!(super::display_name(pid).as_deref(), Some("zsh" | "bash" | "sh" | "fish" | "dash" | "ksh"))
    }

    pub(super) fn resolve(mut group: u32) -> Option<u32> {
        let mut visited = std::collections::HashSet::new();
        let mut expected_tty = None;
        for _ in 0..8 {
            if group == 0 || !visited.insert(group) { return None; }
            let members: Vec<_> = list(PGRP, group).into_iter().filter_map(info)
                .filter(|p| p.pbi_pgid == group && expected_tty.is_none_or(|tty| p.e_tdev == tty && p.e_tpgid == group)).collect();
            // A group may outlive its leader. Do not assume PGID is a live PID.
            let leader = members.iter().find(|p| p.pbi_pid == group).or(members.first())?;
            let agents: Vec<_> = members.iter().filter(|p| is_agent(p.pbi_pid)).collect();
            if agents.len() == 1 { return Some(agents[0].pbi_pid); }
            if agents.len() > 1 { return Some(leader.pbi_pid); }
            let wrapper = super::display_name(leader.pbi_pid)?;
            if !["zsh (kiro-cli-term)", "bash (kiro-cli-term)", "sh (kiro-cli-term)", "fish (kiro-cli-term)"].contains(&wrapper.as_str()) {
                return Some(leader.pbi_pid);
            }
            let inner: Vec<_> = list(PPID, leader.pbi_pid).into_iter().filter_map(info)
                .filter(|p| p.pbi_ppid == leader.pbi_pid && p.pbi_start_tvsec >= leader.pbi_start_tvsec)
                .filter(|p| p.e_tdev != u32::MAX && p.e_tdev != leader.e_tdev && p.e_tpgid > 0 && p.e_tpgid != u32::MAX)
                .filter(|p| is_shell(p.pbi_pid)).collect();
            if inner.len() != 1 { return Some(leader.pbi_pid); }
            // Revalidate parent lifetime and the inner tty before using its
            // foreground group. Ignore all background children of that shell.
            let parent = info(leader.pbi_pid)?;
            let child = info(inner[0].pbi_pid)?;
            if parent.pbi_start_tvsec != leader.pbi_start_tvsec || parent.pbi_start_tvusec != leader.pbi_start_tvusec
                || child.pbi_ppid != parent.pbi_pid || child.pbi_start_tvsec != inner[0].pbi_start_tvsec
                || child.pbi_start_tvusec != inner[0].pbi_start_tvusec || child.e_tdev != inner[0].e_tdev {
                return None;
            }
            let foreground = list(PGRP, child.e_tpgid).into_iter().filter_map(info)
                .find(|p| p.e_tdev == child.e_tdev && p.pbi_pgid == child.e_tpgid && p.e_tpgid == child.e_tpgid);
            if foreground.is_none() { return None; }
            group = child.e_tpgid;
            expected_tty = Some(child.e_tdev);
        }
        None
    }
}


/// 사용자에게 보여줄 프로세스 이름. argv[0] 의 파일 이름을 쓰고(심볼릭 링크로 실행된 `claude` 처럼
/// 실제 파일 이름이 버전 번호인 경우 대비), 인터프리터면 스크립트 이름을 쓴다. Windows 에서는 `.exe` 를 빼고
/// 소문자로 쓴다.
pub fn display_name(pid: u32) -> Option<String> {
    let name = raw_display_name(pid);
    if cfg!(windows) { name.map(|n| n.to_lowercase()) } else { name }
}

/// Whether the shell `pid` reports its location only through its prompt (OSC 7): PowerShell
/// keeps its process folder unchanged when the location changes.
pub fn location_from_prompt(pid: u32) -> bool {
    cfg!(windows) && name(pid).is_some_and(|n| n.eq_ignore_ascii_case("pwsh") || n.eq_ignore_ascii_case("powershell"))
}

fn raw_display_name(pid: u32) -> Option<String> {
    let base = |s: &str| {
        let file = std::path::Path::new(s).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| s.to_string());
        match file.len().checked_sub(4) {
            Some(cut) if cfg!(windows) && file.is_char_boundary(cut) && file[cut..].eq_ignore_ascii_case(".exe") => file[..cut].to_string(),
            _ => file,
        }
    };
    if let Some(args) = cmdline(pid) {
        if let Some(a0) = args.first() {
            let n = base(a0.trim_start_matches('-'));
            let interp = ["node", "python", "python3", "bun", "deno", "ruby", "perl"];
            if interp.contains(&n.as_str()) {
                if let Some(script) = args.iter().skip(1).find(|a| !a.starts_with('-')) {
                    let s = base(script);
                    return Some(s.strip_suffix(".js").or(s.strip_suffix(".py")).unwrap_or(&s).to_string());
                }
            }
            if !n.is_empty() {
                return Some(n);
            }
        }
    }
    name(pid)
}

/// 프로세스 인자 목록.
#[cfg(target_os = "macos")]
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut size: libc::size_t = 0;
    // SAFETY: 크기를 먼저 받은 뒤 그만큼의 버퍼에 KERN_PROCARGS2 를 읽는다.
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 || size < 4 {
            return None;
        }
        let mut buf = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        buf.truncate(size);
        let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
        let mut i = 4;
        // 실행 파일 경로와 뒤따르는 NUL 패딩을 건너뛴다.
        while i < buf.len() && buf[i] != 0 {
            i += 1;
        }
        while i < buf.len() && buf[i] == 0 {
            i += 1;
        }
        let mut args = Vec::with_capacity(argc);
        for part in buf[i..].split(|&b| b == 0).take(argc) {
            args.push(String::from_utf8_lossy(part).into_owned());
        }
        if args.is_empty() { None } else { Some(args) }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = raw.split(|&b| b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
    if args.is_empty() { None } else { Some(args) }
}


#[cfg(target_os = "macos")]
pub fn name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buf 크기를 정확히 넘긴다.
    let n = unsafe { libc::proc_name(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(target_os = "macos")]
pub fn cwd(pid: u32) -> Option<String> {
    // SAFETY: 구조체 크기를 정확히 넘기고, 커널이 채운 NUL 종료 경로를 읽는다.
    unsafe {
        let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        let n = libc::proc_pidinfo(pid as i32, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size);
        if n != size {
            return None;
        }
        let p = info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char;
        let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
        if s.is_empty() { None } else { Some(s) }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned())
}

/// `struct vnode_fdinfowithpath` in bsd/sys/proc_info.h.
#[cfg(target_os = "macos")]
#[repr(C)]
struct VnodeFdInfoWithPath {
    openflags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guardflags: u32,
    path: libc::vnode_info_path,
}

/// Paths of the files and directories `pid` holds open.
#[cfg(target_os = "macos")]
pub fn open_files(pid: u32) -> Vec<std::path::PathBuf> {
    const PROC_PIDFDVNODEPATHINFO: i32 = 2;
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: a null buffer asks the kernel for the size of the descriptor table.
    let needed = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    // Room for descriptors opened between sizing and reading.
    let capacity = needed as usize / entry + 32;
    // SAFETY: proc_fdinfo is plain data; zeroed values are valid.
    let mut fds: Vec<libc::proc_fdinfo> = (0..capacity).map(|_| unsafe { std::mem::zeroed() }).collect();
    // SAFETY: the buffer size matches the allocated array.
    let bytes = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), (capacity * entry) as i32) };
    if bytes <= 0 {
        return Vec::new();
    }
    fds.truncate(bytes as usize / entry);
    let size = std::mem::size_of::<VnodeFdInfoWithPath>() as i32;
    fds.iter()
        .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_VNODE as u32)
        .filter_map(|fd| {
            // SAFETY: correctly sized output struct; the kernel writes a NUL-terminated path.
            unsafe {
                let mut info: VnodeFdInfoWithPath = std::mem::zeroed();
                let n = libc::proc_pidfdinfo(pid as i32, fd.proc_fd, PROC_PIDFDVNODEPATHINFO, (&mut info as *mut VnodeFdInfoWithPath).cast(), size);
                if n != size {
                    return None;
                }
                let path = std::ffi::CStr::from_ptr(info.path.vip_path.as_ptr() as *const libc::c_char);
                let path = path.to_string_lossy();
                (!path.is_empty()).then(|| std::path::PathBuf::from(path.into_owned()))
            }
        })
        .collect()
}

/// Paths of the files and directories `pid` holds open.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn open_files(pid: u32) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|path| path.is_absolute())
        .collect()
}



#[cfg(test)]
mod tests {
    #[test]
    #[cfg(unix)]
    fn open_files_lists_a_file_this_process_holds_open() {
        #[cfg(target_os = "macos")]
        assert_eq!(std::mem::size_of::<super::VnodeFdInfoWithPath>(), 1200);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held.lock");
        let file = std::fs::File::create(&path).unwrap();
        let held = std::fs::canonicalize(&path).unwrap();
        assert!(super::open_files(std::process::id()).iter().any(|p| p == &held), "{held:?}");
        drop(file);
        assert!(!super::open_files(std::process::id()).iter().any(|p| p == &held));
        assert!(super::open_files(u32::MAX - 1).is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn nested_pty_follows_only_the_foreground_job_and_recovers_shell() {
        use std::os::unix::process::CommandExt;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("bridge.c");
        let binary = dir.path().join("bridge");
        let command = dir.path().join("command");
        std::fs::write(&driver, include_str!("../tests/fixtures/nested-pty.c")).unwrap();
        let compile = std::process::Command::new("/usr/bin/cc").arg(&driver).arg("-o").arg(&binary).output().unwrap();
        assert!(compile.status.success(), "{}", String::from_utf8_lossy(&compile.stderr));
        let child = std::process::Command::new("/bin/bash")
            .args(["-c", "exec -a 'zsh (kiro-cli-term)' \"$1\" \"$2\"", "bridge"])
            .arg(&binary).arg(dir.path()).process_group(0)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().unwrap();
        struct Cleanup(std::process::Child);
        impl Drop for Cleanup { fn drop(&mut self) { unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM); } let _ = self.0.wait(); } }
        let child = Cleanup(child);
        let wait = |expected: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let actual = super::foreground_pid(child.0.id()).and_then(super::display_name);
                if actual.as_deref() == Some(expected) { break; }
                assert!(Instant::now() < deadline, "expected {expected}, got {actual:?}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait("bash");
        std::fs::write(&command, "(exec -a codex /bin/sleep 30)").unwrap();
        wait("codex");
        // Initial idle identity has no title/spinner and no GUI observation cache.
        assert_eq!(super::display_name(super::foreground_pid(child.0.id()).unwrap()).as_deref(), Some("codex"));
        std::fs::write(&command, "stop").unwrap(); wait("bash");
        std::fs::write(&command, format!("(exec -a codex /bin/sleep 30) & echo $! > '{}/background'", dir.path().display())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let background = loop {
            if let Ok(raw) = std::fs::read_to_string(dir.path().join("background")) {
                if let Ok(pid) = raw.trim().parse::<u32>() {
                    if super::display_name(pid).as_deref() == Some("codex") { break pid; }
                }
            }
            assert!(Instant::now() < deadline, "background agent did not start");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(background > 0); wait("bash");
        std::fs::write(&command, "(exec -a claude /bin/sleep 30)").unwrap(); wait("claude");
        std::fs::write(&command, "stop").unwrap(); wait("bash");
        std::fs::write(&command, "(exec -a vim /bin/sleep 30)").unwrap(); wait("vim");
        std::fs::write(&command, "stop").unwrap(); wait("bash");
        assert_eq!(super::foreground_pid(u32::MAX), None);
    }

    #[test]
    #[cfg(unix)]
    fn reads_own_process_cwd() {
        let me = std::process::id();
        let cwd = super::cwd(me).expect("cwd");
        assert_eq!(std::path::Path::new(&cwd).canonicalize().unwrap(), std::env::current_dir().unwrap().canonicalize().unwrap());
        assert!(super::name(me).is_some());
    }

    #[test]
    #[cfg(unix)]
    fn display_name_uses_argv0_of_symlinked_binary() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("claude");
        std::os::unix::fs::symlink("/bin/sleep", &link).unwrap();
        let mut child = std::process::Command::new(&link).arg("5").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let n = super::display_name(child.id());
        let _ = child.kill();
        assert_eq!(n.as_deref(), Some("claude"));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn same_dir(a: &str, b: &Path) -> bool {
        Path::new(a).canonicalize().ok() == b.canonicalize().ok()
    }

    fn stop(pid: u32) {
        let _ = Command::new("taskkill.exe").args(["/F", "/T", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }

    #[test]
    fn name_command_line_and_folder_of_this_process() {
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap();
        assert!(name(me).unwrap().eq_ignore_ascii_case(&exe.file_stem().unwrap().to_string_lossy()));
        assert_eq!(cmdline(me).unwrap(), std::env::args().collect::<Vec<_>>());
        assert!(same_dir(&cwd(me).unwrap(), &std::env::current_dir().unwrap()));
    }

    #[test]
    fn folder_of_a_child_is_its_start_folder() {
        let dir = tempfile::tempdir().unwrap();
        let child = Command::new("ping.exe").args(["-n", "30", "127.0.0.1"]).current_dir(dir.path()).stdout(Stdio::null()).spawn().unwrap();
        let found = cwd(child.id());
        stop(child.id());
        assert!(found.as_deref().is_some_and(|p| same_dir(p, dir.path())), "{found:?}");
    }

    #[test]
    fn window_applications_are_not_the_foreground() {
        let shell = Command::new("powershell.exe").args(["-NoProfile", "-Command", "Start-Process winver.exe; Start-Sleep 30"]).stdout(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut window = None;
        while Instant::now() < deadline && window.is_none() {
            let procs = snapshot_names(shell.id());
            window = procs.into_iter().find(|n| n.eq_ignore_ascii_case("winver.exe"));
            std::thread::sleep(Duration::from_millis(100));
        }
        let foreground = foreground_of(shell.id());
        stop(shell.id());
        assert!(window.is_some(), "winver started");
        assert_eq!(foreground, Some(shell.id()));
    }

    /// Executable names of the direct children of `parent`.
    fn snapshot_names(parent: u32) -> Vec<String> {
        let out = Command::new("powershell.exe").args(["-NoProfile", "-Command", &format!("Get-CimInstance Win32_Process -Filter 'ParentProcessId={parent}' | ForEach-Object Name")]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()
    }

    #[test]
    fn foreground_is_the_newest_descendant_and_prefers_an_agent() {
        let dir = tempfile::tempdir().unwrap();
        let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let agent = dir.path().join("codex.exe");
        std::fs::copy(system.join("cmd.exe"), &agent).unwrap();
        let shell = Command::new("cmd.exe").args(["/c", "ping -n 30 127.0.0.1 >nul"]).stdout(Stdio::null()).spawn().unwrap();
        let busy = {
            use std::os::windows::process::CommandExt;
            Command::new("cmd.exe").raw_arg(format!("/c \"\"{}\" /c ping -n 30 127.0.0.1 >nul\"", agent.display())).stdout(Stdio::null()).spawn().unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut plain, mut agent_seen) = (None, None);
        while Instant::now() < deadline && (plain.is_none() || agent_seen.is_none()) {
            plain = foreground_of(shell.id()).and_then(name).filter(|n| n.eq_ignore_ascii_case("ping"));
            agent_seen = foreground_of(busy.id()).and_then(name).filter(|n| n.eq_ignore_ascii_case("codex"));
            std::thread::sleep(Duration::from_millis(50));
        }
        stop(shell.id());
        stop(busy.id());
        assert!(plain.is_some(), "a shell's newest descendant is in the foreground");
        assert!(agent_seen.is_some(), "an agent stays in the foreground while it runs a command");
    }
}
