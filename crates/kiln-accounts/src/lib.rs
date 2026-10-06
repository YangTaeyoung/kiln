//! Claude Code·OpenAI Codex CLI 의 구독 계정 여러 개를 저장·전환하고 사용량을 보여 준다.
//!
//! Claude Code 자격증명은 macOS 키체인 항목 `Claude Code-credentials`, 그 밖의 OS 는
//! `~/.claude/.credentials.json`(0600)에 있다. Codex 는 `$CODEX_HOME/auth.json`(0600)이다.
//! 프로필 스냅샷은 macOS 키체인(그 밖의 OS 는 0600 파일)에 두고, 설정 파일에는 비밀값을 넣지 않는다.

pub mod claude;
pub mod codex;
mod limit;
mod manager;
mod login;
pub mod store;
mod ui;

pub use limit::{LimitHit, detect_limit};
pub use manager::{AccountManager, Env};
pub use login::LoginStatus;
pub use store::{CredentialStore, FileStore, MemoryStore, SecurityCli};
pub use ui::{AccountsEvent, accounts_settings_ui};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Claude, Tool::Codex];

    pub fn key(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Tool::Claude => "Claude Code",
            Tool::Codex => "Codex",
        }
    }
}

/// 저장된 계정 하나. 비밀값은 들어 있지 않다.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub tool: Tool,
    pub label: String,
    pub email: Option<String>,
    pub added_unix: u64,
}

/// 사용량. 창마다 (사용률 0..1, 초기화 unix 초). `status` 는 `allowed`·`allowed_warning`·`rejected`
/// 같은 상태 값이거나, 조회 실패면 `unavailable: <이유>` 다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub five_hour: Option<(f32, Option<i64>)>,
    pub seven_day: Option<(f32, Option<i64>)>,
    pub status: String,
}

impl Usage {
    pub fn unavailable(reason: impl std::fmt::Display) -> Self {
        Usage { five_hour: None, seven_day: None, status: format!("unavailable: {reason}") }
    }

    pub fn is_unavailable(&self) -> bool {
        self.status.starts_with("unavailable")
    }

    /// 초기화 시각이 지난 창은 사용률 0 으로 본 창 값.
    pub fn effective(w: Option<(f32, Option<i64>)>, now_unix: i64) -> Option<(f32, Option<i64>)> {
        w.map(|(u, r)| if r.is_some_and(|r| r <= now_unix) { (0.0, r) } else { (u, r) })
    }

    /// 지금 이 계정의 한도가 소진된 상태인지.
    pub fn exhausted_at(&self, now_unix: i64) -> bool {
        let windows = [self.five_hour, self.seven_day];
        let full = windows.iter().any(|w| Usage::effective(*w, now_unix).is_some_and(|(u, _)| u >= 0.999));
        let resets: Vec<i64> = windows.iter().filter_map(|w| w.and_then(|(_, r)| r)).collect();
        let all_reset_passed = !resets.is_empty() && resets.iter().all(|r| *r <= now_unix);
        full || (self.status.starts_with("rejected") && !all_reset_passed)
    }
}

/// 새 계정 로그인에 쓰는 명령. 앱이 터미널에서 실행한 뒤 `save_current` 를 부른다.
pub fn login_command(tool: Tool) -> String {
    match tool {
        Tool::Claude => "claude auth login".to_string(),
        Tool::Codex => "codex login".to_string(),
    }
}

/// 가장 최근 세션을 이어 가는 명령.
pub fn resume_command(tool: Tool) -> &'static str {
    match tool {
        Tool::Claude => "claude --continue",
        Tool::Codex => "codex resume --last",
    }
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_detection() {
        let now = 1_000_000;
        let full = Usage { five_hour: Some((1.0, Some(now + 60))), seven_day: Some((0.2, None)), status: "allowed".into() };
        assert!(full.exhausted_at(now));
        assert!(!full.exhausted_at(now + 61));
        let rejected = Usage { five_hour: Some((0.5, Some(now + 60))), seven_day: None, status: "rejected".into() };
        assert!(rejected.exhausted_at(now));
        assert!(!rejected.exhausted_at(now + 60));
        let rejected_no_reset = Usage { status: "rejected".into(), ..Default::default() };
        assert!(rejected_no_reset.exhausted_at(now));
        assert!(!Usage { status: "allowed_warning".into(), five_hour: Some((0.95, None)), seven_day: None }.exhausted_at(now));
        assert!(!Usage::unavailable("x").exhausted_at(now));
    }

    #[test]
    fn commands() {
        assert_eq!(resume_command(Tool::Claude), "claude --continue");
        assert_eq!(resume_command(Tool::Codex), "codex resume --last");
        assert_eq!(login_command(Tool::Codex), "codex login");
    }
}
