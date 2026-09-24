//! FileTree 위젯 UI 테스트와 스냅샷.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::{Decoration, EditorEvent, FileTree};

struct State {
    tree: FileTree,
    events: Vec<EditorEvent>,
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn path(&self) -> &Path {
        &self.root
    }
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kiln");
    let r = root.as_path();
    for dir in ["src/editor", "src/bin", "assets", "target/debug", "docs"] {
        std::fs::create_dir_all(r.join(dir)).unwrap();
    }
    std::fs::write(r.join(".gitignore"), "/target\n*.log\n").unwrap();
    for f in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "build.log",
        "Dockerfile",
        "src/main.rs",
        "src/lib.rs",
        "src/editor/mod.rs",
        "src/editor/view.ts",
        "src/bin/tool.py",
        "assets/logo.svg",
        "assets/app.json",
        "docs/guide.md",
        "target/debug/kiln",
    ] {
        std::fs::write(r.join(f), "x").unwrap();
    }
    Fixture { _tmp: tmp, root }
}

fn harness(root: &Path) -> Harness<'static, State> {
    let mut tree = FileTree::new(root.to_path_buf());
    tree.set_use_trash(false);
    let h = Harness::builder().with_size(egui::vec2(300.0, 440.0)).wgpu().build_ui_state(
        |ui, s: &mut State| {
            let ev = s.tree.ui(ui);
            s.events.extend(ev);
        },
        State { tree, events: Vec::new() },
    );
    common::apply_theme(&h.ctx);
    h
}

fn take_events(h: &mut Harness<'static, State>) -> Vec<EditorEvent> {
    std::mem::take(&mut h.state_mut().events)
}

#[test]
fn clicking_expands_dirs_and_opens_files() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    assert!(h.query_by_label("target").is_none(), "gitignored dir hidden");
    assert!(h.query_by_label("build.log").is_none());
    h.get_by_label("src").click();
    h.run();
    assert!(h.state().tree.is_expanded(&root.join("src")));
    h.get_by_label("main.rs").click();
    h.run();
    assert_eq!(take_events(&mut h), vec![EditorEvent::OpenFile(root.join("src/main.rs"))]);
}

#[test]
fn keyboard_navigation() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("assets").click();
    h.run();
    // assets 가 펼쳐진 상태: 오른쪽 → 첫 자식 선택, 아래 → 다음 자식, Enter → 열기
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(h.state().tree.selected(), Some(root.join("assets/app.json").as_path()));
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(take_events(&mut h), vec![EditorEvent::OpenFile(root.join("assets/logo.svg"))]);
    // 왼쪽 → 부모 선택, 다시 왼쪽 → 접기
    h.key_press(Key::ArrowLeft);
    h.run();
    assert_eq!(h.state().tree.selected(), Some(root.join("assets").as_path()));
    h.key_press(Key::ArrowLeft);
    h.run();
    assert!(!h.state().tree.is_expanded(&root.join("assets")));
}

#[test]
fn rename_with_f2_emits_event_and_moves_file() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("README.md").click();
    h.run();
    take_events(&mut h);
    h.key_press(Key::F2);
    h.run();
    // 확장자를 뺀 이름 부분이 선택된 상태에서 입력한다.
    for c in "INTRO".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(
        take_events(&mut h),
        vec![EditorEvent::FileRenamed { from: root.join("README.md"), to: root.join("INTRO.md") }]
    );
    assert!(root.join("INTRO.md").exists());
    assert!(!root.join("README.md").exists());
}

#[test]
fn new_file_from_header_button() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("docs").click();
    h.run();
    take_events(&mut h);
    h.hover_at(egui::pos2(150.0, 15.0));
    h.run();
    h.get_by_label("New File…").click();
    h.run();
    for c in "notes.md".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!(root.join("docs/notes.md").exists());
    assert_eq!(take_events(&mut h), vec![EditorEvent::OpenFile(root.join("docs/notes.md"))]);
    assert!(h.query_by_label("notes.md").is_some());
}

#[test]
fn delete_asks_for_confirmation() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("Dockerfile").click();
    h.run();
    take_events(&mut h);
    h.key_press(Key::Delete);
    h.run();
    assert!(root.join("Dockerfile").exists(), "not deleted before confirming");
    h.get_by_label("Delete").click();
    h.run();
    assert!(!root.join("Dockerfile").exists());
    assert_eq!(take_events(&mut h), vec![EditorEvent::FileDeleted(root.join("Dockerfile"))]);
}

#[test]
fn cmd_backspace_then_cancel_keeps_file() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("Cargo.toml").click();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Backspace);
    h.run();
    h.get_by_label("Cancel").click();
    h.run();
    assert!(root.join("Cargo.toml").exists());
}

#[test]
fn watcher_picks_up_new_files_in_expanded_dirs() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("src").click();
    h.run();
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(root.join("src/fresh.rs"), "").unwrap();
    std::fs::write(root.join("target/debug/noise.o"), "").unwrap();
    let ok = common::wait_until(&mut h, 5.0, |h| h.query_by_label("fresh.rs").is_some());
    assert!(ok, "new file should appear via file watcher");
    let reloaded: Vec<PathBuf> = h.state().tree.last_reloaded().to_vec();
    assert!(reloaded.iter().all(|p| p == &root || p == &root.join("src")), "{reloaded:?}");
}

#[test]
fn snapshot_file_tree() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    let mut deco = HashMap::new();
    deco.insert(root.join("src/editor/view.ts"), Decoration { color: Color32::from_rgb(0xe5, 0xc0, 0x7b), badge: Some('M') });
    deco.insert(root.join("src/bin/tool.py"), Decoration { color: Color32::from_rgb(0x7e, 0xc6, 0x7a), badge: Some('U') });
    deco.insert(root.join("Cargo.toml"), Decoration { color: Color32::from_rgb(0xe5, 0xc0, 0x7b), badge: Some('M') });
    h.state_mut().tree.set_decorations(deco);
    h.state_mut().tree.reveal(&root.join("src/editor/view.ts"));
    h.state_mut().tree.set_expanded(&root.join("src/bin"), true);
    h.run();
    h.get_by_label("view.ts").click();
    h.run();
    h.hover_at(egui::pos2(120.0, 230.0));
    h.run();
    h.snapshot("file_tree");
}

#[test]
fn snapshot_file_tree_show_ignored() {
    let d = fixture();
    let root = d.path().to_path_buf();
    let mut h = harness(&root);
    h.state_mut().tree.set_show_ignored(true);
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("file_tree_ignored");
}
