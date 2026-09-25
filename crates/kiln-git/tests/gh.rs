//! gh JSON 파싱 테스트(cli/cli 저장소에서 받은 고정 픽스처 사용).

use kiln_git::diff::parse_diff;
use kiln_git::gh::{
    CheckItem, ChecksState, PrFilter, PrState, ReviewDecision, parse_pr_brief, parse_pr_list, parse_pr_view,
    prefill_from_commits, rollup,
};
use kiln_git::util::{parse_iso8601, relative_time, short_relative_time};

const PR_LIST: &str = include_str!("fixtures/pr_list.json");
const PR_VIEW: &str = include_str!("fixtures/pr_view.json");
const PR_VIEW_OPEN: &str = include_str!("fixtures/pr_view_open.json");
const PR_DIFF: &str = include_str!("fixtures/pr_diff.patch");

#[test]
fn pr_list_fixture_parses_all_fields() {
    let list = parse_pr_list(PR_LIST).unwrap();
    assert_eq!(list.len(), 8);
    let first = &list[0];
    assert_eq!(first.number, 14509);
    assert_eq!(first.author.login, "williammartin");
    assert_eq!(first.pr_state(), PrState::Merged);
    assert_eq!(first.review(), Some(ReviewDecision::Approved));
    assert_eq!(first.checks(), Some(ChecksState::Pass));
    assert!(first.url.starts_with("https://github.com/cli/cli/pull/"));
    assert!(!first.updated_at.is_empty());
    let open = list.iter().find(|p| p.number == 14485).unwrap();
    assert_eq!(open.pr_state(), PrState::Open);
    assert_eq!((open.additions, open.deletions), (13, 15));
    assert_eq!(open.review(), Some(ReviewDecision::ReviewRequired));
    assert_eq!(list.iter().filter(|p| p.pr_state() == PrState::Closed).count(), 2);
}

#[test]
fn pr_view_fixture_parses_reviews_files_and_commits() {
    let d = parse_pr_view(PR_VIEW).unwrap();
    assert_eq!(d.number, 14507);
    assert_eq!(d.pr_state(), PrState::Merged);
    assert_eq!(d.changed_files, 26);
    assert_eq!(d.files.len(), 26);
    assert_eq!((d.additions, d.deletions), (588, 186));
    assert_eq!(d.commits.len(), 6);
    assert_eq!(d.reviews.len(), 8);
    assert_eq!(d.reviews[0].state, "COMMENTED");
    assert!(d.reviews[0].body.contains("Copilot review overview"));
    assert_eq!(d.checks(), Some(ChecksState::Pass));
    let files_add: u64 = d.files.iter().map(|f| f.additions).sum();
    assert_eq!(files_add, d.additions);
    assert!(d.status_check_rollup.iter().all(|c| c.typename == "CheckRun"));
    let lint = d.status_check_rollup.iter().find(|c| c.display_name() == "lint").unwrap();
    assert_eq!(lint.workflow_name.as_deref(), Some("Lint"));
    assert!(lint.url().unwrap().starts_with("https://github.com/cli/cli/actions/"));

    let o = parse_pr_view(PR_VIEW_OPEN).unwrap();
    assert_eq!(o.pr_state(), PrState::Open);
    assert_eq!(o.comments.len(), 1);
    assert_eq!(o.comments[0].author.login, "cli-triage");
}

#[test]
fn pr_diff_fixture_parses_every_changed_file() {
    let d = parse_pr_view(PR_VIEW).unwrap();
    let files = parse_diff(PR_DIFF);
    assert_eq!(files.len() as u64, d.changed_files);
    let added: usize = files.iter().map(|f| f.added()).sum();
    let removed: usize = files.iter().map(|f| f.removed()).sum();
    assert_eq!((added as u64, removed as u64), (d.additions, d.deletions));
    for f in &files {
        let meta = d.files.iter().find(|m| m.path == f.path()).unwrap_or_else(|| panic!("{} not in files", f.path()));
        assert_eq!((f.added() as u64, f.removed() as u64), (meta.additions, meta.deletions), "{}", f.path());
    }
}

