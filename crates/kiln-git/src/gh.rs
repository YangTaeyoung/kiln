//! GitHub CLI(`gh`) 연동: PR 모델, JSON 파싱, 백엔드 트레이트.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cmd::{GitError, GitResult, gh, gh_stdin};
use crate::github::RepoRef;

/// 체크 결과 요약.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksState {
    Pass,
    Fail,
    Pending,
}

/// 리뷰 결정.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl ReviewDecision {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "APPROVED" => Some(Self::Approved),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "REVIEW_REQUIRED" => Some(Self::ReviewRequired),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => kiln_common::i18n::tr("승인됨"),
            Self::ChangesRequested => kiln_common::i18n::tr("변경 요청됨"),
            Self::ReviewRequired => kiln_common::i18n::tr("리뷰 필요"),
        }
    }
}

/// PR 상태.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl PrState {
    pub fn parse(s: &str) -> Self {
        match s {
            "MERGED" => Self::Merged,
            "CLOSED" => Self::Closed,
            _ => Self::Open,
        }
    }
}

/// 상태바 등에 쓰는 PR 요약.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrBrief {
    pub number: u64,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub checks: Option<ChecksState>,
    pub review: Option<ReviewDecision>,
    pub url: String,
}

// ---------------------------------------------------------------- JSON 원본 타입

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Author {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_bot: bool,
}

/// `statusCheckRollup` 항목(CheckRun 또는 StatusContext).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    #[serde(rename = "__typename", default)]
    pub typename: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub details_url: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub workflow_name: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

impl CheckItem {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().or(self.context.as_deref()).unwrap_or("check")
    }

    pub fn url(&self) -> Option<&str> {
        self.details_url.as_deref().or(self.target_url.as_deref()).filter(|s| !s.is_empty())
    }

    /// 개별 체크 상태. 건너뜀/중립은 `None`.
    pub fn outcome(&self) -> Option<ChecksState> {
        if let Some(s) = &self.state {
            return match s.as_str() {
                "SUCCESS" => Some(ChecksState::Pass),
                "FAILURE" | "ERROR" => Some(ChecksState::Fail),
                _ => Some(ChecksState::Pending),
            };
        }
        match self.status.as_deref() {
            Some("COMPLETED") | None => match self.conclusion.as_deref() {
                Some("SUCCESS") => Some(ChecksState::Pass),
                Some("NEUTRAL") | Some("SKIPPED") => None,
                Some("FAILURE") | Some("TIMED_OUT") | Some("CANCELLED") | Some("ACTION_REQUIRED")
                | Some("STARTUP_FAILURE") | Some("STALE") => Some(ChecksState::Fail),
                Some(_) => Some(ChecksState::Pending),
                None => Some(ChecksState::Pending),
            },
            _ => Some(ChecksState::Pending),
        }
    }

    /// 표시용 상태 문구.
    pub fn outcome_label(&self) -> String {
        let raw = match (&self.state, self.status.as_deref()) {
            (Some(s), _) => s.as_str(),
            (None, Some("COMPLETED")) => self.conclusion.as_deref().unwrap_or("COMPLETED"),
            (None, Some(s)) => s,
            (None, None) => return kiln_common::i18n::tr("알 수 없음").into(),
        };
        let label = match raw.to_ascii_uppercase().as_str() {
            "SUCCESS" => kiln_common::i18n::tr("성공"),
            "FAILURE" => kiln_common::i18n::tr("실패"),
            "ERROR" => kiln_common::i18n::tr("오류"),
            "NEUTRAL" => kiln_common::i18n::tr("중립"),
            "SKIPPED" => kiln_common::i18n::tr("건너뜀"),
            "CANCELLED" => kiln_common::i18n::tr("취소됨"),
            "TIMED_OUT" => kiln_common::i18n::tr("시간 초과"),
            "ACTION_REQUIRED" => kiln_common::i18n::tr("조치 필요"),
            "STARTUP_FAILURE" => kiln_common::i18n::tr("시작 실패"),
            "STALE" => kiln_common::i18n::tr("오래됨"),
            "COMPLETED" => kiln_common::i18n::tr("완료"),
            "IN_PROGRESS" => kiln_common::i18n::tr("진행 중"),
            "QUEUED" => kiln_common::i18n::tr("대기 중"),
            "PENDING" => kiln_common::i18n::tr("대기 중"),
            "WAITING" => kiln_common::i18n::tr("대기 중"),
            "REQUESTED" => kiln_common::i18n::tr("요청됨"),
            "EXPECTED" => kiln_common::i18n::tr("예정됨"),
            _ => return raw.to_lowercase().replace('_', " "),
        };
        label.into()
    }
}

