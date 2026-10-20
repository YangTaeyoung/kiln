//! OpenAI Codex CLI 자격증명(`$CODEX_HOME/auth.json`)과 세션 기록의 사용량.

use crate::Usage;
use base64::Engine;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// `CODEX_HOME` 이 있으면 그 경로, 없으면 `~/.codex`.
pub fn codex_home(home: &Path, env_codex_home: Option<&Path>) -> PathBuf {
    env_codex_home.map(Path::to_path_buf).unwrap_or_else(|| home.join(".codex"))
}

pub fn auth_path(codex_home: &Path) -> PathBuf {
    codex_home.join("auth.json")
}

/// Direct Keyring key used by the official CLI (0.162.1 storage.rs): the standard library's
/// canonical form of the home, which on Windows keeps the `\\?\` prefix.
pub(crate) fn keyring_account(home: &Path) -> String {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let hash = format!("{:x}", Sha256::digest(canonical.to_string_lossy().as_bytes()));
    format!("cli|{}", &hash[..16])
}

pub(crate) fn read_credentials(home: &Path, store: &dyn crate::CredentialStore) -> anyhow::Result<Option<String>> {
    let config = crate::store::read_secret(&home.join("config.toml"))?;
    let config = config.as_deref().map(toml::from_str::<toml::Value>).transpose()?;
    let mode = config.as_ref().and_then(|doc| doc.get("cli_auth_credentials_store")).and_then(toml::Value::as_str).unwrap_or("file");
    match mode {
        "file" => crate::store::read_secret(&auth_path(home)),
        "keyring" | "auto" => {
            let secret_storage = config.as_ref().and_then(|doc| doc.get("features")).and_then(|features| features.get("secret_auth_storage")).and_then(toml::Value::as_bool).unwrap_or(cfg!(windows));
            // New secrets and ephemeral backends cannot be inferred from absence
            // of auth.json. Report an unknown local state, never a logout.
            anyhow::ensure!(!secret_storage, "credential storage requires CLI status verification");
            match store.get("Codex Auth", &keyring_account(home)) {
                Ok(Some(secret)) => Ok(Some(secret)),
                Ok(None) if mode == "auto" => crate::store::read_secret(&auth_path(home)),
                Err(_) if mode == "auto" => crate::store::read_secret(&auth_path(home)),
                result => result,
            }
        }
        _ => anyhow::bail!("credential storage requires CLI status verification"),
    }
}

/// JWT 의 payload 를 서명 검증 없이 JSON 으로 디코드한다.
pub fn jwt_claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// auth.json 에서 계정 이메일을 꺼낸다. `tokens.id_token` 의 `email` 클레임,
/// 없으면 `https://api.openai.com/profile` 클레임의 `email` 을 본다.
pub fn auth_email(auth_json: &str) -> Option<String> {
    let doc: Value = serde_json::from_str(auth_json).ok()?;
    let token = doc.get("tokens")?.get("id_token")?.as_str()?;
    let claims = jwt_claims(token)?;
    claims
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| claims.get("https://api.openai.com/profile").and_then(|p| p.get("email")).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// auth.json 이 ChatGPT 로그인(토큰 보유)인지.
pub fn has_chatgpt_tokens(auth_json: &str) -> bool {
    serde_json::from_str::<Value>(auth_json)
        .ok()
        .and_then(|d| d.get("tokens").cloned())
        .is_some_and(|t| t.is_object())
}

/// Local absence only. Email is optional in the official authentication schema.
/// Keep unknown/new authentication material distinct from an empty credential.
pub(crate) fn credentials_missing(auth_json: &str) -> bool {
    let Ok(doc) = serde_json::from_str::<Value>(auth_json) else { return false; };
    if !doc.is_object() { return false; }
    if let Some(tokens) = doc.get("tokens").and_then(Value::as_object)
        && tokens.values().any(|value| value.as_str().is_some_and(|s| !s.is_empty()))
    { return false; }
    if doc.get("agent_identity").is_some_and(|value| !value.is_null())
        || ["OPENAI_API_KEY", "personal_access_token", "bedrock_api_key", "bedrock_access_keys"].iter().any(|key| doc.get(*key).is_some_and(|value| !value.is_null()))
    { return false; }
    // Future non-null fields require checking; don't invent a logout verdict.
    !doc.as_object().is_some_and(|fields| fields.iter().any(|(key, value)| !matches!(key.as_str(), "tokens" | "auth_mode" | "last_refresh" | "OPENAI_API_KEY") && !value.is_null()))
}

/// 세션 기록에서 찾은 사용량과 기록 시각(unix 초).
#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    pub usage: Usage,
    pub at_unix: i64,
}

