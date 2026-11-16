//! Windows opened-object filesystem. `path` is provenance, never child I/O.
//! See the November 16 spec amendment for the process-restart durability limit.
use super::super::{Budget, MAX_DEPTH, TOTAL_FILES};
use super::{Kind, R, component, io, relative};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{null, null_mut},
    sync::{Arc, Mutex},
};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem::*},
    Win32::{
        Foundation::{
            HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, RtlNtStatusToDosError,
            STATUS_NO_MORE_FILES, STATUS_PENDING,
        },
        Storage::FileSystem::*,
        System::{
            IO::IO_STATUS_BLOCK,
            Threading::{INFINITE, WaitForSingleObject},
        },
    },
};

struct Anchor {
    file: File,
    identity: (u32, u64),
    // A File clone shares its kernel enumeration cursor. Aliases share this Arc,
    // so the lock covers restart, every page, and terminal completion together.
    scan: Mutex<()>,
}
#[derive(Clone)]
pub(in crate::extensions) struct Dir {
    anchor: Arc<Anchor>,
    parents: Vec<Arc<Anchor>>,
    pub path: PathBuf,
}
fn os_error(status: NTSTATUS) -> std::io::Error {
    std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32)
}
fn error(reason: &'static str) -> std::io::Error {
    std::io::Error::other(reason)
}
// All handles are synchronous. Defensively await a pending completion before
// any stack/Vec argument goes out of scope. A failed wait cannot safely return
// while the kernel still borrows those arguments, hence abort in that impossible
// (invalid owned handle) state instead of introducing a use-after-free.
fn complete(status: NTSTATUS, file: HANDLE, block: &IO_STATUS_BLOCK) -> NTSTATUS {
    if status != STATUS_PENDING {
        return status;
    }
    if file.is_null() || unsafe { WaitForSingleObject(file, INFINITE) } != 0 {
        std::process::abort();
    }
    let status = unsafe { block.Anonymous.Status };
    if status == STATUS_PENDING {
        std::process::abort();
    }
    status
}
fn checked(status: NTSTATUS, file: HANDLE, block: &IO_STATUS_BLOCK) -> std::io::Result<()> {
    let status = complete(status, file, block);
    if status < 0 {
        Err(os_error(status))
    } else {
        Ok(())
    }
}
fn information(file: &File) -> std::io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
        return Err(error("SPECIAL_FILE"));
    }
    Ok(info)
}
fn identity(info: &BY_HANDLE_FILE_INFORMATION) -> (u32, u64) {
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}
fn kind(file: &File) -> std::io::Result<Kind> {
    let info = information(file)?;
    Ok(
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            Kind::Link
        } else if info.dwFileAttributes & FILE_ATTRIBUTE_DEVICE != 0 {
            Kind::Special
        } else if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            Kind::Dir
        } else {
            Kind::File
        },
    )
}
impl Anchor {
    fn new(file: File) -> std::io::Result<Arc<Self>> {
        let info = information(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(error("REPARSE_POINT"));
        }
        Ok(Arc::new(Self {
            file,
            identity: identity(&info),
            scan: Mutex::new(()),
        }))
    }
    fn validate(&self) -> std::io::Result<()> {
        let info = information(&self.file)?;
        if identity(&info) != self.identity
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(error("ROOT_CHANGED_DURING_RECONCILE"));
        }
        Ok(())
    }
}

// Only a single already-validated leaf is passed to the object manager. No
// slash, alternate stream, absolute path, device alias, dot or parent escape.
fn open_leaf(
    parent: &File,
    name: &str,
    access: u32,
    sharing: u32,
    disposition: u32,
    options: u32,
) -> std::io::Result<File> {
    component(name).map_err(error)?;
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let len = u16::try_from(name.len() * 2).map_err(|_| error("INVALID_ASSET_PATH"))?;
    let mut unicode = windows_sys::Win32::Foundation::UNICODE_STRING {
        Length: len,
        MaximumLength: len,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut unicode,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: null_mut(),
        SecurityQualityOfService: null_mut(),
    };
    let mut raw = null_mut();
    let mut block = IO_STATUS_BLOCK::default();
    // SAFETY: typed structures and UTF-16 storage remain live through completion;
    // no OBJ_INHERIT, no async flag, and no caller-controlled compound path.
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &attributes,
            &mut block,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            sharing,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    let status = complete(status, raw, &block);
    if status < 0 {
        return Err(os_error(status));
    }
    if raw.is_null() {
        return Err(error("FILESYSTEM_ACCESS"));
    }
    let file = unsafe { File::from_raw_handle(raw) };
    if kind(&file)? == Kind::Link {
        return Err(error("REPARSE_POINT"));
    }
    Ok(file)
}

