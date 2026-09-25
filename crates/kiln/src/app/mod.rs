//! Kiln GUI.

pub mod conn;
pub mod fonts;
mod icons;
mod keys;
mod layout;
mod palette;
mod state;
pub mod terminal;
mod tools;

use conn::{Conn, ConnEvent};
use egui::{Color32, FontId, Key, KeyboardShortcut, Modifiers, Rect, RichText, Sense, Stroke, pos2, vec2};
use kiln_common::Theme;
use kiln_proto::{SessionId, SpawnSpec};
use layout::{Dir, Nav, Node, PaneId};
use state::{PaneP, Persist, Settings, TabP, WorkspaceP};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use terminal::{LinkTarget, TermSettings, TermView};

pub fn run(path: Option<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kiln")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../../../assets/Kiln.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native("Kiln", options, Box::new(move |cc| Ok(Box::new(KilnApp::new(&cc.egui_ctx, path)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub struct Pane {
    pub id: PaneId,
    pub session: Option<SessionId>,
    pub pending: Option<u32>,
    pub view: Option<TermView>,
    pub cwd: Option<String>,
}

pub struct TermTab {
    pub root: Node,
    pub focused: PaneId,
    pub rects: Vec<(PaneId, Rect)>,
    pub title: Option<String>,
}

pub enum TabKind {
    Terminal(TermTab),
    Tool(Box<dyn tools::ToolTab>),
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
}

pub struct Workspace {
    pub id: u64,
    pub name: String,
    pub root: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub tool: tools::ToolKind,
    pub tool_open: bool,
    pub tools: tools::WorkspaceTools,
    pub renaming: Option<String>,
}

pub enum Action {
    NewWorkspace(Option<PathBuf>),
    PickWorkspaceFolder,
    SelectWorkspace(usize),
    CloseWorkspace(usize),
    RenameWorkspace(usize, String),
    NewTermTab,
    Split(Dir),
    ClosePane(PaneId, bool),
    CloseActive,
    FocusPane(PaneId),
    SelectTab(usize),
    CloseTab(usize, bool),
    NextTab(i32),
    Navigate(Nav),
    FontDelta(f32),
    OpenLink(LinkTarget, Option<String>),
    RestartPane(PaneId),
    ToggleSidebar,
    ToggleTool(tools::ToolKind),
    OpenPalette,
    QuickOpen,
    JumpUnread,
    Equalize,
    FindInTerminal,
    AttachSession(SessionId),
    KillSession(SessionId),
    OpenSettings,
    UpgradeDaemon,
    OpenTab(tools::TabFactory),
    CloseTabByKey(String),
    RenamedFile(PathBuf, PathBuf),
    NewTermAt(PathBuf),
    RunInTerminal(String),
    Toast(String),
}

struct Toast {
    text: String,
    at: Instant,
    kind: ToastKind,
}

#[derive(PartialEq, Clone, Copy)]
enum ToastKind {
    Info,
    Notify,
    Error,
}

struct Confirm {
    title: String,
    body: String,
    ok: String,
    action: Action,
}

pub struct KilnApp {
    conn: Conn,
    workspaces: Vec<Workspace>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    pending_creates: HashMap<u32, PaneId>,
    next_id: u64,
    settings: Settings,
    sidebar_open: bool,
    toasts: Vec<Toast>,
    confirm: Option<Confirm>,
    palette: palette::Palette,
    settings_open: bool,
    last_saved: Option<Persist>,
    last_save_at: Instant,
    focus_terminal: bool,
    restored: bool,
    unread: Vec<(u64, SessionId)>,
    window_focused: bool,
    actions: Vec<Action>,
    theme: Theme,
    pending_input: HashMap<PaneId, String>,
    db: kiln_db::DbManager,
}

fn shells() -> &'static [&'static str] {
    &["zsh", "bash", "fish", "sh", "dash", "tcsh", "csh", "nu", "pwsh", "powershell", "cmd", "login", "-zsh", "-bash"]
}