/// 체크 목록을 하나의 상태로 모은다. 비어 있으면 `None`.
pub fn rollup(items: &[CheckItem]) -> Option<ChecksState> {
    if items.is_empty() {
        return None;
    }
    let mut pending = false;
    for c in items {
        match c.outcome() {
            Some(ChecksState::Fail) => return Some(ChecksState::Fail),
            Some(ChecksState::Pending) => pending = true,
            _ => {}
        }
    }
    Some(if pending { ChecksState::Pending } else { ChecksState::Pass })
}

/// `gh pr list --json ...` 한 건.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrItem {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub review_decision: String,
    #[serde(default)]
    pub status_check_rollup: Vec<CheckItem>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

impl PrItem {
    pub fn pr_state(&self) -> PrState {
        PrState::parse(&self.state)
    }
    pub fn checks(&self) -> Option<ChecksState> {
        rollup(&self.status_check_rollup)
    }
    pub fn review(&self) -> Option<ReviewDecision> {
        ReviewDecision::parse(&self.review_decision)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrComment {
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrReview {
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub body: String,
    /// APPROVED / CHANGES_REQUESTED / COMMENTED / DISMISSED / PENDING
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub submitted_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrFile {
    pub path: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub change_type: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct PrLabel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrCommit {
    #[serde(default)]
    pub oid: String,
    #[serde(default)]
    pub message_headline: String,
}

/// `gh pr view --json ...` 결과.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub mergeable: String,
    #[serde(default)]
    pub review_decision: String,
    #[serde(default)]
    pub status_check_rollup: Vec<CheckItem>,
    #[serde(default)]
    pub reviews: Vec<PrReview>,
    #[serde(default)]
    pub comments: Vec<PrComment>,
    #[serde(default)]
    pub files: Vec<PrFile>,
    #[serde(default)]
    pub labels: Vec<PrLabel>,
    #[serde(default)]
    pub commits: Vec<PrCommit>,
}

impl PrDetail {
    pub fn pr_state(&self) -> PrState {
        PrState::parse(&self.state)
    }
    pub fn checks(&self) -> Option<ChecksState> {
        rollup(&self.status_check_rollup)
    }
    pub fn review(&self) -> Option<ReviewDecision> {
        ReviewDecision::parse(&self.review_decision)
    }
}

pub const PR_LIST_FIELDS: &str =
    "number,title,author,headRefName,baseRefName,state,isDraft,reviewDecision,statusCheckRollup,updatedAt,url,additions,deletions";
pub const PR_VIEW_FIELDS: &str = "number,title,body,author,state,isDraft,headRefName,baseRefName,additions,deletions,changedFiles,url,createdAt,updatedAt,mergeable,reviewDecision,statusCheckRollup,reviews,comments,files,labels,commits";
const PR_BRIEF_FIELDS: &str = "number,title,state,isDraft,statusCheckRollup,reviewDecision,url";

pub fn parse_pr_list(json: &str) -> GitResult<Vec<PrItem>> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

pub fn parse_pr_view(json: &str) -> GitResult<PrDetail> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

pub fn parse_pr_brief(json: &str) -> GitResult<PrBrief> {
    let d: PrItem = serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))?;
    Ok(PrBrief {
        number: d.number,
        title: d.title.clone(),
        state: d.pr_state(),
        is_draft: d.is_draft,
        checks: d.checks(),
        review: d.review(),
        url: d.url.clone(),
    })
}

/// 현재 브랜치에 연결된 PR. 없거나 gh 를 쓸 수 없으면 `None`.
pub fn pr_for_branch(root: &Path, branch: &str) -> Option<PrBrief> {
    let out = gh(root, &["pr", "view", branch, "--json", PR_BRIEF_FIELDS]).ok()?;
    parse_pr_brief(&out).ok()
}

// ---------------------------------------------------------------- 백엔드

/// PR 목록 필터.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PrFilter {
    #[default]
    Open,
    Mine,
    ReviewRequested,
    All,
}

impl PrFilter {
    pub const ALL: [PrFilter; 4] = [PrFilter::Open, PrFilter::Mine, PrFilter::ReviewRequested, PrFilter::All];

