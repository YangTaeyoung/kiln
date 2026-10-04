//! GitHub 허브 데이터: 저장소, 이슈, Actions 실행 모델과 `gh` 기반 백엔드.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Deserializer};

use crate::cmd::{GitError, GitResult, gh, gh_stdin};
use crate::gh::{Author, GhBackend, PrBackend, PrComment, PrLabel};

// ---------------------------------------------------------------- 저장소

/// `owner/name` 형태의 GitHub 저장소 식별자.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self { owner: owner.into(), name: name.into() }
    }

    /// `owner/name` 문자열을 해석한다. 형식이 맞지 않으면 `None`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_end_matches(".git");
        let s = s.rsplit_once("github.com/").map(|(_, r)| r).unwrap_or(s);
        let (o, n) = s.split_once('/')?;
        let n = n.split('/').next().unwrap_or(n);
        if o.is_empty() || n.is_empty() || o.contains(char::is_whitespace) || n.contains(char::is_whitespace) {
            return None;
        }
        Some(Self::new(o, n))
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct TotalCount {
    #[serde(default, rename = "totalCount")]
    total_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
struct BranchRef {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoViewJson {
    name_with_owner: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    default_branch_ref: Option<BranchRef>,
    #[serde(default)]
    stargazer_count: u64,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    is_private: bool,
    #[serde(default)]
    viewer_permission: Option<String>,
    #[serde(default)]
    issues: Option<TotalCount>,
    #[serde(default)]
    pull_requests: Option<TotalCount>,
    #[serde(default)]
    has_issues_enabled: Option<bool>,
}

/// `gh repo view --json ...` 결과 요약.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoInfo {
    pub repo: RepoRef,
    pub description: String,
    pub url: String,
    pub default_branch: String,
    pub stargazer_count: u64,
    /// `PUBLIC` / `PRIVATE` / `INTERNAL`.
    pub visibility: String,
    pub is_private: bool,
    /// `ADMIN` / `MAINTAIN` / `WRITE` / `TRIAGE` / `READ`. 알 수 없으면 빈 문자열.
    pub viewer_permission: String,
    pub open_issues: Option<u64>,
    pub open_prs: Option<u64>,
    pub has_issues: bool,
}

impl RepoInfo {
    /// 쓰기 권한(라벨·담당자 편집, 재실행 등)이 있는지. 권한을 모르면 `true`.
    pub fn can_write(&self) -> bool {
        !matches!(self.viewer_permission.as_str(), "READ" | "TRIAGE" | "NONE")
    }
}

pub const REPO_VIEW_FIELDS: &str = "nameWithOwner,description,url,defaultBranchRef,stargazerCount,visibility,isPrivate,viewerPermission,issues,pullRequests,hasIssuesEnabled";

pub fn parse_repo_view(json: &str) -> GitResult<RepoInfo> {
    let v: RepoViewJson = serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))?;
    let repo = RepoRef::parse(&v.name_with_owner).ok_or_else(|| GitError::Parse(kiln_common::trf!("저장소 이름: {}", v.name_with_owner)))?;
    Ok(RepoInfo {
        repo,
        description: v.description.unwrap_or_default(),
        url: v.url,
        default_branch: v.default_branch_ref.map(|b| b.name).unwrap_or_default(),
        stargazer_count: v.stargazer_count,
        visibility: v.visibility,
        is_private: v.is_private,
        viewer_permission: v.viewer_permission.unwrap_or_default(),
        open_issues: v.issues.map(|c| c.total_count),
        open_prs: v.pull_requests.map(|c| c.total_count),
        has_issues: v.has_issues_enabled.unwrap_or(true),
    })
}

/// 저장소 목록/검색 결과 한 건.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepoListItem {
    #[serde(alias = "fullName")]
    pub name_with_owner: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub description: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub is_private: bool,
    #[serde(default, alias = "stargazersCount")]
    pub stargazer_count: u64,
}

impl RepoListItem {
    pub fn repo(&self) -> Option<RepoRef> {
        RepoRef::parse(&self.name_with_owner)
    }
}

