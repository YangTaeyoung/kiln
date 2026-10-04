//! GitHub 허브 UI 테스트(가짜 백엔드)와 다크 테마 스냅샷.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::fake_gh::{Detect, FakeGh};
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_common::Theme;
use kiln_git::{ActionsPanel, GitEvent, GithubHub, HubTab, IssueFilter, IssuePanel, IssueState, IssueView, RepoRef};

fn now() -> i64 {
    kiln_git::util::parse_iso8601("2026-09-26T00:00:00Z").unwrap()
}

fn settle<S>(h: &mut Harness<'_, S>, mut busy: impl FnMut(&mut S) -> bool) {
    for _ in 0..600 {
        h.step();
        if !busy(h.state_mut()) {
            h.run_steps(3);
            if !busy(h.state_mut()) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("background work did not finish");
}

fn theme(ui: &egui::Ui) -> bool {
    Theme::current().apply(ui.ctx());
    common::install_korean_font(ui.ctx());
    ui.ctx().global_style_mut(|s| s.visuals.text_cursor.blink = false);
    common::fonts_ready(ui.ctx())
}

/// 다중 선택 팝업의 항목.
fn pick<'a, S>(h: &'a Harness<'_, S>, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(Role::CheckBox, label)
}

/// 포커스를 가진 한 줄 입력칸.
fn focused_input<'a, S>(h: &'a Harness<'_, S>) -> egui_kittest::Node<'a> {
    h.query_all_by_role(Role::TextInput).find(|n| n.is_focused()).expect("focused text input")
}

// ---------------------------------------------------------------- IssuePanel

struct IssuePanelState {
    panel: IssuePanel,
    events: Vec<GitEvent>,
}

fn issue_panel_harness(backend: Arc<FakeGh>, size: egui::Vec2) -> Harness<'static, IssuePanelState> {
    let mut panel = IssuePanel::with_backend(backend, Some(RepoRef::new("cli", "cli")));
    panel.set_now(now());
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut IssuePanelState| {
            if !theme(ui) {
                return;
            }
            let ev = s.panel.ui(ui);
            s.events.extend(ev);
        },
        IssuePanelState { panel, events: Vec::new() },
    )
}

#[test]
fn issue_panel_filters_searches_and_opens_issues() {
    let backend = Arc::new(FakeGh::default());
    let mut h = issue_panel_harness(backend.clone(), vec2(420.0, 640.0));
    settle(&mut h, |s| s.panel.is_loading());
    assert_eq!(h.state().panel.items().len(), 10);
    assert!(backend.has_call("issues cli/cli Open "), "{:?}", backend.calls());
    h.snapshot("issue_panel");

    // 라벨 칩으로 거른다.
    h.get_by_label("라벨 gh-pr").click();
    h.run_steps(2);
    assert_eq!(h.state().panel.visible_items().len(), 3);
    h.get_by_label("라벨 gh-pr").click();
    h.run_steps(2);
    assert_eq!(h.state().panel.visible_items().len(), 10);

    // 검색어로 거른다(클라이언트 필터).
    h.get_by_role(Role::TextInput).focus();
    h.run_steps(1);
    h.get_by_role(Role::TextInput).type_text("#14420");
    h.run_steps(2);
    assert_eq!(h.state().panel.visible_items().len(), 1);
    // Enter 는 GitHub 검색으로 다시 읽는다.
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call("issues cli/cli Open #14420"), "{:?}", backend.calls());
    h.state_mut().panel.set_search("");
    h.run_steps(2);

    h.get_by_label("닫힘").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert_eq!(h.state().panel.filter(), IssueFilter::Closed);
    assert_eq!(h.state().panel.items().len(), 5);

    h.get_by_label_contains("#14514").click();
    h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::OpenIssue(14514)));
}

