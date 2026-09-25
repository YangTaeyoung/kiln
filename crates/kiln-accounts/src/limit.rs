//! 터미널 화면 글자에서 구독 사용량 한도 도달 메시지를 찾는다.

use crate::Tool;

/// 한도 도달 감지 결과. `reset_hint` 는 CLI 가 출력한 초기화 시각 문구 그대로다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitHit {
    pub reset_hint: Option<String>,
}

/// Claude Code 의 `You've hit your …` 뒤에 올 수 있는 구독 한도 이름.
const CLAUDE_HIT_KINDS: &[&str] = &["session limit", "weekly limit", "opus limit", "sonnet limit", "fable limit", "usage limit", "limit"];

/// 줄 머리에서 이 문구로 시작하면 한도 도달로 본다(소문자, 아포스트로피는 `'` 로 정규화한 뒤 비교).
const CLAUDE_PREFIXES: &[&str] = &["usage limit reached", "claude usage limit reached", "5-hour limit reached"];
const CODEX_PREFIXES: &[&str] = &["you've hit your usage limit", "you've reached your usage limit", "usage limit reached. you've reached your usage limit"];

/// 줄 앞의 기호·공백(⎿, ■, │, ●, ⚠ 등)을 걷어 낸다.
fn strip_lead(line: &str) -> &str {
    line.trim_start_matches(|c: char| !c.is_alphanumeric())
}

fn normalize(line: &str) -> String {
    line.replace(['\u{2019}', '\u{2018}'], "'").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn claude_match(lower: &str) -> bool {
    if CLAUDE_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    let Some(rest) = lower.strip_prefix("you've hit your ") else {
        return false;
    };
    CLAUDE_HIT_KINDS.iter().any(|k| {
        rest.strip_prefix(k)
            .is_some_and(|after| after.is_empty() || after.starts_with([' ', '.', ',', '\u{b7}', '\u{2219}', '\u{2022}', '-', '(']))
    })
}

fn codex_match(lower: &str) -> bool {
    CODEX_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// `resets …`, `reset at …`, `try again at …` 뒤의 문구를 꺼낸다.
fn reset_hint(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    for key in ["try again at ", "will reset at ", "resets at ", "resets "] {
        if let Some(i) = lower.rfind(key) {
            let tail = &line[i + key.len()..];
            let end = tail.find(" \u{b7} ").or_else(|| tail.find(" | ")).unwrap_or(tail.len());
            let mut hint = tail[..end].trim().trim_end_matches('.').trim();
            if hint.ends_with(')') && !hint.contains('(') {
                hint = &hint[..hint.len() - 1];
            }
            if hint.starts_with(|c: char| c.is_alphanumeric()) {
                return Some(hint.to_string());
            }
        }
    }
    None
}

/// 화면 글자에서 마지막으로 나온 한도 도달 메시지를 찾는다.
pub fn detect_limit(tool: Tool, screen_text: &str) -> Option<LimitHit> {
    screen_text.lines().rev().find_map(|raw| {
        let line = normalize(strip_lead(raw));
        let lower = line.to_lowercase();
        let hit = match tool {
            Tool::Claude => claude_match(&lower),
            Tool::Codex => codex_match(&lower),
        };
        hit.then(|| LimitHit { reset_hint: reset_hint(&line) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(tool: Tool, s: &str) -> Option<Option<String>> {
        detect_limit(tool, s).map(|h| h.reset_hint)
    }

    #[test]
    fn claude_composed_hit_messages() {
        assert_eq!(hint(Tool::Claude, "  ⎿  You've hit your session limit · resets 3pm (Asia/Seoul)"), Some(Some("3pm (Asia/Seoul)".into())));
        assert_eq!(hint(Tool::Claude, "You've hit your weekly limit · resets Oct 1, 9am (Asia/Seoul)"), Some(Some("Oct 1, 9am (Asia/Seoul)".into())));
        assert_eq!(hint(Tool::Claude, "You've hit your Opus limit · resets 5am"), Some(Some("5am".into())));
        assert_eq!(hint(Tool::Claude, "You've hit your usage limit"), Some(None));
        assert_eq!(hint(Tool::Claude, "You've hit your limit · resets 11pm · progress saved"), Some(Some("11pm".into())));
    }

    #[test]
    fn claude_waiting_and_legacy_messages() {
        assert!(detect_limit(Tool::Claude, "Usage limit reached · continuing automatically when it resets · esc to cancel").is_some());
        assert!(detect_limit(Tool::Claude, "Usage limit reached · continuing shortly · esc to cancel").is_some());
        assert_eq!(
            hint(Tool::Claude, "Claude usage limit reached. Your limit will reset at 3pm (Asia/Seoul)."),
            Some(Some("3pm (Asia/Seoul)".into()))
        );
        assert_eq!(hint(Tool::Claude, "5-hour limit reached ∙ resets 7pm"), Some(Some("7pm".into())));
    }

    #[test]
    fn claude_ignores_non_subscription_limits() {
        assert!(detect_limit(Tool::Claude, "You've hit your fast limit · resets in 8m").is_none());
        assert!(detect_limit(Tool::Claude, "Context limit reached · /compact or /clear to continue").is_none());
        assert!(detect_limit(Tool::Claude, "You've hit your monthly spend limit.").is_none());
        assert!(detect_limit(Tool::Claude, "I explained that you've hit your session limit earlier").is_none());
        assert!(detect_limit(Tool::Claude, "").is_none());
    }

    #[test]
    fn codex_hit_messages() {
        let s = "■ You’ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Sep 27th, 2026 3:04 PM.";
        assert_eq!(hint(Tool::Codex, s), Some(Some("Sep 27th, 2026 3:04 PM".into())));
        assert_eq!(hint(Tool::Codex, "You've hit your usage limit. Try again at 9:15 AM."), Some(Some("9:15 AM".into())));
        assert_eq!(hint(Tool::Codex, "You’ve hit your usage limit."), Some(None));
        assert!(detect_limit(Tool::Codex, "Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.").is_some());
        assert!(detect_limit(Tool::Codex, "You've hit your spend cap set by the owner of your workspace.").is_none());
        assert!(detect_limit(Tool::Codex, "You've hit your session limit · resets 3pm").is_none());
    }

    #[test]
    fn uses_last_matching_line() {
        let screen = "You've hit your session limit · resets 3pm\n> continue\nYou've hit your weekly limit · resets Oct 2\n";
        assert_eq!(hint(Tool::Claude, screen), Some(Some("Oct 2".into())));
    }
}
