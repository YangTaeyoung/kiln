//! 프로세스 이름과 작업 디렉토리 조회.

/// 사용자에게 보여줄 프로세스 이름. argv[0] 의 파일 이름을 쓰고(심볼릭 링크로 실행된 `claude` 처럼
/// 실제 파일 이름이 버전 번호인 경우 대비), 인터프리터면 스크립트 이름을 쓴다.
pub fn display_name(pid: u32) -> Option<String> {
    let base = |s: &str| std::path::Path::new(s).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| s.to_string());
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

#[cfg(windows)]
pub fn cmdline(_pid: u32) -> Option<Vec<String>> {
    None
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

#[cfg(windows)]
pub fn name(_pid: u32) -> Option<String> {
    None
}

#[cfg(windows)]
pub fn cwd(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
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