/// `2026-09-24T14:52:55.688Z` 형식(UTC)을 unix 초로 바꾼다.
pub fn parse_rfc3339_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let time = rest.trim_end_matches('Z');
    let (time, offset) = match time.rfind(['+', '-']) {
        Some(i) if i >= 5 => (&time[..i], Some(&time[i..])),
        _ => (time, None),
    };
    let mut t = time.split(':');
    let (hh, mm): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let ss: f64 = t.next().unwrap_or("0").parse().ok()?;
    let off = match offset {
        Some(o) => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let (oh, om) = o[1..].split_once(':').unwrap_or((&o[1..], "0"));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
        None => 0,
    };
    let (yy, mm_) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm_ + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss as i64 - off)
}

/// `rate_limits` 객체 하나를 [`Usage`] 로 바꾼다. 창 길이 300분 이하는 5시간, 그보다 길면 7일 칸에 넣는다.
/// 이미 지난 초기화 시각이면 사용률을 0 으로 본다.
pub fn usage_from_rate_limits(rl: &Value, now_unix: i64) -> Option<Usage> {
    let mut five = None;
    let mut seven = None;
    for key in ["primary", "secondary"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else { continue };
        let reset = w.get("resets_at").and_then(Value::as_i64);
        let mins = w.get("window_minutes").and_then(Value::as_i64).unwrap_or(0);
        let frac = if reset.is_some_and(|r| r <= now_unix) { 0.0 } else { (pct / 100.0) as f32 };
        let slot = (frac, reset);
        if mins > 0 && mins <= 300 {
            five.get_or_insert(slot);
        } else {
            seven.get_or_insert(slot);
        }
    }
    if five.is_none() && seven.is_none() {
        return None;
    }
    let reached = rl.get("rate_limit_reached_type").is_some_and(|v| !v.is_null());
    let plan = rl.get("plan_type").and_then(Value::as_str).unwrap_or("");
    let status = if reached {
        "rejected".to_string()
    } else if plan.is_empty() {
        "allowed".to_string()
    } else {
        format!("allowed · {plan}")
    };
    Some(Usage { five_hour: five, seven_day: seven, status, ..Default::default() })
}

/// JSONL 한 줄에서 `payload.rate_limits` 를 찾아 사용량으로 바꾼다.
pub fn observed_from_line(line: &str, now_unix: i64) -> Option<Observed> {
    if !line.contains("\"rate_limits\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let rl = v.get("payload")?.get("rate_limits")?;
    if rl.is_null() {
        return None;
    }
    let at = v.get("timestamp").and_then(Value::as_str).and_then(parse_rfc3339_utc)?;
    Some(Observed { usage: usage_from_rate_limits(rl, now_unix)?, at_unix: at })
}

const TAIL_BYTES: u64 = 512 * 1024;
const MAX_FILES: usize = 12;

/// 파일 끝 `TAIL_BYTES` 안에서 마지막 사용량 기록을 찾는다.
fn last_in_file(path: &Path, now_unix: i64) -> Option<Observed> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines().rev().find_map(|l| observed_from_line(l, now_unix))
}