pub const REPO_LIST_FIELDS: &str = "nameWithOwner,description,updatedAt,isPrivate,stargazerCount";
pub const REPO_SEARCH_FIELDS: &str = "fullName,description,updatedAt,isPrivate,stargazersCount";

/// `gh repo list` / `gh search repos` JSON 을 해석한다.
pub fn parse_repo_list(json: &str) -> GitResult<Vec<RepoListItem>> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

/// 로그인 사용자와 소속 조직 이름.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    pub login: String,
    pub orgs: Vec<String>,
}

/// `gh api graphql` 의 `viewer { login organizations { nodes { login } } }` 응답을 해석한다.
pub fn parse_viewer(json: &str) -> GitResult<Viewer> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))?;
    let viewer = &v["data"]["viewer"];
    let login = viewer["login"].as_str().ok_or_else(|| GitError::Parse("viewer.login".into()))?.to_string();
    let orgs = viewer["organizations"]["nodes"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n["login"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(Viewer { login, orgs })
}

// ---------------------------------------------------------------- 이슈

fn de_opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// 배열이면 길이, 숫자면 그 값, `{totalCount}` 면 그 값을 센다.
fn de_count<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    use serde_json::Value;
    Ok(match Value::deserialize(d)? {
        Value::Array(a) => a.len(),
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        Value::Object(o) => o.get("totalCount").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        _ => 0,
    })
}

/// 이슈 상태.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueState {
    Open,
    /// 완료로 닫힘.
    Completed,
    /// 계획 없음(또는 중복)으로 닫힘.
    NotPlanned,
}

impl IssueState {
    pub fn parse(state: &str, reason: &str) -> Self {
        match (state, reason) {
            ("OPEN", _) => Self::Open,
            (_, "NOT_PLANNED") | (_, "DUPLICATE") => Self::NotPlanned,
            ("CLOSED", _) => Self::Completed,
            _ => Self::Open,
        }
    }

    pub fn is_open(self) -> bool {
        self == Self::Open
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Open => kiln_common::i18n::tr("열림"),
            Self::Completed => kiln_common::i18n::tr("완료됨"),
            Self::NotPlanned => kiln_common::i18n::tr("계획 없음"),
        }
    }
}

/// 이슈 담당자/작성자 등 사용자(로그인 이름만 쓴다).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct User {
    #[serde(default)]
    pub login: String,
}

/// `gh issue list --json ...` 한 건.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueItem {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub state: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub state_reason: String,
    #[serde(default)]
    pub labels: Vec<PrLabel>,
    /// 댓글 수.
    #[serde(default, deserialize_with = "de_count")]
    pub comments: usize,
    #[serde(default)]
    pub assignees: Vec<User>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub url: String,
}

impl IssueItem {
    pub fn issue_state(&self) -> IssueState {
        IssueState::parse(&self.state, &self.state_reason)
    }
}

/// `gh issue view --json ...` 결과.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub state: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub state_reason: String,
    #[serde(default)]
    pub labels: Vec<PrLabel>,
    #[serde(default)]
    pub assignees: Vec<User>,
    #[serde(default)]
    pub comments: Vec<PrComment>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub closed_at: String,
    #[serde(default)]
    pub url: String,
}

impl IssueDetail {
    pub fn issue_state(&self) -> IssueState {
        IssueState::parse(&self.state, &self.state_reason)
    }
}

pub const ISSUE_LIST_FIELDS: &str = "number,title,author,state,stateReason,labels,comments,assignees,createdAt,updatedAt,url";
pub const ISSUE_VIEW_FIELDS: &str =
    "number,title,body,author,state,stateReason,labels,assignees,comments,createdAt,updatedAt,closedAt,url";

pub fn parse_issue_list(json: &str) -> GitResult<Vec<IssueItem>> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

pub fn parse_issue_view(json: &str) -> GitResult<IssueDetail> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

/// 저장소 라벨(`gh label list --json name,color,description`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    #[serde(default)]
    pub color: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub description: String,
}

pub fn parse_labels(json: &str) -> GitResult<Vec<Label>> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

