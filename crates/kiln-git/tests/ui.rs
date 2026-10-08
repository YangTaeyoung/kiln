//! egui_kittest 기반 UI 스모크 테스트와 스냅샷.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Repo, numbered};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_common::Theme;
use kiln_git::gh::{parse_pr_list, parse_pr_view};
use kiln_git::{
    DiffMode, DiffView, GitEvent, GitPanel, GitResult, MergeMethod, PrBackend, PrCreate, PrCreateDefaults, PrDetail,
    PrFilter, PrItem, PrPanel, PrTab, PrView, ReviewKind,
};

const NOW: i64 = common::BASE_TS + 3600 * 30;

/// 백그라운드 작업이 끝날 때까지 프레임을 돌린다.
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

/// 테마와 번들 글꼴을 적용한다. 글꼴이 아직 활성화되지 않은 프레임이면 false.
fn theme(ui: &egui::Ui) -> bool {
    Theme::current().apply(ui.ctx());
    common::install_korean_font(ui.ctx());
    ui.ctx().global_style_mut(|s| s.visuals.text_cursor.blink = false);
    common::fonts_ready(ui.ctx())
}

// ---------------------------------------------------------------- GitPanel

/// 스테이지/변경/미추적/stash/병합 이력이 있는 저장소.
fn panel_repo() -> Repo {
    let r = Repo::new();
    r.write("README.md", "# Kiln\n");
    r.write("src/main.rs", &numbered(40));
    r.write("src/git/status.rs", &numbered(10));
    r.write("docs/guide.md", "guide\n");
    r.commit_all("Initial import");
    r.git(&["tag", "v0.1.0"]);
    r.git(&["switch", "-q", "-c", "feature/diff-view"]);
    r.write("src/diff.rs", "pub fn diff() {}\n");
    r.commit_all("Add diff parser");
    r.write("src/diff.rs", "pub fn diff() {}\npub fn hunk() {}\n");
    r.commit_all("Support hunk staging");
    r.git(&["switch", "-q", "main"]);
    r.write("README.md", "# Kiln\n\nA fast terminal.\n");
    r.commit_all("Expand README");
    let ts = r.next_ts();
    Repo::git_in(&r.path, &["merge", "-q", "--no-ff", "-m", "Merge branch 'feature/diff-view'", "feature/diff-view"], ts);
    r.write("src/main.rs", &numbered(41));
    r.commit_all("Tweak main loop");

    r.write("stash.txt", "wip\n");
    Repo::git_in(&r.path, &["stash", "push", "-q", "-u", "-m", "half-done refactor"], r.next_ts());

    // 작업트리 상태
    r.write("src/main.rs", &numbered(41).replace("line 5\n", "line five\n"));
    r.write("src/git/status.rs", &numbered(12));
    r.git(&["add", "src/git/status.rs"]);
    r.write("src/new_module.rs", "pub struct Module;\n");
    r.git(&["add", "src/new_module.rs"]);
    std::fs::remove_file(r.path.join("docs/guide.md")).unwrap();
    r.write("notes/todo list.md", "- [ ] ship\n");
    r.write("scratch.txt", "tmp\n");
    r
}

struct PanelState {
    panel: GitPanel,
    events: Vec<GitEvent>,
}

fn panel_harness(root: PathBuf, size: egui::Vec2) -> Harness<'static, PanelState> {
    let mut panel = GitPanel::new(root);
    panel.set_now(NOW);
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut PanelState| {
            if !theme(ui) {
                return;
            }
            let ev = s.panel.ui(ui);
            s.events.extend(ev);
        },
        PanelState { panel, events: Vec::new() },
    )
}

#[test]
fn commit_history_expand_is_available_even_when_history_is_collapsed() {
    let r = Repo::new();
    r.write("README.md", "# Test\n");
    r.commit_all("Initial commit");
    let original_head=r.git(&["rev-parse", "HEAD"]);
    let mut h=panel_harness(r.path.clone(),vec2(280.0,560.0));
    settle(&mut h, |s|s.panel.is_busy());
    h.get_by_label("커밋 기록").click();h.run_steps(2);
    let button=h.get_by_label("커밋 기록 크게 보기");
    assert!(h.ctx.content_rect().contains_rect(button.rect()));
    button.click();h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::OpenHistory));
    assert_eq!(r.git(&["rev-parse", "HEAD"]),original_head);
    h.render().unwrap().save("/tmp/kiln-git-history-entry-280.png").unwrap();
}

