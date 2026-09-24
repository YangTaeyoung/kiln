//! 사이드 도구 패널(탐색기, 검색, Git, PR, DB)과 중앙 도구 탭.

use super::Action;
use super::state::TabP;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolKind {
    Explorer,
    Search,
    Git,
    PullRequests,
    Database,
}

impl ToolKind {
    pub const ALL: [ToolKind; 5] = [ToolKind::Explorer, ToolKind::Search, ToolKind::Git, ToolKind::PullRequests, ToolKind::Database];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "explorer",
            ToolKind::Search => "search",
            ToolKind::Git => "git",
            ToolKind::PullRequests => "prs",
            ToolKind::Database => "db",
        }
    }

    pub fn from_str(s: &str) -> ToolKind {
        match s {
            "search" => ToolKind::Search,
            "git" => ToolKind::Git,
            "prs" => ToolKind::PullRequests,
            "db" => ToolKind::Database,
            _ => ToolKind::Explorer,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "탐색기",
            ToolKind::Search => "검색",
            ToolKind::Git => "소스 제어",
            ToolKind::PullRequests => "풀 리퀘스트",
            ToolKind::Database => "데이터베이스",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "🗀",
            ToolKind::Search => "🔍",
            ToolKind::Git => "⎇",
            ToolKind::PullRequests => "⇄",
            ToolKind::Database => "🛢",
        }
    }
}

/// 중앙 영역의 도구 탭(에디터, diff, PR, DB 테이블 등).
pub trait ToolTab {
    fn title(&self) -> String;
    /// 같은 대상을 다시 열 때 기존 탭을 찾는 키.
    fn key(&self) -> String;
    fn icon(&self) -> &'static str {
        "📄"
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action>;
    fn is_dirty(&self) -> bool {
        false
    }
    fn persist(&self) -> Option<TabP> {
        None
    }
    fn find(&mut self) {}
    fn save(&mut self) -> Option<anyhow::Result<()>> {
        None
    }
}

pub trait ToolTabFactory {
    fn make(&self, root: &Path, ctx: &egui::Context) -> Option<Box<dyn ToolTab>>;
    /// 이미 열린 탭을 재사용할 때 호출된다(예: 줄 이동).
    fn reuse(&self, _tab: &mut dyn ToolTab) {}
}

pub struct WorkspaceTools {
    pub root: PathBuf,
}

impl WorkspaceTools {
    pub fn new(root: &Path, _ctx: &egui::Context) -> Self {
        WorkspaceTools { root: root.to_path_buf() }
    }

    pub fn tick(&mut self, _ctx: &egui::Context) {}

    pub fn on_show(&mut self, _kind: ToolKind) {}

    pub fn quick_open(&mut self) {}

    pub fn restore_tab(&mut self, _t: &TabP, _ctx: &egui::Context) -> Option<Box<dyn ToolTab>> {
        None
    }

    pub fn panel_ui(&mut self, ui: &mut egui::Ui, kind: ToolKind) -> Vec<Action> {
        ui.label(format!("{} — 준비 중", kind.label()));
        Vec::new()
    }

    pub fn overlay_ui(&mut self, _ctx: &egui::Context) -> Vec<Action> {
        Vec::new()
    }

    /// 사이드바 요약(브랜치 등).
    pub fn summary(&self) -> Option<RepoLine> {
        None
    }
}

pub struct RepoLine {
    pub branch: String,
    pub dirty: u32,
    pub ahead: u32,
    pub behind: u32,
    pub pr: Option<(u64, String)>,
}

struct OpenFile {
    path: PathBuf,
    line: Option<usize>,
    col: Option<usize>,
}

impl ToolTabFactory for OpenFile {
    fn make(&self, _root: &Path, _ctx: &egui::Context) -> Option<Box<dyn ToolTab>> {
        let _ = (&self.path, self.line, self.col);
        None
    }
}

pub fn open_file_factory(path: PathBuf, line: Option<usize>, col: Option<usize>) -> Box<dyn ToolTabFactory> {
    Box::new(OpenFile { path, line, col })
}