/// `sessions/YYYY/MM/DD/*.jsonl` 을 최신 날짜부터 훑어 가장 최근 사용량 기록을 돌려준다.
pub fn latest_observed(codex_home: &Path, now_unix: i64) -> Option<Observed> {
    let root = codex_home.join("sessions");
    let sorted_dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        v.sort();
        v.reverse();
        v
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    'outer: for y in sorted_dirs(&root) {
        for m in sorted_dirs(&y) {
            for d in sorted_dirs(&m) {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "jsonl") {
                        let mt = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                        files.push((mt, p));
                    }
                }
                if files.len() >= MAX_FILES {
                    break 'outer;
                }
            }
        }
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .iter()
        .take(MAX_FILES)
        .filter_map(|(_, p)| last_in_file(p, now_unix))
        .max_by_key(|o| o.at_unix)
}

/// Passive usage reader; no reset credit consumption or token refresh.
pub fn probe_usage(auth: &str) -> anyhow::Result<Usage> {
    let doc: Value = serde_json::from_str(auth).map_err(|_| anyhow::anyhow!("invalid account credentials"))?;
    let token = doc.pointer("/tokens/access_token").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("no subscription access token"))?;
    let account = doc.pointer("/tokens/account_id").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("no subscription account id"))?;
    let token = crate::usage_http::header_value(token)?;
    let account = crate::usage_http::header_value(account)?;
    let body = crate::usage_http::get("https://chatgpt.com/backend-api/wham/usage", &format!(
        "Authorization: Bearer {token}\nChatGPT-Account-Id: {account}\nUser-Agent: codex-cli\nAccept: application/json\n"))?;
    usage_from_json(&body).ok_or_else(|| anyhow::anyhow!("usage windows unavailable"))
}

/// The CLI owns credential decryption and refresh; only account metadata and quotas cross this boundary.
pub(crate) fn probe_native(home: &Path, expected_email: &str) -> anyhow::Result<Usage> {
    use anyhow::Context;
    let mut extra = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    if let Some(home) = kiln_common::paths::home() { extra.insert(0, home.join(".local/bin")); }
    let binary = kiln_common::paths::find_executable("codex", &extra).context("Codex CLI unavailable")?;
    repair_hex_auth(home, expected_email)?;
    probe_native_with(&binary, home, expected_email, std::time::Duration::from_secs(20))
}

/// Restore a hex-rendered JSON file only for the selected account.
fn repair_hex_auth(home: &Path, expected_email: &str) -> anyhow::Result<()> {
    let path = auth_path(home);
    let Some(raw) = crate::store::read_secret(&path)? else { return Ok(()); };
    let Some(decoded) = crate::store::decode_credential_json(&raw) else { return Ok(()); };
    anyhow::ensure!(auth_email(&decoded).is_some_and(|email| email.eq_ignore_ascii_case(expected_email)), "Codex account does not match saved profile");
    anyhow::ensure!(crate::store::read_secret(&path)?.as_deref() == Some(&raw), "Codex credentials changed during recovery");
    crate::store::write_secret_atomic(&path, decoded.as_bytes())
}

