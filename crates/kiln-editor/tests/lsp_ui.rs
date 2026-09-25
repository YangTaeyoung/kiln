//! 문제(진단) 패널 UI 테스트와 스냅샷.

mod common;

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_editor::lsp::{Diagnostic, LspConfig, LspManager, Position, Range, Severity};
use kiln_editor::{EditorEvent, diagnostics_ui};

struct State {
    lsp: LspManager,
    events: Vec<EditorEvent>,
}

fn diag(line: u32, a: u32, b: u32, severity: Severity, message: &str, source: Option<&str>, code: Option<&str>) -> Diagnostic {
    Diagnostic {
        range: Range { start: Position::new(line, a), end: Position::new(line, b) },
        severity,
        message: message.into(),
        source: source.map(str::to_owned),
        code: code.map(str::to_owned),
    }
}

fn fixture(root: &Path) -> LspManager {
    let lsp = LspManager::with_config(root.to_path_buf(), LspConfig::default());
    let main = root.join("src/main.rs");
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    std::fs::write(&main, "fn main() {\n    let 값 = undefined_fn();\n}\n").unwrap();
    lsp.inject_diagnostics(main, vec![
        diag(1, 12, 24, Severity::Error, "cannot find function `undefined_fn` in this scope", Some("rustc"), Some("E0425")),
        diag(1, 8, 9, Severity::Warning, "unused variable: `값`", Some("rustc"), None),
        diag(0, 3, 7, Severity::Hint, "consider adding a doc comment", Some("clippy"), None),
    ]);
    lsp.inject_diagnostics(root.join("src/lib/config.rs"), vec![diag(
        9,
        0,
        4,
        Severity::Information,
        "구성 파일을 찾지 못해 기본값을 사용합니다",
        None,
        None,
    )]);
    lsp.inject_diagnostics(root.join("web/app.ts"), vec![diag(
        2,
        6,
        11,
        Severity::Error,
        "Type 'string' is not assignable to type 'number'.",
        Some("ts"),
        Some("2322"),
    )]);
    lsp
}

fn harness(lsp: LspManager) -> Harness<'static, State> {
    let h = Harness::builder().with_size(egui::vec2(460.0, 260.0)).wgpu().build_ui_state(
        |ui, s: &mut State| {
            let ev = diagnostics_ui(ui, &s.lsp);
            s.events.extend(ev);
        },
        State { lsp, events: Vec::new() },
    );
    common::apply_theme(&h.ctx);
    h
}

#[test]
fn clicking_a_diagnostic_opens_it_with_char_column() {
    let dir = tempfile::tempdir().unwrap();
    let root: PathBuf = dir.path().to_path_buf();
    let mut h = harness(fixture(&root));
    h.run();
    h.get_by_label("cannot find function `undefined_fn` in this scope").click();
    h.run();
    // `값` 뒤의 UTF-16 12열은 문자 12열(1부터 13)이다.
    assert_eq!(h.state().events, vec![EditorEvent::OpenAt { path: root.join("src/main.rs"), line: 2, col: 13 }]);

    // 파일 행을 누르면 접히고 그 파일의 진단이 사라진다.
    let file_label = root.join("src/main.rs").display().to_string();
    h.get_by_label(&file_label).click();
    h.run();
    assert!(h.query_by_label("cannot find function `undefined_fn` in this scope").is_none());
    h.get_by_label(&file_label).click();
    h.run();
    assert!(h.query_by_label("cannot find function `undefined_fn` in this scope").is_some());
}

#[test]
fn snapshot_diagnostics_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(fixture(dir.path()));
    h.run();
    h.snapshot("diagnostics_panel");
}

#[test]
fn snapshot_diagnostics_panel_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(LspManager::with_config(dir.path().to_path_buf(), LspConfig::default()));
    h.run();
    h.snapshot("diagnostics_panel_empty");
}
