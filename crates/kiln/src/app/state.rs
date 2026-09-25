//! 디스크에 저장하는 앱 상태(워크스페이스, 탭, 분할, 세션 id, 설정).

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
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct WorkspaceP {
    pub name: String,
    pub root: PathBuf,
    pub tabs: Vec<TabP>,
    pub active_tab: usize,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub tool_open: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum TabP {
    Terminal { root: Node, focused: PaneId, panes: Vec<PaneP>, title: Option<String> },
    Editor { path: PathBuf },
    DbTable { conn: u64, schema: Option<String>, table: String },
    DbConsole { conn: u64 },
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct PaneP {
    pub id: PaneId,
    pub session: Option<u64>,
    pub cwd: Option<String>,
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