fn inline_repo()->Repo {
    let r=Repo::new();
    for n in 0..4 {r.write(&format!("file{n}.txt"),&format!("{n}\n"));r.commit_all(&format!("Commit {n}"));}
    r.git(&["switch","-q","-c","feature"]);
    r.write("feature.txt","Feature\n");r.commit_all("Feature change");
    r.git(&["switch","-q","main"]);r
}
fn inline_panel(root:PathBuf)->Harness<'static,PanelState> {
    Harness::builder().with_size(vec2(1000.0,900.0)).wgpu().build_ui_state(|ui,s:&mut PanelState|{
        if !theme(ui){return;}ui.set_max_width(320.0);s.events.extend(s.panel.ui(ui));
    },PanelState{panel:GitPanel::new(root),events:vec![]})
}
#[test]
fn inline_history_context_actions_and_drag_need_no_edit_mode() {
    let r=inline_repo();let head=r.git(&["rev-parse","HEAD"]);
    let mut h=inline_panel(r.path.clone());settle(&mut h,|s|s.panel.is_busy());
    assert!(h.query_by_label("기록 편집").is_none());
    h.get_by_label("Commit 3").click_secondary();h.run_steps(3);
    for label in ["메시지 수정…","커밋 삭제","체리픽"] {h.get_by_label(label);}
    h.render().unwrap().save("/tmp/kiln-inline-history-menu.png").unwrap();
    h.get_by_label("메시지 수정…").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("커밋 메시지 수정");
    h.get_by_label("취소").click();h.run_steps(3);
    let from=h.get_by_label("Commit 3").rect();let to=h.get_by_label("Commit 1").rect();
    h.hover_at(from.center());h.run_steps(1);h.drag_at(from.center());h.run_steps(1);
    h.hover_at(to.center());h.run_steps(2);h.drop_at(to.center());h.run_steps(2);
    h.get_by_label("Commit 2").click_secondary();h.run_steps(3);
    h.get_by_label("하나로 스쿼시…").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("커밋 스쿼시 검토");h.get_by_label("취소").click();h.run_steps(2);
    assert_eq!(r.git(&["rev-parse","HEAD"]),head);
}
#[test]
fn inline_history_rewords_commit_and_preserves_commit_box_draft() {
    let r=inline_repo();let mut h=inline_panel(r.path.clone());settle(&mut h,|s|s.panel.is_busy());
    *h.state_mut().panel.commit_message_mut()="Unrelated next commit draft".into();
    h.get_by_label("Commit 3").click_secondary();h.run_steps(2);
    h.get_by_label("메시지 수정…").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_all_by_role(egui::accesskit::Role::MultilineTextInput).last().unwrap().focus();h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND,egui::Key::A);
    h.get_all_by_role(egui::accesskit::Role::MultilineTextInput).last().unwrap().type_text("Renamed commit");h.run_steps(2);
    h.get_by_label("메시지 수정").click();settle(&mut h,|s|s.panel.is_busy());
    assert_eq!(r.git(&["log","-1","--format=%s"]).trim(),"Renamed commit");
    assert_eq!(h.state().panel.commit_draft(),"Unrelated next commit draft");
    h.get_by_label("Renamed commit");
}
#[test]
fn inline_history_cherry_picks_other_branch_then_drops_with_review() {
    let r=inline_repo();let mut h=inline_panel(r.path.clone());settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("Feature change").click_secondary();h.run_steps(2);
    h.get_by_label("체리픽").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("현재 브랜치에 체리픽").click();settle(&mut h,|s|s.panel.is_busy());
    assert!(r.path.join("feature.txt").exists());
    assert_eq!(r.git(&["branch","--show-current"]).trim(),"main");
    // Filter to the destination branch to avoid the original source commit.
    h.get_by_label("브랜치 필터").click();h.run_steps(2);h.get_by_label("현재 브랜치").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("Feature change").click_secondary();h.run_steps(2);
    h.get_by_label("커밋 삭제").click();settle(&mut h,|s|s.panel.is_busy());
    h.get_by_label("커밋 삭제 검토");
    h.get_by_label("기본 브랜치·공유 이력에 미치는 영향을 확인했습니다").click();h.run_steps(2);
    h.get_by_label("커밋 삭제").click();settle(&mut h,|s|s.panel.is_busy());
    assert!(!r.path.join("feature.txt").exists());
    assert_eq!(r.git(&["log","-1","--format=%s"]).trim(),"Commit 3");
}

