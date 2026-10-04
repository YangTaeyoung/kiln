//! 가짜 언어 서버(`kiln-fake-lsp`)로 LspManager 전체 경로를 검사한다.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kiln_editor::lsp::{
    LspConfig, LspManager, Position, Range, ServerSpec, Severity, TextChange, apply_text_edits,
};

const WAIT: Duration = Duration::from_secs(10);

fn manager(root: &Path, args: &[&str]) -> LspManager {
    let mut cfg = LspConfig::default();
    cfg.languages.insert("rust".into(), ServerSpec::new(env!("CARGO_BIN_EXE_kiln-fake-lsp"), args));
    LspManager::with_config(root.to_path_buf(), cfg)
}

fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn pos(line: u32, character: u32) -> Position {
    Position::new(line, character)
}

const SRC: &str = "fn helper() {}\nfn main() {\n    let 값 = helper();\n    helper(); bad meh\n    external();\n}\n";

fn open(dir: &Path, name: &str, text: &str, args: &[&str]) -> (LspManager, PathBuf) {
    let m = manager(dir, args);
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    assert!(m.open_document(&p, text));
    assert!(wait_until(|| m.is_ready(&p)), "서버가 준비되지 않았습니다: {:?}", m.status_text());
    (m, p)
}

#[test]
fn diagnostics_hover_definition_references() {
    let dir = tempfile::tempdir().unwrap();
    let (m, p) = open(dir.path(), "a.rs", SRC, &[]);
    assert!(wait_until(|| m.diagnostics_for(&p).len() == 2));
    let d = m.diagnostics_for(&p);
    let bad = d.iter().find(|d| d.severity == Severity::Error).unwrap();
    assert_eq!(bad.range, Range { start: pos(3, 14), end: pos(3, 17) });
    assert_eq!(bad.source.as_deref(), Some("fake"));
    assert_eq!(bad.code.as_deref(), Some("3"));
    assert!(d.iter().any(|d| d.severity == Severity::Warning));
    assert_eq!(m.all_diagnostics().len(), 1);
    assert!(m.diagnostics_version() > 0);

    let hover = m.hover(&p, pos(2, 14)).wait(WAIT).unwrap().unwrap();
    assert_eq!(hover.as_deref(), Some("**hover** `helper`"));
    assert_eq!(m.hover(&p, pos(5, 1)).wait(WAIT).unwrap().unwrap(), None);

    let def = m.definition(&p, pos(2, 16)).wait(WAIT).unwrap().unwrap();
    assert_eq!(def.len(), 1);
    assert_eq!(def[0].path, p);
    assert_eq!(def[0].range, Range { start: pos(0, 3), end: pos(0, 9) });

    let ext = m.definition(&p, pos(4, 6)).wait(WAIT).unwrap().unwrap();
    assert_eq!(ext[0].path, dir.path().join("other.rs"));

    let refs = m.references(&p, pos(0, 4)).wait(WAIT).unwrap().unwrap();
    assert_eq!(refs.len(), 3);
    assert!(refs.iter().all(|r| r.path == p));
    // `값` 은 UTF-16 한 단위(3바이트)라 뒤쪽 `helper` 는 12열에서 시작한다.
    assert!(refs.iter().any(|r| r.range.start == pos(2, 12)));
}