impl KilnApp {
    pub fn new(ctx: &egui::Context, open_path: Option<PathBuf>) -> Self {
        fonts::install(ctx);
        let theme = Theme::current();
        theme.apply(ctx);
        let persisted = state::load();
        ctx.set_zoom_factor(persisted.settings.ui_scale.clamp(0.6, 2.0));
        let mut app = KilnApp {
            conn: Conn::new(ctx.clone()),
            workspaces: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            pending_creates: HashMap::new(),
            next_id: 1,
            settings: persisted.settings.clone(),
            sidebar_open: persisted.sidebar_open || persisted.workspaces.is_empty(),
            toasts: Vec::new(),
            confirm: None,
            palette: palette::Palette::default(),
            settings_open: false,
            last_saved: None,
            last_save_at: Instant::now(),
            focus_terminal: true,
            restored: false,
            unread: Vec::new(),
            window_focused: true,
            actions: Vec::new(),
            theme,
            pending_input: HashMap::new(),
            db: kiln_db::DbManager::load(),
        };
        app.restore(&persisted, ctx);
        if let Some(p) = open_path {
            let p = normalize_path(p.canonicalize().unwrap_or(p));
            if let Some(i) = app.workspaces.iter().position(|w| w.root == p) {
                app.active = i;
            } else {
                app.add_workspace(p, ctx);
            }
        }
        if app.workspaces.is_empty() {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let cwd = std::env::current_dir().ok().filter(|d| d != Path::new("/")).unwrap_or(home);
            app.add_workspace(cwd, ctx);
        }
        app
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn restore(&mut self, p: &Persist, ctx: &egui::Context) {
        let max_pane = p.workspaces.iter().flat_map(|w| w.tabs.iter()).filter_map(|t| match t {
            TabP::Terminal { panes, .. } => panes.iter().map(|x| x.id).max(),
            _ => None,
        }).max().unwrap_or(0);
        self.next_id = max_pane + 1000;
        for wp in &p.workspaces {
            let id = self.id();
            let mut ws = Workspace {
                id,
                name: wp.name.clone(),
                root: wp.root.clone(),
                tabs: Vec::new(),
                active_tab: wp.active_tab,
                tool: tools::ToolKind::from_str(&wp.tool),
                tool_open: wp.tool_open,
                tools: tools::WorkspaceTools::new(&wp.root, ctx, self.db.clone()),
                renaming: None,
            };
            for t in &wp.tabs {
                let tid = self.id();
                match t {
                    TabP::Terminal { root, focused, panes, title } => {
                        for pp in panes {
                            self.panes.insert(pp.id, Pane { id: pp.id, session: pp.session, pending: None, view: pp.session.map(TermView::new), cwd: pp.cwd.clone() });
                        }
                        ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: root.clone(), focused: *focused, rects: vec![], title: title.clone() }) });
                    }
                    other => {
                        if let Some(tab) = ws.tools.restore_tab(other, ctx) {
                            ws.tabs.push(Tab { id: tid, kind: TabKind::Tool(tab) });
                        }
                    }
                }
            }
            if ws.tabs.is_empty() {
                let pane = self.new_pane(Some(wp.root.to_string_lossy().into_owned()));
                let tid = self.id();
                ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) });
            }
            ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
            self.workspaces.push(ws);
        }
        self.active = p.active.min(self.workspaces.len().saturating_sub(1));
    }

    fn persist(&self) -> Persist {
        let workspaces = self.workspaces.iter().map(|w| WorkspaceP {
            name: w.name.clone(),
            root: w.root.clone(),
            active_tab: w.active_tab,
            tool: w.tool.as_str().into(),
            tool_open: w.tool_open,
            tabs: w.tabs.iter().filter_map(|t| match &t.kind {
                TabKind::Terminal(tt) => Some(TabP::Terminal {
                    root: tt.root.clone(),
                    focused: tt.focused,
                    title: tt.title.clone(),
                    panes: tt.root.panes().iter().filter_map(|p| self.panes.get(p)).map(|p| PaneP {
                        id: p.id,
                        session: p.session,
                        cwd: p.session.and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone()).or_else(|| p.cwd.clone()),
                    }).collect(),
                }),
                TabKind::Tool(t) => t.persist(),
            }).collect(),
        }).collect();
        Persist { workspaces, active: self.active, settings: self.settings.clone(), sidebar_open: self.sidebar_open }
    }

    fn save_if_changed(&mut self, force: bool) {
        if !force && self.last_save_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_save_at = Instant::now();
        let p = self.persist();
        if self.last_saved.as_ref() != Some(&p) {
            state::save(&p);
            self.last_saved = Some(p);
        }
    }

    fn new_pane(&mut self, cwd: Option<String>) -> PaneId {
        let id = self.id();
        self.panes.insert(id, Pane { id, session: None, pending: None, view: None, cwd });
        id
    }

    fn spawn_for_pane(&mut self, pane: PaneId) {
        let shell = self.settings.shell.clone();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.pending.is_some() || !self.conn.is_connected() {
            return;
        }
        let ws = self.workspaces.iter().find(|w| w.tabs.iter().any(|t| matches!(&t.kind, TabKind::Terminal(tt) if tt.root.panes().contains(&pane))));
        let spec = SpawnSpec {
            cwd: p.cwd.clone().or_else(|| ws.map(|w| w.root.to_string_lossy().into_owned())),
            program: if shell.is_empty() { None } else { Some(shell) },
            cols: 100,
            rows: 30,
            workspace: ws.map(|w| w.name.clone()),
            ..Default::default()
        };
        if let Some(req) = self.conn.create(spec) {
            p.pending = Some(req);
            self.pending_creates.insert(req, pane);
        }
    }

    fn add_workspace(&mut self, root: PathBuf, ctx: &egui::Context) {
        let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned());
        let pane = self.new_pane(Some(root.to_string_lossy().into_owned()));
        let (wid, tid) = (self.id(), self.id());
        self.workspaces.push(Workspace {
            id: wid,
            name,
            tools: tools::WorkspaceTools::new(&root, ctx, self.db.clone()),
            root,
            tabs: vec![Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) }],
            active_tab: 0,
            tool: tools::ToolKind::Explorer,
            tool_open: false,
            renaming: None,
        });
        self.active = self.workspaces.len() - 1;
        self.focus_terminal = true;
    }

    fn ws(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(self.active)
    }

    fn active_term_tab(&mut self) -> Option<&mut TermTab> {
        let ws = self.workspaces.get_mut(self.active)?;
        match &mut ws.tabs.get_mut(ws.active_tab)?.kind {
            TabKind::Terminal(t) => Some(t),
            _ => None,
        }
    }

    fn focused_pane(&self) -> Option<PaneId> {
        let ws = self.workspaces.get(self.active)?;
        match &ws.tabs.get(ws.active_tab)?.kind {
            TabKind::Terminal(t) => Some(t.focused),
            _ => None,
        }
    }

    fn focused_session(&self) -> Option<SessionId> {
        self.focused_pane().and_then(|p| self.panes.get(&p)).and_then(|p| p.session)
    }

    fn pane_is_busy(&self, pane: PaneId) -> Option<String> {
        let s = self.panes.get(&pane)?.session?;
        let info = self.conn.infos.get(&s)?;
        if info.exited.is_some() {
            return None;
        }
        let fg = info.fg_process.clone()?;
        if shells().contains(&fg.as_str()) { None } else { Some(fg) }
    }

    // ---------- 동작 ----------

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::NewWorkspace(Some(p)) => self.add_workspace(p, ctx),
            Action::NewWorkspace(None) | Action::PickWorkspaceFolder => {
                if let Some(p) = rfd::FileDialog::new().set_title("워크스페이스 루트 폴더 선택").pick_folder() {
                    self.add_workspace(p, ctx);
                }
            }
            Action::SelectWorkspace(i) => {
                if i < self.workspaces.len() {
                    self.active = i;
                    self.focus_terminal = true;
                }
            }
            Action::CloseWorkspace(i) => {
                if i < self.workspaces.len() {
                    let ws = self.workspaces.remove(i);
                    for t in ws.tabs {
                        if let TabKind::Terminal(tt) = t.kind {
                            for p in tt.root.panes() {
                                if let Some(pane) = self.panes.remove(&p)
                                    && let Some(s) = pane.session {
                                        self.conn.kill(s);
                                    }
                            }
                        }
                    }
                    if self.workspaces.is_empty() {
                        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
                        self.add_workspace(home, ctx);
                    }
                    self.active = self.active.min(self.workspaces.len() - 1);
                }
            }
            Action::RenameWorkspace(i, name) => {
                if let Some(w) = self.workspaces.get_mut(i)
                    && !name.trim().is_empty() {
                        w.name = name.trim().to_string();
                    }
            }
            Action::NewTermTab => {
                let cwd = self.focused_session().and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone())
                    .or_else(|| self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned()));
                let pane = self.new_pane(cwd);
                let tid = self.id();
                if let Some(ws) = self.ws() {
                    ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) });
                    ws.active_tab = ws.tabs.len() - 1;
                }
                self.focus_terminal = true;
            }
            Action::Split(dir) => {
                let cwd = self.focused_session().and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone());
                if self.active_term_tab().is_none() {
                    self.apply(Action::NewTermTab, ctx);
                    return;
                }
                let pane = self.new_pane(cwd);
                if let Some(tt) = self.active_term_tab() {
                    let f = tt.focused;
                    tt.root.split(f, dir, pane);
                    tt.focused = pane;
                }
                self.focus_terminal = true;
            }
            Action::CloseActive => {
                let ws = &self.workspaces[self.active];
                match ws.tabs.get(ws.active_tab).map(|t| &t.kind) {
                    Some(TabKind::Terminal(tt)) => {
                        let p = tt.focused;
                        self.apply(Action::ClosePane(p, false), ctx);
                    }
                    Some(TabKind::Tool(_)) => {
                        let i = ws.active_tab;
                        self.apply(Action::CloseTab(i, false), ctx);
                    }
                    None => {}
                }
            }
            Action::ClosePane(p, force) => {
                if !force && self.settings.confirm_close_running
                    && let Some(proc_name) = self.pane_is_busy(p) {
                        self.confirm = Some(Confirm {
                            title: "실행 중인 프로세스".into(),
                            body: format!("이 창에서 `{proc_name}` 이(가) 실행 중입니다. 종료하면 프로세스도 함께 종료됩니다."),
                            ok: "종료".into(),
                            action: Action::ClosePane(p, true),
                        });
                        return;
                    }
                if let Some(pane) = self.panes.remove(&p)
                    && let Some(s) = pane.session {
                        self.conn.kill(s);
                    }
                let ws = &mut self.workspaces[self.active];
                let mut remove_tab = None;
                for (i, t) in ws.tabs.iter_mut().enumerate() {
                    if let TabKind::Terminal(tt) = &mut t.kind
                        && tt.root.panes().contains(&p) {
                            if tt.root.remove(p) {
                                if tt.focused == p {
                                    tt.focused = tt.root.panes()[0];
                                }
                            } else {
                                remove_tab = Some(i);
                            }
                        }
                }
                if let Some(i) = remove_tab {
                    ws.tabs.remove(i);
                    if ws.tabs.is_empty() {
                        self.apply(Action::NewTermTab, ctx);
                    }
                    let ws = &mut self.workspaces[self.active];
                    ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
                }
                self.focus_terminal = true;
            }
            Action::CloseTab(i, force) => {
                let ws = &self.workspaces[self.active];
                let Some(tab) = ws.tabs.get(i) else { return };
                match &tab.kind {
                    TabKind::Terminal(tt) => {
                        let panes = tt.root.panes();
                        if !force && self.settings.confirm_close_running {
                            let busy: Vec<String> = panes.iter().filter_map(|p| self.pane_is_busy(*p)).collect();
                            if !busy.is_empty() {
                                self.confirm = Some(Confirm {
                                    title: "실행 중인 프로세스".into(),
                                    body: format!("이 탭에서 {} 이(가) 실행 중입니다. 탭을 닫으면 모두 종료됩니다.", busy.join(", ")),
                                    ok: "탭 닫기".into(),
                                    action: Action::CloseTab(i, true),
                                });
                                return;
                            }
                        }
                        for p in panes {
                            if let Some(pane) = self.panes.remove(&p)
                                && let Some(s) = pane.session {
                                    self.conn.kill(s);
                                }
                        }
                    }
                    TabKind::Tool(t) => {
                        if !force && t.is_dirty() {
                            self.confirm = Some(Confirm {
                                title: "저장하지 않은 변경".into(),
                                body: format!("{} 에 저장하지 않은 변경이 있습니다. 버리고 닫을까요?", t.title()),
                                ok: "버리고 닫기".into(),
                                action: Action::CloseTab(i, true),
                            });
                            return;
                        }
                    }
                }
                let ws = &mut self.workspaces[self.active];
                ws.tabs.remove(i);
                if ws.tabs.is_empty() {
                    self.apply(Action::NewTermTab, ctx);
                } else {
                    let ws = &mut self.workspaces[self.active];
                    if ws.active_tab >= i && ws.active_tab > 0 {
                        ws.active_tab -= 1;
                    }
                }
            }
            Action::FocusPane(p) => {
                if let Some(tt) = self.active_term_tab() {
                    tt.focused = p;
                }
                self.focus_terminal = true;
            }
            Action::SelectTab(i) => {
                if let Some(ws) = self.ws()
                    && i < ws.tabs.len() {
                        ws.active_tab = i;
                    }
                self.focus_terminal = true;
            }
            Action::NextTab(d) => {
                if let Some(ws) = self.ws() {
                    let n = ws.tabs.len() as i32;
                    ws.active_tab = ((ws.active_tab as i32 + d).rem_euclid(n)) as usize;
                }
                self.focus_terminal = true;
            }
            Action::Navigate(nav) => {
                if let Some(tt) = self.active_term_tab()
                    && let Some(n) = layout::neighbor(&tt.rects, tt.focused, nav) {
                        tt.focused = n;
                    }
                self.focus_terminal = true;
            }
            Action::Equalize => {
                if let Some(tt) = self.active_term_tab() {
                    tt.root.equalize();
                }
            }
            Action::FontDelta(d) => {
                self.settings.font_size = if d == 0.0 { 13.5 } else { (self.settings.font_size + d).clamp(8.0, 32.0) };
            }
            Action::OpenLink(target, _cwd) => match target {
                LinkTarget::Url(u) => {
                    let _ = open::that_detached(u);
                }
                LinkTarget::File { path, line, col } => {
                    self.open_file(path, line, col, ctx);
                }
            },
            Action::RestartPane(p) => {
                if let Some(pane) = self.panes.get_mut(&p) {
                    if let Some(s) = pane.session.take() {
                        pane.cwd = self.conn.infos.get(&s).and_then(|i| i.cwd.clone()).or(pane.cwd.clone());
                        self.conn.kill(s);
                    }
                    pane.view = None;
                }
                self.spawn_for_pane(p);
            }
            Action::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Action::ToggleTool(kind) => {
                if let Some(ws) = self.ws() {
                    if ws.tool == kind && ws.tool_open {
                        ws.tool_open = false;
                        self.focus_terminal = true;
                    } else {
                        ws.tool = kind;
                        ws.tool_open = true;
                        ws.tools.on_show(kind);
                    }
                }
            }
            Action::OpenPalette => self.palette.open(),
            Action::QuickOpen => {
                if let Some(ws) = self.ws() {
                    ws.tools.quick_open();
                }
            }
            Action::JumpUnread => {
                if let Some((_, sid)) = self.unread.pop() {
                    self.reveal_session(sid);
                }
            }
            Action::FindInTerminal => {
                if let Some(p) = self.focused_pane() {
                    if let Some(v) = self.panes.get_mut(&p).and_then(|p| p.view.as_mut()) {
                        v.open_search();
                    }
                } else if let Some(ws) = self.ws() {
                    let i = ws.active_tab;
                    if let Some(TabKind::Tool(t)) = ws.tabs.get_mut(i).map(|t| &mut t.kind) {
                        t.find();
                    }
                }
            }
            Action::AttachSession(sid) => {
                let pane = self.id();
                self.panes.insert(pane, Pane { id: pane, session: Some(sid), pending: None, view: Some(TermView::new(sid)), cwd: None });
                let tid = self.id();
                if let Some(ws) = self.ws() {
                    ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) });
                    ws.active_tab = ws.tabs.len() - 1;
                }
                self.focus_terminal = true;
            }
            Action::KillSession(sid) => self.conn.kill(sid),
            Action::OpenSettings => self.settings_open = true,
            Action::UpgradeDaemon => {
                let exe = conn::daemon_exe();
                self.conn.send(kiln_proto::ClientMsg::Upgrade { req: 0, exe: exe.to_string_lossy().into_owned() });
            }
            Action::OpenTab(factory) => {
                let ws = &mut self.workspaces[self.active];
                if let Some(i) = ws.tabs.iter().position(|t| matches!(&t.kind, TabKind::Tool(x) if x.key() == factory.key)) {
                    ws.active_tab = i;
                    if let TabKind::Tool(x) = &mut ws.tabs[i].kind {
                        factory.reuse(x.as_mut());
                    }
                } else {
                    let env = self.workspaces[self.active].tools.env();
                    match factory.make(ctx, &env) {
                        Ok(tab) => {
                            let tid = self.id();
                            let ws = &mut self.workspaces[self.active];
                            ws.tabs.push(Tab { id: tid, kind: TabKind::Tool(tab) });
                            ws.active_tab = ws.tabs.len() - 1;
                        }
                        Err(e) => self.toast(e, ToastKind::Error),
                    }
                }
            }
            Action::CloseTabByKey(key) => {
                let ws = &self.workspaces[self.active];
                if let Some(i) = ws.tabs.iter().position(|t| matches!(&t.kind, TabKind::Tool(x) if x.key() == key)) {
                    self.apply(Action::CloseTab(i, true), ctx);
                }
            }
            Action::RenamedFile(from, to) => {
                let key = format!("file:{}", from.display());
                let ws = &self.workspaces[self.active];
                if let Some(i) = ws.tabs.iter().position(|t| matches!(&t.kind, TabKind::Tool(x) if x.key() == key && !x.is_dirty())) {
                    self.apply(Action::CloseTab(i, true), ctx);
                    if to.is_file() {
                        self.apply(Action::OpenTab(tools::open_file_factory(to, None, None)), ctx);
                    }
                }
            }
            Action::NewTermAt(dir) => {
                let pane = self.new_pane(Some(dir.to_string_lossy().into_owned()));
                let tid = self.id();
                if let Some(ws) = self.ws() {
                    ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) });
                    ws.active_tab = ws.tabs.len() - 1;
                }
                self.focus_terminal = true;
            }
            Action::RunInTerminal(cmd) => {
                let cwd = self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned());
                let pane = self.new_pane(cwd);
                let tid = self.id();
                if let Some(ws) = self.ws() {
                    ws.tabs.push(Tab { id: tid, kind: TabKind::Terminal(TermTab { root: Node::Leaf(pane), focused: pane, rects: vec![], title: None }) });
                    ws.active_tab = ws.tabs.len() - 1;
                }
                self.spawn_for_pane(pane);
                self.pending_input.insert(pane, format!("{cmd}\r"));
                self.focus_terminal = true;
            }
            Action::Toast(t) => self.toast(t, ToastKind::Info),
        }
    }

    fn open_file(&mut self, path: PathBuf, line: Option<usize>, col: Option<usize>, _ctx: &egui::Context) {
        if path.is_dir() {
            self.actions.push(Action::NewTermAt(path));
            return;
        }
        self.actions.push(Action::OpenTab(tools::open_file_factory(path, line, col)));
    }

    fn reveal_session(&mut self, sid: SessionId) {
        for (wi, ws) in self.workspaces.iter_mut().enumerate() {
            for (ti, t) in ws.tabs.iter_mut().enumerate() {
                if let TabKind::Terminal(tt) = &mut t.kind {
                    for p in tt.root.panes() {
                        if self.panes.get(&p).and_then(|x| x.session) == Some(sid) {
                            self.active = wi;
                            ws.active_tab = ti;
                            tt.focused = p;
                            self.focus_terminal = true;
                            return;
                        }
                    }
                }
            }
        }
    }

    fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toasts.push(Toast { text: text.into(), at: Instant::now(), kind });
    }


    // ---------- 이벤트 ----------

    fn handle_conn_events(&mut self, ctx: &egui::Context) {
        for e in std::mem::take(&mut self.conn.events) {
            match e {
                ConnEvent::Created { req, session } => {
                    if let Some(pid) = self.pending_creates.remove(&req)
                        && let Some(p) = self.panes.get_mut(&pid) {
                            p.session = Some(session);
                            p.pending = None;
                            p.view = Some(TermView::new(session));
                            if let Some(input) = self.pending_input.remove(&pid) {
                                self.conn.input(session, input.into_bytes());
                            }
                        }
                }
                ConnEvent::Notification { session, title, body } => {
                    let focused_here = self.window_focused && self.focused_session() == Some(session);
                    if !focused_here {
                        let id = self.id();
                        self.unread.push((id, session));
                        let text = if title.is_empty() { body.clone() } else { format!("{title} — {body}") };
                        self.toast(text.clone(), ToastKind::Notify);
                        if !self.window_focused {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                            if self.settings.os_notifications {
                                os_notify(if title.is_empty() { "Kiln" } else { &title }, &body);
                            }
                        }
                    } else {
                        self.conn.send(kiln_proto::ClientMsg::ClearAttention { session });
                    }
                }
                ConnEvent::Exited { .. } => {}
                ConnEvent::Error(m) => self.toast(m, ToastKind::Error),
                ConnEvent::Connected => {
                    if self.restored {
                        self.toast("데몬에 다시 연결됨", ToastKind::Info);
                    }
                }
                ConnEvent::Upgrading => self.toast("데몬 업그레이드 중 — 세션은 유지됩니다", ToastKind::Info),
                ConnEvent::SearchResult { found } => {
                    if let Some(p) = self.focused_pane()
                        && let Some(v) = self.panes.get_mut(&p).and_then(|p| p.view.as_mut()) {
                            v.set_search_result(found);
                        }
                }
            }
        }
        // 세션 목록을 받은 뒤: 사라진 세션의 창은 새 셸로 채운다.
        if self.conn.is_connected() && self.conn.sessions_listed {
            self.restored = true;
            let visible: Vec<PaneId> = self.workspaces.iter().flat_map(|w| {
                w.tabs.iter().filter_map(|t| match &t.kind {
                    TabKind::Terminal(tt) => Some(tt.root.panes()),
                    _ => None,
                }).flatten().collect::<Vec<_>>()
            }).collect();
            for p in visible {
                let needs = match self.panes.get(&p) {
                    Some(pane) => pane.pending.is_none() && pane.session.is_none_or(|s| !self.conn.exists(s)),
                    None => false,
                };
                if needs {
                    if let Some(pane) = self.panes.get_mut(&p) {
                        pane.session = None;
                        pane.view = None;
                    }
                    self.spawn_for_pane(p);
                }
            }
        }
        // 연결이 끊기면 대기 중인 생성 요청을 버린다.
        if !self.conn.is_connected() && !self.pending_creates.is_empty() {
            for (_, p) in self.pending_creates.drain() {
                if let Some(pane) = self.panes.get_mut(&p) {
                    pane.pending = None;
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let mac = cfg!(target_os = "macos");
        // macOS: ⌘, 그 외: Ctrl+Shift (터미널의 Ctrl 조합과 겹치지 않게).
        let base = if mac { Modifiers::MAC_CMD } else { Modifiers::CTRL | Modifiers::SHIFT };
        let base_shift = if mac { Modifiers::MAC_CMD | Modifiers::SHIFT } else { Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT };
        let base_alt = if mac { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::CTRL | Modifiers::ALT };
        let table: Vec<(Modifiers, Key, Action)> = vec![
            (base, Key::T, Action::NewTermTab),
            (base, Key::D, Action::Split(Dir::Horizontal)),
            (base_shift, Key::D, Action::Split(Dir::Vertical)),
            (base, Key::W, Action::CloseActive),
            (base, Key::N, Action::NewWorkspace(None)),
            (base, Key::P, Action::QuickOpen),
            (base_shift, Key::P, Action::OpenPalette),
            (base, Key::K, Action::OpenPalette),
            (base, Key::B, Action::ToggleSidebar),
            (base, Key::Equals, Action::FontDelta(1.0)),
            (base, Key::Plus, Action::FontDelta(1.0)),
            (base, Key::Minus, Action::FontDelta(-1.0)),
            (base, Key::Num0, Action::FontDelta(0.0)),
            (base, Key::F, Action::FindInTerminal),
            (base_shift, Key::E, Action::ToggleTool(tools::ToolKind::Explorer)),
            (base_shift, Key::F, Action::ToggleTool(tools::ToolKind::Search)),
            (base_shift, Key::G, Action::ToggleTool(tools::ToolKind::Git)),
            (base_shift, Key::R, Action::ToggleTool(tools::ToolKind::PullRequests)),
            (base_shift, Key::B, Action::ToggleTool(tools::ToolKind::Database)),
            (base_shift, Key::M, Action::ToggleTool(tools::ToolKind::Problems)),
            (base_shift, Key::U, Action::JumpUnread),
            (base_shift, Key::OpenBracket, Action::NextTab(-1)),
            (base_shift, Key::CloseBracket, Action::NextTab(1)),
            (base_alt, Key::ArrowLeft, Action::Navigate(Nav::Left)),
            (base_alt, Key::ArrowRight, Action::Navigate(Nav::Right)),
            (base_alt, Key::ArrowUp, Action::Navigate(Nav::Up)),
            (base_alt, Key::ArrowDown, Action::Navigate(Nav::Down)),
            (base_alt, Key::Equals, Action::Equalize),
            (base, Key::Comma, Action::OpenSettings),
        ];
        // consume_shortcut 은 추가 Shift/Alt 를 무시하므로 수식키가 많은 조합부터 검사한다.
        let mut table = table;
        table.sort_by_key(|(m, _, _)| std::cmp::Reverse(m.shift as u8 + m.alt as u8 + m.ctrl as u8 + m.mac_cmd as u8));
        for (m, k, a) in table {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k))) {
                self.actions.push(a);
            }
        }
        let nums = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in nums.iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_shortcut(&KeyboardShortcut::new(base, *k))) {
                self.actions.push(Action::SelectWorkspace(i));
            }
            let tab_mod = if mac { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::ALT };
            if ctx.input_mut(|inp| inp.consume_shortcut(&KeyboardShortcut::new(tab_mod, *k))) {
                self.actions.push(Action::SelectTab(i));
            }
        }
    }
}