    pub fn label(self) -> &'static str {
        match self {
            PrFilter::Open => kiln_common::i18n::tr("열림"),
            PrFilter::Mine => kiln_common::i18n::tr("내 PR"),
            PrFilter::ReviewRequested => kiln_common::i18n::tr("리뷰 요청됨"),
            PrFilter::All => kiln_common::i18n::tr("전체"),
        }
    }

    /// `gh pr list` 추가 인자.
    pub fn gh_args(self) -> Vec<&'static str> {
        match self {
            PrFilter::Open => vec!["--state", "open"],
            PrFilter::Mine => vec!["--state", "open", "--author", "@me"],
            PrFilter::ReviewRequested => vec!["--state", "open", "--search", "review-requested:@me"],
            PrFilter::All => vec!["--state", "all"],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewKind {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MergeMethod {
    #[default]
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Squash => kiln_common::i18n::tr("스쿼시 후 병합"),
            MergeMethod::Merge => kiln_common::i18n::tr("병합 커밋 만들기"),
            MergeMethod::Rebase => kiln_common::i18n::tr("리베이스 후 병합"),
        }
    }
    fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

/// PR 생성 요청.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PrCreate {
    #[serde(default)]
    pub head: String,
    pub title: String,
    pub body: String,
    pub base: String,
    pub draft: bool,
}

/// PR 생성 폼 기본값.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrCreateDefaults {
    pub head: String,
    pub base: String,
    pub bases: Vec<String>,
    pub title: String,
    pub body: String,
}

/// PR 데이터 소스. 기본 구현은 `gh` CLI 이고, 테스트에서는 가짜 구현을 주입한다.
pub trait PrBackend: Send + Sync + 'static {
    fn list(&self, filter: PrFilter) -> GitResult<Vec<PrItem>>;
    fn view(&self, number: u64) -> GitResult<PrDetail>;
    fn diff(&self, number: u64) -> GitResult<String>;
    fn create_defaults(&self) -> GitResult<PrCreateDefaults>;
    /// 생성된 PR URL 을 돌려준다.
    fn create(&self, req: &PrCreate) -> GitResult<String>;
    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> GitResult<()>;
    fn merge(&self, number: u64, method: MergeMethod, delete_branch: bool) -> GitResult<String>;
    fn checkout(&self, number: u64) -> GitResult<String>;
    fn mark_ready(&self, number: u64) -> GitResult<()>;
}

/// `gh` CLI 기반 백엔드. `repo` 가 있으면 PR 명령에 `-R owner/name` 을 붙인다.
pub struct GhBackend {
    pub(crate) root: PathBuf,
    repo: Option<RepoRef>,
}

impl GhBackend {
    pub fn new(root: PathBuf) -> Self {
        Self { root, repo: None }
    }

    /// 지정한 저장소를 대상으로 하는 백엔드. `None` 이면 `new` 와 같다.
    pub fn for_repo(root: PathBuf, repo: Option<RepoRef>) -> Self {
        Self { root, repo }
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        self.repo.as_ref()
    }

    fn pr_gh(&self, args: &[&str]) -> GitResult<String> {
        self.pr_gh_stdin(args, None)
    }

    fn pr_gh_stdin(&self, args: &[&str], input: Option<&[u8]>) -> GitResult<String> {
        let extra = crate::github::repo_args(self.repo.as_ref());
        let mut all: Vec<&str> = args.to_vec();
        all.extend(extra.iter().map(String::as_str));
        gh_stdin(&self.root, &all, input)
    }
}

impl PrBackend for GhBackend {
    fn list(&self, filter: PrFilter) -> GitResult<Vec<PrItem>> {
        let mut args = vec!["pr", "list", "--limit", "50", "--json", PR_LIST_FIELDS];
        args.extend(filter.gh_args());
        parse_pr_list(&self.pr_gh(&args)?)
    }

