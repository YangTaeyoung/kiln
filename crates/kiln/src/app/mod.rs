//! Kiln GUI: 스페이스(워크스페이스) → 페이지 → 카드(터미널·에디터·DB 등) 분할 트리.

pub mod conn;
mod keys;
mod launchers;
pub mod projects;
mod product_ui;
mod keymap;
mod quick;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod updater;
mod notifications;
mod layout;
mod palette;
mod rotation;
mod settings;
mod state;
pub mod terminal;
mod tools;
mod workspace_repos;
mod workspace_activity;
mod agent_launch;
mod remote_terminal;
mod pane_layout;
mod agent_request;
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
    kiln_common::i18n::set_language(kiln_common::i18n::load_language());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Kiln")
        .with_inner_size([1400.0, 880.0])
        .with_min_inner_size([720.0, 440.0])
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../../../assets/Kiln.png")).unwrap_or_default());
    if cfg!(target_os = "macos") {
        // 제목 표시줄을 숨기고 내용이 창 위까지 올라가게 한다(신호등 버튼은 상단 바 위에 뜬다).
        viewport = viewport.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    // Honor an isolated Kiln configuration for eframe window geometry as well.
    // Preserve the existing default location for ordinary installations.
    let persistence_path = std::env::var_os("KILN_CONFIG_DIR")
        .map(|directory| PathBuf::from(directory).join("egui.ron"));
    let options = eframe::NativeOptions { viewport, persistence_path, ..Default::default() };
    eframe::run_native("Kiln", options, Box::new(move |cc| {
        #[cfg(target_os = "macos")]
        macos::install_quit_guard(&cc.egui_ctx);
        #[cfg(target_os = "macos")]
        updater::install();
        let mut app = KilnApp::new(&cc.egui_ctx, path);
        app.quick.register(&cc.egui_ctx);
        if let Some(error)=app.quick.error.clone(){app.toast(kiln_common::i18n::tr("빠른 터미널"),error,ToastKind::Error,None);}
        Ok(Box::new(app))
    }))
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
    pub manual_split: bool,
    pub root: Node,
    pub focused: PaneId,
    pub rects: Vec<(PaneId, Rect)>,
    pub zoomed: Option<PaneId>,
    pub title: Option<String>,
    pub agent_request: Option<PathBuf>,
    pub agent_request_offset: usize,
}

impl Page {
    fn new(id: u64, pane: PaneId) -> Page {
        Page { id, manual_split:false, root: Node::Leaf(pane), focused: pane, rects: vec![], zoomed: None, title: None, agent_request:None,agent_request_offset:0 }
    }
}

pub struct Workspace {
    pub id: u64,
    pub name: String,
    pub root: PathBuf,
    pub pages: Vec<Page>,
    pub active_page: usize,
    pub sheet: Option<tools::ToolKind>,
    pub last_inspector: tools::ToolKind,
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
    CloseWorkspaceConfirmed(usize),
    QuitConfirmed,
    QuitPreservingDrafts,
    RenameWorkspace(usize, String),
    NewPage,
    OpenFolder,
    NewAgentTask,
    DirectAgent { tool: kiln_accounts::Tool, cwd: Option<PathBuf> },
    OpenAgentFolder(kiln_accounts::Tool),
    OpenSsh { alias: String, config_path: Option<PathBuf> },
    SelectPage(usize),
    ClosePage(usize, bool),
    NextPage(i32),
    Split(Dir),
    SplitPane(PaneId, Dir),
    ClosePane(PaneId, bool),
    CloseActive,
    FocusPane(PaneId),
    RevealPane(PaneId),
    OpenRecent,
    OpenLaunchers,
    OpenProjects,
    OpenRecovery,
    ToggleQuickTerminal,
    ReturnQuickTerminal,
    RunSavedCommand(launchers::SelectedCommand),
    Navigate(Nav),
    Equalize,
    Arrange(Dir),
    ArrangeGrid,
    ToggleAutoFocus,
    ToggleFullscreen,
    ToggleZoom(Option<PaneId>),
    FontDelta(f32),
    OpenLink(LinkTarget),
    RestartPane(PaneId),
    RetryTerminalLaunch(PaneId),
    DiscardTerminalLaunch(PaneId),
    ToggleSidebar,
    ToggleSheet(tools::ToolKind),
    OpenSheet(tools::ToolKind),
    ToggleInspector,
    CloseSheet,
    OpenPalette,
    QuickOpen,
    JumpUnread,
    ToggleNotifications,
    FindInFocused,
    AttachSession(SessionId),
    KillSession(SessionId),
    OpenSettings,
    SetLanguage(kiln_common::i18n::Language),
    OpenTerminalSettings,
    ShowAgentRequest(usize),
    CopyAgentRequest(usize),
    UpgradeDaemon,
    OpenTab(tools::TabFactory),
    CloseTabByKey(String),
    RenamedFile(PathBuf, PathBuf),
    NewTermAt(PathBuf),
    RunInTerminal(String),
    RunInTerminalAt { cwd: PathBuf, command: String },
    LaunchAgent { cwd: PathBuf, program: String, context: String, request: String },
    Toast(String),
    SetTheme(String),
    RevealSession(SessionId),
    /// 계정 로그인 명령을 새 터미널 카드에서 실행한다.
    RunLogin(kiln_accounts::Tool),
    /// 다음 계정으로 바꾸고 세션의 에이전트를 이어서 실행한다.
    RotateAccount(SessionId, kiln_accounts::Tool),
    /// GitHub 저장소(`owner/name`)를 고른 폴더 아래로 복제하고 새 스페이스로 연다.
    CloneRepo(String),
    /// 현재 스페이스 저장소의 Git 로그 카드를 연다.
    OpenHistory,
}

pub(crate) struct Toast {
    title: String,
    body: String,
    at: Instant,
    kind: ToastKind,
    session: Option<SessionId>,
    /// 알림 안의 버튼(이름, 동작). 누르면 동작을 실행하고 알림을 닫는다.
    button: Option<(String, Action)>,
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
    /// Present only for a running-process panel close; never mutate settings on cancel.
    skip_running_confirmation: Option<bool>,
}

pub struct KilnApp {
    conn: Conn,
    workspaces: Vec<Workspace>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    pending_creates: HashMap<u32, PaneId>,
    launch_specs: HashMap<PaneId, SpawnSpec>,
    pane_layout: pane_layout::PaneLayout,
    cancelled_creates: std::collections::HashSet<u32>,
    cancelled_sessions: std::collections::HashSet<SessionId>,
    terminal_launch_drafts: HashMap<PaneId, state::TerminalLaunchDraft>,
    next_id: u64,
    settings: Settings,
    sidebar_open: bool,
    toasts: Vec<Toast>,
    confirm: Option<Confirm>,
    quit_confirmed: bool,
    palette: palette::Palette,
    recent_panes: Vec<PaneId>,
    launchers: launchers::Launcher,
    projects: projects::Projects,
    keymap: keymap::Keymap,
    quick: quick::QuickTerminal,
    recovery_open: bool,
    recovery_messages: Vec<String>,
    save_error: Option<String>,
    rename_page: Option<(usize, String)>,
    settings_ui: settings::SettingsUi,
    notifications: notifications::NotificationCenter,
    last_saved: Option<Persist>,
    last_save_at: Instant,
    focus_terminal: bool,
    restored: bool,
    window_focused: bool,
    actions: Vec<Action>,
    theme: Theme,
    pending_input: HashMap<PaneId, String>,
    pending_agent_prompts: HashMap<PaneId, String>,
    agent_request_view: Option<agent_request::RequestView>,
    db: kiln_db::DbManager,
    rotator: rotation::Rotator,
    clones: (std::sync::mpsc::Sender<Result<PathBuf, String>>, std::sync::mpsc::Receiver<Result<PathBuf, String>>),
}

pub(crate) fn shells() -> &'static [&'static str] {
    &["zsh", "bash", "fish", "sh", "dash", "tcsh", "csh", "nu", "pwsh", "powershell", "cmd", "login", "-zsh", "-bash"]
}

fn is_inspector(kind:tools::ToolKind)->bool {
    matches!(kind,tools::ToolKind::Explorer|tools::ToolKind::Search|tools::ToolKind::Git|tools::ToolKind::PullRequests|tools::ToolKind::Database)
}

fn restored_inspector(last:Option<&str>,sheet:Option<&str>)->tools::ToolKind {
    last.into_iter().chain(sheet).find_map(|value| match value {
        "explorer"|"search"|"git"|"prs"|"db"=>Some(tools::ToolKind::from_str(value)),
        _=>None,
    }).unwrap_or(tools::ToolKind::Explorer)
}

