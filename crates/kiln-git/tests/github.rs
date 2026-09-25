//! GitHub 허브 JSON 파싱 테스트(cli/cli 저장소에서 읽기 전용 gh 호출로 받은 픽스처 사용).

mod common;

use common::fake_gh::*;
use kiln_git::github::{
    IssueEdit, gh_command, issue_number_from_url, parse_assignees, parse_issue_list, parse_issue_view, parse_labels,
    parse_repo_list, parse_repo_view, parse_run_list, parse_viewer, repo_args,
};
use kiln_git::{IssueFilter, IssueState, RepoRef, RunFilter, RunStatus};

#[test]
fn repo_view_fixture_parses_counts_and_permission() {
    let r = parse_repo_view(REPO_VIEW).unwrap();
    assert_eq!(r.repo, RepoRef::new("cli", "cli"));
    assert_eq!(r.default_branch, "trunk");
    assert_eq!(r.stargazer_count, 46406);
    assert_eq!(r.visibility, "PUBLIC");
    assert!(!r.is_private);
    assert_eq!(r.open_issues, Some(1027));
    assert_eq!(r.open_prs, Some(62));
    assert_eq!(r.viewer_permission, "READ");
    assert!(!r.can_write());
    assert!(r.has_issues);
}

#[test]
fn repo_list_and_search_share_one_model() {
    let list = parse_repo_list(REPO_LIST).unwrap();
    assert_eq!(list.len(), 8);
    assert_eq!(list[0].name_with_owner, "cli/cli");
    assert_eq!(list[0].repo(), Some(RepoRef::new("cli", "cli")));
    assert!(list.iter().all(|r| r.name_with_owner.starts_with("cli/")));

    let found = parse_repo_list(SEARCH_REPOS).unwrap();
    assert_eq!(found.len(), 6);
    assert_eq!(found[0].name_with_owner, "alacritty/alacritty");
    assert!(found[0].stargazer_count > 60_000);
    assert!(!found[0].updated_at.is_empty());
}

#[test]
fn issue_list_fixture_counts_comments_and_labels() {
    let v = parse_issue_list(ISSUE_LIST).unwrap();
    assert_eq!(v.len(), 10);
    assert_eq!(v[0].number, 14512);
    assert_eq!(v[0].comments, 2);
    assert_eq!(v[0].author.login, "williammartin");
    assert_eq!(v[0].labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["enhancement", "gh-pr"]);
    assert!(v.iter().all(|i| i.issue_state() == IssueState::Open));
    assert_eq!(v.iter().find(|i| i.number == 14420).unwrap().comments, 8);

    let closed = parse_issue_list(ISSUE_LIST_CLOSED).unwrap();
    let states: Vec<IssueState> = closed.iter().map(|i| i.issue_state()).collect();
    assert_eq!(states[0], IssueState::NotPlanned);
    assert_eq!(states[1], IssueState::Completed);
}

#[test]
fn issue_view_fixture_parses_comments_timeline() {
    let d = parse_issue_view(ISSUE_VIEW).unwrap();
    assert_eq!(d.number, 14420);
    assert_eq!(d.issue_state(), IssueState::Open);
    assert_eq!(d.comments.len(), 8);
    assert!(d.comments.iter().all(|c| !c.author.login.is_empty() && !c.created_at.is_empty()));
    assert_eq!(d.labels[0].name, "more-info-needed");
    assert_eq!(d.labels[0].color, "830eb5");
    assert!(d.url.ends_with("/issues/14420"));
    assert!(d.closed_at.is_empty());
}

#[test]
fn labels_and_assignees_fixtures_parse() {
    let l = parse_labels(LABELS).unwrap();
    assert_eq!(l.len(), 30);
    assert!(l.iter().all(|x| !x.name.is_empty() && x.color.len() == 6));
    let a = parse_assignees(ASSIGNEES).unwrap();
    assert_eq!(a.len(), 21);
    assert!(a.iter().all(|x| !x.is_empty()));
}

