//! 고정 픽스처(cli/cli)를 돌려주고 호출을 기록하는 GitHub 허브용 가짜 백엔드.

use std::sync::{Arc, Mutex};

use kiln_git::gh::{parse_pr_list, parse_pr_view};
use kiln_git::github::{
    parse_assignees, parse_issue_list, parse_issue_view, parse_labels, parse_repo_list, parse_repo_view, parse_run_list,
};
use kiln_git::{
    CloseReason, GitError, GitResult, GithubBackend, IssueCreate, IssueDetail, IssueEdit, IssueFilter, IssueItem, Label,
    MergeMethod, PrBackend, PrCreate, PrCreateDefaults, PrDetail, PrFilter, PrItem, RepoInfo, RepoListItem, RepoRef,
    ReviewKind, RunFilter, RunItem, Viewer,
};

pub const ISSUE_LIST: &str = include_str!("../fixtures/issue_list.json");
pub const ISSUE_LIST_CLOSED: &str = include_str!("../fixtures/issue_list_closed.json");
pub const ISSUE_VIEW: &str = include_str!("../fixtures/issue_view.json");
pub const LABELS: &str = include_str!("../fixtures/label_list.json");
pub const ASSIGNEES: &str = include_str!("../fixtures/assignees.json");
pub const RUNS: &str = include_str!("../fixtures/run_list.json");
pub const RUNS_FAILED: &str = include_str!("../fixtures/run_list_failed.json");
pub const REPO_VIEW: &str = include_str!("../fixtures/repo_view.json");
pub const REPO_LIST: &str = include_str!("../fixtures/repo_list.json");
pub const SEARCH_REPOS: &str = include_str!("../fixtures/search_repos.json");

/// 저장소 감지 결과 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detect {
    Ok,
    Auth,
    Missing,
    NoRemote,
}

pub struct FakeGh {
    calls: Arc<Mutex<Vec<String>>>,
    pub detect: Detect,
    issue_state: Mutex<(String, String)>,
}

impl Default for FakeGh {
    fn default() -> Self {
        Self::new(Detect::Ok)
    }
}