/// 테스트용 상태 조회.
impl KilnApp {
    #[doc(hidden)]
    pub fn debug_focused_text(&self) -> Option<String> {
        let s = self.focused_session()?;
        let sc = self.conn.screens.get(&s)?;
        Some(sc.lines.iter().map(|l| l.text().trim_end().to_string()).collect::<Vec<_>>().join("\n"))
    }

    #[doc(hidden)]
    pub fn debug_image_count(&self) -> usize {
        self.conn.textures.len()
    }

    #[doc(hidden)]
    pub fn debug_pane_count(&self) -> usize {
        let ws = &self.workspaces[self.active];
        match &ws.tabs[ws.active_tab].kind {
            TabKind::Terminal(t) => t.root.panes().len(),
            _ => 0,
        }
    }

    #[doc(hidden)]
    pub fn debug_active_tab_title(&self) -> String {
        let ws = &self.workspaces[self.active];
        match &ws.tabs[ws.active_tab].kind {
            TabKind::Terminal(_) => "terminal".into(),
            TabKind::Tool(t) => t.title(),
        }
    }
}

fn os_notify(title: &str, body: &str) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!("display notification \"{}\" with title \"{}\"", esc(&body), esc(&title));
            let _ = std::process::Command::new("osascript").arg("-e").arg(script).output();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("notify-send").arg(&title).arg(&body).output();
        }
        #[cfg(windows)]
        {
            let _ = (&title, &body);
        }
    });
}