#[test]
fn run_list_fixtures_map_status_and_conclusion() {
    let ok = parse_run_list(RUNS).unwrap();
    assert_eq!(ok.len(), 15);
    assert!(ok.iter().all(|r| r.run_status() == RunStatus::Success));
    assert!(ok[0].database_id > 0 && ok[0].url.contains("/actions/runs/"));
    let failed = parse_run_list(RUNS_FAILED).unwrap();
    assert!(failed.iter().all(|r| r.run_status() == RunStatus::Failure));
    assert_eq!(failed[0].event_label(), "풀 리퀘스트");

    let mut r = ok[0].clone();
    for (status, conclusion, want) in [
        ("in_progress", "", RunStatus::InProgress),
        ("queued", "", RunStatus::Queued),
        ("waiting", "", RunStatus::Queued),
        ("completed", "cancelled", RunStatus::Cancelled),
        ("completed", "skipped", RunStatus::Skipped),
        ("completed", "timed_out", RunStatus::Failure),
        ("completed", "startup_failure", RunStatus::Failure),
        ("completed", "success", RunStatus::Success),
    ] {
        r.status = status.into();
        r.conclusion = conclusion.into();
        assert_eq!(r.run_status(), want, "{status}/{conclusion}");
    }
    assert!(RunStatus::InProgress.is_active() && RunStatus::Queued.is_active() && !RunStatus::Failure.is_active());
}

#[test]
fn repo_ref_parses_names_and_urls() {
    assert_eq!(RepoRef::parse("cli/cli"), Some(RepoRef::new("cli", "cli")));
    assert_eq!(RepoRef::parse("https://github.com/cli/go-gh.git"), Some(RepoRef::new("cli", "go-gh")));
    assert_eq!(RepoRef::parse("https://github.com/cli/cli/issues/1"), Some(RepoRef::new("cli", "cli")));
    assert_eq!(RepoRef::parse("nope"), None);
    assert_eq!(RepoRef::parse("a b/c"), None);
    assert_eq!(RepoRef::new("o", "n").to_string(), "o/n");
}

#[test]
fn command_helpers_add_repo_flag() {
    let r = RepoRef::new("cli", "cli");
    assert_eq!(repo_args(Some(&r)), vec!["-R", "cli/cli"]);
    assert!(repo_args(None).is_empty());
    assert_eq!(gh_command("run view 5 --log", Some(&r)), "gh run view 5 --log -R cli/cli");
    assert_eq!(gh_command("run watch 5", None), "gh run watch 5");
    assert_eq!(issue_number_from_url("Creating issue\nhttps://github.com/cli/cli/issues/14600\n"), Some(14600));
    assert_eq!(issue_number_from_url("oops"), None);
}

#[test]
fn filters_map_to_gh_arguments() {
    assert_eq!(IssueFilter::Open.gh_args(), vec!["--state", "open"]);
    assert!(IssueFilter::Mine.gh_args().windows(2).any(|w| w == ["--author", "@me"]));
    assert!(IssueFilter::Assigned.gh_args().windows(2).any(|w| w == ["--assignee", "@me"]));
    assert_eq!(IssueFilter::Closed.gh_args(), vec!["--state", "closed"]);
    let f = RunFilter { branch: Some("trunk".into()), workflow: Some("Lint".into()) };
    assert_eq!(f.gh_args(), vec!["--branch", "trunk", "--workflow", "Lint"]);
    assert!(RunFilter::default().gh_args().is_empty());
}

#[test]
fn issue_edit_diff_computes_additions_and_removals() {
    let before = vec!["bug".to_string(), "gh-pr".to_string()];
    let after = vec!["gh-pr".to_string(), "enhancement".to_string()];
    let (add, remove) = IssueEdit::diff_labels(&before, &after);
    assert_eq!(add, vec!["enhancement"]);
    assert_eq!(remove, vec!["bug"]);
    assert!(IssueEdit::default().is_empty());
}

#[test]
fn viewer_graphql_response_parses_orgs() {
    let v = parse_viewer(r#"{"data":{"viewer":{"login":"octocat","organizations":{"nodes":[{"login":"cli"},{"login":"github"}]}}}}"#).unwrap();
    assert_eq!(v.login, "octocat");
    assert_eq!(v.orgs, vec!["cli", "github"]);
    assert!(parse_viewer("{}").is_err());
}
