//! GitHub Actions 실행 목록 패널(ActionsPanel).

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Id, Layout, Margin, Rect, RichText, Sense, Ui, pos2, vec2};
use kiln_common::Task;

use super::gh_widgets::*;
use super::panel::one_line_job;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::GhBackend;
use crate::github::{GithubBackend, RepoRef, RunFilter, RunItem, RunStatus, gh_command};
use crate::util::{now_unix, parse_iso8601, short_relative_time};

const ROW_H: f32 = 54.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    Rerun,
    Cancel,
}

/// Actions 실행 목록 패널.
pub struct ActionsPanel {
    backend: Arc<dyn GithubBackend>,
    repo: Option<RepoRef>,
    filter: RunFilter,
    load: Option<Task<GitResult<Vec<RunItem>>>>,
    started: bool,
    items: Vec<RunItem>,
    /// 지금까지 본 브랜치/워크플로 이름(필터 선택지).
    branches: Vec<String>,
    workflows: Vec<String>,
    error: Option<GitError>,
    selected: Option<u64>,
    action: Option<(ActionKind, u64, Task<GitResult<()>>)>,
    action_msg: Option<(BannerKind, String, Option<String>)>,
    embedded: bool,
    loaded_at: Option<f64>,
    now_override: Option<i64>,
}

impl ActionsPanel {
    /// 작업 폴더 저장소(`repo` 가 `None`)나 지정 저장소의 Actions 패널.
    pub fn new(root: PathBuf, repo: Option<RepoRef>) -> Self {
        Self::with_backend(Arc::new(GhBackend::new(root)), repo)
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(backend: Arc<dyn GithubBackend>, repo: Option<RepoRef>) -> Self {
        Self {
            backend,
            repo,
            filter: RunFilter::default(),
            load: None,
            started: false,
            items: Vec::new(),
            branches: Vec::new(),
            workflows: Vec::new(),
            error: None,
            selected: None,
            action: None,
            action_msg: None,
            embedded: false,
            loaded_at: None,
            now_override: None,
        }
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        self.repo.as_ref()
    }

    pub fn refresh(&mut self) {
        self.started = false;
    }

    pub fn items(&self) -> &[RunItem] {
        &self.items
    }

    pub fn filter(&self) -> &RunFilter {
        &self.filter
    }

    pub fn set_filter(&mut self, f: RunFilter) {
        if self.filter != f {
            self.filter = f;
            self.refresh();
        }
    }

    /// 진행 중이거나 대기 중인 실행 수.
    pub fn active_count(&self) -> usize {
        self.items.iter().filter(|r| r.run_status().is_active()).count()
    }

    /// 실행 하나를 펼쳐 동작 버튼을 보인다.
    pub fn select(&mut self, run_id: Option<u64>) {
        self.selected = run_id;
    }

    /// 허브 안에 넣을 때 제목을 숨긴다.
    pub fn set_embedded(&mut self, on: bool) {
        self.embedded = on;
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started || self.load.as_mut().is_some_and(|t| t.is_pending()) || self.action.as_mut().is_some_and(|(_, _, t)| t.is_pending())
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let (b, r, f) = (self.backend.clone(), self.repo.clone(), self.filter.clone());
            self.load = Some(Task::spawn(ctx, move || b.runs(r.as_ref(), &f)));
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            match r {
                Ok(v) => {
                    for run in &v {
                        if !run.head_branch.is_empty() && !self.branches.contains(&run.head_branch) {
                            self.branches.push(run.head_branch.clone());
                        }
                        if !run.workflow_name.is_empty() && !self.workflows.contains(&run.workflow_name) {
                            self.workflows.push(run.workflow_name.clone());
                        }
                    }
                    self.branches.sort();
                    self.workflows.sort();
                    self.items = v;
                    self.error = None;
                    self.loaded_at = Some(ctx.input(|i| i.time));
                }
                Err(e) => {
                    self.items.clear();
                    self.error = Some(e);
                }
            }
        }
        if let Some((kind, id, t)) = &mut self.action
            && let Some(r) = t.take()
        {
            let (kind, id) = (*kind, *id);
            self.action = None;
            match r {
                Ok(()) => {
                    let title = match kind {
                        ActionKind::Rerun => format!("실행 {id}을(를) 다시 시작했습니다"),
                        ActionKind::Cancel => format!("실행 {id}을(를) 취소했습니다"),
                    };
                    self.action_msg = Some((BannerKind::Success, title, None));
                    self.refresh();
                }
                Err(e) => self.action_msg = Some((BannerKind::Error, "작업 실패".into(), Some(e.to_string()))),
            }
        }
        // 진행 중인 실행이 있으면 15초마다 새로 고친다.
        if self.load.is_none() && self.active_count() > 0 {
            let now = ctx.input(|i| i.time);
            match self.loaded_at {
                Some(at) if now - at >= 15.0 => self.refresh(),
                Some(_) => ctx.request_repaint_after(std::time::Duration::from_secs(15)),
                None => self.loaded_at = Some(now),
            }
        }
    }