fn probe_native_with(binary: &Path, home: &Path, expected_email: &str, timeout: std::time::Duration) -> anyhow::Result<Usage> {
    use anyhow::{Context, ensure};
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let mut command = Command::new(binary);
    #[cfg(unix)]
    { use std::os::unix::process::CommandExt; command.process_group(0); }
    kiln_common::process::hide_window(&mut command);
    let mut child = command.args(["--no-daemon", "app-server"]).env("CODEX_HOME", home)
        .current_dir(home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("Cannot start Codex usage reader")?;
    #[cfg(windows)]
    let job = kiln_common::process::Job::for_process(child.id(), true);
    let mut input = child.stdin.take().context("Codex input unavailable")?;
    let output = child.stdout.take().context("Codex output unavailable")?;
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(output);
        loop {
            let mut line = Vec::new();
            match reader.by_ref().take(1024 * 1024 + 1).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() > 1024 * 1024 => break,
                Ok(_) => if tx.send(serde_json::from_slice::<Value>(&line).ok()).is_err() { break; },
            }
        }
    });
    let result = (|| {
        let deadline = std::time::Instant::now() + timeout;
        let mut request = |id: u64, method: &str, params: Value| -> anyhow::Result<Value> {
            writeln!(input, "{}", serde_json::json!({"id":id,"method":method,"params":params}))?;
            input.flush()?;
            loop {
                ensure!(std::time::Instant::now() < deadline, "Codex usage request timed out");
                let reply = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).context("Codex usage request timed out")?.context("Invalid Codex reply")?;
                if reply["id"].as_u64() != Some(id) { continue; }
                if let Some(error) = reply.get("error") {
                    let message = error["message"].as_str().unwrap_or_default();
                    if message.contains("401") || message.contains("authentication required") {
                        anyhow::bail!("authentication required");
                    }
                    anyhow::bail!("Codex usage request failed; check CLI sign-in");
                }
                if id == 1 { writeln!(input, "{{\"method\":\"initialized\"}}")?; input.flush()?; }
                return reply.get("result").cloned().context("Missing Codex result");
            }
        };
        request(1, "initialize", serde_json::json!({"clientInfo":{"name":"kiln_usage","version":env!("CARGO_PKG_VERSION")}}))?;
        let account = request(2, "account/read", serde_json::json!({"refreshToken":false}))?;
        ensure!(!account["account"].is_null(), "authentication required");
        ensure!(account.pointer("/account/email").and_then(Value::as_str).is_some_and(|email| email.eq_ignore_ascii_case(expected_email)), "Codex account does not match saved profile");
        let limits = request(3, "account/rateLimits/read", serde_json::json!({"excludeResetCreditDetails":true}))?;
        let account = request(4, "account/read", serde_json::json!({"refreshToken":false}))?;
        ensure!(!account["account"].is_null(), "authentication required");
        ensure!(account.pointer("/account/email").and_then(Value::as_str).is_some_and(|email| email.eq_ignore_ascii_case(expected_email)), "Codex account changed during usage request");
        usage_from_native(&limits).context("Codex usage windows unavailable")
    })();
    #[cfg(unix)]
    // SAFETY: this process group was created for this owned reader and its children.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
    #[cfg(windows)]
    drop(job);
    let _ = child.kill();
    let _ = child.wait();
    drop(rx);
    let _ = reader.join();
    result
}

fn usage_from_native(body: &Value) -> Option<Usage> {
    let limits = body.get("rateLimits")?;
    let window = |minutes| {
        let value = ["primary", "secondary"].into_iter().filter_map(|key| limits.get(key)).find(|v| v["windowDurationMins"].as_i64() == Some(minutes))?;
        let percent = value["usedPercent"].as_f64()?;
        if !percent.is_finite() || percent < 0.0 { return None; }
        Some(((percent / 100.0).min(1.0) as f32, value["resetsAt"].as_i64()))
    };
    let (five_hour, seven_day) = (window(300), window(10080));
    let credits = limits.get("credits").filter(|credits| credits.is_object()).map(|credits| crate::Credits {
        has_credits: credits["hasCredits"].as_bool().unwrap_or(false),
        unlimited: credits["unlimited"].as_bool().unwrap_or(false),
        balance: credits["balance"].as_str().filter(|balance| !balance.is_empty()).map(str::to_owned),
    });
    let resets = body.get("rateLimitResetCredits").filter(|resets| resets.is_object()).and_then(|resets| {
        let available = u32::try_from(resets["availableCount"].as_u64()?).ok()?;
        let next_expiry = resets["credits"].as_array().and_then(|credits| credits.iter()
            .filter(|credit| credit["status"].as_str() == Some("available"))
            .filter_map(|credit| credit["expiresAt"].as_i64())
            .min());
        Some(crate::ResetCredits { available, next_expiry })
    });
    (five_hour.is_some() || seven_day.is_some()).then(|| Usage { five_hour, seven_day, status: "allowed".into(), credits, resets, ..Default::default() })
}

