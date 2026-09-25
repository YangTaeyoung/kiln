//! Kiln GUI: 스페이스(워크스페이스) → 페이지 → 카드(터미널·에디터·DB 등) 분할 트리.

pub mod conn;
mod keys;
mod layout;
mod palette;
mod settings;
mod state;
pub mod terminal;
mod tools;
mod ui;

use conn::{Conn, ConnEvent};
use egui::{Key, KeyboardShortcut, Modifiers, Rect};
use kiln_common::Theme;
use kiln_common::icons;
use kiln_proto::{SessionId, SpawnSpec};
use layout::{Dir, Nav, Node, PaneId};
use state::{PageP, PaneP, Persist, Settings, WorkspaceP};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use terminal::{LinkTarget, TermView};

pub fn run(path: Option<PathBuf>) -> anyhow::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Kiln")
        .with_inner_size([1400.0, 880.0])
        .with_min_inner_size([720.0, 440.0])
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../../../assets/Kiln.png")).unwrap_or_default());
    if cfg!(target_os = "macos") {
        // 제목 표시줄을 숨기고 내용이 창 위까지 올라가게 한다(신호등 버튼은 상단 바 위에 뜬다).
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Kiln", options, Box::new(move |cc| Ok(Box::new(KilnApp::new(&cc.egui_ctx, path)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub enum PaneKind {
    Term { session: Option<SessionId>, pending: Option<u32>, view: Option<TermView> },
    Tool(Box<dyn tools::ToolTab>),
}

pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub cwd: Option<String>,
}

impl Pane {
    pub fn session(&self) -> Option<SessionId> {
        match &self.kind {
            PaneKind::Term { session, .. } => *session,
            PaneKind::Tool(_) => None,
        }
    }

    fn tool(&self) -> Option<&dyn tools::ToolTab> {
        match &self.kind {
            PaneKind::Tool(t) => Some(t.as_ref()),
            PaneKind::Term { .. } => None,
        }
    }
}

pub struct Page {
    pub id: u64,
    pub root: Node,
    pub focused: PaneId,
    pub rects: Vec<(PaneId, Rect)>,
    pub zoomed: Option<PaneId>,
    pub title: Option<String>,
}

impl Page {
    fn new(id: u64, pane: PaneId) -> Page {
        Page { id, root: Node::Leaf(pane), focused: pane, rects: vec![], zoomed: None, title: None }
    }
}

pub struct Workspace {
    pub id: u64,
    pub name: String,
    pub root: PathBuf,
    pub pages: Vec<Page>,
    pub active_page: usize,
    pub sheet: Option<tools::ToolKind>,
    pub tools: tools::WorkspaceTools,
    pub renaming: Option<String>,
}

impl Workspace {
    fn page(&self) -> &Page {
        &self.pages[self.active_page.min(self.pages.len() - 1)]
    }

    fn page_mut(&mut self) -> &mut Page {
        let i = self.active_page.min(self.pages.len() - 1);
        &mut self.pages[i]
    }

    fn all_panes(&self) -> Vec<PaneId> {
        self.pages.iter().flat_map(|p| p.root.panes()).collect()
    }
}

pub enum Action {
    NewWorkspace(Option<PathBuf>),
    SelectWorkspace(usize),
    CloseWorkspace(usize),
    RenameWorkspace(usize, String),
    NewPage,
    SelectPage(usize),
    ClosePage(usize, bool),
    NextPage(i32),
    Split(Dir),
    SplitPane(PaneId, Dir),
    ClosePane(PaneId, bool),
    CloseActive,
    FocusPane(PaneId),
    Navigate(Nav),
    Equalize,
    ToggleZoom(Option<PaneId>),
    FontDelta(f32),
    OpenLink(LinkTarget),
    RestartPane(PaneId),
    ToggleSidebar,
    ToggleSheet(tools::ToolKind),
    CloseSheet,
    OpenPalette,
    QuickOpen,
    JumpUnread,
    FindInFocused,
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
    SetTheme(String),
    RevealSession(SessionId),
}

