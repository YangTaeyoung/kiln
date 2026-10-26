//! 플랫폼별 로컬 IPC. Unix 는 Unix domain socket, Windows 는 네임드 파이프.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

pub trait Duplex: Read + Write + Send + 'static {}
impl<T: Read + Write + Send + 'static> Duplex for T {}

pub struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

/// Bound protocol I/O on this upgrade-only connection with one absolute
/// deadline. Establishing the OS connection itself may still block.
pub fn connect_control(name: &str, deadline: Instant) -> io::Result<Conn> {
    let Conn { reader, writer } = imp::connect_nonblocking(name)?;
    Ok(Conn {
        reader: Box::new(DeadlineIo { inner: reader, deadline }),
        writer: Box::new(DeadlineIo { inner: writer, deadline }),
    })
}

struct DeadlineIo<T> {
    inner: T,
    deadline: Instant,
}

impl<T> DeadlineIo<T> {
    fn retry<R>(&mut self, mut operation: impl FnMut(&mut T) -> io::Result<R>) -> io::Result<R> {
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "daemon upgrade protocol timed out"));
            }
            match operation(&mut self.inner) {
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                result => return result,
            }
        }
    }
}

impl<T: Read> Read for DeadlineIo<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.retry(|inner| inner.read(bytes))
    }
}

impl<T: Write> Write for DeadlineIo<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.retry(|inner| match inner.write(bytes) {
            // A byte-mode PIPE_NOWAIT write may succeed with zero bytes when
            // its buffer is full. Treat that as backpressure, not WriteZero.
            Ok(0) if !bytes.is_empty() => Err(io::ErrorKind::WouldBlock.into()),
            result => result,
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.retry(Write::flush)
    }
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

    pub(super) fn connect_nonblocking(name: &str) -> io::Result<Conn> {
        let stream = UnixStream::connect(name)?;
        stream.set_nonblocking(true)?;
        split(stream)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
    use interprocess::os::windows::named_pipe::local_socket::{RecvHalf as PipeRecvHalf, SendHalf as PipeSendHalf, Stream as PipeStream};

    struct ControlReader(PipeRecvHalf);

    impl Read for ControlReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            use std::os::windows::io::{AsHandle, AsRawHandle};
            use windows_sys::Win32::{Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED}, System::Pipes::PeekNamedPipe};
            if bytes.is_empty() { return Ok(0); }
            // interprocess 2.4.4 maps PIPE_NOWAIT's empty-read ERROR_NO_DATA
            // through BrokenPipe to EOF. Peek first to keep an idle pipe
            // distinct from a closed peer, then read only available bytes.
            let mut available = 0;
            // SAFETY: the receive half owns this live pipe handle; only the
            // initialized count is written, without consuming any pipe data.
            let ok = unsafe { PeekNamedPipe(self.0.as_handle().as_raw_handle(), std::ptr::null_mut(), 0,
                std::ptr::null_mut(), &mut available, std::ptr::null_mut()) };
            if ok == 0 {
                let error = io::Error::last_os_error();
                return if matches!(error.raw_os_error().map(|e| e as u32), Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED)) {
                    Ok(0)
                } else { Err(error) };
            }
            if available == 0 { return Err(io::ErrorKind::WouldBlock.into()); }
            let count = bytes.len().min(available as usize);
            self.0.read(&mut bytes[..count])
        }
    }

    struct ControlWriter(PipeSendHalf);

    impl Write for ControlWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { self.0.write(bytes) }
        fn flush(&mut self) -> io::Result<()> { self.0.flush() }
    }

    impl Drop for ControlWriter {
        fn drop(&mut self) {
            // A timed-out control request must close rather than leave a
            // background pipe flusher waiting for an unresponsive peer.
            self.0.as_ref().assume_flushed();
        }
    }

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

    pub(super) fn connect_nonblocking(name: &str) -> io::Result<Conn> {
        let n = name.to_ns_name::<GenericNamespaced>()?;
        let stream = PipeStream::connect(n)?;
        stream.set_nonblocking(true)?;
        let (reader, writer) = stream.split();
        Ok(Conn { reader: Box::new(ControlReader(reader)), writer: Box::new(ControlWriter(writer)) })
    }
}

pub use imp::*;

#[cfg(test)]
mod control_write_tests {
    use super::*;

    struct FullBuffer { zero_writes: usize, bytes: Vec<u8> }
    impl Write for FullBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.zero_writes > 0 {
                self.zero_writes -= 1;
                return Ok(0);
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    #[test]
    fn zero_byte_nonblocking_writes_wait_for_capacity_without_losing_bytes() {
        let mut writer = DeadlineIo {
            inner: FullBuffer { zero_writes: 1, bytes: Vec::new() },
            deadline: Instant::now() + Duration::from_secs(1),
        };
        writer.write_all(b"owned frame").unwrap();
        assert_eq!(writer.inner.bytes, b"owned frame");
    }

    #[test]
    fn zero_byte_nonblocking_writes_still_expire_at_the_deadline() {
        let mut writer = DeadlineIo {
            inner: FullBuffer { zero_writes: usize::MAX, bytes: Vec::new() },
            deadline: Instant::now() + Duration::from_millis(50),
        };
        assert_eq!(writer.write_all(b"owned frame").unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(writer.inner.bytes.is_empty());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn a_path_like_socket_name_is_a_valid_pipe() {
        let name = std::env::temp_dir().join(format!("kiln-path-socket-{}", std::process::id())).join("d.sock");
        let name = name.to_string_lossy().into_owned();
        let listener = Listener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let mut byte = [0u8; 1];
            conn.reader.read_exact(&mut byte).unwrap();
            byte[0]
        });
        let mut client = connect(&name).unwrap();
        client.writer.write_all(&[42]).unwrap();
        client.writer.flush().unwrap();
        assert_eq!(server.join().unwrap(), 42);
    }
}
