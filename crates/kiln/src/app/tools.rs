//! 사이드 도구 패널(탐색기, 검색, Git, PR, DB)과 중앙 도구 탭.

use super::Action;
use super::state::{ToolP, WorkspaceDrafts};
use kiln_common::Task;
use kiln_db::{ConnId, DbEvent, DbManager, DbPanel, DbTab, TableSection};
use kiln_db::schema::SchemaAction;
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
    Remote,
    Problems,
}

impl ToolKind {
    pub const ALL: [ToolKind; 7] = [ToolKind::Explorer, ToolKind::Search, ToolKind::Git, ToolKind::PullRequests, ToolKind::Database, ToolKind::Remote, ToolKind::Problems];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "explorer",
            ToolKind::Search => "search",
            ToolKind::Git => "git",
            ToolKind::PullRequests => "prs",
            ToolKind::Database => "db",
            ToolKind::Remote => "remote",
            ToolKind::Problems => "problems",
        }
    }

    pub fn from_str(s: &str) -> ToolKind {
        match s {
            "search" => ToolKind::Search,
            "git" => ToolKind::Git,
            "prs" => ToolKind::PullRequests,
            "db" => ToolKind::Database,
            "remote" => ToolKind::Remote,
            "problems" => ToolKind::Problems,
            _ => ToolKind::Explorer,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ToolKind::Explorer => kiln_common::i18n::tr("파일"),
            ToolKind::Search => kiln_common::i18n::tr("검색"),
            ToolKind::Git => kiln_common::i18n::tr("소스 제어"),
            ToolKind::PullRequests => "GitHub",
            ToolKind::Database => kiln_common::i18n::tr("데이터베이스"),
            ToolKind::Remote => kiln_common::i18n::tr("원격 연결"),
            ToolKind::Problems => kiln_common::i18n::tr("문제"),
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            ToolKind::Explorer => "⇧⌘E",
            ToolKind::Search => "⇧⌘F",
            ToolKind::Git => "⇧⌘G",
            ToolKind::PullRequests => "⇧⌘R",
            ToolKind::Database => "⇧⌘B",
            ToolKind::Remote => "",
            ToolKind::Problems => "⇧⌘M",
        }
    }

    pub fn vicon(&self) -> kiln_common::icons::Icon {
        use kiln_common::icons::Icon;
        match self {
            ToolKind::Explorer => Icon::Folder,
            ToolKind::Search => Icon::Search,
            ToolKind::Git => Icon::Branch,
            ToolKind::PullRequests => Icon::GitHub,
            ToolKind::Database => Icon::Database,
            ToolKind::Remote => Icon::Plug,
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
    fn recovery_notice(&self) -> Option<String> { None }
    fn discard_recovery(&mut self, _discard: bool) {}
    fn find(&mut self) {}
    fn goto(&mut self, _line: usize, _col: usize) {}
    fn on_focus_regained(&mut self) {}
    fn request_focus(&mut self) {}
    fn status_text(&self) -> Option<String> {
        None
    }
    fn path(&self) -> Option<&Path> {
        None
    }
    /// 보이지 않는 탭도 매 프레임 호출된다(LSP 응답 반영 등).
    fn tick(&mut self) {}
    fn show_db_section(&mut self, _section: TableSection) {}
    fn request_db_schema_action(&mut self, _action: SchemaAction) {}
    fn request_db_table_editor(&mut self) {}
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
            Ok(Box::new(EditorTab { ed, focus_pending: true, recovered: false, suppress_recovery: false }) as Box<dyn ToolTab>)
        }),
        reuse: Some(Box::new(move |t| {
            if let Some(l) = line {
                t.goto(l, col.unwrap_or(1));
            }
            let _ = p2;
        })),
    }
}

pub(super) fn diff_factory(root: PathBuf, path: PathBuf, staged: bool) -> TabFactory {
    TabFactory {
        key: format!("diff:{}:{staged}", path.display()),
        make: Box::new(move |_, _| {
            Ok(Box::new(DiffTab { persist: ToolP::Diff { root: root.clone(), path: path.clone(), staged }, view: DiffView::for_file(&root, &path, staged), key: format!("diff:{}:{staged}", path.display()) }) as Box<dyn ToolTab>)
        }),
        reuse: Some(Box::new(|_t| {})),
    }
}

fn commit_factory(root: PathBuf, sha: String) -> TabFactory {
    TabFactory {
        key: format!("commit:{}:{sha}", root.display()),
        make: Box::new(move |_, _| {
            Ok(Box::new(DiffTab { persist: ToolP::Commit { root: root.clone(), sha: sha.clone() }, view: DiffView::for_commit(&root, &sha), key: format!("commit:{}:{sha}",root.display()) }) as Box<dyn ToolTab>)
        }),
        reuse: None,
    }
}

fn repo_key(repo: &Option<RepoRef>) -> String {
    repo.as_ref().map(|r| format!("{}:", r.full_name())).unwrap_or_default()
}

