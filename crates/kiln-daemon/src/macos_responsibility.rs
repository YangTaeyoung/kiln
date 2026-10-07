//! Bootstrap the long-lived terminal owner before starting threads or restoring
//! PTYs. A detached owner can outlive its responsible GUI. macOS then attributes
//! new command-line children to themselves rather than the signed Kiln app.
//!
//! Replace only Kiln's daemon/PTY host with the same executable and PID. The
//! private responsibility SPI is dynamically resolved; its absence is an error
//! the caller reports without preventing session restoration. Public SETEXEC
//! preserves parenthood and every non-CLOEXEC restore descriptor.

use std::ffi::CString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

const MARKER: &str = "KILN_MACOS_RESPONSIBILITY_PID";

struct SpawnAttributes(libc::posix_spawnattr_t);

impl Drop for SpawnAttributes {
    fn drop(&mut self) {
        // SAFETY: created successfully by posix_spawnattr_init below.
        unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
    }
}

fn check(code: libc::c_int, operation: &str) -> anyhow::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{operation}: {}",
            std::io::Error::from_raw_os_error(code)
        ))
    }
}

pub fn ensure_own_responsibility() -> anyhow::Result<()> {
    let pid = std::process::id().to_string();
    if std::env::var_os(MARKER).as_deref() == Some(std::ffi::OsStr::new(&pid)) {
        return Ok(());
    }
    // Resolve at runtime: this SPI is not a supported Apple permission API.
    // Chromium's launch_mac.cc passes PosixSpawnAttr::get(), the ADDRESS of the
    // initialized opaque attr, not its internal heap pointer. See docs guide.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_spawnattrs_setdisclaim".as_ptr(),
        )
    };
    anyhow::ensure!(!symbol.is_null(), "macOS responsibility SPI unavailable");
    type SetDisclaim =
        unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
    let set_disclaim: SetDisclaim = unsafe { std::mem::transmute(symbol) };
    let executable = CString::new(std::env::current_exe()?.as_os_str().as_bytes())?;
    let args = std::env::args_os()
        .map(|arg| CString::new(arg.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut env = std::env::vars_os()
        .filter(|(key, _)| key != MARKER)
        .map(|(key, value)| {
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry)
        })
        .collect::<Result<Vec<_>, _>>()?;
    env.push(CString::new(format!("{MARKER}={pid}"))?);
    let pointers = |strings: &[CString]| {
        strings
            .iter()
            .map(|s| s.as_ptr().cast_mut())
            .chain(std::iter::once(std::ptr::null_mut()))
            .collect::<Vec<_>>()
    };
    let argv = pointers(&args);
    let envp = pointers(&env);
    let mut raw = std::ptr::null_mut();
    check(
        unsafe { libc::posix_spawnattr_init(&mut raw) },
        "initialize spawn attributes",
    )?;
    let mut attr = SpawnAttributes(raw);
    check(
        unsafe { set_disclaim(&mut attr.0, 1) },
        "claim background responsibility",
    )?;
    check(
        unsafe { libc::posix_spawnattr_setflags(&mut attr.0, libc::POSIX_SPAWN_SETEXEC as i16) },
        "set same-PID replacement",
    )?;
    let mut ignored_pid = 0;
    // SAFETY: all null-terminated arrays and initialized attributes live through
    // this call. No file actions or CLOEXEC_DEFAULT: upgrade FDs stay open.
    let code = unsafe {
        libc::posix_spawn(
            &mut ignored_pid,
            executable.as_ptr(),
            std::ptr::null(),
            &attr.0,
            argv.as_ptr(),
            envp.as_ptr(),
        )
    };
    check(code, "replace background owner")?;
    anyhow::bail!("same-PID replacement unexpectedly returned")
}