#[test]
fn issue_create_form_submits_with_labels_and_assignees() {
    let backend = Arc::new(FakeGh::default());
    let mut h = issue_panel_harness(backend.clone(), vec2(460.0, 720.0));
    settle(&mut h, |s| s.panel.is_loading());
    h.get_by_label("새 이슈").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(h.state().panel.is_form_open());

    // 제목이 비어 있으면 생성 버튼을 눌러도 요청하지 않는다.
    h.get_by_label("생성").click();
    h.run_steps(2);
    assert!(!backend.calls().iter().any(|c| c.starts_with("create_issue")));
    let inputs: Vec<_> = h.query_all_by_role(Role::TextInput).collect();
    inputs[0].focus();
    h.run_steps(1);
    h.query_all_by_role(Role::TextInput).next().unwrap().type_text("Hub crashes on empty repo");
    h.run_steps(1);
    h.get_by_role(Role::MultilineTextInput).focus();
    h.run_steps(1);
    h.get_by_role(Role::MultilineTextInput).type_text("Steps:\n1. open hub");
    h.run_steps(1);

    h.get_by_label("라벨 선택").click();
    h.run_steps(3);
    pick(&h, "bug").click();
    h.run_steps(1);
    pick(&h, "enhancement").click();
    h.run_steps(2);
    h.snapshot("issue_create_labels_popup");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.get_by_label("담당자 선택").click();
    h.run_steps(3);
    focused_input(&h).type_text("william");
    h.run_steps(2);
    pick(&h, "williammartin").click();
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.snapshot("issue_create_form");

    h.get_by_label("생성").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(
        backend.has_call("create_issue cli/cli Hub crashes on empty repo | Steps:\n1. open hub | labels=bug,enhancement | assignees=williammartin"),
        "{:?}",
        backend.calls()
    );
    assert!(h.state().events.contains(&GitEvent::OpenIssue(14600)));
    assert!(!h.state().panel.is_form_open());
}

// ---------------------------------------------------------------- IssueView

struct IssueViewState {
    view: IssueView,
    events: Vec<GitEvent>,
}

fn issue_view_harness(backend: Arc<FakeGh>, size: egui::Vec2) -> Harness<'static, IssueViewState> {
    let mut view = IssueView::with_backend(Some(RepoRef::new("cli", "cli")), 14420, backend);
    view.set_now(now());
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut IssueViewState| {
            if !theme(ui) {
                return;
            }
            let ev = s.view.ui(ui);
            s.events.extend(ev);
        },
        IssueViewState { view, events: Vec::new() },
    )
}

#[test]
fn issue_view_comments_closes_reopens_and_edits_labels() {
    let backend = Arc::new(FakeGh::default());
    let mut h = issue_view_harness(backend.clone(), vec2(1000.0, 1500.0));
    settle(&mut h, |s| s.view.is_loading());
    assert_eq!(h.state().view.detail().unwrap().number, 14420);
    h.snapshot("issue_view");

    // 댓글
    h.get_by_role(Role::MultilineTextInput).scroll_to_me();
    h.run_steps(3);
    h.get_by_role(Role::MultilineTextInput).focus();
    h.run_steps(1);
    h.get_by_role(Role::MultilineTextInput).type_text("Thanks, looking into it");
    h.run_steps(1);
    h.get_by_label("댓글").click();
    h.run_steps(1);
    settle(&mut h, |s| s.view.is_loading());
    assert!(backend.has_call("comment cli/cli 14420 Thanks, looking into it"), "{:?}", backend.calls());
    assert!(h.state_mut().view.comment_mut().is_empty());

    // 닫기 → 다시 열기
    h.get_by_label("이슈 닫기").click();
    h.run_steps(1);
    settle(&mut h, |s| s.view.is_loading());
    assert!(backend.has_call("close cli/cli 14420 Completed"));
    assert_eq!(h.state().view.state(), Some(IssueState::Completed));
    h.get_by_label("다시 열기").click();
    h.run_steps(1);
    settle(&mut h, |s| s.view.is_loading());
    assert!(backend.has_call("reopen cli/cli 14420"));
    assert_eq!(h.state().view.state(), Some(IssueState::Open));

    // 라벨 편집: 팝업에서 고르고 닫으면 반영한다.
    h.get_by_label("라벨 편집").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("라벨 편집").click();
    h.run_steps(1);
    settle(&mut h, |s| s.view.is_loading());
    pick(&h, "bug").click();
    h.run_steps(1);
    pick(&h, "more-info-needed").click();
    h.run_steps(2);
    h.snapshot("issue_view_label_picker");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    settle(&mut h, |s| s.view.is_loading());
    assert!(backend.has_call("edit cli/cli 14420 +l[bug] -l[more-info-needed] +a[] -a[]"), "{:?}", backend.calls());

    // 담당자 편집
    h.get_by_label("담당자 편집").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("담당자 편집").click();
    h.run_steps(2);
    pick(&h, "babakks").click();
    h.run_steps(1);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    settle(&mut h, |s| s.view.is_loading());
    assert!(backend.has_call("edit cli/cli 14420 +l[] -l[] +a[babakks] -a[]"), "{:?}", backend.calls());

    h.get_by_label("브라우저에서 열기").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::OpenUrl("https://github.com/cli/cli/issues/14420".into())));
}

