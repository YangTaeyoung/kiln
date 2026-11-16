//! Opt-in native storage for controllers crossing a filesystem trust boundary.
//! Legacy desktop path I/O is unchanged. All actual I/O is relative to retained fds.
use super::*;
use anyhow::bail;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString, OsStr, OsString},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
    sync::Mutex,
};
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity(u64, u64);
fn identity(f: &File) -> Result<Identity> {
    let m = f.metadata()?;
    Ok(Identity(m.dev(), m.ino()))
}
fn open_at(dir: &File, name: &OsStr, flags: i32, mode: u32) -> Result<File> {
    let name = CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn names(dir: &File) -> Result<Vec<OsString>> {
    let fd = open_at(dir, OsStr::new("."), libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    use std::os::fd::IntoRawFd;
    let raw = fd.into_raw_fd();
    let ptr = unsafe { libc::fdopendir(raw) };
    if ptr.is_null() {
        unsafe { libc::close(raw) };
        bail!("unavailable mailbox directory")
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let guard = Directory(ptr);
    let mut out = vec![];
    loop {
        let item = unsafe { libc::readdir(guard.0) };
        if item.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        out.push(OsString::from_vec(name.to_vec()));
        ensure!(out.len() <= 256, "agent command queue is full");
    }
    Ok(out)
}
pub(in super::super) struct RetainedStorage {
    chain: Vec<(File, OsString, Identity)>,
    dir: File,
    files: Mutex<BTreeMap<String, File>>,
    queue: Mutex<Option<File>>,
}
impl RetainedStorage {
    pub(in super::super) fn open(path: &Path) -> Result<Self> {
        ensure!(path.is_absolute(), "absolute storage required");
        let mut dir = File::open("/")?;
        let mut chain = vec![];
        for part in path.components() {
            match part {
                std::path::Component::RootDir => {}
                std::path::Component::Normal(name) => {
                    let child = open_at(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
                    chain.push((dir, name.into(), identity(&child)?));
                    dir = child
                }
                _ => bail!("invalid storage directory"),
            }
        }
        let storage = Self {
            chain,
            dir,
            files: Mutex::new(BTreeMap::new()),
            queue: Mutex::new(None),
        };
        storage.file("config.json")?;
        match storage.file("events.jsonl") {
            Ok(_) => {}
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
            Err(e) => return Err(e),
        }
        storage.with_queue(false, |_| Ok(()))?;
        storage.validate()?;
        Ok(storage)
    }
    pub(in super::super) fn matches_descriptor(
        &self,
        expected: &super::super::Descriptor,
    ) -> Result<()> {
        let mut file = self.file("config.json")?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = vec![];
        file.take((MAX_RECORD + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_RECORD, "oversized native config");
        #[derive(serde::Deserialize)]
        struct Projection {
            descriptor: super::super::Descriptor,
        }
        let projection: Projection = serde_json::from_slice(&bytes)?;
        ensure!(
            &projection.descriptor == expected,
            "native descriptor changed"
        );
        self.validate()
    }
    fn validate_directory(&self) -> Result<()> {
        for (parent, name, id) in &self.chain {
            let current = open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
            ensure!(
                identity(&current)? == *id,
                "native storage directory changed"
            );
        }
        Ok(())
    }
    fn file(&self, name: &str) -> Result<File> {
        self.validate_directory()?;
        let current = open_at(&self.dir, OsStr::new(name), libc::O_RDONLY, 0)?;
        let m = current.metadata()?;
        ensure!(m.is_file() && m.nlink() == 1, "invalid native storage file");
        let mut files = self.files.lock().unwrap();
        if let Some(pinned) = files.get(name) {
            ensure!(
                identity(pinned)? == identity(&current)?,
                "native storage file changed"
            );
            Ok(pinned.try_clone()?)
        } else {
            files.insert(name.into(), current.try_clone()?);
            Ok(current)
        }
    }
    pub(in super::super) fn validate(&self) -> Result<()> {
        self.validate_directory()?;
        let keys = self
            .files
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for name in keys {
            self.file(&name)?;
        }
        let queue = self.queue.lock().unwrap();
        if let Some(pinned) = queue.as_ref() {
            let current = open_at(
                &self.dir,
                OsStr::new("commands"),
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?;
            ensure!(
                identity(pinned)? == identity(&current)?,
                "native mailbox directory changed"
            );
        }
        Ok(())
    }
    fn with_queue<T>(
        &self,
        create: bool,
        action: impl FnOnce(Option<&File>) -> Result<T>,
    ) -> Result<T> {
        self.validate_directory()?;
        let mut queue = self.queue.lock().unwrap();
        let current = match open_at(
            &self.dir,
            OsStr::new("commands"),
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        ) {
            Ok(f) => Some(f),
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                if create {
                    let name = c"commands";
                    let r = unsafe { libc::mkdirat(self.dir.as_raw_fd(), name.as_ptr(), 0o700) };
                    if r < 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::AlreadyExists
                    {
                        bail!("mailbox unavailable")
                    }
                    Some(open_at(
                        &self.dir,
                        OsStr::new("commands"),
                        libc::O_RDONLY | libc::O_DIRECTORY,
                        0,
                    )?)
                } else {
                    None
                }
            }
            Err(e) => return Err(e),
        };
        if let Some(current) = current {
            if let Some(pinned) = queue.as_ref() {
                ensure!(
                    identity(pinned)? == identity(&current)?,
                    "native mailbox directory changed"
                );
            } else {
                *queue = Some(current)
            }
        } else {
            ensure!(queue.is_none(), "native mailbox disappeared")
        };
        action(queue.as_ref())
    }
    pub(in super::super) fn tail(&self, offset: &mut u64) -> Result<Vec<Envelope>> {
        self.validate()?;
        let mut file = self.file("events.jsonl")?;
        let before = *offset;
        let result = tail_file(&mut file, offset);
        if let Err(e) = self.validate() {
            *offset = before;
            return Err(e);
        }
        result
    }
    pub(in super::super) fn journal_len(&self) -> Result<u64> {
        self.validate()?;
        let len = self.file("events.jsonl")?.metadata()?.len();
        ensure!(len <= MAX_JOURNAL, "oversized agent journal");
        Ok(len)
    }
    pub(in super::super) fn failure_reason(&self) -> Option<String> {
        let mut file = match self.file("failure.json") {
            Ok(file) => file,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return None;
            }
            Err(_) => return Some("Native failure metadata is unavailable".into()),
        };
        if file.seek(SeekFrom::Start(0)).is_err() {
            return Some("Native failure metadata is unavailable".into());
        }
        let mut bytes = vec![];
        if file
            .take((MAX_RECORD + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_RECORD
        {
            return Some("Native failure metadata is unavailable".into());
        }
        Some(
            serde_json::from_slice::<Failure>(&bytes)
                .map(|f| f.reason)
                .unwrap_or_else(|_| "Native failure metadata is unavailable".into()),
        )
    }
    pub(in super::super) fn worker_absent(&self) -> bool {
        // A try_clone shares the flock description; never leave a probe lock on a retained fd.
        if self.file("host.lock").is_err() {
            return false;
        }
        open_at(&self.dir, OsStr::new("host.lock"), libc::O_RDONLY, 0)
            .is_ok_and(|file| file.try_lock().is_ok())
    }
    pub(in super::super) fn published_ids(&self) -> Result<BTreeSet<String>> {
        self.validate()?;
        self.with_queue(false, |dir| {
            let Some(dir) = dir else {
                return Ok(BTreeSet::new());
            };
            let mut ids = BTreeSet::new();
            for name in names(dir)? {
                if Path::new(&name).extension() != Some(OsStr::new("json")) {
                    continue;
                }
                let file = open_at(dir, &name, libc::O_RDONLY, 0)?;
                let m = file.metadata()?;
                ensure!(
                    m.is_file() && m.nlink() == 1 && m.len() <= MAX_RECORD as u64,
                    "invalid native mailbox file"
                );
                let mut bytes = vec![];
                file.take((MAX_RECORD + 1) as u64).read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= MAX_RECORD, "oversized native mailbox file");
                ids.insert(serde_json::from_slice::<QueuedCommand>(&bytes)?.id);
            }
            Ok(ids)
        })
    }
    pub(in super::super) fn enqueue(
        &self,
        command: &QueuedCommand,
    ) -> std::result::Result<(), PublicationError> {
        let mut uncertain = false;
        let result = (|| -> Result<()> {
            self.validate()?;
            ensure!(valid_id(&command.id), "invalid command ID");
            ensure!(
                self.failure_reason().is_none(),
                "native storage unavailable"
            );
            let bytes = serde_json::to_vec(command)?;
            ensure!(bytes.len() <= MAX_RECORD, "oversized agent record");
            self.with_queue(true, |dir| {
                let dir = dir.context("mailbox unavailable")?;
                ensure!(names(dir)?.len() < 256, "agent command queue is full");
                let temp = OsString::from(format!("{}.{}.tmp", command.id, unique()));
                let target = CString::new(format!("{}.json", command.id))?;
                let temp_c = CString::new(temp.as_bytes())?;
                let mut file = open_at(
                    dir,
                    &temp,
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )?;
                let result = (|| -> Result<()> {
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    self.validate_directory()?;
                    let current = open_at(
                        &self.dir,
                        OsStr::new("commands"),
                        libc::O_RDONLY | libc::O_DIRECTORY,
                        0,
                    )?;
                    ensure!(
                        identity(dir)? == identity(&current)?,
                        "native mailbox directory changed"
                    );
                    uncertain = true;
                    ensure!(
                        unsafe {
                            libc::renameat(
                                dir.as_raw_fd(),
                                temp_c.as_ptr(),
                                dir.as_raw_fd(),
                                target.as_ptr(),
                            )
                        } == 0,
                        "mailbox publication failed"
                    );
                    dir.sync_all()?;
                    Ok(())
                })();
                if result.is_err() {
                    unsafe { libc::unlinkat(dir.as_raw_fd(), temp_c.as_ptr(), 0) };
                }
                result
            })
        })();
        result.map_err(|error| PublicationError { uncertain, error })
    }
    pub(in super::super) fn summary(&self) -> Result<(Vec<Envelope>, bool)> {
        self.validate()?;
        let mut file = self.file("events.jsonl")?;
        let len = file.metadata()?.len();
        ensure!(len <= MAX_JOURNAL, "oversized agent journal");
        let budget = 256 * 1024usize;
        let mut bytes = vec![];
        Read::by_ref(&mut file)
            .take((budget + 1) as u64)
            .read_to_end(&mut bytes)?;
        let mut events = vec![];
        let mut consumed = 0usize;
        for line in bytes.split_inclusive(|b| *b == b'\n').take(256) {
            if line.last() != Some(&b'\n') || consumed + line.len() > budget {
                break;
            }
            events.push(serde_json::from_slice(line)?);
            consumed += line.len();
        }
        self.validate()?;
        Ok((events, consumed as u64 != len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> (tempfile::TempDir, PathBuf) {
        let r = tempfile::tempdir().unwrap();
        let p = fs::canonicalize(r.path()).unwrap();
        fs::write(p.join("config.json"), b"{}").unwrap();
        (r, p)
    }
    fn event(sequence: u64, text: &str) -> String {
        serde_json::to_string(&Envelope {
            sequence,
            generation: "g".into(),
            event: Event::Message {
                message: super::super::super::Message {
                    id: format!("m-{sequence}"),
                    role: super::super::super::Role::User,
                    text: text.into(),
                    attachments: vec![],
                },
                append: false,
            },
        })
        .unwrap()
            + "\n"
    }
    #[test]
    fn retained_journal_refuses_symlink_and_regular_replacement() {
        let (_r, p) = root();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::write(outside.path(), event(1, "outside canary")).unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("events.jsonl")).unwrap();
        assert!(RetainedStorage::open(&p).is_err());
        fs::remove_file(p.join("events.jsonl")).unwrap();
        fs::write(p.join("events.jsonl"), event(1, "trusted")).unwrap();
        let s = RetainedStorage::open(&p).unwrap();
        let mut offset = 0;
        assert_eq!(s.tail(&mut offset).unwrap().len(), 1);
        fs::rename(p.join("events.jsonl"), p.join("old.jsonl")).unwrap();
        fs::copy(outside.path(), p.join("events.jsonl")).unwrap();
        let before = offset;
        assert!(s.tail(&mut offset).is_err());
        assert_eq!(offset, before);
    }
    #[test]
    fn retained_mailbox_never_publishes_into_replaced_directory() {
        let (_r, p) = root();
        fs::write(p.join("events.jsonl"), event(1, "trusted")).unwrap();
        let s = RetainedStorage::open(&p).unwrap();
        let command = QueuedCommand {
            id: "first".into(),
            generation: "g".into(),
            command: super::super::super::Command::Interrupt,
        };
        s.enqueue(&command).unwrap();
        assert!(s.published_ids().unwrap().contains("first"));
        let outside = tempfile::tempdir().unwrap();
        fs::rename(p.join("commands"), p.join("commands-old")).unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("commands")).unwrap();
        let mut second = command;
        second.id = "second".into();
        assert!(s.enqueue(&second).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        assert!(!p.join("commands-old/second.json").exists());
    }
    #[test]
    fn retained_worker_health_probe_releases_its_lock() {
        let (_r, p) = root();
        fs::write(p.join("host.lock"), b"").unwrap();
        let s = RetainedStorage::open(&p).unwrap();
        assert!(s.worker_absent());
        let worker = OpenOptions::new()
            .read(true)
            .write(true)
            .open(p.join("host.lock"))
            .unwrap();
        assert!(
            worker.try_lock().is_ok(),
            "mobile health probe retained the worker's exclusive lock"
        );
    }
    #[test]
    fn retained_config_fifo_and_conversation_rename_fail_without_blocking() {
        let (_r, p) = root();
        let s = RetainedStorage::open(&p).unwrap();
        fs::remove_file(p.join("config.json")).unwrap();
        let name = CString::new(p.join("config.json").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(s.validate().is_err());
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        fs::remove_file(p.join("config.json")).unwrap();
        fs::write(p.join("config.json"), b"{}").unwrap();
        let s = RetainedStorage::open(&p).unwrap();
        let sibling = p.with_extension("retained");
        fs::rename(&p, &sibling).unwrap();
        fs::create_dir(&p).unwrap();
        assert!(s.validate().is_err());
        fs::remove_dir(&p).unwrap();
        fs::rename(&sibling, &p).unwrap();
    }
}
