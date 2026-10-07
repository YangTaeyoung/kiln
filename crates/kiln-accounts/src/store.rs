//! 비밀값 저장소. macOS 는 `security` CLI 로 키체인 generic password 를,
//! 그 밖의 OS 는 0600 평문 파일을 다룬다. 테스트는 [`MemoryStore`] 를 쓴다.

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// generic password 읽기·쓰기·삭제. 값이 없으면 `Ok(None)`.
pub trait CredentialStore: Send + Sync {
    /// service + account 로 비밀값을 읽는다.
    fn get(&self, service: &str, account: &str) -> Result<Option<String>>;
    /// service 만으로 읽어 (비밀값, account 속성)을 돌려준다.
    fn get_by_service(&self, service: &str) -> Result<Option<(String, String)>>;
    fn set(&self, service: &str, account: &str, value: &str) -> Result<()>;
    /// 지웠으면 true, 원래 없었으면 false.
    fn delete(&self, service: &str, account: &str) -> Result<bool>;
}

/// 프로세스 메모리에만 두는 저장소.
#[derive(Default)]
pub struct MemoryStore {
    items: Mutex<HashMap<(String, String), String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.lock().is_empty()
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        Ok(self.items.lock().get(&(service.to_string(), account.to_string())).cloned())
    }

    fn get_by_service(&self, service: &str) -> Result<Option<(String, String)>> {
        let items = self.items.lock();
        let mut hits: Vec<_> = items.iter().filter(|((s, _), _)| s == service).collect();
        hits.sort_by(|a, b| a.0.1.cmp(&b.0.1));
        Ok(hits.first().map(|((_, a), v)| ((*v).clone(), a.clone())))
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<()> {
        self.items.lock().insert((service.to_string(), account.to_string()), value.to_string());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        Ok(self.items.lock().remove(&(service.to_string(), account.to_string())).is_some())
    }
}

/// `/usr/bin/security` 를 호출하는 macOS 키체인 구현.
/// 테스트 빌드에서는 `KILN_ACCOUNTS_REAL_KEYCHAIN=1` 이 없으면 호출을 거부한다.
pub struct SecurityCli;

const REAL_KEYCHAIN_OPT_IN: &str = "KILN_ACCOUNTS_REAL_KEYCHAIN";

fn security(args: &[&str]) -> Result<Option<String>> {
    if cfg!(test) && std::env::var(REAL_KEYCHAIN_OPT_IN).as_deref() != Ok("1") {
        bail!("real keychain access is disabled in tests (set {REAL_KEYCHAIN_OPT_IN}=1)");
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .context("failed to run /usr/bin/security")?;
    security_result(args.first().copied().unwrap_or_default(), out.status.success(), out.status.code(), &out.stdout, &out.stderr)
}

fn security_result(operation: &str, success: bool, code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Result<Option<String>> {
    if success { return Ok(Some(String::from_utf8_lossy(stdout).into_owned())); }
    let msg = String::from_utf8_lossy(stderr).trim().to_string();
    // security returns errSecItemNotFound (-25300) as exit status 44. Only an
    // absent item in a lookup/delete is optional; a missing keychain/write is not.
    if matches!(operation, "find-generic-password" | "delete-generic-password")
        && code == Some(44) && msg.contains("The specified item could not be found in the keychain.") {
        return Ok(None);
    }
    bail!("security {operation} failed: {msg}")
}

fn trim_newline(s: String) -> String {
    s.trim_end_matches('\n').to_string()
}

/// `find-generic-password` 메타 출력에서 `"acct"<blob>="..."` 값을 꺼낸다.
pub fn parse_keychain_account(meta: &str) -> String {
    let key = "\"acct\"<blob>=\"";
    let Some(start) = meta.find(key).map(|i| i + key.len()) else {
        return String::new();
    };
    let mut out = String::new();
    let mut chars = meta[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push('\\');
                    out.push(n);
                }
            }
            '"' => return out,
            c => out.push(c),
        }
    }
    String::new()
}

impl CredentialStore for SecurityCli {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        Ok(security(&["find-generic-password", "-s", service, "-a", account, "-w"])?.map(trim_newline))
    }

    fn get_by_service(&self, service: &str) -> Result<Option<(String, String)>> {
        let Some(meta) = security(&["find-generic-password", "-s", service])? else {
            return Ok(None);
        };
        let Some(value) = security(&["find-generic-password", "-s", service, "-w"])? else {
            return Ok(None);
        };
        Ok(Some((trim_newline(value), parse_keychain_account(&meta))))
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<()> {
        security(&["add-generic-password", "-U", "-s", service, "-a", account, "-w", value]).map(|_| ())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        Ok(security(&["delete-generic-password", "-s", service, "-a", account])?.is_some())
    }
}

/// `base_dir/<service>/<account>` 0600 파일에 비밀값을 두는 저장소(Linux/Windows).
pub struct FileStore {
    pub base_dir: PathBuf,
}

