//! Renews an expiring Claude Code OAuth token the way the official CLI does: under the CLI's
//! refresh locks for the credential's configuration folder, with the refresh grant at the
//! CLI's token endpoint, saving only while the stored refresh token is still the one sent.
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The CLI's token endpoint and default OAuth client.
pub(crate) const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(crate) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// A token this close to its expiry is renewed, matching the CLI.
const EXPIRY_MARGIN_MS: i64 = 300_000;
/// The CLI treats a refresh lock untouched for this long as abandoned.
const LOCK_STALE: Duration = Duration::from_secs(60);

/// The token endpoint rejected the refresh token; the account has to sign in again.
#[derive(Debug)]
pub(crate) struct SignInRequired;
impl std::fmt::Display for SignInRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "sign-in required") }
}
impl std::error::Error for SignInRequired {}

fn oauth(doc: &Value) -> Option<&Map<String, Value>> {
    doc.get("claudeAiOauth")?.as_object()
}

/// Whether the OAuth token in `secret` expires within the renewal margin at `now_ms`.
pub(crate) fn expiring(secret: &str, now_ms: i64) -> bool {
    serde_json::from_str::<Value>(secret).ok().as_ref().and_then(oauth)
        .and_then(|oauth| oauth.get("expiresAt")?.as_i64())
        .is_some_and(|expires_at| now_ms.saturating_add(EXPIRY_MARGIN_MS) >= expires_at)
}

/// The refresh grant the CLI would send for `secret`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Grant {
    pub refresh_token: String,
    pub client_id: String,
    pub scope: Option<String>,
}

impl Grant {
    /// None when `secret` holds no refresh token.
    pub(crate) fn from_secret(secret: &str) -> Option<Grant> {
        let doc: Value = serde_json::from_str(secret).ok()?;
        let oauth = oauth(&doc)?;
        let refresh_token = oauth.get("refreshToken")?.as_str().filter(|token| !token.is_empty())?.to_owned();
        let client_id = oauth.get("clientId").and_then(Value::as_str).filter(|id| !id.is_empty()).unwrap_or(CLIENT_ID).to_owned();
        let scopes: Vec<&str> = oauth.get("scopes").and_then(Value::as_array).map(|scopes| scopes.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        Some(Grant { refresh_token, client_id, scope: (!scopes.is_empty()).then(|| scopes.join(" ")) })
    }

    fn body(&self) -> Value {
        let mut body = serde_json::json!({"grant_type": "refresh_token", "refresh_token": self.refresh_token, "client_id": self.client_id});
        if let Some(scope) = &self.scope { body["scope"] = Value::String(scope.clone()); }
        body
    }
}

/// The token endpoint's answer to a refresh grant.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Renewed {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub scope: Option<String>,
    pub refresh_token_expires_in: Option<i64>,
}

pub(crate) fn parse_renewal(body: &Value) -> Result<Renewed> {
    let text = |key: &str| body.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(str::to_owned);
    let access_token = text("access_token").context("invalid token renewal response")?;
    let expires_in = body.get("expires_in").and_then(Value::as_i64).filter(|seconds| *seconds > 0).context("invalid token renewal response")?;
    Ok(Renewed {
        access_token,
        refresh_token: text("refresh_token"),
        expires_in,
        scope: text("scope"),
        refresh_token_expires_in: body.get("refresh_token_expires_in").and_then(Value::as_i64).filter(|seconds| *seconds > 0),
    })
}

/// `current` with the renewed tokens when its refresh token is still `sent` (or empty); None
/// when another process renewed it first, whose credentials then stand.
pub(crate) fn merged(current: &str, sent: &str, renewed: &Renewed, now_ms: i64) -> Result<Option<String>> {
    let mut doc: Value = serde_json::from_str(current).context("credentials are not JSON")?;
    let oauth = doc.get_mut("claudeAiOauth").and_then(Value::as_object_mut).context("credentials hold no Claude subscription login")?;
    let stored = oauth.get("refreshToken").and_then(Value::as_str).unwrap_or_default();
    if !stored.is_empty() && stored != sent { return Ok(None); }
    let at = |seconds: i64| Value::from(now_ms.saturating_add(seconds.saturating_mul(1000)));
    oauth.insert("accessToken".into(), Value::String(renewed.access_token.clone()));
    oauth.insert("refreshToken".into(), Value::String(renewed.refresh_token.clone().unwrap_or_else(|| sent.to_owned())));
    oauth.insert("expiresAt".into(), at(renewed.expires_in));
    if let Some(scope) = &renewed.scope {
        oauth.insert("scopes".into(), scope.split_whitespace().map(|s| Value::String(s.to_owned())).collect());
    }
    if let Some(seconds) = renewed.refresh_token_expires_in { oauth.insert("refreshTokenExpiresAt".into(), at(seconds)); }
    Ok(Some(doc.to_string()))
}