impl FakeGh {
    pub fn new(detect: Detect) -> Self {
        Self { calls: Arc::default(), detect, issue_state: Mutex::new(("OPEN".into(), String::new())) }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn has_call(&self, c: &str) -> bool {
        self.calls().iter().any(|x| x == c)
    }

    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

fn rn(repo: Option<&RepoRef>) -> String {
    repo.map(|r| r.full_name()).unwrap_or_else(|| "-".into())
}

/// 담당자와 댓글 수를 섞은 열린 이슈 목록.
pub fn open_issues() -> Vec<IssueItem> {
    let mut v = parse_issue_list(ISSUE_LIST).unwrap();
    v[0].assignees = vec![kiln_git::github::User { login: "williammartin".into() }];
    v[2].assignees = vec![kiln_git::github::User { login: "babakks".into() }, kiln_git::github::User { login: "BagToad".into() }];
    v[4].assignees = vec![kiln_git::github::User { login: "andyfeller".into() }];
    v
}

/// 성공/실패/진행 중/대기/취소가 섞인 실행 목록.
pub fn runs() -> Vec<RunItem> {
    let mut v = parse_run_list(RUNS).unwrap();
    let failed = parse_run_list(RUNS_FAILED).unwrap();
    v[0].status = "in_progress".into();
    v[0].conclusion = String::new();
    v[0].workflow_name = "Tests".into();
    v[0].display_title = "Tighten auth status error handling".into();
    v[0].head_branch = "fix/auth-status-rate-limit-network".into();
    v[0].event = "pull_request".into();
    v[1].status = "queued".into();
    v[1].conclusion = String::new();
    v[1].workflow_name = "Lint".into();
    v[3].conclusion = "cancelled".into();
    let mut out = vec![v[0].clone(), v[1].clone()];
    out.extend(failed.into_iter().take(2));
    out.extend(v.into_iter().skip(2).take(8));
    out
}

impl GithubBackend for FakeGh {
    fn repo_info(&self, repo: Option<&RepoRef>) -> GitResult<RepoInfo> {
        self.log(format!("repo_info {}", rn(repo)));
        match (repo, self.detect) {
            (Some(r), _) if r.full_name() != "cli/cli" => Ok(RepoInfo {
                repo: r.clone(),
                description: "Go module for interacting with gh and the GitHub API".into(),
                url: format!("https://github.com/{r}"),
                default_branch: "trunk".into(),
                stargazer_count: 439,
                visibility: "PUBLIC".into(),
                open_issues: Some(12),
                open_prs: Some(3),
                has_issues: true,
                ..Default::default()
            }),
            (Some(_), _) | (None, Detect::Ok) => parse_repo_view(REPO_VIEW),
            (None, Detect::Auth) => Err(GitError::GhAuth(String::new())),
            (None, Detect::Missing) => Err(GitError::GhMissing),
            (None, Detect::NoRemote) => Err(GitError::Failed("no git remotes found".into())),
        }
    }

    fn viewer(&self) -> GitResult<Viewer> {
        self.log("viewer".into());
        Ok(Viewer { login: "octocat".into(), orgs: vec!["cli".into(), "kiln-dev".into()] })
    }

    fn list_repos(&self, owner: Option<&str>) -> GitResult<Vec<RepoListItem>> {
        self.log(format!("list_repos {}", owner.unwrap_or("-")));
        match owner {
            Some("cli") => parse_repo_list(REPO_LIST),
            _ => Ok(vec![
                RepoListItem {
                    name_with_owner: "octocat/kiln".into(),
                    description: "A fast, modern terminal workspace".into(),
                    updated_at: "2026-09-25T10:00:00Z".into(),
                    is_private: true,
                    stargazer_count: 12,
                },
                RepoListItem {
                    name_with_owner: "octocat/dotfiles".into(),
                    description: "My shell and editor configuration".into(),
                    updated_at: "2026-09-20T08:00:00Z".into(),
                    is_private: false,
                    stargazer_count: 3,
                },
                RepoListItem {
                    name_with_owner: "octocat/hello-world".into(),
                    description: String::new(),
                    updated_at: "2026-07-01T08:00:00Z".into(),
                    is_private: false,
                    stargazer_count: 1,
                },
            ]),
        }
    }

    fn search_repos(&self, query: &str) -> GitResult<Vec<RepoListItem>> {
        self.log(format!("search_repos {query}"));
        parse_repo_list(SEARCH_REPOS)
    }

    fn pr_backend(&self, repo: Option<&RepoRef>) -> Arc<dyn PrBackend> {
        self.log(format!("pr_backend {}", rn(repo)));
        Arc::new(FakePr { calls: self.calls.clone(), repo: rn(repo) })
    }

    fn issues(&self, repo: Option<&RepoRef>, filter: IssueFilter, search: &str) -> GitResult<Vec<IssueItem>> {
        self.log(format!("issues {} {filter:?} {search}", rn(repo)));
        let open = open_issues();
        Ok(match filter {
            IssueFilter::Open => open,
            IssueFilter::Mine => open.into_iter().filter(|i| i.author.login == "williammartin").collect(),
            IssueFilter::Assigned => open.into_iter().filter(|i| !i.assignees.is_empty()).collect(),
            IssueFilter::Closed => parse_issue_list(ISSUE_LIST_CLOSED).unwrap(),
        })
    }

    fn issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<IssueDetail> {
        self.log(format!("issue {} {number}", rn(repo)));
        let mut d = parse_issue_view(ISSUE_VIEW).unwrap();
        let (state, reason) = self.issue_state.lock().unwrap().clone();
        d.state = state;
        d.state_reason = reason;
        d.assignees = vec![kiln_git::github::User { login: "williammartin".into() }];
        d.labels.push(kiln_git::gh::PrLabel { name: "gh-run".into(), color: "0e8a16".into() });
        d.comments.truncate(3);
        Ok(d)
    }

    fn create_issue(&self, repo: Option<&RepoRef>, req: &IssueCreate) -> GitResult<u64> {
        self.log(format!(
            "create_issue {} {} | {} | labels={} | assignees={}",
            rn(repo),
            req.title,
            req.body,
            req.labels.join(","),
            req.assignees.join(",")
        ));
        Ok(14600)
    }

    fn comment_issue(&self, repo: Option<&RepoRef>, number: u64, body: &str) -> GitResult<()> {
        self.log(format!("comment {} {number} {body}", rn(repo)));
        Ok(())
    }

    fn close_issue(&self, repo: Option<&RepoRef>, number: u64, reason: CloseReason) -> GitResult<()> {
        self.log(format!("close {} {number} {reason:?}", rn(repo)));
        let r = match reason {
            CloseReason::Completed => "COMPLETED",
            CloseReason::NotPlanned => "NOT_PLANNED",
        };
        *self.issue_state.lock().unwrap() = ("CLOSED".into(), r.into());
        Ok(())
    }

    fn reopen_issue(&self, repo: Option<&RepoRef>, number: u64) -> GitResult<()> {
        self.log(format!("reopen {} {number}", rn(repo)));
        *self.issue_state.lock().unwrap() = ("OPEN".into(), "REOPENED".into());
        Ok(())
    }

    fn edit_issue(&self, repo: Option<&RepoRef>, number: u64, edit: &IssueEdit) -> GitResult<()> {
        self.log(format!(
            "edit {} {number} +l[{}] -l[{}] +a[{}] -a[{}]",
            rn(repo),
            edit.add_labels.join(","),
            edit.remove_labels.join(","),
            edit.add_assignees.join(","),
            edit.remove_assignees.join(",")
        ));
        Ok(())
    }

    fn labels(&self, repo: Option<&RepoRef>) -> GitResult<Vec<Label>> {
        self.log(format!("labels {}", rn(repo)));
        parse_labels(LABELS)
    }

    fn assignable_users(&self, repo: Option<&RepoRef>) -> GitResult<Vec<String>> {
        self.log(format!("assignees {}", rn(repo)));
        parse_assignees(ASSIGNEES)
    }

    fn runs(&self, repo: Option<&RepoRef>, filter: &RunFilter) -> GitResult<Vec<RunItem>> {
        self.log(format!("runs {} {:?} {:?}", rn(repo), filter.branch, filter.workflow));
        Ok(runs()
            .into_iter()
            .filter(|r| filter.branch.as_ref().is_none_or(|b| &r.head_branch == b))
            .filter(|r| filter.workflow.as_ref().is_none_or(|w| &r.workflow_name == w))
            .collect())
    }

    fn rerun(&self, repo: Option<&RepoRef>, run_id: u64, failed_only: bool) -> GitResult<()> {
        self.log(format!("rerun {} {run_id} failed_only={failed_only}", rn(repo)));
        Ok(())
    }

    fn cancel_run(&self, repo: Option<&RepoRef>, run_id: u64) -> GitResult<()> {
        self.log(format!("cancel {} {run_id}", rn(repo)));
        Ok(())
    }
}

/// 허브의 PR 패널에 쓰는 가짜 PR 백엔드.
pub struct FakePr {
    calls: Arc<Mutex<Vec<String>>>,
    repo: String,
}

impl PrBackend for FakePr {
    fn list(&self, filter: PrFilter) -> GitResult<Vec<PrItem>> {
        self.calls.lock().unwrap().push(format!("pr_list {} {filter:?}", self.repo));
        let mut v = parse_pr_list(include_str!("../fixtures/pr_list.json")).unwrap();
        for p in v.iter_mut().take(4) {
            p.state = "OPEN".into();
        }
        Ok(v.into_iter().filter(|p| p.state == "OPEN").collect())
    }
    fn view(&self, _number: u64) -> GitResult<PrDetail> {
        parse_pr_view(include_str!("../fixtures/pr_view.json"))
    }
    fn diff(&self, _number: u64) -> GitResult<String> {
        Ok(String::new())
    }
    fn create_defaults(&self) -> GitResult<PrCreateDefaults> {
        Ok(PrCreateDefaults::default())
    }
    fn create(&self, _req: &PrCreate) -> GitResult<String> {
        Ok(String::new())
    }
    fn review(&self, _number: u64, _kind: ReviewKind, _body: &str) -> GitResult<()> {
        Ok(())
    }
    fn merge(&self, _number: u64, _method: MergeMethod, _delete_branch: bool) -> GitResult<String> {
        Ok(String::new())
    }
    fn checkout(&self, _number: u64) -> GitResult<String> {
        Ok(String::new())
    }
    fn mark_ready(&self, _number: u64) -> GitResult<()> {
        Ok(())
    }
}