#[test]
fn git_panel_renders_sections_and_emits_events() {
    let r = panel_repo();
    let mut h = panel_harness(r.path.clone(), vec2(380.0, 900.0));
    settle(&mut h, |s| s.panel.is_busy());

    let st = h.state().panel.status().expect("status loaded").clone();
    assert_eq!(st.staged().count(), 2);
    assert_eq!(st.unstaged().count(), 2);
    assert_eq!(st.untracked().count(), 2);

    // 장식은 패널 루트 기준 절대 경로를 쓴다.
    let deco = h.state().panel.file_decorations();
    assert_eq!(deco.get(&r.path.join("src/main.rs")).map(|d| d.1), Some('M'));
    assert_eq!(deco.get(&r.path.join("src/new_module.rs")).map(|d| d.1), Some('A'));
    assert_eq!(deco.get(&r.path.join("docs/guide.md")).map(|d| d.1), Some('D'));
    assert_eq!(deco.get(&r.path.join("scratch.txt")).map(|d| d.1), Some('?'));

    h.snapshot("git_panel");

    // 파일 행 클릭 → OpenDiff
    h.get_by_label("src/main.rs").click();
    h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::OpenDiff { path: r.path.join("src/main.rs"), staged: false }));

    // A normal click selects; opening the diff is an explicit row action.
    h.get_by_label("Tweak main loop").click_secondary();
    h.run_steps(2);
    h.get_by_label("커밋 diff 열기").click();
    h.run_steps(2);
    let head = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    assert!(h.state().events.contains(&GitEvent::History(kiln_git::history::HistoryEvent::OpenCommit(head))));
}

#[test]
fn git_panel_stage_all_and_commit_with_keyboard_shortcut() {
    let r = panel_repo();
    let mut h = panel_harness(r.path.clone(), vec2(380.0, 900.0));
    settle(&mut h, |s| s.panel.is_busy());

    h.get_by_label("모든 변경 사항 스테이징").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_busy());
    assert_eq!(h.state().panel.status().unwrap().staged().count(), 4);

    h.get_by_label("모두 스테이징 취소").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_busy());
    assert_eq!(h.state().panel.status().unwrap().staged().count(), 0);

    h.get_by_label("모든 변경 사항 스테이징").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_busy());

    // 메시지 입력 후 Cmd+Enter
    let msg_box = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    msg_box.focus();
    h.run_steps(1);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text("Refine status handling");
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_busy());
    let last = r.git(&["log", "-1", "--format=%s"]);
    assert_eq!(last.trim(), "Refine status handling");
    assert_eq!(h.state().panel.status().unwrap().staged().count(), 0);
    assert!(h.state_mut().panel.commit_message_mut().is_empty());
}

#[test]
fn git_panel_shows_merge_conflicts() {
    let r = Repo::new();
    r.write("config.toml", "value = 1\n");
    r.commit_all("Base config");
    r.git(&["switch", "-q", "-c", "other"]);
    r.write("config.toml", "value = 2\n");
    r.commit_all("Other value");
    r.git(&["switch", "-q", "main"]);
    r.write("config.toml", "value = 3\n");
    r.commit_all("Main value");
    assert!(!r.git_status(&["merge", "other"]));

    let mut h = panel_harness(r.path.clone(), vec2(380.0, 560.0));
    settle(&mut h, |s| s.panel.is_busy());
    assert_eq!(h.state().panel.status().unwrap().conflicted_count(), 1);
    assert_eq!(h.state().panel.file_decorations().get(&r.path.join("config.toml")).map(|d| d.1), Some('C'));
    h.snapshot("git_panel_conflict");

    // 충돌 파일 클릭은 파일 열기
    h.get_by_label("config.toml").click();
    h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::OpenFile(r.path.join("config.toml"))));
}