    fn run(&mut self, ctx: &egui::Context, kind: ActionKind, id: u64, f: impl FnOnce(&dyn GithubBackend, Option<&RepoRef>) -> GitResult<()> + Send + 'static) {
        let (b, r) = (self.backend.clone(), self.repo.clone());
        self.action_msg = None;
        self.action = Some((kind, id, Task::spawn(ctx, move || f(b.as_ref(), r.as_ref()))));
    }

    /// 패널을 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let mut events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg_panel).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 8 }).show(ui, |ui| self.ui_toolbar(ui));
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line.shrink2(vec2(12.0, 0.0)), 0.0, t.border);
            ui.add_space(4.0);
            self.ui_list(ui, &mut events);
        });
        events
    }

    fn ui_toolbar(&mut self, ui: &mut Ui) {
        let t = theme();
        ui.horizontal(|ui| {
            if self.embedded {
                if self.started && self.load.is_none() && self.error.is_none() {
                    let active = self.active_count();
                    let s = if active > 0 { format!("실행 {}개 · 진행 중 {active}", self.items.len()) } else { format!("실행 {}개", self.items.len()) };
                    ui.label(faint(s));
                }
            } else {
                ui.label(RichText::new("Actions").font(kiln_common::fonts::semibold(13.5)).color(t.text));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.load.is_some() {
                    spinner(ui, 12.0);
                } else if icon_button(ui, Icon::Refresh, "새로 고침").clicked() {
                    self.refresh();
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let bv = self.filter.branch.clone().unwrap_or_else(|| "전체".into());
            let br = filter_button(ui, "브랜치", &bv, self.filter.branch.is_some());
            let bid = Id::new("actions_branch_filter");
            if br.clicked() {
                egui::Popup::toggle_id(ui.ctx(), bid);
            }
            let wv = self.filter.workflow.clone().unwrap_or_else(|| "전체".into());
            let wr = filter_button(ui, "워크플로", &wv, self.filter.workflow.is_some());
            let wid = Id::new("actions_workflow_filter");
            if wr.clicked() {
                egui::Popup::toggle_id(ui.ctx(), wid);
            }
            let mut opts: Vec<(Option<String>, String)> = vec![(None, "모든 브랜치".into())];
            opts.extend(self.branches.iter().map(|b| (Some(b.clone()), b.clone())));
            if let Some(v) = single_pick_popup(ui, bid, br.rect, &opts, &self.filter.branch) {
                let f = RunFilter { branch: v, ..self.filter.clone() };
                self.set_filter(f);
            }
            let mut opts: Vec<(Option<String>, String)> = vec![(None, "모든 워크플로".into())];
            opts.extend(self.workflows.iter().map(|w| (Some(w.clone()), w.clone())));
            if let Some(v) = single_pick_popup(ui, wid, wr.rect, &opts, &self.filter.workflow) {
                let f = RunFilter { workflow: v, ..self.filter.clone() };
                self.set_filter(f);
            }
        });
        if let Some((k, title, detail)) = self.action_msg.clone() {
            ui.add_space(6.0);
            if banner(ui, k, &title, detail.as_deref(), true) {
                self.action_msg = None;
            }
        }
    }

    fn ui_list(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        if let Some(e) = self.error.clone() {
            if gh_error_state(ui, &e, "워크플로 실행", events) == ErrorAction::Retry {
                self.refresh();
            }
            return;
        }
        if self.load.is_some() && self.items.is_empty() {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                spinner(ui, 14.0);
                ui.label(dim("워크플로 실행 불러오는 중…"));
            });
            return;
        }
        if self.items.is_empty() {
            let filtered = self.filter != RunFilter::default();
            empty_state_icon(ui, Icon::Actions, "워크플로 실행이 없습니다", if filtered { "다른 브랜치나 워크플로를 선택해 보세요." } else { "" });
            return;
        }
        let now = self.now_override.unwrap_or_else(now_unix);
        let items = self.items.clone();
        let busy = self.action.is_some();
        egui::ScrollArea::vertical().id_salt("actions_list").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for run in &items {
                let status = run.run_status();
                let sel = self.selected == Some(run.database_id);
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, format!("{} {}", run.workflow_name, run.display_title)));
                if ui.is_rect_visible(rect) {
                    kiln_common::widgets::paint_row(ui.painter(), rect.shrink2(vec2(6.0, 1.0)), sel, resp.hovered());
                    paint_run_row(ui, rect, run, status, now, self.now_override.is_none());
                }
                if resp.clicked() {
                    self.selected = if sel { None } else { Some(run.database_id) };
                }
                resp.on_hover_text(format!("{} — {}\n{}", run.workflow_name, status.label(), run.url));
                if sel {
                    egui::Frame::new().inner_margin(Margin { left: 38, right: 12, top: 2, bottom: 10 }).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            self.ui_run_actions(ui, run, status, busy, events);
                        });
                    });
                }
            }
        });
    }

    fn ui_run_actions(&mut self, ui: &mut Ui, run: &RunItem, status: RunStatus, busy: bool, events: &mut Vec<GitEvent>) {
        let id = run.database_id;
        let repo = self.repo.clone();
        if status.is_active() {
            if tool_button(ui, Some(Icon::Play), "실행 지켜보기").on_hover_text("터미널에서 gh run watch").clicked() {
                events.push(GitEvent::RunInTerminal(gh_command(&format!("run watch {id}"), repo.as_ref())));
            }
        } else if tool_button(ui, Some(Icon::Open), "로그 보기").on_hover_text("터미널에서 gh run view --log").clicked() {
            events.push(GitEvent::RunInTerminal(gh_command(&format!("run view {id} --log"), repo.as_ref())));
        }
        ui.add_enabled_ui(!busy, |ui| {
            if status.is_active() {
                if tool_button(ui, Some(Icon::Stop), "실행 취소").clicked() {
                    self.run(ui.ctx(), ActionKind::Cancel, id, move |b, r| b.cancel_run(r, id));
                }
            } else {
                if tool_button(ui, Some(Icon::Refresh), "다시 실행").clicked() {
                    self.run(ui.ctx(), ActionKind::Rerun, id, move |b, r| b.rerun(r, id, false));
                }
                if status == RunStatus::Failure && tool_button(ui, Some(Icon::Refresh), "실패한 작업만 다시 실행").clicked() {
                    self.run(ui.ctx(), ActionKind::Rerun, id, move |b, r| b.rerun(r, id, true));
                }
            }
        });
        if tool_button(ui, Some(Icon::External), "브라우저에서 열기").clicked() && !run.url.is_empty() {
            events.push(GitEvent::OpenUrl(run.url.clone()));
        }
        if busy && self.action.as_ref().is_some_and(|(_, aid, _)| *aid == id) {
            spinner(ui, 12.0);
        }
    }
}

