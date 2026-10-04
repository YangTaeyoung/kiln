//! Project/task composer. Git calls run off the UI thread; archiving only hides a
//! task and never removes its directory, branch, or running sessions.
//! Reference: https://www.onorca.dev/docs/model/worktrees
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke};
use kiln_common::{
    Theme, fonts,
    widgets::{self, ButtonKind},
};
use kiln_git::worktrees::{self, Create, Snapshot, Worktree};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectAction {
    Open {
        path: PathBuf,
        name: String,
        command: Option<String>,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Planned,
    #[default]
    Active,
    Review,
    Done,
}
impl TaskStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Planned => "예정",
            Self::Active => "진행 중",
            Self::Review => "검토 중",
            Self::Done => "완료",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Task {
    path: PathBuf,
    name: String,
    note: String,
    status: TaskStatus,
    archived: bool,
    base: String,
    #[serde(default)]
    command: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StartMode {
    #[default]
    Terminal,
    Agent,
}
#[derive(Clone, PartialEq, Eq)]
struct Draft {
    folder: String,
    name: String,
    worktree: bool,
    branch: String,
    base: String,
    existing: bool,
    mode: StartMode,
    command: String,
}
// Read-only file discovery: do not execute shell startup files or user-supplied commands.
fn executable_in(name: &str, directories: &[PathBuf]) -> Option<PathBuf> {
    for dir in directories.iter().filter(|p|p.is_absolute()).take(64) {
        let names=if cfg!(windows){vec![format!("{name}.exe"),format!("{name}.cmd"),format!("{name}.bat")]}else{vec![name.to_owned()]};
        for name in names {let path=dir.join(name);let Ok(meta)=std::fs::metadata(&path) else {continue;};
            if !meta.is_file(){continue;}
            #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;if meta.permissions().mode() & 0o111 == 0 {continue;}}
            return Some(path);
        }
    }
    None
}
fn detected_agents() -> Vec<(String,Option<PathBuf>)> {
    let mut dirs:Vec<PathBuf>=std::env::var_os("PATH").map(|p|std::env::split_paths(&p).collect()).unwrap_or_default();
    #[cfg(target_os="macos")] dirs.extend([PathBuf::from("/opt/homebrew/bin"),PathBuf::from("/usr/local/bin")]);
    if let Some(home)=std::env::var_os("HOME"){dirs.push(PathBuf::from(home).join(".local/bin"));}
    ["codex","claude","gemini"].into_iter().map(|name|(name.into(),executable_in(name,&dirs))).collect()
}

impl Draft {
    fn new(root: &Path) -> Self {
        Self {
            folder: root.to_string_lossy().into_owned(),
            name: String::new(),
            worktree: false,
            branch: String::new(),
            base: "HEAD".into(),
            existing: false,
            mode: StartMode::Terminal,
            command: "codex".into(),
        }
    }
    fn validate(&self) -> Result<(PathBuf, String, Option<String>), String> {
        let path = expand_path(&self.folder)?;
        let path = path.canonicalize().unwrap_or(path);
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err("프로젝트 이름을 1~100자로 입력하세요.".into());
        }
        // Worktree creation only creates a folder/branch. Start mode is chosen
        // in the following open step, so hidden command fields cannot block it.
        let command = if self.worktree { None } else { match self.mode {
            StartMode::Terminal => None,
            StartMode::Agent => {
                if self.command.trim().is_empty()
                    || self.command.len() > 16384
                    || self.command.chars().any(char::is_control)
                {
                    return Err("에이전트 실행 명령을 한 줄로 입력하세요.".into());
                }
                Some(self.command.clone())
            }
        }};
        if !self.worktree && !path.is_dir() {
            return Err("기존 프로젝트 폴더를 선택하세요.".into());
        }
        if self.worktree && (self.branch.trim().is_empty() || self.base.trim().is_empty()) {
            return Err("작업 브랜치와 시작 지점을 입력하세요.".into());
        }
        Ok((path, name.into(), command))
    }
}
#[derive(Clone)]
enum Screen {
    List,
    Compose { draft: Draft, original: Draft },
    Detail { task: Task, original: Task },
    Discard(Box<Screen>, bool),
}
enum JobResult {
    Loaded(Result<Snapshot, String>),
    Created(Result<(Worktree, Task), String>),
}
enum Intent {
    None,
    Close,
    List,
    Compose,
    Detail(Task),
    OpenSaved(Task),
    Refresh,
    Start,
    OpenWithoutRecord,
    RecoverRecords,
    Save,
    Archive,
    Restore,
    Discard,
    Resume,
}

pub struct Projects {
    open: bool,
    root: PathBuf,
    snapshot: Option<Snapshot>,
    tasks: Vec<Task>,
    path: PathBuf,
    disk: Option<Vec<u8>>,
    load_failed: bool,
    failed_source: Option<Vec<u8>>,
    error: Option<String>,
    notice: Option<String>,
    repo_error: Option<String>,
    agent_bins: Vec<(String, Option<PathBuf>)>,
    query: String,
    archived: bool,
    screen: Screen,
    pending: Option<Receiver<JobResult>>,
    creating: bool,
    focus_name: bool,
}
impl Default for Projects {
    fn default() -> Self {
        Self::from_path(kiln_common::paths::config_file("projects.json"))
    }
}
impl Projects {
    fn from_path(path: PathBuf) -> Self {
        let mut this = Self {
            open: false,
            root: PathBuf::new(),
            snapshot: None,
            tasks: vec![],
            path,
            disk: None,
            load_failed: false,
            failed_source: None,
            error: None,
            notice: None,
            repo_error: None,
            agent_bins: detected_agents(),
            query: String::new(),
            archived: false,
            screen: Screen::List,
            pending: None,
            creating: false,
            focus_name: false,
        };
        this.reload();
        this
    }
    fn reload(&mut self) {
        match read_tasks(&self.path) {
            Ok((tasks, disk)) => {
                self.tasks = tasks;
                self.disk = disk;
                self.load_failed = false;
                self.failed_source = None;
                self.error = None;
            }
            Err(e) => {
                self.load_failed = true;
                self.failed_source = std::fs::metadata(&self.path).ok().filter(|m|m.len()<=4*1024*1024).and_then(|_|std::fs::read(&self.path).ok());
                self.error = Some(format!(
                    "프로젝트 기록을 읽지 못했습니다. 원본 파일은 유지됩니다. {e}"
                ));
            }
        }
    }
    fn recover_records(&mut self) -> Result<PathBuf,String> {
        use std::io::Write;
        if !self.load_failed{return Err("프로젝트 기록을 정상적으로 읽었습니다. 복구할 필요가 없습니다.".into());}
        let original=self.failed_source.as_ref().ok_or("원본을 안전하게 읽지 못해 복구하지 않았습니다. 기록 저장 없이 폴더를 열 수 있습니다.")?;
        if std::fs::read(&self.path).map_err(|e|e.to_string())? != *original {return Err("파일이 변경되었습니다. 새로 고침 후 복구하세요.".into());}
        let mut backup=None;
        for n in 1..=1000 {
            let path=self.path.with_extension(format!("json.backup-{n}"));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file)=>{file.write_all(original).and_then(|_|file.sync_all()).map_err(|e|format!("백업 실패: {e}"))?;backup=Some(path);break;}
                Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,
                Err(e)=>return Err(format!("백업 실패: {e}")),
            }
        }
        let backup=backup.ok_or("백업 파일 이름을 확보하지 못했습니다.")?;
        if std::fs::read(&self.path).map_err(|e|e.to_string())? != *original {return Err("백업 중 파일이 변경되었습니다. 원본을 유지했습니다.".into());}
        kiln_common::store::save_json(&self.path,&Vec::<Task>::new()).map_err(|e|format!("복구 실패. 백업: {} · {e}",backup.display()))?;
        self.reload();
        if self.load_failed{return Err("복구한 기록을 다시 읽지 못했습니다.".into());}
        Ok(backup)
    }
    pub fn open(&mut self, root: &Path) {
        if self.open {
            return;
        }
        self.open = true;
        self.pending = None;
        self.snapshot = None;
        self.notice = None;
        self.repo_error = None;
        self.agent_bins = detected_agents();
        self.root = root.into();
        self.screen = Screen::List;
        self.query.clear();
        self.archived = false;
        self.reload();
        self.refresh();
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn task_context(&self, path: &Path) -> Option<(String, String, &'static str)> {
        if let Some(task) = self.tasks.iter().find(|task| task.path == path) {
            return Some((task.name.clone(), task.note.clone(), task.status.label()));
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.tasks
            .iter()
            .find(|task| task.path == path)
            .map(|task| (task.name.clone(), task.note.clone(), task.status.label()))
    }
    /// A running git operation must finish before application shutdown.
    pub fn is_busy(&self) -> bool {
        self.creating
    }
    pub fn has_unsaved_edits(&self) -> bool {
        fn dirty(s: &Screen) -> bool {
            match s {
                Screen::Compose { draft, original } => draft != original,
                Screen::Detail { task, original } => task != original,
                Screen::Discard(s, _) => dirty(s),
                Screen::List => false,
            }
        }
        dirty(&self.screen)
    }
    fn refresh(&mut self) {
        if self.pending.is_some() {
            return;
        }
        let root = self.root.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(JobResult::Loaded(
                worktrees::inspect(&root).map_err(|e| e.to_string()),
            ));
        });
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let result = self.pending.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Disconnected) => Some(JobResult::Loaded(Err(
                "작업 프로세스가 종료됐습니다. 다시 시도하세요.".into(),
            ))),
            Err(_) => None,
        });
        if let Some(result) = result {
            self.pending = None;
            self.creating = false;
            match result {
                JobResult::Loaded(Ok(snapshot)) => {
                    self.root = snapshot.root.clone();
                    self.snapshot = Some(snapshot);
                    self.repo_error = None;
                }
                JobResult::Loaded(Err(e)) => {
                    self.snapshot = None;
                    self.repo_error = Some(e);
                }
                JobResult::Created(Err(e)) => self.error = Some(e),
                JobResult::Created(Ok((tree, task))) => {
                    if let Err(e) = self.save_task(task.clone()) {
                        self.error = Some(format!(
                            "Worktree는 생성됐지만 프로젝트 메모를 저장하지 못했습니다. {} — {e}",
                            tree.path.display()
                        ));
                    } else {
                        self.notice = Some("Worktree를 만들었습니다. 프로젝트를 열어 터미널이나 에이전트를 시작하세요.".into());
                    }
                    self.screen = Screen::Detail {
                        task: task.clone(),
                        original: task,
                    };
                    self.refresh();
                }
            }
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn save_task(&mut self, task: Task) -> Result<(), String> {
        if self.load_failed {
            return Err("기록 파일을 다시 불러온 뒤 저장하세요.".into());
        }
        let (_, bytes) = read_tasks(&self.path)?;
        if bytes != self.disk {
            return Err(
                "다른 창에서 프로젝트 기록을 변경했습니다. 메모를 복사한 뒤 편집을 취소하고 새로 고침하세요.".into(),
            );
        }
        validate_task(&task)?;
        let mut tasks = self.tasks.clone();
        if let Some(existing) = tasks.iter_mut().find(|t| t.path == task.path) {
            *existing = task;
        } else {
            tasks.push(task);
        }
        if tasks.len() > 1000 {
            return Err("프로젝트 기록은 최대 1,000개까지 저장할 수 있습니다.".into());
        }
        let bytes = serde_json::to_vec_pretty(&tasks).map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("프로젝트 기록이 4MB 제한을 초과했습니다.".into());
        }
        kiln_common::store::save_json(&self.path, &tasks).map_err(|e| e.to_string())?;
        self.disk = Some(bytes);
        self.tasks = tasks;
        Ok(())
    }
    fn all_tasks(&self) -> Vec<Task> {
        let mut tasks = self.tasks.clone();
        if let Some(snapshot) = &self.snapshot {
            for tree in snapshot.trees.iter().filter(|t| !t.bare) {
                if !tasks.iter().any(|t| t.path == tree.path) {
                    tasks.push(Task {
                        path: tree.path.clone(),
                        name: tree
                            .branch
                            .clone()
                            .unwrap_or_else(|| "Detached HEAD".into()),
                        note: String::new(),
                        status: TaskStatus::Active,
                        archived: false,
                        base: String::new(),
                        command: None,
                    });
                }
            }
        }
        tasks
    }
    /// Call every frame, including when closed, so background jobs are collected.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<ProjectAction> {
        self.poll(ctx);
        if !self.open {
            return None;
        }
        let t = Theme::current();
        let viewport = ctx.content_rect();
        let width = (viewport.width() - 48.0).clamp(240.0, 720.0);
        let budget = (if self.error.is_some() { 54.0 } else { 0.0 }) + if self.load_failed { 36.0 } else { 0.0 };
        let height = (viewport.height() - 150.0 - budget).clamp(48.0, 510.0);
        let frame = Frame::new()
            .fill(t.bg_elevated)
            .stroke(Stroke::new(1.0, t.border_strong))
            .corner_radius(CornerRadius::same(12))
            .shadow(t.shadow())
            .inner_margin(Margin::same(16));
        let mut intent = Intent::None;
        let mut screen = self.screen.clone();
        let tasks = self.all_tasks();
        let modal = egui::Modal::new(egui::Id::new("projects-manager")).frame(frame).backdrop_color(Color32::from_black_alpha(100)).show(ctx, |ui| {
            ui.set_width(width); ui.spacing_mut().scroll.floating = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new(match screen { Screen::List => "프로젝트 관리", Screen::Compose { .. } => "새 프로젝트", Screen::Detail { .. } => "프로젝트 관리", Screen::Discard(..) => "편집 취소" }).font(fonts::semibold(20.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if widgets::icon_button(ui, kiln_common::icons::Icon::Close, 28.0, false, "프로젝트 창 닫기").clicked() { intent = Intent::Close; } });
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical().id_salt("project-body").max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_min_width(width - 12.0);
                ui.add_enabled_ui(!self.creating, |ui| match &mut screen {
                    Screen::List => {
                        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("프로젝트 이름, 폴더, 메모 검색").desired_width(f32::INFINITY));
                        ui.horizontal(|ui| { ui.selectable_value(&mut self.archived, false, "프로젝트"); ui.selectable_value(&mut self.archived, true, "보관함"); });
                        if self.pending.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("저장소 확인 중…"); }); }
                        if let Some(notice) = &self.notice { ui.label(RichText::new(notice).color(t.text_dim)); }
                        let query = self.query.to_lowercase(); let mut count = 0;
                        for task in tasks.iter().filter(|task| task.archived == self.archived && format!("{} {} {}", task.name, task.path.display(), task.note).to_lowercase().contains(&query)) {
                            count += 1; ui.add_space(6.0);
                            let label = format!("{}  ·  {}\n{}", task.name, task.status.label(), task.path.display());
                            ui.horizontal(|ui| {
                                if ui.add_sized([(ui.available_width() - 36.0).max(80.0), 56.0], egui::Button::new(label).wrap())
                                    .on_hover_text(if task.archived { "프로젝트 관리" } else { "프로젝트 열기" }).clicked() {
                                    intent = if task.archived { Intent::Detail(task.clone()) } else { Intent::OpenSaved(task.clone()) };
                                }
                                if widgets::icon_button(ui, kiln_common::icons::Icon::Pencil, 28.0, false, "프로젝트 편집").clicked() {
                                    intent = Intent::Detail(task.clone());
                                }
                            });
                        }
                        if count == 0 { ui.add_space(18.0); ui.label(if self.archived { "보관한 프로젝트가 없습니다." } else if !query.is_empty() { "검색과 일치하는 프로젝트가 없습니다." } else { "첫 프로젝트의 폴더를 선택하세요. 터미널과 AI 에이전트 중 시작 방식을 선택할 수 있습니다." }); ui.add_space(18.0); }
                    }
                    Screen::Compose { draft, .. } => {
                        let label = ui.label("프로젝트 이름"); let name = ui.add(egui::TextEdit::singleline(&mut draft.name).hint_text("예: 결제 서비스").desired_width(f32::INFINITY)).labelled_by(label.id);
                        if self.focus_name { name.request_focus(); self.focus_name = false; }
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| { ui.selectable_value(&mut draft.worktree, false, "기존 폴더"); ui.add_enabled_ui(self.snapshot.is_some(), |ui| { if ui.selectable_value(&mut draft.worktree, true, "새 worktree").clicked() && draft.folder == self.root.to_string_lossy() { draft.folder = self.root.parent().unwrap_or(&self.root).join("new-task").to_string_lossy().into_owned(); } }); });
                        field(ui, if draft.worktree { "새 worktree 폴더" } else { "프로젝트 폴더" }, &mut draft.folder);
                        if !draft.worktree && widgets::button(ui, "폴더 선택…", ButtonKind::Secondary).clicked() { if let Some(path) = rfd::FileDialog::new().set_directory(&self.root).pick_folder() { draft.folder = path.to_string_lossy().into_owned(); } }
                        if self.snapshot.is_none() && self.pending.is_none() {
                            ui.label(RichText::new("Git 저장소가 아닌 폴더도 프로젝트로 열 수 있습니다.").small().color(t.text_dim));
                            ui.collapsing("Worktree 사용 안내",|ui| {ui.label("Worktree는 Git 저장소에서 작업을 분리할 때 사용합니다. Git 프로젝트를 연 뒤 다시 시도하세요.");if let Some(error)=&self.repo_error {ui.label(error);}});
                        }
                        if draft.worktree {
                            ui.label(RichText::new(format!("저장소: {}", self.root.display())).small().color(t.text_dim));
                            ui.checkbox(&mut draft.existing, "기존 로컬 브랜치 사용");
                            field(ui, "작업 브랜치", &mut draft.branch);
                            if !draft.existing {
                                field(ui, "시작 브랜치 / 커밋", &mut draft.base);
                                if let Some(snapshot) = &self.snapshot { egui::ComboBox::from_id_salt("task-base").selected_text("브랜치에서 선택…").show_ui(ui, |ui| { for r in &snapshot.refs { ui.selectable_value(&mut draft.base, r.clone(), r); } }); }
                            } else if let Some(snapshot) = &self.snapshot { egui::ComboBox::from_id_salt("task-branch").selected_text("로컬 브랜치 선택…").show_ui(ui, |ui| { for r in &snapshot.local_refs { ui.selectable_value(&mut draft.branch, r.clone(), r); } }); }
                            ui.label(RichText::new("기존 파일과 브랜치는 덮어쓰지 않습니다. 의존성 설치는 새 폴더에서 진행하세요.").small().color(t.text_dim));
                        }
                        if !draft.worktree {
                        ui.add_space(10.0); ui.label("시작 방식");
                        ui.horizontal_wrapped(|ui| { ui.selectable_value(&mut draft.mode, StartMode::Terminal, "터미널"); ui.selectable_value(&mut draft.mode, StartMode::Agent, "AI 에이전트"); });
                        if draft.mode == StartMode::Agent {
                            ui.horizontal_wrapped(|ui| { for command in ["codex", "claude", "gemini"] { ui.selectable_value(&mut draft.command, command.into(), command); } });
                            field(ui, "에이전트 실행 명령", &mut draft.command);
                            let binary=draft.command.split_whitespace().next().unwrap_or("");
                            if let Some((_,path))=self.agent_bins.iter().find(|(name,_)| name==binary) {
                                if let Some(path)=path {
                                    ui.label(RichText::new(format!("실행 파일 확인: {}",path.display())).small().color(t.text_dim));
                                    if cfg!(unix) && ui.small_button("확인한 실행 경로 사용").clicked() {
                                        let args=draft.command.trim_start()[binary.len()..].to_string();
                                        draft.command=format!("'{}'{}",path.to_string_lossy().replace('\'',"'\\''"),args);
                                    }
                                } else {ui.label(RichText::new("앱에서 실행 파일을 찾지 못했습니다. 셸 별칭이나 사용자 경로는 직접 확인하세요.").small().color(t.orange));}
                            }
                            ui.label(RichText::new("실행 파일 확인은 로그인·실행 성공을 보장하지 않습니다.").small().color(t.text_dim));
                            ui.collapsing("설치와 상태 연결 안내",|ui| {
                                ui.horizontal_wrapped(|ui| {ui.hyperlink_to("Codex 설치","https://developers.openai.com/codex/cli/");ui.hyperlink_to("Claude Code 설치","https://code.claude.com/docs/en/setup");ui.hyperlink_to("Gemini CLI 설치","https://geminicli.com/docs/get-started/installation/");});
                                ui.label("시작 후 터미널에서 로그인을 진행하세요. Kiln은 에이전트 훅을 자동 설정하지 않습니다.");
                                ui.label("완료·입력 요청 상태를 연결하려면 터미널의 ‘연동’ 안내에서 테스트하세요. 신호를 연결하지 않은 에이전트는 상태 확인 안 됨으로 표시됩니다.");
                            });
                        }
                        }
                        ui.label(RichText::new(if draft.worktree { "먼저 worktree를 만듭니다. 완료 후 프로젝트 열기에서 실행 방식을 선택할 수 있습니다." } else { "선택한 폴더를 열고 터미널 또는 에이전트를 시작합니다." }).small().color(t.text_dim));
                    }
                    Screen::Detail { task, .. } => {
                        field(ui, "프로젝트 이름", &mut task.name);
                        ui.label(RichText::new("아래 상태와 메모는 이 프로젝트 폴더 전체에 적용됩니다.").small().color(t.text_dim));
                        ui.label(RichText::new(task.path.display().to_string()).small().color(t.text_dim));
                        if !task.base.is_empty() { ui.label(RichText::new(format!("시작 지점: {}", task.base)).small().color(t.text_dim)); }
                        ui.horizontal_wrapped(|ui| { for status in [TaskStatus::Planned, TaskStatus::Active, TaskStatus::Review, TaskStatus::Done] { ui.selectable_value(&mut task.status, status, status.label()); } });
                        let label = ui.label("프로젝트 메모"); ui.add(egui::TextEdit::multiline(&mut task.note).hint_text("다음 할 일, 재현 방법, 검토할 내용을 남기세요.").desired_rows(4).desired_width(f32::INFINITY)).labelled_by(label.id);
                        ui.label(RichText::new("보관은 목록만 정리합니다. 파일, 브랜치와 실행 중인 세션은 유지됩니다.").small().color(t.text_dim));
                    }
                    Screen::Discard(..) => { ui.label("저장하지 않은 프로젝트 이름과 메모를 버릴까요?"); ui.label("이미 생성된 폴더와 worktree는 유지됩니다."); }
                });
            });
            if self.load_failed {ui.label(RichText::new("프로젝트 기록 저장을 중단했습니다. 원본은 유지되며 일반 폴더는 기록 없이 열 수 있습니다.").small().color(t.orange));}
            if let Some(error) = &self.error { ui.add_space(6.0); egui::ScrollArea::vertical().id_salt("project-error").max_height(44.0).show(ui, |ui| { ui.label(RichText::new(error).color(t.red)); }); }
            ui.add_space(10.0); ui.separator(); ui.add_space(8.0);
            if self.creating { ui.horizontal(|ui| { ui.spinner(); ui.label("Worktree 생성 중… 잠시 기다려 주세요."); }); }
            else { ui.horizontal_wrapped(|ui| match &screen {
                Screen::List => { if widgets::button(ui, "새 프로젝트", ButtonKind::Primary).clicked() { intent = Intent::Compose; } if widgets::button(ui, "새로 고침", ButtonKind::Secondary).clicked() { intent = Intent::Refresh; } if self.load_failed && widgets::button(ui,"원본 백업 후 빈 목록 복구",ButtonKind::Secondary).clicked(){intent=Intent::RecoverRecords;} }
                Screen::Compose { draft, .. } => { if self.load_failed && !draft.worktree { if widgets::button(ui,"기록 저장 없이 열기",ButtonKind::Primary).clicked(){intent=Intent::OpenWithoutRecord;} } else if widgets::button(ui, if draft.worktree { "Worktree 만들기" } else { "프로젝트 열기" }, ButtonKind::Primary).clicked() { intent = Intent::Start; } if !self.load_failed && self.error.is_some() && !draft.worktree && widgets::button(ui,"기록 저장 없이 열기",ButtonKind::Secondary).clicked(){intent=Intent::OpenWithoutRecord;} if widgets::button(ui, "목록", ButtonKind::Secondary).clicked() { intent = Intent::List; } }
                Screen::Detail { task, .. } => {
                    if widgets::button(ui, "열기…", ButtonKind::Primary).clicked() { intent = Intent::Start; }
                    if widgets::button(ui, "메모 저장", ButtonKind::Secondary).clicked() { intent = Intent::Save; }
                    if widgets::button(ui, if task.archived { "복원" } else { "보관" }, ButtonKind::Secondary).clicked() { intent = if task.archived { Intent::Restore } else { Intent::Archive }; }
                    if widgets::button(ui, "목록", ButtonKind::Ghost).clicked() { intent = Intent::List; }
                }
                Screen::Discard(..) => { if widgets::button(ui, "편집 계속", ButtonKind::Primary).clicked() { intent = Intent::Resume; } if widgets::button(ui, "변경 버리기", ButtonKind::Danger).clicked() { intent = Intent::Discard; } }
            }); }
        });
        self.screen = screen;
        if modal.should_close() && matches!(intent, Intent::None) {
            intent = if matches!(self.screen, Screen::Discard(..)) {
                Intent::Resume
            } else {
                Intent::Close
            };
        }
        self.apply(intent)
    }
    fn apply(&mut self, intent: Intent) -> Option<ProjectAction> {
        if self.creating {
            return None;
        }
        match intent {
            Intent::None => (),
            Intent::RecoverRecords => match self.recover_records() {
                Ok(backup)=>self.notice=Some(format!("빈 프로젝트 목록으로 복구했습니다. 원본 백업: {}",backup.display())),
                Err(error)=>self.error=Some(error),
            },
            Intent::OpenWithoutRecord => {
                if let Screen::Compose{draft,..}=&self.screen {
                    if draft.worktree {self.error=Some("기록 없이 열기는 기존 폴더에서만 사용할 수 있습니다.".into());return None;}
                    match draft.validate() {
                        Ok((path,name,command))=>{self.open=false;self.screen=Screen::List;return Some(ProjectAction::Open{path,name,command});}
                        Err(error)=>self.error=Some(error),
                    }
                }
            },
            Intent::Close | Intent::List => {
                if self.has_unsaved_edits() {
                    self.screen = Screen::Discard(
                        Box::new(self.screen.clone()),
                        matches!(intent, Intent::Close),
                    );
                } else {
                    if matches!(intent, Intent::Close) {
                        self.open = false;
                    }
                    self.screen = Screen::List;
                    self.error = None;
                }
            }
            Intent::Discard => {
                if matches!(self.screen, Screen::Discard(_, true)) {
                    self.open = false;
                }
                self.screen = Screen::List;
                self.error = None;
            }
            Intent::Resume => {
                if let Screen::Discard(screen, _) = self.screen.clone() {
                    self.screen = *screen;
                }
            }
            Intent::Refresh => {
                self.reload();
                self.notice = None;
                self.refresh();
            }
            Intent::Compose => {
                let draft = Draft::new(&self.root);
                self.screen = Screen::Compose {
                    original: draft.clone(),
                    draft,
                };
                self.focus_name = true;
                self.error = None;
            }
            Intent::OpenSaved(task) => {
                // This shortcut belongs to saved rows only; never replace an in-progress editor.
                if !matches!(self.screen, Screen::List) || task.archived || !self.tasks.contains(&task) {
                    return None;
                }
                let mut draft = Draft::new(&task.path);
                draft.name = task.name.clone();
                if let Some(command) = &task.command {
                    draft.mode = StartMode::Agent;
                    draft.command = command.clone();
                }
                match draft.validate() {
                    Ok((path, name, command)) => {
                        self.open = false;
                        self.error = None;
                        return Some(ProjectAction::Open { path, name, command });
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            Intent::Detail(task) => {
                self.screen = Screen::Detail {
                    original: task.clone(),
                    task,
                };
                self.error = None;
            }
            Intent::Save | Intent::Archive | Intent::Restore => {
                if let Screen::Detail { mut task, .. } = self.screen.clone() {
                    if matches!(intent, Intent::Archive) {
                        task.archived = true;
                    }
                    if matches!(intent, Intent::Restore) {
                        task.archived = false;
                    }
                    match self.save_task(task.clone()) {
                        Ok(()) => {
                            self.screen = if matches!(intent, Intent::Save) {
                                Screen::Detail {
                                    original: task.clone(),
                                    task,
                                }
                            } else {
                                Screen::List
                            };
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
            }
            Intent::Start => match self.screen.clone() {
                Screen::Detail { task, .. } => {
                    if let Err(e) = self.save_task(task.clone()) {
                        self.error = Some(e);
                        return None;
                    }
                    let mut draft = Draft::new(&task.path);
                    draft.name = task.name;
                    if let Some(command) = task.command {
                        draft.mode = StartMode::Agent;
                        draft.command = command;
                    }
                    self.screen = Screen::Compose {
                        original: draft.clone(),
                        draft,
                    };
                    self.error = None;
                }
                Screen::Compose { draft, .. } => match draft.validate() {
                    Err(e) => self.error = Some(e),
                    Ok((path, name, command)) => {
                        let old = self.tasks.iter().find(|task| task.path == path);
                        let task = Task {
                            path: path.clone(),
                            name: name.clone(),
                            note: old.map(|t| t.note.clone()).unwrap_or_default(),
                            status: old.map(|t| t.status).unwrap_or_default(),
                            archived: false,
                            base: if draft.worktree {
                                draft.base.clone()
                            } else {
                                old.map(|t| t.base.clone()).unwrap_or_default()
                            },
                            command: command.clone(),
                        };
                        if draft.worktree {
                            if self.pending.is_some() {
                                self.error = Some("저장소 확인을 마친 뒤 다시 시도하세요.".into());
                                return None;
                            }
                            let root = self.root.clone();
                            let request = Create {
                                path,
                                branch: draft.branch,
                                base: draft.base,
                                existing_branch: draft.existing,
                            };
                            let (tx, rx) = mpsc::channel();
                            self.pending = Some(rx);
                            self.creating = true;
                            self.error = None;
                            std::thread::spawn(move || {
                                let result = worktrees::create(&root, &request)
                                    .map(|tree| {
                                        let mut task = task;
                                        task.path = tree.path.clone();
                                        (tree, task)
                                    })
                                    .map_err(|e| e.to_string());
                                let _ = tx.send(JobResult::Created(result));
                            });
                        } else {
                            match self.save_task(task) {
                                Ok(()) => {
                                    self.open = false;
                                    self.screen = Screen::List;
                                    return Some(ProjectAction::Open {
                                        path,
                                        name,
                                        command,
                                    });
                                }
                                Err(e) => self.error = Some(e),
                            }
                        }
                    }
                },
                _ => (),
            },
        }
        None
    }
}
fn field(ui: &mut egui::Ui, title: &str, text: &mut String) {
    ui.add_space(6.0);
    let label = ui.label(title);
    ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
        .labelled_by(label.id);
}
fn expand_path(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err("폴더의 전체 경로를 입력하세요.".into());
    }
    let path = if raw == "~" || raw.starts_with("~/") {
        PathBuf::from(std::env::var_os("HOME").ok_or("홈 폴더를 찾지 못했습니다.")?)
            .join(raw.strip_prefix("~/").unwrap_or(""))
    } else {
        raw.into()
    };
    if !path.is_absolute() {
        return Err("전체 경로 또는 ~/로 시작하는 경로를 입력하세요.".into());
    }
    Ok(path)
}
fn validate_task(task: &Task) -> Result<(), String> {
    if !task.path.is_absolute()
        || task.name.trim().is_empty()
        || task.name.chars().count() > 100
        || task.name.chars().any(char::is_control)
        || task.note.len() > 65536
        || task.base.len() > 4096
        || task.command.as_ref().is_some_and(|s| {
            s.trim().is_empty() || s.len() > 16384 || s.chars().any(char::is_control)
        })
    {
        return Err("프로젝트 이름은 100자, 메모는 64KB 이내로 입력하세요.".into());
    }
    Ok(())
}
fn read_tasks(path: &Path) -> Result<(Vec<Task>, Option<Vec<u8>>), String> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 4 * 1024 * 1024 => return Err("프로젝트 기록이 4MB보다 큽니다.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], None)),
        Err(e) => return Err(e.to_string()),
        _ => (),
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("프로젝트 기록이 4MB보다 큽니다.".into());
    }
    let tasks: Vec<Task> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if tasks.len() > 1000 {
        return Err("프로젝트 기록 수 제한을 초과했습니다.".into());
    }
    for task in &tasks {
        validate_task(task)?;
    }
    Ok((tasks, Some(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(path: &Path) -> Task {
        Task {
            path: path.into(),
            name: "로그인 수정".into(),
            note: "재현 완료".into(),
            status: TaskStatus::Review,
            archived: false,
            base: "main".into(),
            command: None,
        }
    }
    #[test]
    fn corrupt_project_records_do_not_block_folder_open_and_recovery_preserves_original() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("projects.json");
        let original=b"broken project records";std::fs::write(&path,original).unwrap();
        let mut p=Projects::from_path(path.clone());p.root=dir.path().into();p.open=true;p.apply(Intent::Compose);
        if let Screen::Compose{draft,..}=&mut p.screen {draft.name="Recovered folder".into();}
        assert!(p.apply(Intent::Start).is_none());
        assert!(matches!(p.apply(Intent::OpenWithoutRecord),Some(ProjectAction::Open{command:None,..})));
        assert_eq!(std::fs::read(&path).unwrap(),original);
        let backup=p.recover_records().unwrap();assert_eq!(std::fs::read(backup).unwrap(),original);assert_eq!(std::fs::read(&path).unwrap(),b"[]");assert!(!p.load_failed);
        std::fs::write(&path,b"[]\n").unwrap();p.open=true;p.apply(Intent::Compose);
        if let Screen::Compose{draft,..}=&mut p.screen {draft.name="Unsaved metadata".into();}
        assert!(!p.load_failed);assert!(p.apply(Intent::Start).is_none());
        assert!(matches!(p.apply(Intent::OpenWithoutRecord),Some(ProjectAction::Open{..})));
        assert_eq!(std::fs::read(&path).unwrap(),b"[]\n");
        std::fs::write(&path,original).unwrap();p.reload();std::fs::write(&path,b"external edit").unwrap();
        assert!(p.recover_records().is_err());assert_eq!(std::fs::read(&path).unwrap(),b"external edit");
        std::fs::remove_file(&path).unwrap();std::fs::create_dir(&path).unwrap();p.reload();assert!(p.recover_records().is_err());assert!(path.is_dir());
    }
    #[test]
    fn damaged_record_actions_fit_minimum_scaled_window() {
        use egui_kittest::{Harness,kittest::Queryable};
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("projects.json");std::fs::write(&path,b"bad").unwrap();
        let mut p=Projects::from_path(path);p.root=dir.path().into();p.open=true;let mut initialized=false;
        let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).build_ui_state(|ui,p:&mut Projects|{
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            assert!(p.ui(ui.ctx()).is_none());
        },p);
        h.run_steps(3);
        for label in ["원본 백업 후 빈 목록 복구","새 프로젝트"] {assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}");}
        h.render().unwrap().save("/tmp/kiln-project-record-recovery-small.png").unwrap();
        h.get_by_label("새 프로젝트").click();h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("기록 저장 없이 열기").rect()));
        h.render().unwrap().save("/tmp/kiln-project-record-fallback-small.png").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn agent_discovery_only_accepts_executable_files_and_preserves_path_priority() {
        use std::os::unix::fs::PermissionsExt;
        let first=tempfile::tempdir().unwrap();let second=tempfile::tempdir().unwrap();
        let a=first.path().join("codex");let b=second.path().join("codex");
        std::fs::write(&a,"#!/bin/sh\nexit 1").unwrap();std::fs::set_permissions(&a,std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&b,"#!/bin/sh\nexit 1").unwrap();std::fs::set_permissions(&b,std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs=vec![first.path().into(),second.path().into()];
        assert_eq!(executable_in("codex",&dirs),Some(b));
        std::fs::set_permissions(&a,std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(executable_in("codex",&dirs),Some(a));
        assert!(executable_in("not-installed",&dirs).is_none());
        assert!(executable_in("codex",&[PathBuf::from(".")]).is_none());
    }
    #[test]
    fn non_git_initial_folder_keeps_normal_task_creation_available() {
        let dir=tempfile::tempdir().unwrap();let mut p=Projects::from_path(dir.path().join("projects.json"));
        p.root=dir.path().into();p.open=true;
        let (tx,rx)=mpsc::channel();tx.send(JobResult::Loaded(Err("Git 저장소가 아닙니다".into()))).unwrap();p.pending=Some(rx);
        p.poll(&egui::Context::default());assert!(p.notice.is_none());assert!(p.error.is_none());assert!(p.repo_error.is_some());
        p.apply(Intent::Compose);
        if let Screen::Compose{draft,..}=&mut p.screen {draft.name="첫 작업".into();draft.mode=StartMode::Terminal;}
        assert!(matches!(p.apply(Intent::Start),Some(ProjectAction::Open{command:None,..})));
    }

    #[test]
    fn archive_preserves_files_and_restores_notes_and_status() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("keep.txt");
        std::fs::write(&file, "unchanged").unwrap();
        let path = d.path().join("projects.json");
        let mut p = Projects::from_path(path.clone());
        let t = task(d.path());
        p.save_task(t.clone()).unwrap();
        p.screen = Screen::Detail {
            task: t.clone(),
            original: t.clone(),
        };
        p.apply(Intent::Archive);
        assert!(p.tasks[0].archived);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "unchanged");
        let mut restored = Projects::from_path(path);
        let t = restored.tasks[0].clone();
        restored.screen = Screen::Detail {
            task: t.clone(),
            original: t,
        };
        restored.apply(Intent::Restore);
        assert!(!restored.tasks[0].archived);
        assert_eq!(restored.tasks[0].note, "재현 완료");
        assert_eq!(restored.tasks[0].status, TaskStatus::Review);
    }
    #[test]
    fn corrupt_and_external_changes_are_preserved() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("projects.json");
        std::fs::write(&path, "broken").unwrap();
        let mut p = Projects::from_path(path.clone());
        assert!(p.save_task(task(d.path())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
        std::fs::remove_file(&path).unwrap();
        p.reload();
        p.save_task(task(d.path())).unwrap();
        std::fs::write(&path, "[]").unwrap();
        assert!(p.save_task(task(d.path())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    }
    #[test]
    fn saved_row_opens_directly_with_saved_command_and_preserves_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Projects::from_path(dir.path().join("projects.json"));
        let mut saved = task(dir.path());
        saved.command = Some("codex --model example".into());
        p.save_task(saved.clone()).unwrap();
        p.open = true;
        assert_eq!(p.apply(Intent::OpenSaved(saved.clone())), Some(ProjectAction::Open {
            path: dir.path().canonicalize().unwrap(), name: saved.name.clone(), command: saved.command.clone(),
        }));
        p.open = true;
        p.apply(Intent::Compose);
        if let Screen::Compose { draft, .. } = &mut p.screen { draft.name = "unsaved".into(); }
        assert!(p.apply(Intent::OpenSaved(saved)).is_none());
        assert!(p.has_unsaved_edits());
    }
    #[test]
    fn saved_row_rejects_missing_directory_or_unrecorded_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Projects::from_path(dir.path().join("projects.json"));
        let saved = task(dir.path());
        p.save_task(saved.clone()).unwrap();
        let mut unrecorded = saved;
        unrecorded.command = Some("arbitrary command".into());
        assert!(p.apply(Intent::OpenSaved(unrecorded)).is_none());
        let mut missing = task(&dir.path().join("missing"));
        missing.name = "missing".into();
        p.tasks.push(missing.clone());
        p.open = true;
        assert!(p.apply(Intent::OpenSaved(missing)).is_none());
        assert!(p.open);
        assert!(p.error.is_some());
    }
    #[test]
    fn draft_discard_cancel_and_explicit_agent_start() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Projects::from_path(d.path().join("projects.json"));
        p.root = d.path().into();
        p.open = true;
        p.apply(Intent::Compose);
        if let Screen::Compose { draft, .. } = &mut p.screen {
            draft.name = "agent task".into();
            draft.mode = StartMode::Agent;
            draft.command = "codex --model example".into();
        }
        assert!(p.has_unsaved_edits());
        assert!(p.apply(Intent::Close).is_none());
        assert!(p.has_unsaved_edits());
        assert!(matches!(p.screen, Screen::Discard(..)));
        p.apply(Intent::Resume);
        assert!(p.has_unsaved_edits());
        assert_eq!(
            p.apply(Intent::Start),
            Some(ProjectAction::Open {
                path: d.path().canonicalize().unwrap(),
                name: "agent task".into(),
                command: Some("codex --model example".into())
            })
        );
        assert!(!p.is_open());
        assert!(!p.has_unsaved_edits());
    }
    #[test]
    fn worktree_creation_does_not_validate_hidden_agent_command() {
        let dir=tempfile::tempdir().unwrap(); let mut draft=Draft::new(dir.path());
        draft.name="feature".into(); draft.mode=StartMode::Agent; draft.command.clear();
        assert!(draft.validate().is_err(),"visible agent mode requires a command");
        draft.worktree=true; draft.branch="feature".into(); draft.base="main".into();
        draft.folder=dir.path().join("worktree").display().to_string();
        assert_eq!(draft.validate().unwrap().2,None,"creation must defer agent selection to the open step");
        draft.command="invalid\ncommand".into();
        assert!(draft.validate().is_ok(),"hidden stale fields cannot prevent creation");
        draft.branch.clear();assert!(draft.validate().is_err(),"visible worktree branch remains required");
    }
    #[test]
    fn invalid_paths_and_agent_controls_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let mut draft = Draft::new(d.path());
        draft.name = "task".into();
        draft.folder = "relative".into();
        assert!(draft.validate().is_err());
        draft.folder = d.path().display().to_string();
        draft.mode = StartMode::Agent;
        draft.command = "codex\nrm anything".into();
        assert!(draft.validate().is_err());
        draft.mode = StartMode::Terminal;
        assert_eq!(draft.validate().unwrap().2, None);
    }
    #[test]
    fn minimum_scaled_window_keeps_footer_and_errors_visible() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        let mut p = Projects::from_path(dir.path().join("projects.json"));
        p.open = true;
        p.root = dir.path().into();
        p.apply(Intent::Compose);
        let mut initialized = false;
        let mut h = Harness::builder()
            .with_size([720.0 / 1.3, 440.0 / 1.3])
            .build_ui_state(
                |ui, state: &mut Projects| {
                    if !initialized {
                        fonts::install(ui.ctx());
                        Theme::current().apply(ui.ctx());
                        initialized = true;
                        return;
                    }
                    assert!(state.ui(ui.ctx()).is_none());
                },
                p,
            );
        h.run_steps(3);
        h.get_by_label("프로젝트 열기").click();
        h.run_steps(3);
        assert!(
            h.ctx
                .content_rect()
                .contains_rect(h.get_by_label("프로젝트 이름을 1~100자로 입력하세요.").rect())
        );
        assert!(
            h.ctx
                .content_rect()
                .contains_rect(h.get_by_label("프로젝트 열기").rect())
        );
        h.render()
            .unwrap()
            .save("/tmp/kiln-project-composer-small.png")
            .unwrap();
    }
    #[test]
    fn background_creation_preserves_agent_choice_without_executing_it() {
        let repo = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        ] {
            let result = std::process::Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(result.status.success());
        }
        let mut p = Projects::from_path(out.path().join("projects.json"));
        p.open = true;
        p.root = repo.path().into();
        p.snapshot = Some(worktrees::inspect(repo.path()).unwrap());
        p.apply(Intent::Compose);
        if let Screen::Compose { draft, .. } = &mut p.screen {
            draft.name = "Async task".into();
            draft.worktree = true;
            draft.folder = out.path().join("task").display().to_string();
            draft.branch = "feature/async".into();
            draft.base = "main".into();
            draft.mode = StartMode::Agent;
            draft.command = "codex".into();
        }
        assert!(p.apply(Intent::Start).is_none());
        assert!(p.is_busy());
        assert!(p.apply(Intent::Close).is_none());
        assert!(p.is_open());
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while p.pending.is_some() && std::time::Instant::now() < deadline {
            p.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!p.is_busy());
        assert!(p.error.is_none(), "{:?}", p.error);
        assert!(matches!(p.screen, Screen::Detail { .. }));
        assert_eq!(p.tasks[0].command, None, "creation does not retain a hidden start command");
        assert!(p.apply(Intent::Start).is_none());
        assert!(matches!(&p.screen, Screen::Compose { draft, .. } if draft.mode == StartMode::Terminal));
        // Start mode is selected once, after creation, before explicit opening.
        if let Screen::Compose { draft, .. } = &mut p.screen {
            draft.mode = StartMode::Agent; draft.command = "codex".into();
        }
        assert!(matches!(
            p.apply(Intent::Start),
            Some(ProjectAction::Open {
                command: Some(_),
                ..
            })
        ));
    }
    #[test]
    fn list_detail_and_worktree_screens_fit_minimum_scaled_viewport() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir = tempfile::tempdir().unwrap();
        for screen in ["list", "detail", "worktree"] {
            let mut p = Projects::from_path(dir.path().join(format!("{screen}.json")));
            p.open = true;
            p.root = dir.path().into();
            let task = task(dir.path());
            p.save_task(task.clone()).unwrap();
            if screen == "detail" {
                p.apply(Intent::Detail(task));
            }
            if screen == "worktree" {
                p.snapshot = Some(Snapshot {
                    root: dir.path().into(),
                    trees: vec![],
                    refs: vec!["main".into()],
                    local_refs: vec!["main".into()],
                });
                p.apply(Intent::Compose);
                if let Screen::Compose { draft, .. } = &mut p.screen {
                    draft.worktree = true;
                    draft.name = "새 기능".into();
                    draft.branch = "feature/new".into();
                }
            }
            let mut initialized = false;
            let mut h = Harness::builder()
                .with_size([720.0 / 1.3, 440.0 / 1.3])
                .build_ui_state(
                    |ui, state: &mut Projects| {
                        if !initialized {
                            fonts::install(ui.ctx());
                            Theme::current().apply(ui.ctx());
                            initialized = true;
                            return;
                        }
                        assert!(state.ui(ui.ctx()).is_none());
                    },
                    p,
                );
            h.run_steps(3);
            let label = match screen {
                "list" => "새 프로젝트",
                "detail" => "메모 저장",
                _ => "Worktree 만들기",
            };
            assert!(
                h.ctx
                    .content_rect()
                    .contains_rect(h.get_by_label(label).rect())
            );
            h.render()
                .unwrap()
                .save(format!("/tmp/kiln-project-{screen}-small.png"))
                .unwrap();
        }
    }
}