#[test]
fn issue_view_narrow_layout_stacks_sidebar() {
    let backend = Arc::new(FakeGh::default());
    let mut h = issue_view_harness(backend, vec2(560.0, 900.0));
    settle(&mut h, |s| s.view.is_loading());
    h.snapshot("issue_view_narrow");
}

// ---------------------------------------------------------------- ActionsPanel

struct ActionsState {
    panel: ActionsPanel,
    events: Vec<GitEvent>,
}

fn actions_harness(backend: Arc<FakeGh>, size: egui::Vec2) -> Harness<'static, ActionsState> {
    let mut panel = ActionsPanel::with_backend(backend, Some(RepoRef::new("cli", "cli")));
    panel.set_now(now());
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut ActionsState| {
            if !theme(ui) {
                return;
            }
            let ev = s.panel.ui(ui);
            s.events.extend(ev);
        },
        ActionsState { panel, events: Vec::new() },
    )
}

#[test]
fn actions_panel_reruns_cancels_and_opens_logs() {
    let backend = Arc::new(FakeGh::default());
    let mut h = actions_harness(backend.clone(), vec2(460.0, 760.0));
    settle(&mut h, |s| s.panel.is_loading());
    let items = h.state().panel.items().to_vec();
    assert_eq!(items.len(), 12);
    assert_eq!(h.state().panel.active_count(), 2);

    // 실패한 실행: 다시 실행 / 실패한 작업만 / 로그
    let failed = items.iter().find(|r| r.run_status() == kiln_git::RunStatus::Failure).unwrap().clone();
    h.state_mut().panel.select(Some(failed.database_id));
    h.run_steps(2);
    h.snapshot("actions_panel");
    h.get_by_label("다시 실행").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call(&format!("rerun cli/cli {} failed_only=false", failed.database_id)), "{:?}", backend.calls());
    h.state_mut().panel.select(Some(failed.database_id));
    h.run_steps(2);
    h.get_by_label("실패한 작업만 다시 실행").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call(&format!("rerun cli/cli {} failed_only=true", failed.database_id)));
    h.state_mut().panel.select(Some(failed.database_id));
    h.run_steps(2);
    h.get_by_label("로그 보기").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::RunInTerminal(format!("gh run view {} --log -R cli/cli", failed.database_id))));
    h.get_by_label("브라우저에서 열기").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::OpenUrl(failed.url.clone())));

    // 진행 중인 실행: 지켜보기 / 취소 (행 클릭으로 펼친다)
    let running = items[0].clone();
    h.state_mut().panel.select(None);
    h.run_steps(1);
    h.get_by_label_contains(&running.display_title).click();
    h.run_steps(2);
    h.get_by_label("실행 지켜보기").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::RunInTerminal(format!("gh run watch {} -R cli/cli", running.database_id))));
    h.get_by_label("실행 취소").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call(&format!("cancel cli/cli {}", running.database_id)));
}

#[test]
fn actions_panel_filters_by_branch_and_workflow() {
    let backend = Arc::new(FakeGh::default());
    let mut h = actions_harness(backend.clone(), vec2(460.0, 640.0));
    settle(&mut h, |s| s.panel.is_loading());
    h.get_by_label("워크플로 전체").click();
    h.run_steps(2);
    h.snapshot("actions_workflow_filter");
    h.get_by_label("Lint").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call("runs cli/cli None Some(\"Lint\")"), "{:?}", backend.calls());
    assert!(h.state().panel.items().iter().all(|r| r.workflow_name == "Lint"));

    h.get_by_label("브랜치 전체").click();
    h.run_steps(2);
    h.get_by_label("trunk").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert!(backend.has_call("runs cli/cli Some(\"trunk\") Some(\"Lint\")"), "{:?}", backend.calls());
}

// ---------------------------------------------------------------- GithubHub

struct HubState {
    hub: GithubHub,
    events: Vec<GitEvent>,
}