fn paint_run_row(ui: &Ui, rect: Rect, run: &RunItem, status: RunStatus, now: i64, animate: bool) {
    let t = theme();
    let p = ui.painter();
    let (icon, c) = run_status_style(status);
    let y1 = rect.top() + 17.0;
    let y2 = rect.top() + 37.0;
    let ic = pos2(rect.left() + 22.0, y1);
    if status == RunStatus::InProgress {
        // 진행 중: 회전하는 호
        let time = if animate { ui.input(|i| i.time) as f32 } else { 0.0 };
        let r = 6.0;
        p.circle_stroke(ic, r, egui::Stroke::new(1.6, alpha(c, 0.25)));
        let start = time * 4.0;
        let pts: Vec<egui::Pos2> = (0..=12).map(|i| {
            let a = start + i as f32 * 0.16;
            pos2(ic.x + r * a.cos(), ic.y + r * a.sin())
        }).collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.8, c)));
        if animate {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
    } else {
        paint_icon(p, Rect::from_center_size(ic, vec2(15.0, 15.0)), icon, c);
    }
    // 오른쪽: 상태 + 시간
    let when = parse_iso8601(&run.created_at).map(|ts| short_relative_time(ts, now)).unwrap_or_default();
    let tg = p.layout_no_wrap(when, kiln_common::fonts::regular(11.0), t.text_faint);
    let tx = rect.right() - 16.0 - tg.size().x;
    p.galley(pos2(tx, y1 - tg.size().y / 2.0), tg, t.text_faint);
    let sg = p.layout_no_wrap(status.label().to_string(), kiln_common::fonts::medium(11.0), c);
    let sx = tx - 8.0 - sg.size().x;
    p.galley(pos2(sx, y1 - sg.size().y / 2.0), sg, c);
    let x0 = rect.left() + 38.0;
    let g = p.layout_job(one_line_job(&[(&run.display_title, 13.5, t.text)], (sx - x0 - 10.0).max(40.0)));
    p.galley(pos2(x0, y1 - g.size().y / 2.0), g, t.text);
    // 둘째 줄: 워크플로 · #번호 · 브랜치 칩 · 이벤트
    let mut meta = run.workflow_name.clone();
    if run.number > 0 {
        meta.push_str(&format!(" #{}", run.number));
    }
    if run.attempt > 1 {
        meta.push_str(&format!(" (시도 {})", run.attempt));
    }
    let g = p.layout_job(one_line_job(&[(&meta, 11.0, t.text_dim)], (rect.width() * 0.45).max(60.0)));
    let mut x = x0 + g.size().x + 8.0;
    p.galley(pos2(x0, y2 - g.size().y / 2.0), g, t.text_dim);
    if !run.head_branch.is_empty() {
        let bg = p.layout_no_wrap(run.head_branch.clone(), kiln_common::fonts::mono(10.5), t.accent);
        let bw = (bg.size().x + 12.0).min(rect.right() - 16.0 - x - 60.0);
        if bw > 30.0 {
            let br = Rect::from_min_size(pos2(x, y2 - 8.5), vec2(bw, 17.0));
            p.rect_filled(br, egui::CornerRadius::same(5), alpha(t.accent, if t.dark { 0.14 } else { 0.10 }));
            let job = one_line_job(&[(&run.head_branch, 10.5, t.accent)], bw - 10.0);
            let mut job = job;
            for s in &mut job.sections {
                s.format.font_id = kiln_common::fonts::mono(10.5);
            }
            let g = p.layout_job(job);
            p.galley(pos2(br.left() + 6.0, y2 - g.size().y / 2.0), g, t.accent);
            x = br.right() + 8.0;
        }
    }
    let eg = p.layout_job(one_line_job(&[(run.event_label(), 11.0, t.text_faint)], (rect.right() - 16.0 - x).max(10.0)));
    p.galley(pos2(x, y2 - eg.size().y / 2.0), eg, t.text_faint);
}
