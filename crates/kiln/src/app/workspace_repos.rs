//! One parent workspace, many repositories. A detail view never changes the agent cwd.
use super::{
    Action,
    state::LocalRepositoryDrafts,
    tools::{ToolKind, diff_factory, git_event_action, history_factory, pr_factory},
};
use egui::{RichText, Ui};
use kiln_common::{Task, Theme, icons::Icon, widgets::icon_button};
use kiln_git::{
    GhBackend, GitError, GitPanel, GithubBackend, GithubHub, PrBackend, PrFilter, PrItem,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn agent_choice(ui: &mut Ui, selected: bool, label: &str, icon: Icon) -> egui::Response {
    let theme = Theme::current();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(100.0, 32.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let fill = if selected {
        theme.bg_selected
    } else if response.hovered() {
        theme.bg_hover
    } else {
        theme.bg_elevated
    };
    ui.painter().rect_filled(rect, 6, fill);
    if selected {
        ui.painter().rect_stroke(
            rect,
            6,
            egui::Stroke::new(1.0, theme.accent),
            egui::StrokeKind::Inside,
        );
    }
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    kiln_common::icons::paint(ui.painter(), mark, icon, theme.text);
    ui.painter().text(
        egui::pos2(rect.left() + 33.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        theme.text,
    );
    kiln_common::widgets::focus_ring(ui, &response, 6);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

type RepositoryStatus = (
    Result<kiln_git::status::Status, String>,
    Option<kiln_git::worktrees::WorktreeIdentity>,
);

struct Repository {
    root: PathBuf,
    status: Option<Result<kiln_git::status::Status, String>>,
    status_task: Option<Task<RepositoryStatus>>,
    status_dirty: bool,
    status_at: Option<Instant>,
    worktree: Option<kiln_git::worktrees::WorktreeIdentity>,
    expanded: bool,
    git: Option<GitPanel>,
    hub: Option<GithubHub>,
    draft: LocalRepositoryDrafts,
    prs: Option<Result<Vec<PrItem>, GitError>>,
    pr_task: Option<Task<Result<Vec<PrItem>, GitError>>>,
    github_target: Option<String>,
    github_target_task: Option<Task<Result<String, GitError>>>,
    pr_at: Option<Instant>,
    github_refresh_at: Option<Instant>,
    github_target_dirty: bool,
    pr_dirty: bool,
    github_refresh_error: Option<GitError>,
    github_hub_stale: bool,
}
impl Repository {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            status: None,
            status_task: None,
            status_dirty: false,
            status_at: None,
            worktree: None,
            expanded: false,
            git: None,
            hub: None,
            draft: LocalRepositoryDrafts::default(),
            prs: None,
            pr_task: None,
            github_target: None,
            github_target_task: None,
            pr_at: None,
            github_refresh_at: None,
            github_target_dirty: false,
            pr_dirty: false,
            github_refresh_error: None,
            github_hub_stale: false,
        }
    }
    fn github_error(&self) -> Option<&GitError> {
        self.github_refresh_error
            .as_ref()
            .or_else(|| self.prs.as_ref().and_then(|result| result.as_ref().err()))
    }
    fn busy(&mut self) -> bool {
        self.git.as_mut().is_some_and(GitPanel::is_busy)
            || self.hub.as_ref().is_some_and(GithubHub::is_submitting)
    }
    fn drafts(&self) -> LocalRepositoryDrafts {
        LocalRepositoryDrafts {
            commit_message: self
                .git
                .as_ref()
                .map(|g| g.commit_draft().to_owned())
                .unwrap_or_else(|| self.draft.commit_message.clone()),
            github: self
                .hub
                .as_ref()
                .map(GithubHub::recovery_drafts)
                .unwrap_or_else(|| self.draft.github.clone()),
        }
    }
}
pub(super) struct RepositoryWorkspace {
    root: PathBuf,
    ctx: egui::Context,
    scan: Option<Task<kiln_git::discovery::Inventory>>,
    inventory: Option<kiln_git::discovery::Inventory>,
    repos: BTreeMap<PathBuf, Repository>,
    detail: Option<PathBuf>,
    filter: String,
    show_all_git: bool,
    show_all_github: bool,
    git_review_order: Vec<PathBuf>,
    github_review_order: Vec<PathBuf>,
    agent_program: String,
    prompt: String,
    context_open: bool,
    focus_prompt: bool,
    discard: Option<PathBuf>,
    context_cache: String,
    last_refresh: Instant,
    last_scan: Instant,
}
impl RepositoryWorkspace {
    pub fn new(root: &Path, ctx: &egui::Context) -> Self {
        let path = root.to_path_buf();
        let scan = Task::spawn(ctx, move || kiln_git::discovery::discover(&path));
        Self {
            root: root.into(),
            ctx: ctx.clone(),
            scan: Some(scan),
            inventory: None,
            repos: BTreeMap::new(),
            detail: None,
            filter: String::new(),
            show_all_git: false,
            show_all_github: false,
            git_review_order: vec![],
            github_review_order: vec![],
            agent_program: "codex".into(),
            prompt: String::new(),
            context_open: false,
            focus_prompt: false,
            discard: None,
            context_cache: String::new(),
            last_refresh: Instant::now(),
            last_scan: Instant::now(),
        }
    }
    pub fn loading(&self) -> bool {
        self.inventory.is_none()
    }
    pub fn decorations(&self) -> std::collections::HashMap<PathBuf, kiln_editor::Decoration> {
        let mut result = std::collections::HashMap::new();
        let t = kiln_common::Theme::current();
        for (path, repo) in &self.repos {
            if !self
                .inventory
                .as_ref()
                .is_some_and(|i| i.roots.contains(path))
            {
                continue;
            }
            if let Some(Ok(status)) = &repo.status {
                for entry in &status.entries {
                    if entry.kind == kiln_git::status::EntryKind::Ignored {
                        continue;
                    }
                    let ch = kiln_git::status::decoration_char(entry);
                    let color = match ch {
                        'M' => t.yellow,
                        'A' | '?' => t.green,
                        'D' => t.red,
                        'R' => t.blue,
                        'C' | 'U' => t.orange,
                        'T' => t.purple,
                        _ => t.text_dim,
                    };
                    result.insert(
                        path.join(entry.path.trim_end_matches('/')),
                        kiln_editor::Decoration {
                            color,
                            badge: Some(ch),
                        },
                    );
                }
            }
        }
        result
    }
    pub fn summary(&self) -> Option<super::tools::RepoLine> {
        let inventory = self.inventory.as_ref()?;
        let mut dirty = 0;
        let mut ahead = 0;
        let mut behind = 0;
        for path in &inventory.roots {
            if let Some(Ok(status)) = self.repos.get(path).and_then(|r| r.status.as_ref()) {
                dirty += status.changed_count() + status.conflicted_count();
                ahead += status.branch.ahead;
                behind += status.branch.behind;
            }
        }
        Some(super::tools::RepoLine {
            branch: kiln_common::trf!("{}개 저장소", inventory.roots.len()),
            dirty,
            ahead,
            behind,
            pr: None,
        })
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
    pub fn restore_prompt(&mut self, text: &str) {
        self.prompt = text.into();
        self.context_open = !text.is_empty();
    }
    pub fn clear_prompt_if(&mut self, request: &str) {
        if self.prompt == request {
            self.prompt.clear();
        }
    }
    pub fn open_agent_task(&mut self) {
        self.detail = None;
        self.context_open = true;
        self.focus_prompt = true;
    }
    pub fn is_agent_task_open(&self) -> bool {
        self.context_open
    }
    pub fn close_agent_task(&mut self) {
        self.context_open = false;
    }
    pub fn is_multi(&self) -> bool {
        if self.context_open || !self.prompt.is_empty() || !self.drafts().is_empty() {
            return true;
        }
        self.inventory
            .as_ref()
            .is_some_and(|i| i.roots.len() != 1 || i.roots.first().is_some_and(|p| p != &self.root))
    }
    fn accept_inventory(&mut self, found: kiln_git::discovery::Inventory) {
        for path in &found.roots {
            self.repos
                .entry(path.clone())
                .or_insert_with(|| Repository::new(path.clone()));
        }
        if self
            .detail
            .as_ref()
            .is_some_and(|path| !found.roots.contains(path))
        {
            let busy = self
                .detail
                .as_ref()
                .and_then(|path| self.repos.get_mut(path))
                .is_some_and(Repository::busy);
            if !busy {
                self.detail = None;
            }
        }
        self.inventory = Some(found);
        self.context_cache = self.context();
        self.last_scan = Instant::now();
        self.scan = None;
    }
    pub fn tick(&mut self) {
        if let Some(found) = self.scan.as_mut().and_then(Task::take) {
            self.accept_inventory(found);
        }
        // Agents can create or remove sibling repositories without reopening the workspace.
        if self.scan.is_none() && self.last_scan.elapsed() >= Duration::from_secs(60) {
            let root = self.root.clone();
            self.scan = Some(Task::spawn(&self.ctx, move || {
                kiln_git::discovery::discover(&root)
            }));
        }
        let refresh = self.last_refresh.elapsed() > Duration::from_secs(15);
        if refresh {
            self.last_refresh = Instant::now();
        }
        for repo in self.repos.values_mut() {
            if let Some((result, worktree)) = repo.status_task.as_mut().and_then(Task::take) {
                repo.status = Some(result);
                repo.worktree = worktree;
                repo.status_at = Some(Instant::now());
                repo.status_task = None;
                repo.status_dirty = false;
            }
            if let Some(result) = repo.github_target_task.as_mut().and_then(Task::take) {
                repo.github_target_task = None;
                repo.github_target_dirty = false;
                repo.github_refresh_at = Some(Instant::now());
                match result {
                    Ok(target) => {
                        if repo
                            .github_target
                            .as_ref()
                            .is_some_and(|old| old != &target)
                        {
                            repo.prs = None;
                            repo.pr_at = None;
                            repo.github_hub_stale = true;
                        }
                        repo.github_target = Some(target);
                        repo.pr_dirty |= repo
                            .pr_at
                            .is_none_or(|at| at.elapsed() > Duration::from_secs(60));
                    }
                    Err(error) => {
                        repo.github_refresh_error = Some(error.clone());
                        if repo.prs.is_none() {
                            repo.prs = Some(Err(error));
                        }
                    }
                }
            }
            if let Some(result) = repo.pr_task.as_mut().and_then(Task::take) {
                match result {
                    Ok(prs) => {
                        repo.prs = Some(Ok(prs));
                        repo.github_refresh_error = None;
                    }
                    Err(error) => {
                        repo.github_refresh_error = Some(error.clone());
                        if !matches!(repo.prs, Some(Ok(_))) {
                            repo.prs = Some(Err(error));
                        }
                    }
                }
                repo.pr_at = Some(Instant::now());
                repo.github_refresh_at = repo.pr_at;
                repo.pr_dirty = false;
                repo.pr_task = None;
            }
            if repo
                .github_refresh_at
                .or(repo.pr_at)
                .is_some_and(|at| at.elapsed() > Duration::from_secs(60))
            {
                repo.github_target_dirty = true;
                repo.github_refresh_at = Some(Instant::now());
            }
            if repo.github_hub_stale && !repo.hub.as_ref().is_some_and(GithubHub::is_submitting) {
                if let Some(hub) = repo.hub.take() {
                    repo.draft.github = hub.recovery_drafts();
                }
                // Keep drafts and an explicit user-selected repository. When selected is
                // None, the recreated hub follows this checkout's newly detected target.
                repo.github_hub_stale = false;
            }
            if refresh {
                repo.status_dirty = true;
            }
        }
        // Share results by gh's host-qualified repository URL, including an in-flight
        // request that began before a second worktree finished resolving its target.
        let mut github_results = BTreeMap::new();
        for repo in self.repos.values() {
            if let (Some(target), Some(result), Some(at)) =
                (&repo.github_target, &repo.prs, repo.pr_at)
            {
                let cached = github_results.entry(target.clone()).or_insert((
                    at,
                    result.clone(),
                    repo.github_refresh_error.clone(),
                ));
                if at > cached.0 {
                    *cached = (at, result.clone(), repo.github_refresh_error.clone());
                }
            }
        }
        for repo in self.repos.values_mut() {
            if let Some((at, result, error)) = repo
                .github_target
                .as_ref()
                .and_then(|target| github_results.get(target))
            {
                if repo.pr_at.is_none_or(|old| *at > old) {
                    repo.pr_at = Some(*at);
                    repo.prs = Some(result.clone());
                    repo.github_refresh_error = error.clone();
                    repo.pr_dirty = false;
                }
            }
        }
        let jobs = self
            .repos
            .values()
            .filter(|r| r.status_task.is_some())
            .count();
        for path in self
            .pending_status_roots()
            .into_iter()
            .take(4usize.saturating_sub(jobs))
        {
            let repo = self.repos.get_mut(&path).expect("queued repository exists");
            repo.status_dirty = false;
            repo.status_task = Some(Task::spawn(&self.ctx, move || {
                let status = kiln_git::repo::status(&path).map_err(|e| e.to_string());
                let worktree = kiln_git::worktrees::identity(&path).ok().flatten();
                (status, worktree)
            }));
        }
    }
    // A worktree or clone is only coalesced after gh resolves its actual host/repository.
    // Local folder names, common Git directories and origin URLs are insufficient:
    // gh may select a different default remote for each local checkout.
    fn github_roots(&self) -> Vec<PathBuf> {
        let ordered = self.ordered_roots();
        let mut representatives = BTreeMap::new();
        for path in self.repos.keys().filter(|path| ordered.contains(path)) {
            if let Some(target) = self.repos[path]
                .github_target
                .as_ref()
                .filter(|s| !s.is_empty())
            {
                representatives
                    .entry(target.to_lowercase())
                    .or_insert(path.clone());
            }
        }
        ordered
            .into_iter()
            .filter(|path| {
                self.repos[path]
                    .github_target
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .is_none_or(|target| representatives.get(&target.to_lowercase()) == Some(path))
            })
            .collect()
    }
    fn github_aliases(&self, path: &Path) -> Vec<PathBuf> {
        let Some(target) = self
            .repos
            .get(path)
            .and_then(|repo| repo.github_target.as_ref())
        else {
            return vec![path.to_path_buf()];
        };
        self.inventory
            .as_ref()
            .into_iter()
            .flat_map(|i| &i.roots)
            .filter(|other| {
                self.repos
                    .get(*other)
                    .and_then(|repo| repo.github_target.as_ref())
                    == Some(target)
            })
            .cloned()
            .collect()
    }
    fn start_github_jobs(&mut self, ctx: &egui::Context) {
        let mut jobs = self
            .repos
            .values()
            .filter(|r| r.pr_task.is_some() || r.github_target_task.is_some())
            .count();
        let dirty_targets: std::collections::BTreeSet<_> = self
            .repos
            .values()
            .filter(|repo| repo.pr_dirty)
            .filter_map(|repo| repo.github_target.clone())
            .collect();
        let mut in_flight: std::collections::BTreeSet<_> = self
            .repos
            .values()
            .filter(|repo| repo.pr_task.is_some())
            .filter_map(|repo| repo.github_target.clone())
            .collect();
        for path in self.github_roots() {
            if jobs >= 4 {
                break;
            }
            let repo = self.repos.get_mut(&path).unwrap();
            let Some(target) = repo.github_target.as_ref() else {
                continue;
            };
            if repo.github_target_dirty
                || repo.github_target_task.is_some()
                || repo.pr_task.is_some()
            {
                continue;
            }
            if (repo.prs.is_none() || dirty_targets.contains(target))
                && in_flight.insert(target.clone())
            {
                repo.pr_task = Some(Task::spawn(ctx, move || {
                    GhBackend::new(path).list(PrFilter::Open)
                }));
                jobs += 1;
            }
        }
        // Resolve every checkout, including hidden aliases: its default remote may change.
        for path in self.ordered_roots() {
            if jobs >= 4 {
                break;
            }
            let repo = self.repos.get_mut(&path).unwrap();
            if repo.github_target_task.is_some() || repo.pr_task.is_some() {
                continue;
            }
            if repo.github_target_dirty || (repo.github_target.is_none() && repo.prs.is_none()) {
                repo.github_refresh_at = Some(Instant::now());
                repo.github_target_task = Some(Task::spawn(ctx, move || {
                    GhBackend::new(path).repo_info(None).and_then(|info| {
                        if info.url.is_empty() {
                            Err(GitError::Parse("GitHub repository URL missing".into()))
                        } else {
                            Ok(info.url.trim_end_matches('/').to_lowercase())
                        }
                    })
                }));
                jobs += 1;
            }
        }
    }
    fn github_error_summary(&self) -> (usize, bool) {
        self.github_roots()
            .iter()
            .filter_map(|p| self.repos.get(p))
            .fold((0, false), |(auth, missing), repo| {
                match repo.github_error() {
                    Some(GitError::GhAuth(_)) => (auth + 1, missing),
                    Some(GitError::GhMissing) => (auth, true),
                    _ => (auth, missing),
                }
            })
    }
    fn pending_status_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<_> = self
            .repos
            .iter()
            .filter(|(path, repo)| {
                self.inventory
                    .as_ref()
                    .is_some_and(|i| i.roots.contains(path))
                    && repo.status_task.is_none()
                    && (repo.status.is_none() || repo.status_dirty)
            })
            .map(|(path, repo)| (repo.status.is_some(), repo.status_at, path.clone()))
            .collect();
        // New repositories must not wait behind repeated refreshes of earlier names.
        // Once all are loaded, revisit the oldest result first, including failures.
        roots.sort();
        roots.into_iter().map(|(_, _, path)| path).collect()
    }
    fn ordered_roots(&self) -> Vec<PathBuf> {
        let mut roots = self
            .inventory
            .as_ref()
            .map(|i| i.roots.clone())
            .unwrap_or_default();
        let group = |path: &PathBuf| {
            self.repos
                .get(path)
                .and_then(|r| r.worktree.as_ref())
                .filter(|identity| identity.linked && roots.contains(&identity.main_root))
                .map(|identity| identity.main_root.clone())
                .unwrap_or_else(|| path.clone())
        };
        let groups: BTreeMap<_, _> = roots
            .iter()
            .map(|path| (path.clone(), group(path)))
            .collect();
        let mut dirty_groups = std::collections::BTreeSet::new();
        for path in &roots {
            if self
                .repos
                .get(path)
                .and_then(|r| r.status.as_ref())
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| s.changed_count() + s.conflicted_count() > 0)
            {
                dirty_groups.insert(groups[path].clone());
            }
        }
        roots.sort_by_key(|path| {
            (
                !dirty_groups.contains(&groups[path]),
                groups[path].clone(),
                path != &groups[path],
                path.clone(),
            )
        });
        roots
    }
    pub fn drafts(&self) -> BTreeMap<PathBuf, LocalRepositoryDrafts> {
        self.repos
            .iter()
            .filter_map(|(p, r)| {
                let d = r.drafts();
                (d != LocalRepositoryDrafts::default()).then(|| (p.clone(), d))
            })
            .collect()
    }
    pub fn restore(&mut self, drafts: &BTreeMap<PathBuf, LocalRepositoryDrafts>) {
        for (p, d) in drafts {
            self.repos
                .entry(p.clone())
                .or_insert_with(|| Repository::new(p.clone()))
                .draft = d.clone();
        }
    }
    pub fn unsaved(&self) -> Vec<String> {
        let mut items: Vec<_> = self
            .drafts()
            .into_iter()
            .filter(|(_, d)| !d.commit_message.is_empty() || !d.github.repositories.is_empty())
            .map(|(p, _)| kiln_common::trf!("{} — Git / GitHub 초안", self.label(&p)))
            .collect();
        if !self.prompt.is_empty() {
            items.push(kiln_common::i18n::tr("작성 중인 에이전트 요청").into());
        }
        items
    }
    fn label(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .ok()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
    }
    fn context(&self) -> String {
        let mut s = format!(
            "# Kiln 작업 공간\n\n루트: {}\n\n이 상위 폴더를 하나의 작업 공간으로 유지하세요. 요청을 구현하는 데 필요한 여러 저장소를 함께 조사하고 변경하세요. 각 저장소의 지침과 API/의존 관계를 직접 확인하고, 이름만으로 프론트/백 역할이나 연결 관계를 단정하지 마세요. Git 명령은 해당 저장소에서 실행하세요.\n\n## 저장소 구성\n",
            self.root.display()
        );
        for p in self
            .repos
            .keys()
            .filter(|p| self.inventory.as_ref().is_some_and(|i| i.roots.contains(p)))
        {
            s.push_str(&format!("\n- {} ({})\n", self.label(p), p.display()));
            for name in [
                "AGENTS.md",
                "CLAUDE.md",
                "README.md",
                "package.json",
                "go.mod",
                "Cargo.toml",
                "pyproject.toml",
                "docker-compose.yml",
                "compose.yaml",
            ] {
                if p.join(name).is_file() {
                    s.push_str(&format!("  - 확인할 파일: {name}\n"));
                }
            }
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if self.root.join(name).is_file() {
                s.push_str(&format!(
                    "\n상위 공통 지침: {}\n",
                    self.root.join(name).display()
                ));
            }
        }
        if self
            .inventory
            .as_ref()
            .is_some_and(|i| i.limited || i.unreadable > 0)
        {
            s.push_str(
                "\n탐색 범위/권한 제한이 있으므로 필요한 저장소와 파일을 추가 확인하세요.\n",
            );
        }
        s
    }
    fn context_files(&self, path: &Path) -> Vec<&str> {
        let heading = format!("- {} ({})", self.label(path), path.display());
        self.context_cache
            .lines()
            .skip_while(|line| *line != heading)
            .skip(1)
            .take_while(|line| line.starts_with("  - 확인할 파일: "))
            .filter_map(|line| line.strip_prefix("  - 확인할 파일: "))
            .collect()
    }
    fn review_roots(&mut self, kind: ToolKind) -> Vec<PathBuf> {
        let roots = if kind == ToolKind::PullRequests {
            self.github_roots()
        } else {
            self.ordered_roots()
        };
        if !self.filter.trim().is_empty()
            || if kind == ToolKind::PullRequests {
                self.show_all_github
            } else {
                self.show_all_git
            }
        {
            return roots;
        }
        let relevant: Vec<_> = roots
            .into_iter()
            .filter(|path| {
                let repo = &self.repos[path];
                if kind == ToolKind::PullRequests {
                    repo.github_error().is_some()
                        || repo
                            .prs
                            .as_ref()
                            .is_some_and(|r| r.as_ref().is_ok_and(|prs| !prs.is_empty()))
                } else {
                    repo.status.as_ref().is_some_and(|r| {
                        r.as_ref().map_or(true, |status| !status.entries.is_empty())
                    })
                }
            })
            .collect();
        let order = if kind == ToolKind::PullRequests {
            &mut self.github_review_order
        } else {
            &mut self.git_review_order
        };
        order.retain(|path| relevant.contains(path));
        for path in relevant {
            if !order.contains(&path) {
                order.push(path);
            }
        }
        order.clone()
    }
    pub fn ui(&mut self, ui: &mut Ui, kind: ToolKind) -> Vec<Action> {
        self.tick();
        let mut actions = vec![];
        let theme = Theme::current();
        if self.context_open {
            let ready = self.inventory.is_some();
            let context = self.context_cache.clone();
            if !ready {
                ui.label(kiln_common::i18n::tr("저장소 구성을 확인하는 중…"));
            }
            egui::ScrollArea::vertical()
                .id_salt("agent_request_input")
                .max_height((ui.available_height() * 0.38).clamp(70.0, 180.0))
                .show(ui, |ui| {
                    let request = ui.add(
                        egui::TextEdit::multiline(&mut self.prompt)
                            .desired_rows(5)
                            .hint_text(kiln_common::i18n::tr("무엇을 만들거나 바꿀까요?"))
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus_prompt) {
                        request.request_focus();
                    }
                });
            ui.horizontal(|ui| {
                for (program, label, icon) in [
                    ("codex", "Codex", Icon::Codex),
                    ("claude", "Claude", Icon::Claude),
                ] {
                    if agent_choice(ui, self.agent_program == program, label, icon).clicked() {
                        self.agent_program = program.into();
                    }
                }
            });
            let can_start = ready
                && !self.prompt.trim().is_empty()
                && self.prompt.chars().count() <= super::agent_launch::MAX_REQUEST_CHARS;
            if ui
                .add_enabled_ui(can_start, |ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 32.0),
                        egui::Button::new(RichText::new(kiln_common::i18n::tr("작업 시작")).color(if can_start {
                            theme.accent_fg
                        } else {
                            theme.text
                        }))
                        .fill(if can_start {
                            theme.accent
                        } else {
                            theme.bg_hover
                        }),
                    )
                })
                .inner
                .clicked()
            {
                actions.push(Action::LaunchAgent {
                    cwd: self.root.clone(),
                    program: self.agent_program.clone(),
                    context: context.clone(),
                    request: self.prompt.clone(),
                });
            }
            if self.prompt.chars().count() > super::agent_launch::MAX_REQUEST_CHARS {
                ui.label(kiln_common::i18n::tr("요청은 32,000자 이내로 입력해 주세요."));
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("agent_context_body")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::CollapsingHeader::new(kiln_common::i18n::tr("함께 전달할 저장소 구성")).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let path = self.root.display().to_string();
                            let width = (ui.available_width() - 32.0).max(20.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 24.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(width);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&path).small().color(theme.text_dim),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(path);
                                },
                            );
                            if ui
                                .add_enabled_ui(ready, |ui| {
                                    icon_button(ui, Icon::Copy, 24.0, false, kiln_common::i18n::tr("저장소 구성 복사"))
                                })
                                .inner
                                .clicked()
                            {
                                ui.ctx().copy_text(context.clone());
                            }
                        });
                        let paths = self
                            .inventory
                            .as_ref()
                            .map(|inventory| inventory.roots.clone())
                            .unwrap_or_default();
                        for path in paths {
                            ui.add_space(7.0);
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            ui.add(
                                egui::Label::new(RichText::new(name.as_ref()).strong()).truncate(),
                            )
                            .on_hover_text(path.display().to_string());
                            let relative = self.label(&path);
                            if relative != name {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(relative).small().color(theme.text_dim),
                                    )
                                    .truncate(),
                                );
                            }
                            let files = self.context_files(&path);
                            if !files.is_empty() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(files.join(" · "))
                                            .small()
                                            .color(theme.text_dim),
                                    )
                                    .wrap(),
                                );
                            }
                        }
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(kiln_common::i18n::tr("전달 원문")).show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&context).small().color(theme.text_dim),
                                )
                                .wrap(),
                            );
                        });
                    });
                });
            return actions;
        }
        ui.horizontal_wrapped(|ui| {
            let busy = self
                .detail
                .as_ref()
                .and_then(|p| self.repos.get_mut(p))
                .is_some_and(Repository::busy);
            if self.detail.is_some()
                && ui
                    .add_enabled_ui(!busy, |ui| {
                        icon_button(ui, Icon::Undo, 28.0, false, kiln_common::i18n::tr("전체 저장소로 돌아가기"))
                    })
                    .inner
                    .on_disabled_hover_text(kiln_common::i18n::tr("Git 작업이 끝나면 돌아갈 수 있습니다."))
                    .clicked()
            {
                self.detail = None;
            }
            let count = self.inventory.as_ref().map_or(0, |i| i.roots.len());
            ui.label(
                RichText::new(kiln_common::trf!("저장소 {count}"))
                    .strong()
                    .color(theme.text),
            )
            .on_hover_text(self.root.display().to_string());
            if let Some(inventory) = self.inventory.as_ref().filter(|i| i.limited || i.unreadable > 0) {
                let mut detail = String::from(kiln_common::i18n::tr("하위 5단계, 최대 128개 저장소·20,000개 폴더까지 탐색합니다. 숨김·의존성·빌드 폴더와 심볼릭 링크는 제외합니다."));
                if inventory.limited { detail.push_str(kiln_common::i18n::tr("\n탐색 한도 때문에 확인하지 않은 경로가 있습니다.")); }
                if inventory.unreadable > 0 { detail.push_str(&kiln_common::trf!("\n읽지 못한 폴더: {}개.", inventory.unreadable)); }
                let info = icon_button(ui, if inventory.unreadable > 0 { Icon::Warning } else { Icon::Info }, 24.0, false, kiln_common::i18n::tr("저장소 탐색 정보"))
                    .on_hover_text(&detail);
                egui::Popup::menu(&info).show(|ui| {
                    ui.set_max_width(280.0);
                    ui.add(egui::Label::new(&detail).wrap());
                });
            }
            let idle = self.scan.is_none()
                && self
                    .repos
                    .values()
                    .all(|r| r.status_task.is_none() && r.pr_task.is_none());
            if ui
                .add_enabled_ui(idle, |ui| {
                    icon_button(ui, Icon::Refresh, 28.0, false, kiln_common::i18n::tr("저장소 새로고침"))
                })
                .inner
                .clicked()
            {
                let root = self.root.clone();
                self.scan = Some(Task::spawn(ui.ctx(), move || {
                    kiln_git::discovery::discover(&root)
                }));
                for r in self.repos.values_mut() {
                    r.status_dirty = true;
                    r.github_target_dirty = true;
                    r.pr_dirty = true;
                }
            }
        });
        if let Some(path) = self.detail.clone() {
            ui.label(
                RichText::new(self.label(&path))
                    .strong()
                    .color(kiln_common::Theme::current().text),
            );
            if let Some(repo) = self.repos.get_mut(&path) {
                let events = if kind == ToolKind::Git {
                    let git = repo.git.get_or_insert_with(|| {
                        let mut git = GitPanel::new(path.clone());
                        *git.commit_message_mut() = repo.draft.commit_message.clone();
                        git
                    });
                    git.ui(ui)
                } else {
                    let hub = repo.hub.get_or_insert_with(|| {
                        let mut h = GithubHub::new(path.clone());
                        h.restore_drafts(&repo.draft.github);
                        h
                    });
                    // Continue pumping a pending submission, while preventing new actions
                    // against the old target until the deferred hub replacement is safe.
                    ui.add_enabled_ui(!repo.github_hub_stale, |ui| hub.ui(ui))
                        .inner
                };
                let remote = repo.hub.as_ref().and_then(GithubHub::repo);
                actions.extend(
                    events
                        .into_iter()
                        .filter_map(|e| git_event_action(&path, remote.as_ref(), e)),
                );
            }
            return actions;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(if kind == ToolKind::PullRequests {
                    kiln_common::i18n::tr("저장소·PR 찾기")
                } else {
                    kiln_common::i18n::tr("저장소·변경 파일 찾기")
                })
                .desired_width(f32::INFINITY),
        );
        let pending = self.inventory.as_ref().map_or(0, |inventory| {
            inventory
                .roots
                .iter()
                .filter(|path| {
                    self.repos.get(*path).is_some_and(|repo| {
                        if kind == ToolKind::PullRequests {
                            repo.prs.is_none() && repo.github_error().is_none()
                        } else {
                            repo.status.is_none()
                        }
                    })
                })
                .count()
        });
        ui.horizontal(|ui| {
            let show_all = if kind == ToolKind::PullRequests {
                &mut self.show_all_github
            } else {
                &mut self.show_all_git
            };
            ui.selectable_value(
                show_all,
                false,
                if kind == ToolKind::PullRequests {
                    kiln_common::i18n::tr("PR 있음")
                } else {
                    kiln_common::i18n::tr("변경만")
                },
            );
            ui.selectable_value(show_all, true, kiln_common::i18n::tr("전체 저장소"));
            if pending > 0 {
                let label = kiln_common::trf!("{pending}개 저장소 확인 중");
                let response = ui.spinner().on_hover_text(&label);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, &label)
                });
            }
        });
        let missing: Vec<_> = self
            .drafts()
            .into_iter()
            .filter(|(p, _)| !self.inventory.as_ref().is_some_and(|i| i.roots.contains(p)))
            .collect();
        if !missing.is_empty() {
            egui::CollapsingHeader::new(kiln_common::i18n::tr("연결이 끊긴 저장소의 초안"))
                .default_open(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("missing_drafts")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for (path, draft) in missing {
                                ui.label(path.display().to_string());
                                ui.horizontal_wrapped(|ui| {
                                    if ui.button(kiln_common::i18n::tr("초안 복사")).clicked() {
                                        let mut text = kiln_common::trf!(
                                            "저장소: {}\n\n커밋 메시지:\n{}\n",
                                            path.display(),
                                            draft.commit_message
                                        );
                                        for (name, d) in draft.github.repositories {
                                            if let Some(pr) = d.pull_request {
                                                text.push_str(&format!(
                                                    "\n{name} PR: {}\n{}\n",
                                                    pr.request.title, pr.request.body
                                                ));
                                            }
                                            if let Some(issue) = d.issue {
                                                text.push_str(&kiln_common::trf!(
                                                    "\n{name} 이슈: {}\n{}\n",
                                                    issue.title, issue.body
                                                ));
                                            }
                                        }
                                        ui.ctx().copy_text(text);
                                    }
                                    let busy =
                                        self.repos.get_mut(&path).is_some_and(Repository::busy);
                                    if ui
                                        .add_enabled(!busy, egui::Button::new(kiln_common::i18n::tr("초안 폐기…")))
                                        .clicked()
                                    {
                                        self.discard = Some(path.clone());
                                    }
                                });
                            }
                        });
                });
        }
        if let Some(path) = self.discard.clone() {
            egui::Modal::new(egui::Id::new("discard_missing_repo")).show(ui.ctx(), |ui| {
                ui.heading(kiln_common::i18n::tr("보관된 초안을 폐기할까요?"));
                ui.label(path.display().to_string());
                ui.label(kiln_common::i18n::tr("이 저장소의 커밋 메시지와 GitHub 작성 초안을 제거합니다."));
                ui.horizontal(|ui| {
                    if ui.button(kiln_common::i18n::tr("취소")).clicked() {
                        self.discard = None;
                    }
                    if ui.button(kiln_common::i18n::tr("초안 폐기")).clicked() {
                        self.repos.remove(&path);
                        self.discard = None;
                    }
                });
            });
        }
        if kind == ToolKind::PullRequests {
            self.start_github_jobs(ui.ctx());
            let (auth, missing) = self.github_error_summary();
            if missing {
                ui.label(kiln_common::i18n::tr("GitHub CLI를 찾을 수 없습니다"));
                if ui.small_button(kiln_common::i18n::tr("설치·경로 확인")).clicked() {
                    actions.push(Action::OpenLink(super::terminal::LinkTarget::Url(
                        "https://cli.github.com".into(),
                    )));
                }
            }
            if auth > 0 {
                ui.label(kiln_common::trf!("{auth}개 저장소에 GitHub 로그인이 필요합니다"));
                if ui
                    .small_button(kiln_common::i18n::tr("터미널에서 로그인"))
                    .on_hover_text(kiln_common::i18n::tr("gh auth login · 대상 호스트는 로그인 과정에서 선택합니다"))
                    .clicked()
                {
                    actions.push(Action::RunInTerminalAt {
                        cwd: self.root.clone(),
                        command: "gh auth login".into(),
                    });
                }
            }
        }
        let query = self.filter.to_lowercase();
        let root = self.root.clone();
        let mut detail = None;
        let mut visible = 0;
        let review_pr = kind == ToolKind::PullRequests && !self.show_all_github && query.is_empty();
        let current = self.review_roots(kind);
        egui::ScrollArea::vertical()
            .id_salt("workspace_repositories")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for path in &current {
                    let aliases = if kind == ToolKind::PullRequests {
                        self.github_aliases(path)
                    } else {
                        vec![path.clone()]
                    };
                    let alias_matches = aliases.iter().any(|p| {
                        p.strip_prefix(&root)
                            .unwrap_or(p)
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query)
                    });
                    let repository_tooltip = aliases
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let Some(repo) = self.repos.get_mut(path) else {
                        continue;
                    };
                    let name = path
                        .strip_prefix(&root)
                        .ok()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or_else(|| path.file_name().map(Path::new).unwrap_or(path))
                        .display()
                        .to_string();
                    let pr_matches = kind == ToolKind::PullRequests
                        && repo
                            .prs
                            .as_ref()
                            .and_then(|r| r.as_ref().ok())
                            .is_some_and(|prs| {
                                prs.iter().any(|pr| {
                                    pr.title.to_lowercase().contains(&query)
                                        || format!("#{}", pr.number).contains(&query)
                                })
                            });
                    if !query.is_empty()
                        && !alias_matches
                        && !pr_matches
                        && !repo
                            .status
                            .as_ref()
                            .and_then(|s| s.as_ref().ok())
                            .is_some_and(|s| {
                                s.entries
                                    .iter()
                                    .any(|e| e.path.to_lowercase().contains(&query))
                            })
                    {
                        continue;
                    }
                    visible += 1;
                    ui.push_id(path, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.add_space(4.0);
                        let linked = repo
                            .worktree
                            .as_ref()
                            .is_some_and(|identity| identity.linked);
                        let has_changes = repo
                            .status
                            .as_ref()
                            .and_then(|s| s.as_ref().ok())
                            .is_some_and(|s| !s.entries.is_empty());
                        ui.horizontal(|ui| {
                            if linked {
                                ui.add_space(12.0);
                            }
                            let has_prs = repo
                                .prs
                                .as_ref()
                                .and_then(|result| result.as_ref().ok())
                                .is_some_and(|prs| !prs.is_empty());
                            if kind == ToolKind::Git && has_changes
                                || kind == ToolKind::PullRequests && has_prs && !review_pr
                            {
                                let content = if kind == ToolKind::Git {
                                    kiln_common::i18n::tr("변경")
                                } else {
                                    "PR"
                                };
                                let tip = if repo.expanded {
                                    kiln_common::trf!("{name} {content} 접기")
                                } else {
                                    kiln_common::trf!("{name} {content} 펼치기")
                                };
                                if icon_button(
                                    ui,
                                    if repo.expanded {
                                        Icon::ChevronDown
                                    } else {
                                        Icon::ChevronRight
                                    },
                                    24.0,
                                    false,
                                    &tip,
                                )
                                .clicked()
                                {
                                    repo.expanded = !repo.expanded;
                                }
                            } else {
                                ui.allocate_exact_size(
                                    egui::vec2(24.0, 24.0),
                                    egui::Sense::hover(),
                                );
                            }
                            let github_error = (kind == ToolKind::PullRequests)
                                .then(|| repo.github_error())
                                .flatten();
                            let action_count = if kind == ToolKind::Git || github_error.is_some() {
                                2.0
                            } else {
                                1.0
                            };
                            let actions_width = action_count * (26.0 + ui.spacing().item_spacing.x);
                            let pr_count = repo
                                .prs
                                .as_ref()
                                .and_then(|result| result.as_ref().ok())
                                .map_or(0, Vec::len);
                            let show_pr_count = kind == ToolKind::PullRequests && pr_count > 0;
                            let count_width = if show_pr_count {
                                40.0 + ui.spacing().item_spacing.x
                            } else {
                                0.0
                            };
                            let width =
                                (ui.available_width() - actions_width - count_width).max(40.0);
                            let repository_button = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(width, 26.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width(width);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&name).strong().color(theme.text),
                                            )
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                        )
                                    },
                                )
                                .inner;
                            repository_button.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name)
                            });
                            kiln_common::widgets::focus_ring(ui, &repository_button, 4);
                            if repository_button
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(repository_tooltip)
                                .clicked()
                            {
                                detail = Some(path.clone());
                            }
                            if show_pr_count {
                                ui.add_sized(
                                    egui::vec2(40.0, 26.0),
                                    egui::Label::new(
                                        RichText::new(format!("PR {pr_count}"))
                                            .small()
                                            .color(theme.text_dim),
                                    )
                                    .truncate(),
                                );
                            }
                            if let Some(error) = github_error {
                                let label = match error {
                                    GitError::GhAuth(_) => kiln_common::i18n::tr("로그인 필요"),
                                    GitError::GhMissing => kiln_common::i18n::tr("gh 경로 확인 필요"),
                                    _ => kiln_common::i18n::tr("GitHub 확인 실패"),
                                };
                                if icon_button(ui, Icon::Warning, 26.0, false, label)
                                    .on_hover_text(error.to_string())
                                    .clicked()
                                {
                                    detail = Some(path.clone());
                                }
                            }
                            if icon_button(
                                ui,
                                if kind == ToolKind::Git {
                                    Icon::Branch
                                } else {
                                    Icon::GitHub
                                },
                                26.0,
                                false,
                                if kind == ToolKind::Git {
                                    kiln_common::i18n::tr("Git 작업 열기")
                                } else {
                                    kiln_common::i18n::tr("GitHub 열기")
                                },
                            )
                            .clicked()
                            {
                                detail = Some(path.clone());
                            }
                            if kind == ToolKind::Git
                                && icon_button(ui, Icon::History, 26.0, false, kiln_common::i18n::tr("커밋 기록 열기"))
                                    .clicked()
                            {
                                actions.push(Action::OpenTab(history_factory(path.clone())));
                            }
                        });
                        if let Some(Ok(status)) = &repo.status
                            && kind == ToolKind::Git
                        {
                            ui.scope(|ui| {
                                ui.spacing_mut().interact_size.y = 18.0;
                                ui.horizontal_wrapped(|ui| {
                                    if linked {
                                        ui.add_space(12.0);
                                    }
                                    ui.add_space(24.0 + ui.spacing().item_spacing.x);
                                    let branch = status.branch.display_name();
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width().min(150.0), 18.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&branch)
                                                        .small()
                                                        .color(theme.text_dim),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(branch)
                                        },
                                    );
                                    if linked {
                                        ui.label(
                                            RichText::new(kiln_common::i18n::tr("워크트리")).small().color(theme.text_dim),
                                        )
                                        .on_hover_text(
                                            repo.worktree
                                                .as_ref()
                                                .unwrap()
                                                .main_root
                                                .display()
                                                .to_string(),
                                        );
                                    }
                                    let changes = status.changed_count();
                                    if changes > 0 {
                                        ui.label(
                                            RichText::new(kiln_common::trf!("{changes} 변경"))
                                                .small()
                                                .color(theme.text),
                                        );
                                    }
                                    let conflicts = status.conflicted_count();
                                    if conflicts > 0 {
                                        ui.label(
                                            RichText::new(kiln_common::trf!("{conflicts} 충돌"))
                                                .small()
                                                .color(theme.red),
                                        );
                                    }
                                });
                            });
                        }
                        match &repo.status {
                            Some(Ok(status)) => {
                                if kind == ToolKind::Git && (repo.expanded || !query.is_empty()) {
                                    for entry in status
                                        .entries
                                        .iter()
                                        .filter(|e| {
                                            query.is_empty()
                                                || name.to_lowercase().contains(&query)
                                                || e.path.to_lowercase().contains(&query)
                                        })
                                        .take(8)
                                    {
                                        ui.horizontal_wrapped(|ui| {
                                            if entry.is_staged() {
                                                ui.label(
                                                    RichText::new("S").small().color(theme.green),
                                                )
                                                .on_hover_text(kiln_common::i18n::tr("스테이징됨"));
                                            }
                                            if ui
                                                .add(
                                                    egui::Button::new(&entry.path)
                                                        .wrap()
                                                        .frame(false),
                                                )
                                                .clicked()
                                            {
                                                actions.push(Action::OpenTab(diff_factory(
                                                    path.clone(),
                                                    path.join(&entry.path),
                                                    entry.is_staged() && !entry.is_unstaged(),
                                                )));
                                            }
                                        });
                                    }
                                    let matches = status
                                        .entries
                                        .iter()
                                        .filter(|e| {
                                            query.is_empty()
                                                || name.to_lowercase().contains(&query)
                                                || e.path.to_lowercase().contains(&query)
                                        })
                                        .count();
                                    if matches > 8 {
                                        if ui
                                            .small_button(kiln_common::trf!("{}개 더 보기", matches - 8))
                                            .clicked()
                                        {
                                            detail = Some(path.clone());
                                        }
                                    }
                                }
                            }
                            Some(Err(error)) => {
                                ui.add(
                                    egui::Label::new(RichText::new(error).color(theme.red)).wrap(),
                                );
                            }
                            None => {
                                ui.spinner();
                            }
                        }
                        if kind == ToolKind::PullRequests {
                            match &repo.prs {
                                Some(Ok(prs)) => {
                                    for pr in prs
                                        .iter()
                                        .filter(|pr| {
                                            (review_pr || repo.expanded || !query.is_empty())
                                                && (query.is_empty()
                                                    || alias_matches
                                                    || pr.title.to_lowercase().contains(&query)
                                                    || format!("#{}", pr.number).contains(&query))
                                        })
                                        .take(if review_pr { 3 } else { usize::MAX })
                                    {
                                        let checks = match pr.checks() {
                                            Some(kiln_git::ChecksState::Pass) => kiln_common::i18n::tr("검사 통과"),
                                            Some(kiln_git::ChecksState::Fail) => kiln_common::i18n::tr("검사 실패"),
                                            Some(kiln_git::ChecksState::Pending) => kiln_common::i18n::tr("검사 진행 중"),
                                            None => "",
                                        };
                                        let title = if checks.is_empty() {
                                            format!("#{} {}", pr.number, pr.title)
                                        } else {
                                            format!("#{} {} · {checks}", pr.number, pr.title)
                                        };
                                        ui.horizontal(|ui| {
                                            ui.add_space(24.0 + ui.spacing().item_spacing.x);
                                            let response = ui.add(
                                                egui::Label::new(&title)
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                            );
                                            response.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::Button,
                                                    true,
                                                    &title,
                                                )
                                            });
                                            kiln_common::widgets::focus_ring(ui, &response, 4);
                                            if response
                                                .on_hover_text(&title)
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                actions.push(Action::OpenTab(pr_factory(
                                                    path.clone(),
                                                    None,
                                                    pr.number,
                                                )));
                                            }
                                        });
                                    }
                                    if review_pr
                                        && prs.len() > 3
                                        && ui
                                            .small_button(kiln_common::trf!("PR {}개 더 보기", prs.len() - 3))
                                            .clicked()
                                    {
                                        detail = Some(path.clone());
                                    }
                                }
                                Some(Err(_)) => {}
                                None => {
                                    ui.label(kiln_common::i18n::tr("PR 확인 중…"));
                                }
                            }
                        }
                        ui.separator();
                    });
                }
                if visible == 0 {
                    if self.inventory.is_none() {
                        ui.spinner();
                    } else if query.is_empty() && pending > 0 {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(kiln_common::i18n::tr("확인 중…"));
                        });
                    } else if query.is_empty() {
                        let empty = self
                            .inventory
                            .as_ref()
                            .is_none_or(|inventory| inventory.roots.is_empty());
                        ui.label(if empty {
                            kiln_common::i18n::tr("Git 저장소가 없습니다.")
                        } else if kind == ToolKind::PullRequests {
                            kiln_common::i18n::tr("열린 PR이 없습니다.")
                        } else {
                            kiln_common::i18n::tr("변경 사항이 없습니다.")
                        });
                    } else {
                        ui.label(if kind == ToolKind::PullRequests {
                            kiln_common::i18n::tr("일치하는 저장소·PR이 없습니다.")
                        } else {
                            kiln_common::i18n::tr("일치하는 저장소·변경 파일이 없습니다.")
                        });
                    }
                }
            });
        if let Some(path) = detail {
            self.detail = Some(path);
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    fn fixture(n: usize) -> RepositoryWorkspace {
        let ctx = egui::Context::default();
        let root = PathBuf::from("/workspace/product");
        let mut w = RepositoryWorkspace::new(&root, &ctx);
        w.scan = None;
        let paths: Vec<_> = (0..n)
            .map(|i| {
                root.join(if i == 0 {
                    "frontend".into()
                } else if i == 1 {
                    "backend".into()
                } else {
                    format!("service-{i}")
                })
            })
            .collect();
        w.inventory = Some(kiln_git::discovery::Inventory {
            roots: paths.clone(),
            ..Default::default()
        });
        for p in paths {
            let mut r = Repository::new(p.clone());
            let mut status = kiln_git::status::Status::default();
            status.branch.head = Some("feature/login".into());
            status.entries = (0..12)
                .map(|i| kiln_git::status::StatusEntry {
                    path: format!("src/feature-{i}.rs"),
                    orig_path: None,
                    index: '.',
                    worktree: 'M',
                    kind: kiln_git::status::EntryKind::Ordinary,
                    submodule: false,
                })
                .collect();
            r.status = Some(Ok(status));
            r.prs = Some(Ok(vec![]));
            w.repos.insert(p, r);
        }
        w.context_cache = w.context();
        w
    }
    #[test]
    fn periodic_refresh_retains_all_snapshots_while_only_four_repositories_reload() {
        let mut workspace = fixture(12);
        let before = workspace.summary().unwrap();
        let decorations_before = workspace.decorations().len();
        workspace.last_refresh = Instant::now() - Duration::from_secs(16);

        workspace.tick();

        // Tasks can finish on worker threads, but the next tick consumes their results.
        // Starting the refresh itself must never erase the complete displayed snapshot.
        assert_eq!(
            workspace
                .repos
                .values()
                .filter(|r| matches!(r.status, Some(Ok(_))))
                .count(),
            12
        );
        assert_eq!(
            workspace
                .repos
                .values()
                .filter(|r| r.status_task.is_some())
                .count(),
            4
        );
        assert_eq!(
            workspace.repos.values().filter(|r| r.status_dirty).count(),
            8
        );
        let after = workspace.summary().unwrap();
        assert_eq!(after.branch, before.branch);
        assert_eq!(after.dirty, before.dirty);
        assert_eq!(after.ahead, before.ahead);
        assert_eq!(after.behind, before.behind);
        assert_eq!(workspace.decorations().len(), decorations_before);
    }

    #[test]
    fn overview_summary_and_decorations_cover_both_repositories() {
        let w = fixture(2);
        let summary = w.summary().unwrap();
        assert_eq!(summary.dirty, 24);
        assert_eq!(summary.branch, "2개 저장소");
        let decorations = w.decorations();
        assert_eq!(decorations.len(), 24);
        assert!(decorations.contains_key(&w.root.join("frontend/src/feature-0.rs")));
        assert!(decorations.contains_key(&w.root.join("backend/src/feature-0.rs")));
    }
    #[test]
    fn searching_shows_remaining_matching_count() {
        let mut w = fixture(1);
        w.filter = "feature".into();
        let repository = w.root.join("frontend");
        let mut h = Harness::builder().with_size([500., 740.]).build_ui_state(
            |ui, w: &mut RepositoryWorkspace| {
                w.ui(ui, ToolKind::Git);
            },
            w,
        );
        h.run_steps(3);
        assert_eq!(h.query_all_by_label("4개 더 보기").count(), 1);
        h.get_by_label("4개 더 보기").click();
        h.run_steps(1);
        assert_eq!(h.state().detail, Some(repository));
    }
    #[test]
    fn repository_drafts_and_request_restore_after_repository_disappears() {
        let mut w = fixture(2);
        let paths: Vec<_> = w.repos.keys().cloned().collect();
        for (i, r) in w.repos.values_mut().enumerate() {
            r.draft.commit_message = format!("message-{i}");
            r.draft.github.repositories.insert(
                format!("owner/repo-{i}"),
                kiln_git::RepositoryDrafts {
                    issue: Some(kiln_git::IssueCreate {
                        title: format!("issue-{i}"),
                        body: "unsent".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
        }
        w.restore_prompt("프론트와 API 함께 수정");
        let drafts = w.drafts();
        let serialized = serde_json::to_string(&drafts).unwrap();
        let drafts = serde_json::from_str(&serialized).unwrap();
        let mut restored = fixture(0);
        restored.restore(&drafts);
        restored.restore_prompt(w.prompt());
        assert!(restored.is_multi());
        assert_eq!(
            restored.drafts().get(&paths[0]).unwrap().commit_message,
            "message-0"
        );
        assert_eq!(
            restored.drafts().get(&paths[1]).unwrap().commit_message,
            "message-1"
        );
        assert_eq!(restored.prompt(), w.prompt());
        assert!(!restored.context().contains("/workspace/product/frontend"));
        assert_eq!(restored.unsaved().len(), 3);
    }
    #[test]
    fn agent_launch_stays_available_during_background_inventory_refresh() {
        let mut workspace = fixture(2);
        workspace.open_agent_task();
        workspace.prompt = "Add shared login across frontend and backend".into();
        let (finish, pending) = std::sync::mpsc::channel();
        workspace.scan = Some(Task::spawn(&workspace.ctx, move || {
            pending.recv().unwrap_or_default()
        }));
        let mut h = Harness::builder().with_size([340., 400.]).build_ui_state(
            |ui, state: &mut (RepositoryWorkspace, Vec<Action>)| {
                state.1.extend(state.0.ui(ui, ToolKind::Git))
            },
            (workspace, vec![]),
        );
        h.run_steps(3);
        assert!(h.query_by_label("저장소 구성을 확인하는 중…").is_none());
        h.get_by_label("작업 시작").click();
        h.run_steps(2);
        assert!(matches!(
            h.state().1.as_slice(),
            [Action::LaunchAgent { .. }]
        ));
        finish
            .send(kiln_git::discovery::Inventory::default())
            .unwrap();
    }
    #[test]
    #[ignore = "manual visual review artifact"]
    fn render_agent_composer_narrow() {
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reviews/2026-10-04-composer");
        std::fs::create_dir_all(&output).unwrap();
        for (width, theme, suffix) in [
            (320., "kiln-dark", "320-dark"),
            (480., "kiln-dark", "480-dark"),
            (320., "kiln-light", "320-light"),
        ] {
            Theme::set_current(theme);
            let mut workspace = fixture(128);
            workspace.context_cache = workspace.context_cache.replace("- frontend (/workspace/product/frontend)\n", "- frontend (/workspace/product/frontend)\n  - 확인할 파일: AGENTS.md\n  - 확인할 파일: README.md\n  - 확인할 파일: package.json\n");
            workspace.context_cache = workspace.context_cache.replace("- backend (/workspace/product/backend)\n", "- backend (/workspace/product/backend)\n  - 확인할 파일: CLAUDE.md\n  - 확인할 파일: go.mod\n");
            workspace.open_agent_task();
            workspace.prompt = "프론트엔드와 백엔드에 로그인 기능을 추가해줘. 기존 인증 흐름과 각 저장소의 지침을 먼저 확인해줘.".into();
            let mut h = Harness::builder()
                .with_size([width, 620.])
                .wgpu()
                .build_ui_state(
                    |ui, workspace: &mut RepositoryWorkspace| {
                        kiln_common::fonts::install(ui.ctx());
                        Theme::current().apply(ui.ctx());
                        ui.painter()
                            .rect_filled(ui.max_rect(), 0.0, Theme::current().bg_panel);
                        workspace.ui(ui, ToolKind::Git);
                    },
                    workspace,
                );
            h.run_steps(5);
            h.get_by_label("함께 전달할 저장소 구성").click();
            h.run_steps(4);
            h.render()
                .unwrap()
                .save(output.join(format!("composer-{suffix}.png")))
                .unwrap();
            if width == 320.0 {
                h.state_mut().prompt.clear();
                h.run_steps(3);
                h.render()
                    .unwrap()
                    .save(output.join(format!("composer-empty-{suffix}.png")))
                    .unwrap();
                h.state_mut().prompt =
                    "긴 요청을 확인하고 두 저장소를 함께 수정해줘.\n".repeat(100);
                h.ctx.set_zoom_factor(1.3);
                h.run_steps(3);
                h.set_size(egui::vec2(320.0 / 1.3, 440.0 / 1.3));
                h.run_steps(4);
                assert!(
                    h.ctx
                        .content_rect()
                        .contains_rect(h.get_by_label("작업 시작").rect())
                );
                assert!(
                    h.ctx
                        .content_rect()
                        .contains_rect(h.get_by_label("Claude").rect())
                );
                h.render()
                    .unwrap()
                    .save(output.join(format!("composer-130-{suffix}.png")))
                    .unwrap();
            }
        }
        Theme::set_current("kiln-dark");
    }
    #[test]
    fn composer_controls_stay_next_to_request_with_many_repositories() {
        let mut workspace = fixture(128);
        workspace.open_agent_task();
        workspace.prompt = "Implement shared authentication".into();
        let mut h = Harness::builder().with_size([320., 400.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::Git);
            },
            workspace,
        );
        h.run_steps(3);
        let agent_before = h.get_by_label("Codex").rect();
        let launch_before = h.get_by_label("작업 시작").rect();
        h.get_by_label("함께 전달할 저장소 구성").click();
        h.run_steps(3);
        assert_eq!(agent_before, h.get_by_label("Codex").rect());
        assert_eq!(launch_before, h.get_by_label("작업 시작").rect());
        assert!(h.get_by_label("함께 전달할 저장소 구성").rect().top() > launch_before.bottom());
        assert!(h.ctx.content_rect().contains_rect(launch_before));
        h.state_mut().prompt = "Long request line\n".repeat(200);
        h.run_steps(3);
        assert!(
            h.ctx
                .content_rect()
                .contains_rect(h.get_by_label("작업 시작").rect())
        );
    }
    #[test]
    fn composer_selects_agent_then_launches_without_showing_raw_context() {
        let mut workspace = fixture(2);
        workspace.open_agent_task();
        workspace.prompt = "Implement login".into();
        let mut h = Harness::builder().with_size([320., 600.]).build_ui_state(
            |ui, state: &mut (RepositoryWorkspace, Vec<Action>)| {
                state.1.extend(state.0.ui(ui, ToolKind::Git));
            },
            (workspace, vec![]),
        );
        h.run_steps(3);
        h.get_by_label("함께 전달할 저장소 구성").click();
        h.run_steps(3);
        h.get_by_label("frontend");
        h.get_by_label("backend");
        assert!(h.query_by_label(&h.state().0.context_cache).is_none());
        h.get_by_label("Claude").click();
        h.run_steps(2);
        assert!(h.state().1.is_empty());
        h.get_by_label("작업 시작").click();
        h.run_steps(2);
        assert!(
            matches!(h.state().1.as_slice(), [Action::LaunchAgent { program, .. }] if program == "claude")
        );
    }
    #[test]
    fn github_review_shows_pr_titles_and_searches_them_directly() {
        let mut workspace = fixture(2);
        workspace.repos.get_mut(&workspace.root.join("frontend")).unwrap().prs = Some(Ok(serde_json::from_value(serde_json::json!([{ "number": 7, "title": "Login UI" }, { "number": 8, "title": "Billing update" }])).unwrap()));
        let mut h = Harness::builder().with_size([320., 500.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::PullRequests);
            },
            workspace,
        );
        h.run_steps(3);
        h.get_by_label("#7 Login UI");
        assert!(h.query_by_label("backend").is_none());
        assert!(h.query_by_label("12 변경").is_none());
        h.state_mut().filter = "billing".into();
        h.run_steps(3);
        h.get_by_label("#8 Billing update");
        assert!(h.query_by_label("#7 Login UI").is_none());
    }
    #[test]
    fn all_clean_workspace_has_visible_empty_state_and_repo_access() {
        let mut workspace = fixture(1);
        workspace
            .repos
            .values_mut()
            .next()
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .clear();
        let mut h = Harness::builder().with_size([320., 400.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::Git);
            },
            workspace,
        );
        h.run_steps(3);
        assert!(
            h.ctx
                .content_rect()
                .contains_rect(h.get_by_label("변경 사항이 없습니다.").rect())
        );
        h.get_by_label("전체 저장소").click();
        h.run_steps(3);
        h.get_by_label("frontend");
        h.get_by_label("Git 작업 열기");
    }
    #[test]
    fn review_filter_keeps_order_and_searches_clean_repositories() {
        let mut workspace = fixture(3);
        let frontend = workspace.root.join("frontend");
        let backend = workspace.root.join("backend");
        workspace
            .repos
            .get_mut(&backend)
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .clear();
        let initial = workspace.review_roots(ToolKind::Git);
        assert!(!initial.contains(&backend));
        workspace
            .repos
            .get_mut(&backend)
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .push(kiln_git::status::StatusEntry {
                path: "new.rs".into(),
                orig_path: None,
                index: '.',
                worktree: 'M',
                kind: kiln_git::status::EntryKind::Ordinary,
                submodule: false,
            });
        let updated = workspace.review_roots(ToolKind::Git);
        assert_eq!(&updated[..initial.len()], initial.as_slice());
        assert_eq!(updated.last(), Some(&backend));
        workspace
            .repos
            .get_mut(&frontend)
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .clear();
        workspace.filter = "frontend".into();
        assert!(workspace.review_roots(ToolKind::Git).contains(&frontend));
    }
    #[test]
    fn parent_context_launch_does_not_switch_to_a_repository() {
        let mut w = fixture(2);
        w.context_open = true;
        w.prompt = "add login's API and UI".into();
        let root = w.root.clone();
        let mut h = Harness::builder().with_size([500., 700.]).build_ui_state(
            |ui, state: &mut (RepositoryWorkspace, Vec<Action>)| {
                state.1.extend(state.0.ui(ui, ToolKind::Git));
            },
            (w, vec![]),
        );
        h.run_steps(3);
        h.get_by_label("작업 시작").click();
        h.run_steps(3);
        let action = h
            .state()
            .1
            .iter()
            .find_map(|a| {
                if let Action::LaunchAgent {
                    cwd,
                    context,
                    request,
                    ..
                } = a
                {
                    Some((cwd, context, request))
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(action.0, &root);
        assert!(action.1.contains("frontend"));
        assert!(action.1.contains("backend"));
        assert!(action.2.contains("login's"));
        assert_eq!(h.state().0.root, root);
    }
    #[test]
    fn search_renders_a_matching_file_beyond_the_initial_eight() {
        let mut w = fixture(2);
        w.filter = "feature-11".into();
        let mut h = Harness::builder().with_size([420., 760.]).build_ui_state(
            |ui, w: &mut RepositoryWorkspace| {
                w.ui(ui, ToolKind::Git);
            },
            w,
        );
        h.run_steps(3);
        assert_eq!(h.query_all_by_label("src/feature-11.rs").count(), 2);
        assert_eq!(h.query_all_by_label("src/feature-0.rs").count(), 0);
    }
    #[test]
    fn overview_starts_compact_and_expands_changes_on_demand() {
        let workspace = fixture(2);
        let mut h = Harness::builder().with_size([420., 700.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::Git);
            },
            workspace,
        );
        h.run_steps(3);
        assert_eq!(h.query_all_by_label("src/feature-0.rs").count(), 0);
        h.get_by_label("frontend 변경 펼치기").click();
        h.run_steps(3);
        assert_eq!(h.query_all_by_label("src/feature-0.rs").count(), 1);
        h.get_by_label("frontend 변경 접기").click();
        h.run_steps(3);
        assert_eq!(h.query_all_by_label("src/feature-0.rs").count(), 0);
    }
    #[test]
    fn github_overview_expands_pull_requests_only_when_requested() {
        let mut workspace = fixture(1);
        let repo = workspace.repos.values_mut().next().unwrap();
        repo.prs = Some(Ok(serde_json::from_value(
            serde_json::json!([{ "number": 7, "title": "Login" }]),
        )
        .unwrap()));
        repo.pr_at = Some(Instant::now());
        workspace.show_all_github = true;
        let mut h = Harness::builder().with_size([420., 700.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::PullRequests);
            },
            workspace,
        );
        h.run_steps(3);
        h.get_by_label("PR 1");
        assert!(h.query_by_label("#7 Login").is_none());
        h.get_by_label("frontend PR 펼치기").click();
        h.run_steps(3);
        h.get_by_label("#7 Login");
    }
    #[test]
    fn linked_worktrees_stay_with_their_main_repository() {
        let mut workspace = fixture(3);
        let main = workspace.root.join("frontend");
        let linked = workspace.root.join("backend");
        workspace.repos.get_mut(&linked).unwrap().worktree =
            Some(kiln_git::worktrees::WorktreeIdentity {
                main_root: main.clone(),
                root: linked.clone(),
                branch: Some("review".into()),
                linked: true,
            });
        let order = workspace.ordered_roots();
        let position = order.iter().position(|path| path == &main).unwrap();
        assert_eq!(order[position + 1], linked);
    }
    #[test]
    fn dynamic_long_repository_content_cannot_widen_overview() {
        for kind in [ToolKind::Git, ToolKind::PullRequests] {
            let mut workspace = fixture(1);
            let old_path = workspace.root.join("frontend");
            let long_path = workspace
                .root
                .join("repository-with-a-very-long-name-".repeat(12));
            let mut repo = workspace.repos.remove(&old_path).unwrap();
            repo.root = long_path.clone();
            repo.status.as_mut().unwrap().as_mut().unwrap().branch.head =
                Some("feature/long-branch-name/".repeat(30));
            repo.prs = Some(Err(GitError::Failed(
                "A GitHub response with a very long detail ".repeat(40),
            )));
            repo.pr_at = Some(Instant::now());
            workspace.repos.insert(long_path.clone(), repo);
            workspace.inventory.as_mut().unwrap().roots = vec![long_path];
            let mut h = Harness::builder().with_size([340., 600.]).build_ui_state(
                move |ui, state: &mut (RepositoryWorkspace, f32)| {
                    let right = ui.max_rect().right();
                    state.0.ui(ui, kind);
                    state.1 = ui.min_rect().right() - right;
                },
                (workspace, 0.),
            );
            h.run_steps(5);
            assert!(h.state().1 <= 1., "{kind:?} grew {} points", h.state().1);
        }
    }
    #[test]
    #[ignore = "manual visual review artifact"]
    fn render_compact_repository_overview() {
        let mut workspace = fixture(128);
        workspace.inventory.as_mut().unwrap().limited = true;
        let main = workspace.root.join("frontend");
        let linked = workspace.root.join("frontend-login-worktree");
        let mut worktree = workspace
            .repos
            .remove(&workspace.root.join("service-2"))
            .unwrap();
        worktree.root = linked.clone();
        worktree.worktree = Some(kiln_git::worktrees::WorktreeIdentity {
            main_root: main.clone(),
            root: linked.clone(),
            branch: Some("feature/login".into()),
            linked: true,
        });
        workspace.repos.insert(linked.clone(), worktree);
        workspace
            .inventory
            .as_mut()
            .unwrap()
            .roots
            .retain(|p| p != &workspace.root.join("service-2"));
        workspace.inventory.as_mut().unwrap().roots.push(linked);
        for (index, repo) in workspace.repos.values_mut().enumerate() {
            let status = repo.status.as_mut().unwrap().as_mut().unwrap();
            if index > 4 {
                status.entries.clear();
            }
            if index == 1 {
                status.branch.head =
                    Some("feature/authentication-across-frontend-and-backend".into());
            }
        }
        let mut h = Harness::builder()
            .with_size([360., 800.])
            .wgpu()
            .build_ui_state(
                |ui, workspace: &mut RepositoryWorkspace| {
                    kiln_common::fonts::install(ui.ctx());
                    Theme::current().apply(ui.ctx());
                    workspace.ui(ui, ToolKind::Git);
                },
                workspace,
            );
        h.run_steps(5);
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reviews/2026-09-30-continuity");
        std::fs::create_dir_all(&output).unwrap();
        h.render()
            .unwrap()
            .save(output.join("repositories-compact.png"))
            .unwrap();
    }
    #[test]
    #[ignore = "manual visual review artifact"]
    fn render_review_filters() {
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reviews/2026-10-01-workflow");
        std::fs::create_dir_all(&output).unwrap();
        for (width, theme, suffix) in [
            (320., "kiln-dark", "320-dark"),
            (480., "kiln-dark", "480-dark"),
            (320., "kiln-light", "320-light"),
        ] {
            Theme::set_current(theme);
            for kind in [ToolKind::Git, ToolKind::PullRequests] {
                let mut workspace = fixture(6);
                for (index, repo) in workspace.repos.values_mut().enumerate() {
                    if index > 2 {
                        repo.status
                            .as_mut()
                            .unwrap()
                            .as_mut()
                            .unwrap()
                            .entries
                            .clear();
                    }
                    if index < 2 {
                        repo.prs = Some(Ok(serde_json::from_value(serde_json::json!([{ "number": 17 + index, "title": "Connect frontend and API authentication" }])).unwrap()));
                    }
                    if index == 2 {
                        repo.prs = Some(Err(GitError::Failed("Repository access denied".into())));
                    }
                }
                let mut h = Harness::builder()
                    .with_size([width, 620.])
                    .wgpu()
                    .build_ui_state(
                        move |ui, workspace: &mut RepositoryWorkspace| {
                            kiln_common::fonts::install(ui.ctx());
                            Theme::current().apply(ui.ctx());
                            ui.painter()
                                .rect_filled(ui.max_rect(), 0.0, Theme::current().bg_panel);
                            workspace.ui(ui, kind);
                        },
                        workspace,
                    );
                h.run_steps(5);
                let name = if kind == ToolKind::Git {
                    "changes"
                } else {
                    "github"
                };
                h.render()
                    .unwrap()
                    .save(output.join(format!("{name}-{suffix}.png")))
                    .unwrap();
            }
        }
        Theme::set_current("kiln-dark");
    }
    #[test]
    #[ignore = "manual visual review artifact"]
    fn render_github_overview_auth() {
        let mut workspace = fixture(6);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        for path in &paths[..3] {
            workspace.repos.get_mut(path).unwrap().prs =
                Some(Err(GitError::GhAuth("Run gh auth login".into())));
        }
        workspace.repos.get_mut(&paths[3]).unwrap().prs = Some(Err(GitError::Failed(
            "enterprise.example permission denied".into(),
        )));
        let prs: Vec<PrItem> = serde_json::from_value(
            serde_json::json!([{ "number": 7, "title": "Add shared login" }]),
        )
        .unwrap();
        workspace.repos.get_mut(&paths[4]).unwrap().prs = Some(Ok(prs));
        let mut h = Harness::builder()
            .with_size([360., 800.])
            .wgpu()
            .build_ui_state(
                |ui, workspace: &mut RepositoryWorkspace| {
                    kiln_common::fonts::install(ui.ctx());
                    Theme::current().apply(ui.ctx());
                    workspace.ui(ui, ToolKind::PullRequests);
                },
                workspace,
            );
        h.run_steps(5);
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reviews/2026-09-30-continuity");
        std::fs::create_dir_all(&output).unwrap();
        h.render()
            .unwrap()
            .save(output.join("github-auth-overview.png"))
            .unwrap();
    }
    #[test]
    fn periodic_github_refresh_keeps_results_and_deduplication() {
        let mut workspace = fixture(2);
        let old = Instant::now() - Duration::from_secs(61);
        let prs: Vec<PrItem> =
            serde_json::from_value(serde_json::json!([{ "number": 7, "title": "Keep visible" }]))
                .unwrap();
        for repo in workspace.repos.values_mut() {
            repo.github_target = Some("https://github.com/team/project".into());
            repo.prs = Some(Ok(prs.clone()));
            repo.pr_at = Some(old);
        }
        workspace.tick();
        assert_eq!(workspace.github_roots().len(), 1);
        for repo in workspace.repos.values() {
            assert!(repo.github_target_dirty);
            assert_eq!(repo.prs.as_ref().unwrap().as_ref().unwrap().len(), 1);
            assert!(repo.github_target.is_some());
        }
    }
    #[test]
    fn changed_github_target_invalidates_only_that_checkouts_old_prs() {
        let mut workspace = fixture(2);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        for repo in workspace.repos.values_mut() {
            repo.github_target = Some("https://github.com/team/project".into());
        }
        let mut hub = GithubHub::new(paths[1].clone());
        let mut drafts = kiln_git::GithubDrafts::default();
        drafts.selected = Some("team/explicit-choice".into());
        drafts.repositories.insert(
            "team/project".into(),
            kiln_git::RepositoryDrafts {
                issue: Some(kiln_git::IssueCreate {
                    title: "Keep draft".into(),
                    body: "Unsubmitted text".into(),
                    labels: vec![],
                    assignees: vec![],
                }),
                ..Default::default()
            },
        );
        hub.restore_drafts(&drafts);
        workspace.repos.get_mut(&paths[1]).unwrap().hub = Some(hub);
        workspace
            .repos
            .get_mut(&paths[1])
            .unwrap()
            .github_target_task = Some(Task::spawn(&workspace.ctx, || {
            Ok("https://enterprise.example/team/other".into())
        }));
        for _ in 0..100 {
            workspace.tick();
            if workspace.repos[&paths[1]].github_target_task.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(workspace.github_roots().len(), 2);
        assert!(workspace.repos[&paths[1]].prs.is_none());
        assert!(
            workspace.repos[&paths[1]].hub.is_none(),
            "old detail must not reopen against a stale target"
        );
        assert_eq!(
            workspace.repos[&paths[1]].draft.github.repositories["team/project"]
                .issue
                .as_ref()
                .unwrap()
                .body,
            "Unsubmitted text"
        );
        assert_eq!(
            workspace.repos[&paths[1]].draft.github.selected.as_deref(),
            Some("team/explicit-choice")
        );
        assert!(workspace.repos[&paths[0]].prs.as_ref().unwrap().is_ok());
    }
    #[test]
    fn github_refresh_failure_retains_good_results_and_retries_later() {
        let mut workspace = fixture(1);
        let path = workspace.root.join("frontend");
        let repo = workspace.repos.get_mut(&path).unwrap();
        repo.github_target = Some("https://github.com/team/project".into());
        repo.pr_task = Some(Task::spawn(&workspace.ctx, || {
            Err(GitError::GhAuth("expired session".into()))
        }));
        for _ in 0..100 {
            workspace.tick();
            if workspace.repos[&path].pr_task.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(workspace.repos[&path].prs.as_ref().unwrap().is_ok());
        assert_eq!(workspace.github_error_summary(), (1, false));
        workspace.repos.get_mut(&path).unwrap().github_refresh_at =
            Some(Instant::now() - Duration::from_secs(61));
        workspace.tick();
        assert!(workspace.repos[&path].github_target_dirty);
    }
    #[test]
    fn github_reuses_an_in_flight_request_from_another_checkout() {
        let mut workspace = fixture(2);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        for repo in workspace.repos.values_mut() {
            repo.github_target = Some("https://github.com/team/project".into());
            repo.prs = None;
        }
        workspace.repos.get_mut(&paths[1]).unwrap().pr_task =
            Some(Task::spawn(&workspace.ctx, || Ok(vec![])));
        let mut h = Harness::builder().with_size([360., 500.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::PullRequests);
            },
            workspace,
        );
        h.run_steps(3);
        assert!(
            h.state().repos[&paths[0]].pr_task.is_none(),
            "same remote must reuse the other checkout's in-flight request"
        );
    }
    #[test]
    fn github_auth_is_one_actionable_notice_without_hiding_other_repositories() {
        let mut workspace = fixture(4);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        for path in &paths[..2] {
            workspace.repos.get_mut(path).unwrap().prs =
                Some(Err(GitError::GhAuth("not logged in".into())));
        }
        workspace.repos.get_mut(&paths[2]).unwrap().prs = Some(Err(GitError::Failed(
            "enterprise.example permission denied".into(),
        )));
        workspace.repos.get_mut(&paths[3]).unwrap().prs = Some(Ok(serde_json::from_value(
            serde_json::json!([{ "number": 9, "title": "Healthy PR" }]),
        )
        .unwrap()));
        assert_eq!(workspace.github_error_summary(), (2, false));
        let mut h = Harness::builder().with_size([360., 800.]).build_ui_state(
            |ui, state: &mut (RepositoryWorkspace, Vec<Action>)| {
                state.1.extend(state.0.ui(ui, ToolKind::PullRequests))
            },
            (workspace, vec![]),
        );
        h.run_steps(3);
        assert!(
            h.state().1.is_empty(),
            "login must never start automatically"
        );
        h.get_by_label("2개 저장소에 GitHub 로그인이 필요합니다");
        h.get_by_label("GitHub 확인 실패");
        h.get_by_label("service-3");
        h.get_by_label("터미널에서 로그인").click();
        h.run_steps(2);
        assert!(
            matches!(h.state().1.as_slice(), [Action::RunInTerminalAt { cwd, command }] if cwd == &PathBuf::from("/workspace/product") && command == "gh auth login")
        );
    }
    #[test]
    fn github_deduplicates_resolved_targets_but_not_different_hosts() {
        let mut workspace = fixture(3);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        workspace.repos.get_mut(&paths[0]).unwrap().github_target =
            Some("https://github.com/team/project".into());
        workspace.repos.get_mut(&paths[1]).unwrap().github_target =
            Some("https://github.com/team/project".into());
        workspace.repos.get_mut(&paths[2]).unwrap().github_target =
            Some("https://enterprise.example/team/project".into());
        assert_eq!(
            workspace.github_roots(),
            vec![paths[0].clone(), paths[2].clone()]
        );
        let prs: Vec<PrItem> =
            serde_json::from_value(serde_json::json!([{ "number": 7, "title": "Shared change" }]))
                .unwrap();
        let repo = workspace.repos.get_mut(&paths[1]).unwrap();
        repo.prs = Some(Ok(prs));
        repo.pr_at = Some(Instant::now());
        let mut aliases = workspace.github_aliases(&paths[0]);
        aliases.sort();
        assert_eq!(aliases, vec![paths[0].clone(), paths[1].clone()]);
        workspace.tick();
        assert_eq!(
            workspace.repos[&paths[0]]
                .prs
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            workspace.repos[&paths[2]]
                .prs
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .len(),
            0
        );
    }
    #[test]
    fn discovery_limits_use_a_header_icon_with_accessible_details() {
        let mut workspace = fixture(2);
        let inventory = workspace.inventory.as_mut().unwrap();
        inventory.limited = true;
        inventory.unreadable = 3;
        let mut h = Harness::builder().with_size([320., 600.]).build_ui_state(
            |ui, workspace: &mut RepositoryWorkspace| {
                workspace.ui(ui, ToolKind::Git);
            },
            workspace,
        );
        h.run_steps(3);
        assert!(h.query_by_label("탐색 범위 제한").is_none());
        let count = h.get_by_label("저장소 2").rect();
        let info = h.get_by_label("저장소 탐색 정보").rect();
        assert!(count.y_range().intersects(info.y_range()));
        h.get_by_label("저장소 탐색 정보").click();
        h.run_steps(3);
        h.get_by_label("하위 5단계, 최대 128개 저장소·20,000개 폴더까지 탐색합니다. 숨김·의존성·빌드 폴더와 심볼릭 링크는 제외합니다.\n탐색 한도 때문에 확인하지 않은 경로가 있습니다.\n읽지 못한 폴더: 3개.");
    }
    #[test]
    fn workspace_summary_stays_quiet_when_a_repository_is_unavailable() {
        let mut workspace = fixture(2);
        workspace.repos.values_mut().next().unwrap().status = Some(Err("not a repository".into()));
        assert_eq!(workspace.summary().unwrap().branch, "2개 저장소");
    }
    #[test]
    fn status_queue_prioritizes_unseen_then_oldest_results() {
        let mut workspace = fixture(3);
        let paths: Vec<_> = workspace.repos.keys().cloned().collect();
        let now = Instant::now();
        for (index, path) in paths.iter().enumerate() {
            let repo = workspace.repos.get_mut(path).unwrap();
            repo.status_dirty = true;
            repo.status_at = Some(now - Duration::from_secs(index as u64 * 10));
        }
        workspace.repos.get_mut(&paths[2]).unwrap().status = None;
        assert_eq!(
            workspace.pending_status_roots(),
            vec![paths[2].clone(), paths[1].clone(), paths[0].clone()]
        );
        workspace.repos.get_mut(&paths[2]).unwrap().status = Some(Err("permission denied".into()));
        assert_eq!(
            workspace.pending_status_roots()[0],
            paths[2],
            "old failures must also be retried fairly"
        );
    }
    #[test]
    fn long_parent_path_does_not_expand_the_agent_inspector() {
        let mut workspace = fixture(128);
        workspace.root = PathBuf::from(format!(
            "/workspace/{}/parent",
            "long-directory-name/".repeat(30)
        ));
        workspace.context_cache = workspace.context();
        workspace.context_open = true;
        let mut h = Harness::builder().with_size([340., 600.]).build_ui_state(
            |ui, state: &mut (RepositoryWorkspace, f32)| {
                let limit = ui.max_rect().right();
                state.0.ui(ui, ToolKind::Git);
                state.1 = ui.min_rect().right() - limit;
            },
            (workspace, 0.),
        );
        h.run_steps(3);
        h.get_by_label("함께 전달할 저장소 구성").click();
        h.run_steps(3);
        assert!(
            h.state().1 <= 1.,
            "content widened inspector by {} points",
            h.state().1
        );
    }
    #[test]
    fn refreshed_inventory_removes_deleted_detail_but_preserves_its_draft() {
        let mut w = fixture(2);
        let deleted = w.root.join("frontend");
        let retained = w.root.join("backend");
        w.detail = Some(deleted.clone());
        w.repos.get_mut(&deleted).unwrap().draft.commit_message = "keep this draft".into();
        w.accept_inventory(kiln_git::discovery::Inventory {
            roots: vec![retained],
            ..Default::default()
        });
        assert!(w.detail.is_none());
        assert_eq!(w.drafts()[&deleted].commit_message, "keep this draft");
        assert!(!w.context_cache.contains("/workspace/product/frontend"));
    }
    #[test]
    fn inventory_refresh_is_scheduled_without_losing_current_repositories() {
        let mut w = fixture(2);
        w.last_scan = Instant::now() - Duration::from_secs(61);
        w.tick();
        assert!(w.scan.is_some());
        assert_eq!(w.inventory.as_ref().unwrap().roots.len(), 2);
    }
    #[test]
    fn new_agent_task_accepts_typing_without_a_second_click() {
        let mut w = fixture(0);
        w.open_agent_task();
        let mut h = Harness::builder().with_size([500., 500.]).build_ui_state(
            |ui, w: &mut RepositoryWorkspace| {
                w.ui(ui, ToolKind::Git);
            },
            w,
        );
        h.run_steps(3);
        h.input_mut()
            .events
            .push(egui::Event::Text("add login".into()));
        h.run_steps(2);
        assert_eq!(h.state().prompt(), "add login");
    }
    #[test]
    fn global_agent_task_opens_without_git_repository_or_subproject() {
        let mut w = fixture(0);
        let root = w.root.clone();
        assert!(
            w.is_multi(),
            "an empty parent uses the overview, not single-repo Git errors"
        );
        w.open_agent_task();
        assert!(w.is_agent_task_open());
        assert!(w.is_multi());
        assert_eq!(w.root, root);
        w.close_agent_task();
        assert!(!w.is_agent_task_open());
    }
    #[test]
    fn overview_keeps_changed_repositories_first_and_actions_accessible() {
        let mut w = fixture(2);
        let backend = w.root.join("backend");
        w.repos
            .get_mut(&backend)
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .clear();
        let mut h = Harness::builder().with_size([500., 760.]).build_ui_state(
            |ui, w: &mut RepositoryWorkspace| {
                w.ui(ui, ToolKind::Git);
            },
            w,
        );
        h.run_steps(3);
        h.get_by_label("frontend");
        assert!(h.query_by_label("backend").is_none());
        h.get_by_label("전체 저장소").click();
        h.run_steps(2);
        assert!(h.get_by_label("frontend").rect().top() < h.get_by_label("backend").rect().top());
        assert!(h.query_by_label("에이전트 컨텍스트").is_none());
        assert!(h.query_by_label("변경 0개").is_none());
        assert_eq!(h.query_all_by_label("Git 작업 열기").count(), 2);
        assert_eq!(h.query_all_by_label("커밋 기록 열기").count(), 2);
        assert!(h.query_by_label("새 에이전트 작업").is_none());
        h.state_mut().open_agent_task();
        h.run_steps(3);
        assert!(h.query_by_label("작업 시작").is_some());
        assert!(h.query_by_label("함께 전달할 저장소 구성").is_some());
        assert!(h.query_by_label("새 에이전트 작업").is_none());
        assert!(h.query_by_label("저장소 새로고침").is_none());
        assert!(h.query_by_label("frontend").is_none());
        assert!(h.query_by_label("Git 작업 열기").is_none());
    }
    #[test]
    fn additional_changes_open_the_repository_instead_of_a_dead_end_label() {
        let mut w = fixture(1);
        w.repos.values_mut().next().unwrap().expanded = true;
        let root = w.root.clone();
        let mut h = Harness::builder().with_size([500., 760.]).build_ui_state(
            |ui, w: &mut RepositoryWorkspace| {
                w.ui(ui, ToolKind::Git);
            },
            w,
        );
        h.run_steps(3);
        h.get_by_label("4개 더 보기").click();
        h.run_steps(1);
        assert_eq!(h.state().detail, Some(root.join("frontend")));
    }
    #[test]
    fn narrow_workspace_context_keeps_launch_controls_visible() {
        let mut w = fixture(20);
        w.context_open = true;
        w.prompt = "프론트와 백엔드에 로그인 기능 추가".into();
        let mut h = Harness::builder()
            .with_size([554., 338.])
            .wgpu()
            .build_ui_state(
                |ui, w: &mut RepositoryWorkspace| {
                    kiln_common::fonts::install(ui.ctx());
                    kiln_common::Theme::current().apply(ui.ctx());
                    w.ui(ui, ToolKind::Git);
                },
                w,
            );
        h.run_steps(5);
        h.get_by_label("함께 전달할 저장소 구성").click();
        h.run_steps(5);
        assert!(h.get_by_label("작업 시작").rect().bottom() < 338.);
        h.render()
            .unwrap()
            .save("/tmp/kiln-workspace-multi-minimum.png")
            .unwrap();
    }
}
