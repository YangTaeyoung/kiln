//! 사이드 도구 패널(탐색기, 검색, Git, PR, DB)과 중앙 도구 탭.

use super::Action;
use super::state::ToolP;
use kiln_common::Task;
use kiln_db::{ConnId, DbEvent, DbManager, DbPanel, DbTab};
use kiln_editor::{Decoration, Editor, EditorEvent, FileTree, LspManager, QuickOpen, SearchPanel};
use kiln_git::github::RepoRef;
use kiln_git::history::HistoryEvent;
use kiln_git::{DiffView, GitEvent, GitPanel, GithubHub, HistoryView, IssueView, PrView, RepoSummary};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolKind {
    Explorer,
    Search,
    Git,
    PullRequests,
    Database,
    Problems,
}

impl ToolKind {
    pub const ALL: [ToolKind; 6] = [ToolKind::Explorer, ToolKind::Search, ToolKind::Git, ToolKind::PullRequests, ToolKind::Database, ToolKind::Problems];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "explorer",
            ToolKind::Search => "search",
            ToolKind::Git => "git",
            ToolKind::PullRequests => "prs",
            ToolKind::Database => "db",
            ToolKind::Problems => "problems",
        }
    }

    pub fn from_str(s: &str) -> ToolKind {
        match s {
            "search" => ToolKind::Search,
            "git" => ToolKind::Git,
            "prs" => ToolKind::PullRequests,
            "db" => ToolKind::Database,
            "problems" => ToolKind::Problems,
            _ => ToolKind::Explorer,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "파일",
            ToolKind::Search => "검색",
            ToolKind::Git => "소스 제어",
            ToolKind::PullRequests => "GitHub",
            ToolKind::Database => "데이터베이스",
            ToolKind::Problems => "문제",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "⇧⌘E",
            ToolKind::Search => "⇧⌘F",
            ToolKind::Git => "⇧⌘G",
            ToolKind::PullRequests => "⇧⌘R",
            ToolKind::Database => "⇧⌘B",
            ToolKind::Problems => "⇧⌘M",
        }
    }

    pub fn vicon(&self) -> kiln_common::icons::Icon {
        use kiln_common::icons::Icon;
        match self {
            ToolKind::Explorer => Icon::Folder,
            ToolKind::Search => Icon::Search,
            ToolKind::Git => Icon::Branch,
            ToolKind::PullRequests => Icon::PullRequest,
            ToolKind::Database => Icon::Database,
            ToolKind::Problems => Icon::Warning,
        }
    }
}

/// 중앙 영역의 도구 탭(에디터, diff, PR, DB 테이블 등).
pub trait ToolTab {
    fn title(&self) -> String;
    /// 같은 대상을 다시 열 때 기존 탭을 찾는 키.
    fn key(&self) -> String;
    fn icon(&self) -> &'static str {
        ""
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action>;
    fn is_dirty(&self) -> bool {
        false
    }
    fn persist(&self) -> Option<ToolP> {
        None
    }
    fn find(&mut self) {}
    fn goto(&mut self, _line: usize, _col: usize) {}
    fn on_focus_regained(&mut self) {}
    fn status_text(&self) -> Option<String> {
        None
    }
    fn path(&self) -> Option<&Path> {
        None
    }
    /// 보이지 않는 탭도 매 프레임 호출된다(LSP 응답 반영 등).
    fn tick(&mut self) {}
    /// 새 페이지 하나를 차지하는 카드인지. 이런 카드는 다른 카드를 열 때 교체되지 않는다.
    fn own_page(&self) -> bool {
        false
    }
    /// 카드 머리글 아이콘을 직접 그린다. 그렸으면 true.
    fn paint_icon(&self, _ui: &egui::Ui, _rect: egui::Rect) -> bool {
        false
    }
}

/// 도구 탭을 만든다. 같은 키의 탭이 열려 있으면 `reuse` 가 호출된다.
/// 탭을 만들 때 쓰는 워크스페이스 환경.
pub struct TabEnv {
    pub lsp: LspManager,
}

