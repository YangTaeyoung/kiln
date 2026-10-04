//! GitHub 허브(GithubHub): 저장소 칩 + [풀 리퀘스트 | 이슈 | Actions] 탭.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{CornerRadius, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use kiln_common::Task;

use super::actions_panel::ActionsPanel;
use super::gh_widgets::*;
use super::issue_panel::IssuePanel;
use super::pr_panel::PrPanel;
use super::repo_picker::{RepoPicker, RepoPickerAction};
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::GhBackend;
use crate::github::{GithubBackend, RepoInfo, RepoRef};

/// 허브 탭.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HubTab {
    #[default]
    PullRequests,
    Issues,
    Actions,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GithubDrafts {
    pub repositories: std::collections::BTreeMap<String, RepositoryDrafts>,
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepositoryDrafts {
    pub pull_request: Option<super::pr_panel::PrCreationDraft>,
    pub issue: Option<crate::github::IssueCreate>,
}

impl RepositoryDrafts {
    fn is_empty(&self) -> bool { self.pull_request.is_none() && self.issue.is_none() }
}

/// 선택된 저장소의 패널 묶음.
struct Panels {
    repo: RepoRef,
    pr: PrPanel,
    issues: IssuePanel,
    actions: ActionsPanel,
}

/// GitHub 허브 위젯.
pub struct GithubHub {
    root: PathBuf,
    backend: Arc<dyn GithubBackend>,
    tab: HubTab,
    /// 작업 폴더에서 찾은 저장소.
    workspace: Option<RepoInfo>,
    detect: Option<Task<GitResult<RepoInfo>>>,
    detect_error: Option<GitError>,
    started: bool,
    /// 사용자가 고른 다른 저장소. `None` 이면 작업 폴더 저장소.
    selected: Option<RepoRef>,
    selected_info: Option<RepoInfo>,
    info_task: Option<(RepoRef, Task<GitResult<RepoInfo>>)>,
    pending_info: Option<RepoRef>,
    panels: Option<Panels>,
    drafts: GithubDrafts,
    picker: RepoPicker,
    now_override: Option<i64>,
}

impl GithubHub {
    pub fn is_submitting(&self) -> bool { self.panels.as_ref().is_some_and(|p| p.pr.is_submitting() || p.issues.is_submitting()) }

    pub fn recovery_drafts(&self) -> GithubDrafts {
        let mut drafts = self.drafts.clone();
        drafts.selected = self.selected.as_ref().map(RepoRef::full_name);
        if let Some(p) = &self.panels {
            let current = RepositoryDrafts { pull_request: p.pr.creation_draft(), issue: p.issues.creation_draft() };
            if current.is_empty() { drafts.repositories.remove(&p.repo.full_name()); }
            else { drafts.repositories.insert(p.repo.full_name(), current); }
        }
        drafts
    }

    pub fn restore_drafts(&mut self, drafts: &GithubDrafts) {
        self.drafts = drafts.clone();
        if let Some(panels) = &mut self.panels && let Some(draft) = drafts.repositories.get(&panels.repo.full_name()) {
            if let Some(pr) = &draft.pull_request { panels.pr.restore_creation_draft(pr); }
            if let Some(issue) = &draft.issue { panels.issues.restore_creation_draft(issue); }
        }
        if let Some(repo) = drafts.selected.as_deref().and_then(RepoRef::parse) { self.select_repo(repo); }
    }

    pub fn new(root: PathBuf) -> Self {
        let backend: Arc<dyn GithubBackend> = Arc::new(GhBackend::new(root.clone()));
        Self::with_backend(root, backend)
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(root: PathBuf, backend: Arc<dyn GithubBackend>) -> Self {
        Self {
            root,
            picker: RepoPicker::new(backend.clone()),
            backend,
            tab: HubTab::PullRequests,
            workspace: None,
            detect: None,
            detect_error: None,
            started: false,
            selected: None,
            selected_info: None,
            info_task: None,
            pending_info: None,
            panels: None,
            drafts: GithubDrafts::default(),
            now_override: None,
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn tab(&self) -> HubTab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: HubTab) {
        self.tab = tab;
    }

    /// 저장소 정보와 모든 패널을 다시 읽는다.
    pub fn refresh(&mut self) {
        self.started = false;
        self.detect_error = None;
        if let Some(r) = self.selected.clone() {
            self.info_task = None;
            self.request_info(r);
        }
        if let Some(p) = &mut self.panels {
            p.pr.refresh();
            p.issues.refresh();
            p.actions.refresh();
        }
    }

    /// 허브가 지금 보는 저장소. 아직 모르면 `None`.
    pub fn repo(&self) -> Option<RepoRef> {
        self.selected.clone().or_else(|| self.workspace.as_ref().map(|w| w.repo.clone()))
    }

    /// 허브가 지금 보는 저장소의 정보.
    pub fn repo_info(&self) -> Option<&RepoInfo> {
        match &self.selected {
            Some(_) => self.selected_info.as_ref(),
            None => self.workspace.as_ref(),
        }
    }

    /// 작업 폴더에서 찾은 저장소.
    pub fn workspace_repo(&self) -> Option<&RepoRef> {
        self.workspace.as_ref().map(|w| &w.repo)
    }

    /// 작업 폴더와 다른 저장소를 대상으로 삼는다. 작업 폴더 저장소와 같으면 선택을 푼다.
    pub fn select_repo(&mut self, repo: RepoRef) {
        if self.is_submitting() { return; }
        if self.workspace.as_ref().is_some_and(|w| w.repo == repo) {
            self.use_workspace_repo();
            return;
        }
        if self.selected.as_ref() == Some(&repo) {
            return;
        }
        self.selected = Some(repo.clone());
        self.selected_info = None;
        self.request_info(repo);
    }

    /// 작업 폴더 저장소로 돌아간다.
    pub fn use_workspace_repo(&mut self) {
        if self.is_submitting() { return; }
        self.selected = None;
        self.selected_info = None;
        self.info_task = None;
        self.pending_info = None;
    }

    /// 오류(gh 없음/로그인 필요/저장소 아님). 정상이면 `None`.
    pub fn error(&self) -> Option<&GitError> {
        if self.selected.is_some() { None } else { self.detect_error.as_ref() }
    }

    pub fn pr_panel(&mut self) -> Option<&mut PrPanel> {
        self.panels.as_mut().map(|p| &mut p.pr)
    }

    pub fn issue_panel(&mut self) -> Option<&mut IssuePanel> {
        self.panels.as_mut().map(|p| &mut p.issues)
    }

    pub fn actions_panel(&mut self) -> Option<&mut ActionsPanel> {
        self.panels.as_mut().map(|p| &mut p.actions)
    }

    /// 저장소 선택 팝업을 연다/닫는다.
    pub fn toggle_repo_picker(&mut self, ctx: &egui::Context) {
        self.picker.toggle(ctx);
    }

    pub fn is_loading(&mut self) -> bool {
        let panels_busy = match &mut self.panels {
            Some(p) => match self.tab {
                HubTab::PullRequests => p.pr.is_loading(),
                HubTab::Issues => p.issues.is_loading(),
                HubTab::Actions => p.actions.is_loading(),
            },
            None => false,
        };
        !self.started
            || self.detect.as_mut().is_some_and(|t| t.is_pending())
            || self.info_task.as_mut().is_some_and(|(_, t)| t.is_pending())
            || self.pending_info.is_some()
            || self.picker.is_loading()
            || panels_busy
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
        self.picker.set_now(ts);
        if let Some(p) = &mut self.panels {
            p.pr.set_now(ts);
            p.issues.set_now(ts);
            p.actions.set_now(ts);
        }
    }

    /// 저장소 정보 읽기를 예약한다. 작업은 다음 `pump` 에서 시작한다.
    fn request_info(&mut self, repo: RepoRef) {
        self.info_task = None;
        self.pending_info = Some(repo);
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let b = self.backend.clone();
            self.detect = Some(Task::spawn(ctx, move || b.repo_info(None)));
        }
        if let Some(t) = &mut self.detect
            && let Some(r) = t.take()
        {
            self.detect = None;
            match r {
                Ok(info) => {
                    self.workspace = Some(info);
                    self.detect_error = None;
                }
                Err(e) => {
                    self.workspace = None;
                    self.detect_error = Some(e);
                }
            }
        }
        if let Some(repo) = self.pending_info.take() {
            let b = self.backend.clone();
            let r = repo.clone();
            self.info_task = Some((repo, Task::spawn(ctx, move || b.repo_info(Some(&r)))));
        }
        if let Some((repo, t)) = &mut self.info_task
            && let Some(r) = t.take()
        {
            let repo = repo.clone();
            self.info_task = None;
            if self.selected.as_ref() == Some(&repo) {
                match r {
                    Ok(info) => self.selected_info = Some(info),
                    Err(_) => {
                        self.selected_info = Some(RepoInfo { repo, ..Default::default() });
                    }
                }
            }
        }
        // 패널은 대상 저장소가 바뀌면 새로 만든다.
        let target = self.repo();
        if self.panels.as_ref().is_some_and(|p| target.as_ref() != Some(&p.repo)) {
            self.drafts = self.recovery_drafts();
        }
        match (&target, &self.panels) {
            (Some(r), Some(p)) if &p.repo == r => {}
            (Some(r), _) => {
                let local = self.selected.is_none();
                let repo_arg = if local { None } else { Some(r.clone()) };
                let mut pr = PrPanel::with_backend(self.backend.pr_backend(repo_arg.as_ref()));
                pr.set_embedded(true);
                pr.set_can_create(local);
                let mut issues = IssuePanel::with_backend(self.backend.clone(), repo_arg.clone());
                issues.set_embedded(true);
                let mut actions = ActionsPanel::with_backend(self.backend.clone(), repo_arg);
                actions.set_embedded(true);
                if let Some(ts) = self.now_override {
                    pr.set_now(ts);
                    issues.set_now(ts);
                    actions.set_now(ts);
                }
                if let Some(drafts) = self.drafts.repositories.get(&r.full_name()) {
                    if let Some(draft) = &drafts.pull_request { pr.restore_creation_draft(draft); }
                    if let Some(draft) = &drafts.issue { issues.restore_creation_draft(draft); }
                }
                self.panels = Some(Panels { repo: r.clone(), pr, issues, actions });
            }
            (None, _) => self.panels = None,
        }
    }

    /// 허브를 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let mut events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg_panel).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
                self.ui_header(ui, &mut events);
            });
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, t.border);
            self.ui_body(ui, &mut events);
        });
        events
    }

    fn ui_header(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let t = theme();
        let drafts = self.recovery_drafts();
        if !drafts.repositories.is_empty() {
            egui::CollapsingHeader::new(kiln_common::trf!("보관된 작성 초안 · 저장소 {}개", drafts.repositories.len()))
                .id_salt("github_saved_drafts").show(ui, |ui| {
                    ui.label(faint(kiln_common::i18n::tr("전송 전 GitHub에서 이미 등록되었는지 확인하세요.")));
                    for (name, draft) in &drafts.repositories {
                        for (exists, tab, title) in [(draft.pull_request.is_some(), HubTab::PullRequests, kiln_common::i18n::tr("풀 리퀘스트")), (draft.issue.is_some(), HubTab::Issues, kiln_common::i18n::tr("이슈"))] {
                            if exists && ui.add_enabled(!self.is_submitting(), egui::Button::new(kiln_common::trf!("{name} · {title} 이어 쓰기"))).clicked() {
                                if let Some(repo) = RepoRef::parse(name) {
                                    if self.workspace.as_ref().is_some_and(|w| w.repo == repo) { self.use_workspace_repo(); }
                                    else { self.select_repo(repo); }
                                    self.tab = tab;
                                    if let Some(panels) = &mut self.panels && panels.repo.full_name() == *name {
                                        match tab { HubTab::PullRequests => panels.pr.open_create_form(), HubTab::Issues => panels.issues.open_create_form(), _ => {} }
                                    }
                                }
                            }
                        }
                    }
                });
        }
        let info = self.repo_info().cloned();
        let repo = self.repo();
        let chip = ui
            .horizontal(|ui| {
                let label = match &repo {
                    Some(r) => r.full_name(),
                    None if self.detect.is_some() => kiln_common::i18n::tr("저장소 확인 중…").into(),
                    None => kiln_common::i18n::tr("저장소 선택").into(),
                };
                let chip = repo_chip(ui, &label, info.as_ref().is_some_and(|i| i.is_private), self.selected.is_some());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.is_busy_header() {
                        spinner(ui, 12.0);
                    } else if icon_button(ui, Icon::Refresh, kiln_common::i18n::tr("모두 새로 고침")).clicked() {
                        self.refresh();
                    }
                    if let Some(i) = &info
                        && !i.url.is_empty()
                        && icon_button(ui, Icon::External, kiln_common::i18n::tr("GitHub에서 열기")).clicked()
                    {
                        events.push(GitEvent::OpenUrl(i.url.clone()));
                    }
                    if let Some(i) = &info
                        && i.stargazer_count > 0
                    {
                        ui.label(faint(compact_count(i.stargazer_count)));
                        icon_label(ui, Icon::Star, t.text_faint, 12.0);
                    }
                });
                chip
            })
            .inner;
        if self.is_submitting() { ui.label(faint(kiln_common::i18n::tr("전송 중에는 저장소를 바꿀 수 없습니다"))); }
        if chip.clicked() && !self.is_submitting() {
            self.picker.toggle(ui.ctx());
        }
        let ws = self.workspace.as_ref().map(|w| w.repo.clone());
        if let Some(a) = self.picker.show(ui, chip.rect, info.as_ref(), ws.as_ref()) {
            match a {
                RepoPickerAction::Select(r) => self.select_repo(r),
                RepoPickerAction::UseWorkspace => self.use_workspace_repo(),
                RepoPickerAction::Clone(n) => events.push(GitEvent::CloneRepo { name_with_owner: n }),
            }
        }
        if let Some(i) = &info
            && !i.description.is_empty()
        {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(&i.description).size(12.0).color(t.text_faint)).truncate());
        }
        if repo.is_some() {
            ui.add_space(8.0);
            let active = self.panels.as_ref().map(|p| p.actions.active_count() as u64).filter(|n| *n > 0);
            let mut tab = self.tab;
            let tabs = [
                (HubTab::PullRequests, Icon::PullRequest, kiln_common::i18n::tr("풀 리퀘스트"), info.as_ref().and_then(|i| i.open_prs)),
                (HubTab::Issues, Icon::Issue, kiln_common::i18n::tr("이슈"), info.as_ref().and_then(|i| i.open_issues)),
                (HubTab::Actions, Icon::Actions, "Actions", active),
            ];
            if count_tabs(ui, &mut tab, &tabs) {
                self.tab = tab;
            }
        }
    }

    fn is_busy_header(&mut self) -> bool {
        self.detect.is_some() || self.info_task.is_some()
    }

    fn ui_body(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        if self.repo().is_none() {
            if self.detect.is_some() || !self.started {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    spinner(ui, 14.0);
                    ui.label(dim(kiln_common::i18n::tr("GitHub 저장소 확인 중…")));
                });
                return;
            }
            if let Some(e) = self.detect_error.clone() {
                egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                    if matches!(e, GitError::GhMissing | GitError::GhAuth(_)) {
                        if gh_error_state(ui, &e, "GitHub", events) == ErrorAction::Retry {
                            self.refresh();
                        }
                        return;
                    }
                    let clicked = empty_panel(
                        ui,
                        Icon::Repo,
                        kiln_common::i18n::tr("GitHub 저장소를 찾을 수 없습니다"),
                        &no_repo_detail(&e),
                        None,
                        &[
                            (None, kiln_common::i18n::tr("다른 저장소 선택…"), kiln_common::widgets::ButtonKind::Primary),
                            (Some(kiln_common::icons::Icon::Refresh), kiln_common::i18n::tr("다시 시도"), kiln_common::widgets::ButtonKind::Secondary),
                        ],
                    );
                    match clicked {
                        Some(0) => self.picker.toggle(ui.ctx()),
                        Some(_) => self.refresh(),
                        None => {}
                    }
                });
            }
            return;
        }
        let Some(p) = &mut self.panels else { return };
        let ev = match self.tab {
            HubTab::PullRequests => p.pr.ui(ui),
            HubTab::Issues => p.issues.ui(ui),
            HubTab::Actions => p.actions.ui(ui),
        };
        events.extend(ev);
    }
}