#[test]
fn git_panel_branch_picker_lists_and_creates_branches() {
    let r = panel_repo();
    let mut h = panel_harness(r.path.clone(), vec2(380.0, 700.0));
    settle(&mut h, |s| s.panel.is_busy());
    h.get_by_label_contains("브랜치 main").click();
    h.run_steps(3);
    std::thread::sleep(Duration::from_millis(200));
    h.run_steps(3);
    assert!(h.query_by_label("feature/diff-view").is_some(), "branch list shows local branches");
    h.snapshot("git_panel_branch_picker");

    h.get_by_role(egui::accesskit::Role::TextInput).type_text("topic/new-thing");
    h.run_steps(2);
    h.get_by_label_contains("새 브랜치").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_busy());
    assert_eq!(h.state().panel.status().unwrap().branch.head.as_deref(), Some("topic/new-thing"));
}

#[test]
fn git_panel_reports_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = panel_harness(dir.path().to_path_buf(), vec2(320.0, 260.0));
    settle(&mut h, |s| s.panel.is_busy());
    assert!(matches!(h.state().panel.error(), Some(kiln_git::GitError::NotARepo)));
    assert!(h.query_by_label("저장소 초기화").is_some());
    h.snapshot("git_panel_not_repo");
}

// ---------------------------------------------------------------- DiffView

fn diff_harness(view: DiffView, size: egui::Vec2) -> Harness<'static, DiffView> {
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, v: &mut DiffView| {
            if !theme(ui) {
                return;
            }
            v.ui(ui);
        },
        view,
    )
}

fn diff_repo() -> Repo {
    let r = Repo::new();
    let src = "use std::io;\n\nfn main() {\n    let name = \"kiln\";\n    println!(\"hello {}\", name);\n    run(name);\n}\n\nfn run(name: &str) {\n    for i in 0..3 {\n        println!(\"{i}: {name}\");\n    }\n}\n\nfn helper() -> u32 {\n    42\n}\n\nfn tail() {}\n";
    r.write("src/main.rs", src);
    r.commit_all("Initial");
    let changed = src
        .replace("    let name = \"kiln\";", "    let name = \"kiln-terminal\";")
        .replace("    run(name);\n", "    run(name);\n    helper();\n")
        .replace("    42\n", "    41 + 1\n")
        .replace("fn tail() {}\n", "fn tail() {\n    // done\n}\n");
    r.write("src/main.rs", &changed);
    r
}

#[test]
fn diff_view_unified_and_split_modes_render_file_diff() {
    let r = diff_repo();
    let mut v = DiffView::for_file(&r.path, &r.path.join("src/main.rs"), false);
    v.set_now(NOW);
    let mut h = diff_harness(v, vec2(760.0, 520.0));
    settle(&mut h, |v| v.is_loading());
    assert_eq!(h.state().files().len(), 1);
    assert_eq!(h.state().files()[0].hunks.len(), 2);
    h.snapshot("diff_view_unified");

    h.get_by_label("나란히").click();
    h.run_steps(2);
    assert_eq!(h.state().mode(), DiffMode::SideBySide);
    h.snapshot("diff_view_split");
}

#[test]
fn diff_view_stage_hunk_button_stages_only_that_hunk() {
    let r = diff_repo();
    let mut h = diff_harness(DiffView::for_file(&r.path, &r.path.join("src/main.rs"), false), vec2(760.0, 520.0));
    settle(&mut h, |v| v.is_loading());
    h.get_by_label("헝크 스테이징 2").click();
    h.run_steps(1);
    settle(&mut h, |v| v.is_loading());
    let staged = kiln_git::repo::file_diff(&r.path, "src/main.rs", true).unwrap().unwrap();
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.hunks[0].lines.iter().any(|l| l.text.contains("41 + 1")));
    // 뷰가 다시 읽혀 남은 헝크만 보인다.
    assert_eq!(h.state().files()[0].hunks.len(), 1);

    let mut hs = diff_harness(DiffView::for_file(&r.path, &r.path.join("src/main.rs"), true), vec2(760.0, 320.0));
    settle(&mut hs, |v| v.is_loading());
    hs.snapshot("diff_view_staged");
    hs.get_by_label("헝크 스테이징 취소 1").click();
    hs.run_steps(1);
    settle(&mut hs, |v| v.is_loading());
    assert!(kiln_git::repo::file_diff(&r.path, "src/main.rs", true).unwrap().is_none());
}

