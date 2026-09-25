//! 이력(Log) 화면 kittest 공용: 하네스, 대기, 시나리오 저장소.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use crate::common::{self, Repo};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_common::Theme;
use kiln_git::HistoryView;
use kiln_git::history::HistoryEvent;

pub const NOW: i64 = common::BASE_TS + 3600 * 40;

pub struct State {
    pub view: HistoryView,
    pub events: Vec<HistoryEvent>,
}

pub fn theme(ui: &egui::Ui) -> bool {
    Theme::current().apply(ui.ctx());
    common::install_korean_font(ui.ctx());
    ui.ctx().global_style_mut(|s| s.visuals.text_cursor.blink = false);
    common::fonts_ready(ui.ctx())
}

pub fn harness(root: PathBuf, size: egui::Vec2) -> Harness<'static, State> {
    let mut view = HistoryView::new(root);
    view.set_now(NOW);
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut State| {
            if !theme(ui) {
                return;
            }
            let ev = s.view.ui(ui);
            s.events.extend(ev);
        },
        State { view, events: Vec::new() },
    )
}

/// 백그라운드 로드·작업이 끝날 때까지 프레임을 돌린다.
pub fn settle(h: &mut Harness<'_, State>) {
    for _ in 0..800 {
        h.step();
        if !h.state_mut().view.is_busy() {
            h.run_steps(3);
            if !h.state_mut().view.is_busy() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("history view did not settle");
}

/// 작성자를 지정해 커밋한다.
pub fn commit_as(r: &Repo, msg: &str, author: &str) -> String {
    r.git(&["add", "-A"]);
    let ts = r.next_ts();
    Repo::git_in(&r.path, &["commit", "-q", "-m", msg, &format!("--author={author}")], ts);
    r.git(&["rev-parse", "HEAD"]).trim().to_string()
}

/// 원격·태그·병합·로컬 브랜치가 있는 저장소. 마지막 네 커밋은 푸시되지 않았다.
pub fn ui_repo() -> Repo {
    let r = Repo::new();
    r.write("README.md", "# Kiln\n");
    r.write("src/main.rs", &common::numbered(30));
    r.commit_all("Initial import");
    r.git(&["tag", "v0.1.0"]);
    r.git(&["switch", "-q", "-c", "feature/diff-view"]);
    r.write("src/diff.rs", "pub fn diff() {}\n");
    commit_as(&r, "Add diff parser", "Mina Park <mina@kiln.dev>");
    r.write("src/diff.rs", "pub fn diff() {}\npub fn hunk() {}\n");
    commit_as(&r, "Support hunk staging", "Mina Park <mina@kiln.dev>");
    r.git(&["switch", "-q", "main"]);
    r.write("README.md", "# Kiln\n\nA fast terminal.\n");
    commit_as(&r, "Expand README", "Jun Seo <jun@kiln.dev>");
    let ts = r.next_ts();
    Repo::git_in(&r.path, &["merge", "-q", "--no-ff", "-m", "Merge branch 'feature/diff-view'", "feature/diff-view"], ts);
    r.git(&["tag", "-a", "v0.2.0", "-m", "Release 0.2"]);
    let remote = r.tempdir().join("origin.git");
    Repo::git_in(r.tempdir(), &["init", "-q", "--bare", "origin.git"], 0);
    r.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    r.git(&["push", "-q", "-u", "origin", "main"]);
    r.write("src/log.rs", "pub struct Log;\n");
    r.commit_all("Add log view");
    r.write("src/graph.rs", "pub const LANES: usize = 8;\n");
    commit_as(&r, "Tweak graph colors", "Jun Seo <jun@kiln.dev>");
    r.git(&["branch", "wip/experiment"]);
    r.write("src/date.rs", "pub fn fmt() {}\n");
    r.commit_all("Fix date column");
    r.write("src/detail.rs", "pub fn pane() {}\n\npub fn files() {}\n");
    r.write("src/main.rs", &common::numbered(32));
    r.git(&["mv", "src/log.rs", "src/log_view.rs"]);
    r.commit_all("Polish detail pane\n\nAdds the changed files list and author card.\nKeeps the table virtualized.");
    r
}

/// 표의 커밋 행(같은 이름의 상세 패널 글자보다 앞에 있다).
pub fn row<'a>(h: &'a Harness<'_, State>, subject: &'a str) -> egui_kittest::Node<'a> {
    h.get_all_by_label(subject).next().unwrap_or_else(|| panic!("row {subject}"))
}

pub fn subjects(r: &Repo) -> Vec<String> {
    r.git(&["log", "--format=%s", "--first-parent"]).lines().map(str::to_string).collect()
}