/// 경로 구분자와 제어문자를 `_` 로 바꾼다. 빈 값과 `.`/`..` 는 거부한다.
fn sanitize(name: &str) -> Result<String> {
    if name.is_empty() {
        bail!("empty name");
    }
    let cleaned: String = name.chars().map(|c| if c == '/' || c == '\\' || (c as u32) < 0x20 { '_' } else { c }).collect();
    if cleaned == "." || cleaned == ".." {
        bail!("invalid name {name:?}");
    }
    Ok(cleaned)
}

impl FileStore {
    fn path(&self, service: &str, account: &str) -> Result<PathBuf> {
        Ok(self.base_dir.join(sanitize(service)?).join(sanitize(account)?))
    }
}

impl CredentialStore for FileStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>> {
        read_secret(&self.path(service, account)?)
    }

    fn get_by_service(&self, service: &str) -> Result<Option<(String, String)>> {
        let dir = self.base_dir.join(sanitize(service)?);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(None);
        };
        let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for n in names {
            if let Some(v) = read_secret(&dir.join(&n))? {
                return Ok(Some((v, n)));
            }
        }
        Ok(None)
    }

    fn set(&self, service: &str, account: &str, value: &str) -> Result<()> {
        write_secret_atomic(&self.path(service, account)?, value.as_bytes())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool> {
        match std::fs::remove_file(self.path(service, account)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// 파일 내용을 통째로 읽는다. 없으면 `Ok(None)`.
pub fn read_secret(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::from(e).context(format!("failed to read {}", path.display()))),
    }
}

/// 같은 디렉토리의 임시 파일(0600)에 쓰고 fsync 한 뒤 rename 한다. 디렉토리는 0700 으로 만든다.
pub fn write_secret_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        set_mode(dir, 0o700);
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("secret");
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = dir.join(format!(".{name}.kiln-{}-{nonce}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut f = open_private(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_missing_items_are_optional_and_failed_writes_never_succeed() {
        let absent = b"security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.";
        for operation in ["find-generic-password", "delete-generic-password"] {
            assert_eq!(security_result(operation, false, Some(44), b"", absent).unwrap(), None);
            assert!(security_result(operation, false, Some(1), b"", b"A default keychain could not be found.").is_err());
            assert!(security_result(operation, false, Some(44), b"", b"Keychain could not be found.").is_err());
            assert!(security_result(operation, false, Some(51), b"", b"User interaction is not allowed.").is_err());
        }
        assert!(security_result("add-generic-password", false, Some(44), b"", absent).is_err());
        assert!(security_result("add-generic-password", false, Some(1), b"", b"A default keychain could not be found.").is_err());
        assert_eq!(security_result("add-generic-password", true, Some(0), b"", b"").unwrap(), Some(String::new()));
    }

    #[test]
    fn parses_keychain_account_attribute() {
        let meta = "keychain: \"/Users/x/Library/Keychains/login.keychain-db\"\nattributes:\n    \"acct\"<blob>=\"claude-code-user\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";
        assert_eq!(parse_keychain_account(meta), "claude-code-user");
        assert_eq!(parse_keychain_account("\"acct\"<blob>=\"a\\\"b\""), "a\\\"b");
        assert_eq!(parse_keychain_account("no acct"), "");
    }

    #[test]
    fn memory_store_round_trip() {
        let s = MemoryStore::new();
        assert_eq!(s.get("svc", "a").unwrap(), None);
        s.set("svc", "a", "v1").unwrap();
        s.set("svc", "a", "v2").unwrap();
        assert_eq!(s.get("svc", "a").unwrap().as_deref(), Some("v2"));
        assert_eq!(s.get_by_service("svc").unwrap(), Some(("v2".into(), "a".into())));
        assert!(s.delete("svc", "a").unwrap());
        assert!(!s.delete("svc", "a").unwrap());
    }

    #[test]
    fn file_store_writes_private_files() {
        let d = tempfile::tempdir().unwrap();
        let s = FileStore { base_dir: d.path().join("creds") };
        s.set("svc", "../id", "secret").unwrap();
        let p = d.path().join("creds/svc/.._id");
        assert!(p.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(s.get("svc", "../id").unwrap().as_deref(), Some("secret"));
        assert_eq!(s.get_by_service("svc").unwrap().map(|x| x.1), Some(".._id".into()));
        assert!(s.set("svc", "..", "x").is_err());
        assert!(s.delete("svc", "../id").unwrap());
        assert_eq!(s.get("svc", "../id").unwrap(), None);
    }

    #[test]
    fn security_cli_refuses_in_tests_without_opt_in() {
        if std::env::var(REAL_KEYCHAIN_OPT_IN).as_deref() == Ok("1") {
            return;
        }
        assert!(SecurityCli.get("kiln-accounts-test", "none").is_err());
    }
}
