//! Remote files without storing credentials in the application configuration.
//! Every operation runs on its own worker; dropping a job cancels it.
pub mod aws_profiles;
mod ftp;
mod s3_backend;
mod sftp;
pub mod ssh_config;
pub mod ui;

use anyhow::{Context, Result, bail};
use kiln_accounts::CredentialStore;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub endpoint: RemoteEndpoint,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum S3Authentication {
    Default,
    Manual,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteEndpoint {
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
        path_style: bool,
        prefix: String,
        #[serde(default)]
        aws_profile: Option<String>,
        #[serde(default)]
        aws_auth: Option<S3Authentication>,
    },
    Ftp {
        host: String,
        port: u16,
        username: String,
        tls: bool,
        root: String,
    },
    Sftp {
        alias: String,
        config_path: Option<PathBuf>,
        root: String,
    },
}
impl RemoteEndpoint {
    pub fn root(&self) -> &str {
        match self {
            Self::S3 { prefix, .. } => prefix,
            Self::Ftp { root, .. } | Self::Sftp { root, .. } => root,
        }
    }
    pub fn protocol(&self) -> &'static str {
        match self {
            Self::S3 { .. } => "S3",
            Self::Ftp { tls: true, .. } => "FTPS",
            Self::Ftp { .. } => "FTP",
            Self::Sftp { .. } => "SFTP",
        }
    }
}
impl ConnectionProfile {
    pub fn validate(&self) -> Result<()> {
        safe_text(&self.id)?;
        safe_text(&self.name)?;
        if self.id.is_empty() || self.name.trim().is_empty() {
            bail!("A connection name is required");
        }
        match &self.endpoint {
            RemoteEndpoint::S3 {
                bucket,
                region,
                endpoint,
                prefix,
                ..
            } => {
                if bucket.is_empty() || bucket.contains('/') || region.is_empty() {
                    bail!("Bucket and region are required");
                }
                safe_text(bucket)?;
                safe_text(region)?;
                safe_text(prefix)?;
                if let Some(endpoint) = endpoint {
                    let u = url::Url::parse(endpoint)?;
                    if !matches!(u.scheme(), "https" | "http")
                        || u.host_str().is_none()
                        || !u.username().is_empty()
                        || u.password().is_some()
                    {
                        bail!("Use an HTTP(S) endpoint without embedded credentials");
                    }
                }
            }
            RemoteEndpoint::Ftp {
                host,
                port,
                username,
                root,
                ..
            } => {
                safe_text(host)?;
                safe_text(username)?;
                safe_text(root)?;
                if host.is_empty() || *port == 0 || username.is_empty() {
                    bail!("Host, port and username are required");
                }
            }
            RemoteEndpoint::Sftp { alias, root, .. } => {
                ssh_config::validate_alias(alias)?;
                safe_text(root)?;
            }
        }
        Ok(())
    }
}
/// Deliberately not serializable: use `save_secrets`, backed by the OS credential store.
#[derive(Clone, Default)]
pub struct Secrets {
    pub password: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub session_token: Option<String>,
}
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets([redacted])")
    }
}
#[derive(Serialize, Deserialize)]
struct SecretRecord {
    password: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    session_token: Option<String>,
}
pub fn save_secrets(store: &dyn CredentialStore, id: &str, secrets: &Secrets) -> Result<()> {
    safe_text(id)?;
    let value = serde_json::to_string(&SecretRecord {
        password: secrets.password.clone(),
        access_key: secrets.access_key.clone(),
        secret_key: secrets.secret_key.clone(),
        session_token: secrets.session_token.clone(),
    })?;
    store.set("dev.kiln.remote", id, &value)
}
pub fn load_secrets(store: &dyn CredentialStore, id: &str) -> Result<Secrets> {
    let Some(value) = store.get("dev.kiln.remote", id)? else {
        return Ok(Secrets::default());
    };
    let v: SecretRecord =
        serde_json::from_str(&value).context("Invalid saved remote credentials")?;
    Ok(Secrets {
        password: v.password,
        access_key: v.access_key,
        secret_key: v.secret_key,
        session_token: v.session_token,
    })
}
pub fn delete_secrets(store: &dyn CredentialStore, id: &str) -> Result<bool> {
    store.delete("dev.kiln.remote", id)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct ListPage {
    pub entries: Vec<RemoteEntry>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Operation {
    List {
        path: String,
        cursor: Option<String>,
    },
    Stat {
        path: String,
    },
    Download {
        path: String,
        local: PathBuf,
    },
    Upload {
        local: PathBuf,
        path: String,
        overwrite: bool,
    },
    Rename {
        from: String,
        to: String,
        overwrite: bool,
    },
    Delete {
        path: String,
        is_dir: bool,
    },
    CreateDir {
        path: String,
    },
}
#[derive(Debug)]
pub enum RemoteResult {
    Listed(ListPage),
    Entry(RemoteEntry),
    Done,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TransferProgress {
    pub bytes: u64,
    pub total: Option<u64>,
}
#[derive(Default)]
pub(crate) struct Control {
    cancel: AtomicBool,
    bytes: AtomicU64,
    total: AtomicU64,
}
impl Control {
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("Operation cancelled")
        }
        Ok(())
    }
    pub fn advance(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }
    pub fn total(&self, n: u64) {
        self.total.store(n, Ordering::Relaxed);
    }
}
pub struct JobHandle {
    rx: crossbeam_channel::Receiver<Result<RemoteResult>>,
    control: Arc<Control>,
}
impl JobHandle {
    pub fn try_recv(&self) -> Option<Result<RemoteResult>> {
        self.rx.try_recv().ok()
    }
    pub fn cancel(&self) {
        self.control.cancel.store(true, Ordering::Relaxed);
    }
    pub fn progress(&self) -> TransferProgress {
        let total = self.control.total.load(Ordering::Relaxed);
        TransferProgress {
            bytes: self.control.bytes.load(Ordering::Relaxed),
            total: (total > 0).then_some(total),
        }
    }
}
impl Drop for JobHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub fn spawn(profile: ConnectionProfile, secrets: Secrets, operation: Operation) -> JobHandle {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let control = Arc::new(Control::default());
    let c = control.clone();
    std::thread::spawn(move || {
        let result = (|| {
            profile.validate()?;
            c.check()?;
            match profile.endpoint {
                RemoteEndpoint::S3 { .. } => {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    rt.block_on(async {if matches!(operation,Operation::Upload{..}) {s3_backend::run(&profile,&secrets,&operation,&c).await} else {tokio::select! {result=s3_backend::run(&profile,&secrets,&operation,&c)=>result,_=cancelled(&c)=>Err(anyhow::anyhow!("Operation cancelled"))}}})
                }
                RemoteEndpoint::Ftp { .. } => ftp::run(&profile, &secrets, &operation, &c),
                RemoteEndpoint::Sftp { .. } => sftp::run(&profile, &operation, &c),
            }
        })();
        let _ = tx.send(result);
    });
    JobHandle { rx, control }
}
async fn cancelled(c: &Control) {
    loop {
        if c.cancel.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
pub(crate) fn safe_text(value: &str) -> Result<()> {
    if value.chars().any(|c| c == '\0' || c == '\r' || c == '\n') {
        bail!("Names and paths cannot contain control characters")
    }
    Ok(())
}
pub(crate) fn modified_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Utc> = time.into();
    time.format("%Y-%m-%d %H:%M UTC").to_string()
}
pub(crate) fn modified_string(value: String) -> String {
    chrono::DateTime::parse_from_rfc3339(&value)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(&value))
        .map(|v| {
            v.with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M UTC")
                .to_string()
        })
        .unwrap_or(value)
}
pub(crate) fn leaf(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_owned()
}
pub(crate) fn join(path: &str, name: &str) -> String {
    format!("{}/{}", path.trim_end_matches('/'), name)
}
pub(crate) fn sorted(mut entries: Vec<RemoteEntry>) -> Vec<RemoteEntry> {
    entries.retain(|e| e.name != "." && e.name != "..");
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}
pub(crate) fn destination_temp(local: &Path) -> Result<tempfile::NamedTempFile> {
    let parent = local
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    tempfile::NamedTempFile::new_in(parent).context("Cannot create local download file")
}
pub(crate) fn temporary_remote(path: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{path}.kiln-upload-{}-{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_are_separate_and_redacted() {
        let s = Secrets {
            password: Some("private-example".into()),
            ..Default::default()
        };
        assert!(!format!("{s:?}").contains("private-example"));
        let store = kiln_accounts::MemoryStore::new();
        save_secrets(&store, "fixture", &s).unwrap();
        assert_eq!(
            load_secrets(&store, "fixture").unwrap().password,
            s.password
        );
        assert!(delete_secrets(&store, "fixture").unwrap());
    }
    #[test]
    fn control_and_path_validation() {
        let c = Control::default();
        c.cancel.store(true, Ordering::Relaxed);
        assert!(c.check().is_err());
        assert!(safe_text("a\nrm b").is_err());
        assert!(safe_text("한글 이름.txt").is_ok());
    }
    #[test]
    fn sorting_keeps_folder_first() {
        let e = |name: &str, is_dir| RemoteEntry {
            name: name.into(),
            path: name.into(),
            is_dir,
            size: 0,
            modified: None,
        };
        let list = sorted(vec![
            e("z", false),
            e("b", true),
            e("..", true),
            e("a", false),
        ]);
        assert_eq!(
            list.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "z"]
        );
    }
}
