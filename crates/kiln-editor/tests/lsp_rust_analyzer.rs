//! 실제 rust-analyzer 로 hover·definition 을 확인한다. 설치되어 있어야 하며 기본으로는 건너뛴다.
//! `cargo test -p kiln-editor --test lsp_rust_analyzer -- --ignored` 로 실행한다.

use std::time::{Duration, Instant};

use kiln_editor::lsp::{LspConfig, LspManager, Position};

#[test]
#[ignore]
fn rust_analyzer_hover_and_definition() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"ra_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "fn helper(x: i32) -> i32 {\n    x + 1\n}\n\nfn main() {\n    let 값 = helper(2);\n    println!(\"{값}\");\n}\n";
    let main = root.join("src/main.rs");
    std::fs::write(&main, src).unwrap();

    let m = LspManager::with_config(root.clone(), LspConfig::defaults());
    assert!(m.open_document(&main, src), "rust-analyzer 를 찾지 못했습니다");
    let start = Instant::now();
    // 인덱싱이 끝나 정의 요청이 결과를 돌려줄 때까지 기다린다.
    // `값` 은 UTF-16 한 단위라 `helper` 는 5줄 12열에서 시작한다.
    let mut def = Vec::new();
    while start.elapsed() < Duration::from_secs(120) {
        if m.is_ready(&main)
            && let Some(Ok(d)) = m.definition(&main, Position::new(5, 15)).wait(Duration::from_secs(20))
            && !d.is_empty()
        {
            def = d;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("rust-analyzer ready+definition after {:.1}s, status {:?}", start.elapsed().as_secs_f32(), m.status_text());
    assert_eq!(def.len(), 1, "정의를 찾지 못했습니다");
    assert_eq!(def[0].path, main);
    assert_eq!(def[0].range.start, Position::new(0, 3));

    // 인덱싱 도중에는 hover 가 비어 있을 수 있어 내용이 올 때까지 다시 묻는다.
    let mut hover = String::new();
    while start.elapsed() < Duration::from_secs(120) {
        if let Some(Ok(Some(h))) = m.hover(&main, Position::new(5, 15)).wait(Duration::from_secs(20)) {
            hover = h;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("hover after {:.1}s", start.elapsed().as_secs_f32());
    println!("hover: {hover}");
    assert!(hover.contains("fn helper(x: i32) -> i32"), "{hover}");
    m.shutdown();
}

/// 편집기에 붙인 rust-analyzer: 저장하면 타입 오류 진단이 편집기에 들어온다.
#[test]
#[ignore]
fn rust_analyzer_through_editor_reports_diagnostics() {
    use kiln_editor::{Editor, Pos, Selection};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"ra_editor\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let main = root.join("src/main.rs");
    std::fs::write(&main, "fn main() {\n    let s: String = 1;\n    println!(\"{s}\");\n}\n").unwrap();
    let m = LspManager::with_config(root.clone(), LspConfig::defaults());
    let mut ed = Editor::open_with_lsp(&main, Some(m.clone())).unwrap();
    assert!(ed.lsp_attached());
    let start = Instant::now();
    while !m.is_ready(&main) && start.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(100));
    }
    ed.set_selection(Selection::caret(Pos::new(3, 1)));
    ed.save().unwrap();
    while start.elapsed() < Duration::from_secs(120) {
        ed.poll_lsp();
        if ed.diagnostics().iter().any(|d| d.severity == kiln_editor::lsp::Severity::Error) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let diags = ed.diagnostics();
    println!("diagnostics after {:.1}s: {:?}", start.elapsed().as_secs_f32(), diags.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(diags.iter().any(|d| d.range.start.line == 1), "{diags:?}");
    m.shutdown();
}
