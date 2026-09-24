//! 플랫폼별 로컬 IPC. Unix 는 Unix domain socket, Windows 는 네임드 파이프.

use std::io::{self, Read, Write};

pub trait Duplex: Read + Write + Send + 'static {}
impl<T: Read + Write + Send + 'static> Duplex for T {}

pub struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
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
            let n = name.to_ns_name::<GenericNamespaced>()?;
            Ok(Listener(ListenerOptions::new().name(n).create_sync()?))
        }

        pub fn accept(&self) -> io::Result<Conn> {
            let s = self.0.accept()?;
            let (r, w) = s.split();
            Ok(Conn { reader: Box::new(r), writer: Box::new(w) })
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
