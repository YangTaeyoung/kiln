//! Per-launch capability; never serialize this state into a public DTO.
use anyhow::{Result, ensure};
use ring::rand::{SecureRandom, SystemRandom};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs::{self},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub struct Auth {
    token: String,
    expires: Instant,
    pub expires_unix: u64,
}
impl Auth {
    pub fn new(ttl: Duration) -> Result<Self> {
        ensure!(
            !ttl.is_zero() && ttl <= Duration::from_secs(8 * 3600),
            "invalid token lifetime"
        );
        let mut random = [0u8; 32];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| anyhow::anyhow!("randomness unavailable"))?;
        let token = random.iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self {
            token,
            expires: Instant::now() + ttl,
            expires_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + ttl.as_secs(),
        })
    }
    pub fn accepts(&self, authorization: Option<&str>) -> bool {
        let Some(value) = authorization.and_then(|v| v.strip_prefix("Bearer ")) else {
            return false;
        };
        if Instant::now() >= self.expires || value.len() != self.token.len() {
            return false;
        }
        value
            .bytes()
            .zip(self.token.bytes())
            .fold(0u8, |a, (b, c)| a | (b ^ c))
            == 0
    }
    pub fn pairing(&self, path: &Path, url: &str) -> Result<PairingGuard> {
        self.pairing_before_write(path, url, || {})
    }
    fn pairing_before_write(
        &self,
        path: &Path,
        url: &str,
        before_write: impl FnOnce(),
    ) -> Result<PairingGuard> {
        let parent = path
            .parent()
            .filter(|p| p.is_absolute())
            .ok_or_else(|| anyhow::anyhow!("pairing path must be absolute"))?;
        if !parent.exists() {
            let mut b = fs::DirBuilder::new();
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
            b.create(parent)?;
        }
        let canonical = fs::canonicalize(parent)?;
        ensure!(
            fs::symlink_metadata(parent)?.is_dir(),
            "pairing directory must not be a symlink"
        );
        let dir = crate::metadata::directory(&canonical)?;
        let m = dir.metadata()?;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.permissions().mode() & 0o777 == 0o700,
            "pairing directory must be owner-private (0700)"
        );
        let name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("invalid pairing filename"))?;
        let name = std::ffi::CString::new(name.as_bytes())?;
        let mut random = [0u8; 16];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| anyhow::anyhow!("randomness unavailable"))?;
        let temporary = std::ffi::CString::new(format!(
            ".kiln-pairing-{}.tmp",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ))?;
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        ensure!(fd >= 0, "pairing file exists or is unavailable");
        let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
        let m = f.metadata()?;
        let identity = (m.dev(), m.ino());
        struct Staging<'a> {
            dir: &'a std::fs::File,
            name: &'a std::ffi::CStr,
        }
        impl Drop for Staging<'_> {
            fn drop(&mut self) {
                unsafe {
                    libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0);
                }
            }
        }
        let _staging = Staging {
            dir: &dir,
            name: &temporary,
        };
        before_write();
        f.write_all(&serde_json::to_vec(&serde_json::json!({"version":1,"url":url,"token":self.token,"expires_unix":self.expires_unix}))?)?;
        f.sync_all()?;
        // linkat publishes the complete private inode atomically and never clobbers an existing target.
        ensure!(
            unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    temporary.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            } == 0,
            "pairing file exists or is unavailable"
        );
        let guard = PairingGuard {
            path: path.into(),
            identity,
        };
        dir.sync_all()?;
        Ok(guard)
    }
}
pub struct PairingGuard {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for PairingGuard {
    fn drop(&mut self) {
        if let Ok(m) = fs::symlink_metadata(&self.path) {
            if (m.dev(), m.ino()) == self.identity {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairing_is_complete_before_visible_and_failure_cleans_staging() {
        let root = tempfile::tempdir().unwrap();
        let directory = std::fs::canonicalize(root.path()).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("pairing.json");
        let auth = Auth::new(Duration::from_secs(60)).unwrap();
        let guard = auth
            .pairing_before_write(&path, "http://127.0.0.1:1234", || {
                assert!(
                    !path.exists(),
                    "partial final pairing file is visible before write"
                );
            })
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(payload["version"], 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let original = fs::read(&path).unwrap();
        assert!(auth.pairing(&path, "http://127.0.0.1:9999").is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            1,
            "failed no-clobber publication leaked staging"
        );
        drop(guard);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    }
}