#[test]
fn diff_view_commit_mode_shows_header_and_file_list() {
    let r = diff_repo();
    r.write("docs/readme.md", "# Docs\n\nSome text.\n");
    r.git(&["mv", "src/main.rs", "src/app.rs"]);
    r.write("src/app.rs", &r.read("src/app.rs"));
    let sha = r.commit_all("Move entry point and document it\n\nThe binary now starts from app.rs.\nDocs added.");
    let mut v = DiffView::for_commit(&r.path, &sha);
    v.set_now(NOW);
    let mut h = diff_harness(v, vec2(900.0, 600.0));
    settle(&mut h, |v| v.is_loading());
    assert_eq!(h.state().files().len(), 2);
    h.snapshot("diff_view_commit");
}

#[test]
fn diff_view_virtualizes_large_diffs() {
    let r = Repo::new();
    r.write("big.txt", &numbered(20_000));
    r.commit_all("big");
    let changed: String = (1..=20_000).map(|i| if i % 3 == 0 { format!("changed {i}\n") } else { format!("line {i}\n") }).collect();
    r.write("big.txt", &changed);
    let mut h = diff_harness(DiffView::for_file(&r.path, &r.path.join("big.txt"), false), vec2(700.0, 400.0));
    settle(&mut h, |v| v.is_loading());
    let lines: usize = h.state().files()[0].hunks.iter().map(|h| h.lines.len()).sum();
    assert!(lines > 20_000);
    let start = std::time::Instant::now();
    h.run_steps(5);
    assert!(start.elapsed() < Duration::from_secs(5), "frames over a large diff stay fast");
}

// ---------------------------------------------------------------- PR