pub fn usage_from_json(body: &Value) -> Option<Usage> {
    let limits = body.get("rate_limit")?;
    let window = |seconds: i64| {
        let value = ["primary_window", "secondary_window"].into_iter().filter_map(|key| limits.get(key)).find(|value| value.get("limit_window_seconds").and_then(Value::as_i64) == Some(seconds))?;
        let percent = value.get("used_percent")?.as_f64()?;
        if !percent.is_finite() || percent < 0.0 { return None; }
        Some(((percent / 100.0).min(1.0) as f32, value.get("reset_at").and_then(Value::as_i64)))
    };
    let five_hour = window(18_000);
    let seven_day = window(604_800);
    (five_hour.is_some() || seven_day.is_some()).then(|| Usage { five_hour, seven_day,
        status: if limits.get("limit_reached").and_then(Value::as_bool) == Some(true) { "rejected" } else { "allowed" }.into(), ..Default::default() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    #[ignore = "requires explicit owner authorization for a live account quota request"]
    fn live_account_quota_probe() {
        let home = std::env::var_os("KILN_VERIFY_CODEX_HOME").expect("explicit account home");
        let email = std::env::var("KILN_VERIFY_CODEX_EMAIL").expect("explicit account identity");
        let usage = probe_native(Path::new(&home), &email).expect("live account quota request");
        assert!(!usage.is_unavailable());
        assert!(usage.five_hour.is_some() || usage.seven_day.is_some());
    }

    #[test]
    fn hex_auth_recovery_checks_identity_and_preserves_unknown_formats() {
        let dir = tempfile::tempdir().unwrap();
        let json = fake_auth("fixture@example.test", "fixture");
        let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
        std::fs::write(auth_path(dir.path()), &hex).unwrap();
        assert!(repair_hex_auth(dir.path(), "other@example.test").is_err());
        assert_eq!(std::fs::read_to_string(auth_path(dir.path())).unwrap(), hex);
        repair_hex_auth(dir.path(), "fixture@example.test").unwrap();
        assert_eq!(std::fs::read_to_string(auth_path(dir.path())).unwrap(), json);
        std::fs::write(auth_path(dir.path()), "encrypted-fixture").unwrap();
        repair_hex_auth(dir.path(), "fixture@example.test").unwrap();
        assert_eq!(std::fs::read_to_string(auth_path(dir.path())).unwrap(), "encrypted-fixture");
    }

    #[cfg(unix)]
    #[test]
    fn credits_and_reset_credits_are_read_from_rate_limits() {
        let primary = serde_json::json!({"usedPercent": 10, "windowDurationMins": 300, "resetsAt": 4_200_000_000i64});
        let body = serde_json::json!({
            "rateLimits": {"primary": primary, "credits": {"hasCredits": true, "unlimited": false, "balance": "120"}},
            "rateLimitResetCredits": {"availableCount": 2, "credits": [
                {"id": "a", "resetType": "codexRateLimits", "status": "available", "grantedAt": 1, "expiresAt": 2000},
                {"id": "b", "resetType": "codexRateLimits", "status": "available", "grantedAt": 1, "expiresAt": 1500},
                {"id": "c", "resetType": "codexRateLimits", "status": "redeemed", "grantedAt": 1, "expiresAt": 100},
                {"id": "d", "resetType": "codexRateLimits", "status": "available", "grantedAt": 1, "expiresAt": null}]}
        });
        let usage = usage_from_native(&body).unwrap();
        assert_eq!(usage.credits, Some(crate::Credits { has_credits: true, unlimited: false, balance: Some("120".into()) }));
        assert_eq!(usage.resets, Some(crate::ResetCredits { available: 2, next_expiry: Some(1500) }));
        let counted = usage_from_native(&serde_json::json!({"rateLimits": {"primary": primary, "credits": null}, "rateLimitResetCredits": {"availableCount": 1, "credits": null}})).unwrap();
        assert_eq!((counted.credits, counted.resets), (None, Some(crate::ResetCredits { available: 1, next_expiry: None })));
        let none = usage_from_native(&serde_json::json!({"rateLimits": {"primary": primary}, "rateLimitResetCredits": null})).unwrap();
        assert_eq!(none.resets, None);
    }

    #[test]
    fn native_usage_checks_identity_and_redacts_provider_errors() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("codex-fixture");
        std::fs::write(&script, r#"#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 if r['method']=='initialize': result={}
 elif r['method']=='account/read': result={'account':{'type':'chatgpt','email':'fixture@example.test'}}
 elif r['method']=='account/rateLimits/read': result={'rateLimits':{'primary':{'usedPercent':42,'windowDurationMins':300,'resetsAt':4200000000}}}
 else: raise RuntimeError('unexpected request')
 print(json.dumps({'id':r['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let timeout = std::time::Duration::from_secs(3);
        assert_eq!(probe_native_with(&script, dir.path(), "fixture@example.test", timeout).unwrap().five_hour.unwrap().0, 0.42);
        assert!(probe_native_with(&script, dir.path(), "other@example.test", timeout).unwrap_err().to_string().contains("does not match"));
        let source = std::fs::read_to_string(&script).unwrap().replace("'result':result", "'error':{'message':'PRIVATE PROVIDER BODY'}");
        std::fs::write(&script, source).unwrap();
        let error = probe_native_with(&script, dir.path(), "fixture@example.test", timeout).unwrap_err().to_string();
        assert!(error.contains("request failed"));
        assert!(!error.contains("PRIVATE"));
        std::fs::write(&script, "#!/usr/bin/env python3\nimport time\ntime.sleep(10)\n").unwrap();
        let started = std::time::Instant::now();
        assert!(probe_native_with(&script, dir.path(), "fixture@example.test", std::time::Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn keyring_key_hashes_the_path_as_the_cli_canonicalizes_it() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let hash = format!("{:x}", Sha256::digest(canonical.to_string_lossy().as_bytes()));
        assert_eq!(keyring_account(dir.path()), format!("cli|{}", &hash[..16]));
    }

    /// `KILN_CODEX_KEYRING_HOME` is a fixture Codex home whose `config.toml` selects the
    /// keyring store with `secret_auth_storage` off and that Codex itself signed in to.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the Credential Manager entry a fixture Codex sign-in wrote"]
    fn the_entry_the_cli_writes_is_read_under_its_key() {
        let home = std::path::PathBuf::from(std::env::var_os("KILN_CODEX_KEYRING_HOME").expect("fixture home"));
        let secret = read_credentials(&home, &crate::store::KeyringStore).unwrap().expect("Codex Auth entry");
        assert!(secret.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn usage_json_maps_actual_durations_and_preserves_exhaustion() {
        let usage = usage_from_json(&serde_json::json!({"rate_limit":{
            "primary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_at":4200000000i64},
            "secondary_window":{"used_percent":101,"limit_window_seconds":18000,"reset_at":4200000000i64}
        }})).unwrap();
        assert_eq!(usage.five_hour.unwrap().0, 1.0);
        assert_eq!(usage.seven_day.unwrap().0, 0.1);
        assert!(usage.exhausted_at(crate::now_unix()));
        assert!(usage_from_json(&serde_json::json!({"rate_limit":{"primary_window":{"used_percent":25,"limit_window_seconds":3600}}})).is_none());
    }

    pub fn fake_jwt(claims: &Value) -> String {
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}.sig", e.encode(br#"{"alg":"none"}"#), e.encode(serde_json::to_vec(claims).unwrap()))
    }

    pub fn fake_auth(email: &str, marker: &str) -> String {
        serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": fake_jwt(&serde_json::json!({"email": email, "sub": marker})),
                "access_token": format!("access-{marker}"),
                "refresh_token": format!("refresh-{marker}"),
                "account_id": marker,
            },
            "last_refresh": "2026-09-20T00:00:00Z",
        })
        .to_string()
    }

    #[test]
    fn email_from_id_token_claims() {
        assert_eq!(auth_email(&fake_auth("me@openai.test", "a")).as_deref(), Some("me@openai.test"));
        let nested = serde_json::json!({"tokens": {"id_token": fake_jwt(&serde_json::json!({"https://api.openai.com/profile": {"email": "p@x.com"}}))}}).to_string();
        assert_eq!(auth_email(&nested).as_deref(), Some("p@x.com"));
        assert_eq!(auth_email(r#"{"OPENAI_API_KEY":"sk-x"}"#), None);
        assert_eq!(auth_email(r#"{"tokens":{"id_token":"garbage"}}"#), None);
    }

    #[test]
    fn jwt_payload_with_padding_is_accepted() {
        let e = base64::engine::general_purpose::URL_SAFE;
        let jwt = format!("h.{}.s", e.encode(br#"{"email":"pad@x.com"}"#));
        assert_eq!(jwt_claims(&jwt).unwrap()["email"], "pad@x.com");
    }

    #[test]
    fn rfc3339_to_unix() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_utc("2026-09-24T14:52:55.688Z"), Some(1_790_261_575));
        assert_eq!(parse_rfc3339_utc("2026-09-24T23:52:55+09:00"), Some(1_790_261_575));
    }

    #[test]
    fn rate_limits_map_to_windows() {
        let now = 1_790_000_000;
        let rl = serde_json::json!({
            "limit_id": "codex",
            "primary": {"used_percent": 42.0, "window_minutes": 300, "resets_at": now + 3600},
            "secondary": {"used_percent": 66.0, "window_minutes": 10080, "resets_at": now + 86400},
            "plan_type": "pro",
            "rate_limit_reached_type": null
        });
        let u = usage_from_rate_limits(&rl, now).unwrap();
        assert_eq!(u.five_hour, Some((0.42, Some(now + 3600))));
        assert_eq!(u.seven_day, Some((0.66, Some(now + 86400))));
        assert_eq!(u.status, "allowed · pro");

        let weekly_only = serde_json::json!({"primary": {"used_percent": 100.0, "window_minutes": 10080, "resets_at": now - 1}, "secondary": null, "rate_limit_reached_type": "primary"});
        let u = usage_from_rate_limits(&weekly_only, now).unwrap();
        assert_eq!(u.five_hour, None);
        assert_eq!(u.seven_day, Some((0.0, Some(now - 1))));
        assert_eq!(u.status, "rejected");
    }

    #[test]
    fn latest_observed_scans_session_tree() {
        let d = tempfile::tempdir().unwrap();
        let day = d.path().join("sessions/2026/09/24");
        std::fs::create_dir_all(&day).unwrap();
        let line = |ts: &str, pct: f64| {
            serde_json::json!({"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {"primary": {"used_percent": pct, "window_minutes": 10080, "resets_at": 1_890_000_000i64}, "secondary": null}}}).to_string()
        };
        let null_line = serde_json::json!({"timestamp": "2026-09-24T15:00:00Z", "type": "event_msg", "payload": {"type": "token_count", "rate_limits": null}}).to_string();
        std::fs::write(day.join("rollout-a.jsonl"), format!("{}\n{}\n{}\n", line("2026-09-24T10:00:00Z", 10.0), line("2026-09-24T14:52:55Z", 66.0), null_line)).unwrap();
        std::fs::write(day.join("rollout-b.jsonl"), format!("{}\n", line("2026-09-24T12:00:00Z", 50.0))).unwrap();
        let o = latest_observed(d.path(), 1_790_000_000).unwrap();
        assert_eq!(o.at_unix, parse_rfc3339_utc("2026-09-24T14:52:55Z").unwrap());
        assert_eq!(o.usage.seven_day.unwrap().0, 0.66);
        assert!(latest_observed(&d.path().join("missing"), 0).is_none());
    }
}
