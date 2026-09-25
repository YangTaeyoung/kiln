//! 시간 표시 유틸리티.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `now` 기준 상대 시간 문구(`3시간 전`).
pub fn relative_time(ts: i64, now: i64) -> String {
    let d = (now - ts).max(0);
    let (n, unit) = if d < 45 {
        return "방금".into();
    } else if d < 3600 {
        ((d + 30) / 60, "분")
    } else if d < 86_400 {
        ((d + 1800) / 3600, "시간")
    } else if d < 86_400 * 30 {
        ((d + 43_200) / 86_400, "일")
    } else if d < 86_400 * 365 {
        ((d + 86_400 * 15) / (86_400 * 30), "개월")
    } else {
        (d / (86_400 * 365), "년")
    };
    format!("{}{unit} 전", n.max(1))
}

/// 짧은 상대 시간(`3시간`, `2일`).
pub fn short_relative_time(ts: i64, now: i64) -> String {
    let d = (now - ts).max(0);
    if d < 60 {
        "방금".into()
    } else if d < 3600 {
        format!("{}분", d / 60)
    } else if d < 86_400 {
        format!("{}시간", d / 3600)
    } else if d < 86_400 * 30 {
        format!("{}일", d / 86_400)
    } else if d < 86_400 * 365 {
        format!("{}개월", d / (86_400 * 30))
    } else {
        format!("{}년", d / (86_400 * 365))
    }
}

/// `2026-09-23T21:12:43Z` 형식(UTC)을 유닉스 초로 바꾼다.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut t = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss;
    // 오프셋(`+09:00`)
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = rest.chars().next()
        && (sign == '+' || sign == '-')
        && rest.len() >= 6
    {
        let oh: i64 = rest[1..3].parse().ok()?;
        let om: i64 = rest[4..6].parse().ok()?;
        let off = oh * 3600 + om * 60;
        t -= if sign == '+' { off } else { -off };
    }
    Some(t)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// ISO 시간을 상대 시간 문구로.
pub fn relative_iso(s: &str, now: i64) -> String {
    parse_iso8601(s).map(|t| relative_time(t, now)).unwrap_or_default()
}