/// 저장소를 찾지 못한 이유를 사람이 읽을 문구로.
fn no_repo_detail(e: &GitError) -> String {
    match e {
        GitError::NotARepo => kiln_common::i18n::tr("이 폴더는 Git 저장소가 아닙니다. GitHub 저장소를 직접 선택할 수 있습니다.").into(),
        GitError::Failed(m) if m.contains("no git remotes") || m.contains("none of the git remotes") => {
            kiln_common::i18n::tr("GitHub 원격 저장소가 연결되어 있지 않습니다. GitHub 저장소를 직접 선택할 수 있습니다.").into()
        }
        other => other.to_string(),
    }
}

/// 저장소 칩 버튼(책 아이콘 + 이름 + ▾).
fn repo_chip(ui: &mut Ui, label: &str, private: bool, overridden: bool) -> egui::Response {
    let t = theme();
    let g = ui.painter().layout_no_wrap(label.to_string(), kiln_common::fonts::semibold(13.0), t.text);
    let max_w = (ui.available_width() - 90.0).max(120.0);
    let w = (g.size().x + 52.0).min(max_w);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, kiln_common::trf!("저장소 {label}")));
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { t.bg_hover } else { t.bg_elevated };
        let stroke = if overridden { alpha(t.accent, 0.6) } else { t.border };
        ui.painter().rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let y = rect.center().y;
        paint_icon(
            ui.painter(),
            Rect::from_center_size(pos2(rect.left() + 17.0, y), vec2(14.0, 14.0)),
            if private { Icon::Lock } else { Icon::Repo },
            if overridden { t.accent } else { t.text_dim },
        );
        let job = super::panel::one_line_job(&[(label, 13.0, t.text)], rect.width() - 52.0);
        let mut job = job;
        for s in &mut job.sections {
            s.format.font_id = kiln_common::fonts::semibold(13.0);
        }
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(rect.left() + 30.0, y - g.size().y / 2.0), g, t.text);
        paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.right() - 13.0, y), vec2(11.0, 11.0)), Icon::ChevronDown, t.text_faint);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    resp
}