    fn view(&self, number: u64) -> GitResult<PrDetail> {
        let n = number.to_string();
        parse_pr_view(&self.pr_gh(&["pr", "view", &n, "--json", PR_VIEW_FIELDS])?)
    }

    fn diff(&self, number: u64) -> GitResult<String> {
        let n = number.to_string();
        self.pr_gh(&["pr", "diff", &n, "--color", "never"])
    }

    fn create_defaults(&self) -> GitResult<PrCreateDefaults> {
        let root = &self.root;
        let st = crate::repo::status(root)?;
        let head = st.branch.head.clone().ok_or_else(|| GitError::Failed(kiln_common::i18n::tr("분리된 HEAD입니다 — 먼저 브랜치로 전환하세요").into()))?;
        let repo_name = self.repo.as_ref().map(RepoRef::full_name);
        let mut view_args = vec!["repo", "view"];
        if let Some(n) = &repo_name {
            view_args.push(n);
        }
        view_args.extend(["--json", "defaultBranchRef", "-q", ".defaultBranchRef.name"]);
        let base = gh(root, &view_args)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "main".into());
        let mut bases: Vec<String> = crate::repo::branches(root)
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.remote && b.name.starts_with("origin/"))
            .map(|b| b.name.trim_start_matches("origin/").to_string())
            .filter(|b| b != &head)
            .collect();
        if !bases.contains(&base) {
            bases.insert(0, base.clone());
        }
        let commits = crate::repo::commits_since(root, &format!("origin/{base}"))
            .or_else(|_| crate::repo::commits_since(root, &base))
            .unwrap_or_default();
        let (title, body) = prefill_from_commits(&head, &commits);
        Ok(PrCreateDefaults { head, base, bases, title, body })
    }

    fn create(&self, req: &PrCreate) -> GitResult<String> {
        let mut args = vec!["pr", "create", "--title", &req.title, "--base", &req.base, "--body-file", "-"];
        if !req.head.is_empty() { args.extend(["--head", &req.head]); }
        if req.draft {
            args.push("--draft");
        }
        let out = self.pr_gh_stdin(&args, Some(req.body.as_bytes()))?;
        Ok(out.lines().rev().find(|l| l.starts_with("http")).unwrap_or(out.trim()).to_string())
    }

    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> GitResult<()> {
        let n = number.to_string();
        let flag = match kind {
            ReviewKind::Approve => "--approve",
            ReviewKind::RequestChanges => "--request-changes",
            ReviewKind::Comment => "--comment",
        };
        let mut args = vec!["pr", "review", &n, flag];
        if !body.trim().is_empty() || kind != ReviewKind::Approve {
            args.push("--body-file");
            args.push("-");
        }
        self.pr_gh_stdin(&args, Some(body.as_bytes())).map(|_| ())
    }

    fn merge(&self, number: u64, method: MergeMethod, delete_branch: bool) -> GitResult<String> {
        let n = number.to_string();
        let mut args = vec!["pr", "merge", &n, method.flag()];
        if delete_branch {
            args.push("--delete-branch");
        }
        self.pr_gh(&args)
    }

    fn checkout(&self, number: u64) -> GitResult<String> {
        let n = number.to_string();
        self.pr_gh(&["pr", "checkout", &n])
    }

    fn mark_ready(&self, number: u64) -> GitResult<()> {
        let n = number.to_string();
        self.pr_gh(&["pr", "ready", &n]).map(|_| ())
    }
}

/// 브랜치 커밋으로 PR 제목/본문을 만든다. 커밋이 하나면 그 제목·본문을, 여럿이면 브랜치 이름과 커밋 목록을 쓴다.
pub fn prefill_from_commits(branch: &str, commits: &[(String, String)]) -> (String, String) {
    match commits {
        [] => (humanize_branch(branch), String::new()),
        [(s, b)] => (s.clone(), b.clone()),
        many => {
            let body = many.iter().map(|(s, _)| format!("- {s}")).collect::<Vec<_>>().join("\n");
            (humanize_branch(branch), body)
        }
    }
}

fn humanize_branch(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let s = last.replace(['-', '_'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