/// The CLI's two refresh locks for a configuration folder, held until dropped.
pub(crate) struct RefreshLock {
    held: Vec<PathBuf>,
}

impl RefreshLock {
    /// The lock the CLI takes inside the folder, then its legacy lock beside the folder.
    fn paths(config_dir: &Path) -> (PathBuf, PathBuf) {
        let real = std::fs::canonicalize(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
        let mut legacy = real.into_os_string();
        legacy.push(".lock");
        (config_dir.join(".oauth_refresh.lock"), PathBuf::from(legacy))
    }

    /// Takes both locks, trying `attempts` times `pause` apart; None while another process
    /// holds them.
    pub(crate) fn acquire(config_dir: &Path, attempts: u32, pause: Duration) -> Result<Option<RefreshLock>> {
        let (inner, legacy) = Self::paths(config_dir);
        for attempt in 0..attempts {
            if take(&inner)? {
                match take(&legacy) {
                    Ok(true) => return Ok(Some(RefreshLock { held: vec![inner, legacy] })),
                    Ok(false) => { let _ = std::fs::remove_dir(&inner); }
                    // The CLI also proceeds on its inner lock when the legacy lock cannot be made.
                    Err(_) => return Ok(Some(RefreshLock { held: vec![inner] })),
                }
            }
            if attempt + 1 < attempts { std::thread::sleep(pause); }
        }
        Ok(None)
    }
}

/// Creates the lock folder at `path`; an abandoned one is replaced. False while it is held.
fn take(path: &Path) -> Result<bool> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let abandoned = std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > LOCK_STALE);
            if !abandoned { return Ok(false); }
            let _ = std::fs::remove_dir(path);
            Ok(std::fs::create_dir(path).is_ok())
        }
        Err(error) => Err(error).with_context(|| format!("cannot lock {}", path.display())),
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        for path in self.held.drain(..).rev() { let _ = std::fs::remove_dir(path); }
    }
}

/// Sends `grant` to the CLI's token endpoint.
pub(crate) fn request(grant: &Grant) -> Result<Renewed> {
    post(TOKEN_URL, grant, "=https")
}

