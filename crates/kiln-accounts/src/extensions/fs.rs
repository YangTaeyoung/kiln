//! Handle-relative I/O. Windows never uses diagnostic paths for child operations.
use super::*;
#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{ffi::OsStrExt, fs::MetadataExt},
};
use std::path::Component;
#[cfg(any(unix, test))]
use std::path::PathBuf;
#[cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
};

#[cfg(unix)]
pub(super) struct Dir {
    file: File,
    pub path: PathBuf,
}
fn io<T>(v: std::io::Result<T>) -> R<T> {
    v.map_err(|_| "FILESYSTEM_ACCESS")
}
pub(super) fn component(s: &str) -> R<()> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.len() > 240
        || s.ends_with([' ', '.'])
        || s.chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err("INVALID_ASSET_PATH");
    }
    let stem = s.split('.').next().unwrap().to_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err("INVALID_ASSET_PATH");
    }
    Ok(())
}
pub(super) fn relative(s: &str) -> R<()> {
    if s.is_empty()
        || Path::new(s)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err("INVALID_ASSET_PATH");
    }
    for c in s.split('/') {
        component(c)?;
    }
    Ok(())
}
#[cfg(unix)]
fn c(s: &std::ffi::OsStr) -> R<std::ffi::CString> {
    std::ffi::CString::new(s.as_bytes()).map_err(|_| "INVALID_ASSET_PATH")
}
#[cfg(unix)]
fn opened(fd: i32) -> R<File> {
    if fd < 0 {
        Err("FILESYSTEM_ACCESS_OR_LINK")
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
#[cfg(test)]
thread_local! {
    pub(super) static TEST_SYNC_FAULT:std::cell::RefCell<Option<String>>=const{std::cell::RefCell::new(None)};
    pub(super) static TEST_KIND_FAULT:std::cell::RefCell<Option<PathBuf>>=const{std::cell::RefCell::new(None)};
    pub(super) static TEST_SYNC:std::cell::RefCell<Vec<String>>=const{std::cell::RefCell::new(Vec::new())};
    pub(super) static TEST_READDIR_FAULT:std::cell::RefCell<Option<(PathBuf,usize)>>=const{std::cell::RefCell::new(None)};
}
#[cfg(unix)]
fn errno_ptr() -> *mut libc::c_int {
    #[cfg(target_os = "macos")]
    {
        unsafe { libc::__error() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsafe { libc::__errno_location() }
    }
}
#[cfg(unix)]
impl Dir {
    pub fn open(path: &Path) -> R<Self> {
        let path = io(std::path::absolute(path))?;
        #[cfg(unix)]
        {
            let mut dir = Self {
                file: io(File::open("/"))?,
                path: PathBuf::from("/"),
            };
            for p in path.components() {
                if let Component::Normal(name) = p {
                    dir = dir.child_os(name, false)?;
                }
            }
            Ok(dir)
        }
    }
    fn child_os(&self, name: &std::ffi::OsStr, create: bool) -> R<Self> {
        #[cfg(unix)]
        {
            let name_c = c(name)?;
            if create {
                let rc = unsafe { libc::mkdirat(self.file.as_raw_fd(), name_c.as_ptr(), 0o700) };
                if rc < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err("FILESYSTEM_ACCESS");
                }
            }
            if create {
                self.sync()?;
            }
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            Ok(Self {
                file: opened(fd)?,
                path: self.path.join(name),
            })
        }
    }
    pub fn dir(&self, name: &str, create: bool) -> R<Self> {
        component(name)?;
        self.child_os(name.as_ref(), create)
    }
    pub fn parent(&self, rel: &str, create: bool) -> R<(Self, String)> {
        relative(rel)?;
        let mut parts = rel.split('/').collect::<Vec<_>>();
        let leaf = parts.pop().unwrap().to_string();
        #[cfg(unix)]
        let mut dir = Self {
            file: opened(unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c(".".as_ref())?.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            })?,
            path: self.path.clone(),
        };

        for part in parts {
            dir = dir.dir(part, create)?;
        }
        Ok((dir, leaf))
    }
    pub fn sync(&self) -> R<()> {
        #[cfg(test)]
        if TEST_SYNC_FAULT.with(|t| {
            t.borrow()
                .as_ref()
                .is_some_and(|suffix| self.path.to_string_lossy().ends_with(suffix))
        }) {
            return Err("DIRECTORY_SYNC_FAILED");
        }
        #[cfg(unix)]
        io(self.file.sync_all())?;
        #[cfg(test)]
        TEST_SYNC.with(|t| {
            t.borrow_mut()
                .push(self.path.to_string_lossy().into_owned())
        });
        Ok(())
    }
    pub fn sync_tree(&self) -> R<()> {
        for name in self.list()? {
            if self.kind(&name)? == Some(Kind::Dir) {
                self.dir(&name, false)?.sync_tree()?;
            }
        }
        self.sync()
    }
    pub fn revalidate(&self) -> R<()> {
        let current = Self::open(&self.path)?;
        #[cfg(unix)]
        {
            let a = io(self.file.metadata())?;
            let b = io(current.file.metadata())?;
            if a.dev() != b.dev() || a.ino() != b.ino() {
                return Err("ROOT_CHANGED_DURING_RECONCILE");
            }
        }

        Ok(())
    }
    pub fn private(&self) -> R<()> {
        #[cfg(unix)]
        {
            let m = io(self.file.metadata())?;
            if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
                return Err("PRIVATE_STATE_PERMISSIONS");
            }
        }
        Ok(())
    }
    pub fn read_private(&self, name: &str, limit: usize) -> R<Vec<u8>> {
        #[cfg(unix)]
        {
            let file = opened(unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c(name.as_ref())?.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            let m = io(file.metadata())?;
            if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
                return Err("PRIVATE_STATE_PERMISSIONS");
            }
        }
        self.read(name, limit).map(|v| v.0)
    }
    pub fn lock(&self, name: &str) -> R<File> {
        component(name)?;
        #[cfg(unix)]
        let file = opened(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c(name.as_ref())?.as_ptr(),
                libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;

        file.try_lock().map_err(|_| "PROFILE_RECONCILE_BUSY")?;
        Ok(file)
    }
    pub fn list(&self) -> R<Vec<String>> {
        #[cfg(unix)]
        {
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c(".".as_ref())?.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err("FILESYSTEM_ACCESS");
            }
            let raw = unsafe { libc::fdopendir(fd) };
            if raw.is_null() {
                unsafe { libc::close(fd) };
                return Err("FILESYSTEM_ACCESS");
            }
            let mut out = Vec::new();
            let mut error = None;
            loop {
                unsafe { *errno_ptr() = 0 };
                #[cfg(test)]
                let injected = TEST_READDIR_FAULT.with(|t| {
                    t.borrow()
                        .as_ref()
                        .is_some_and(|(path, after)| path == &self.path && out.len() >= *after)
                });
                #[cfg(not(test))]
                let injected = false;
                let p = if injected {
                    unsafe { *errno_ptr() = libc::EIO };
                    std::ptr::null_mut()
                } else {
                    unsafe { libc::readdir(raw) }
                };
                if p.is_null() {
                    if unsafe { *errno_ptr() } != 0 {
                        error = Some("DIRECTORY_ENUMERATION_FAILED");
                    }
                    break;
                }
                let bytes = unsafe { std::ffi::CStr::from_ptr((*p).d_name.as_ptr()) }.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                match std::str::from_utf8(bytes) {
                    Ok(s) => out.push(s.to_string()),
                    Err(_) => {
                        error = Some("INVALID_ASSET_PATH");
                        break;
                    }
                }
                if out.len() > TOTAL_FILES {
                    error = Some("TOTAL_FILES_16384");
                    break;
                }
            }
            unsafe { libc::closedir(raw) };
            if let Some(e) = error {
                return Err(e);
            }
            out.sort();
            Ok(out)
        }
    }
    pub fn kind(&self, name: &str) -> R<Option<Kind>> {
        #[cfg(test)]
        if TEST_KIND_FAULT.with(|t| {
            t.borrow()
                .as_ref()
                .is_some_and(|p| *p == self.path.join(name))
        }) {
            return Err("FILESYSTEM_ACCESS");
        }
        component(name)?;
        #[cfg(unix)]
        {
            let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    self.file.as_raw_fd(),
                    c(name.as_ref())?.as_ptr(),
                    st.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } < 0
            {
                return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err("FILESYSTEM_ACCESS")
                };
            }
            let st = unsafe { st.assume_init() };
            let ty = st.st_mode & libc::S_IFMT;
            Ok(Some(if ty == libc::S_IFDIR {
                Kind::Dir
            } else if ty == libc::S_IFREG {
                Kind::File
            } else if ty == libc::S_IFLNK {
                Kind::Link
            } else {
                Kind::Special
            }))
        }
    }
    pub fn read(&self, name: &str, limit: usize) -> R<(Vec<u8>, bool)> {
        component(name)?;
        #[cfg(unix)]
        let mut file = opened(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c(name.as_ref())?.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;

        let before = io(file.metadata())?;
        if !before.is_file() {
            return Err("SPECIAL_FILE");
        }
        if before.len() > limit as u64 {
            return Err("FILE_SIZE_LIMIT");
        }
        let mut bytes = Vec::new();
        io((&mut file).take((limit + 1) as u64).read_to_end(&mut bytes))?;
        let after = io(file.metadata())?;
        if bytes.len() > limit
            || before.len() != after.len()
            || io(before.modified())? != io(after.modified())?
        {
            return Err("SOURCE_CHANGED_DURING_READ");
        }
        #[cfg(unix)]
        {
            if before.dev() != after.dev()
                || before.ino() != after.ino()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
            {
                return Err("SOURCE_CHANGED_DURING_READ");
            }
            Ok((bytes, before.mode() & 0o111 != 0))
        }
    }
    pub fn write_new(&self, name: &str, bytes: &[u8], exec: bool) -> R<()> {
        component(name)?;

        #[cfg(unix)]
        let mut file = opened(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c(name.as_ref())?.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                if exec { 0o700 } else { 0o600 },
            )
        })?;

        io(file.write_all(bytes))?;
        io(file.sync_all())?;
        Ok(())
    }
    pub fn rename(&self, from: &str, to: &Dir, name: &str) -> R<()> {
        component(from)?;
        component(name)?;
        #[cfg(unix)]
        {
            if unsafe {
                libc::renameat(
                    self.file.as_raw_fd(),
                    c(from.as_ref())?.as_ptr(),
                    to.file.as_raw_fd(),
                    c(name.as_ref())?.as_ptr(),
                )
            } < 0
            {
                return Err("ATOMIC_PUBLICATION_FAILED");
            }
            #[cfg(test)]
            TEST_SYNC.with(|t| t.borrow_mut().push(format!("publish:{name}")));
            self.sync()?;
            to.sync()?;
            Ok(())
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn swap(&self, from: &str, to: &Dir, name: &str) -> R<()> {
        component(from)?;
        component(name)?;
        #[cfg(target_os = "macos")]
        let rc = unsafe {
            libc::renameatx_np(
                self.file.as_raw_fd(),
                c(from.as_ref())?.as_ptr(),
                to.file.as_raw_fd(),
                c(name.as_ref())?.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        #[cfg(target_os = "linux")]
        let rc = unsafe {
            libc::renameat2(
                self.file.as_raw_fd(),
                c(from.as_ref())?.as_ptr(),
                to.file.as_raw_fd(),
                c(name.as_ref())?.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            if rc < 0 {
                return Err("ATOMIC_PUBLICATION_FAILED");
            }
            #[cfg(test)]
            TEST_SYNC.with(|t| t.borrow_mut().push(format!("publish:{name}")));
            self.sync()?;
            to.sync()?;
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (from, to, name);
            Err("ATOMIC_DIRECTORY_EXCHANGE_UNAVAILABLE")
        }
    }
    pub fn remove(&self, name: &str) -> R<()> {
        match self.kind(name)? {
            None => return Ok(()),
            Some(Kind::Dir) => {
                let dir = self.dir(name, false)?;
                for child in dir.list()? {
                    dir.remove(&child)?;
                }
            }
            _ => {}
        }
        #[cfg(unix)]
        {
            let flags = if self.kind(name)? == Some(Kind::Dir) {
                libc::AT_REMOVEDIR
            } else {
                0
            };
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), c(name.as_ref())?.as_ptr(), flags) }
                < 0
            {
                return Err("FILESYSTEM_ACCESS");
            }
            self.sync()?;
            #[cfg(test)]
            TEST_SYNC.with(|t| t.borrow_mut().push(format!("remove:{name}")));
            Ok(())
        }
    }
    pub fn atomic(&self, name: &str, bytes: &[u8]) -> R<()> {
        let temp = format!("write-{}", nonce());
        self.write_new(&temp, bytes, false)?;
        self.rename(&temp, self, name)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Dir,
    File,
    Link,
    Special,
}
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::Dir;

#[cfg(all(test, windows))]
mod windows_tests;
