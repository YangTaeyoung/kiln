//! Kiln 의 Git 연동: `git` CLI 기반 상태/조작, diff 뷰어, GitHub PR(`gh` CLI).
//!
//! 모든 git/gh 명령은 백그라운드 스레드에서 실행되고 UI 는 결과를 폴링한다.

pub mod cmd;
pub mod diff;
pub mod discovery;
pub mod gh;
pub mod github;
pub mod graph;
pub mod history;
pub mod history_guard;
pub mod repo;
pub mod status;
mod ui;
pub mod util;
pub mod worktrees;

use std::path::{Path, PathBuf};

pub use cmd::{GitError, GitResult};
pub use gh::{
    ChecksState, GhBackend, MergeMethod, PrBackend, PrBrief, PrCreate, PrCreateDefaults, PrDetail, PrFilter, PrItem,
    PrState, ReviewDecision, ReviewKind, pr_for_branch,
};
pub use github::{
    CloseReason, GithubBackend, IssueCreate, IssueDetail, IssueEdit, IssueFilter, IssueItem, IssueState, Label, RepoInfo,
    RepoListItem, RepoRef, RunFilter, RunItem, RunStatus, Viewer,
};
pub use ui::actions_panel::ActionsPanel;
pub use ui::diff_view::{DiffMode, DiffView};
pub use ui::history_view::HistoryView;
pub use ui::hub::{GithubHub, HubTab, GithubDrafts, RepositoryDrafts};
pub use ui::issue_panel::IssuePanel;
pub use ui::issue_view::IssueView;
pub use ui::repo_picker::RepoPicker;
pub use ui::panel::GitPanel;
pub use ui::pr_panel::{PrPanel, PrCreationDraft};
pub use ui::pr_view::{PrTab, PrView};

/// Git UI 가 앱에 요청하는 동작.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitEvent {
    /// 에디터로 파일 열기(절대 경로).
    OpenFile(PathBuf),
    /// 파일 diff 탭 열기(절대 경로). `DiffView::for_file` 에 그대로 넘긴다.
    OpenDiff { path: PathBuf, staged: bool },
    OpenPr(u64),
    OpenCommit(String),
    /// Open the current panel repository's editable commit history.
    OpenHistory,
    History(history::HistoryEvent),
    RunInTerminal(String),
    /// 이슈 상세 탭 열기. 대상 저장소는 `GithubHub::repo()` 로 얻는다.
    OpenIssue(u64),
    /// 저장소를 새 스페이스로 복제한다(`gh repo clone <name_with_owner>`).
    CloneRepo { name_with_owner: String },
    /// 브라우저로 URL 열기.
    OpenUrl(String),
}

/// 상태바용 저장소 요약.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSummary {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub conflicted: u32,
    pub pr: Option<PrBrief>,
}

/// 저장소 요약을 만든다(블로킹, gh 조회 포함). 저장소가 아니거나 git 이 없으면 `None`.
pub fn repo_summary(root: &Path) -> Option<RepoSummary> {
    let st = repo::status(root).ok()?;
    let branch = st.branch.display_name();
    let pr = st.branch.head.as_deref().and_then(|b| pr_for_branch(root, b));
    Some(RepoSummary {
        branch,
        ahead: st.branch.ahead,
        behind: st.branch.behind,
        changed: st.changed_count(),
        conflicted: st.conflicted_count(),
        pr,
    })
}