/// `repos/{o}/{r}/assignees` REST 응답에서 로그인 이름만 뽑는다.
pub fn parse_assignees(json: &str) -> GitResult<Vec<String>> {
    let v: Vec<User> = serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))?;
    Ok(v.into_iter().map(|u| u.login).filter(|l| !l.is_empty()).collect())
}

/// 이슈 목록 필터.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IssueFilter {
    #[default]
    Open,
    Mine,
    Assigned,
    Closed,
}

impl IssueFilter {
    pub const ALL: [IssueFilter; 4] = [IssueFilter::Open, IssueFilter::Mine, IssueFilter::Assigned, IssueFilter::Closed];

    pub fn label(self) -> &'static str {
        match self {
            IssueFilter::Open => kiln_common::i18n::tr("열림"),
            IssueFilter::Mine => kiln_common::i18n::tr("내 이슈"),
            IssueFilter::Assigned => kiln_common::i18n::tr("나에게 할당"),
            IssueFilter::Closed => kiln_common::i18n::tr("닫힘"),
        }
    }

    /// `gh issue list` 추가 인자.
    pub fn gh_args(self) -> Vec<&'static str> {
        match self {
            IssueFilter::Open => vec!["--state", "open"],
            IssueFilter::Mine => vec!["--state", "open", "--author", "@me"],
            IssueFilter::Assigned => vec!["--state", "open", "--assignee", "@me"],
            IssueFilter::Closed => vec!["--state", "closed"],
        }
    }
}

/// 이슈 생성 요청.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IssueCreate {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
}

/// 이슈 라벨/담당자 변경 요청.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssueEdit {
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub add_assignees: Vec<String>,
    pub remove_assignees: Vec<String>,
}

impl IssueEdit {
    pub fn is_empty(&self) -> bool {
        self.add_labels.is_empty() && self.remove_labels.is_empty() && self.add_assignees.is_empty() && self.remove_assignees.is_empty()
    }

    /// 현재 목록(`before`)과 원하는 목록(`after`)의 차이로 라벨 변경을 만든다.
    pub fn diff_labels(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
        let add = after.iter().filter(|a| !before.contains(a)).cloned().collect();
        let remove = before.iter().filter(|b| !after.contains(b)).cloned().collect();
        (add, remove)
    }
}

/// 이슈를 닫는 이유.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CloseReason {
    #[default]
    Completed,
    NotPlanned,
}

/// `gh issue create` 출력(URL)에서 이슈 번호를 뽑는다.
pub fn issue_number_from_url(out: &str) -> Option<u64> {
    out.lines().rev().find_map(|l| {
        let l = l.trim();
        let (_, n) = l.rsplit_once("/issues/")?;
        n.trim_end_matches('/').parse().ok()
    })
}

// ---------------------------------------------------------------- Actions

/// `gh run list --json ...` 한 건.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunItem {
    pub database_id: u64,
    #[serde(default)]
    pub display_title: String,
    #[serde(default)]
    pub workflow_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub conclusion: String,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub number: u64,
    #[serde(default)]
    pub attempt: u64,
}

/// 실행 상태 요약.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Success,
    Failure,
    InProgress,
    Queued,
    Cancelled,
    Skipped,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Success => kiln_common::i18n::tr("성공"),
            RunStatus::Failure => kiln_common::i18n::tr("실패"),
            RunStatus::InProgress => kiln_common::i18n::tr("진행 중"),
            RunStatus::Queued => kiln_common::i18n::tr("대기"),
            RunStatus::Cancelled => kiln_common::i18n::tr("취소됨"),
            RunStatus::Skipped => kiln_common::i18n::tr("건너뜀"),
        }
    }

    /// 아직 끝나지 않은 실행인지.
    pub fn is_active(self) -> bool {
        matches!(self, RunStatus::InProgress | RunStatus::Queued)
    }
}

impl RunItem {
    pub fn run_status(&self) -> RunStatus {
        match self.status.to_ascii_lowercase().as_str() {
            "in_progress" => RunStatus::InProgress,
            "queued" | "waiting" | "requested" | "pending" => RunStatus::Queued,
            _ => match self.conclusion.to_ascii_lowercase().as_str() {
                "success" => RunStatus::Success,
                "cancelled" => RunStatus::Cancelled,
                "skipped" | "neutral" | "stale" => RunStatus::Skipped,
                "" => RunStatus::Queued,
                _ => RunStatus::Failure,
            },
        }
    }

