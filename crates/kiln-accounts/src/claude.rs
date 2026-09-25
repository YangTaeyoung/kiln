//! Claude Code 자격증명·계정 식별 정보·사용량 조회.

use crate::Usage;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Claude Code 가 쓰는 키체인 service 이름.
pub const CLAUDE_SERVICE: &str = "Claude Code-credentials";

/// 사용량 헤더(anthropic-ratelimit-unified-*)를 받는 최소 inference 요청 주소.
pub const PROBE_URL: &str = "https://api.anthropic.com/v1/messages";
/// 프로브 요청 본문(haiku 1토큰 출력).
pub const PROBE_BODY: &str = r#"{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;

/// Linux/Windows 의 Claude Code 자격증명 파일. `CLAUDE_CONFIG_DIR` 가 있으면 그 아래를 쓴다.
pub fn credentials_path(home: &Path, claude_config_dir: Option<&Path>) -> PathBuf {
    match claude_config_dir {
        Some(d) => d.join(".credentials.json"),
        None => home.join(".claude").join(".credentials.json"),
    }
}

/// `~/.claude.json` 경로.
pub fn claude_json_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn load_doc(path: &Path) -> Result<Map<String, Value>> {
    let data = std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("failed to parse {}", path.display()))
}

/// `oauthAccount` 필드를 그대로 돌려준다. 필드가 없으면 `None`.
pub fn read_oauth_account(path: &Path) -> Result<Option<Value>> {
    Ok(load_doc(path)?.remove("oauthAccount"))
}

/// 다른 필드는 그대로 두고 `oauthAccount` 만 바꿔 0600 으로 원자적으로 쓴다. `None` 이면 필드를 지운다.
pub fn write_oauth_account(path: &Path, oauth: Option<&Value>) -> Result<()> {
    let mut doc = load_doc(path)?;
    match oauth {
        Some(v) => {
            doc.insert("oauthAccount".into(), v.clone());
        }
        None => {
            doc.remove("oauthAccount");
        }
    }
    crate::store::write_secret_atomic(path, &serde_json::to_vec(&doc)?)
}

/// `oauthAccount` 에서 이메일(`emailAddress` 또는 `email`)을 꺼낸다.
pub fn oauth_email(oauth: &Value) -> Option<String> {
    ["emailAddress", "email"]
        .iter()
        .find_map(|k| oauth.get(*k).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 자격증명 JSON 에서 accessToken 을 꺼낸다. 최상위 또는 `claudeAiOauth` 아래를 본다.
pub fn access_token(cred_json: &str) -> Result<String> {
    let doc: Value = serde_json::from_str(cred_json).context("failed to parse credentials JSON")?;
    let scope = doc.get("claudeAiOauth").unwrap_or(&doc);
    match scope.get("accessToken").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => bail!("accessToken field not found"),
    }
}

/// 소문자 헤더 이름 → 값 목록에서 사용량을 만든다. 상태 헤더가 없으면 `None`.
pub fn usage_from_headers(get: impl Fn(&str) -> Option<String>) -> Option<Usage> {
    let status = get("anthropic-ratelimit-unified-status").filter(|s| !s.is_empty())?;
    let window = |w: &str| -> Option<(f32, Option<i64>)> {
        let util = get(&format!("anthropic-ratelimit-unified-{w}-utilization")).and_then(|s| s.trim().parse::<f32>().ok())?;
        let reset = get(&format!("anthropic-ratelimit-unified-{w}-reset")).and_then(|s| s.trim().parse::<i64>().ok());
        Some((util, reset))
    };
    Some(Usage { five_hour: window("5h"), seven_day: window("7d"), status })
}

/// `curl -D -` 가 출력한 응답 헤더 블록을 (상태 코드, 소문자 헤더 목록)으로 나눈다.
/// 1xx·리다이렉트 블록이 여러 개면 마지막 블록을 쓴다.
pub fn parse_header_dump(dump: &str) -> (u16, Vec<(String, String)>) {
    let mut code = 0;
    let mut headers = Vec::new();
    for line in dump.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HTTP/") {
            code = line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
            headers.clear();
        } else if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    (code, headers)
}