/// 픽스처를 돌려주고 호출을 기록하는 가짜 백엔드.
#[derive(Default)]
struct FakeBackend {
    fail_create: std::sync::atomic::AtomicBool,
    calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

fn fake_list() -> Vec<PrItem> {
    let mut v = parse_pr_list(include_str!("fixtures/pr_list.json")).unwrap();
    // 일부 항목의 상태를 바꾼다.
    v[0].state = "OPEN".into();
    v[0].is_draft = true;
    v[0].review_decision = String::new();
    v[1].state = "OPEN".into();
    v[1].review_decision = "CHANGES_REQUESTED".into();
    v[1].status_check_rollup[0].conclusion = Some("FAILURE".into());
    v[2].state = "OPEN".into();
    v[2].status_check_rollup[0].status = Some("IN_PROGRESS".into());
    v
}

impl PrBackend for FakeBackend {
    fn list(&self, filter: PrFilter) -> GitResult<Vec<PrItem>> {
        self.log(format!("list {filter:?}"));
        let v = fake_list();
        Ok(match filter {
            PrFilter::All => v,
            PrFilter::Mine => v.into_iter().filter(|p| p.author.login == "williammartin" && p.state == "OPEN").collect(),
            _ => v.into_iter().filter(|p| p.state == "OPEN").collect(),
        })
    }
    fn view(&self, number: u64) -> GitResult<PrDetail> {
        self.log(format!("view {number}"));
        let mut d = parse_pr_view(include_str!("fixtures/pr_view.json")).unwrap();
        d.state = "OPEN".into();
        d.body = "## Summary\n\nImproves **CLI recording** rendering and adds a `--font` flag.\n\n- Frames terminal output with a 48px safe area\n- Records fallback font provenance\n  - nested detail\n\n```go\nfunc frame(img *Image) *Image {\n    return pad(img, 48)\n}\n```\n\n> Note: legacy contracts keep working.\n\nSee [the docs](https://cli.github.com) for more.".into();
        d.status_check_rollup[1].conclusion = Some("FAILURE".into());
        Ok(d)
    }
    fn diff(&self, number: u64) -> GitResult<String> {
        self.log(format!("diff {number}"));
        Ok(include_str!("fixtures/pr_diff.patch").to_string())
    }
    fn create_defaults(&self) -> GitResult<PrCreateDefaults> {
        self.log("defaults".into());
        Ok(PrCreateDefaults {
            head: "feature/diff-view".into(),
            base: "main".into(),
            bases: vec!["main".into(), "release/1.x".into()],
            title: "Add diff view".into(),
            body: "- Add diff parser\n- Support hunk staging".into(),
        })
    }
    fn create(&self, req: &PrCreate) -> GitResult<String> {
        self.log(format!("create {} -> {} draft={}", req.title, req.base, req.draft));
        if self.fail_create.load(std::sync::atomic::Ordering::SeqCst) { return Err(kiln_git::GitError::Failed("offline".into())); }
        Ok("https://github.com/example/kiln/pull/7".into())
    }
    fn review(&self, number: u64, kind: ReviewKind, body: &str) -> GitResult<()> {
        self.log(format!("review {number} {kind:?} {body}"));
        Ok(())
    }
    fn merge(&self, number: u64, method: MergeMethod, delete_branch: bool) -> GitResult<String> {
        self.log(format!("merge {number} {method:?} delete={delete_branch}"));
        Ok(String::new())
    }
    fn checkout(&self, number: u64) -> GitResult<String> {
        self.log(format!("checkout {number}"));
        Ok(String::new())
    }
    fn mark_ready(&self, number: u64) -> GitResult<()> {
        self.log(format!("ready {number}"));
        Ok(())
    }
}

struct PrPanelState {
    panel: PrPanel,
    events: Vec<GitEvent>,
}

#[test]
fn pr_panel_lists_filters_and_opens_prs() {
    let backend = Arc::new(FakeBackend::default());
    let mut panel = PrPanel::with_backend(backend.clone());
    panel.set_now(common::BASE_TS);
    let mut h = Harness::builder().with_size(vec2(420.0, 520.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, s: &mut PrPanelState| {
            if !theme(ui) {
                return;
            }
            let ev = s.panel.ui(ui);
            s.events.extend(ev);
        },
        PrPanelState { panel, events: Vec::new() },
    );
    // 상대 시간 기준 시각을 고정한다.
    h.state_mut().panel.set_now(1_790_300_000);
    settle(&mut h, |s| s.panel.is_loading());
    assert_eq!(h.state().panel.items().len(), 4);
    h.snapshot("pr_panel_open");

    h.get_by_label("전체").click();
    h.run_steps(1);
    settle(&mut h, |s| s.panel.is_loading());
    assert_eq!(h.state().panel.filter(), PrFilter::All);
    assert_eq!(h.state().panel.items().len(), 8);
    assert!(backend.calls().contains(&"list All".to_string()));

    h.get_by_label_contains("#14507").click();
    h.run_steps(2);
    assert!(h.state().events.contains(&GitEvent::OpenPr(14507)));
}

#[test]
fn pr_panel_create_form_prefills_and_submits() {
    let backend = Arc::new(FakeBackend::default());
    let panel = PrPanel::with_backend(backend.clone());
    let mut h = Harness::builder().with_size(vec2(420.0, 560.0)).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, p: &mut PrPanel| {
            if !theme(ui) {
                return;
            }
            p.ui(ui);
        },
        panel,
    );
    settle(&mut h, |p| p.is_loading());
    h.get_by_label("새 PR").click();
    h.run_steps(1);
    settle(&mut h, |p| p.is_loading());
    assert!(h.query_all_by_value("Add diff view").next().is_some(), "title prefilled from commits");
    h.snapshot("pr_create_form");

    h.get_by_label("초안으로 만들기").click();
    h.run_steps(1);
    h.get_by_label("초안 PR 만들기").click();
    h.run_steps(1);
    settle(&mut h, |p| p.is_loading());
    assert!(backend.calls().iter().any(|c| c == "create Add diff view -> main draft=true"), "{:?}", backend.calls());
}

fn pr_view_harness(backend: Arc<FakeBackend>, size: egui::Vec2) -> Harness<'static, PrView> {
    let mut v = PrView::with_backend(PathBuf::from("."), 14507, backend);
    v.set_now(1_790_300_000);
    Harness::builder().with_size(size).with_pixels_per_point(1.5).wgpu().build_ui_state(
        |ui, v: &mut PrView| {
            if !theme(ui) {
                return;
            }
            v.ui(ui);
        },
        v,
    )
}