    /// 표시용 이벤트 이름.
    pub fn event_label(&self) -> &str {
        match self.event.as_str() {
            "push" => kiln_common::i18n::tr("푸시"),
            "pull_request" | "pull_request_target" => kiln_common::i18n::tr("풀 리퀘스트"),
            "schedule" => kiln_common::i18n::tr("예약"),
            "workflow_dispatch" => kiln_common::i18n::tr("수동 실행"),
            "issues" => kiln_common::i18n::tr("이슈"),
            "issue_comment" => kiln_common::i18n::tr("이슈 댓글"),
            "release" => kiln_common::i18n::tr("릴리스"),
            "dynamic" => kiln_common::i18n::tr("동적"),
            other => other,
        }
    }
}

pub const RUN_LIST_FIELDS: &str =
    "databaseId,displayTitle,workflowName,status,conclusion,headBranch,event,createdAt,updatedAt,url,number,attempt";

pub fn parse_run_list(json: &str) -> GitResult<Vec<RunItem>> {
    serde_json::from_str(json).map_err(|e| GitError::Parse(e.to_string()))
}

/// 실행 목록 필터. `None` 이면 제한 없음.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFilter {
    pub branch: Option<String>,
    pub workflow: Option<String>,
}

impl RunFilter {
    /// `gh run list` 추가 인자.
    pub fn gh_args(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(b) = &self.branch {
            v.push("--branch".into());
            v.push(b.clone());
        }
        if let Some(w) = &self.workflow {
            v.push("--workflow".into());
            v.push(w.clone());
        }
        v
    }
}

/// `gh` 명령에 붙일 `-R owner/name` 인자.
pub fn repo_args(repo: Option<&RepoRef>) -> Vec<String> {
    repo.map(|r| vec!["-R".to_string(), r.full_name()]).unwrap_or_default()
}

/// 터미널에서 실행할 `gh` 명령 문자열. `repo` 가 있으면 `-R` 을 붙인다.
pub fn gh_command(args: &str, repo: Option<&RepoRef>) -> String {
    match repo {
        Some(r) => format!("gh {args} -R {}", r.full_name()),
        None => format!("gh {args}"),
    }
}

// ---------------------------------------------------------------- 백엔드

/// GitHub 허브 데이터 소스. `repo` 가 `None` 이면 작업 폴더의 저장소를 쓴다.
/// 기본 구현은 `gh` CLI(`GhBackend`)이고, 테스트에서는 가짜 구현을 주입한다.
pub trait GithubBackend: Send + Sync + 'static {
    /// `repo` 의 정보. `None` 이면 작업 폴더에서 저장소를 찾는다.
    fn repo_info(&self, repo: Option<&RepoRef>) -> GitResult<RepoInfo>;
    fn viewer(&self) -> GitResult<Viewer>;
    /// `owner` 가 `None` 이면 로그인 사용자의 저장소.
    fn list_repos(&self, owner: Option<&str>) -> GitResult<Vec<RepoListItem>>;
    fn search_repos(&self, query: &str) -> GitResult<Vec<RepoListItem>>;
    /// `repo` 를 대상으로 하는 PR 백엔드.
    fn pr_backend(&self, repo: Option<&RepoRef>) -> Arc<dyn PrBackend>;

    /// `search` 가 비어 있지 않으면 GitHub 검색 구문으로 넘긴다.
    fn issues(&self, repo: Option<&RepoRef>, filter: IssueFilter, search: &str) -> GitResult<Vec<IssueItem>>;
    fn issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<IssueDetail>;
    /// 만든 이슈 번호를 돌려준다.
    fn create_issue(&self, repo: Option<&RepoRef>, req: &IssueCreate) -> GitResult<u64>;
    fn comment_issue(&self, repo: Option<&RepoRef>, number: u64, body: &str) -> GitResult<()>;
    fn close_issue(&self, repo: Option<&RepoRef>, number: u64, reason: CloseReason) -> GitResult<()>;
    fn reopen_issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<()>;
    fn edit_issue(&self, repo: Option<&RepoRef>, number: u64, edit: &IssueEdit) -> GitResult<()>;
    fn labels(&self, repo: Option<&RepoRef>) -> GitResult<Vec<Label>>;
    fn assignable_users(&self, repo: Option<&RepoRef>) -> GitResult<Vec<String>>;

    fn runs(&self, repo: Option<&RepoRef>, filter: &RunFilter) -> GitResult<Vec<RunItem>>;
    /// `failed_only` 면 실패한 작업만 다시 실행한다.
    fn rerun(&self, repo: Option<&RepoRef>, run_id: u64, failed_only: bool) -> GitResult<()>;
    fn cancel_run(&self, repo: Option<&RepoRef>, run_id: u64) -> GitResult<()>;
}