/// accessToken 으로 프로브 요청을 보내 사용량 헤더를 읽는다.
/// 토큰은 curl 인자에 넣지 않고 표준입력(`-H @-`)으로 넘긴다.
pub fn probe_usage(access_token: &str) -> Result<Usage> {
    let mut child = Command::new("curl")
        .args(["-sS", "-o", if cfg!(windows) { "NUL" } else { "/dev/null" }, "-D", "-", "--max-time", "15", "-X", "POST", PROBE_URL, "-H", "@-", "--data-binary", PROBE_BODY])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run curl")?;
    {
        let mut stdin = child.stdin.take().context("curl stdin unavailable")?;
        write!(
            stdin,
            "Authorization: Bearer {access_token}\nanthropic-beta: oauth-2025-04-20\nanthropic-version: 2023-06-01\nContent-Type: application/json\n"
        )?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("request failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let (code, headers) = parse_header_dump(&String::from_utf8_lossy(&out.stdout));
    if code == 401 || code == 403 {
        bail!("authentication failed (HTTP {code}) — token may be expired");
    }
    let get = |k: &str| headers.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    usage_from_headers(get).with_context(|| format!("no rate-limit headers (HTTP {code})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_token_top_level_and_nested() {
        assert_eq!(access_token(r#"{"claudeAiOauth":{"accessToken":"tok-a","refreshToken":"r"}}"#).unwrap(), "tok-a");
        assert_eq!(access_token(r#"{"accessToken":"tok-b"}"#).unwrap(), "tok-b");
        assert!(access_token(r#"{"claudeAiOauth":{}}"#).is_err());
        assert!(access_token("not json").is_err());
    }

    #[test]
    fn oauth_account_round_trip_keeps_other_fields() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".claude.json");
        std::fs::write(&p, r#"{"numStartups":3,"oauthAccount":{"emailAddress":"a@x.com"},"projects":{}}"#).unwrap();
        let oa = read_oauth_account(&p).unwrap().unwrap();
        assert_eq!(oauth_email(&oa).as_deref(), Some("a@x.com"));
        write_oauth_account(&p, Some(&serde_json::json!({"emailAddress":"b@x.com"}))).unwrap();
        let doc: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(doc["numStartups"], 3);
        assert_eq!(doc["oauthAccount"]["emailAddress"], "b@x.com");
        assert!(doc.get("projects").is_some());
        write_oauth_account(&p, None).unwrap();
        assert!(read_oauth_account(&p).unwrap().is_none());
    }

    #[test]
    fn oauth_email_falls_back_to_email_key() {
        assert_eq!(oauth_email(&serde_json::json!({"email":"c@x.com"})).as_deref(), Some("c@x.com"));
        assert_eq!(oauth_email(&serde_json::json!({"emailAddress":""})), None);
    }

    #[test]
    fn parses_rate_limit_headers() {
        let dump = "HTTP/1.1 100 Continue\r\n\r\nHTTP/2 200\r\ncontent-type: application/json\r\nAnthropic-Ratelimit-Unified-Status: allowed\r\nanthropic-ratelimit-unified-5h-status: allowed\r\nanthropic-ratelimit-unified-5h-utilization: 0.77\r\nanthropic-ratelimit-unified-5h-reset: 1790000000\r\nanthropic-ratelimit-unified-7d-utilization: 0.12\r\n\r\n";
        let (code, headers) = parse_header_dump(dump);
        assert_eq!(code, 200);
        let get = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let u = usage_from_headers(get).unwrap();
        assert_eq!(u.status, "allowed");
        assert_eq!(u.five_hour, Some((0.77, Some(1_790_000_000))));
        assert_eq!(u.seven_day, Some((0.12, None)));
        assert!(usage_from_headers(|_| None).is_none());
    }

    #[test]
    fn credentials_path_honours_config_dir() {
        let home = Path::new("/h");
        assert_eq!(credentials_path(home, None), PathBuf::from("/h/.claude/.credentials.json"));
        assert_eq!(credentials_path(home, Some(Path::new("/c"))), PathBuf::from("/c/.credentials.json"));
    }
}