pub(crate) struct Toast {
    title: String,
    body: String,
    at: Instant,
    kind: ToastKind,
    session: Option<SessionId>,
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum ToastKind {
    Info,
    Notify,
    Error,
}

pub(crate) struct Confirm {
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
    settings_ui: settings::SettingsUi,
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

pub(crate) fn shells() -> &'static [&'static str] {
    &["zsh", "bash", "fish", "sh", "dash", "tcsh", "csh", "nu", "pwsh", "powershell", "cmd", "login", "-zsh", "-bash"]
}

impl KilnApp {
    pub fn new(ctx: &egui::Context, open_path: Option<PathBuf>) -> Self {
        kiln_common::fonts::install(ctx);
        let persisted = state::load();
        Theme::set_current(&persisted.settings.theme);
        let theme = Theme::current();
        theme.apply(ctx);
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
            settings_ui: settings::SettingsUi::default(),
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
        app.restore(persisted, ctx);
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

    fn restore(&mut self, mut p: Persist, ctx: &egui::Context) {
        let max_new = p.workspaces.iter().flat_map(|w| w.pages.iter()).flat_map(|pg| pg.panes.iter().map(|x| x.id)).max().unwrap_or(0);
        let max_old = p
            .workspaces
            .iter()
            .flat_map(|w| w.tabs.iter())
            .filter_map(|t| match t {
                state::TabP::Terminal { panes, .. } => panes.iter().map(|x| x.id).max(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        self.next_id = max_new.max(max_old) + 1000;
        for wp in &mut p.workspaces {
            let mut nid = self.next_id;
            wp.migrate(&mut nid);
            self.next_id = nid + 1;
        }
        for wp in &p.workspaces {
            let id = self.id();
            let mut ws = Workspace {
                id,
                name: wp.name.clone(),
                root: wp.root.clone(),
                pages: Vec::new(),
                active_page: wp.active_page,
                sheet: wp.sheet.as_deref().map(tools::ToolKind::from_str),
                tools: tools::WorkspaceTools::new(&wp.root, ctx, self.db.clone()),
                renaming: None,
            };
            for pg in &wp.pages {
                let mut root = pg.root.clone();
                let mut alive = true;
                for pp in &pg.panes {
                    let kind = match &pp.tool {
                        Some(t) => match ws.tools.restore_tool(t, ctx) {
                            Some(tab) => PaneKind::Tool(tab),
                            None => {
                                // 복원할 수 없는 도구 카드(지워진 파일 등)는 빼고, 페이지가 비면 페이지를 버린다.
                                if !root.remove(pp.id) {
                                    alive = false;
                                }
                                continue;
                            }
                        },
                        None => PaneKind::Term { session: pp.session, pending: None, view: pp.session.map(TermView::new) },
                    };
                    self.panes.insert(pp.id, Pane { id: pp.id, kind, cwd: pp.cwd.clone() });
                }
                let panes = root.panes();
                if alive && !panes.is_empty() && panes.iter().all(|x| self.panes.contains_key(x)) {
                    let focused = if panes.contains(&pg.focused) { pg.focused } else { panes[0] };
                    let pid = self.id();
                    ws.pages.push(Page { id: pid, root, focused, rects: vec![], zoomed: None, title: pg.title.clone() });
                }
            }
            if ws.pages.is_empty() {
                let pane = self.new_term_pane(Some(wp.root.to_string_lossy().into_owned()));
                let pid = self.id();
                ws.pages.push(Page::new(pid, pane));
            }
            ws.active_page = ws.active_page.min(ws.pages.len() - 1);
            self.workspaces.push(ws);
        }
        self.active = p.active.min(self.workspaces.len().saturating_sub(1));
    }

    fn persist(&self) -> Persist {
        let workspaces = self
            .workspaces
            .iter()
            .map(|w| WorkspaceP {
                name: w.name.clone(),
                root: w.root.clone(),
                active_page: w.active_page,
                sheet: w.sheet.map(|k| k.as_str().to_string()),
                pages: w
                    .pages
                    .iter()
                    .map(|pg| PageP {
                        root: pg.root.clone(),
                        focused: pg.focused,
                        title: pg.title.clone(),
                        panes: pg
                            .root
                            .panes()
                            .iter()
                            .filter_map(|p| self.panes.get(p))
                            .map(|p| PaneP {
                                id: p.id,
                                session: p.session(),
                                cwd: p.session().and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone()).or_else(|| p.cwd.clone()),
                                tool: p.tool().and_then(|t| t.persist()),
                            })
                            .collect(),
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();
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

    fn new_term_pane(&mut self, cwd: Option<String>) -> PaneId {
        let id = self.id();
        self.panes.insert(id, Pane { id, kind: PaneKind::Term { session: None, pending: None, view: None }, cwd });
        id
    }

    fn workspace_of(&self, pane: PaneId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.pages.iter().any(|p| p.root.panes().contains(&pane)))
    }

    fn spawn_for_pane(&mut self, pane: PaneId) {
        let shell = self.settings.shell.clone();
        let ws_info = self.workspace_of(pane).map(|w| (w.root.to_string_lossy().into_owned(), w.name.clone()));
        let connected = self.conn.is_connected();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let cwd = p.cwd.clone();
        let PaneKind::Term { pending, .. } = &mut p.kind else { return };
        if pending.is_some() || !connected {
            return;
        }
        let spec = SpawnSpec {
            cwd: cwd.or_else(|| ws_info.as_ref().map(|w| w.0.clone())),
            program: if shell.is_empty() { None } else { Some(shell) },
            cols: 100,
            rows: 30,
            workspace: ws_info.map(|w| w.1),
            ..Default::default()
        };
        if let Some(req) = self.conn.create(spec) {
            *pending = Some(req);
            self.pending_creates.insert(req, pane);
        }
    }

    fn add_workspace(&mut self, root: PathBuf, ctx: &egui::Context) {
        let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned());
        let pane = self.new_term_pane(Some(root.to_string_lossy().into_owned()));
        let (wid, pid) = (self.id(), self.id());
        self.workspaces.push(Workspace {
            id: wid,
            name,
            tools: tools::WorkspaceTools::new(&root, ctx, self.db.clone()),
            root,
            pages: vec![Page::new(pid, pane)],
            active_page: 0,
            sheet: None,
            renaming: None,
        });
        self.active = self.workspaces.len() - 1;
        self.focus_terminal = true;
    }

    fn ws(&mut self) -> &mut Workspace {
        let i = self.active.min(self.workspaces.len() - 1);
        &mut self.workspaces[i]
    }

    fn focused_pane(&self) -> Option<PaneId> {
        self.workspaces.get(self.active).map(|w| w.page().focused)
    }

    fn focused_session(&self) -> Option<SessionId> {
        self.focused_pane().and_then(|p| self.panes.get(&p)).and_then(|p| p.session())
    }

    fn focused_cwd(&self) -> Option<String> {
        self.focused_session().and_then(|s| self.conn.infos.get(&s)).and_then(|i| i.cwd.clone())
    }

    fn pane_is_busy(&self, pane: PaneId) -> Option<String> {
        let s = self.panes.get(&pane)?.session()?;
        let info = self.conn.infos.get(&s)?;
        if info.exited.is_some() {
            return None;
        }
        let fg = info.fg_process.clone()?;
        if shells().contains(&fg.as_str()) { None } else { Some(fg) }
    }

    fn drop_pane(&mut self, p: PaneId) {
        if let Some(pane) = self.panes.remove(&p) {
            if let Some(s) = pane.session() {
                self.conn.kill(s);
            }
        }
    }

    /// 새 도구 카드를 현재 페이지에 넣는다. 저장 안 된 변경이 없는 도구 카드가 있으면 그 자리를 바꾸고,
    /// 없으면 포커스된 카드 오른쪽에 붙인다.
    fn place_card(&mut self, kind: PaneKind) -> PaneId {
        let id = self.id();
        self.panes.insert(id, Pane { id, kind, cwd: None });
        let page_panes = self.ws().page().root.panes();
        let replace = page_panes.iter().copied().find(|p| *p != id && self.panes.get(p).and_then(|x| x.tool()).is_some_and(|t| !t.is_dirty()));
        let page = self.ws().page_mut();
        match replace {
            Some(old) => {
                replace_leaf(&mut page.root, old, id);
                page.focused = id;
                if page.zoomed == Some(old) {
                    page.zoomed = Some(id);
                }
                self.panes.remove(&old);
            }
            None => {
                let f = page.focused;
                page.root.split(f, Dir::Horizontal, id);
                page.focused = id;
                page.zoomed = None;
            }
        }
        id
    }

    // ---------- 동작 ----------

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::NewWorkspace(Some(p)) => self.add_workspace(p, ctx),
            Action::NewWorkspace(None) => {
                if let Some(p) = rfd::FileDialog::new().set_title("스페이스로 열 폴더 선택").pick_folder() {
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
                    for p in ws.all_panes() {
                        self.drop_pane(p);
                    }
                    ws.tools.lsp.shutdown();
                    if self.workspaces.is_empty() {
                        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
                        self.add_workspace(home, ctx);
                    }
                    self.active = self.active.min(self.workspaces.len() - 1);
                }
            }
            Action::RenameWorkspace(i, name) => {
                if let Some(w) = self.workspaces.get_mut(i) {
                    if !name.trim().is_empty() {
                        w.name = name.trim().to_string();
                    }
                }
            }
            Action::NewPage => {
                let cwd = self.focused_cwd().or_else(|| self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned()));
                let pane = self.new_term_pane(cwd);
                let pid = self.id();
                let ws = self.ws();
                ws.pages.push(Page::new(pid, pane));
                ws.active_page = ws.pages.len() - 1;
                self.focus_terminal = true;
            }
            Action::SelectPage(i) => {
                let ws = self.ws();
                if i < ws.pages.len() {
                    ws.active_page = i;
                }
                self.focus_terminal = true;
            }
            Action::NextPage(d) => {
                let ws = self.ws();
                let n = ws.pages.len() as i32;
                ws.active_page = ((ws.active_page as i32 + d).rem_euclid(n)) as usize;
                self.focus_terminal = true;
            }
            Action::ClosePage(i, force) => {
                let Some(page) = self.workspaces[self.active].pages.get(i) else { return };
                let panes = page.root.panes();
                if !force {
                    let busy: Vec<String> = if self.settings.confirm_close_running { panes.iter().filter_map(|p| self.pane_is_busy(*p)).collect() } else { vec![] };
                    let dirty: Vec<String> = panes.iter().filter_map(|p| self.panes.get(p).and_then(|x| x.tool())).filter(|t| t.is_dirty()).map(|t| t.title()).collect();
                    if !busy.is_empty() || !dirty.is_empty() {
                        let mut parts = Vec::new();
                        if !busy.is_empty() {
                            parts.push(format!("실행 중: {}", busy.join(", ")));
                        }
                        if !dirty.is_empty() {
                            parts.push(format!("저장 안 됨: {}", dirty.join(", ")));
                        }
                        self.confirm = Some(Confirm { title: "페이지를 닫을까요?".into(), body: parts.join("\n"), ok: "페이지 닫기".into(), action: Action::ClosePage(i, true) });
                        return;
                    }
                }
                for p in panes {
                    self.drop_pane(p);
                }
                let ws = self.ws();
                ws.pages.remove(i);
                if ws.pages.is_empty() {
                    self.apply(Action::NewPage, ctx);
                } else if ws.active_page >= i && ws.active_page > 0 {
                    ws.active_page -= 1;
                }
            }
            Action::Split(dir) => {
                let f = self.ws().page().focused;
                self.apply(Action::SplitPane(f, dir), ctx);
            }
            Action::SplitPane(target, dir) => {
                let cwd = self
                    .panes
                    .get(&target)
                    .and_then(|p| p.session())
                    .and_then(|s| self.conn.infos.get(&s))
                    .and_then(|i| i.cwd.clone())
                    .or_else(|| self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned()));
                let pane = self.new_term_pane(cwd);
                let page = self.ws().page_mut();
                if page.root.split(target, dir, pane) {
                    page.focused = pane;
                    page.zoomed = None;
                }
                self.focus_terminal = true;
            }
            Action::CloseActive => {
                let p = self.ws().page().focused;
                self.apply(Action::ClosePane(p, false), ctx);
            }
            Action::ClosePane(p, force) => {
                if !force {
                    if let Some(proc_name) = self.pane_is_busy(p).filter(|_| self.settings.confirm_close_running) {
                        self.confirm = Some(Confirm {
                            title: format!("{proc_name} 을(를) 종료할까요?"),
                            body: "이 카드를 닫으면 실행 중인 프로세스도 함께 종료됩니다.".into(),
                            ok: "종료하고 닫기".into(),
                            action: Action::ClosePane(p, true),
                        });
                        return;
                    }
                    if let Some(t) = self.panes.get(&p).and_then(|x| x.tool()).filter(|t| t.is_dirty()) {
                        self.confirm = Some(Confirm {
                            title: "저장하지 않은 변경".into(),
                            body: format!("{} 의 변경을 버리고 닫을까요?", t.title()),
                            ok: "버리고 닫기".into(),
                            action: Action::ClosePane(p, true),
                        });
                        return;
                    }
                }
                self.drop_pane(p);
                let ws = self.ws();
                let mut empty_page = None;
                for (i, page) in ws.pages.iter_mut().enumerate() {
                    if page.root.panes().contains(&p) {
                        if page.zoomed == Some(p) {
                            page.zoomed = None;
                        }
                        if page.root.remove(p) {
                            if page.focused == p {
                                page.focused = page.root.panes()[0];
                            }
                        } else {
                            empty_page = Some(i);
                        }
                    }
                }
                if let Some(i) = empty_page {
                    ws.pages.remove(i);
                    if ws.pages.is_empty() {
                        self.apply(Action::NewPage, ctx);
                    }
                    let ws = self.ws();
                    ws.active_page = ws.active_page.min(ws.pages.len() - 1);
                }
                self.focus_terminal = true;
            }
            Action::FocusPane(p) => {
                self.ws().page_mut().focused = p;
                self.focus_terminal = true;
            }
            Action::Navigate(nav) => {
                let page = self.ws().page_mut();
                if let Some(n) = layout::neighbor(&page.rects, page.focused, nav) {
                    page.focused = n;
                }
                self.focus_terminal = true;
            }
            Action::Equalize => self.ws().page_mut().root.equalize(),
            Action::ToggleZoom(p) => {
                let page = self.ws().page_mut();
                let target = p.unwrap_or(page.focused);
                page.zoomed = if page.zoomed.is_some() { None } else { Some(target) };
                page.focused = target;
                self.focus_terminal = true;
            }
            Action::FontDelta(d) => {
                self.settings.font_size = if d == 0.0 { 13.5 } else { (self.settings.font_size + d).clamp(8.0, 32.0) };
            }
            Action::OpenLink(target) => match target {
                LinkTarget::Url(u) => {
                    let _ = open::that_detached(u);
                }
                LinkTarget::File { path, line, col } => {
                    if path.is_dir() {
                        self.actions.push(Action::NewTermAt(path));
                    } else {
                        self.actions.push(Action::OpenTab(tools::open_file_factory(path, line, col)));
                    }
                }
            },
            Action::RestartPane(p) => {
                if let Some(pane) = self.panes.get_mut(&p) {
                    let mut cwd = pane.cwd.clone();
                    if let PaneKind::Term { session, view, .. } = &mut pane.kind {
                        if let Some(s) = session.take() {
                            cwd = self.conn.infos.get(&s).and_then(|i| i.cwd.clone()).or(cwd);
                            self.conn.kill(s);
                        }
                        *view = None;
                    }
                    if let Some(pane) = self.panes.get_mut(&p) {
                        pane.cwd = cwd;
                    }
                }
                self.spawn_for_pane(p);
            }
            Action::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Action::ToggleSheet(kind) => {
                let ws = self.ws();
                if ws.sheet == Some(kind) {
                    ws.sheet = None;
                    self.focus_terminal = true;
                } else {
                    ws.sheet = Some(kind);
                    ws.tools.on_show(kind);
                }
            }
            Action::CloseSheet => {
                self.ws().sheet = None;
                self.focus_terminal = true;
            }
            Action::OpenPalette => self.palette.open(),
            Action::QuickOpen => self.ws().tools.quick_open(),
            Action::JumpUnread => {
                if let Some((_, sid)) = self.unread.pop() {
                    self.reveal_session(sid);
                }
            }
            Action::FindInFocused => {
                if let Some(p) = self.focused_pane() {
                    match self.panes.get_mut(&p).map(|x| &mut x.kind) {
                        Some(PaneKind::Term { view: Some(v), .. }) => v.open_search(),
                        Some(PaneKind::Tool(t)) => t.find(),
                        _ => {}
                    }
                }
            }
            Action::AttachSession(sid) => {
                let pane = self.id();
                self.panes.insert(pane, Pane { id: pane, kind: PaneKind::Term { session: Some(sid), pending: None, view: Some(TermView::new(sid)) }, cwd: None });
                let pid = self.id();
                let ws = self.ws();
                ws.pages.push(Page::new(pid, pane));
                ws.active_page = ws.pages.len() - 1;
                self.focus_terminal = true;
            }
            Action::KillSession(sid) => self.conn.kill(sid),
            Action::OpenSettings => self.settings_ui.open = true,
            Action::UpgradeDaemon => {
                let exe = conn::daemon_exe();
                self.conn.send(kiln_proto::ClientMsg::Upgrade { req: 0, exe: exe.to_string_lossy().into_owned() });
            }
            Action::OpenTab(factory) => {
                // 같은 대상이 이미 카드로 열려 있으면 그 카드로 간다.
                let found = self.workspaces[self.active].pages.iter().enumerate().find_map(|(pi, pg)| {
                    pg.root.panes().into_iter().find(|p| self.panes.get(p).and_then(|x| x.tool()).is_some_and(|t| t.key() == factory.key)).map(|p| (pi, p))
                });
                if let Some((pi, p)) = found {
                    let ws = self.ws();
                    ws.active_page = pi;
                    ws.pages[pi].focused = p;
                    if let Some(Pane { kind: PaneKind::Tool(t), .. }) = self.panes.get_mut(&p) {
                        factory.reuse(t.as_mut());
                    }
                    return;
                }
                let env = self.workspaces[self.active].tools.env();
                match factory.make(ctx, &env) {
                    Ok(tab) => {
                        self.place_card(PaneKind::Tool(tab));
                    }
                    Err(e) => self.toast("열 수 없습니다", e, ToastKind::Error, None),
                }
            }
            Action::CloseTabByKey(key) => {
                let found = self.workspaces[self.active].all_panes().into_iter().find(|p| self.panes.get(p).and_then(|x| x.tool()).is_some_and(|t| t.key() == key));
                if let Some(p) = found {
                    self.apply(Action::ClosePane(p, true), ctx);
                }
            }
            Action::RenamedFile(from, to) => {
                let key = format!("file:{}", from.display());
                let found = self.workspaces[self.active].all_panes().into_iter().find(|p| self.panes.get(p).and_then(|x| x.tool()).is_some_and(|t| t.key() == key && !t.is_dirty()));
                if let Some(p) = found {
                    self.apply(Action::ClosePane(p, true), ctx);
                    if to.is_file() {
                        self.apply(Action::OpenTab(tools::open_file_factory(to, None, None)), ctx);
                    }
                }
            }
            Action::NewTermAt(dir) => {
                let pane = self.new_term_pane(Some(dir.to_string_lossy().into_owned()));
                let page = self.ws().page_mut();
                let f = page.focused;
                page.root.split(f, Dir::Horizontal, pane);
                page.focused = pane;
                self.focus_terminal = true;
            }
            Action::RunInTerminal(cmd) => {
                let cwd = self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned());
                let pane = self.new_term_pane(cwd);
                let page = self.ws().page_mut();
                let f = page.focused;
                page.root.split(f, Dir::Vertical, pane);
                page.focused = pane;
                self.spawn_for_pane(pane);
                self.pending_input.insert(pane, format!("{cmd}\r"));
                self.focus_terminal = true;
            }
            Action::Toast(t) => self.toast(t, String::new(), ToastKind::Info, None),
            Action::RevealSession(s) => self.reveal_session(s),
            Action::SetTheme(name) => {
                Theme::set_current(&name);
                self.theme = Theme::current();
                self.theme.apply(ctx);
                self.settings.theme = name;
            }
        }
    }

    fn reveal_session(&mut self, sid: SessionId) {
        for (wi, ws) in self.workspaces.iter_mut().enumerate() {
            for (pi, page) in ws.pages.iter_mut().enumerate() {
                for p in page.root.panes() {
                    if self.panes.get(&p).and_then(|x| x.session()) == Some(sid) {
                        self.active = wi;
                        ws.active_page = pi;
                        page.focused = p;
                        self.focus_terminal = true;
                        return;
                    }
                }
            }
        }
    }

    fn toast(&mut self, title: impl Into<String>, body: impl Into<String>, kind: ToastKind, session: Option<SessionId>) {
        self.toasts.push(Toast { title: title.into(), body: body.into(), at: Instant::now(), kind, session });
    }

    // ---------- 이벤트 ----------

    fn handle_conn_events(&mut self, ctx: &egui::Context) {
        for e in std::mem::take(&mut self.conn.events) {
            match e {
                ConnEvent::Created { req, session } => {
                    if let Some(pid) = self.pending_creates.remove(&req) {
                        if let Some(Pane { kind: PaneKind::Term { session: s, pending, view }, .. }) = self.panes.get_mut(&pid) {
                            *s = Some(session);
                            *pending = None;
                            *view = Some(TermView::new(session));
                            if let Some(input) = self.pending_input.remove(&pid) {
                                self.conn.input(session, input.into_bytes());
                            }
                        }
                    }
                }
                ConnEvent::Notification { session, title, body } => {
                    let focused_here = self.window_focused && self.focused_session() == Some(session);
                    if !focused_here {
                        let id = self.id();
                        self.unread.push((id, session));
                        let who = self.conn.infos.get(&session).and_then(|i| i.fg_process.clone()).unwrap_or_else(|| "터미널".into());
                        let t = if title.is_empty() { who } else { title.clone() };
                        self.toast(t.clone(), body.clone(), ToastKind::Notify, Some(session));
                        if !self.window_focused {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                            if self.settings.os_notifications {
                                os_notify(&t, &body);
                            }
                        }
                    } else {
                        self.conn.send(kiln_proto::ClientMsg::ClearAttention { session });
                    }
                }
                ConnEvent::Exited { .. } => {}
                ConnEvent::Error(m) => self.toast("오류", m, ToastKind::Error, None),
                ConnEvent::Connected => {
                    if self.restored {
                        self.toast("데몬에 다시 연결됨", "", ToastKind::Info, None);
                    }
                }
                ConnEvent::Upgrading => self.toast("데몬을 새 버전으로 교체하는 중", "실행 중인 세션은 그대로 유지됩니다", ToastKind::Info, None),
                ConnEvent::SearchResult { found } => {
                    if let Some(p) = self.focused_pane() {
                        if let Some(Pane { kind: PaneKind::Term { view: Some(v), .. }, .. }) = self.panes.get_mut(&p) {
                            v.set_search_result(found);
                        }
                    }
                }
            }
        }
        // 세션 목록을 받은 뒤: 사라진 세션의 카드는 새 셸로 채운다.
        if self.conn.is_connected() && self.conn.sessions_listed {
            self.restored = true;
            let visible: Vec<PaneId> = self.workspaces.iter().flat_map(|w| w.all_panes()).collect();
            for p in visible {
                let needs = match self.panes.get(&p).map(|x| &x.kind) {
                    Some(PaneKind::Term { session, pending, .. }) => pending.is_none() && session.is_none_or(|s| !self.conn.exists(s)),
                    _ => false,
                };
                if needs {
                    if let Some(Pane { kind: PaneKind::Term { session, view, .. }, .. }) = self.panes.get_mut(&p) {
                        *session = None;
                        *view = None;
                    }
                    self.spawn_for_pane(p);
                }
            }
        }
        if !self.conn.is_connected() && !self.pending_creates.is_empty() {
            for (_, p) in self.pending_creates.drain() {
                if let Some(Pane { kind: PaneKind::Term { pending, .. }, .. }) = self.panes.get_mut(&p) {
                    *pending = None;
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let mac = cfg!(target_os = "macos");
        let base = if mac { Modifiers::MAC_CMD } else { Modifiers::CTRL | Modifiers::SHIFT };
        let base_shift = if mac { Modifiers::MAC_CMD | Modifiers::SHIFT } else { Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT };
        let base_alt = if mac { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::CTRL | Modifiers::ALT };
        use tools::ToolKind as T;
        let mut table: Vec<(Modifiers, Key, Action)> = vec![
            (base, Key::T, Action::NewPage),
            (base, Key::D, Action::Split(Dir::Horizontal)),
            (base_shift, Key::D, Action::Split(Dir::Vertical)),
            (base, Key::W, Action::CloseActive),
            (base, Key::N, Action::NewWorkspace(None)),
            (base, Key::P, Action::QuickOpen),
            (base, Key::K, Action::OpenPalette),
            (base_shift, Key::P, Action::OpenPalette),
            (base, Key::B, Action::ToggleSidebar),
            (base, Key::Equals, Action::FontDelta(1.0)),
            (base, Key::Plus, Action::FontDelta(1.0)),
            (base, Key::Minus, Action::FontDelta(-1.0)),
            (base, Key::Num0, Action::FontDelta(0.0)),
            (base, Key::F, Action::FindInFocused),
            (base_shift, Key::Enter, Action::ToggleZoom(None)),
            (base_shift, Key::E, Action::ToggleSheet(T::Explorer)),
            (base_shift, Key::F, Action::ToggleSheet(T::Search)),
            (base_shift, Key::G, Action::ToggleSheet(T::Git)),
            (base_shift, Key::R, Action::ToggleSheet(T::PullRequests)),
            (base_shift, Key::B, Action::ToggleSheet(T::Database)),
            (base_shift, Key::M, Action::ToggleSheet(T::Problems)),
            (base_shift, Key::U, Action::JumpUnread),
            (base_shift, Key::OpenBracket, Action::NextPage(-1)),
            (base_shift, Key::CloseBracket, Action::NextPage(1)),
            (base_alt, Key::ArrowLeft, Action::Navigate(Nav::Left)),
            (base_alt, Key::ArrowRight, Action::Navigate(Nav::Right)),
            (base_alt, Key::ArrowUp, Action::Navigate(Nav::Up)),
            (base_alt, Key::ArrowDown, Action::Navigate(Nav::Down)),
            (base_alt, Key::Equals, Action::Equalize),
            (base, Key::Comma, Action::OpenSettings),
        ];
        // consume_shortcut 은 추가 Shift/Alt 를 무시하므로 수식키가 많은 조합부터 검사한다.
        table.sort_by_key(|(m, _, _)| std::cmp::Reverse(m.shift as u8 + m.alt as u8 + m.ctrl as u8 + m.mac_cmd as u8));
        for (m, k, a) in table {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k))) {
                self.actions.push(a);
            }
        }
        let nums = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        let page_mod = if mac { Modifiers::MAC_CMD | Modifiers::ALT } else { Modifiers::ALT };
        for (i, k) in nums.iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_shortcut(&KeyboardShortcut::new(page_mod, *k))) {
                self.actions.push(Action::SelectPage(i));
            }
            if ctx.input_mut(|inp| inp.consume_shortcut(&KeyboardShortcut::new(base, *k))) {
                self.actions.push(Action::SelectWorkspace(i));
            }
        }
    }
}