fn post(url: &str, grant: &Grant, protocol: &str) -> Result<Renewed> {
    // The grant travels on stdin; response bodies and curl diagnostics are never shown.
    let dump = tempfile::NamedTempFile::new()?;
    let mut child = kiln_common::process::hide_window(&mut Command::new("curl"))
        .args(["-q", "-sS", "--max-time", "30", "--connect-timeout", "5", "--max-filesize", "65536", "--proto", protocol,
            "-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", "-D"])
        .arg(dump.path()).arg(url)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("token renewal could not start")?;
    let sent = child.stdin.take().context("token renewal input unavailable")?.write_all(grant.body().to_string().as_bytes());
    let mut bytes = Vec::new();
    let received = child.stdout.take().context("token renewal response unavailable")?.take(65_537).read_to_end(&mut bytes);
    if bytes.len() > 65_536 || received.is_err() { let _ = child.kill(); }
    let status = child.wait()?;
    sent.context("token renewal input failed")?;
    if bytes.len() > 65_536 { bail!("token renewal response too large"); }
    received.context("token renewal response failed")?;
    if !status.success() { bail!("token renewal failed or timed out"); }
    let mut header_dump = String::new();
    dump.reopen()?.take(32_769).read_to_string(&mut header_dump)?;
    let (code, _) = crate::claude::parse_header_dump(&header_dump);
    let body: Option<Value> = serde_json::from_slice(&bytes).ok();
    if matches!(code, 400 | 401) && body.as_ref().and_then(|b| b.get("error")?.as_str()) == Some("invalid_grant") {
        return Err(SignInRequired.into());
    }
    if code != 200 { bail!("token renewal failed (HTTP {code})"); }
    parse_renewal(&body.context("invalid token renewal response")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(access: &str, refresh: &str, expires_at: i64) -> String {
        serde_json::json!({
            "claudeAiOauth": {"accessToken": access, "refreshToken": refresh, "expiresAt": expires_at,
                "scopes": ["user:inference", "user:profile"], "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"},
            "mcpOAuth": {"server": {"accessToken": "kept"}}
        }).to_string()
    }

    fn renewed() -> Renewed {
        Renewed { access_token: "at-new".into(), refresh_token: Some("rt-new".into()), expires_in: 28_800, scope: None, refresh_token_expires_in: None }
    }

    #[test]
    fn tokens_are_renewed_from_five_minutes_before_expiry() {
        let at = 10_000_000;
        assert!(!expiring(&secret("a", "r", at), at - 300_001));
        assert!(expiring(&secret("a", "r", at), at - 300_000));
        assert!(expiring(&secret("a", "r", at), at + 1));
        assert!(!expiring(r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#, at));
        assert!(!expiring("not json", at));
    }

    #[test]
    fn the_grant_carries_the_stored_refresh_token_client_and_scopes() {
        let grant = Grant::from_secret(&secret("a", "rt-old", 1)).unwrap();
        assert_eq!(grant, Grant { refresh_token: "rt-old".into(), client_id: CLIENT_ID.into(), scope: Some("user:inference user:profile".into()) });
        let own = serde_json::json!({"claudeAiOauth": {"accessToken": "a", "refreshToken": "r", "clientId": "own-client", "scopes": []}}).to_string();
        assert_eq!(Grant::from_secret(&own).unwrap(), Grant { refresh_token: "r".into(), client_id: "own-client".into(), scope: None });
        assert!(Grant::from_secret(r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":""}}"#).is_none());
        assert!(Grant::from_secret(r#"{"accessToken":"a","refreshToken":"r"}"#).is_none());
    }

    #[test]
    fn a_renewal_needs_an_access_token_and_a_lifetime() {
        let full = parse_renewal(&serde_json::json!({"access_token": "at", "refresh_token": "rt", "expires_in": 28800, "scope": "a b", "refresh_token_expires_in": 100})).unwrap();
        assert_eq!(full, Renewed { access_token: "at".into(), refresh_token: Some("rt".into()), expires_in: 28_800, scope: Some("a b".into()), refresh_token_expires_in: Some(100) });
        let kept = parse_renewal(&serde_json::json!({"access_token": "at", "expires_in": 60})).unwrap();
        assert_eq!(kept.refresh_token, None);
        assert!(parse_renewal(&serde_json::json!({"access_token": "at"})).is_err());
        assert!(parse_renewal(&serde_json::json!({"access_token": "", "expires_in": 60})).is_err());
        assert!(parse_renewal(&serde_json::json!({"access_token": "at", "expires_in": 0})).is_err());
    }

    #[test]
    fn renewed_tokens_replace_only_the_token_fields() {
        let now = 1_000_000;
        let next = merged(&secret("at-old", "rt-old", 5), "rt-old", &renewed(), now).unwrap().unwrap();
        let doc: Value = serde_json::from_str(&next).unwrap();
        let oauth = &doc["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "at-new");
        assert_eq!(oauth["refreshToken"], "rt-new");
        assert_eq!(oauth["expiresAt"], now + 28_800_000);
        assert_eq!(oauth["scopes"], serde_json::json!(["user:inference", "user:profile"]));
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(oauth["rateLimitTier"], "default_claude_max_20x");
        assert_eq!(doc["mcpOAuth"]["server"]["accessToken"], "kept");
        let scoped = Renewed { refresh_token: None, scope: Some("user:inference".into()), refresh_token_expires_in: Some(60), ..renewed() };
        let next: Value = serde_json::from_str(&merged(&secret("at-old", "rt-old", 5), "rt-old", &scoped, now).unwrap().unwrap()).unwrap();
        assert_eq!(next["claudeAiOauth"]["refreshToken"], "rt-old");
        assert_eq!(next["claudeAiOauth"]["scopes"], serde_json::json!(["user:inference"]));
        assert_eq!(next["claudeAiOauth"]["refreshTokenExpiresAt"], now + 60_000);
    }

    #[test]
    fn credentials_renewed_by_another_process_stand() {
        assert_eq!(merged(&secret("at-other", "rt-other", 5), "rt-old", &renewed(), 0).unwrap(), None);
        let emptied = secret("at-old", "", 5);
        assert!(merged(&emptied, "rt-old", &renewed(), 0).unwrap().is_some());
        assert!(merged(r#"{"mcpOAuth":{}}"#, "rt-old", &renewed(), 0).is_err());
    }

    #[test]
    fn the_cli_locks_are_taken_together_and_released_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        let (inner, legacy) = RefreshLock::paths(&dir);
        assert_eq!(inner, dir.join(".oauth_refresh.lock"));
        assert_eq!(legacy, PathBuf::from(format!("{}.lock", std::fs::canonicalize(&dir).unwrap().display())));
        let lock = RefreshLock::acquire(&dir, 1, Duration::ZERO).unwrap().expect("free locks");
        assert!(inner.is_dir() && legacy.is_dir());
        assert!(RefreshLock::acquire(&dir, 2, Duration::from_millis(10)).unwrap().is_none(), "a held lock is not taken twice");
        drop(lock);
        assert!(!inner.exists() && !legacy.exists());
    }

    #[test]
    fn a_held_legacy_lock_keeps_the_inner_lock_free() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        let (inner, legacy) = RefreshLock::paths(&dir);
        std::fs::create_dir(&legacy).unwrap();
        assert!(RefreshLock::acquire(&dir, 1, Duration::ZERO).unwrap().is_none());
        assert!(!inner.exists(), "the inner lock is released when the legacy lock is busy");
    }

    #[test]
    fn an_abandoned_lock_is_taken_over() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        let (inner, _) = RefreshLock::paths(&dir);
        std::fs::create_dir(&inner).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(120);
        std::fs::File::open(&inner).unwrap().set_modified(old).unwrap();
        assert!(RefreshLock::acquire(&dir, 1, Duration::ZERO).unwrap().is_some());
    }

    // Local loopback only: no real provider credentials or requests.
    fn fixture(response: Vec<u8>) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/oauth/token", server.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let length = head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            request.extend_from_slice(&body);
            let _ = stream.write_all(&response);
            request
        });
        (url, thread)
    }

    #[test]
    fn the_grant_is_posted_as_json_and_the_answer_parsed() {
        let body = br#"{"access_token":"at-new","refresh_token":"rt-new","expires_in":28800}"#;
        let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        response.extend_from_slice(body);
        let (url, server) = fixture(response);
        let grant = Grant { refresh_token: "rt-fixture".into(), client_id: CLIENT_ID.into(), scope: Some("user:inference".into()) };
        assert_eq!(post(&url, &grant, "=http").unwrap(), renewed());
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        assert!(request.starts_with("POST /v1/oauth/token "), "{request}");
        assert!(request.to_ascii_lowercase().contains("content-type: application/json"));
        let sent: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(sent, serde_json::json!({"grant_type": "refresh_token", "refresh_token": "rt-fixture", "client_id": CLIENT_ID, "scope": "user:inference"}));
    }

    #[test]
    fn a_rejected_refresh_token_asks_for_sign_in_without_the_response_text() {
        let body = br#"{"error":"invalid_grant","error_description":"private-detail"}"#;
        let mut response = format!("HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        response.extend_from_slice(body);
        let (url, server) = fixture(response);
        let grant = Grant { refresh_token: "rt-fixture".into(), client_id: CLIENT_ID.into(), scope: None };
        let error = post(&url, &grant, "=http").unwrap_err();
        assert!(error.downcast_ref::<SignInRequired>().is_some(), "{error:#}");
        assert!(!format!("{error:#}").contains("private-detail"));
        server.join().unwrap();
        let (url, server) = fixture(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 13\r\n\r\nprivate-token".to_vec());
        let error = post(&url, &grant, "=http").unwrap_err();
        assert!(error.downcast_ref::<SignInRequired>().is_none());
        assert_eq!(format!("{error:#}"), "token renewal failed (HTTP 503)");
        server.join().unwrap();
    }
}
