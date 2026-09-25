//! QuickOpen 모달 UI 테스트와 스냅샷.

mod common;

use std::path::{Path, PathBuf};

use egui::{Event, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::QuickOpen;

struct State {
    qo: QuickOpen,
    picked: Option<PathBuf>,
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    for f in [
        "src/main.rs",
        "src/lib.rs",
        "src/editor/mod.rs",
        "src/editor/view.rs",
        "src/editor/find.rs",
        "src/file_tree.rs",
        "src/quick_open.rs",
        "src/search_panel.rs",
        "docs/maintenance.md",
        "tests/main_window_test.rs",
        "Cargo.toml",
        "README.md",
        "web/src/App.tsx",
        "web/src/main.ts",
        "web/package.json",
        "target/debug/main.d",
    ] {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    (tmp, root)
}

fn harness(root: &Path) -> Harness<'static, State> {
    let mut qo = QuickOpen::new();
    qo.open(root);
    let h = Harness::builder().with_size(egui::vec2(760.0, 480.0)).wgpu().build_ui_state(
        |ui, s: &mut State| {
            let t = kiln_common::Theme::current();
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
            if let Some(p) = s.qo.ui(ui.ctx()) {
                s.picked = Some(p);
            }
        },
        State { qo, picked: None },
    );
    common::apply_theme(&h.ctx);
    h
}

fn type_text(h: &mut Harness<'static, State>, s: &str) {
    for c in s.chars() {
        h.event(Event::Text(c.to_string()));
    }
}

fn wait_indexed(h: &mut Harness<'static, State>) {
    assert!(common::wait_until(h, 5.0, |h| !h.state().qo.is_indexing() && h.state().qo.file_count() > 0));
}

#[test]
fn fuzzy_query_and_enter_opens_best_match() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    wait_indexed(&mut h);
    assert_eq!(h.state().qo.file_count(), 16, "gitignored target/ is excluded");
    type_text(&mut h, "main");
    assert!(common::wait_until(&mut h, 5.0, |h| h.state().qo.result_paths().first().map(String::as_str) == Some("src/main.rs")));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().picked.as_deref(), Some(root.join("src/main.rs").as_path()));
    assert!(!h.state().qo.is_open());
}

#[test]
fn arrow_keys_move_selection_and_escape_closes() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    wait_indexed(&mut h);
    type_text(&mut h, "editor");
    // 빈 검색어의 전체 목록(16개)이 아니라 걸러진 결과가 올 때까지 기다린다.
    assert!(common::wait_until(&mut h, 5.0, |h| (3..16).contains(&h.state().qo.result_paths().len())));
    let results = h.state().qo.result_paths();
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::ArrowUp);
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().picked.as_deref(), Some(root.join(&results[1]).as_path()));
    h.state_mut().qo.open(&root);
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().qo.is_open());
    assert!(h.state().picked.is_some());
}

#[test]
fn clicking_a_result_opens_it() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    wait_indexed(&mut h);
    type_text(&mut h, "app");
    assert!(common::wait_until(&mut h, 5.0, |h| {
        let r = h.state().qo.result_paths();
        r.len() < 16 && r.iter().any(|p| p == "web/src/App.tsx")
    }));
    h.get_by_label("web/src/App.tsx").click();
    h.run();
    assert_eq!(h.state().picked.as_deref(), Some(root.join("web/src/App.tsx").as_path()));
}

#[test]
fn recently_opened_files_come_first_for_empty_query() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    wait_indexed(&mut h);
    h.state_mut().qo.note_recent(&root.join("web/src/main.ts"));
    h.state_mut().qo.open(&root);
    assert!(common::wait_until(&mut h, 5.0, |h| h.state().qo.result_paths().first().map(String::as_str) == Some("web/src/main.ts")));
}

#[test]
fn snapshot_quick_open() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    wait_indexed(&mut h);
    type_text(&mut h, "srced");
    assert!(common::wait_until(&mut h, 5.0, |h| h.state().qo.result_paths().len() == 3));
    h.key_press(Key::ArrowDown);
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("quick_open");
}
