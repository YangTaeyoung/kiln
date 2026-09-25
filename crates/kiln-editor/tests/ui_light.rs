//! 밝은 테마(kiln-light)에서 주요 편집 패널 스냅샷. 테마는 전역 값이라 한 테스트에서 차례로 그린다.

mod common;

use std::collections::HashMap;
use std::path::Path;

use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::lsp::{Diagnostic, LspConfig, LspManager, Position, Range, Severity};
use kiln_editor::{Decoration, Editor, FileTree, QuickOpen, SearchPanel, diagnostics_ui};

const SAMPLE: &str = r#"use std::collections::HashMap;

/// Counts word frequencies in `text`.
pub fn word_counts(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        // 소문자로 정규화
        let key = word.to_lowercase();
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Fast = 1,
    Careful,
}
"#;

fn harness<S: 'static>(size: egui::Vec2, state: S, mut f: impl FnMut(&mut egui::Ui, &mut S) + 'static) -> Harness<'static, S> {
    let h = Harness::builder().with_size(size).wgpu().build_ui_state(
        move |ui, s: &mut S| {
            if !common::fonts_ready(ui.ctx()) {
                return;
            }
            f(ui, s);
        },
        state,
    );
    common::apply_theme(&h.ctx);
    h
}

fn project(root: &Path) {
    let files: &[(&str, &str)] = &[
        ("src/main.rs", "fn main() {\n    let config = Config::load();\n    run(config);\n}\n"),
        ("src/config.rs", "pub struct Config {\n    pub name: String,\n}\n\nimpl Config {\n    pub fn load() -> Config {\n        Config { name: \"kiln\".into() }\n    }\n}\n"),
        ("src/editor/view.ts", "export const view = 1;\n"),
        ("src/editor/mod.rs", "pub mod view;\n"),
        ("README.md", "# Kiln\n\nConfiguration lives in `config.toml`.\n"),
        ("Cargo.toml", "[package]\nname = \"kiln\"\n"),
        ("docs/guide.md", "guide\n"),
        ("assets/app.json", "{}\n"),
    ];
    for (f, body) in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
}

fn diag(line: u32, a: u32, b: u32, severity: Severity, message: &str, source: Option<&str>) -> Diagnostic {
    Diagnostic {
        range: Range { start: Position::new(line, a), end: Position::new(line, b) },
        severity,
        message: message.into(),
        source: source.map(str::to_owned),
        code: None,
    }
}

#[test]
fn light_theme_snapshots() {
    kiln_common::Theme::set_current("kiln-light");
    let mut results = egui_kittest::SnapshotResults::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("kiln");
    project(&root);

    // 파일 탐색기
    let mut tree = FileTree::new(root.clone());
    tree.set_expanded(&root.join("src"), true);
    tree.set_expanded(&root.join("src/editor"), true);
    let t = kiln_common::Theme::current();
    let mut deco = HashMap::new();
    deco.insert(root.join("src/editor/view.ts"), Decoration { color: t.yellow, badge: Some('M') });
    deco.insert(root.join("Cargo.toml"), Decoration { color: t.yellow, badge: Some('M') });
    deco.insert(root.join("docs/guide.md"), Decoration { color: t.green, badge: Some('U') });
    tree.set_decorations(deco);
    let mut h = harness(egui::vec2(300.0, 380.0), tree, |ui, tree: &mut FileTree| {
        tree.ui(ui);
    });
    h.run();
    h.get_by_label("view.ts").click();
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("light_file_tree");
    results.extend_harness(&mut h);

    // 프로젝트 검색
    let mut panel = SearchPanel::new(root.clone());
    panel.focus();
    let mut h = harness(egui::vec2(340.0, 380.0), panel, |ui, p: &mut SearchPanel| {
        p.ui(ui);
    });
    h.run();
    h.state_mut().focus();
    h.run();
    for c in "config".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    assert!(common::wait_until(&mut h, 5.0, |h| !h.state().is_searching() && h.state().summary().is_some()));
    h.get_by_label("src/main.rs:2").click();
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("light_search_panel");
    results.extend_harness(&mut h);

    // 빠른 열기
    let mut qo = QuickOpen::new();
    qo.open(&root);
    let mut h = harness(egui::vec2(700.0, 360.0), qo, |ui, qo: &mut QuickOpen| {
        let t = kiln_common::Theme::current();
        ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
        qo.ui(ui.ctx());
    });
    assert!(common::wait_until(&mut h, 15.0, |h| h.state().result_paths().len() >= 5));
    for c in "src".chars() {
        h.event(Event::Text(c.to_string()));
    }
    assert!(common::wait_until(&mut h, 15.0, |h| h.state().result_paths().len() == 4));
    h.key_press(Key::ArrowDown);
    h.run();
    h.remove_cursor();
    h.run();
    h.snapshot("light_quick_open");
    results.extend_harness(&mut h);

    // 편집기 + 찾기 막대
    let mut ed = Editor::from_text("demo.rs", SAMPLE);
    ed.goto(5, 13);
    let mut h = harness(egui::vec2(820.0, 420.0), ed, |ui, ed: &mut Editor| {
        ed.ui(ui);
    });
    h.run();
    let id = h.state().id();
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::F);
    h.run();
    for c in "count".chars() {
        h.event(Event::Text(c.to_string()));
    }
    h.run();
    h.snapshot("light_editor_find");
    results.extend_harness(&mut h);

    // 문제 패널
    let lsp = LspManager::with_config(root.clone(), LspConfig::default());
    lsp.inject_diagnostics(root.join("src/main.rs"), vec![
        diag(1, 8, 14, Severity::Error, "cannot find type `Config` in this scope", Some("rustc")),
        diag(1, 8, 14, Severity::Warning, "unused variable: `config`", Some("rustc")),
    ]);
    lsp.inject_diagnostics(root.join("src/config.rs"), vec![diag(0, 0, 3, Severity::Information, "구성 파일을 찾지 못해 기본값을 사용합니다", None)]);
    let mut h = harness(egui::vec2(460.0, 220.0), lsp, |ui, lsp: &mut LspManager| {
        diagnostics_ui(ui, lsp);
    });
    h.run();
    h.snapshot("light_diagnostics_panel");
    results.extend_harness(&mut h);

    kiln_common::Theme::set_current("kiln-dark");
}