/// Windows 확장 경로 접두(`\\?\`)를 떼어 일반 경로로 만든다.
fn normalize_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn short_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(home) = std::env::var_os("HOME") {
        let h = home.to_string_lossy();
        if let Some(rest) = s.strip_prefix(h.as_ref()) {
            return format!("~{rest}");
        }
    }
    s
}

impl eframe::App for KilnApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.conn.pump();
        let focused_now = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused_now && !self.window_focused
            && let Some(ws) = self.workspaces.get_mut(self.active) {
                for t in &mut ws.tabs {
                    if let TabKind::Tool(x) = &mut t.kind {
                        x.on_focus_regained();
                    }
                }
            }
        self.window_focused = focused_now;
        self.handle_conn_events(ctx);
        let quick_open = self.workspaces.get(self.active).is_some_and(|w| w.tools.quick_is_open());
        if self.confirm.is_none() && !self.palette.is_open() && !quick_open {
            self.shortcuts(ctx);
        }
        let active = self.active;
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            ws.tools.tick(i == active);
            for t in &mut ws.tabs {
                if let TabKind::Tool(x) = &mut t.kind {
                    x.tick();
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.ui_sidebar(ui);
        self.ui_statusbar(ui);
        self.ui_tool_panel(ui);
        self.ui_center(ui);
        self.ui_overlays(ui);

        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.apply(a, ctx);
        }
        self.save_if_changed(false);
    }

    fn on_exit(&mut self) {
        self.save_if_changed(true);
        for ws in &self.workspaces {
            ws.tools.lsp.shutdown();
        }
    }
}

// UI 는 별도 파일.
mod ui;