#[test]
fn completion_items_and_signature_help() {
    let dir = tempfile::tempdir().unwrap();
    let (m, p) = open(dir.path(), "c.rs", "fn main() {\n    x.be\n    f(1, \n}\n", &[]);
    assert_eq!(m.completion_triggers(&p), vec!["."]);
    assert_eq!(m.signature_triggers(&p), vec!["(", ","]);
    let list = m.completion(&p, pos(1, 8), None).wait(WAIT).unwrap().unwrap();
    assert!(!list.is_incomplete);
    assert_eq!(list.items.len(), 4);
    let beta = list.items.iter().find(|i| i.label == "beta").unwrap();
    let edit = beta.edit.as_ref().unwrap();
    assert_eq!(edit.range, Range { start: pos(1, 6), end: pos(1, 8) });
    assert_eq!(edit.new_text, "beta_edit");
    let snip = list.items.iter().find(|i| i.label == "println!").unwrap();
    assert_eq!(snip.insert_text, "println!(\"msg\")");
    assert_eq!(snip.cursor_offset, Some(10));
    assert_eq!(list.items.iter().find(|i| i.label == "gamma").unwrap().sort_text, "zz");

    let triggered = m.completion(&p, pos(1, 6), Some(".".into())).wait(WAIT).unwrap().unwrap();
    assert_eq!(triggered.items.len(), 4);

    let sig = m.signature_help(&p, pos(2, 9)).wait(WAIT).unwrap().unwrap().unwrap();
    let (a, b) = sig.active_param.unwrap();
    assert_eq!(&sig.label[a..b], "b: i32");
}

fn change(a: (u32, u32), b: (u32, u32), text: &str) -> TextChange {
    TextChange { range: Some(Range { start: pos(a.0, a.1), end: pos(b.0, b.1) }), text: text.into() }
}

fn check_incremental_sync(args: &[&str]) {
    let dir = tempfile::tempdir().unwrap();
    let (m, p) = open(dir.path(), "u.rs", "😀한 x\n둘째 줄\n", args);
    assert!(wait_until(|| m.diagnostics_version() > 0));
    m.change_document(&p, &[change((0, 4), (0, 4), "bad ")]);
    m.change_document(&p, &[change((1, 0), (1, 2), "첫"), change((1, 2), (1, 3), "meh 😀")]);
    assert_eq!(m.document_text(&p).unwrap(), "😀한 bad x\n첫 meh 😀\n");
    assert_eq!(m.document_version(&p), Some(2));
    // 서버가 본 내용에서 계산한 진단 범위가 UTF-16 기준으로 맞아야 한다.
    assert!(wait_until(|| m.diagnostics_for(&p).len() == 2));
    let d = m.diagnostics_for(&p);
    let bad = d.iter().find(|d| d.severity == Severity::Error).unwrap();
    assert_eq!(bad.range, Range { start: pos(0, 4), end: pos(0, 7) });
    let meh = d.iter().find(|d| d.severity == Severity::Warning).unwrap();
    assert_eq!(meh.range, Range { start: pos(1, 2), end: pos(1, 5) });
    m.change_document(&p, &[TextChange { range: None, text: "meh\n".into() }]);
    assert!(wait_until(|| {
        let d = m.diagnostics_for(&p);
        d.len() == 1 && d[0].range.start == pos(0, 0)
    }));
}

#[test]
fn incremental_changes_with_korean_and_emoji() {
    check_incremental_sync(&[]);
}

#[test]
fn full_sync_server_gets_whole_text() {
    check_incremental_sync(&["--full-sync"]);
}

#[test]
fn rename_edits_open_doc_in_queue_and_closed_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("other.rs");
    std::fs::write(&other, "pub fn helper() {}\r\n// helper\r\n").unwrap();
    let (m, p) = open(dir.path(), "a.rs", SRC, &[]);
    let edit = m.rename(&p, pos(0, 5), "도우미").wait(WAIT).unwrap().unwrap();
    assert_eq!(edit.changes.len(), 2);
    m.apply_workspace_edit(&edit).unwrap();
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "pub fn 도우미() {}\r\n// 도우미\r\n");
    let queued = m.take_pending_edits(&p);
    assert_eq!(queued.len(), 3);
    let renamed = apply_text_edits(SRC, &queued);
    assert_eq!(renamed.matches("도우미").count(), 3);
    assert!(!renamed.contains("helper"));
    assert!(m.take_pending_edits(&p).is_empty());
}

#[test]
fn format_returns_trailing_whitespace_edits() {
    let dir = tempfile::tempdir().unwrap();
    let text = "fn a() {  \n\tx();\t\n}\n";
    let (m, p) = open(dir.path(), "f.rs", text, &[]);
    let edits = m.format(&p, 4, true).wait(WAIT).unwrap().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(apply_text_edits(text, &edits), "fn a() {\n\tx();\n}\n");
}