type MakeTab = Box<dyn FnOnce(&egui::Context, &TabEnv) -> Result<Box<dyn ToolTab>, String>>;
type ReuseTab = Box<dyn FnOnce(&mut dyn ToolTab)>;

pub struct TabFactory {
    pub key: String,
    make: MakeTab,
    reuse: Option<ReuseTab>,
}

impl TabFactory {
    pub fn make(self, ctx: &egui::Context, env: &TabEnv) -> Result<Box<dyn ToolTab>, String> {
        (self.make)(ctx, env)
    }

    pub fn reuse(self, tab: &mut dyn ToolTab) {
        if let Some(r) = self.reuse {
            r(tab);
        }
    }
}

pub fn open_file_factory(path: PathBuf, line: Option<usize>, col: Option<usize>) -> TabFactory {
    let key = format!("file:{}", path.display());
    let p2 = path.clone();
    TabFactory {
        key,
        make: Box::new(move |ctx, env| {
            let mut ed = Editor::open_with_lsp(&path, Some(env.lsp.clone())).map_err(|e| format!("{}: {e}", path.display()))?;
            if let Some(l) = line {
                ed.goto(l, col.unwrap_or(1));
            }
            let _ = ctx;
            Ok(Box::new(EditorTab { ed, focus_pending: true }) as Box<dyn ToolTab>)
        }),
        reuse: Some(Box::new(move |t| {
            if let Some(l) = line {
                t.goto(l, col.unwrap_or(1));
            }
            let _ = p2;
        })),
    }
}

fn diff_factory(root: PathBuf, path: PathBuf, staged: bool) -> TabFactory {
    TabFactory {
        key: format!("diff:{}:{staged}", path.display()),
        make: Box::new(move |_, _| {
            let title = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(Box::new(DiffTab { view: DiffView::for_file(&root, &path, staged), title: format!("{title} ({})", if staged { "스테이징됨" } else { "변경" }), key: format!("diff:{}:{staged}", path.display()) }) as Box<dyn ToolTab>)
        }),
        reuse: Some(Box::new(|_t| {})),
    }
}

fn commit_factory(root: PathBuf, sha: String) -> TabFactory {
    TabFactory {
        key: format!("commit:{sha}"),
        make: Box::new(move |_, _| {
            let short: String = sha.chars().take(8).collect();
            Ok(Box::new(DiffTab { view: DiffView::for_commit(&root, &sha), title: format!("커밋 {short}"), key: format!("commit:{sha}") }) as Box<dyn ToolTab>)
        }),
        reuse: None,
    }
}

fn repo_key(repo: &Option<RepoRef>) -> String {
    repo.as_ref().map(|r| format!("{}:", r.full_name())).unwrap_or_default()
}