impl GhBackend {
    fn root_path(&self) -> &std::path::Path {
        &self.root
    }

    fn run_repo(&self, repo: Option<&RepoRef>, args: &[&str]) -> GitResult<String> {
        let extra = repo_args(repo);
        let mut all: Vec<&str> = args.to_vec();
        all.extend(extra.iter().map(String::as_str));
        gh(self.root_path(), &all)
    }

    fn run_repo_stdin(&self, repo: Option<&RepoRef>, args: &[&str], input: &str) -> GitResult<String> {
        let extra = repo_args(repo);
        let mut all: Vec<&str> = args.to_vec();
        all.extend(extra.iter().map(String::as_str));
        gh_stdin(self.root_path(), &all, Some(input.as_bytes()))
    }
}

impl GithubBackend for GhBackend {
    fn repo_info(&self, repo: Option<&RepoRef>) -> GitResult<RepoInfo> {
        let name = repo.map(RepoRef::full_name);
        let mut args = vec!["repo", "view"];
        if let Some(n) = &name {
            args.push(n);
        }
        args.extend(["--json", REPO_VIEW_FIELDS]);
        parse_repo_view(&gh(self.root_path(), &args)?)
    }

    fn viewer(&self) -> GitResult<Viewer> {
        let q = "query{viewer{login organizations(first:100){nodes{login}}}}";
        parse_viewer(&gh(self.root_path(), &["api", "graphql", "-f", &format!("query={q}")])?)
    }

    fn list_repos(&self, owner: Option<&str>) -> GitResult<Vec<RepoListItem>> {
        let mut args = vec!["repo", "list"];
        if let Some(o) = owner {
            args.push(o);
        }
        args.extend(["--limit", "100", "--json", REPO_LIST_FIELDS]);
        parse_repo_list(&gh(self.root_path(), &args)?)
    }

    fn search_repos(&self, query: &str) -> GitResult<Vec<RepoListItem>> {
        let mut args = vec!["search", "repos"];
        args.extend(query.split_whitespace());
        args.extend(["--limit", "30", "--json", REPO_SEARCH_FIELDS]);
        parse_repo_list(&gh(self.root_path(), &args)?)
    }

    fn pr_backend(&self, repo: Option<&RepoRef>) -> Arc<dyn PrBackend> {
        Arc::new(GhBackend::for_repo(self.root.clone(), repo.cloned()))
    }

    fn issues(&self, repo: Option<&RepoRef>, filter: IssueFilter, search: &str) -> GitResult<Vec<IssueItem>> {
        let mut args = vec!["issue", "list", "--limit", "60", "--json", ISSUE_LIST_FIELDS];
        args.extend(filter.gh_args());
        let search = search.trim();
        if !search.is_empty() {
            args.extend(["--search", search]);
        }
        parse_issue_list(&self.run_repo(repo, &args)?)
    }

    fn issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<IssueDetail> {
        let n = number.to_string();
        parse_issue_view(&self.run_repo(repo, &["issue", "view", &n, "--json", ISSUE_VIEW_FIELDS])?)
    }