fn replace_leaf(node: &mut Node, from: PaneId, to: PaneId) -> bool {
    match node {
        Node::Leaf(p) if *p == from => {
            *p = to;
            true
        }
        Node::Leaf(_) => false,
        Node::Split { a, b, .. } => replace_leaf(a, from, to) || replace_leaf(b, from, to),
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
        self.workspaces[self.active].page().root.panes().len()
    }

    /// 포커스된 카드 제목(터미널이면 "terminal").
    #[doc(hidden)]
    pub fn debug_active_tab_title(&self) -> String {
        match self.focused_pane().and_then(|p| self.panes.get(&p)).map(|p| &p.kind) {
            Some(PaneKind::Tool(t)) => t.title(),
            _ => "terminal".into(),
        }
    }

    #[doc(hidden)]
    pub fn debug_open_settings(&mut self, section: usize) {
        self.settings_ui.open = true;
        self.settings_ui.section = section;
    }

    #[doc(hidden)]
    pub fn debug_open_palette(&mut self) {
        self.palette.open();
    }

    #[doc(hidden)]
    pub fn debug_set_theme(&mut self, ctx: &egui::Context, name: &str) {
        self.apply(Action::SetTheme(name.into()), ctx);
    }

    #[doc(hidden)]
    pub fn debug_toast(&mut self, title: &str, body: &str) {
        self.toast(title, body, ToastKind::Notify, None);
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

pub(crate) fn short_path(p: &Path) -> String {
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
        if focused_now && !self.window_focused {
            let ids: Vec<PaneId> = self.workspaces.get(self.active).map(|w| w.all_panes()).unwrap_or_default();
            for p in ids {
                if let Some(Pane { kind: PaneKind::Tool(t), .. }) = self.panes.get_mut(&p) {
                    t.on_focus_regained();
                }
            }
        }
        self.window_focused = focused_now;
        self.handle_conn_events(ctx);
        let quick_open = self.workspaces.get(self.active).is_some_and(|w| w.tools.quick_is_open());
        if self.confirm.is_none() && !self.palette.is_open() && !quick_open && !self.settings_ui.open {
            self.shortcuts(ctx);
        }
        let active = self.active;
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            ws.tools.tick(i == active);
        }
        for p in self.panes.values_mut() {
            if let PaneKind::Tool(t) = &mut p.kind {
                t.tick();
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.theme = Theme::current();
        self.ui_topbar(ui);
        self.ui_spaces(ui);
        self.ui_canvas(ui);
        self.ui_sheet(ctx);
        self.ui_overlays(ctx);

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
