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
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_size: f32,
    pub ui_scale: f32,
    pub option_as_meta: bool,
    pub confirm_close_running: bool,
    pub os_notifications: bool,
    pub shell: String,
    pub copy_on_select: bool,
    pub theme: String,
    pub line_height: f32,
    pub cursor_blink: bool,
    pub card_gap: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_size: 13.5,
            ui_scale: 1.0,
            option_as_meta: true,
            confirm_close_running: true,
            os_notifications: true,
            shell: String::new(),
            copy_on_select: false,
            theme: "kiln-dark".into(),
            line_height: 1.25,
            cursor_blink: false,
            card_gap: 8.0,
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
    /// 이전 형식(탭). 읽을 때 페이지로 옮긴다.
    #[serde(skip_serializing)]
    pub tabs: Vec<TabP>,
    #[serde(skip_serializing)]
    pub active_tab: usize,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PageP {
    pub root: Node,
    pub focused: PaneId,
    pub panes: Vec<PaneP>,
    #[serde(default)]
    pub title: Option<String>,
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
}

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
                TabP::Terminal { root, focused, panes, title } => PageP { root, focused, panes, title },
                other => {
                    *next_id += 1;
                    let id = *next_id;
                    let tool = match other {
                        TabP::Editor { path } => ToolP::Editor { path },
                        TabP::DbTable { conn, schema, table } => ToolP::DbTable { conn, schema, table },
                        TabP::DbConsole { conn } => ToolP::DbConsole { conn },
                        TabP::Terminal { .. } => unreachable!(),
                    };
                    PageP { root: Node::Leaf(id), focused: id, panes: vec![PaneP { id, session: None, cwd: None, tool: Some(tool) }], title: None }
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

pub fn load() -> Persist {
    let mut p: Persist = kiln_common::store::load_json(&path());
    if p.workspaces.is_empty() {
        p.sidebar_open = true;
    }
    p
}

pub fn save(p: &Persist) {
    if let Err(e) = kiln_common::store::save_json(&path(), p) {
        log::warn!("save state: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
