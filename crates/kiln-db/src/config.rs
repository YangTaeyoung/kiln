//! 연결 설정 모델, URL 가져오기, 비밀번호 저장소(OS 키체인 / 메모리).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 연결 식별자.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConnId(pub u64);

impl ConnId {
    /// 현재 시각과 카운터로 새 ID 를 만든다.
    pub fn new_unique() -> ConnId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        ConnId(t.wrapping_mul(16) + (SEQ.fetch_add(1, Ordering::Relaxed) % 16))
    }
}

impl std::fmt::Display for ConnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Driver {
    Postgres,
    MySql,
    MariaDb,
    Sqlite,
}

impl Driver {
    pub const ALL: [Driver; 4] = [
        Driver::Postgres,
        Driver::MySql,
        Driver::MariaDb,
        Driver::Sqlite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Driver::Postgres => "PostgreSQL",
            Driver::MySql => "MySQL",
            Driver::MariaDb => "MariaDB",
            Driver::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::MySql | Driver::MariaDb => 3306,
            Driver::Sqlite => 0,
        }
    }

    pub fn is_mysql(self) -> bool {
        matches!(self, Driver::MySql | Driver::MariaDb)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 5] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyCa,
        SslMode::VerifyFull,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyCa => "verify-ca",
            SslMode::VerifyFull => "verify-full",
        }
    }

    pub fn parse(s: &str) -> Option<SslMode> {
        Some(match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "disable" | "disabled" | "false" => SslMode::Disable,
            "prefer" | "preferred" | "allow" => SslMode::Prefer,
            "require" | "required" | "true" => SslMode::Require,
            "verify-ca" => SslMode::VerifyCa,
            "verify-full" | "verify-identity" => SslMode::VerifyFull,
            _ => return None,
        })
    }
}

/// 저장되는 연결 설정. 비밀번호는 `save_password_in_file` 일 때만 파일에 들어간다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnConfig {
    pub id: ConnId,
    pub name: String,
    pub driver: Driver,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    /// SQLite 파일 경로.
    pub file: String,
    pub ssl_mode: SslMode,
    /// 색상 라벨(RGB).
    pub color: Option<[u8; 3]>,
    pub connect_timeout_secs: u32,
    pub save_password_in_file: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_in_file: Option<String>,
}

impl Default for ConnConfig {
    fn default() -> Self {
        ConnConfig {
            id: ConnId(0),
            name: String::new(),
            driver: Driver::Postgres,
            host: "localhost".into(),
            port: 5432,
            user: String::new(),
            database: String::new(),
            file: String::new(),
            ssl_mode: SslMode::Prefer,
            color: None,
            connect_timeout_secs: 10,
            save_password_in_file: false,
            password_in_file: None,
        }
    }
}

impl ConnConfig {
    /// 목록에 보여줄 이름. 비어 있으면 호스트/파일로 만든다.
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        match self.driver {
            Driver::Sqlite => std::path::Path::new(&self.file)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sqlite".into()),
            _ => {
                let database=self.database.trim();
                let user=self.user.trim();
                let host=self.host.trim();
                if !database.is_empty() {
                    if host.is_empty(){database.into()}else{format!("{database}@{host}")}
                } else if !user.is_empty() && !host.is_empty() {
                    format!("{user}@{host}")
                } else if !host.is_empty() {
                    host.into()
                } else {
                    self.driver.label().into()
                }
            }
        }
    }

    /// 연결 요약 문자열(툴팁·부제목용).
    pub fn summary(&self) -> String {
        match self.driver {
            Driver::Sqlite => format!("SQLite · {}", self.file),
            d => format!(
                "{} · {}@{}:{}/{}",
                d.label(),
                self.user,
                self.host,
                self.port,
                self.database
            ),
        }
    }

    /// URL 에서 설정을 만든다. 비밀번호가 있으면 함께 돌려준다.
    pub fn from_url(input: &str) -> Result<(ConnConfig, Option<String>), String> {
        let input = input.trim();
        let (scheme, rest) = input
            .split_once(':')
            .ok_or_else(|| kiln_common::i18n::tr("URL에 스킴이 없습니다").to_string())?;
        let driver = match scheme.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Driver::Postgres,
            "mysql" => Driver::MySql,
            "mariadb" => Driver::MariaDb,
            "sqlite" | "sqlite3" | "file" => Driver::Sqlite,
            other => return Err(kiln_common::trf!("지원하지 않는 스킴 '{other}'")),
        };
        let mut cfg = ConnConfig {
            driver,
            port: driver.default_port(),
            ..ConnConfig::default()
        };
        if driver == Driver::Sqlite {
            let path = rest.trim_start_matches("//");
            let path = path.split('?').next().unwrap_or("");
            if path.is_empty() {
                return Err(kiln_common::i18n::tr("sqlite URL에 경로가 없습니다").into());
            }
            cfg.file = percent_decode(path);
            cfg.host.clear();
            cfg.name = cfg.display_name();
            return Ok((cfg, None));
        }
        let url = url::Url::parse(input).map_err(|e| kiln_common::trf!("잘못된 URL: {e}"))?;
        cfg.host = url.host_str().unwrap_or("localhost").to_string();
        cfg.port = url.port().unwrap_or(driver.default_port());
        cfg.user = percent_decode(url.username());
        cfg.database = percent_decode(url.path().trim_start_matches('/'));
        let password = url.password().map(percent_decode);
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "sslmode" | "ssl-mode" | "ssl_mode" | "ssl" => {
                    if let Some(m) = SslMode::parse(&v) {
                        cfg.ssl_mode = m;
                    }
                }
                "connect_timeout" => {
                    if let Ok(t) = v.parse() {
                        cfg.connect_timeout_secs = t;
                    }
                }
                _ => {}
            }
        }
        cfg.name = cfg.display_name();
        Ok((cfg, password))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 비밀번호 저장소. 키체인이 없으면 메모리에만 둔다.