fn check(json: &str) -> CheckItem {
    serde_json::from_str(json).unwrap()
}

#[test]
fn checks_rollup_prioritizes_failure_then_pending() {
    let pass = check(r#"{"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS"}"#);
    let skipped = check(r#"{"__typename":"CheckRun","name":"opt","status":"COMPLETED","conclusion":"SKIPPED"}"#);
    let running = check(r#"{"__typename":"CheckRun","name":"test","status":"IN_PROGRESS","conclusion":""}"#);
    let failed = check(r#"{"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"FAILURE"}"#);
    let ctx_pending = check(r#"{"__typename":"StatusContext","context":"ci/legacy","state":"PENDING","targetUrl":"https://ci"}"#);
    let ctx_error = check(r#"{"__typename":"StatusContext","context":"ci/legacy","state":"ERROR"}"#);

    assert_eq!(rollup(&[]), None);
    assert_eq!(rollup(&[pass.clone(), skipped.clone()]), Some(ChecksState::Pass));
    assert_eq!(rollup(&[pass.clone(), running.clone()]), Some(ChecksState::Pending));
    assert_eq!(rollup(&[pass.clone(), ctx_pending.clone()]), Some(ChecksState::Pending));
    assert_eq!(rollup(&[running, failed.clone(), pass.clone()]), Some(ChecksState::Fail));
    assert_eq!(rollup(&[pass, ctx_error]), Some(ChecksState::Fail));
    assert_eq!(ctx_pending.display_name(), "ci/legacy");
    assert_eq!(ctx_pending.url(), Some("https://ci"));
    assert_eq!(failed.outcome_label(), "실패");
    assert_eq!(skipped.outcome(), None);
}

#[test]
fn pr_brief_parses_view_json() {
    let b = parse_pr_brief(PR_VIEW_OPEN).unwrap();
    assert_eq!(b.number, 14485);
    assert_eq!(b.state, PrState::Open);
    assert!(!b.is_draft);
    assert_eq!(b.checks, Some(ChecksState::Pass));
    assert_eq!(b.review, Some(ReviewDecision::ReviewRequired));
}

#[test]
fn filters_map_to_gh_arguments() {
    assert_eq!(PrFilter::Open.gh_args(), vec!["--state", "open"]);
    assert!(PrFilter::Mine.gh_args().contains(&"@me"));
    assert!(PrFilter::ReviewRequested.gh_args().contains(&"review-requested:@me"));
    assert_eq!(PrFilter::All.gh_args(), vec!["--state", "all"]);
}

#[test]
fn create_form_prefill_uses_single_commit_or_commit_list() {
    assert_eq!(prefill_from_commits("feat/add-login", &[]), ("Add login".into(), String::new()));
    let one = vec![("Fix crash on start".to_string(), "Details here".to_string())];
    assert_eq!(prefill_from_commits("x", &one), ("Fix crash on start".into(), "Details here".into()));
    let many = vec![("One".to_string(), String::new()), ("Two".to_string(), String::new())];
    assert_eq!(prefill_from_commits("fix/some_thing", &many), ("Some thing".into(), "- One\n- Two".into()));
}

#[test]
fn iso_dates_and_relative_times() {
    assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso8601("2026-09-23T21:12:43Z"), Some(1_790_197_963));
    assert_eq!(parse_iso8601("2026-09-24T06:12:43+09:00"), Some(1_790_197_963));
    assert_eq!(parse_iso8601("garbage"), None);
    let now = 1_000_000;
    assert_eq!(relative_time(now - 10, now), "방금");
    assert_eq!(relative_time(now - 3600, now), "1시간 전");
    assert_eq!(relative_time(now - 3 * 86_400, now), "3일 전");
    assert_eq!(short_relative_time(now - 7200, now), "2시간");
}