#[test]
fn progress_shows_in_status_until_end() {
    let dir = tempfile::tempdir().unwrap();
    let (m, p) = open(dir.path(), "a.rs", "fn a() {}\n", &[]);
    assert!(wait_until(|| m.status_text().as_deref() == Some("kiln-fake-lsp: 인덱싱 중 50%")), "{:?}", m.status_text());
    m.save_document(&p);
    assert!(wait_until(|| m.status_text().as_deref() == Some("kiln-fake-lsp")), "{:?}", m.status_text());
    m.close_document(&p);
    assert!(m.document_text(&p).is_none());
}

#[test]
fn crash_restarts_server_and_reopens_documents() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("crash-once");
    std::fs::write(&marker, "").unwrap();
    let marker_s = marker.to_string_lossy().into_owned();
    // Progress-token replies and hover are asynchronous: crash on the method, not an assumed message count.
    let (m, p) = open(dir.path(), "a.rs", "fn a() {}\n", &["--crash-on", "textDocument/hover", "--crash-marker", &marker_s]);
    let first = m.hover(&p, pos(0, 4)).wait(WAIT).unwrap();
    assert!(first.is_err(), "{first:?}");
    // 재시작 중에 바뀐 내용도 다시 열 때 반영되어야 한다.
    m.change_document(&p, &[change((0, 9), (0, 9), " // bad")]);
    assert!(wait_until(|| m.is_ready(&p)));
    assert!(wait_until(|| m.diagnostics_for(&p).len() == 1), "{:?}", m.diagnostics_for(&p));
    let hover = m.hover(&p, pos(0, 4)).wait(WAIT).unwrap().unwrap();
    assert_eq!(hover.as_deref(), Some("**hover** `a`"));
}

#[test]
fn repeated_crashes_mark_server_failed() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), &["--crash-after", "1"]);
    let p = dir.path().join("a.rs");
    assert!(m.open_document(&p, "fn a() {}"));
    assert!(wait_until(|| m.status_text().as_deref() == Some("kiln-fake-lsp: 실행 실패")), "{:?}", m.status_text());
    assert!(m.hover(&p, pos(0, 0)).wait(WAIT).unwrap().is_err());
    // 실패한 서버에는 새 문서를 붙이지 않는다.
    assert!(!m.open_document(&dir.path().join("b.rs"), ""));
}

#[test]
fn shutdown_stops_servers() {
    let dir = tempfile::tempdir().unwrap();
    let (m, p) = open(dir.path(), "a.rs", "fn a() {}\n", &[]);
    let clone = m.clone();
    m.shutdown();
    assert_eq!(clone.status_text(), None);
    assert!(clone.document_text(&p).is_none());
    assert!(clone.hover(&p, pos(0, 0)).wait(WAIT).unwrap().is_err());
}

#[test]
fn requests_before_ready_are_queued() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager(dir.path(), &[]);
    let p = dir.path().join("q.rs");
    assert!(m.open_document(&p, "fn queued() {}\n"));
    // 준비 전에 보낸 요청도 응답을 받는다.
    let hover = m.hover(&p, pos(0, 5)).wait(WAIT).unwrap().unwrap();
    assert_eq!(hover.as_deref(), Some("**hover** `queued`"));
}

#[test]
fn dropping_last_manager_stops_server_process() {
    let dir = tempfile::tempdir().unwrap();
    let tag = format!("kiln-drop-{}-{}", std::process::id(), dir.path().file_name().unwrap().to_string_lossy());
    let running = || {
        std::process::Command::new("pgrep")
            .args(["-f", &tag])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    };
    let (m, _p) = open(dir.path(), "a.rs", "fn a() {}\n", &["--tag", &tag]);
    assert!(running());
    drop(m);
    assert!(wait_until(|| !running()), "서버 프로세스가 남아 있습니다");
}