pub(crate) struct Secrets {
    use_keychain: Mutex<bool>,
    mem: Mutex<HashMap<ConnId, String>>,
    warning: Mutex<Option<String>>,
}

const KEYCHAIN_SERVICE: &str = "dev.kiln.db";

impl Secrets {
    pub(crate) fn new(use_keychain: bool) -> Secrets {
        Secrets {
            use_keychain: Mutex::new(use_keychain),
            mem: Mutex::new(HashMap::new()),
            warning: Mutex::new(None),
        }
    }

    fn fallback(&self, err: impl std::fmt::Display) {
        log::warn!("keychain unavailable, keeping passwords in memory only: {err}");
        *self.use_keychain.lock() = false;
        *self.warning.lock() = Some(kiln_common::trf!(
            "OS 키체인을 사용할 수 없습니다({err}). 비밀번호는 이번 세션 동안 메모리에만 보관됩니다"
        ));
    }

    pub(crate) fn warning(&self) -> Option<String> {
        self.warning.lock().clone()
    }

    pub(crate) fn set(&self, id: ConnId, pw: &str) {
        self.mem.lock().insert(id, pw.to_string());
        if *self.use_keychain.lock() {
            match keyring::Entry::new(KEYCHAIN_SERVICE, &id.to_string())
                .and_then(|e| e.set_password(pw))
            {
                Ok(()) => {}
                Err(e) => self.fallback(e),
            }
        }
    }

    pub(crate) fn get(&self, id: ConnId) -> Option<String> {
        if let Some(p) = self.mem.lock().get(&id) {
            return Some(p.clone());
        }
        if *self.use_keychain.lock() {
            match keyring::Entry::new(KEYCHAIN_SERVICE, &id.to_string())
                .and_then(|e| e.get_password())
            {
                Ok(p) => {
                    self.mem.lock().insert(id, p.clone());
                    return Some(p);
                }
                Err(keyring::Error::NoEntry) => {}
                Err(e) => self.fallback(e),
            }
        }
        None
    }

    pub(crate) fn delete(&self, id: ConnId) {
        self.mem.lock().remove(&id);
        if *self.use_keychain.lock()
            && let Ok(e) = keyring::Entry::new(KEYCHAIN_SERVICE, &id.to_string())
        {
            let _ = e.delete_credential();
        }
    }
}

/// 설정 파일 경로 묶음.
#[derive(Clone, Debug)]
pub(crate) struct StorePaths {
    pub connections: PathBuf,
    pub history: PathBuf,
}

#[cfg(test)]
mod display_name_tests {
    use super::*;
    #[test]
    fn unnamed_connection_never_displays_an_empty_at_sign_component() {
        let mut cfg=ConnConfig::default();
        cfg.name.clear();cfg.user.clear();cfg.database.clear();cfg.host="localhost".into();
        assert_eq!(cfg.display_name(),"localhost");
        cfg.user="alice".into();assert_eq!(cfg.display_name(),"alice@localhost");
        cfg.database="app".into();assert_eq!(cfg.display_name(),"app@localhost");
        cfg.host="  ".into();assert_eq!(cfg.display_name(),"app");
        cfg.database.clear();assert_eq!(cfg.display_name(),cfg.driver.label());
        cfg.name="개발 DB".into();assert_eq!(cfg.display_name(),"개발 DB");
    }
}
