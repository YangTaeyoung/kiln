//! No-follow, bounded descriptor projection. Authentication is handled before callers enter here.
use anyhow::{Result, ensure};
use kiln::app::agent_gui::Descriptor;
use serde::Deserialize;
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::fs::OpenOptionsExt,
};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
};
pub fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
/// Traverse every component relative to a retained directory fd, refusing symlinks.
pub fn directory(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "absolute directory required");
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open("/")?;
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(name) => f = openat_dir(&f, name)?,
            _ => anyhow::bail!("invalid directory"),
        }
    }
    Ok(f)
}
fn openat_dir(parent: &File, name: &OsStr) -> Result<File> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub fn read_descriptor(root: &Path, id: &str) -> Result<Descriptor> {
    ensure!(valid_id(id), "invalid conversation ID");
    let parent = directory(root)?;
    let dir = openat_dir(&parent, OsStr::new(id))?;
    let name = c"config.json";
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    ensure!(fd >= 0, "metadata unavailable");
    let file = unsafe { File::from_raw_fd(fd) };
    let m = file.metadata()?;
    ensure!(m.is_file() && m.len() <= 2 * 1024 * 1024, "metadata bound");
    let mut bytes = vec![];
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 2 * 1024 * 1024, "metadata bound");
    #[derive(Deserialize)]
    struct Projection {
        descriptor: Descriptor,
    }
    let d: Projection = serde_json::from_slice(&bytes)?;
    ensure!(d.descriptor.id == id, "descriptor ID mismatch");
    Ok(d.descriptor)
}
#[derive(Clone)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub identity: PathIdentity,
}
pub fn workspaces(paths: &[PathBuf]) -> Result<Vec<Workspace>> {
    ensure!(
        !paths.is_empty() && paths.len() <= 64,
        "enable one to 64 workspaces"
    );
    let mut out = vec![];
    for path in paths {
        let p = std::fs::canonicalize(path)?;
        directory(&p)?;
        ensure!(
            !out.iter().any(|w: &Workspace| w.path == p),
            "duplicate workspace"
        );
        out.push(Workspace {
            id: format!("workspace-{}", out.len() + 1),
            name: p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .take(256)
                .collect(),
            identity: PathIdentity::capture(&p)?,
            path: p,
        });
    }
    Ok(out)
}
pub fn allowed<'a>(workspaces: &'a [Workspace], cwd: &Path) -> Option<&'a Workspace> {
    // Canonical path AND no-follow traversal: a replaced folder cannot silently retarget scope.
    directory(cwd).ok()?;
    let p = std::fs::canonicalize(cwd).ok()?;
    workspaces
        .iter()
        .find(|w| p.starts_with(&w.path) && w.identity.validate().is_ok())
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PathIdentity {
    pub path: PathBuf,
    device: u64,
    inode: u64,
}
impl PathIdentity {
    pub fn capture(path: &Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let f = directory(path)?;
        let m = f.metadata()?;
        Ok(Self {
            path: path.into(),
            device: m.dev(),
            inode: m.ino(),
        })
    }
    pub fn validate(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let f = directory(&self.path)?;
        let m = f.metadata()?;
        ensure!(
            m.dev() == self.device && m.ino() == self.inode,
            "workspace identity changed"
        );
        Ok(())
    }
}
