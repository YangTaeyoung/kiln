//! SearchPanel UI 테스트와 스냅샷.

mod common;

use std::path::{Path, PathBuf};

use egui::Event;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::{EditorEvent, SearchPanel};

struct State {
    panel: SearchPanel,
    events: Vec<EditorEvent>,
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    let files: &[(&str, &str)] = &[
        ("src/main.rs", "fn main() {\n    let config = Config::load();\n    run(config);\n}\n"),
        ("src/config.rs", "pub struct Config {\n    pub name: String,\n}\n\nimpl Config {\n    pub fn load() -> Config {\n        Config { name: \"kiln\".into() }\n    }\n}\n"),
        ("README.md", "# Kiln\n\nConfiguration lives in `config.toml`.\n"),
        ("target/out.rs", "Config Config Config\n"),
    ];
    for (f, body) in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    (tmp, root)
}

fn harness(root: &Path) -> Harness<'static, State> {
    let panel = SearchPanel::new(root.to_path_buf());
    let h = Harness::builder().with_size(egui::vec2(340.0, 420.0)).wgpu().build_ui_state(
        |ui, s: &mut State| {
            let ev = s.panel.ui(ui);
            s.events.extend(ev);
        },
        State { panel, events: Vec::new() },
    );
    common::apply_theme(&h.ctx);
    h
}

fn type_query(h: &mut Harness<'static, State>, q: &str) {
    h.state_mut().panel.focus();
    h.run();
    for c in q.chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
}

fn wait_done(h: &mut Harness<'static, State>) {
    assert!(common::wait_until(h, 5.0, |h| !h.state().panel.is_searching() && h.state().panel.summary().is_some()));
}

#[test]
fn typing_searches_after_debounce_and_click_opens_location() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "config");
    wait_done(&mut h);
    let s = h.state().panel.summary().unwrap();
    assert_eq!(s.files, 3, "target/ ignored");
    assert_eq!(s.matches, 3 + 4 + 2);
    h.get_by_label("src/main.rs:2").click();
    h.run();
    assert_eq!(
        std::mem::take(&mut h.state_mut().events),
        vec![EditorEvent::OpenAt { path: root.join("src/main.rs"), line: 2, col: 9 }]
    );
}

#[test]
fn toggles_change_results() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "Config");
    wait_done(&mut h);
    let all = h.state().panel.summary().unwrap().matches;
    h.get_by_label("대/소문자 구분").click();
    h.run();
    wait_done(&mut h);
    let cs = h.state().panel.summary().unwrap().matches;
    assert!(cs < all);
    h.get_by_label("단어 단위로").click();
    h.run();
    wait_done(&mut h);
    assert_eq!(h.state().panel.summary().unwrap().matches, 5);
}

#[test]
fn clicking_file_header_collapses_its_matches() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "config");
    wait_done(&mut h);
    assert!(h.query_by_label("src/config.rs:1").is_some());
    h.get_by_label("src/config.rs").click();
    h.run();
    assert!(h.query_by_label("src/config.rs:1").is_none());
}

#[test]
fn replace_all_after_confirmation() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "Config");
    h.get_by_label("대/소문자 구분").click();
    h.get_by_label("단어 단위로").click();
    h.run();
    wait_done(&mut h);
    h.get_by_label("바꾸기 전환").click();
    h.run();
    let rid = egui::Id::new(("kiln-search-panel", root.clone())).with("replace");
    h.ctx.memory_mut(|m| m.request_focus(rid));
    h.run();
    for c in "Settings".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.get_by_label("모두 바꾸기").click();
    h.run();
    h.get_by_label("바꾸기").click();
    assert!(common::wait_until(&mut h, 5.0, |_| std::fs::read_to_string(root.join("src/main.rs")).unwrap().contains("Settings::load")));
    let cfg = std::fs::read_to_string(root.join("src/config.rs")).unwrap();
    assert!(cfg.contains("pub struct Settings") && !cfg.contains("Config"));
    assert!(std::fs::read_to_string(root.join("target/out.rs")).unwrap().contains("Config"), "ignored files untouched");
}

#[test]
fn invalid_regex_shows_error() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    h.get_by_label("정규식 사용").click();
    h.run();
    type_query(&mut h, "(unclosed");
    assert!(common::wait_until(&mut h, 5.0, |h| !h.state().panel.is_searching() && h.state().panel.error().is_some()));
    assert!(h.state().panel.results().is_empty());
    h.remove_cursor();
    h.run();
    h.snapshot("search_panel_error");
}

#[test]
fn snapshot_search_panel() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "config");
    wait_done(&mut h);
    h.get_by_label("src/main.rs:2").click();
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("search_panel");
}

#[test]
fn snapshot_search_panel_replace_preview() {
    let (_t, root) = fixture();
    let mut h = harness(&root);
    h.run();
    type_query(&mut h, "Config");
    wait_done(&mut h);
    h.get_by_label("바꾸기 전환").click();
    h.get_by_label("검색 세부 정보 전환").click();
    h.run();
    let rid = egui::Id::new(("kiln-search-panel", root.clone())).with("replace");
    h.ctx.memory_mut(|m| m.request_focus(rid));
    h.run();
    for c in "Settings".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("search_panel_replace");
}