impl Dir {
    pub fn open(path: &Path) -> R<Self> {
        // Reject namespace/device forms before normalization can hide components.
        if !path.is_absolute() {
            return Err("INVALID_ASSET_PATH");
        }
        if path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err("INVALID_ASSET_PATH");
        }
        let path = io(std::path::absolute(path))?;
        let mut parts = path.components();
        let prefix = match parts.next() {
            Some(Component::Prefix(p)) => p.kind(),
            _ => return Err("INVALID_ASSET_PATH"),
        };
        let root = match prefix {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) if d.is_ascii_alphabetic() => {
                PathBuf::from(format!("{}:\\", d as char))
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                for name in [server, share] {
                    component(name.to_str().ok_or("INVALID_ASSET_PATH")?)?;
                }
                PathBuf::from(format!(
                    "\\\\{}\\{}\\",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ))
            }
            _ => return Err("INVALID_ASSET_PATH"),
        };
        if !matches!(parts.next(), Some(Component::RootDir)) {
            return Err("INVALID_ASSET_PATH");
        }
        // This is the only pathname open. It names a drive/UNC namespace root,
        // never a user-selected descendant. Every ancestor after it is rooted.
        let file = io(OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&root))?;
        let mut dir = Self {
            anchor: io(Anchor::new(file))?,
            parents: Vec::new(),
            path: root,
        };
        for part in parts {
            let Component::Normal(name) = part else {
                return Err("INVALID_ASSET_PATH");
            };
            dir = dir.dir(name.to_str().ok_or("INVALID_ASSET_PATH")?, false)?;
        }
        Ok(dir)
    }
    fn validate(&self) -> std::io::Result<()> {
        for parent in &self.parents {
            parent.validate()?;
        }
        self.anchor.validate()
    }
    fn child(
        &self,
        name: &str,
        access: u32,
        share: u32,
        disposition: u32,
        options: u32,
    ) -> std::io::Result<File> {
        self.validate()?;
        #[cfg(test)]
        super::windows_tests::after_validate();
        let file = open_leaf(&self.anchor.file, name, access, share, disposition, options)?;
        self.validate()?;
        Ok(file)
    }
    fn with_directory(&self, file: File, name: &str) -> R<Self> {
        let mut parents = self.parents.clone();
        parents.push(self.anchor.clone());
        Ok(Self {
            anchor: io(Anchor::new(file))?,
            parents,
            path: self.path.join(name),
        })
    }
    pub fn dir(&self, name: &str, create: bool) -> R<Self> {
        component(name)?;
        let file = io(self.child(
            name,
            FILE_LIST_DIRECTORY | FILE_TRAVERSE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            if create { FILE_OPEN_IF } else { FILE_OPEN },
            FILE_DIRECTORY_FILE,
        ))?;
        let dir = self.with_directory(file, name)?;
        if create {
            self.sync()?;
        }
        Ok(dir)
    }
    pub fn parent(&self, rel: &str, create: bool) -> R<(Self, String)> {
        relative(rel)?;
        let mut parts = rel.split('/').collect::<Vec<_>>();
        let leaf = parts.pop().unwrap().to_owned();
        let mut dir = self.clone();
        for part in parts {
            dir = dir.dir(part, create)?;
        }
        Ok((dir, leaf))
    }
    pub fn revalidate(&self) -> R<()> {
        self.validate().map_err(|_| "ROOT_CHANGED_DURING_RECONCILE")
    }
    pub fn private(&self) -> R<()> {
        self.revalidate()
    }
    pub fn sync(&self) -> R<()> {
        #[cfg(test)]
        if super::TEST_SYNC_FAULT.with(|t| {
            t.borrow()
                .as_ref()
                .is_some_and(|s| self.path.to_string_lossy().ends_with(s))
        }) {
            return Err("DIRECTORY_SYNC_FAILED");
        }
        self.revalidate()?;
        // Namespace ordering point only. Windows process-restart recovery keeps
        // file/journal FlushFileBuffers; no power-loss directory-fsync claim.
        #[cfg(test)]
        super::TEST_SYNC.with(|t| {
            t.borrow_mut()
                .push(self.path.to_string_lossy().into_owned())
        });
        Ok(())
    }
    pub fn sync_tree(&self) -> R<()> {
        self.sync_tree_at(0, &Budget::new())
    }
    fn sync_tree_at(&self, depth: usize, budget: &Budget) -> R<()> {
        budget.check()?;
        if depth > MAX_DEPTH {
            return Err("DEPTH_16");
        }
        for name in self.list()? {
            if self.kind(&name)? == Some(Kind::Dir) {
                self.dir(&name, false)?.sync_tree_at(depth + 1, budget)?;
            }
        }
        self.sync()
    }
    pub fn kind(&self, name: &str) -> R<Option<Kind>> {
        component(name)?;
        #[cfg(test)]
        if super::TEST_KIND_FAULT.with(|t| {
            t.borrow()
                .as_ref()
                .is_some_and(|p| *p == self.path.join(name))
        }) {
            return Err("FILESYSTEM_ACCESS");
        }
        match self.child(
            name,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
        ) {
            Ok(file) => io(kind(&file)).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("FILESYSTEM_ACCESS_OR_LINK"),
        }
    }
    pub fn read(&self, name: &str, limit: usize) -> R<(Vec<u8>, bool)> {
        component(name)?;
        let file = io(self.child(
            name,
            FILE_READ_DATA,
            FILE_SHARE_READ,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        ))?;
        #[cfg(test)]
        super::windows_tests::after_read_open();
        let result = read_file(&file, limit)?;
        self.revalidate()?;
        Ok((result, false))
    }
    pub fn read_private(&self, name: &str, limit: usize) -> R<Vec<u8>> {
        self.read(name, limit).map(|v| v.0)
    }
    pub fn write_new(&self, name: &str, bytes: &[u8], _exec: bool) -> R<()> {
        component(name)?;
        let mut file = io(self.child(
            name,
            FILE_WRITE_DATA,
            FILE_SHARE_READ,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
        ))?;
        io(file.write_all(bytes))?;
        io(file.sync_all())?;
        self.revalidate()
    }
    pub fn lock(&self, name: &str) -> R<File> {
        component(name)?;
        let file = io(self.child(
            name,
            FILE_READ_DATA | FILE_WRITE_DATA,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        ))?;
        file.try_lock().map_err(|_| "PROFILE_RECONCILE_BUSY")?;
        Ok(file)
    }
    pub fn list(&self) -> R<Vec<String>> {
        let _cursor = self
            .anchor
            .scan
            .lock()
            .map_err(|_| "DIRECTORY_ENUMERATION_FAILED")?;
        self.revalidate()?;
        #[cfg(test)]
        super::windows_tests::after_validate();
        // u64 storage guarantees FILE_DIRECTORY_INFORMATION's 8-byte alignment.
        // 64 KiB holds every legal component; overflow is an error, never a
        // truncated inventory. No growth/retry can accidentally skip records.
        let mut storage = vec![0u64; 8192];
        let mut out = Vec::new();
        let mut first = true;
        let budget = Budget::new();
        loop {
            budget.check()?;
            #[cfg(test)]
            if super::TEST_READDIR_FAULT.with(|t| {
                t.borrow()
                    .as_ref()
                    .is_some_and(|(p, n)| p == &self.path && out.len() >= *n)
            }) {
                return Err("DIRECTORY_ENUMERATION_FAILED");
            }
            let mut block = IO_STATUS_BLOCK::default();
            let status = unsafe {
                NtQueryDirectoryFile(
                    self.anchor.file.as_raw_handle(),
                    null_mut(),
                    None,
                    null(),
                    &mut block,
                    storage.as_mut_ptr().cast(),
                    (storage.len() * 8) as u32,
                    FileDirectoryInformation,
                    false,
                    null(),
                    first,
                )
            };
            let status = complete(status, self.anchor.file.as_raw_handle(), &block);
            first = false;
            if !scan_page(status, block.Information, storage.len() * 8)? {
                break;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), block.Information)
            };
            out.extend(parse_records(bytes)?);
            if out.len() > TOTAL_FILES {
                return Err("TOTAL_FILES_16384");
            }
            self.revalidate()?;
        }
        self.revalidate()?;
        out.sort();
        if out.windows(2).any(|w| w[0] == w[1]) {
            return Err("DIRECTORY_CHANGED_DURING_SCAN");
        }
        Ok(out)
    }
    #[allow(dead_code)] // Kept as the common filesystem interface; owned publication uses rename_checked.
    pub fn rename(&self, from: &str, to: &Dir, name: &str) -> R<()> {
        self.rename_impl(from, Some(to), name, false)
            .map_err(|_| "ATOMIC_PUBLICATION_FAILED")?;
        #[cfg(test)]
        super::TEST_SYNC.with(|t| t.borrow_mut().push(format!("publish:{name}")));
        self.sync()?;
        to.sync()
    }
    // Ownership is checked against the same DELETE-capable object that moves.
    // A name replacement between the caller's fingerprint and acquisition must
    // not turn a foreign object into the journal backup.
    pub fn rename_checked(&self, from: &str, to: &Dir, name: &str, expected: &str) -> R<()> {
        component(from)?;
        component(name)?;
        #[cfg(test)]
        super::windows_tests::before_mutation();
        let file = io(self.child(from, DELETE | FILE_READ_DATA, FILE_SHARE_READ, FILE_OPEN, 0))?;
        let object = if io(kind(&file))? == Kind::Dir {
            let dir = self.with_directory(file, from)?;
            let fingerprint = super::super::scan::snapshot_open_directory(&dir)?.fingerprint();
            if fingerprint != expected {
                return Err("CONCURRENT_PROFILE_EDIT");
            }
            OwnedSource::Directory(dir)
        } else {
            let fingerprint =
                super::super::Snapshot::one(read_file(&file, super::super::FILE_BYTES)?)
                    .fingerprint();
            if fingerprint != expected {
                return Err("CONCURRENT_PROFILE_EDIT");
            }
            OwnedSource::File(file)
        };
        self.revalidate()?;
        to.revalidate()?;
        #[cfg(test)]
        super::windows_tests::before_rename();
        io(rename_handle(
            object.file(),
            Some(&to.anchor.file),
            name,
            false,
        ))?;
        self.sync()?;
        to.sync()
    }
    #[cfg(test)]
    pub(in crate::extensions) fn rename_windows(
        &self,
        from: &str,
        to: &Dir,
        name: &str,
    ) -> Result<(), (&'static str, std::io::Error)> {
        self.rename_windows_native(from, Some(to), name)
    }
    #[cfg(test)]
    pub(in crate::extensions) fn rename_windows_native(
        &self,
        from: &str,
        to: Option<&Dir>,
        name: &str,
    ) -> Result<(), (&'static str, std::io::Error)> {
        self.rename_impl(from, to, name, true)
            .map_err(|e| ("rooted native rename", e))
    }
    fn rename_impl(
        &self,
        from: &str,
        to: Option<&Dir>,
        name: &str,
        replace: bool,
    ) -> std::io::Result<()> {
        component(from)
            .and_then(|_| component(name))
            .map_err(error)?;
        let to_dir = to.unwrap_or(self);
        to_dir.validate()?;
        let source = self.child(
            from,
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
        )?;
        // Source is the acquired object until completion. No reopening its name.
        self.validate()?;
        to_dir.validate()?;
        #[cfg(test)]
        super::windows_tests::before_rename();
        rename_handle(&source, to.map(|d| &d.anchor.file), name, replace)?;
        self.validate()?;
        to_dir.validate()
    }
    pub fn atomic(&self, name: &str, bytes: &[u8]) -> R<()> {
        component(name)?;
        let temp = format!("write-{}", super::super::nonce());
        self.write_new(&temp, bytes, false)?;
        // Only private state/report atomic files intentionally replace. Assets
        // use rename()'s expected-empty destination, including rollback.
        if self.rename_impl(&temp, Some(self), name, true).is_err() {
            let _ = self.remove(&temp);
            return Err("ATOMIC_PUBLICATION_FAILED");
        }
        #[cfg(test)]
        super::TEST_SYNC.with(|t| t.borrow_mut().push(format!("publish:{name}")));
        self.sync()
    }
    pub fn remove(&self, name: &str) -> R<()> {
        self.remove_impl(name, None)
    }
    pub fn remove_checked(&self, name: &str, expected: &str) -> R<()> {
        self.remove_impl(name, Some(expected))
    }
    fn remove_impl(&self, name: &str, expected: Option<&str>) -> R<()> {
        component(name)?;
        #[cfg(test)]
        super::windows_tests::before_mutation();
        let file = match self.child(name, DELETE | FILE_READ_DATA, FILE_SHARE_READ, FILE_OPEN, 0) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("FILESYSTEM_ACCESS_OR_LINK"),
        };
        let mut count = 0;
        let budget = Budget::new();
        // Acquire the whole selected tree before the first deletion. No duplicate
        // no-delete-sharing ordinary anchor competes with its DELETE handle.
        let tree = Mutation::acquire(self, name, file, 0, &mut count, &budget)?;
        if let Some(expected) = expected {
            if tree.snapshot(&budget)?.fingerprint() != expected {
                return Err("CONCURRENT_PROFILE_EDIT");
            }
        }
        #[cfg(test)]
        super::windows_tests::before_remove();
        tree.remove(&budget)?;
        self.sync()?;
        #[cfg(test)]
        super::TEST_SYNC.with(|t| t.borrow_mut().push(format!("remove:{name}")));
        Ok(())
    }
}
enum OwnedSource {
    File(File),
    Directory(Dir),
}
impl OwnedSource {
    fn file(&self) -> &File {
        match self {
            Self::File(f) => f,
            Self::Directory(d) => &d.anchor.file,
        }
    }
}
fn read_file(file: &File, limit: usize) -> R<Vec<u8>> {
    if io(kind(file))? != Kind::File {
        return Err("SPECIAL_FILE");
    }
    let before = io(file.metadata())?;
    if before.len() > limit as u64 {
        return Err("FILE_SIZE_LIMIT");
    }
    let mut bytes = Vec::new();
    io(file
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes))?;
    let after = io(file.metadata())?;
    if bytes.len() > limit
        || before.len() != after.len()
        || io(before.modified())? != io(after.modified())?
    {
        return Err("SOURCE_CHANGED_DURING_READ");
    }
    Ok(bytes)
}
fn rename_handle(
    source: &File,
    to: Option<&File>,
    name: &str,
    replace: bool,
) -> std::io::Result<()> {
    if let Some(to) = to {
        if identity(&information(source)?).0 != identity(&information(to)?).0 {
            return Err(error("CROSS_VOLUME_PUBLICATION"));
        }
    }
    let leaf: Vec<u16> = name.encode_utf16().collect();
    let bytes = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName) + leaf.len() * 2;
    let mut storage = vec![
        0usize;
        bytes
            .max(size_of::<FILE_RENAME_INFORMATION>())
            .div_ceil(size_of::<usize>())
    ];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut block = IO_STATUS_BLOCK::default();
    let status = unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory = to.map_or(null_mut(), |d| d.as_raw_handle());
        (*info).FileNameLength = (leaf.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            leaf.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast(),
            leaf.len(),
        );
        NtSetInformationFile(
            source.as_raw_handle(),
            &mut block,
            info.cast(),
            bytes as u32,
            FileRenameInformation,
        )
    };
    checked(status, source.as_raw_handle(), &block)
}
fn dispose(file: File) -> R<()> {
    let info = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    let mut block = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut block,
            (&info as *const FILE_DISPOSITION_INFORMATION).cast(),
            size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
            FileDispositionInformation,
        )
    };
    io(checked(status, file.as_raw_handle(), &block))?;
    // Ordinary disposition completes on close. Consume, never query after mark.
    drop(file);
    Ok(())
}
enum Mutation {
    File(File),
    Directory(Dir, Vec<(String, Mutation)>),
}
impl Mutation {
    fn acquire(
        parent: &Dir,
        name: &str,
        file: File,
        depth: usize,
        count: &mut usize,
        budget: &Budget,
    ) -> R<Self> {
        budget.check()?;
        if depth > MAX_DEPTH {
            return Err("DEPTH_16");
        }
        *count += 1;
        if *count > TOTAL_FILES {
            return Err("TOTAL_FILES_16384");
        }
        match io(kind(&file))? {
            Kind::File => Ok(Self::File(file)),
            Kind::Dir => {
                let dir = parent.with_directory(file, name)?;
                let mut children = Vec::new();
                for child in dir.list()? {
                    let file = io(dir.child(
                        &child,
                        DELETE | FILE_READ_DATA,
                        FILE_SHARE_READ,
                        FILE_OPEN,
                        0,
                    ))?;
                    children.push((
                        child.clone(),
                        Self::acquire(&dir, &child, file, depth + 1, count, budget)?,
                    ));
                }
                Ok(Self::Directory(dir, children))
            }
            _ => Err("REPARSE_POINT"),
        }
    }
    fn snapshot(&self, budget: &Budget) -> R<super::super::Snapshot> {
        let mut out = super::super::Snapshot {
            directory: matches!(self, Self::Directory(..)),
            files: vec![],
            dirs: vec![],
        };
        let mut bytes = 0;
        self.collect("", &mut out, &mut bytes, budget)?;
        Ok(out)
    }
    fn collect(
        &self,
        prefix: &str,
        out: &mut super::super::Snapshot,
        bytes: &mut usize,
        budget: &Budget,
    ) -> R<()> {
        budget.check()?;
        match self {
            Self::File(file) => {
                let data = read_file(file, super::super::FILE_BYTES)?;
                *bytes += data.len();
                if *bytes > super::super::TOTAL_BYTES {
                    return Err("TOTAL_BYTES_268435456");
                }
                out.files.push(super::super::FileData {
                    path: prefix.to_owned(),
                    bytes: data,
                    exec: false,
                });
            }
            Self::Directory(dir, children) => {
                dir.revalidate()?;
                for (name, child) in children {
                    let path = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}/{name}")
                    };
                    if matches!(child, Self::Directory(..)) {
                        out.dirs.push(path.clone());
                    }
                    child.collect(&path, out, bytes, budget)?;
                }
            }
        }
        Ok(())
    }
    fn remove(self, budget: &Budget) -> R<()> {
        budget.check()?;
        match self {
            Self::File(file) => dispose(file),
            Self::Directory(dir, children) => {
                dir.revalidate()?;
                for (_, child) in children {
                    child.remove(budget)?;
                }
                dir.revalidate()?;
                // Descendant parent Arcs have all closed. Exactly one owner of
                // the mutation-root remains; no duplicate handle delays delete.
                let anchor =
                    Arc::try_unwrap(dir.anchor).map_err(|_| "MUTATION_HANDLE_STILL_SHARED")?;
                dispose(anchor.file)
            }
        }
    }
}