#[test]
fn pr_view_renders_conversation_checks_and_files() {
    let backend = Arc::new(FakeBackend::default());
    let mut h = pr_view_harness(backend.clone(), vec2(900.0, 900.0));
    settle(&mut h, |v| v.is_loading());
    assert_eq!(h.state().detail().unwrap().number, 14507);
    h.snapshot("pr_view_conversation");

    h.state_mut().set_tab(PrTab::Checks);
    h.run_steps(2);
    h.snapshot("pr_view_checks");

    h.get_by_label_contains("변경된 파일").click();
    h.run_steps(1);
    settle(&mut h, |v| v.is_loading());
    assert!(backend.calls().contains(&"diff 14507".to_string()));
    h.snapshot("pr_view_files");
}

#[test]
fn pr_view_review_and_merge_actions_call_backend() {
    let backend = Arc::new(FakeBackend::default());
    let mut h = pr_view_harness(backend.clone(), vec2(900.0, 1400.0));
    settle(&mut h, |v| v.is_loading());

    h.get_by_label("승인").scroll_to_me();
    h.run_steps(3);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
    h.run_steps(1);
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text("Looks good");
    h.run_steps(1);
    h.get_by_label("승인").click();
    h.run_steps(1);
    settle(&mut h, |v| v.is_loading());
    assert!(backend.calls().iter().any(|c| c == "review 14507 Approve Looks good"), "{:?}", backend.calls());

    h.get_by_label("병합…").click();
    h.run_steps(2);
    h.get_by_label("리베이스 후 병합").click();
    h.run_steps(3);
    h.snapshot("pr_view_merge_dialog");
    h.get_by_label("병합 확인").click();
    h.run_steps(1);
    settle(&mut h, |v| v.is_loading());
    assert!(backend.calls().iter().any(|c| c == "merge 14507 Rebase delete=true"), "{:?}", backend.calls());

    h.get_by_label_contains("체크아웃").click();
    h.run_steps(1);
    settle(&mut h, |v| v.is_loading());
    assert!(backend.calls().contains(&"checkout 14507".to_string()));
}

#[test]
fn pr_draft_reopens_without_overwriting_restored_fields_or_submitting() {
    let backend = Arc::new(FakeBackend::default());
    let draft = kiln_git::PrCreationDraft {
        request: PrCreate { title: "Saved title".into(), body: "Saved body".into(), base: "release".into(), draft: true, ..Default::default() },
        head: "feature/saved".into(), bases: vec!["main".into(), "release".into()],
    };
    let mut panel = PrPanel::with_backend(backend.clone()); panel.restore_creation_draft(&draft);
    let mut h = Harness::builder().with_size(vec2(460.0, 720.0)).build_ui_state(
        |ui, p: &mut PrPanel| { if theme(ui) { p.ui(ui); } }, panel);
    settle(&mut h, |p| p.is_loading());
    assert_eq!(h.state().creation_draft(), Some(draft.clone()));
    h.get_by_label("초안 보관하고 닫기").click(); h.run_steps(2);
    h.get_by_label("초안 이어 쓰기").click(); h.run_steps(2);
    assert_eq!(h.state().creation_draft(), Some(draft));
    assert!(!backend.calls().iter().any(|c| c.starts_with("create ")));
    h.get_by_label("초안 PR 만들기").click(); h.run_steps(1);
    settle(&mut h, |p| p.is_loading());
    assert!(h.state().creation_draft().is_none(), "only confirmed success clears the draft");
    assert!(h.query_by_label("새 PR").is_some(), "the form can be opened after successful submission");
}

