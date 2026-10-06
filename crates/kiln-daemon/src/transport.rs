//! 플랫폼별 로컬 IPC. Unix 는 Unix domain socket, Windows 는 네임드 파이프.

use std::io::{self, Read, Write};

pub trait Duplex: Read + Write + Send + 'static {}
impl<T: Read + Write + Send + 'static> Duplex for T {}

pub struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

/// Close only this IPC connection, leaving the persistent PTY child intact.
/// Framed host readers use this to unblock their decoder when dropped.
pub type Shutdown = std::sync::Arc<dyn Fn() + Send + Sync>;

pub fn connect_with_shutdown(name: &str) -> io::Result<(Conn, Option<Shutdown>)> {
    #[cfg(unix)]
    {
        let stream = std::os::unix::net::UnixStream::connect(name)?;
        let cancel = stream.try_clone()?;
        let shutdown: Shutdown = std::sync::Arc::new(move || { let _ = cancel.shutdown(std::net::Shutdown::Both); });
        Ok((imp::split(stream)?, Some(shutdown)))
    }
    #[cfg(windows)]
    { Ok((imp::connect(name)?, None)) }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    pub struct Listener(pub UnixListener);

    impl Listener {
        pub fn bind(name: &str) -> io::Result<Self> {
            let path = Path::new(name);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
            if path.exists() {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(io::ErrorKind::AddrInUse, "daemon already running"));
                }
                std::fs::remove_file(path)?;
            }
            Ok(Listener(UnixListener::bind(path)?))
        }

        pub fn accept(&self) -> io::Result<Conn> {
            let (s, _) = self.0.accept()?;
            split(s)
        }
    }

    pub fn split(s: UnixStream) -> io::Result<Conn> {
        let r = s.try_clone()?;
        Ok(Conn { reader: Box::new(r), writer: Box::new(s) })
    }

    pub fn connect(name: &str) -> io::Result<Conn> {
        split(UnixStream::connect(name)?)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};

    pub struct Listener(pub interprocess::local_socket::Listener);

    impl Listener {
        pub fn bind(name: &str) -> io::Result<Self> {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            use interprocess::os::windows::security_descriptor::SecurityDescriptor;
            let n = name.to_ns_name::<GenericNamespaced>()?;
            let mut opts = ListenerOptions::new().name(n);
            // 권한 상승 여부와 무관하게 같은 사용자만 접근하도록 현재 사용자 SID 에 전체 권한을 준다.
            if let Some(sid) = current_user_sid() {
                let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)")).map_err(io::Error::other)?;
                opts = opts.security_descriptor(SecurityDescriptor::deserialize(&sddl)?);
            }
            Ok(Listener(opts.create_sync()?))
        }

        pub fn accept(&self) -> io::Result<Conn> {
            let s = self.0.accept()?;
            let (r, w) = s.split();
            Ok(Conn { reader: Box::new(r), writer: Box::new(w) })
        }
    }

    /// 현재 프로세스 토큰의 사용자 SID 문자열(예: S-1-5-21-...).
    fn current_user_sid() -> Option<String> {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        // SAFETY: 토큰 정보를 충분한 버퍼에 받아 SID 를 문자열로 바꾸고 할당을 해제한다.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return None;
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut out: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut out) == 0 {
                return None;
            }
            let s = widestring::U16CStr::from_ptr_str(out).to_string_lossy();
            LocalFree(out.cast());
            Some(s)
        }
    }

    pub fn connect(name: &str) -> io::Result<Conn> {
        let n = name.to_ns_name::<GenericNamespaced>()?;
        let s = Stream::connect(n)?;
        let (r, w) = s.split();
        Ok(Conn { reader: Box::new(r), writer: Box::new(w) })
    }
}

pub use imp::*;