pub(super) fn scan_page(status: NTSTATUS, bytes: usize, capacity: usize) -> R<bool> {
    if bytes > capacity {
        return Err("DIRECTORY_ENUMERATION_FAILED");
    }
    if status == STATUS_NO_MORE_FILES && bytes == 0 {
        return Ok(false);
    }
    if status != 0 || bytes == 0 {
        return Err("DIRECTORY_ENUMERATION_FAILED");
    }
    Ok(true)
}

pub(super) fn parse_records(bytes: &[u8]) -> R<Vec<String>> {
    let header = std::mem::offset_of!(FILE_DIRECTORY_INFORMATION, FileName);
    let length_offset = std::mem::offset_of!(FILE_DIRECTORY_INFORMATION, FileNameLength);
    let mut pos = 0;
    let mut out = Vec::new();
    loop {
        let record = bytes.get(pos..).ok_or("DIRECTORY_ENUMERATION_FAILED")?;
        if record.len() < header {
            return Err("DIRECTORY_ENUMERATION_FAILED");
        }
        let next = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(record[length_offset..length_offset + 4].try_into().unwrap())
            as usize;
        let extent = if next == 0 { record.len() } else { next };
        if len == 0
            || len % 2 != 0
            || extent > record.len()
            || extent < header
            || len > extent - header
            || (next != 0 && next % 8 != 0)
            || (next == 0 && extent - header - len > 7)
        {
            return Err("DIRECTORY_ENUMERATION_FAILED");
        }
        let units = record[header..header + len]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        let name = String::from_utf16(&units).map_err(|_| "INVALID_ASSET_PATH")?;
        if name != "." && name != ".." {
            component(&name)?;
            out.push(name);
        }
        if next == 0 {
            return Ok(out);
        }
        pos = pos
            .checked_add(next)
            .ok_or("DIRECTORY_ENUMERATION_FAILED")?;
    }
}