pub(super) fn pr_factory(root: PathBuf, repo: Option<RepoRef>, number: u64) -> TabFactory {
    let key = format!("pr:{}:{}{number}",root.display(), repo_key(&repo));
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| Ok(Box::new(PrTab { recovered: false, suppress_recovery: false, view: PrView::for_repo(root.clone(), repo.clone(), number), number, root, repo, key }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

pub fn history_factory(root: PathBuf) -> TabFactory {
    TabFactory {
        key: format!("git-history:{}",root.display()),
        make: Box::new(move |_, _| Ok(Box::new(HistoryTab { view: HistoryView::new(root.clone()) }) as Box<dyn ToolTab>)),
        reuse: Some(Box::new(|t| t.on_focus_regained())),
    }
}

fn range_factory(root: PathBuf, from: String, to: Option<String>) -> TabFactory {
    let key = format!("range:{}:{from}..{}",root.display(), to.clone().unwrap_or_default());
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| {
            Ok(Box::new(DiffTab { persist: ToolP::Range { root: root.clone(), from: from.clone(), to: to.clone() }, view: DiffView::for_range(&root, &from, to.as_deref()), key: key.clone() }) as Box<dyn ToolTab>)
        }),
        reuse: None,
    }
}

fn issue_factory(root: PathBuf, repo: Option<RepoRef>, number: u64) -> TabFactory {
    let key = format!("issue:{}:{}{number}",root.display(), repo_key(&repo));
    TabFactory {
        key: key.clone(),
        make: Box::new(move |_, _| Ok(Box::new(IssueTab { recovered: false, suppress_recovery: false, number, view: IssueView::new(root.clone(), repo.clone(), number), root, repo, key }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

pub fn db_table_factory(db: DbManager, conn: ConnId, schema: Option<String>, table: String) -> TabFactory {
    TabFactory {
        key: format!("db:{}:{}.{}", conn.0, schema.clone().unwrap_or_default(), table),
        make: Box::new(move |_, _| Ok(Box::new(DbTabW { recovered: false, suppress_recovery: false, tab: DbTab::table(db, conn, schema.clone(), table.clone()), persist: ToolP::DbTable { conn: conn.0, schema, table } }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

fn db_schema_factory(db: DbManager, conn: ConnId, schema: Option<String>, table: String, section: TableSection, action: Option<SchemaAction>) -> TabFactory {
    let base = db_table_factory(db, conn, schema, table);
    let key = base.key.clone();
    let reused_action = action.clone();
    TabFactory {
        key,
        make: Box::new(move |ctx, env| {
            let mut tab = base.make(ctx, env)?;
            tab.show_db_section(section);
            if let Some(action) = action { tab.request_db_schema_action(action); }
            Ok(tab)
        }),
        reuse: Some(Box::new(move |tab| {
            tab.show_db_section(section);
            if let Some(action) = reused_action { tab.request_db_schema_action(action); }
        })),
    }
}

pub fn db_console_factory(db: DbManager, conn: ConnId, n: u64) -> TabFactory {
    TabFactory {
        key: format!("dbconsole:{}:{n}", conn.0),
        make: Box::new(move |_, _| Ok(Box::new(DbTabW { recovered: false, suppress_recovery: false, tab: DbTab::console(db, conn), persist: ToolP::DbConsole { conn: conn.0 } }) as Box<dyn ToolTab>)),
        reuse: None,
    }
}

fn db_table_editor_factory(db: DbManager, conn: ConnId, schema: Option<String>, table: String) -> TabFactory {
    let base = db_table_factory(db, conn, schema, table);
    TabFactory {
        key: base.key.clone(),
        make: Box::new(move |ctx, env| {
            let mut tab = base.make(ctx, env)?;
            tab.request_db_table_editor();
            Ok(tab)
        }),
        reuse: Some(Box::new(|tab| tab.request_db_table_editor())),
    }
}

// ---------------------------------------------------------------- 탭 구현

struct EditorTab {
    ed: Editor,
    focus_pending: bool,
    recovered: bool,
    suppress_recovery: bool,
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
        let path = self.ed.path().to_path_buf();
        let language_override = self.ed.language_override().map(str::to_owned);
        if !self.suppress_recovery && let Some(draft) = self.ed.recovery_draft() {
            return Some(ToolP::EditorDraft { path, draft, language_override });
        }
        Some(ToolP::Editor { path, language_override })
    }
    fn recovery_notice(&self) -> Option<String> {
        (self.recovered && self.ed.is_dirty()).then(|| kiln_common::trf!("{} — 복원된 미저장 파일{}", self.ed.path().display(), if self.ed.recovery_conflict() { kiln_common::i18n::tr(" · 디스크 변경과 충돌: 저장 전 검토 필요") } else { "" }))
    }
    fn discard_recovery(&mut self, discard: bool) { self.suppress_recovery = discard; }
    fn find(&mut self) {
        self.ed.open_find(false);
    }
    fn goto(&mut self, line: usize, col: usize) {
        self.ed.goto(line, col);
        self.focus_pending = true;
    }
    fn request_focus(&mut self) { self.focus_pending = true; }
    fn on_focus_regained(&mut self) {
        self.ed.reload_if_changed_on_disk();
    }
    fn status_text(&self) -> Option<String> {
        let s = self.ed.status();
        Some(kiln_common::trf!("줄 {}, 열 {}   {}   {}   {}", s.line, s.col, if s.language == "Plain Text" { kiln_common::i18n::tr("일반 텍스트") } else { &s.language }, s.encoding.label(), match s.line_ending {
            kiln_editor::LineEnding::Lf => "LF",
            kiln_editor::LineEnding::CrLf => "CRLF",
        }))
    }
    fn path(&self) -> Option<&Path> {
        Some(self.ed.path())
    }
}

struct DiffTab {
    persist: ToolP,
    view: DiffView,
    key: String,
}

// Derive app-owned title fragments at render time so open tabs follow locale changes.
fn diff_title(persist: &ToolP) -> String {
    let short = |value: &str| value.chars().take(8).collect::<String>();
    match persist {
        ToolP::Diff { path, staged, .. } => {
            let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
            let state = if *staged { kiln_common::i18n::tr("스테이징됨") } else { kiln_common::i18n::tr("변경") };
            format!("{name} ({state})")
        }
        ToolP::Commit { sha, .. } => { let short = short(sha); kiln_common::trf!("커밋 {short}") }
        ToolP::Range { from, to, .. } => match to {
            Some(to) => format!("{} → {}", short(from), short(to)),
            None => kiln_common::trf!("{} → 작업 트리", short(from)),
        },
        _ => unreachable!("DiffTab only stores diff, commit, or range metadata"),
    }
}

impl ToolTab for DiffTab {
    fn persist(&self) -> Option<ToolP> { Some(self.persist.clone()) }
    fn title(&self) -> String {
        diff_title(&self.persist)
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
    recovered: bool,
    suppress_recovery: bool,
    view: PrView,
    number: u64,
    root: PathBuf,
    repo: Option<RepoRef>,
    key: String,
}

impl ToolTab for PrTab {
    fn is_dirty(&self) -> bool { !self.view.review_draft().is_empty() }
    fn discard_recovery(&mut self, suppressed: bool) { self.suppress_recovery = suppressed; }
    fn recovery_notice(&self) -> Option<String> {
        (self.recovered && self.is_dirty()).then(|| kiln_common::trf!("{} — 복원된 미전송 초안", self.title()))
    }
    fn persist(&self) -> Option<ToolP> {
        let root = self.root.clone(); let repo = self.repo.as_ref().map(RepoRef::full_name); let number = self.number;
        Some(if self.is_dirty() && !self.suppress_recovery {
            ToolP::PrDraft { root, repo, number, body: self.view.review_draft().to_owned() }
        } else { ToolP::Pr { root, repo, number } })
    }
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
        kiln_common::i18n::tr("Git 로그").into()
    }
    fn key(&self) -> String {
        format!("git-history:{}",self.view.root().display())
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
                HistoryEvent::RunInTerminal(cmd) => Action::RunInTerminalAt {cwd:root.clone(),command:cmd},
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
        Some(ToolP::RepositoryHistory {root:self.view.root().to_path_buf()})
    }
    fn own_page(&self) -> bool {
        true
    }
}

struct IssueTab {
    recovered: bool,
    suppress_recovery: bool,
    number: u64,
    view: IssueView,
    root: PathBuf,
    repo: Option<RepoRef>,
    key: String,
}

impl ToolTab for IssueTab {
    fn is_dirty(&self) -> bool { !self.view.comment_draft().is_empty() }
    fn discard_recovery(&mut self, suppressed: bool) { self.suppress_recovery = suppressed; }
    fn recovery_notice(&self) -> Option<String> {
        (self.recovered && self.is_dirty()).then(|| kiln_common::trf!("{} — 복원된 미전송 초안", self.title()))
    }
    fn persist(&self) -> Option<ToolP> {
        let root = self.root.clone(); let repo = self.repo.as_ref().map(RepoRef::full_name); let number = self.number;
        Some(if self.is_dirty() && !self.suppress_recovery {
            ToolP::IssueDraft { root, repo, number, body: self.view.comment_draft().to_owned() }
        } else { ToolP::Issue { root, repo, number } })
    }
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
    recovered: bool,
    suppress_recovery: bool,
    tab: DbTab,
    persist: ToolP,
}

impl ToolTab for DbTabW {
    fn title(&self) -> String {
        self.tab.title()
    }
    fn key(&self) -> String {
        if let Some(table) = self.tab.table_ref() {
            format!("db:{}:{}.{}", self.tab.conn().0, table.schema.clone().unwrap_or_default(), table.table)
        } else { format!("dbconsole:{}:{:p}", self.tab.conn().0, self) }
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        self.tab.ui(ui);
        Vec::new()
    }
    fn is_dirty(&self) -> bool {
        self.tab.has_unsaved_changes()
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
        if matches!(self.persist, ToolP::DbTable { .. }) {
            let table = self.tab.table_ref()?;
            let conn = self.tab.conn().0;
            if !self.suppress_recovery && let Some(draft) = self.tab.table_draft() {
                return Some(ToolP::DbTableDraft { conn, schema: table.schema.clone(), table: table.table.clone(), draft });
            }
            return Some(ToolP::DbTable { conn, schema: table.schema.clone(), table: table.table.clone() });
        }
        if !self.suppress_recovery && let Some(document) = self.tab.console_document() {
            return Some(ToolP::DbConsoleDocument { conn: self.tab.conn().0, document });
        }
        Some(self.persist.clone())
    }
    fn recovery_notice(&self) -> Option<String> {
        (self.recovered && self.tab.has_unsaved_changes()).then(|| kiln_common::trf!("{} — 복원된 DB 초안 · 실행/제출되지 않음", self.tab.title()))
    }
    fn discard_recovery(&mut self, discard: bool) { self.suppress_recovery = discard; }
    fn request_focus(&mut self) { self.tab.request_focus(); }
    fn show_db_section(&mut self, section: TableSection) { self.tab.show_table_section(section); }
    fn request_db_schema_action(&mut self, action: SchemaAction) { self.tab.request_schema_action(action); }
    fn request_db_table_editor(&mut self) { self.tab.request_table_editor(); }
}

/// `repo` 는 이벤트를 낸 화면이 보고 있는 저장소(없으면 워크스페이스 저장소).
pub(super) fn git_event_action(root: &Path, repo: Option<&RepoRef>, e: GitEvent) -> Option<Action> {
    Some(match e {
        GitEvent::OpenFile(p) => Action::OpenTab(open_file_factory(p, None, None)),
        GitEvent::OpenDiff { path, staged } => Action::OpenTab(diff_factory(root.to_path_buf(), path, staged)),
        GitEvent::OpenPr(n) => Action::OpenTab(pr_factory(root.to_path_buf(), repo.cloned(), n)),
        GitEvent::OpenIssue(n) => Action::OpenTab(issue_factory(root.to_path_buf(), repo.cloned(), n)),
        GitEvent::CloneRepo { name_with_owner } => Action::CloneRepo(name_with_owner),
        GitEvent::OpenCommit(sha) => Action::OpenTab(commit_factory(root.to_path_buf(), sha)),
        GitEvent::OpenHistory => Action::OpenTab(history_factory(root.to_path_buf())),
        GitEvent::History(event) => match event {
            HistoryEvent::OpenCommit(sha)=>Action::OpenTab(commit_factory(root.to_path_buf(),sha)),
            HistoryEvent::OpenDiff{from,to}=>Action::OpenTab(range_factory(root.to_path_buf(),from,to)),
            HistoryEvent::OpenFile(path)=>Action::OpenTab(open_file_factory(path,None,None)),
            HistoryEvent::RunInTerminal(command)=>Action::RunInTerminalAt{cwd:root.to_path_buf(),command},
            HistoryEvent::Toast(message)=>Action::Toast(message),
        },
        GitEvent::RunInTerminal(cmd) => Action::RunInTerminalAt {cwd:root.to_path_buf(),command:cmd},
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
    repositories: super::workspace_repos::RepositoryWorkspace,
    pub root: PathBuf,
    canonical_root: PathBuf,
    workspace_id: u64,
    pub lsp: LspManager,
    ctx: egui::Context,
    db: DbManager,
    tree: Option<FileTree>,
    search: Option<SearchPanel>,
    git: Option<GitPanel>,
    hub: Option<GithubHub>,
    db_panel: Option<DbPanel>,
    remote: kiln_remote::ui::RemoteManager,
    remote_panel: Option<kiln_remote::ui::RemotePanel>,
    quick: QuickOpen,
    summary: Option<RepoSummary>,
    summary_task: Option<Task<Option<RepoSummary>>>,
    summary_at: Option<Instant>,
    worktree_identity: Option<kiln_git::worktrees::WorktreeIdentity>,
    worktree_identity_task: Option<Task<Option<kiln_git::worktrees::WorktreeIdentity>>>,
    worktree_identity_at: Option<Instant>,
    deco_at: Option<Instant>,
    console_seq: u64,
    focus_search: bool,
    suppress_recovery: bool,
    legacy_panel: Option<ToolKind>,
}

impl WorkspaceTools {
    pub fn new(root: &Path, ctx: &egui::Context, db: DbManager, workspace_id: u64) -> Self {
        let lsp = LspManager::new(root.to_path_buf());
        lsp.set_repaint_ctx(ctx);
        WorkspaceTools {
            repositories: super::workspace_repos::RepositoryWorkspace::new(root,ctx).with_id_salt(workspace_id),
            root: root.to_path_buf(),
            workspace_id,
            canonical_root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            lsp,
            ctx: ctx.clone(),
            db,
            tree: None,
            search: None,
            git: None,
            hub: None,
            db_panel: None,
            remote: kiln_remote::ui::RemoteManager::load(),
            remote_panel: None,
            quick: QuickOpen::new().with_id_salt(workspace_id),
            summary: None,
            summary_task: None,
            summary_at: None,
            worktree_identity: None,
            worktree_identity_task: None,
            worktree_identity_at: None,
            deco_at: None,
            console_seq: 0,
            focus_search: false,
            suppress_recovery: false,
            legacy_panel: None,
        }
    }

    pub fn drafts(&self) -> WorkspaceDrafts {
        if self.suppress_recovery { return WorkspaceDrafts::default(); }
        WorkspaceDrafts {
            repositories:self.repositories.drafts(),
            agent_prompt:self.repositories.prompt().to_owned(),
            commit_message: self.git.as_ref().map(|g| g.commit_draft().to_owned()).unwrap_or_default(),
            github: self.hub.as_ref().map(GithubHub::recovery_drafts).unwrap_or_default(),
        }
    }

    pub fn restore_drafts(&mut self, drafts: &WorkspaceDrafts) {
        self.repositories.restore(&drafts.repositories);
        self.repositories.restore_prompt(&drafts.agent_prompt);
        if !drafts.commit_message.is_empty() { *self.git().commit_message_mut() = drafts.commit_message.clone(); }
        if !drafts.github.repositories.is_empty() {
            let mut hub = GithubHub::new(self.root.clone()); hub.restore_drafts(&drafts.github); self.hub = Some(hub);
        }
    }

    pub fn unsaved_drafts(&self) -> Vec<String> {
        let mut items = self.repositories.unsaved();
        if self.git.as_ref().is_some_and(|g| !g.commit_draft().is_empty()) { items.push(kiln_common::i18n::tr("Git — 작성 중인 커밋 메시지").into()); }
        if let Some(hub) = &self.hub {
            for (repo, drafts) in hub.recovery_drafts().repositories {
                if drafts.pull_request.is_some() { items.push(kiln_common::trf!("GitHub · {repo} — 작성 중인 풀 리퀘스트")); }
                if drafts.issue.is_some() { items.push(kiln_common::trf!("GitHub · {repo} — 작성 중인 이슈")); }
            }
        }
        items
    }

    pub fn discard_recovery(&mut self, suppressed: bool) { self.suppress_recovery = suppressed; }

    pub fn worktree_identity(&self) -> Option<&kiln_git::worktrees::WorktreeIdentity> {
        self.worktree_identity.as_ref()
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    fn git(&mut self) -> &mut GitPanel {
        let root = self.root.clone();
        self.git.get_or_insert_with(|| GitPanel::new(root).with_id_salt(self.workspace_id))
    }

    /// 매 프레임: 저장소 요약 갱신, 파일 트리 git 색상 반영.
    pub fn tick(&mut self, active: bool) {
        self.repositories.tick();
        if let Some(task) = &mut self.worktree_identity_task
            && let Some(identity) = task.take() {
                self.worktree_identity = identity;
                self.worktree_identity_task = None;
            }
        if self.worktree_identity_task.is_none() && self.worktree_identity_at.is_none_or(|at| at.elapsed() > Duration::from_secs(60)) {
            self.worktree_identity_at = Some(Instant::now());
            let root = self.root.clone();
            self.worktree_identity_task = Some(Task::spawn(&self.ctx, move || kiln_git::worktrees::identity(&root).ok().flatten()));
        }
        if let Some(t) = &mut self.summary_task
            && let Some(r) = t.take() {
                self.summary = r;
                self.summary_task = None;
            }
        let period = if active { Duration::from_secs(15) } else { Duration::from_secs(60) };
        if !self.repositories.loading() && !self.repositories.is_multi() && self.summary_task.is_none() && self.summary_at.is_none_or(|t| t.elapsed() > period) {
            self.summary_at = Some(Instant::now());
            let root = self.root.clone();
            self.summary_task = Some(Task::spawn(&self.ctx, move || kiln_git::repo_summary(&root)));
        }
        if active && self.tree.is_some() && self.deco_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.deco_at = Some(Instant::now());
            let deco: HashMap<PathBuf, Decoration> = if self.repositories.is_multi() {
                self.repositories.decorations()
            } else {
                let git = self.git();
                git.refresh();
                git.file_decorations().into_iter().map(|(p, (color, badge))| (p, Decoration { color, badge: Some(badge) })).collect()
            };
            if let Some(tree) = &mut self.tree {
                tree.set_decorations(deco);
            }
        }
    }

    pub fn on_show(&mut self, kind: ToolKind) {
        match kind {
            ToolKind::Explorer => {
                let root = self.root.clone();
                self.tree.get_or_insert_with(|| FileTree::new(root).with_id_salt(self.workspace_id)).request_focus();
            }
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

    pub fn open_agent_task(&mut self) { self.repositories.open_agent_task(); }
    pub fn clear_agent_prompt_if(&mut self, request:&str) { self.repositories.clear_prompt_if(request); }
    pub fn is_agent_task_open(&self) -> bool { self.repositories.is_agent_task_open() }
    pub fn close_agent_task(&mut self) { self.repositories.close_agent_task(); }

    pub fn has_multiple_repositories(&self)->bool{self.repositories.is_multi()}

    pub fn quick_open(&mut self) {
        self.quick.open(self.root.clone());
    }

    pub fn restore_tool(&mut self, t: &ToolP, ctx: &egui::Context) -> Option<Box<dyn ToolTab>> {
        if let ToolP::PrDraft { root, repo, number, body } = t {
            let repo = repo.as_deref().and_then(RepoRef::parse);
            let mut view = PrView::for_repo(root.clone(), repo.clone(), *number);
            view.restore_review_draft(body);
            let key = pr_factory(root.clone(), repo.clone(), *number).key;
            return Some(Box::new(PrTab { view, root: root.clone(), repo, number: *number, key, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::IssueDraft { root, repo, number, body } = t {
            let repo = repo.as_deref().and_then(RepoRef::parse);
            let mut view = IssueView::new(root.clone(), repo.clone(), *number);
            *view.comment_mut() = body.clone();
            let key = issue_factory(root.clone(), repo.clone(), *number).key;
            return Some(Box::new(IssueTab { view, root: root.clone(), repo, number: *number, key, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::EditorDraft { path, draft, language_override } = t {
            let mut ed = Editor::open(path).unwrap_or_else(|_| Editor::from_text(path.clone(), ""));
            ed.restore_draft(draft);
            ed.set_language_override(language_override.clone());
            ed.set_lsp(self.lsp.clone());
            return Some(Box::new(EditorTab { ed, focus_pending: true, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::DbTableDraft { conn, schema, table, draft } = t {
            let mut tab = DbTab::table(self.db.clone(), ConnId(*conn), schema.clone(), table.clone());
            tab.restore_table_draft(draft);
            return Some(Box::new(DbTabW { tab, persist: ToolP::DbTable { conn: *conn, schema: schema.clone(), table: table.clone() }, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::DbConsoleDocument { conn, document } = t {
            let mut tab = DbTab::console(self.db.clone(), ConnId(*conn));
            tab.restore_console_document(document);
            return Some(Box::new(DbTabW { tab, persist: ToolP::DbConsole { conn: *conn }, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::DbConsoleDraft { conn, sql } = t {
            let mut tab = DbTab::console(self.db.clone(), ConnId(*conn));
            tab.set_console_text(sql);
            return Some(Box::new(DbTabW { tab, persist: ToolP::DbConsole { conn: *conn }, recovered: true, suppress_recovery: false }));
        }
        if let ToolP::Editor { path, language_override } = t {
            let mut ed = Editor::open(path).ok()?;
            ed.set_language_override(language_override.clone());
            ed.set_lsp(self.lsp.clone());
            return Some(Box::new(EditorTab { ed, focus_pending: true, recovered: false, suppress_recovery: false }));
        }
        let f = match t {
            ToolP::Diff { root, path, staged } => diff_factory(root.clone(), path.clone(), *staged),
            ToolP::Commit { root, sha } => commit_factory(root.clone(), sha.clone()),
            ToolP::Range { root, from, to } => range_factory(root.clone(), from.clone(), to.clone()),
            ToolP::Pr { root, repo, number } => pr_factory(root.clone(), repo.as_deref().and_then(RepoRef::parse), *number),
            ToolP::Issue { root, repo, number } => issue_factory(root.clone(), repo.as_deref().and_then(RepoRef::parse), *number),
            ToolP::DbTable { conn, schema, table } => db_table_factory(self.db.clone(), ConnId(*conn), schema.clone(), table.clone()),
            ToolP::DbConsole { conn } => {
                self.console_seq += 1;
                db_console_factory(self.db.clone(), ConnId(*conn), self.console_seq)
            }
            ToolP::Remote { connection, path, draft, profile, pending_operation } => {
                let profile = self.remote.get(connection).or_else(||profile.clone())?;
                remote_factory(self.remote.clone(), profile, path.clone(), draft.clone(), false, pending_operation.is_some())
            },
            ToolP::History => history_factory(self.root.clone()),
            ToolP::RepositoryHistory {root} => history_factory(root.clone()),
            _ => return None,
        };
        f.make(ctx, &self.env()).ok()
    }

    pub fn env(&self) -> TabEnv {
        TabEnv { lsp: self.lsp.clone() }
    }

    pub fn file_menu_ui(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let root = self.root.clone();
        self.tree.get_or_insert_with(|| FileTree::new(root).with_id_salt(self.workspace_id)).menu_ui(ui)
            .into_iter().filter_map(editor_event_action).collect()
    }

    pub fn panel_ui(&mut self, ui: &mut egui::Ui, kind: ToolKind) -> Vec<Action> {
        // Task input remains available while repository discovery runs and never
        // inherits an unrelated legacy Git draft editor.
        if kind == ToolKind::Git && self.repositories.is_agent_task_open() {
            return self.repositories.ui(ui, kind);
        }
        let root = self.root.clone();
        if matches!(kind, ToolKind::Git | ToolKind::PullRequests) && self.repositories.loading() {
            ui.horizontal(|ui| { ui.spinner(); ui.label(kiln_common::i18n::tr("워크스페이스의 저장소를 찾는 중…")); });
            return Vec::new();
        }
        if matches!(kind, ToolKind::Git | ToolKind::PullRequests) && self.repositories.is_multi() {
            // Retain the original live panels, including in-flight operations. Never merge
            // these drafts into an existing repository draft and overwrite either version.
            let has_legacy = match kind {
                ToolKind::Git => self.git.as_ref().is_some_and(|g| !g.commit_draft().is_empty()),
                ToolKind::PullRequests => self.hub.as_ref().is_some_and(|h| !h.recovery_drafts().repositories.is_empty()),
                _ => false,
            };
            if has_legacy || self.legacy_panel == Some(kind) {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.legacy_panel != Some(kind), kiln_common::i18n::tr("전체 저장소")).clicked() {
                        self.legacy_panel = None;
                    }
                    if ui.selectable_label(self.legacy_panel == Some(kind), kiln_common::i18n::tr("기존 초안 이어 쓰기")).clicked() {
                        self.legacy_panel = Some(kind);
                    }
                });
            }
            if self.legacy_panel != Some(kind) {
                return self.repositories.ui(ui, kind);
            }
            ui.label(kiln_common::trf!("기존 작업 위치: {}", root.display()));
            if kind == ToolKind::Git {
                // Even a moved/deleted root must leave the text editable and exportable.
                egui::CollapsingHeader::new(kiln_common::i18n::tr("보관된 커밋 메시지 편집")).show(ui, |ui| {
                    let message = self.git().commit_message_mut();
                    ui.add(egui::TextEdit::multiline(message).desired_width(f32::INFINITY));
                    if ui.button(kiln_common::i18n::tr("커밋 메시지 복사")).clicked() { ui.ctx().copy_text(message.clone()); }
                });
            }
            ui.separator();
        }
        match kind {
            ToolKind::Explorer => {
                let tree = self.tree.get_or_insert_with(|| FileTree::new(root.clone()).with_id_salt(self.workspace_id));
                self.deco_at = None;
                tree.ui_embedded(ui).into_iter().filter_map(editor_event_action).collect()
            }
            ToolKind::Search => {
                let s = self.search.get_or_insert_with(|| SearchPanel::new(root.clone()).with_id_salt(self.workspace_id));
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
            ToolKind::Remote => {
                let manager = self.remote.clone();
                let panel = self.remote_panel.get_or_insert_with(|| kiln_remote::ui::RemotePanel::new(manager.clone()));
                panel.ui(ui).into_iter().filter_map(|event| match event {
                    kiln_remote::ui::RemoteEvent::Open { connection, path } => manager.get(&connection).map(|profile| Action::OpenTab(remote_factory(manager.clone(), profile, path, None, true, false))),
                    kiln_remote::ui::RemoteEvent::Ssh { alias, config_path, options } => Some(Action::OpenSshConnection { alias, config_path, options }),
                }).collect()
            },
            ToolKind::Database => {
                let db = self.db.clone();
                let panel = self.db_panel.get_or_insert_with(|| DbPanel::new(db.clone()));
                let mut acts = Vec::new();
                for e in panel.ui(ui) {
                    match e {
                        DbEvent::OpenTable { conn, schema, table } => acts.push(Action::OpenTab(db_table_factory(db.clone(), conn, schema, table))),
                        DbEvent::EditTable { conn, schema, table } => acts.push(Action::OpenTab(db_table_editor_factory(db.clone(), conn, schema, table))),
                        DbEvent::OpenTableSection { conn, schema, table, section } => acts.push(Action::OpenTab(db_schema_factory(db.clone(), conn, schema, table, section, None))),
                        DbEvent::SchemaAction { conn, schema, table, action } => acts.push(Action::OpenTab(db_schema_factory(db.clone(), conn, schema, table, TableSection::Structure, Some(action)))),
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
        if self.repositories.is_multi() { return self.repositories.summary(); }
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

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn github_comment_and_workspace_drafts_roundtrip_and_reversible_discard() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut tools = WorkspaceTools::new(dir.path(), &ctx, DbManager::in_memory(), 1);
        for doc in [
            ToolP::PrDraft { root: dir.path().into(), repo: Some("owner/repo".into()), number: 7, body: "review draft".into() },
            ToolP::IssueDraft { root: dir.path().into(), repo: Some("owner/repo".into()), number: 8, body: "comment draft".into() },
        ] {
            let json = serde_json::to_string(&doc).unwrap();
            let restored: ToolP = serde_json::from_str(&json).unwrap();
            let mut tab = tools.restore_tool(&restored, &ctx).unwrap();
            assert!(tab.is_dirty()); assert!(tab.recovery_notice().is_some());
            assert!(tab.persist().as_ref() == Some(&doc));
            tab.discard_recovery(true);
            assert!(matches!(tab.persist(), Some(ToolP::Pr { .. } | ToolP::Issue { .. })));
            assert!(tab.is_dirty(), "a failed checkpoint must retain the live draft");
            tab.discard_recovery(false);
            assert!(tab.persist().as_ref() == Some(&doc));
        }
        let mut draft = WorkspaceDrafts::default();
        draft.commit_message = "commit body".into();
        draft.github.repositories.insert("owner/repo".into(), kiln_git::RepositoryDrafts {
            pull_request: Some(kiln_git::PrCreationDraft { request: kiln_git::PrCreate { title: "PR title".into(), body: "PR body".into(), base: "main".into(), draft: true, ..Default::default() }, head: "feature".into(), bases: vec!["main".into()] }),
            issue: Some(kiln_git::IssueCreate { title: "issue".into(), labels: vec!["bug".into()], assignees: vec!["alice".into()], ..Default::default() }),
        });
        tools.restore_drafts(&draft);
        assert!(tools.drafts() == draft);
        assert_eq!(tools.unsaved_drafts().len(), 3);
        tools.discard_recovery(true);
        assert!(tools.drafts() == WorkspaceDrafts::default());
        tools.discard_recovery(false);
        assert!(tools.drafts() == draft);
    }

    #[test]
    fn renamed_database_table_uses_live_target_for_key_and_restore() {
        let db = DbManager::in_memory();
        let wrapper = DbTabW { recovered:false, suppress_recovery:false,
            tab:DbTab::table(db,ConnId(73),Some("main".into()),"renamed".into()),
            persist:ToolP::DbTable {conn:73,schema:Some("main".into()),table:"old_name".into()} };
        assert_eq!(wrapper.key(),"db:73:main.renamed");
        assert!(matches!(wrapper.persist(),Some(ToolP::DbTable {conn:73,table,..}) if table=="renamed"));
    }

    #[test]
    fn editor_language_override_survives_clean_and_dirty_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.txt");
        std::fs::write(&path, "select 1").unwrap();
        let ctx = egui::Context::default();
        let mut tools = WorkspaceTools::new(dir.path(), &ctx, DbManager::in_memory(), 1);
        let clean = ToolP::Editor { path: path.clone(), language_override: Some("SQL".into()) };
        let tab = tools.restore_tool(&clean, &ctx).unwrap();
        assert!(tab.persist().as_ref() == Some(&clean));
        let mut ed = Editor::open(&path).unwrap();
        ed.insert_text("select 2");
        let dirty = ToolP::EditorDraft { path, draft: ed.recovery_draft().unwrap(), language_override: Some("SQL".into()) };
        let restored: ToolP = serde_json::from_str(&serde_json::to_string(&dirty).unwrap()).unwrap();
        let mut tab = tools.restore_tool(&restored, &ctx).unwrap();
        assert!(tab.is_dirty());
        assert!(tab.persist().as_ref() == Some(&dirty));
        tab.discard_recovery(true);
        assert!(matches!(tab.persist(), Some(ToolP::Editor { language_override: Some(ref name), .. }) if name == "SQL"));
        let mut legacy = serde_json::to_value(&dirty).unwrap();
        legacy.get_mut("EditorDraft").unwrap().as_object_mut().unwrap().remove("language_override");
        assert!(matches!(serde_json::from_value::<ToolP>(legacy).unwrap(), ToolP::EditorDraft { language_override: None, .. }));
    }

    #[test]
    fn restored_file_and_sql_drafts_are_visible_and_explicit_discard_is_persisted() {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("draft.txt");
        std::fs::write(&path,"original").unwrap();
        let mut ed=Editor::open(&path).unwrap(); ed.select_all(); ed.insert_text("unsaved");
        let ctx=egui::Context::default();
        let mut tools=WorkspaceTools::new(dir.path(),&ctx,DbManager::in_memory(),1);
        let mut tab=tools.restore_tool(&ToolP::EditorDraft { path:path.clone(),draft:ed.recovery_draft().unwrap(), language_override:None },&ctx).unwrap();
        assert!(tab.is_dirty()); assert!(tab.recovery_notice().is_some());
        assert!(matches!(tab.persist(),Some(ToolP::EditorDraft { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(),"original");
        tab.discard_recovery(true); assert!(matches!(tab.persist(),Some(ToolP::Editor { .. })));
        let mut sql=tools.restore_tool(&ToolP::DbConsoleDraft { conn:999,sql:"delete from example;".into() },&ctx).unwrap();
        assert!(sql.is_dirty()); assert!(sql.recovery_notice().is_some());
        assert!(matches!(sql.persist(),Some(ToolP::DbConsoleDocument { document, .. }) if document.text=="delete from example;"));
        sql.discard_recovery(true); assert!(matches!(sql.persist(),Some(ToolP::DbConsole { .. })));
    }
    #[test]
    fn git_document_types_keep_their_identity_across_serialization() {
        let root=PathBuf::from("/tmp/recovery-fixture");
        let docs=[ToolP::Diff {root:root.clone(),path:PathBuf::from("a.rs"),staged:true},ToolP::Commit {root:root.clone(),sha:"abc123".into()},ToolP::Range {root:root.clone(),from:"main".into(),to:None},ToolP::Pr {root:root.clone(),repo:Some("owner/repo".into()),number:12},ToolP::Issue {root,repo:None,number:9}];
        for doc in docs { let json=serde_json::to_string(&doc).unwrap(); let restored:ToolP=serde_json::from_str(&json).unwrap(); assert!(restored==doc); }
    }
}

#[cfg(test)] mod multi_repository_tests {
 use super::*;
 #[test]
 fn opening_files_focuses_navigation_and_consumes_terminal_keys() {
  use egui_kittest::{Harness, kittest::Queryable};
  let dir = tempfile::tempdir().unwrap();
  let file = dir.path().join("README.md");
  std::fs::write(&file, "hello").unwrap();
  let ctx = egui::Context::default();
  let mut tools = WorkspaceTools::new(dir.path(), &ctx, DbManager::in_memory(), 1);
  tools.on_show(ToolKind::Explorer);
  let mut h = Harness::builder().with_size([500., 500.]).build_ui_state(
   |ui, state: &mut (WorkspaceTools, Vec<Action>, usize)| {
    kiln_common::fonts::install(ui.ctx());
    if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name(kiln_common::fonts::SEMIBOLD.into()))) { return; }
    state.1.extend(state.0.panel_ui(ui, ToolKind::Explorer));
    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::Enter)) { state.2 += 1; }
   }, (tools, vec![], 0));
  h.run_steps(3);
  h.get_by_label("README.md");
  h.key_press(egui::Key::ArrowDown); h.run_steps(2);
  h.key_press(egui::Key::Enter); h.run_steps(2);
  assert!(h.state().1.iter().any(|action| matches!(action, Action::OpenTab(_))));
  assert_eq!(h.state().0.tree.as_ref().unwrap().selected(), Some(file.as_path()));
  assert_eq!(h.state().2, 0, "file navigation keys must not remain available to a terminal below the inspector");
 }
 #[test]
 fn explicit_agent_task_bypasses_initial_scan_and_legacy_git_editor() {
  use egui_kittest::{Harness, kittest::Queryable};
  let dir = tempfile::tempdir().unwrap();
  let ctx = egui::Context::default();
  let mut tools = WorkspaceTools::new(dir.path(), &ctx, DbManager::in_memory(), 1);
  let mut drafts = WorkspaceDrafts::default();
  drafts.commit_message = "keep legacy draft".into();
  tools.restore_drafts(&drafts);
  tools.legacy_panel = Some(ToolKind::Git);
  assert!(tools.repositories.loading());
  tools.open_agent_task();
  let mut h = Harness::builder().with_size([600., 500.]).build_ui_state(
   |ui, tools: &mut WorkspaceTools| { tools.panel_ui(ui, ToolKind::Git); }, tools);
  h.run_steps(3);
  h.get_by_label("작업 시작");
  assert!(h.query_by_label("기존 초안 이어 쓰기").is_none());
  assert!(h.query_by_label("보관된 커밋 메시지 편집").is_none());
  assert_eq!(h.state().drafts().commit_message, "keep legacy draft");
 }
 #[test]
 fn multi_repository_view_keeps_legacy_drafts_accessible_without_overwrite() {
  use egui_kittest::{Harness, kittest::Queryable};
  for kind in [ToolKind::Git, ToolKind::PullRequests] {
   let dir = tempfile::tempdir().unwrap();
   let ctx = egui::Context::default();
   let mut tools = WorkspaceTools::new(dir.path(), &ctx, DbManager::in_memory(), 1);
   let mut draft = WorkspaceDrafts::default();
   draft.commit_message = "original root message".into();
   draft.github.repositories.insert("owner/old".into(), kiln_git::RepositoryDrafts {
    issue: Some(kiln_git::IssueCreate { title: "old issue".into(), ..Default::default() }), ..Default::default()
   });
   draft.repositories.insert(dir.path().to_path_buf(), super::super::state::LocalRepositoryDrafts {
    commit_message: "separate repository message".into(), ..Default::default()
   });
   tools.restore_drafts(&draft);
   let deadline = Instant::now() + Duration::from_secs(2);
   while tools.repositories.loading() && Instant::now() < deadline {
    tools.repositories.tick();
    std::thread::sleep(Duration::from_millis(1));
   }
   assert!(!tools.repositories.loading());
   let mut h = Harness::builder().with_size([600., 800.]).build_ui_state(move |ui, t: &mut WorkspaceTools| { kiln_common::fonts::install(ui.ctx()); t.panel_ui(ui, kind); }, tools);
   h.run_steps(3);
   let git_address = h.state().git.as_ref().unwrap() as *const GitPanel;
   let hub_address = h.state().hub.as_ref().unwrap() as *const GithubHub;
   h.get_by_label("기존 초안 이어 쓰기").click(); h.run_steps(3);
   assert_eq!(h.state().legacy_panel, Some(kind));
   assert_eq!(git_address, h.state().git.as_ref().unwrap() as *const GitPanel);
   assert_eq!(hub_address, h.state().hub.as_ref().unwrap() as *const GithubHub);
   assert!(h.state().drafts() == draft);
   if kind == ToolKind::Git {
    h.get_by_label("보관된 커밋 메시지 편집").click(); h.run_steps(3);
    assert!(h.query_by_label("커밋 메시지 복사").is_some());
   }
   h.get_by_label("전체 저장소").click(); h.run_steps(3);
   assert_eq!(h.state().legacy_panel, None);
   assert!(h.state().drafts() == draft);
  }
 }
 #[test]fn repository_tabs_and_commands_keep_their_own_working_tree(){
  let a=PathBuf::from("/workspace/frontend");let b=PathBuf::from("/workspace/backend");
  assert_ne!(history_factory(a.clone()).key,history_factory(b.clone()).key);
  assert_ne!(pr_factory(a.clone(),None,12).key,pr_factory(b.clone(),None,12).key);
  assert_ne!(issue_factory(a.clone(),None,12).key,issue_factory(b.clone(),None,12).key);
  assert_ne!(commit_factory(a.clone(),"same-sha".into()).key,commit_factory(b.clone(),"same-sha".into()).key);
  assert_ne!(range_factory(a.clone(),"main".into(),None).key,range_factory(b.clone(),"main".into(),None).key);
  assert!(matches!(git_event_action(&b,None,GitEvent::RunInTerminal("git status".into())),Some(Action::RunInTerminalAt{cwd,..}) if cwd==b));
  assert!(matches!(git_event_action(&b,None,GitEvent::OpenHistory),Some(Action::OpenTab(factory)) if factory.key==history_factory(b.clone()).key));
  let tab=HistoryTab{view:HistoryView::new(a.clone())};assert!(matches!(tab.persist(),Some(ToolP::RepositoryHistory{root}) if root==a));
 }
}

#[cfg(test)]
mod language_title_tests {
    use super::*;

    #[test]
    fn language_changes_refresh_diff_titles_without_changing_file_or_revision_data() {
        let root = PathBuf::from("/workspace");
        let metadata = [
            ToolP::Diff { root: root.clone(), path: PathBuf::from("설정.txt"), staged: true },
            ToolP::Commit { root: root.clone(), sha: "0123456789abcdef".into() },
            ToolP::Range { root: root.clone(), from: "0123456789abcdef".into(), to: None },
        ];
        let korean = kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || metadata.iter().map(diff_title).collect::<Vec<_>>());
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                let titles = metadata.iter().map(diff_title).collect::<Vec<_>>();
                assert!(titles[0].starts_with("설정.txt ("), "user file name must stay intact");
                assert!(titles[1].contains("01234567"));
                assert!(titles[2].contains("01234567"));
                if language != kiln_common::i18n::Language::Korean {
                    for (translated, source) in titles.iter().zip(&korean) { assert_ne!(translated, source, "stale title in {language:?}"); }
                }
                let range = ToolP::Range { root: root.clone(), from: "0123456789abcdef".into(), to: Some("fedcba9876543210".into()) };
                assert_eq!(diff_title(&range), "01234567 → fedcba98");
            });
        }
    }
}

fn remote_factory(manager: kiln_remote::ui::RemoteManager, profile: kiln_remote::ConnectionProfile, path: String, draft: Option<kiln_remote::ui::RemoteDraft>, connect: bool, interrupted: bool) -> TabFactory {
    let key = format!("remote:{}",profile.id);
    TabFactory { key:key.clone(), make: Box::new(move |ctx,_| {
        let mut browser=kiln_remote::ui::RemoteBrowser::new(manager,profile,path);
        if let Some(draft)=draft {browser.restore(&draft);}
        if interrupted {browser.restore_pending_notice();}
        if connect {browser.connect(ctx);}
        Ok(Box::new(RemoteTab { browser,key,suppress_recovery:false,ctx:ctx.clone() }))
    }), reuse:Some(Box::new(|tab|tab.on_focus_regained())) }
}
struct RemoteTab { browser:kiln_remote::ui::RemoteBrowser,key:String,suppress_recovery:bool,ctx:egui::Context }
impl ToolTab for RemoteTab {
    fn title(&self)->String{self.browser.title()}
    fn key(&self)->String{self.key.clone()}
    fn ui(&mut self,ui:&mut egui::Ui)->Vec<Action>{self.browser.ui(ui);Vec::new()}
    fn is_dirty(&self)->bool{self.browser.is_dirty()||self.browser.pending_operation().is_some()}
    fn persist(&self)->Option<ToolP>{Some(ToolP::Remote {connection:self.browser.connection().into(),path:self.browser.path().into(),draft:if self.suppress_recovery{None}else{self.browser.draft()},profile:Some(self.browser.profile().clone()),pending_operation:self.browser.pending_operation()})}
    fn recovery_notice(&self)->Option<String>{if self.browser.pending_operation().is_some(){Some(kiln_common::trf!("{} — 진행 중인 원격 전송",self.browser.title()))}else{self.is_dirty().then(||kiln_common::trf!("{} — 저장하지 않은 원격 편집",self.browser.title()))}}
    fn discard_recovery(&mut self,suppress:bool){self.suppress_recovery=suppress;}
    fn tick(&mut self){self.browser.tick(&self.ctx);}
    fn paint_icon(&self,ui:&egui::Ui,rect:egui::Rect)->bool{kiln_common::icons::paint(ui.painter(),rect,kiln_remote::ui::provider_icon(self.browser.profile()),kiln_common::Theme::current().text);true}
}