#[cfg(test)]
mod inspector_state_tests {
    use super::*;
    #[test]
    fn remembered_inspector_restores_without_inheriting_secondary_tools() {
        use tools::ToolKind::*;
        assert_eq!(restored_inspector(Some("prs"),Some("db")),PullRequests);
        assert_eq!(restored_inspector(None,Some("git")),Git);
        assert_eq!(restored_inspector(Some("db"),Some("search")),Database);
        assert_eq!(restored_inspector(None,Some("db")),Database);
        assert_eq!(restored_inspector(Some("unknown"),Some("problems")),Explorer);
        assert_eq!(restored_inspector(None,None),Explorer);
        let old:state::WorkspaceP=serde_json::from_str(r#"{"name":"parent","root":"/workspace","sheet":"git"}"#).unwrap();
        assert_eq!(old.last_inspector,None);
        assert_eq!(restored_inspector(old.last_inspector.as_deref(),old.sheet.as_deref()),Git);
    }
}

impl KilnApp {
    pub fn new(ctx: &egui::Context, open_path: Option<PathBuf>) -> Self {
        kiln_common::i18n::set_language(kiln_common::i18n::load_language());
        kiln_common::fonts::install(ctx);
        let load_report = state::load_with_report();
        let persisted = load_report.state;
        let first_run = persisted.workspaces.is_empty() && open_path.is_none();
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
            launch_specs: HashMap::new(),
            pane_layout: Default::default(),
            cancelled_creates: Default::default(),
            cancelled_sessions: Default::default(),
            terminal_launch_drafts: HashMap::new(),
            next_id: 1,
            settings: persisted.settings.clone(),
            sidebar_open: persisted.sidebar_open || persisted.workspaces.is_empty(),
            toasts: Vec::new(),
            confirm: None,
            quit_confirmed: false,
            palette: palette::Palette::default(),
            recent_panes: persisted.recent_panes.clone(),
            launchers: launchers::Launcher::default(),
            projects: projects::Projects::default(),
            keymap: keymap::Keymap::load(),
            quick: quick::QuickTerminal::default(),
            recovery_open: load_report.warning.is_some(),
            recovery_messages: load_report.warning.into_iter().collect(),
            save_error: None,
            rename_page: None,
            settings_ui: settings::SettingsUi::default(),
            notifications: notifications::NotificationCenter::new(persisted.notifications.clone()),
            last_saved: None,
            last_save_at: Instant::now(),
            focus_terminal: true,
            restored: false,
            window_focused: true,
            actions: Vec::new(),
            theme,
            pending_input: HashMap::new(),
            pending_agent_prompts: HashMap::new(),
            agent_request_view:None,
            db: kiln_db::DbManager::load(),
            rotator: rotation::Rotator::new({
                let m = match std::env::var_os("KILN_ACCOUNTS_SANDBOX") {
                    Some(dir) => kiln_accounts::AccountManager::with_env(kiln_accounts::Env::sandbox(Path::new(&dir), false).0),
                    None => kiln_accounts::AccountManager::load(),
                };
                m.set_repaint_context(ctx);
                m
            }),
            clones: std::sync::mpsc::channel(),
        };
        app.restore(persisted, ctx);
        if !app.recovery_messages.is_empty() && state::path().is_file() {
            let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let backup=kiln_common::paths::config_file(&format!("state.recovery-{stamp}.json"));
            match std::fs::copy(state::path(),&backup) {
                Ok(_)=>app.recovery_messages.push(kiln_common::trf!("복원 전 상태 보관: {}",backup.display())),
                Err(error)=>app.recovery_messages.push(kiln_common::trf!("복원 전 상태 백업 실패: {error}")),
            }
        }
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
        if first_run { let root = app.workspaces[app.active].root.clone(); app.projects.open(&root); }
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
                last_inspector: restored_inspector(wp.last_inspector.as_deref(),wp.sheet.as_deref()),
                tools: tools::WorkspaceTools::new(&wp.root, ctx, self.db.clone()),
                renaming: None,
            };
            ws.tools.restore_drafts(&wp.drafts);
            for pg in &wp.pages {
                let mut root = pg.root.clone();
                let mut alive = true;
                for pp in &pg.panes {
                    let kind = match &pp.tool {
                        Some(t) => match ws.tools.restore_tool(t, ctx) {
                            Some(tab) => PaneKind::Tool(tab),
                            None => {
                                self.recovery_messages.push(kiln_common::trf!("{} · 패널 {}를 복원하지 못했습니다. 파일 또는 DB 연결을 확인하세요.", wp.name, pp.id));
                                self.recovery_open=true;
                                // 복원할 수 없는 도구 카드(지워진 파일 등)는 빼고, 페이지가 비면 페이지를 버린다.
                                if !root.remove(pp.id) {
                                    alive = false;
                                }
                                continue;
                            }
                        },
                        None => PaneKind::Term { session: pp.session, pending: None, view: pp.session.map(TermView::new) },
                    };
                    if pp.tool.is_none() {
                        if let Some(spec) = &pp.launch { self.launch_specs.insert(pp.id, spec.clone()); }
                    }
                    self.panes.insert(pp.id, Pane { id: pp.id, kind, cwd: pp.cwd.clone() });
                }
                let panes = root.panes();
                if alive && !panes.is_empty() && panes.iter().all(|x| self.panes.contains_key(x)) {
                    let focused = if panes.contains(&pg.focused) { pg.focused } else { panes[0] };
                    let pid = self.id();
                    ws.pages.push(Page { id: pid, manual_split:pg.manual_split, zoomed:pg.zoomed.filter(|id|panes.contains(id)), root, focused, rects:vec![],title:pg.title.clone(),agent_request:pg.agent_request.clone(),agent_request_offset:pg.agent_request_offset });
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
        for draft in p.terminal_launch_drafts {
            if self.panes.get(&draft.pane).is_some_and(|p| matches!(p.kind, PaneKind::Term { .. })) {
                self.terminal_launch_drafts.insert(draft.pane, draft);
                self.recovery_open = true;
            }
        }
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
                last_inspector: Some(w.last_inspector.as_str().to_owned()),
                drafts: w.tools.drafts(),
                pages: w
                    .pages
                    .iter()
                    .map(|pg| PageP {
                        manual_split:pg.manual_split,
                        zoomed:pg.zoomed,
                        root: pg.root.clone(),
                        focused: pg.focused,
                        title: pg.title.clone(),
                        agent_request:pg.agent_request.clone(),agent_request_offset:pg.agent_request_offset,
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
                                launch: self.launch_specs.get(&p.id).cloned(),
                            })
                            .collect(),
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();
        let mut launch_drafts: std::collections::BTreeMap<_, _> = self.terminal_launch_drafts.iter().map(|(id,draft)| (*id,draft.clone())).collect();
        for (pane, command) in &self.pending_input {
            launch_drafts.insert(*pane, state::TerminalLaunchDraft { pane:*pane, command:Some(command.trim_end_matches('\r').to_owned()), reason:kiln_common::i18n::tr("실행 확인 전 종료된 요청입니다. 실행 여부를 확인하고 다시 시작하세요.").into() });
        }
        Persist { workspaces, active: self.active, settings: self.settings.clone(), sidebar_open: self.sidebar_open, notifications: self.notifications.items.clone(), recent_panes: self.recent_panes.clone(), terminal_launch_drafts:launch_drafts.into_values().collect() }
    }

    fn save_if_changed(&mut self, force: bool) {
        if !force && self.last_save_at.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_save_at = Instant::now();
        let p = self.persist();
        if self.last_saved.as_ref() != Some(&p) {
            match state::save(&p) {
                Ok(()) => { self.last_saved = Some(p); self.save_error = None; }
                Err(error) => {
                    if self.save_error.as_ref() != Some(&error) { self.toast(kiln_common::i18n::tr("작업 상태 저장 실패"), &error, ToastKind::Error, None); }
                    self.save_error = Some(error);
                }
            }
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
        if self.terminal_launch_drafts.contains_key(&pane) { return; }
        let shell = self.settings.shell.clone();
        let ws_info = self.workspace_of(pane).map(|w| (w.root.to_string_lossy().into_owned(), w.name.clone()));
        let connected = self.conn.is_connected();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let cwd = p.cwd.clone();
        let PaneKind::Term { pending, .. } = &mut p.kind else { return };
        if pending.is_some() || !connected {
            return;
        }
        let mut spec = self.launch_specs.get(&pane).cloned().unwrap_or_else(|| SpawnSpec {
            cwd: cwd.or_else(|| ws_info.as_ref().map(|w| w.0.clone())),
            program: if shell.is_empty() { None } else { Some(shell) },
            cols: 100,
            rows: 30,
            workspace: ws_info.as_ref().map(|w| w.1.clone()),
            ..Default::default()
        });
        if spec.workspace.is_none() { spec.workspace = ws_info.map(|w| w.1); }
        if let Some(req) = self.conn.create(spec) {
            *pending = Some(req);
            self.pending_creates.insert(req, pane);
        }
    }

    fn launch_terminal_page(&mut self, spec: SpawnSpec, ctx: &egui::Context) {
        let pane = self.new_term_pane(spec.cwd.clone());
        let pid = self.id();
        let mut page = Page::new(pid, pane);
        // Agent tasks keep following their live CLI title. SSH aliases remain
        // stable because a remote shell may publish only its working directory.
        page.title = spec.name.clone().filter(|name|name != "Codex" && name != "Claude Code");
        self.launch_specs.insert(pane, spec);
        let ws = self.ws();
        ws.pages.push(page);
        ws.active_page = ws.pages.len() - 1;
        self.spawn_for_pane(pane);
        self.focus_terminal = true;
        self.reveal_work_surface(ctx);
    }

    fn add_workspace(&mut self, root: PathBuf, ctx: &egui::Context) {
        let root = normalize_path(root.canonicalize().unwrap_or(root));
        if let Some(index) = self.workspaces.iter().position(|ws| normalize_path(ws.tools.canonical_root().to_owned()) == root) {
            self.active = index;
            self.focus_terminal = true;
            self.reveal_work_surface(ctx);
            return;
        }
        if !root.is_dir() {
            self.toast(kiln_common::i18n::tr("폴더를 열 수 없습니다"), root.display().to_string(), ToastKind::Error, None);
            return;
        }
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
            last_inspector: tools::ToolKind::Explorer,
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
        self.workspaces.get(self.active).and_then(|w| w.pages.get(w.active_page)).map(|page| page.focused)
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
        self.pending_input.remove(&p);
        self.launch_specs.remove(&p);
        self.pending_agent_prompts.remove(&p);
        self.terminal_launch_drafts.remove(&p);
        let requests: Vec<_> = self.pending_creates.iter().filter_map(|(req,pane)| (*pane==p).then_some(*req)).collect();
        for req in requests { self.pending_creates.remove(&req); self.cancelled_creates.insert(req); }
        if let Some(pane) = self.panes.remove(&p) {
            if let Some(s) = pane.session() {
                self.conn.kill(s);
            }
        }
    }

    fn block_disconnected_close(&mut self, panes: &[PaneId]) -> bool {
        if !self.conn.is_connected() && panes.iter().any(|id| self.panes.get(id).and_then(Pane::session).is_some()) {
            self.toast(kiln_common::i18n::tr("연결 후 종료할 수 있습니다"), kiln_common::i18n::tr("세션을 종료하지 못해 패널을 유지했습니다. 연결이 복구되면 다시 닫으세요."), ToastKind::Error, None);
            return true;
        }
        false
    }

    fn retain_terminal_launch(&mut self, pane: PaneId, reason: String) {
        let command = self.pending_input.remove(&pane).map(|input| input.trim_end_matches('\r').to_owned());
        self.terminal_launch_drafts.insert(pane, state::TerminalLaunchDraft { pane, command, reason });
        self.recovery_open = true;
    }

    pub(crate) fn terminal_launch_error(&self, pane: PaneId) -> Option<&str> {
        self.terminal_launch_drafts.get(&pane).map(|draft| draft.reason.as_str())
    }

    /// Open a tool without evicting another saved document. The first tool sits
    /// beside a terminal; subsequent tools get persistent tasks of their own.
    fn place_card(&mut self, kind: PaneKind) -> PaneId {
        let has_tool = self.ws().page().root.panes().iter().any(|p| self.panes.get(p).and_then(Pane::tool).is_some());
        let id = self.id();
        self.panes.insert(id, Pane { id, kind, cwd: None });
        if has_tool {
            let page_id = self.id();
            let ws = self.ws();
            ws.pages.push(Page::new(page_id, id));
            ws.active_page = ws.pages.len() - 1;
        } else {
            let page = self.ws().page_mut();
            let focused = page.focused;
            page.root.split(focused, Dir::Horizontal, id);
            page.focused = id;
            page.zoomed = None;
        }
        id
    }

    // ---------- 동작 ----------

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        self.cancel_pane_layout();
        match a {
            Action::NewAgentTask => {
                self.workspaces[self.active].tools.open_agent_task();
                self.workspaces[self.active].sheet=Some(tools::ToolKind::Git);
                self.focus_terminal=false;
            }
            Action::OpenFolder => {
                let root = self.workspaces[self.active].root.clone();
                if let Some(path) = rfd::FileDialog::new().set_title(kiln_common::i18n::tr("작업 폴더 열기")).set_directory(root).pick_folder() {
                    self.add_workspace(path,ctx);
                }
            }
            Action::NewWorkspace(Some(p)) => self.add_workspace(p, ctx),
            Action::NewWorkspace(None) | Action::OpenProjects => {
                let root = self.workspaces[self.active].root.clone();
                self.projects.open(&root);
            }
            Action::OpenRecovery => self.recovery_open = true,
            Action::ReturnQuickTerminal => { self.quick.open=false; self.quick.view=None; if let Some(pane)=self.quick.pane {self.reveal_pane(pane);self.reveal_work_surface(ctx);} },
            Action::ToggleQuickTerminal => { self.quick.open = !self.quick.open; self.quick.focus_pending=self.quick.open; },
            Action::SelectWorkspace(i) => {
                if i < self.workspaces.len() {
                    self.active = i;
                    self.focus_terminal = true;
                }
            }
            Action::CloseWorkspace(i) => {
                if let Some(ws) = self.workspaces.get(i) {
                    let dirty = self.unsaved_items(Some(i));
                    let busy:Vec<String>=if self.settings.confirm_close_running{ws.all_panes().iter().filter_map(|p|self.pane_is_busy(*p)).collect()}else{vec![]};
                    if !dirty.is_empty() || !busy.is_empty() {
                        self.confirm = Some(Confirm { skip_running_confirmation: None,
                            title: kiln_common::trf!("{} 작업 공간을 닫을까요?", ws.name),
                            body: kiln_common::trf!("이 작업 공간의 터미널 세션이 종료됩니다.\n실행 중: {}\n저장하지 않은 변경:\n{}", if busy.is_empty(){kiln_common::i18n::tr("없음").into()}else{busy.join(", ")},if dirty.is_empty(){kiln_common::i18n::tr("없음").into()}else{dirty.join("\n")}),
                            ok: if dirty.is_empty() { kiln_common::i18n::tr("실행 종료 후 작업 공간 닫기") } else { kiln_common::i18n::tr("변경 버리고 작업 공간 닫기") }.into(),
                            action: Action::CloseWorkspaceConfirmed(i),
                        });
                    } else {
                        self.apply(Action::CloseWorkspaceConfirmed(i), ctx);
                    }
                }
            }
            Action::QuitPreservingDrafts => {
                #[cfg(feature = "updater-test")]
                crate::updater_fixture_event("quit-preserving-drafts");
                self.save_if_changed(true);
                if self.save_error.is_none(){ self.quit_confirmed=true; ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                else { self.recovery_open=true; #[cfg(target_os="macos")] macos::reply_to_termination(false); }
            }
            Action::QuitConfirmed => {
                #[cfg(feature = "updater-test")]
                crate::updater_fixture_event("quit-discarding-drafts");
                let launch_drafts = std::mem::take(&mut self.terminal_launch_drafts);
                let pending_input = std::mem::take(&mut self.pending_input);
                for pane in self.panes.values_mut() { if let PaneKind::Tool(tool) = &mut pane.kind { tool.discard_recovery(true); } }
                for ws in &mut self.workspaces { ws.tools.discard_recovery(true); }
                self.save_if_changed(true);
                if self.save_error.is_none() {
                    self.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.terminal_launch_drafts = launch_drafts;
                    self.pending_input = pending_input;
                    // A failed checkpoint must not permanently suppress live drafts.
                    for pane in self.panes.values_mut() {
                        if let PaneKind::Tool(tool) = &mut pane.kind { tool.discard_recovery(false); }
                    }
                    for ws in &mut self.workspaces { ws.tools.discard_recovery(false); }
                    self.recovery_open = true;
                    #[cfg(target_os="macos")] macos::reply_to_termination(false);
                }
            }
            Action::CloseWorkspaceConfirmed(i) => {
                if i < self.workspaces.len() {
                    if self.block_disconnected_close(&self.workspaces[i].all_panes()) { return; }
                    let ws = self.workspaces.remove(i);
                    if i < self.active { self.active -= 1; }
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
            Action::OpenAgentFolder(tool) => {
                let root = self.ws().root.clone();
                let Some(cwd) = rfd::FileDialog::new().set_title(kiln_common::i18n::tr("작업 폴더 열기")).set_directory(root).pick_folder() else { return; };
                let cwd = normalize_path(cwd.canonicalize().unwrap_or(cwd));
                let spec = match remote_terminal::agent(tool, cwd.clone()) {
                    Ok(spec) => spec,
                    Err(error) => { self.toast(kiln_common::i18n::tr("작업을 시작할 수 없습니다"), error, ToastKind::Error, None); return; }
                };
                let existing = self.workspaces.iter().any(|ws| normalize_path(ws.tools.canonical_root().to_owned()) == cwd);
                self.add_workspace(cwd, ctx);
                if existing { self.launch_terminal_page(spec, ctx); }
                else if let Some(pane) = self.focused_pane() {
                    self.ws().page_mut().title = None;
                    self.launch_specs.insert(pane, spec);
                    self.spawn_for_pane(pane);
                    self.focus_terminal = true;
                    self.reveal_work_surface(ctx);
                }
            }
            Action::DirectAgent { tool, cwd } => {
                let cwd = cwd.unwrap_or_else(|| self.ws().root.clone());
                let spec = match remote_terminal::agent(tool, cwd) {
                    Ok(spec) => spec,
                    Err(error) => { self.toast(kiln_common::i18n::tr("작업을 시작할 수 없습니다"), error, ToastKind::Error, None); return; }
                };
                self.launch_terminal_page(spec, ctx);
            }
            Action::OpenSsh { alias, config_path } => {
                let mut spec = match remote_terminal::ssh(&alias, config_path.as_deref()) {
                    Ok(spec) => spec,
                    Err(error) => { self.toast(kiln_common::i18n::tr("SSH 터미널을 열 수 없습니다"), error, ToastKind::Error, None); return; }
                };
                spec.cwd = Some(self.ws().root.to_string_lossy().into_owned());
                self.launch_terminal_page(spec, ctx);
            }
            Action::NewPage => {
                let cwd = self.focused_cwd().or_else(|| self.workspaces.get(self.active).map(|w| w.root.to_string_lossy().into_owned()));
                let pane = self.new_term_pane(cwd);
                let pid = self.id();
                let ws = self.ws();
                ws.pages.push(Page::new(pid, pane));
                ws.active_page = ws.pages.len() - 1;
                self.focus_terminal = true;
                self.reveal_work_surface(ctx);
            }
            Action::SelectPage(i) => {
                let ws = self.ws();
                if i < ws.pages.len() {
                    ws.active_page = i;
                }
                self.focus_terminal = true;
                self.reveal_work_surface(ctx);
            }
            Action::NextPage(d) => {
                let ws = self.ws();
                let n = ws.pages.len() as i32;
                ws.active_page = ((ws.active_page as i32 + d).rem_euclid(n)) as usize;
                self.focus_terminal = true;
                self.reveal_work_surface(ctx);
            }
            Action::ClosePage(i, force) => {
                let Some(page) = self.workspaces[self.active].pages.get(i) else { return };
                let panes = page.root.panes();
                if self.block_disconnected_close(&panes) { return; }
                if !force {
                    let busy: Vec<String> = if self.settings.confirm_close_running { panes.iter().filter_map(|p| self.pane_is_busy(*p)).collect() } else { vec![] };
                    let mut dirty: Vec<String> = panes.iter().filter_map(|p| self.panes.get(p).and_then(|x| x.tool())).filter(|t| t.is_dirty()).map(|t| t.title()).collect();
                    if panes.iter().any(|p| self.terminal_launch_drafts.get(p).is_some_and(|d|d.command.is_some())) { dirty.push(kiln_common::i18n::tr("보관한 터미널 실행 요청").into()); }
                    if !busy.is_empty() || !dirty.is_empty() {
                        let mut parts = Vec::new();
                        if !busy.is_empty() {
                            parts.push(kiln_common::trf!("종료할 프로세스: {}", busy.join(", ")));
                        }
                        if !dirty.is_empty() {
                            parts.push(kiln_common::trf!("버릴 변경: {}", dirty.join(", ")));
                        }
                        self.confirm = Some(Confirm { skip_running_confirmation: None, title: kiln_common::i18n::tr("작업 탭을 닫을까요?").into(), body: kiln_common::trf!("{}\n이 탭의 패널과 터미널 세션이 모두 닫힙니다.",parts.join("\n")), ok: if dirty.is_empty(){kiln_common::i18n::tr("실행 종료 후 탭 닫기")}else{kiln_common::i18n::tr("변경 버리고 탭 닫기")}.into(), action: Action::ClosePage(i, true) });
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
                if self.block_disconnected_close(&[p]) { return; }
                if !force {
                    if self.terminal_launch_drafts.get(&p).is_some_and(|draft|draft.command.is_some()) {
                        self.confirm=Some(Confirm { skip_running_confirmation: None, title:kiln_common::i18n::tr("보관한 실행 요청을 버릴까요?").into(), body:kiln_common::i18n::tr("이 패널의 실행 요청이 삭제됩니다.").into(), ok:kiln_common::i18n::tr("요청 버리고 닫기").into(), action:Action::ClosePane(p,true) });
                        return;
                    }
                    if let Some(proc_name) = self.pane_is_busy(p).filter(|_| self.settings.confirm_close_running) {
                        self.confirm = Some(Confirm { skip_running_confirmation: Some(false),
                            title: kiln_common::i18n::tr("실행 중인 프로세스를 종료할까요?").into(),
                            body: kiln_common::trf!("프로세스: {proc_name}\n이 패널을 닫으면 위 프로세스도 함께 종료됩니다."),
                            ok: kiln_common::i18n::tr("종료하고 닫기").into(),
                            action: Action::ClosePane(p, true),
                        });
                        return;
                    }
                    if let Some(t) = self.panes.get(&p).and_then(|x| x.tool()).filter(|t| t.is_dirty()) {
                        self.confirm = Some(Confirm { skip_running_confirmation: None,
                            title: kiln_common::i18n::tr("저장하지 않은 변경").into(),
                            body: kiln_common::trf!("대상: {}\n저장하지 않은 변경을 버리고 패널을 닫습니다.", t.title()),
                            ok: kiln_common::i18n::tr("버리고 닫기").into(),
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
            Action::ToggleFullscreen => {
                let fullscreen=ctx.input(|input|input.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            }
            Action::ToggleAutoFocus => { let page=self.ws().page_mut();page.manual_split=!page.manual_split; }
            Action::ArrangeGrid => {
                let page=self.ws().page_mut();
                if let Some(root)=Node::grid(&page.root.panes()){page.root=root;page.zoomed=None;}
            }
            Action::Arrange(dir) => {
                let page = self.ws().page_mut();
                let panes = page.root.panes();
                if let Some(root) = Node::arranged(&panes, dir) { page.root = root; page.zoomed = None; }
            }
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
                if self.block_disconnected_close(&[p]) { return; }
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
            Action::RetryTerminalLaunch(p) => {
                if !self.conn.is_connected() {
                    self.toast(kiln_common::i18n::tr("연결을 기다리는 중입니다"), kiln_common::i18n::tr("연결이 복구되면 다시 실행하세요. 요청은 보관되어 있습니다."), ToastKind::Info, None);
                    return;
                }
                if let Some(draft) = self.terminal_launch_drafts.remove(&p) {
                    if let Some(command) = draft.command { self.pending_input.insert(p,format!("{command}\r")); }
                    self.reveal_pane(p);
                    self.reveal_work_surface(ctx);
                    self.spawn_for_pane(p);
                }
            }
            Action::DiscardTerminalLaunch(p) => {
                self.launch_specs.remove(&p);
                self.pending_agent_prompts.remove(&p);
                self.terminal_launch_drafts.remove(&p);
                self.pending_input.remove(&p);
                self.spawn_for_pane(p);
            }
            Action::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Action::ToggleSheet(kind) => {
                if self.ws().sheet == Some(kind) && !self.ws().tools.is_agent_task_open() {
                    self.apply(Action::CloseSheet,ctx);
                } else { self.apply(Action::OpenSheet(kind),ctx); }
            }
            Action::OpenSheet(kind) => {
                let ws=self.ws();
                ws.tools.close_agent_task();
                ws.sheet=Some(kind);
                if is_inspector(kind) { ws.last_inspector=kind; }
                ws.tools.on_show(kind);
                self.focus_terminal=false;
            }
            Action::ToggleInspector => {
                let ws=self.ws();
                if ws.sheet.is_some_and(is_inspector) && !ws.tools.is_agent_task_open() {
                    self.apply(Action::CloseSheet,ctx);
                } else {
                    let kind=ws.last_inspector;
                    self.apply(Action::OpenSheet(kind),ctx);
                }
            }
            Action::CloseSheet => {
                self.ws().sheet = None;
                self.focus_terminal = true;
            }
            Action::OpenPalette => self.palette.open(),
            Action::OpenRecent => self.palette.open_recent(self.focused_pane().map(|id| format!("pane:{id}"))),
            Action::OpenLaunchers => self.launchers.open(),
            Action::RevealPane(p) => {
                self.reveal_pane(p);
                self.reveal_work_surface(ctx);
            },
            Action::RunSavedCommand(command) => {
                let cwd = command.cwd.unwrap_or_else(|| self.workspaces[self.active].root.clone());
                if !cwd.is_dir() {
                    self.toast(kiln_common::i18n::tr("명령을 실행할 수 없습니다"), kiln_common::i18n::tr("실행 폴더를 찾을 수 없습니다. 실행할 폴더를 다시 선택하세요."), ToastKind::Error, None);
                } else {
                    let pane = self.new_term_pane(Some(cwd.to_string_lossy().into_owned()));
                    let page_id = self.id();
                    let mut page = Page::new(page_id, pane);
                    page.title = Some(command.name);
                    self.ws().pages.push(page);
                    self.ws().active_page = self.ws().pages.len() - 1;
                    self.spawn_for_pane(pane);
                    self.pending_input.insert(pane, format!("{}\r", command.command));
                    self.focus_terminal = true;
                    self.reveal_work_surface(ctx);
                }
            },
            Action::QuickOpen => self.ws().tools.quick_open(),
            Action::ToggleNotifications => self.notifications.open = !self.notifications.open,
            Action::JumpUnread => {
                let session = self.notifications.items.iter().rev().find(|n| !n.read && n.session.is_some_and(|s| self.conn.exists(s))).and_then(|n| n.session);
                if let Some(sid) = session {
                    if self.panes.values().any(|pane| pane.session() == Some(sid)) { self.reveal_session(sid); self.reveal_work_surface(ctx); }
                    else { self.actions.push(Action::AttachSession(sid)); }
                } else { self.notifications.open = true; }
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
                self.acknowledge_session(sid);
                let pane = self.id();
                self.panes.insert(pane, Pane { id: pane, kind: PaneKind::Term { session: Some(sid), pending: None, view: Some(TermView::new(sid)) }, cwd: None });
                let pid = self.id();
                let ws = self.ws();
                ws.pages.push(Page::new(pid, pane));
                ws.active_page = ws.pages.len() - 1;
                self.focus_terminal = true;
                self.reveal_work_surface(ctx);
            }
            Action::KillSession(sid) => {
                if !self.conn.kill(sid) { self.toast(kiln_common::i18n::tr("연결 후 종료할 수 있습니다"), kiln_common::i18n::tr("세션이 유지됩니다. 연결이 복구되면 다시 종료하세요."), ToastKind::Error, Some(sid)); }
            }
            Action::OpenSettings => self.settings_ui.open = true,
            Action::SetLanguage(language) => {
                match kiln_common::i18n::save_language(language) {
                    Ok(()) => {
                        kiln_common::fonts::install(ctx);
                        for pane in self.panes.values_mut() {
                            if let PaneKind::Term { view: Some(view), .. } = &mut pane.kind {
                                view.invalidate_fonts();
                            }
                        }
                        if let Some(view) = &mut self.quick.view { view.invalidate_fonts(); }
                        #[cfg(target_os="macos")]
                        { macos::refresh_settings_menu(); updater::refresh_menu(); }
                        ctx.request_repaint();
                    }
                    Err(error) => self.toast(kiln_common::i18n::tr("설정"), kiln_common::trf!("언어 설정을 저장하지 못했습니다: {error}"), ToastKind::Error, None),
                }
            }
            Action::ShowAgentRequest(index) => self.show_agent_request(index),
            Action::CopyAgentRequest(index) => self.copy_agent_request(index,ctx),
            Action::OpenTerminalSettings => { self.settings_ui.section=1; self.settings_ui.open=true; },
            Action::UpgradeDaemon => {
                let exe = conn::daemon_exe();
                self.conn.send(kiln_proto::ClientMsg::Upgrade { req: 0, exe: exe.to_string_lossy().into_owned() });
            }
            Action::OpenTab(factory) => {
                // 같은 대상이 이미 카드로 열려 있으면 그 카드로 간다.
                let found = self.workspaces[self.active].pages.iter().enumerate().find_map(|(pi, pg)| {
                    pg.root.panes().into_iter().find(|p| self.panes.get(p).and_then(|x| x.tool()).is_some_and(|t| t.key() == factory.key)).map(|p| (pi, p))
                });
                if let Some((_pi, p)) = found {
                    self.reveal_pane(p);
                    if let Some(Pane { kind: PaneKind::Tool(t), .. }) = self.panes.get_mut(&p) {
                        factory.reuse(t.as_mut());
                    }
                    return;
                }
                let env = self.workspaces[self.active].tools.env();
                match factory.make(ctx, &env) {
                    Ok(tab) if tab.own_page() => {
                        let title = tab.title();
                        let id = self.id();
                        self.panes.insert(id, Pane { id, kind: PaneKind::Tool(tab), cwd: None });
                        let pid = self.id();
                        let ws = self.ws();
                        let mut page = Page::new(pid, id);
                        page.title = Some(title);
                        ws.pages.push(page);
                        ws.active_page = ws.pages.len() - 1;
                    }
                    Ok(tab) => {
                        self.place_card(PaneKind::Tool(tab));
                    }
                    Err(e) => self.toast(kiln_common::i18n::tr("열 수 없습니다"), e, ToastKind::Error, None),
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
                page.zoomed = None;
                self.focus_terminal = true;
            }
            Action::RunInTerminal(cmd) => {
                let cwd=self.ws().root.clone();
                self.apply(Action::RunInTerminalAt{cwd,command:cmd},ctx);
            }
            Action::LaunchAgent {cwd, program, context, request} => {
                if !cwd.is_dir() {
                    self.toast(kiln_common::i18n::tr("작업을 시작할 수 없습니다"), kiln_common::i18n::tr("작업 공간 폴더를 찾을 수 없습니다."), ToastKind::Error, None);
                    return;
                }
                let prepared=match agent_launch::prepare(&program,&context,&request) {
                    Ok(prepared)=>prepared,
                    Err(error)=>{self.toast(kiln_common::i18n::tr("작업을 시작할 수 없습니다"),error,ToastKind::Error,None);return;}
                };
                let pane=self.new_term_pane(Some(cwd.to_string_lossy().into_owned()));
                let page_id=self.id();let mut page=Page::new(page_id,pane);page.title=Some(prepared.title);page.agent_request=Some(prepared.request_path);page.agent_request_offset=prepared.request_offset;
                self.ws().pages.push(page);self.ws().active_page=self.ws().pages.len()-1;
                self.pending_input.insert(pane,format!("{}\r",prepared.command));
                self.pending_agent_prompts.insert(pane,request);
                self.spawn_for_pane(pane);
                self.ws().tools.close_agent_task();self.ws().sheet=None;
                self.focus_terminal=true;
            }
            Action::RunInTerminalAt {cwd, command:cmd} => {
                let pane = self.new_term_pane(Some(cwd.to_string_lossy().into_owned()));
                let page = self.ws().page_mut();
                let f = page.focused;
                page.root.split(f, Dir::Vertical, pane);
                page.focused = pane;
                page.zoomed = None;
                self.spawn_for_pane(pane);
                self.pending_input.insert(pane, format!("{cmd}\r"));
                self.focus_terminal = true;
            }
            Action::Toast(t) => self.toast(t, String::new(), ToastKind::Info, None),
            Action::RunLogin(tool) => self.apply(Action::RunInTerminal(kiln_accounts::login_command(tool).to_string()), ctx),
            Action::RotateAccount(session, tool) => self.rotator.start(session, tool),
            Action::OpenHistory => {
                if self.ws().tools.has_multiple_repositories(){self.apply(Action::OpenSheet(tools::ToolKind::Git),ctx);self.toast(kiln_common::i18n::tr("저장소별 로그"), kiln_common::i18n::tr("각 저장소의 로그 버튼으로 변경 이력을 확인하세요."), ToastKind::Info,None);return;}
                let root = self.ws().root.clone();
                self.apply(Action::OpenTab(tools::history_factory(root)), ctx);
            }
            Action::CloneRepo(name) => {
                let Some(parent) = rfd::FileDialog::new().set_title(kiln_common::trf!("저장소 복제 위치 선택: {name}")).pick_folder() else { return };
                let dir = name.rsplit('/').next().unwrap_or(&name).to_string();
                let target = parent.join(&dir);
                if target.exists() {
                    self.toast(kiln_common::i18n::tr("이미 있는 폴더입니다"), target.display().to_string(), ToastKind::Error, None);
                    return;
                }
                self.toast(kiln_common::trf!("{name} 복제 중"), target.display().to_string(), ToastKind::Info, None);
                let tx = self.clones.0.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let r = kiln_git::github::clone_repo(&name, &target).map(|_| target).map_err(|e| e.to_string());
                    let _ = tx.send(r);
                    ctx.request_repaint();
                });
            }
            Action::RevealSession(s) => { self.reveal_session(s); self.reveal_work_surface(ctx); },
            Action::SetTheme(name) => {
                Theme::set_current(&name);
                self.theme = Theme::current();
                self.theme.apply(ctx);
                self.settings.theme = name;
                self.conn.sync_palette();
            }
        }
    }

    fn acknowledge_session(&mut self, sid: SessionId) {
        self.notifications.mark_session_read(sid);
        self.conn.send(kiln_proto::ClientMsg::ClearAttention { session: sid });
    }

    fn session_is_observed(&self, session:SessionId, ctx:&egui::Context)->bool {
        let Some((observed,viewport))=self.conn.observed_terminal(ctx) else { return false; };
        if observed!=session { return false; }
        if viewport!=ctx.viewport_id() {
            return self.quick.open && self.quick.pane.and_then(|p|self.panes.get(&p)).and_then(Pane::session)==Some(session);
        }
        if !self.window_focused || self.focused_session()!=Some(session) { return false; }
        let Some(workspace)=self.workspaces.get(self.active) else { return false; };
        let (rail,_,_)=self.sidebar_dimensions(ctx.content_rect().width());
        if workspace.sheet.is_some() && ctx.content_rect().width()-rail<700.0 { return false; }
        self.confirm.is_none() && self.rename_page.is_none() && !self.recovery_open && self.agent_request_view.is_none()
            && !self.projects.is_open() && !self.launchers.is_open() && !self.palette.is_open()
            && !workspace.tools.quick_is_open() && !self.settings_ui.open && !self.notifications.open
            && self.workspaces.iter().all(|workspace|workspace.renaming.is_none())
    }

    fn sidebar_dimensions(&self, viewport_width: f32) -> (f32, f32, f32) {
        if !self.sidebar_open { return (48.0, 48.0, 48.0); }
        let (min, max) = if viewport_width < 900.0 { (144.0, 160.0) } else { (180.0, 320.0) };
        (self.settings.sidebar_width.clamp(min, max), min, max)
    }

    fn reveal_work_surface(&mut self, ctx: &egui::Context) {
        let (rail, _, _) = self.sidebar_dimensions(ctx.content_rect().width());
        if ctx.content_rect().width() - rail < 700.0 { self.ws().sheet = None; }
    }

    fn reveal_pane(&mut self, target: PaneId) {
        let found = self.workspaces.iter().enumerate().find_map(|(wi, ws)| {
            ws.pages.iter().position(|page| page.root.panes().contains(&target)).map(|pi| (wi, pi))
        });
        let Some((wi, pi)) = found.filter(|_| self.panes.contains_key(&target)) else { return; };
        if self.quick.pane == Some(target) { self.quick.open = false; self.quick.view = None; self.quick.focus_pending = false; }
        self.active = wi;
        self.workspaces[wi].active_page = pi;
        let page = &mut self.workspaces[wi].pages[pi];
        page.focused = target;
        if page.zoomed.is_some() { page.zoomed = Some(target); }
        if let Some(session) = self.panes.get(&target).and_then(Pane::session) { self.acknowledge_session(session); }
        self.focus_terminal = true;
    }

    fn remember_focused_pane(&mut self) {
        self.recent_panes.retain(|id| self.panes.contains_key(id));
        let focused = self.workspaces.get(self.active).and_then(|w| w.pages.get(w.active_page)).map(|p| p.focused);
        if let Some(id) = focused {
            if self.recent_panes.first() != Some(&id) {
                self.recent_panes.retain(|p| *p != id);
                self.recent_panes.insert(0, id);
            }
        }
    }

    fn reveal_session(&mut self, sid: SessionId) {
        let target = self.panes.values().find(|pane| pane.session() == Some(sid)).map(|pane| pane.id);
        if let Some(target) = target { self.reveal_pane(target); }
    }

    fn session_workspace_name(&self, session:SessionId)->String {
        self.workspaces.iter().find(|w|w.all_panes().iter().any(|pid|self.panes.get(pid).and_then(Pane::session)==Some(session)))
            .map(|w|w.name.clone()).unwrap_or_else(||kiln_common::i18n::tr("연결되지 않은 세션").into())
    }

    fn toast(&mut self, title: impl Into<String>, body: impl Into<String>, kind: ToastKind, session: Option<SessionId>) {
        let title = title.into();
        let body = body.into();
        let workspace = session.and_then(|sid| self.workspaces.iter().find(|w| w.all_panes().iter().any(|pid| self.panes.get(pid).and_then(|p| p.session()) == Some(sid))))
            .or_else(|| self.workspaces.get(self.active)).map(|w| w.name.clone()).unwrap_or_default();
        self.notifications.push(&title, &body, kind, session, workspace);
        if kind != ToastKind::Notify || (self.settings.notification_toasts && !self.settings.do_not_disturb) {
            self.toasts.push(Toast { title, body, at: Instant::now(), kind, session, button: None });
        }
    }

    fn transient_info(&mut self, title:&str, body:&str) {
        self.toasts.push(Toast { title:title.into(),body:body.into(),at:Instant::now(),kind:ToastKind::Info,session:None,button:None });
    }

    fn handle_rotation(&mut self, events: Vec<rotation::RotationEvent>) {
        use rotation::RotationEvent as E;
        for e in events {
            match e {
                E::LimitReached { session, tool, reset_hint } => {
                    let body = match reset_hint {
                        Some(h) => kiln_common::trf!("{} 사용량 한도에 도달했습니다 · {h}", tool.display_name()),
                        None => kiln_common::trf!("{} 사용량 한도에 도달했습니다", tool.display_name()),
                    };
                    self.notifications.push(kiln_common::i18n::tr("사용량 한도"), &body, ToastKind::Notify, Some(session), self.session_workspace_name(session));
                    if let Some(item) = self.notifications.items.last_mut() { item.rotate_tool = Some(tool); }
                    if self.settings.notification_toasts && !self.settings.do_not_disturb { self.toasts.push(Toast {
                        title: kiln_common::i18n::tr("사용량 한도").into(),
                        body,
                        at: Instant::now(),
                        kind: ToastKind::Notify,
                        session: Some(session),
                        button: Some((kiln_common::i18n::tr("다음 계정으로 전환").into(), Action::RotateAccount(session, tool))),
                    }); }
                }
                E::Switched { session, tool, label } => {
                    self.toast(kiln_common::trf!("계정 전환: {label}"), kiln_common::trf!("{} 대화를 이어서 실행합니다", tool.display_name()), ToastKind::Info, Some(session));
                }
                E::NoAccount { tool } => self.toast(kiln_common::i18n::tr("전환할 계정이 없습니다"), kiln_common::trf!("설정 → 계정에서 {} 계정을 더 등록하세요", tool.display_name()), ToastKind::Error, None),
                E::Failed { tool, error } => self.toast(kiln_common::trf!("{} 계정 전환 실패", tool.display_name()), error, ToastKind::Error, None),
            }
        }
    }

    // ---------- 이벤트 ----------

    fn handle_conn_events(&mut self, ctx: &egui::Context) {
        for e in std::mem::take(&mut self.conn.events) {
            match e {
                ConnEvent::Activity {session,activity} => {
                    if matches!(activity,kiln_proto::AgentActivity::Running|kiln_proto::AgentActivity::Unknown){continue;}
                    let workspace=self.workspaces.iter().find(|w|w.all_panes().iter().any(|p|self.panes.get(p).and_then(Pane::session)==Some(session))).map(|w|w.name.clone()).unwrap_or_else(||kiln_common::i18n::tr("연결되지 않은 세션").into());
                    let body=match activity {kiln_proto::AgentActivity::Waiting=>kiln_common::i18n::tr("입력을 기다리고 있습니다."),kiln_proto::AgentActivity::Failed=>kiln_common::i18n::tr("작업이 실패했습니다. 출력을 확인하세요."),_=>kiln_common::i18n::tr("작업이 완료되었습니다. 결과를 확인하세요.")};
                    self.notifications.push_activity(activity,session,workspace.clone(),body);
                    if self.session_is_observed(session,ctx) {
                        self.notifications.mark_session_read(session);self.conn.send(kiln_proto::ClientMsg::ClearAttention{session});
                    } else if !self.settings.do_not_disturb {
                        let title=format!("{} · {}",workspace,kiln_common::i18n::tr(activity.label()));
                        if self.settings.notification_toasts {self.toasts.push(Toast{title:title.clone(),body:body.into(),at:Instant::now(),kind:ToastKind::Notify,session:Some(session),button:None});}
                        if !self.window_focused {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                            if self.settings.os_notifications {os_notify(&title,body);}
                        }
                    }
                }

                ConnEvent::Created { req, session } => {
                    if self.cancelled_creates.remove(&req) {
                        if !self.conn.kill(session) { self.cancelled_sessions.insert(session); }
                        continue;
                    }
                    if let Some(pid) = self.pending_creates.remove(&req) {
                        if let Some(Pane { kind: PaneKind::Term { session: s, pending, view }, .. }) = self.panes.get_mut(&pid) {
                            *s = Some(session);
                            *pending = None;
                            *view = Some(TermView::new(session));
                            if let Some(input) = self.pending_input.remove(&pid) {
                                self.conn.input(session, input.into_bytes());
                            }
                            if let Some(request)=self.pending_agent_prompts.remove(&pid) {
                                if let Some(workspace)=self.workspaces.iter_mut().find(|w|w.pages.iter().any(|p|p.root.panes().contains(&pid))) {
                                    workspace.tools.clear_agent_prompt_if(&request);
                                }
                            }
                        }
                    }
                }
                ConnEvent::Notification { session, title, body } => {
                    let focused_here = self.session_is_observed(session,ctx);
                    if !focused_here {
                        let who = self.conn.infos.get(&session).and_then(|i| i.fg_process.clone()).unwrap_or_else(|| kiln_common::i18n::tr("터미널").into());
                        let t = if title.is_empty() { who } else { title.clone() };
                        self.toast(t.clone(), body.clone(), ToastKind::Notify, Some(session));
                        if !self.window_focused && !self.settings.do_not_disturb {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                            if self.settings.os_notifications && !self.settings.do_not_disturb {
                                os_notify(&t, &body);
                            }
                        }
                    } else {
                        self.notifications.push(if title.is_empty() { kiln_common::i18n::tr("터미널 알림") } else { &title }, &body, ToastKind::Notify, Some(session), self.workspaces[self.active].name.clone());
                        self.notifications.mark_session_read(session);
                        self.conn.send(kiln_proto::ClientMsg::ClearAttention { session });
                    }
                }
                ConnEvent::Exited { session, code } => {
                    let error = code.is_some_and(|code| code != 0);
                    self.toast(if error { kiln_common::i18n::tr("터미널이 오류로 종료되었습니다") } else { kiln_common::i18n::tr("터미널 세션이 종료되었습니다") },
                        code.map(|c| kiln_common::trf!("종료 코드 {c} · 해당 패널에서 새 셸을 시작할 수 있습니다.")).unwrap_or_else(|| kiln_common::i18n::tr("해당 패널에서 새 셸을 시작할 수 있습니다.").into()),
                        if error { ToastKind::Error } else { ToastKind::Info }, Some(session));
                }
                ConnEvent::Error { req, message } => {
                    if self.cancelled_creates.remove(&req) { continue; }
                    if let Some(pane) = self.pending_creates.remove(&req) {
                        if let Some(Pane { kind:PaneKind::Term { pending, .. }, .. }) = self.panes.get_mut(&pane) { *pending=None; }
                        self.retain_terminal_launch(pane,message);
                    } else { self.toast(kiln_common::i18n::tr("오류"), message, ToastKind::Error, None); }
                }
                ConnEvent::Connected => {
                    if self.restored {
                        self.transient_info(kiln_common::i18n::tr("데몬에 다시 연결됨"), "");
                    }
                }
                ConnEvent::Upgrading => self.transient_info(kiln_common::i18n::tr("데몬을 새 버전으로 교체하는 중"), kiln_common::i18n::tr("실행 중인 세션은 그대로 유지됩니다")),
                ConnEvent::SessionText { session, text } => {
                    let mut out = Vec::new();
                    self.rotator.on_text(&self.conn, session, &text, &mut out);
                    self.handle_rotation(out);
                }
                ConnEvent::SearchResult { found } => {
                    if let Some(p) = self.focused_pane() {
                        if let Some(Pane { kind: PaneKind::Term { view: Some(v), .. }, .. }) = self.panes.get_mut(&p) {
                            v.set_search_result(found);
                        }
                    }
                }
            }
        }
        if self.conn.is_connected() {
            for session in std::mem::take(&mut self.cancelled_sessions) { self.conn.kill(session); }
        }
        // 세션 목록을 받은 뒤: 사라진 세션의 카드는 새 셸로 채운다.
        if self.conn.is_connected() && self.conn.sessions_listed {
            self.restored = true;
            let visible: Vec<PaneId> = self.workspaces.iter().flat_map(|w| w.all_panes()).collect();
            for p in visible {
                let needs = match self.panes.get(&p).map(|x| &x.kind) {
                    Some(PaneKind::Term { session, pending, .. }) => !self.terminal_launch_drafts.contains_key(&p) && pending.is_none() && session.is_none_or(|s| !self.conn.exists(s)),
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
            for (_, p) in std::mem::take(&mut self.pending_creates) {
                if let Some(Pane { kind: PaneKind::Term { pending, .. }, .. }) = self.panes.get_mut(&p) {
                    *pending = None;
                }
                self.retain_terminal_launch(p,kiln_common::i18n::tr("시작 중 연결이 끊겼습니다. 실행 여부를 확인한 뒤 다시 시작하세요.").into());
            }
        }
        // Request IDs belong to one connection; a new client may reuse them.
        if !self.conn.is_connected() { self.cancelled_creates.clear(); }
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
            (base, Key::J, Action::OpenRecent),
            (base_shift, Key::J, Action::OpenLaunchers),
            (base_shift, Key::P, Action::OpenPalette),
            (base, Key::B, Action::ToggleSidebar),
            (base, Key::Equals, Action::FontDelta(1.0)),
            (base, Key::Plus, Action::FontDelta(1.0)),
            (base, Key::Minus, Action::FontDelta(-1.0)),
            (base, Key::Num0, Action::FontDelta(0.0)),
            (base, Key::F, Action::FindInFocused),
            (base_shift, Key::Enter, Action::ToggleZoom(None)),
            (base_shift, Key::E, Action::ToggleSheet(T::Explorer)),
            (base_shift, Key::L, Action::OpenHistory),
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
        for (m, k, a) in &mut table {
            let id = match a { Action::OpenPalette if *k == Key::K => Some("palette"), Action::OpenRecent=>Some("recent"), Action::OpenLaunchers=>Some("launchers"), Action::NewWorkspace(None)=>Some("projects"), Action::QuickOpen=>Some("files"), Action::ToggleSidebar=>Some("sidebar"), Action::NewPage=>Some("new_task"), Action::Split(Dir::Horizontal)=>Some("split_right"), Action::Split(Dir::Vertical)=>Some("split_down"), Action::CloseActive=>Some("close_panel"), Action::ToggleZoom(_)=>Some("focus_panel"), Action::Equalize=>Some("equalize"), Action::NextPage(1)=>Some("next_task"), Action::NextPage(-1)=>Some("previous_task"), Action::JumpUnread=>Some("unread"), _=>None };
            if let Some(id)=id { let binding=self.keymap.resolve(id,KeyboardShortcut::new(*m,*k)); *m=binding.modifiers; *k=binding.logical_key; }
        }
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

/// 테스트용 상태 조회.
impl KilnApp {
    /// Inspect every workspace: inactive editors and pending DB edits are equally important.
    fn unsaved_items(&self, workspace: Option<usize>) -> Vec<String> {
        let mut items: Vec<String> = self.workspaces.iter().enumerate().filter(|(i, _)| workspace.is_none_or(|wanted| *i == wanted))
            .flat_map(|(_, ws)| ws.all_panes().into_iter().filter_map(|id| {
                let tool = self.panes.get(&id)?.tool()?;
                tool.is_dirty().then(|| {
                    let name = tool.path().map(|p| short_path(p)).unwrap_or_else(|| tool.title());
                    format!("• {} — {name}", ws.name)
                })
            })).collect();
        for (_, ws) in self.workspaces.iter().enumerate().filter(|(i, _)| workspace.is_none_or(|wanted| *i == wanted)) {
            items.extend(ws.tools.unsaved_drafts().into_iter().map(|draft| format!("• {} — {draft}", ws.name)));
        }
        for ws in self.workspaces.iter().enumerate().filter(|(i,_)| workspace.is_none_or(|wanted| *i==wanted)).map(|(_,ws)|ws) {
            for pane in ws.all_panes() {
                if self.terminal_launch_drafts.get(&pane).is_some_and(|draft|draft.command.is_some()) || self.pending_input.contains_key(&pane) {
                    items.push(kiln_common::trf!("• {} — 실행 확인이 필요한 터미널 요청",ws.name));
                }
            }
        }
        if workspace.is_none() && self.projects.has_unsaved_edits() { items.push(kiln_common::i18n::tr("• 프로젝트 — 편집 중인 작업 정보").into()); }
        if workspace.is_none() && self.keymap.has_unsaved_edits() { items.push(kiln_common::i18n::tr("• 설정 — 저장하지 않은 단축키").into()); }
        if workspace.is_none() && self.launchers.has_unsaved_edits() { items.push(kiln_common::i18n::tr("• 저장 명령 — 작성 중인 명령").into()); }
        items
    }

    fn guard_window_close(&mut self, ctx: &egui::Context) {
        #[cfg(target_os="macos")]
        let native_requested=macos::take_termination_request();
        #[cfg(not(target_os="macos"))]
        let native_requested=false;
        let requested = native_requested || ctx.input(|i| i.viewport().close_requested());
        if self.quit_confirmed || !requested { return; }
        if self.projects.is_busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            #[cfg(target_os="macos")] macos::reply_to_termination(false);
            self.toast(kiln_common::i18n::tr("Worktree 생성 중"), kiln_common::i18n::tr("진행 중인 Git 작업이 끝난 후 종료하세요."), ToastKind::Info, None);
            return;
        }
        let dirty = self.unsaved_items(None);
        if dirty.is_empty() {
            self.save_if_changed(true);
            if self.save_error.is_none(){if native_requested{self.quit_confirmed=true;ctx.send_viewport_cmd(egui::ViewportCommand::Close);}return;}
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            #[cfg(target_os="macos")] macos::reply_to_termination(false);
            self.recovery_open=true;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        // AppKit's NSTerminateLater runs NSModalPanelRunLoopMode, but winit
        // dispatches queued input in the default mode. End native deferral before
        // showing an egui confirmation so its buttons and keyboard remain live.
        #[cfg(target_os="macos")] macos::reply_to_termination(false);
        // A Dock/background quit must reveal the loss prompt. Focus alone does
        // not restore an invisible or minimized viewport.
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        if self.confirm.is_none() {
            let previous = ctx.memory(|m| m.focused());
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("quit-return-focus"), previous));
        }
        self.confirm = Some(Confirm { skip_running_confirmation: None,
            title: kiln_common::i18n::tr("저장하지 않은 변경이 있습니다").into(),
            body: kiln_common::trf!("{}\n실행 중인 터미널 작업은 계속됩니다.\n\n작성 중인 내용:\n{}",
                if !self.launchers.has_unsaved_edits() && !self.projects.has_unsaved_edits() && !self.keymap.has_unsaved_edits() {
                    kiln_common::i18n::tr("다음에 Kiln을 열면 작성 중인 내용을 이어서 편집할 수 있습니다. 파일과 데이터베이스에는 적용하지 않습니다.")
                } else { kiln_common::i18n::tr("계속 편집하려면 취소하세요. 변경 내용을 버리고 종료하면 아래 항목은 복구할 수 없습니다.") }, dirty.join("\n")),
            ok: kiln_common::i18n::tr("변경 버리고 종료").into(),
            action: Action::QuitConfirmed,
        });
    }

    #[doc(hidden)]
    pub fn debug_queue_action(&mut self, action: Action) { self.actions.push(action); }

    #[doc(hidden)]
    pub fn debug_apply_action(&mut self, ctx: &egui::Context, action: Action) { self.apply(action,ctx); }
    #[doc(hidden)]
    pub fn debug_set_shell(&mut self, shell: String) { self.settings.shell=shell; }
    #[doc(hidden)]
    pub fn debug_launch_drafts(&self) -> Vec<(PaneId,Option<String>)> { self.terminal_launch_drafts.values().map(|draft|(draft.pane,draft.command.clone())).collect() }
    #[doc(hidden)]
    pub fn debug_pending_launches(&self) -> usize { self.pending_creates.len() }
    #[doc(hidden)]
    pub fn debug_focused_launch_spec(&self) -> Option<SpawnSpec> { self.focused_pane().and_then(|pane| self.launch_specs.get(&pane)).cloned() }
    #[doc(hidden)]
    pub fn debug_focused_pane_id(&self) -> Option<PaneId> { self.focused_pane() }
    #[doc(hidden)]
    pub fn debug_busy_process(&self, pane: PaneId) -> Option<String> { self.pane_is_busy(pane) }
    #[doc(hidden)]
    pub fn debug_pane_rects(&self) -> Vec<(PaneId,egui::Rect)> { self.workspaces[self.active].page().rects.clone() }
    #[doc(hidden)]
    pub fn debug_pane_sessions(&self) -> Vec<(PaneId,SessionId)> { self.workspaces[self.active].page().root.panes().into_iter().filter_map(|id|self.panes.get(&id).and_then(|pane|pane.session()).map(|session|(id,session))).collect() }
    #[doc(hidden)]
    pub fn debug_split(&mut self,ctx:&egui::Context,vertical:bool) { self.apply(Action::Split(if vertical {Dir::Vertical}else{Dir::Horizontal}),ctx); }
    #[doc(hidden)]
    pub fn debug_disconnect(&mut self, ctx:&egui::Context) {
        let mut offline=Conn::offline(ctx.clone());
        offline.infos=std::mem::take(&mut self.conn.infos);
        offline.screens=std::mem::take(&mut self.conn.screens);
        self.conn=offline;
    }
    #[doc(hidden)]
    pub fn debug_checkpoint_restore(&mut self,ctx:&egui::Context) {
        self.cancel_pane_layout();
        let snapshot=self.persist();
        let bytes=serde_json::to_vec(&snapshot).unwrap();
        self.workspaces.clear();self.panes.clear();self.pending_input.clear();self.pending_agent_prompts.clear();self.pending_creates.clear();self.launch_specs.clear();self.terminal_launch_drafts.clear();
        self.restore(serde_json::from_slice(&bytes).unwrap(),ctx);
    }

    #[doc(hidden)]
    pub fn debug_unsaved_items(&self) -> Vec<String> { self.unsaved_items(None) }

    #[doc(hidden)]
    pub fn debug_workspace_count(&self) -> usize { self.workspaces.len() }
    #[doc(hidden)]
    pub fn debug_set_sidebar_width(&mut self, width:f32) { self.settings.sidebar_width=width; }
    #[doc(hidden)]
    pub fn debug_begin_project_rename(&mut self, name:&str) {self.workspaces[self.active].renaming=Some(name.into());}
    #[doc(hidden)]
    pub fn debug_sheet_open(&self)->bool {self.workspaces[self.active].sheet.is_some()}
    #[doc(hidden)]
    pub fn debug_active_workspace_root(&self) -> &Path { &self.workspaces[self.active].root }
    #[doc(hidden)]
    pub fn debug_tool_keys(&self) -> Vec<String> { self.panes.values().filter_map(|p|p.tool().map(|t|t.key())).collect() }
    #[doc(hidden)]
    pub fn debug_task_count(&self) -> usize { self.workspaces[self.active].pages.len() }
    #[doc(hidden)]
    pub fn debug_focused_is_visible(&self) -> bool {
        let page=self.workspaces[self.active].page();
        page.rects.iter().any(|(id,_)|*id==page.focused)
    }


    #[doc(hidden)]
    pub fn debug_focused_session(&self) -> Option<SessionId> { self.focused_session() }

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
    pub fn debug_recovery_fixture(&mut self, prompt:&str) {
        let mut drafts=self.ws().tools.drafts();drafts.agent_prompt=prompt.into();self.ws().tools.restore_drafts(&drafts);
        let pane=self.focused_pane().unwrap();
        self.terminal_launch_drafts.insert(pane,state::TerminalLaunchDraft {pane,command:None,reason:"Invalid shell".into()});
        self.recovery_open=true;
    }

    #[doc(hidden)]
    pub fn debug_limit_workspace(&mut self, session:SessionId)->String {
        self.handle_rotation(vec![rotation::RotationEvent::LimitReached { session,tool:kiln_accounts::Tool::Codex,reset_hint:None }]);
        self.notifications.items.last().unwrap().workspace.clone()
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
    pub fn debug_connection_notice_inbox_growth(&mut self, ctx:&egui::Context)->usize {
        let before=self.notifications.items.len();
        self.conn.events.extend([ConnEvent::Upgrading,ConnEvent::Connected]);
        self.handle_conn_events(ctx);
        self.notifications.items.len()-before
    }

    /// Deliver a daemon event through the same path as live attention updates.
    #[doc(hidden)]
    pub fn debug_deliver_attention(&mut self, ctx:&egui::Context, activity:Option<kiln_proto::AgentActivity>)->bool {
        let session=self.focused_session().expect("focused terminal");
        self.conn.events.push(match activity {
            Some(activity)=>ConnEvent::Activity {session,activity},
            None=>ConnEvent::Notification {session,title:"Attention regression".into(),body:"Review output".into()},
        });
        self.handle_conn_events(ctx);
        self.notifications.items.last().expect("recorded notification").read
    }

    #[doc(hidden)]
    pub fn debug_unread_count(&self)->usize {self.notifications.unread_count()}

    #[doc(hidden)]
    pub fn debug_toast_titles(&self) -> Vec<String> {
        self.toasts.iter().map(|t| t.title.clone()).collect()
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
        #[cfg(target_os="macos")]
        {
            let menu_request = macos::take_settings_request();
            let helper_request = crate::native_actions::take_settings_request();
            if menu_request || helper_request {
                self.settings_ui.open = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.guard_window_close(ctx);
        // Window controls stay available while a settings or project modal is open.
        let fullscreen_shortcut=if cfg!(target_os="macos") {
            KeyboardShortcut::new(Modifiers::CTRL|Modifiers::MAC_CMD,Key::F)
        } else { KeyboardShortcut::new(Modifiers::NONE,Key::F11) };
        if ctx.input_mut(|input|input.consume_shortcut(&fullscreen_shortcut)) {
            self.apply(Action::ToggleFullscreen,ctx);
        }
        #[cfg(target_os="macos")]
        crate::status_bar::publish_unread(self.notifications.unread_count());
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
        while let Ok(r) = self.clones.1.try_recv() {
            match r {
                Ok(p) => self.add_workspace(p, ctx),
                Err(e) => self.toast(kiln_common::i18n::tr("저장소 복제 실패"), e, ToastKind::Error, None),
            }
        }
        self.rotator.poll(&mut self.conn);
        let mut out = Vec::new();
        self.rotator.tick(&mut self.conn, &mut out);
        self.handle_rotation(out);
        if self.rotator.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.conn.infos.values().any(|i| i.fg_process.as_deref().and_then(rotation::tool_for).is_some()) {
            ctx.request_repaint_after(Duration::from_secs(3));
        }
        self.cancel_layout_escape(ctx);
        let quick_open = self.workspaces.get(self.active).is_some_and(|w| w.tools.quick_is_open());
        if self.confirm.is_none() && self.rename_page.is_none() && !self.recovery_open && self.agent_request_view.is_none() && !self.projects.is_open() && !self.launchers.is_open() && !self.palette.is_open() && !quick_open && !self.settings_ui.open && !self.notifications.open && self.workspaces.iter().all(|w| w.renaming.is_none()) {
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
        self.conn.begin_terminal_focus_frame();
        self.ui_topbar(ui);
        self.ui_spaces(ui);
        self.ui_sheet(ui);
        self.ui_canvas(ui);
        // A click closing the confirmation must not hit an underlying modal
        // that becomes visible in the same frame.
        let confirmation_was_open = self.confirm.is_some();
        self.ui_overlays(ctx);
        self.ui_notifications(ctx);
        if !confirmation_was_open {
            self.ui_product_overlays(ctx);
            let restore = ctx.data_mut(|data| data.remove_temp::<bool>(egui::Id::new("quit-restore-focus")).unwrap_or(false));
            if restore {
                let previous = ctx.data_mut(|data| data.remove_temp::<Option<egui::Id>>(egui::Id::new("quit-return-focus")).flatten());
                if let Some(id) = previous { ctx.memory_mut(|memory| memory.request_focus(id)); }
            }
        }

        self.conn.finish_terminal_focus_frame(ctx);
        if let Some((session,_))=self.conn.observed_terminal(ctx) {
            if self.session_is_observed(session,ctx) && (self.notifications.items.iter().any(|n|!n.read && n.session==Some(session)) || self.conn.infos.get(&session).is_some_and(|i|i.attention)) {
                self.acknowledge_session(session);
            }
        }
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.apply(a, ctx);
        }
        self.remember_focused_pane();
        self.save_if_changed(false);
    }

    fn on_exit(&mut self) {
        #[cfg(feature = "updater-test")]
        crate::updater_fixture_event("gui-on-exit");
        if !self.rotator.mgr.shutdown_logins() {
            log::warn!("accounts: browser-login cleanup did not finish before GUI exit");
        }
        self.save_if_changed(true);
        for ws in &self.workspaces {
            ws.tools.lsp.shutdown();
        }
        #[cfg(target_os = "macos")]
        macos::reply_to_termination(true);
    }
}
