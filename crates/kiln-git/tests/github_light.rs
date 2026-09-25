//! GitHub 허브 라이트 테마 스냅샷. 테마는 전역 값이라 한 테스트 함수 안에서 순서대로 그린다.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::fake_gh::{Detect, FakeGh};
use egui::vec2;
use egui_kittest::Harness;
use kiln_common::Theme;
use kiln_git::{ActionsPanel, GithubHub, HubTab, IssueView, RepoRef};

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

fn hub(detect: Detect, tab: HubTab, size: egui::Vec2) -> Harness<'static, GithubHub> {
    let mut hub = GithubHub::with_backend(PathBuf::from("."), Arc::new(FakeGh::new(detect)));
    hub.set_now(now());
    hub.set_tab(tab);
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, hub: &mut GithubHub| {
            if theme(ui) {
                hub.ui(ui);
            }
        },
        hub,
    );
    settle(&mut h, |s| s.is_loading());
    h
}

#[test]
fn github_hub_light_theme_snapshots() {
    Theme::set_current("kiln-light");
    let mut results = egui_kittest::SnapshotResults::new();

    let mut h = hub(Detect::Ok, HubTab::Issues, vec2(420.0, 640.0));
    h.snapshot("hub_issues_light");
    results.extend_harness(&mut h);

    let mut h = hub(Detect::Ok, HubTab::Actions, vec2(420.0, 640.0));
    h.snapshot("hub_actions_light");
    results.extend_harness(&mut h);

    let mut h = hub(Detect::Auth, HubTab::PullRequests, vec2(420.0, 480.0));
    h.snapshot("hub_auth_required_light");
    results.extend_harness(&mut h);

    let mut v = IssueView::with_backend(Some(RepoRef::new("cli", "cli")), 14420, Arc::new(FakeGh::default()));
    v.set_now(now());
    let mut h = Harness::builder().with_size(vec2(1000.0, 900.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, v: &mut IssueView| {
            if theme(ui) {
                v.ui(ui);
            }
        },
        v,
    );
    settle(&mut h, |v| v.is_loading());
    h.snapshot("issue_view_light");
    results.extend_harness(&mut h);

    let mut p = ActionsPanel::with_backend(Arc::new(FakeGh::default()), Some(RepoRef::new("cli", "cli")));
    p.set_now(now());
    let mut h = Harness::builder().with_size(vec2(460.0, 520.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, p: &mut ActionsPanel| {
            if theme(ui) {
                p.ui(ui);
            }
        },
        p,
    );
    settle(&mut h, |p| p.is_loading());
    let failed = h.state().items().iter().find(|r| r.run_status() == kiln_git::RunStatus::Failure).map(|r| r.database_id);
    h.state_mut().select(failed);
    h.run_steps(2);
    h.snapshot("actions_panel_light");
    results.extend_harness(&mut h);

    Theme::set_current("kiln-dark");
    results.unwrap();
}