fn hub_harness(backend: Arc<FakeGh>, size: egui::Vec2) -> Harness<'static, HubState> {
    let mut hub = GithubHub::with_backend(PathBuf::from("."), backend);
    hub.set_now(now());
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut HubState| {
            if !theme(ui) {
                return;
            }
            let ev = s.hub.ui(ui);
            s.events.extend(ev);
        },
        HubState { hub, events: Vec::new() },
    )
}

#[test]
fn hub_switches_tabs_and_repositories() {
    let backend = Arc::new(FakeGh::default());
    let mut h = hub_harness(backend.clone(), vec2(420.0, 720.0));
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state().hub.repo(), Some(RepoRef::new("cli", "cli")));
    assert!(backend.has_call("pr_backend -"), "workspace repo uses no -R: {:?}", backend.calls());
    h.snapshot("hub_pull_requests");

    h.get_by_label("이슈").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state().hub.tab(), HubTab::Issues);
    assert!(backend.has_call("issues - Open "));
    h.snapshot("hub_issues");

    h.get_by_label("Actions").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    h.snapshot("hub_actions");

    // 저장소 선택 팝업
    h.get_by_label("저장소 cli/cli").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert!(h.query_by_label("octocat/kiln").is_some());
    h.snapshot("hub_repo_picker");

    h.get_by_label("소유자 cli").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert!(backend.has_call("list_repos cli"));
    h.get_by_label("cli/go-gh 새 프로젝트로 복제").click();
    h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::CloneRepo { name_with_owner: "cli/go-gh".into() }));

    h.get_by_label("저장소 cli/cli").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    h.get_by_label("소유자 octocat").click();
    h.run_steps(2);
    h.get_by_label("octocat/kiln").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state().hub.repo(), Some(RepoRef::new("octocat", "kiln")));
    assert!(backend.has_call("repo_info octocat/kiln"));
    assert!(backend.has_call("runs octocat/kiln None None"), "{:?}", backend.calls());
    h.snapshot("hub_other_repo");

    h.get_by_label("풀 리퀘스트").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert!(backend.has_call("pr_list octocat/kiln Open"), "{:?}", backend.calls());
    assert!(h.query_by_label("새 PR").is_none(), "PR create is only offered for the workspace repo");

    // 작업 폴더 저장소로 돌아가기
    h.state_mut().hub.use_workspace_repo();
    h.run_steps(2);
    assert_eq!(h.state().hub.repo(), Some(RepoRef::new("cli", "cli")));
}

#[test]
fn hub_search_repos_on_enter() {
    let backend = Arc::new(FakeGh::default());
    let mut h = hub_harness(backend.clone(), vec2(460.0, 640.0));
    settle(&mut h, |s| s.hub.is_loading());
    let ctx = h.ctx.clone();
    h.state_mut().hub.toggle_repo_picker(&ctx);
    h.run_steps(2);
    settle(&mut h, |s| s.hub.is_loading());
    focused_input(&h).type_text("terminal emulator");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert!(backend.has_call("search_repos terminal emulator"));
    assert!(h.query_by_label("alacritty/alacritty").is_some());
    h.snapshot("hub_repo_search");
}

#[test]
fn hub_shows_login_instructions_when_not_authenticated() {
    let backend = Arc::new(FakeGh::new(Detect::Auth));
    let mut h = hub_harness(backend, vec2(420.0, 520.0));
    settle(&mut h, |s| s.hub.is_loading());
    assert!(matches!(h.state().hub.error(), Some(kiln_git::GitError::GhAuth(_))));
    h.snapshot("hub_auth_required");
    h.get_by_label("터미널에서 로그인").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::RunInTerminal("gh auth login".into())));
}

#[test]
fn hub_shows_install_hint_and_repo_fallback() {
    let backend = Arc::new(FakeGh::new(Detect::Missing));
    let mut h = hub_harness(backend, vec2(420.0, 480.0));
    settle(&mut h, |s| s.hub.is_loading());
    assert!(matches!(h.state().hub.error(), Some(kiln_git::GitError::GhMissing)));
    h.snapshot("hub_gh_missing");
    h.get_by_label("설치 안내 열기").click();
    h.run_steps(1);
    assert!(h.state().events.contains(&GitEvent::OpenUrl("https://cli.github.com".into())));
}

#[test]
fn hub_without_github_remote_offers_repo_picker() {
    let backend = Arc::new(FakeGh::new(Detect::NoRemote));
    let mut h = hub_harness(backend.clone(), vec2(420.0, 560.0));
    settle(&mut h, |s| s.hub.is_loading());
    h.snapshot("hub_no_remote");
    h.get_by_label("다른 저장소 선택…").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    h.get_by_label("octocat/dotfiles").click();
    h.run_steps(1);
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state().hub.repo(), Some(RepoRef::new("octocat", "dotfiles")));
    assert!(h.state().hub.error().is_none());
}