#[test]
fn failed_pr_submission_keeps_the_complete_draft() {
    let backend = Arc::new(FakeBackend::default());
    backend.fail_create.store(true, std::sync::atomic::Ordering::SeqCst);
    let draft = kiln_git::PrCreationDraft { request: PrCreate { title: "Keep me".into(), body: "Body".into(), base: "main".into(), ..Default::default() }, head: "feature".into(), bases: vec!["main".into()] };
    let mut panel = PrPanel::with_backend(backend.clone()); panel.restore_creation_draft(&draft);
    let mut h = Harness::builder().with_size(vec2(460.0, 720.0)).build_ui_state(|ui, p: &mut PrPanel| { if theme(ui) { p.ui(ui); } }, panel);
    settle(&mut h, |p| p.is_loading());
    h.get_by_label("풀 리퀘스트 만들기").click(); h.run_steps(1); settle(&mut h, |p| p.is_loading());
    assert_eq!(h.state().creation_draft(), Some(draft));
    assert!(h.query_by_label("풀 리퀘스트를 만들 수 없습니다").is_some());
    assert_eq!(backend.calls().iter().filter(|c| c.starts_with("create ")).count(), 1);
}

#[test]
fn pr_creation_long_branches_fit_360_points() {
    let backend=Arc::new(FakeBackend::default());
    let long="feature/결제오류-recovery-long-branch/".repeat(5);
    let draft=kiln_git::PrCreationDraft { request:PrCreate {title:long.clone(),base:long.clone(),..Default::default()},head:long.clone(),bases:vec![long] };
    let mut panel=PrPanel::with_backend(backend);panel.restore_creation_draft(&draft);
    let mut h=Harness::builder().with_size(vec2(360.0,720.0)).wgpu().build_ui_state(|ui,p:&mut PrPanel| {if theme(ui){p.ui(ui);}},panel);
    settle(&mut h,|p|p.is_loading());
    h.render().unwrap().save("/tmp/kiln-pr-create-360.png").unwrap();
    for label in ["초안 버리기…","초안 보관하고 닫기","풀 리퀘스트 만들기"] {
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}: {:?}",h.get_by_label(label).rect());
    }
    for input in h.query_all_by_role(egui::accesskit::Role::TextInput) {
        assert!(h.ctx.content_rect().contains_rect(input.rect()),"input overflow {:?}",input.rect());
    }
}

#[test]
fn same_repository_scoped_commit_inputs_keep_undo_and_drafts_separate() {
    let r=Repo::new();r.write("README.md","# Fixture\n");r.commit_all("Initial fixture");
    let head=r.git(&["rev-parse","HEAD"]);
    struct Workspaces {panels:[GitPanel;2],active:usize}
    let mut h=Harness::builder().with_size(vec2(440.,850.)).build_ui_state(|ui,s:&mut Workspaces|{
        if !theme(ui){return;}
        ui.horizontal(|ui|{if ui.button("First workspace").clicked(){s.active=0;}if ui.button("Second workspace").clicked(){s.active=1;}});
        s.panels[s.active].ui(ui);
    },Workspaces{panels:[GitPanel::new(r.path.clone()).with_id_salt(100),GitPanel::new(r.path.clone()).with_id_salt(200)],active:0});
    settle(&mut h,|s|s.panels[s.active].is_busy());
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();h.event(egui::Event::Text("API review draft".into()));h.run_steps(3);
    h.get_by_label("Second workspace").click();settle(&mut h,|s|s.panels[s.active].is_busy());
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();h.event(egui::Event::Text("Documentation draft".into()));h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND,egui::Key::Z);h.run_steps(3);
    assert_eq!(h.state().panels[0].commit_draft(),"API review draft");
    assert!(!h.state().panels[1].commit_draft().contains("API review"));
    h.get_by_label("First workspace").click();h.run_steps(3);
    assert_eq!(h.state().panels[0].commit_draft(),"API review draft");
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();h.key_press_modifiers(egui::Modifiers::COMMAND,egui::Key::Z);h.run_steps(3);
    assert!(!h.state().panels[0].commit_draft().contains("Documentation"));
    assert_eq!(r.git(&["rev-parse","HEAD"]),head,"typing and undo must not submit commits");
}