fn pr_factory(root: PathBuf, repo: Option<RepoRef>, number: u64) -> TabFactory {
    let key = format!("pr:{}{number}", repo_key(&repo));
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| Ok(Box::new(PrTab { view: PrView::for_repo(root.clone(), repo.clone(), number), number, root, repo, key }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

pub fn history_factory(root: PathBuf) -> TabFactory {
    TabFactory {
        key: "git-history".into(),
        make: Box::new(move |_, _| Ok(Box::new(HistoryTab { view: HistoryView::new(root.clone()) }) as Box<dyn ToolTab>)),
        reuse: Some(Box::new(|t| t.on_focus_regained())),
    }
}

fn range_factory(root: PathBuf, from: String, to: Option<String>) -> TabFactory {
    let key = format!("range:{from}..{}", to.clone().unwrap_or_default());
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| {
            let short = |s: &str| s.chars().take(8).collect::<String>();
            let title = match &to {
                Some(to) => format!("{} → {}", short(&from), short(to)),
                None => format!("{} → 작업 트리", short(&from)),
            };
            Ok(Box::new(DiffTab { view: DiffView::for_range(&root, &from, to.as_deref()), title, key: key.clone() }) as Box<dyn ToolTab>)
        }),
        reuse: None,
    }
}

fn issue_factory(root: PathBuf, repo: Option<RepoRef>, number: u64) -> TabFactory {
    let key = format!("issue:{}{number}", repo_key(&repo));
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| Ok(Box::new(IssueTab { view: IssueView::new(root.clone(), repo.clone(), number), root, repo, key }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

pub fn db_table_factory(db: DbManager, conn: ConnId, schema: Option<String>, table: String) -> TabFactory {
    TabFactory {
        key: format!("db:{}:{}.{}", conn.0, schema.clone().unwrap_or_default(), table),
        make: Box::new(move |_, _| Ok(Box::new(DbTabW { tab: DbTab::table(db, conn, schema.clone(), table.clone()), persist: ToolP::DbTable { conn: conn.0, schema, table } }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

pub fn db_console_factory(db: DbManager, conn: ConnId, n: u64) -> TabFactory {
    TabFactory {
        key: format!("dbconsole:{}:{n}", conn.0),
        make: Box::new(move |_, _| Ok(Box::new(DbTabW { tab: DbTab::console(db, conn), persist: ToolP::DbConsole { conn: conn.0 } }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

// ---------------------------------------------------------------- 탭 구현

struct EditorTab {
    ed: Editor,
    focus_pending: bool,
}

impl ToolTab for EditorTab {
    fn title(&self) -> String {
        self.ed.title()
    }
    fn key(&self) -> String {
        format!("file:{}", self.ed.path().display())
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        if std::mem::take(&mut self.focus_pending) {
            self.ed.request_focus(ui.ctx());
        }
        self.ed.ui(ui);
        self.ed.take_events().into_iter().filter_map(editor_event_action).collect()
    }
    fn tick(&mut self) {
        self.ed.poll_lsp();
    }
    fn is_dirty(&self) -> bool {
        self.ed.is_dirty()
    }
    fn persist(&self) -> Option<ToolP> {
        Some(ToolP::Editor { path: self.ed.path().to_path_buf() })
    }
    fn find(&mut self) {
        self.ed.open_find(false);
    }
    fn goto(&mut self, line: usize, col: usize) {
        self.ed.goto(line, col);
        self.focus_pending = true;
    }
    fn on_focus_regained(&mut self) {
        self.ed.reload_if_changed_on_disk();
    }
    fn status_text(&self) -> Option<String> {
        let s = self.ed.status();
        Some(format!("줄 {}, 열 {}   {}   {}   {}", s.line, s.col, s.language, s.encoding.label(), match s.line_ending {
            kiln_editor::LineEnding::Lf => "LF",
            kiln_editor::LineEnding::CrLf => "CRLF",
        }))
    }
    fn path(&self) -> Option<&Path> {
        Some(self.ed.path())
    }
}

struct DiffTab {
    view: DiffView,
    title: String,
    key: String,
}

impl ToolTab for DiffTab {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn key(&self) -> String {
        self.key.clone()
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        self.view.ui(ui);
        Vec::new()
    }
    fn on_focus_regained(&mut self) {
        self.view.reload();
    }
}

struct PrTab {
    view: PrView,
    number: u64,
    root: PathBuf,
    repo: Option<RepoRef>,
    key: String,
}

impl ToolTab for PrTab {
    fn title(&self) -> String {
        match self.view.detail() {
            Some(d) => format!("#{} {}", self.number, d.title),
            None => format!("PR #{}", self.number),
        }
    }
    fn key(&self) -> String {
        self.key.clone()
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let root = self.root.clone();
        let repo = self.repo.clone();
        self.view.ui(ui).into_iter().filter_map(|e| git_event_action(&root, repo.as_ref(), e)).collect()
    }
}

struct HistoryTab {
    view: HistoryView,
}

impl ToolTab for HistoryTab {
    fn title(&self) -> String {
        "Git 로그".into()
    }
    fn key(&self) -> String {
        "git-history".into()
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let root = self.view.root().to_path_buf();
        self.view
            .ui(ui)
            .into_iter()
            .map(|e| match e {
                HistoryEvent::OpenCommit(sha) => Action::OpenTab(commit_factory(root.clone(), sha)),
                HistoryEvent::OpenDiff { from, to } => Action::OpenTab(range_factory(root.clone(), from, to)),
                HistoryEvent::OpenFile(p) => Action::OpenTab(open_file_factory(p, None, None)),
                HistoryEvent::RunInTerminal(cmd) => Action::RunInTerminal(cmd),
                HistoryEvent::Toast(t) => Action::Toast(t),
            })
            .collect()
    }
    fn on_focus_regained(&mut self) {
        self.view.refresh();
    }
    fn paint_icon(&self, ui: &egui::Ui, rect: egui::Rect) -> bool {
        kiln_common::icons::paint(ui.painter(), rect, kiln_common::icons::Icon::History, kiln_common::Theme::current().accent);
        true
    }
    fn persist(&self) -> Option<ToolP> {
        Some(ToolP::History)
    }
    fn own_page(&self) -> bool {
        true
    }
}

struct IssueTab {
    view: IssueView,
    root: PathBuf,
    repo: Option<RepoRef>,
    key: String,
}

impl ToolTab for IssueTab {
    fn title(&self) -> String {
        self.view.title()
    }
    fn key(&self) -> String {
        self.key.clone()
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let root = self.root.clone();
        let repo = self.repo.clone();
        self.view.ui(ui).into_iter().filter_map(|e| git_event_action(&root, repo.as_ref(), e)).collect()
    }
}

struct DbTabW {
    tab: DbTab,
    persist: ToolP,
}

impl ToolTab for DbTabW {
    fn title(&self) -> String {
        self.tab.title()
    }
    fn key(&self) -> String {
        match &self.persist {
            ToolP::DbTable { conn, schema, table } => format!("db:{conn}:{}.{table}", schema.clone().unwrap_or_default()),
            _ => format!("dbconsole:{}:{:p}", self.tab.conn().0, self),
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        self.tab.ui(ui);
        Vec::new()
    }
    fn is_dirty(&self) -> bool {
        self.tab.pending_changes() > 0
    }
    fn paint_icon(&self, ui: &egui::Ui, rect: egui::Rect) -> bool {
        match self.tab.driver() {
            Some(d) => {
                kiln_db::logo::paint(ui, rect.expand(1.5), d);
                true
            }
            None => false,
        }
    }
    fn persist(&self) -> Option<ToolP> {
        Some(self.persist.clone())
    }
}

/// `repo` 는 이벤트를 낸 화면이 보고 있는 저장소(없으면 워크스페이스 저장소).
fn git_event_action(root: &Path, repo: Option<&RepoRef>, e: GitEvent) -> Option<Action> {
    Some(match e {
        GitEvent::OpenFile(p) => Action::OpenTab(open_file_factory(p, None, None)),
        GitEvent::OpenDiff { path, staged } => Action::OpenTab(diff_factory(root.to_path_buf(), path, staged)),
        GitEvent::OpenPr(n) => Action::OpenTab(pr_factory(root.to_path_buf(), repo.cloned(), n)),
        GitEvent::OpenIssue(n) => Action::OpenTab(issue_factory(root.to_path_buf(), repo.cloned(), n)),
        GitEvent::CloneRepo { name_with_owner } => Action::CloneRepo(name_with_owner),
        GitEvent::OpenCommit(sha) => Action::OpenTab(commit_factory(root.to_path_buf(), sha)),
        GitEvent::RunInTerminal(cmd) => Action::RunInTerminal(cmd),
        GitEvent::OpenUrl(url) => Action::OpenLink(super::terminal::LinkTarget::Url(url)),
    })
}

fn editor_event_action(e: EditorEvent) -> Option<Action> {
    Some(match e {
        EditorEvent::OpenFile(p) => Action::OpenTab(open_file_factory(p, None, None)),
        EditorEvent::OpenAt { path, line, col } => Action::OpenTab(open_file_factory(path, Some(line), Some(col))),
        EditorEvent::FileDeleted(p) => Action::CloseTabByKey(format!("file:{}", p.display())),
        EditorEvent::FileRenamed { from, to } => Action::RenamedFile(from, to),
        EditorEvent::RevealInTerminal(p) => {
            let dir = if p.is_dir() { p } else { p.parent().map(Path::to_path_buf).unwrap_or(p) };
            Action::NewTermAt(dir)
        }
    })
}

// ---------------------------------------------------------------- 워크스페이스별 도구

pub struct WorkspaceTools {
    pub root: PathBuf,
    pub lsp: LspManager,
    ctx: egui::Context,
    db: DbManager,
    tree: Option<FileTree>,
    search: Option<SearchPanel>,
    git: Option<GitPanel>,
    hub: Option<GithubHub>,
    db_panel: Option<DbPanel>,
    quick: QuickOpen,
    summary: Option<RepoSummary>,
    summary_task: Option<Task<Option<RepoSummary>>>,
    summary_at: Option<Instant>,
    deco_at: Option<Instant>,
    console_seq: u64,
    focus_search: bool,
}

impl WorkspaceTools {
    pub fn new(root: &Path, ctx: &egui::Context, db: DbManager) -> Self {
        let lsp = LspManager::new(root.to_path_buf());
        lsp.set_repaint_ctx(ctx);
        WorkspaceTools {
            root: root.to_path_buf(),
            lsp,
            ctx: ctx.clone(),
            db,
            tree: None,
            search: None,
            git: None,
            hub: None,
            db_panel: None,
            quick: QuickOpen::new(),
            summary: None,
            summary_task: None,
            summary_at: None,
            deco_at: None,
            console_seq: 0,
            focus_search: false,
        }
    }

    fn git(&mut self) -> &mut GitPanel {
        let root = self.root.clone();
        self.git.get_or_insert_with(|| GitPanel::new(root))
    }

    /// 매 프레임: 저장소 요약 갱신, 파일 트리 git 색상 반영.
    pub fn tick(&mut self, active: bool) {
        if let Some(t) = &mut self.summary_task
            && let Some(r) = t.take() {
                self.summary = r;
                self.summary_task = None;
            }
        let period = if active { Duration::from_secs(15) } else { Duration::from_secs(60) };
        if self.summary_task.is_none() && self.summary_at.is_none_or(|t| t.elapsed() > period) {
            self.summary_at = Some(Instant::now());
            let root = self.root.clone();
            self.summary_task = Some(Task::spawn(&self.ctx, move || kiln_git::repo_summary(&root)));
        }
        if active && self.tree.is_some() && self.deco_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.deco_at = Some(Instant::now());
            let git = self.git();
            git.refresh();
            let deco: HashMap<PathBuf, Decoration> = git.file_decorations().into_iter().map(|(p, (color, badge))| (p, Decoration { color, badge: Some(badge) })).collect();
            if let Some(tree) = &mut self.tree {
                tree.set_decorations(deco);
            }
        }
    }

    pub fn on_show(&mut self, kind: ToolKind) {
        match kind {
            ToolKind::Search => self.focus_search = true,
            ToolKind::Git => self.git().refresh(),
            ToolKind::PullRequests => {
                if let Some(h) = &mut self.hub {
                    h.refresh();
                }
            }
            _ => {}
        }
    }

    pub fn quick_open(&mut self) {
        self.quick.open(self.root.clone());
    }

    pub fn restore_tool(&mut self, t: &ToolP, ctx: &egui::Context) -> Option<Box<dyn ToolTab>> {
        let f = match t {
            ToolP::Editor { path } if path.exists() => open_file_factory(path.clone(), None, None),
            ToolP::DbTable { conn, schema, table } => db_table_factory(self.db.clone(), ConnId(*conn), schema.clone(), table.clone()),
            ToolP::DbConsole { conn } => {
                self.console_seq += 1;
                db_console_factory(self.db.clone(), ConnId(*conn), self.console_seq)
            }
            ToolP::History => history_factory(self.root.clone()),
            _ => return None,
        };
        f.make(ctx, &self.env()).ok()
    }

    pub fn env(&self) -> TabEnv {
        TabEnv { lsp: self.lsp.clone() }
    }

    pub fn panel_ui(&mut self, ui: &mut egui::Ui, kind: ToolKind) -> Vec<Action> {
        let root = self.root.clone();
        match kind {
            ToolKind::Explorer => {
                let tree = self.tree.get_or_insert_with(|| FileTree::new(root.clone()));
                self.deco_at = None;
                tree.ui(ui).into_iter().filter_map(editor_event_action).collect()
            }
            ToolKind::Search => {
                let s = self.search.get_or_insert_with(|| SearchPanel::new(root.clone()));
                if std::mem::take(&mut self.focus_search) {
                    s.focus();
                }
                s.ui(ui).into_iter().filter_map(editor_event_action).collect()
            }
            ToolKind::Git => {
                let ev = self.git().ui(ui);
                ev.into_iter().filter_map(|e| git_event_action(&root, None, e)).collect()
            }
            ToolKind::PullRequests => {
                let hub = self.hub.get_or_insert_with(|| GithubHub::new(root.clone()));
                let ev = hub.ui(ui);
                let repo = hub.repo();
                ev.into_iter().filter_map(|e| git_event_action(&root, repo.as_ref(), e)).collect()
            }
            ToolKind::Problems => kiln_editor::diagnostics_ui(ui, &self.lsp).into_iter().filter_map(editor_event_action).collect(),
            ToolKind::Database => {
                let db = self.db.clone();
                let panel = self.db_panel.get_or_insert_with(|| DbPanel::new(db.clone()));
                let mut acts = Vec::new();
                for e in panel.ui(ui) {
                    match e {
                        DbEvent::OpenTable { conn, schema, table } => acts.push(Action::OpenTab(db_table_factory(db.clone(), conn, schema, table))),
                        DbEvent::OpenConsole { conn } => {
                            self.console_seq += 1;
                            acts.push(Action::OpenTab(db_console_factory(db.clone(), conn, self.console_seq)));
                        }
                    }
                }
                acts
            }
        }
    }

    pub fn overlay_ui(&mut self, ctx: &egui::Context) -> Vec<Action> {
        match self.quick.ui(ctx) {
            Some(p) => vec![Action::OpenTab(open_file_factory(p, None, None))],
            None => Vec::new(),
        }
    }

    pub fn quick_is_open(&self) -> bool {
        self.quick.is_open()
    }

    pub fn reveal(&mut self, path: &Path) {
        if let Some(t) = &mut self.tree {
            t.reveal(path);
        }
    }

    /// 사이드바 요약(브랜치 등).
    pub fn summary(&self) -> Option<RepoLine> {
        let s = self.summary.as_ref()?;
        Some(RepoLine {
            branch: s.branch.clone(),
            dirty: s.changed + s.conflicted,
            ahead: s.ahead,
            behind: s.behind,
            pr: s.pr.as_ref().map(|p| {
                let st = match (p.state, p.checks) {
                    (kiln_git::PrState::Merged, _) => "merged",
                    (kiln_git::PrState::Closed, _) => "closed",
                    (_, Some(kiln_git::ChecksState::Fail)) => "✗",
                    (_, Some(kiln_git::ChecksState::Pending)) => "…",
                    (_, Some(kiln_git::ChecksState::Pass)) => "✓",
                    _ => "",
                };
                (p.number, st.to_string())
            }),
        })
    }
}

pub struct RepoLine {
    pub branch: String,
    pub dirty: u32,
    pub ahead: u32,
    pub behind: u32,
    pub pr: Option<(u64, String)>,
}