#[test]
fn issue_draft_survives_close_reopen_and_requires_explicit_discard() {
    let backend = Arc::new(FakeGh::default());
    let mut h = issue_panel_harness(backend.clone(), vec2(460.0, 720.0));
    let draft = kiln_git::IssueCreate { title: "Local draft".into(), body: "Not submitted".into(), labels: vec!["bug".into()], assignees: vec!["alice".into()] };
    h.state_mut().panel.restore_creation_draft(&draft);
    settle(&mut h, |s| s.panel.is_loading());
    h.get_by_label("초안 보관하고 닫기").click(); h.run_steps(2);
    assert!(!h.state().panel.is_form_open());
    assert_eq!(h.state().panel.creation_draft(), Some(draft.clone()));
    h.get_by_label("초안 이어 쓰기").click(); h.run_steps(2);
    assert!(h.state().panel.is_form_open());
    h.get_by_label("초안 버리기…").click(); h.run_steps(2);
    h.get_by_label("계속 작성").click(); h.run_steps(2);
    assert_eq!(h.state().panel.creation_draft(), Some(draft));
    h.get_by_label("초안 버리기…").click(); h.run_steps(2);
    h.get_by_label("초안 버리기").click(); h.run_steps(2);
    assert!(h.state().panel.creation_draft().is_none());
    assert!(!backend.calls().iter().any(|c| c.starts_with("create_issue")));
}

#[test]
fn github_drafts_restore_each_repository_and_offer_direct_resume_without_sending() {
    let backend = Arc::new(FakeGh::default());
    let mut h = hub_harness(backend.clone(), vec2(600.0, 840.0));
    let mut drafts = kiln_git::GithubDrafts::default();
    for name in ["cli/cli", "octocat/kiln"] {
        drafts.repositories.insert(name.into(), kiln_git::RepositoryDrafts {
            issue: Some(kiln_git::IssueCreate { title: format!("draft {name}"), ..Default::default() }),
            ..Default::default()
        });
    }
    h.state_mut().hub.restore_drafts(&drafts);
    settle(&mut h, |s| s.hub.is_loading());
    h.get_by_label("보관된 작성 초안 · 저장소 2개").click(); h.run_steps(2);
    h.get_by_label("octocat/kiln · 이슈 이어 쓰기").click(); h.run_steps(2);
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state().hub.repo(), Some(RepoRef::new("octocat", "kiln")));
    assert_eq!(h.state_mut().hub.issue_panel().unwrap().creation_draft().unwrap().title, "draft octocat/kiln");
    h.get_by_label("cli/cli · 이슈 이어 쓰기").click(); h.run_steps(2);
    settle(&mut h, |s| s.hub.is_loading());
    assert_eq!(h.state_mut().hub.issue_panel().unwrap().creation_draft().unwrap().title, "draft cli/cli");
    assert_eq!(h.state().hub.recovery_drafts().repositories, drafts.repositories);
    assert!(!backend.calls().iter().any(|c| c.starts_with("create_issue") || c.starts_with("create ")));
}

#[test]
fn issue_creation_long_repository_fits_360_points() {
    let backend=Arc::new(FakeGh::default());
    let mut panel=IssuePanel::with_backend(backend,Some(RepoRef::new("organization-long-한글".repeat(4),"repository-long".repeat(4))));
    panel.restore_creation_draft(&kiln_git::IssueCreate {title:"긴 이슈 제목long-title".repeat(12),..Default::default()});
    let mut h=Harness::builder().with_size(vec2(360.0,720.0)).wgpu().build_ui_state(|ui,p:&mut IssuePanel|{if theme(ui){p.ui(ui);}},panel);
    settle(&mut h,|p|p.is_loading());
    h.render().unwrap().save("/tmp/kiln-issue-create-360.png").unwrap();
    for label in ["초안 버리기…","초안 보관하고 닫기","생성"] {
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}: {:?}",h.get_by_label(label).rect());
    }
    for input in h.query_all_by_role(Role::TextInput) {assert!(h.ctx.content_rect().contains_rect(input.rect()),"input overflow {:?}",input.rect());}
}
