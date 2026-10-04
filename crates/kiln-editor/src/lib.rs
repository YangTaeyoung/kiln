//! Kiln 편집 기능: 파일 탐색기, 코드 편집기, 빠른 열기, 프로젝트 검색.

pub mod buffer;
pub mod editor;
pub mod file_tree;
pub mod fuzzy;
pub mod highlight;
pub mod lsp;
pub mod quick_open;
pub mod search;
pub mod search_panel;
pub mod syntax;
mod ui_kit;

use std::path::PathBuf;

pub use buffer::{FindOptions, Indent, LineEnding, Pos, Selection};
pub use editor::{EditorDraft, Editor, EditorStatus, Encoding, FoldRange};
pub use file_tree::{Decoration, FileTree};
pub use lsp::{LspManager, diagnostics_ui};
pub use quick_open::QuickOpen;
pub use search_panel::SearchPanel;

/// 편집 위젯들이 앱에 알리는 사건. 줄·열은 1부터 시작한다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    OpenFile(PathBuf),
    OpenAt { path: PathBuf, line: usize, col: usize },
    FileDeleted(PathBuf),
    FileRenamed { from: PathBuf, to: PathBuf },
    RevealInTerminal(PathBuf),
}

/// 문법 세트를 백그라운드에서 미리 로드한다. 앱 시작 시 한 번 호출하면 첫 파일 열기가 빨라진다.
pub fn prewarm() {
    syntax::prewarm();
}