    fn create_issue(&self, repo: Option<&RepoRef>, req: &IssueCreate) -> GitResult<u64> {
        let mut args = vec!["issue", "create", "--title", req.title.as_str(), "--body-file", "-"];
        for l in &req.labels {
            args.extend(["--label", l.as_str()]);
        }
        for a in &req.assignees {
            args.extend(["--assignee", a.as_str()]);
        }
        let out = self.run_repo_stdin(repo, &args, &req.body)?;
        issue_number_from_url(&out).ok_or_else(|| GitError::Parse(kiln_common::trf!("이슈 URL을 찾을 수 없습니다: {}", out.trim())))
    }

    fn comment_issue(&self, repo: Option<&RepoRef>, number: u64, body: &str) -> GitResult<()> {
        let n = number.to_string();
        self.run_repo_stdin(repo, &["issue", "comment", &n, "--body-file", "-"], body).map(|_| ())
    }

    fn close_issue(&self, repo: Option<&RepoRef>, number: u64, reason: CloseReason) -> GitResult<()> {
        let n = number.to_string();
        let r = match reason {
            CloseReason::Completed => "completed",
            CloseReason::NotPlanned => "not planned",
        };
        self.run_repo(repo, &["issue", "close", &n, "--reason", r]).map(|_| ())
    }

    fn reopen_issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<()> {
        let n = number.to_string();
        self.run_repo(repo, &["issue", "reopen", &n]).map(|_| ())
    }

    fn edit_issue(&self, repo: Option<&RepoRef>, number: u64, edit: &IssueEdit) -> GitResult<()> {
        if edit.is_empty() {
            return Ok(());
        }
        let n = number.to_string();
        let mut args = vec!["issue", "edit", n.as_str()];
        for (flag, list) in [
            ("--add-label", &edit.add_labels),
            ("--remove-label", &edit.remove_labels),
            ("--add-assignee", &edit.add_assignees),
            ("--remove-assignee", &edit.remove_assignees),
        ] {
            for v in list {
                args.extend([flag, v.as_str()]);
            }
        }
        self.run_repo(repo, &args).map(|_| ())
    }

    fn labels(&self, repo: Option<&RepoRef>) -> GitResult<Vec<Label>> {
        parse_labels(&self.run_repo(repo, &["label", "list", "--limit", "200", "--json", "name,color,description"])?)
    }

    fn assignable_users(&self, repo: Option<&RepoRef>) -> GitResult<Vec<String>> {
        let path = match repo {
            Some(r) => format!("repos/{}/{}/assignees?per_page=100", r.owner, r.name),
            None => "repos/{owner}/{repo}/assignees?per_page=100".to_string(),
        };
        parse_assignees(&gh(self.root_path(), &["api", &path])?)
    }

    fn runs(&self, repo: Option<&RepoRef>, filter: &RunFilter) -> GitResult<Vec<RunItem>> {
        let extra = filter.gh_args();
        let mut args = vec!["run", "list", "--limit", "50", "--json", RUN_LIST_FIELDS];
        args.extend(extra.iter().map(String::as_str));
        parse_run_list(&self.run_repo(repo, &args)?)
    }

    fn rerun(&self, repo: Option<&RepoRef>, run_id: u64, failed_only: bool) -> GitResult<()> {
        let id = run_id.to_string();
        let mut args = vec!["run", "rerun", id.as_str()];
        if failed_only {
            args.push("--failed");
        }
        self.run_repo(repo, &args).map(|_| ())
    }

    fn cancel_run(&self, repo: Option<&RepoRef>, run_id: u64) -> GitResult<()> {
        let id = run_id.to_string();
        self.run_repo(repo, &["run", "cancel", &id]).map(|_| ())
    }
}

/// 작업 폴더 기준 `gh` 백엔드를 만든다.
pub fn gh_backend(root: PathBuf) -> Arc<dyn GithubBackend> {
    Arc::new(GhBackend::new(root))
}

/// `gh repo clone <name_with_owner> <target>` 를 실행한다(블로킹).
pub fn clone_repo(name_with_owner: &str, target: &Path) -> GitResult<()> {
    let parent = target.parent().unwrap_or(target);
    gh(parent, &["repo", "clone", name_with_owner, &target.to_string_lossy()]).map(|_| ())
}
