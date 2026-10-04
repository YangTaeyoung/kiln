//! 디스크에 저장하는 앱 상태(스페이스, 페이지, 카드 배치, 세션 id, 설정).

use super::layout::{Node, PaneId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Persist {
    pub workspaces: Vec<WorkspaceP>,
    pub active: usize,
    pub settings: Settings,
    pub sidebar_open: bool,
    pub notifications: Vec<super::notifications::Notification>,
    pub recent_panes: Vec<PaneId>,
    pub terminal_launch_drafts: Vec<TerminalLaunchDraft>,
}

/// A launch with no confirmed input delivery. Restored only for explicit review.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TerminalLaunchDraft {
    pub pane: PaneId,
    pub command: Option<String>,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub ui_scale: f32,
    pub option_as_meta: bool,
    pub confirm_close_running: bool,
    pub os_notifications: bool,
    pub notification_toasts: bool,
    pub do_not_disturb: bool,
    pub shell: String,
    pub copy_on_select: bool,
    pub theme: String,
    pub line_height: f32,
    pub cursor_blink: bool,
    pub card_gap: f32,
    pub sidebar_width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_size: 13.5,
            ui_scale: 1.0,
            option_as_meta: true,
            confirm_close_running: true,
            os_notifications: true,
            notification_toasts: true,
            do_not_disturb: false,
            shell: String::new(),
            copy_on_select: false,
            theme: "kiln-dark".into(),
            line_height: 1.25,
            cursor_blink: false,
            card_gap: 8.0,
            sidebar_width: 232.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct WorkspaceP {
    pub name: String,
    pub root: PathBuf,
    pub pages: Vec<PageP>,
    pub active_page: usize,
    pub sheet: Option<String>,
    pub last_inspector: Option<String>,
    pub drafts: WorkspaceDrafts,
    /// 이전 형식(탭). 읽을 때 페이지로 옮긴다.
    #[serde(skip_serializing)]
    pub tabs: Vec<TabP>,
    #[serde(skip_serializing)]
    pub active_tab: usize,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PageP {
    #[serde(default)]
    pub manual_split: bool,
    #[serde(default)]
    pub zoomed: Option<PaneId>,
    pub root: Node,
    pub focused: PaneId,
    pub panes: Vec<PaneP>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent_request: Option<PathBuf>,
    #[serde(default)]
    pub agent_request_offset: usize,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PaneP {
    pub id: PaneId,
    pub session: Option<u64>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub tool: Option<ToolP>,
}

/// 카드에 담긴 도구(터미널이 아닌 카드).
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ToolP {
    Editor { path: PathBuf },
    DbTable { conn: u64, schema: Option<String>, table: String },
    DbConsole { conn: u64 },
    History,
    RepositoryHistory { root: PathBuf },
    EditorDraft { path: PathBuf, draft: kiln_editor::EditorDraft },
    DbConsoleDraft { conn: u64, sql: String },
    DbConsoleDocument { conn: u64, document: kiln_db::ConsoleDocument },
    DbTableDraft { conn: u64, schema: Option<String>, table: String, draft: kiln_db::TableDraft },
    Diff { root: PathBuf, path: PathBuf, staged: bool },
    Commit { root: PathBuf, sha: String },
    Range { root: PathBuf, from: String, to: Option<String> },
    PrDraft { root: PathBuf, repo: Option<String>, number: u64, body: String },
    IssueDraft { root: PathBuf, repo: Option<String>, number: u64, body: String },
    Pr { root: PathBuf, repo: Option<String>, number: u64 },
    Issue { root: PathBuf, repo: Option<String>, number: u64 },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct WorkspaceDrafts {
    pub agent_prompt:String,
    pub repositories: std::collections::BTreeMap<PathBuf, LocalRepositoryDrafts>,
    pub commit_message: String,
    pub github: kiln_git::GithubDrafts,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct LocalRepositoryDrafts {pub commit_message:String,pub github:kiln_git::GithubDrafts}

/// 이전 형식의 탭.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum TabP {
    Terminal { root: Node, focused: PaneId, panes: Vec<PaneP>, title: Option<String> },
    Editor { path: PathBuf },
    DbTable { conn: u64, schema: Option<String>, table: String },
    DbConsole { conn: u64 },
}

impl WorkspaceP {
    /// 이전 탭 형식을 페이지로 옮긴다. 도구 탭은 카드 하나짜리 페이지가 된다.
    pub fn migrate(&mut self, next_id: &mut u64) {
        if !self.pages.is_empty() || self.tabs.is_empty() {
            return;
        }
        for t in std::mem::take(&mut self.tabs) {
            let page = match t {
                TabP::Terminal { root, focused, panes, title } => PageP { root, focused, panes, title, manual_split:false, zoomed:None,agent_request:None,agent_request_offset:0 },
                other => {
                    *next_id += 1;
                    let id = *next_id;
                    let tool = match other {
                        TabP::Editor { path } => ToolP::Editor { path },
                        TabP::DbTable { conn, schema, table } => ToolP::DbTable { conn, schema, table },
                        TabP::DbConsole { conn } => ToolP::DbConsole { conn },
                        TabP::Terminal { .. } => unreachable!(),
                    };
                    PageP { manual_split:false, zoomed:None, root: Node::Leaf(id), focused: id, panes: vec![PaneP { id, session: None, cwd: None, tool: Some(tool) }], title: None,agent_request:None,agent_request_offset:0 }
                }
            };
            self.pages.push(page);
        }
        self.active_page = self.active_tab.min(self.pages.len().saturating_sub(1));
    }
}

pub fn path() -> PathBuf {
    kiln_common::paths::config_file("state.json")
}

pub struct LoadReport {
    pub state: Persist,
    pub warning: Option<String>,
    pub quarantined: Option<PathBuf>,
}

pub fn load_with_report() -> LoadReport { load_path(&path()) }

pub fn load() -> Persist { load_with_report().state }

fn load_path(path: &std::path::Path) -> LoadReport {
    let mut report = LoadReport { state: Persist::default(), warning: None, quarantined: None };
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Persist>(&bytes) {
            Ok(state) => report.state = state,
            Err(error) => {
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                let backup = path.with_file_name(format!("state.corrupt-{stamp}-{}.json", std::process::id()));
                // create_new never replaces a previous recovery file. Keep the source
                // until the backup is fully written, then remove only those exact bytes.
                let result = (|| -> std::io::Result<()> {
                    use std::io::Write;
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
                    let mut file = options.open(&backup)?;
                    file.write_all(&bytes)?; file.sync_all()?;
                    if std::fs::read(path)? != bytes { return Err(std::io::Error::other("state changed while preserving it")); }
                    std::fs::remove_file(path)?;
                    Ok(())
                })();
                match result {
                    Ok(()) => { report.warning = Some(format!("작업 상태 파일을 읽지 못했습니다. 원본을 {}에 보관했습니다. {error}", backup.display())); report.quarantined = Some(backup); }
                    Err(e) => report.warning = Some(format!("작업 상태를 읽지 못했고 원본 보관에도 실패했습니다. 기존 파일을 덮어쓰지 않습니다. {error}; {e}")),
                }
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => report.warning = Some(format!("작업 상태를 읽지 못했습니다. 기존 파일은 유지됩니다. {error}")),
    }
    if report.state.workspaces.is_empty() { report.state.sidebar_open = true; }
    report
}

/// Return failure to the caller so its last-saved cache and recovery UI remain truthful.
pub fn save(p: &Persist) -> Result<(), String> { save_path(&path(), p) }

fn save_path(path: &std::path::Path, p: &Persist) -> Result<(), String> {
    match std::fs::read(path) {
        Ok(bytes) => { serde_json::from_slice::<Persist>(&bytes).map_err(|e| format!("손상된 기존 상태 파일을 덮어쓰지 않았습니다: {e}"))?; }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(format!("기존 상태 파일을 확인하지 못했습니다: {e}")),
    }
    let parent = path.parent().ok_or("상태 파일 경로가 올바르지 않습니다")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let temp = path.with_extension(format!("{}-{stamp}.tmp", std::process::id()));
    let result = (|| -> Result<(), String> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut file, p).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(parent).and_then(|dir| dir.sync_all()).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_state_is_preserved_and_new_state_can_be_saved() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("state.json");
        let original = b"{broken with original draft text";
        std::fs::write(&path, original).unwrap();
        assert!(save_path(&path, &Persist::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let report = load_path(&path);
        assert!(report.warning.is_some());
        assert_eq!(std::fs::read(report.quarantined.unwrap()).unwrap(), original);
        save_path(&path, &Persist::default()).unwrap();
        assert!(load_path(&path).warning.is_none());
    }

    #[test]
    fn state_save_failures_are_returned_and_retry_succeeds() {
        let dir = tempfile::tempdir().unwrap(); let blocker = dir.path().join("parent");
        std::fs::write(&blocker, "not a directory").unwrap();
        let path = blocker.join("state.json");
        assert!(save_path(&path, &Persist::default()).is_err());
        std::fs::remove_file(&blocker).unwrap();
        save_path(&path, &Persist::default()).unwrap();
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600); }
    }

    #[test]
    fn legacy_layout_defaults_and_manual_focus_preference_round_trip() {
        let old=r#"{"root":{"Leaf":7},"focused":7,"panes":[{"id":7,"session":10,"cwd":"/tmp"}],"title":"Build"}"#;
        let mut page:PageP=serde_json::from_str(old).unwrap();
        assert!(!page.manual_split);assert_eq!(page.zoomed,None);
        page.manual_split=true;page.zoomed=Some(7);
        let restored:PageP=serde_json::from_slice(&serde_json::to_vec(&page).unwrap()).unwrap();
        assert!(restored.manual_split);assert_eq!(restored.zoomed,Some(7));assert_eq!(restored.panes[0].session,Some(10));
    }

    #[test]
    fn old_tab_format_migrates_to_pages() {
        let json = r#"{"workspaces":[{"name":"w","root":"/tmp","tabs":[
            {"Terminal":{"root":{"Leaf":3},"focused":3,"panes":[{"id":3,"session":7,"cwd":"/tmp"}],"title":null}},
            {"Editor":{"path":"/tmp/a.rs"}}
        ],"active_tab":1,"tool":"git","tool_open":true}],"active":0}"#;
        let mut p: Persist = serde_json::from_str(json).unwrap();
        let mut next = 100;
        p.workspaces[0].migrate(&mut next);
        let w = &p.workspaces[0];
        assert_eq!(w.pages.len(), 2);
        assert_eq!(w.active_page, 1);
        assert_eq!(w.pages[0].panes[0].session, Some(7));
        assert!(matches!(w.pages[1].panes[0].tool, Some(ToolP::Editor { .. })));
        let out = serde_json::to_string(&p).unwrap();
        assert!(!out.contains("\"tabs\""));
    }
}
