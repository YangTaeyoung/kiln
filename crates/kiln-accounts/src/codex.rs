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
    Some(Usage { five_hour: five, seven_day: seven, status })
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
