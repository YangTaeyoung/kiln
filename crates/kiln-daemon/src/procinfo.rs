//! 프로세스 이름과 작업 디렉토리 조회.

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
}
