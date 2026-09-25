//! 라이트 테마 스냅샷. 테마는 전역 값이라 한 테스트 함수 안에서 순서대로 그린다.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{Repo, numbered};
use egui::vec2;
use egui_kittest::Harness;
use kiln_common::Theme;
use kiln_git::gh::parse_pr_view;
use kiln_git::{
    DiffView, GitPanel, GitResult, MergeMethod, PrBackend, PrCreate, PrCreateDefaults, PrDetail, PrFilter, PrItem, PrView,
    ReviewKind,
};

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

/// 픽스처 PR 하나만 돌려주는 백엔드.
struct Fixture;

impl PrBackend for Fixture {
    fn list(&self, _filter: PrFilter) -> GitResult<Vec<PrItem>> {
        Ok(Vec::new())
    }
    fn view(&self, _number: u64) -> GitResult<PrDetail> {
        let mut d = parse_pr_view(include_str!("fixtures/pr_view.json")).unwrap();
        d.state = "OPEN".into();
        d.body = "## Summary\n\nImproves **CLI recording** rendering and adds a `--font` flag.\n\n- Frames terminal output\n- Records fallback font provenance\n\n```go\nfunc frame() {}\n```".into();
        Ok(d)
    }
    fn diff(&self, _number: u64) -> GitResult<String> {
        Ok(String::new())
    }
    fn create_defaults(&self) -> GitResult<PrCreateDefaults> {
        Ok(PrCreateDefaults { head: String::new(), base: String::new(), bases: Vec::new(), title: String::new(), body: String::new() })
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

#[test]
fn light_theme_snapshots() {
    Theme::set_current("kiln-light");
    let mut results = egui_kittest::SnapshotResults::new();

    // GitPanel
    let r = Repo::new();
    r.write("src/main.rs", &numbered(20));
    r.write("README.md", "# Kiln\n");
    r.commit_all("Initial import");
    r.git(&["tag", "v0.1.0"]);
    r.write("src/lib.rs", "pub fn lib() {}\n");
    r.commit_all("Add library");
    r.write("src/main.rs", &numbered(20).replace("line 3\n", "line three\n"));
    r.write("src/new.rs", "pub struct New;\n");
    r.git(&["add", "src/new.rs"]);
    r.write("notes.txt", "todo\n");
    let mut panel = GitPanel::new(r.path.clone());
    panel.set_now(common::BASE_TS + 3600 * 30);
    let mut h = Harness::builder().with_size(vec2(380.0, 620.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, p: &mut GitPanel| {
            if theme(ui) {
                p.ui(ui);
            }
        },
        panel,
    );
    settle(&mut h, |p| p.is_busy());
    h.snapshot("git_panel_light");
    results.extend_harness(&mut h);

    // DiffView
    let mut v = DiffView::for_file(&r.path, &r.path.join("src/main.rs"), false);
    v.set_now(common::BASE_TS);
    let mut h = Harness::builder().with_size(vec2(760.0, 360.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, v: &mut DiffView| {
            if theme(ui) {
                v.ui(ui);
            }
        },
        v,
    );
    settle(&mut h, |v| v.is_loading());
    h.snapshot("diff_view_light");
    results.extend_harness(&mut h);

    // PrView
    let mut pv = PrView::with_backend(PathBuf::from("."), 14507, Arc::new(Fixture));
    pv.set_now(1_790_300_000);
    let mut h = Harness::builder().with_size(vec2(900.0, 700.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, v: &mut PrView| {
            if theme(ui) {
                v.ui(ui);
            }
        },
        pv,
    );
    settle(&mut h, |v| v.is_loading());
    h.snapshot("pr_view_light");
    results.extend_harness(&mut h);

    Theme::set_current("kiln-dark");
    results.unwrap();
}
