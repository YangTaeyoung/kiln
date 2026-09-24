//! PR 목록 패널(PrPanel)과 PR 생성 폼.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Color32, CornerRadius, FontId, Id, Layout, Margin, RichText, Sense, Stroke, Ui, pos2, vec2};
use kiln_common::Task;

use super::panel::one_line_job;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::{GhBackend, PrBackend, PrCreate, PrCreateDefaults, PrFilter, PrItem};
use crate::util::{now_unix, parse_iso8601, short_relative_time};

const PR_ROW_H: f32 = 46.0;

struct CreateForm {
    defaults: Option<Task<GitResult<PrCreateDefaults>>>,
    head: String,
    bases: Vec<String>,
    req: PrCreate,
    submit: Option<Task<GitResult<String>>>,
    error: Option<String>,
}

/// PR 목록 패널.
pub struct PrPanel {
    backend: Arc<dyn PrBackend>,
    filter: PrFilter,
    search: String,
    load: Option<Task<GitResult<Vec<PrItem>>>>,
    started: bool,
    items: Vec<PrItem>,
    error: Option<GitError>,
    form: Option<CreateForm>,
    created: Option<String>,
    now_override: Option<i64>,
}

impl PrPanel {
    pub fn new(root: PathBuf) -> Self {
        Self::with_backend(Arc::new(GhBackend::new(root)))
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(backend: Arc<dyn PrBackend>) -> Self {
        Self {
            backend,
            filter: PrFilter::Open,
            search: String::new(),
            load: None,
            started: false,
            items: Vec::new(),
            error: None,
            form: None,
            created: None,
            now_override: None,
        }
    }

    pub fn refresh(&mut self) {
        self.started = false;
    }

    pub fn set_filter(&mut self, f: PrFilter) {
        if self.filter != f {
            self.filter = f;
            self.refresh();
        }
    }

    pub fn filter(&self) -> PrFilter {
        self.filter
    }

    pub fn items(&self) -> &[PrItem] {
        &self.items
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started
            || self.load.as_mut().is_some_and(|t| t.is_pending())
            || self.form.as_mut().is_some_and(|f| {
                f.defaults.as_mut().is_some_and(|t| t.is_pending()) || f.submit.as_mut().is_some_and(|t| t.is_pending())
            })
    }

    /// PR 생성 폼을 연다.
    pub fn open_create_form(&mut self) {
        self.form = Some(CreateForm {
            defaults: None,
            head: String::new(),
            bases: Vec::new(),
            req: PrCreate::default(),
            submit: None,
            error: None,
        });
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let b = self.backend.clone();
            let f = self.filter;
            self.load = Some(Task::spawn(ctx, move || b.list(f)));
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            match r {
                Ok(v) => {
                    self.items = v;
                    self.error = None;
                }
                Err(e) => {
                    self.items.clear();
                    self.error = Some(e);
                }
            }
        }
        if let Some(form) = &mut self.form {
            if form.defaults.is_none() && form.head.is_empty() && form.error.is_none() {
                let b = self.backend.clone();
                form.defaults = Some(Task::spawn(ctx, move || b.create_defaults()));
            }
            if let Some(t) = &mut form.defaults
                && let Some(r) = t.take()
            {
                form.defaults = None;
                match r {
                    Ok(d) => {
                        form.head = d.head;
                        form.bases = d.bases;
                        form.req.base = d.base;
                        form.req.title = d.title;
                        form.req.body = d.body;
                    }
                    Err(e) => form.error = Some(e.to_string()),
                }
            }
            if let Some(t) = &mut form.submit
                && let Some(r) = t.take()
            {
                form.submit = None;
                match r {
                    Ok(url) => {
                        self.created = Some(url);
                        self.form = None;
                        self.refresh();
                    }
                    Err(e) => form.error = Some(e.to_string()),
                }
            }
        }
    }

    /// 패널을 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let mut events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg_panel).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            egui::Frame::new().inner_margin(Margin { left: 10, right: 10, top: 8, bottom: 8 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Pull Requests").size(14.0).strong().color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.form.is_none() && primary_button(ui, "New PR", None).on_hover_text("Create a pull request").clicked()
                        {
                            self.open_create_form();
                        }
                        if self.load.is_some() {
                            spinner(ui, 12.0);
                        } else if icon_button(ui, Icon::Refresh, "Refresh").clicked() {
                            self.refresh();
                        }
                    });
                });
                ui.add_space(4.0);
                let mut f = self.filter;
                let opts: Vec<(PrFilter, &str)> = PrFilter::ALL.iter().map(|f| (*f, f.label())).collect();
                if segmented(ui, &mut f, &opts) {
                    self.set_filter(f);
                }
                ui.add_space(4.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Filter by title, author, branch or #number")
                        .desired_width(f32::INFINITY)
                        .frame(input_frame()),
                );
                if let Some(url) = self.created.clone() {
                    ui.add_space(4.0);
                    if banner(ui, BannerKind::Success, "Pull request created", Some(&url), true) {
                        self.created = None;
                    }
                }
            });
            ui.add(egui::Separator::default().spacing(0.0));

            if self.form.is_some() {
                egui::ScrollArea::vertical().id_salt("pr_form").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| self.ui_form(ui));
                });
                return;
            }
            self.ui_list(ui, &mut events);
        });
        events
    }

    fn ui_list(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let t = theme();
        if let Some(e) = &self.error {
            let (title, detail) = match e {
                GitError::GhMissing => ("GitHub CLI not found", "Install gh from https://cli.github.com to see pull requests.".to_string()),
                GitError::GhAuth(_) => ("Not signed in to GitHub", "Run `gh auth login` in a terminal, then refresh.".to_string()),
                GitError::NotARepo => ("Not a git repository", String::new()),
                other => ("Could not load pull requests", other.to_string()),
            };
            let is_auth = matches!(e, GitError::GhAuth(_));
            egui::Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                banner(ui, BannerKind::Warning, title, Some(&detail), false);
                if is_auth {
                    ui.add_space(6.0);
                    if tool_button(ui, None, "Run gh auth login in terminal").clicked() {
                        events.push(GitEvent::RunInTerminal("gh auth login".into()));
                    }
                }
            });
            return;
        }
        if self.load.is_some() && self.items.is_empty() {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                spinner(ui, 14.0);
                ui.label(dim("Loading pull requests…"));
            });
            return;
        }
        let q = self.search.trim().to_lowercase();
        let q_num = q.trim_start_matches('#').parse::<u64>().ok();
        let items: Vec<&PrItem> = self
            .items
            .iter()
            .filter(|p| {
                q.is_empty()
                    || q_num == Some(p.number)
                    || p.title.to_lowercase().contains(&q)
                    || p.author.login.to_lowercase().contains(&q)
                    || p.head_ref_name.to_lowercase().contains(&q)
            })
            .collect();
        if items.is_empty() {
            let msg = match self.filter {
                PrFilter::Mine => "You have no open pull requests",
                PrFilter::ReviewRequested => "No reviews requested from you",
                _ => "No pull requests",
            };
            empty_state(ui, msg, if q.is_empty() { "" } else { "Try a different filter." });
            return;
        }
        let now = self.now_override.unwrap_or_else(now_unix);
        egui::ScrollArea::vertical().id_salt("pr_list").auto_shrink([false, false]).show_rows(
            ui,
            PR_ROW_H,
            items.len(),
            |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for p in &items[range] {
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, PR_ROW_H), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("#{} {}", p.number, p.title)));
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
                    }
                    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, alpha(t.border, 0.6)));
                    let pr_state = p.pr_state();
                    let sc = pr_state_color(pr_state, p.is_draft);
                    let painter = ui.painter();
                    // 상태 아이콘
                    paint_icon(painter, egui::Rect::from_center_size(rect.left_top() + vec2(16.0, 14.0), vec2(14.0, 14.0)), Icon::PullRequest, sc);
                    // 오른쪽 배지들
                    let mut rx = rect.right() - 10.0;
                    let y1 = rect.top() + 14.0;
                    if let Some(c) = p.checks() {
                        let (icon, c) = checks_icon(c);
                        let r = egui::Rect::from_center_size(pos2(rx - 7.0, y1), vec2(14.0, 14.0));
                        paint_icon(painter, r, icon, c);
                        rx = r.left() - 8.0;
                    }
                    let mut chip = |label: &str, fg: Color32| {
                        let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(10.5), fg);
                        let br = egui::Rect::from_min_size(pos2(rx - g.size().x - 10.0, y1 - 8.0), vec2(g.size().x + 10.0, 16.0));
                        painter.rect(br, CornerRadius::same(8), alpha(fg, 0.12), Stroke::new(1.0, alpha(fg, 0.45)), egui::StrokeKind::Inside);
                        painter.galley(br.center() - g.size() / 2.0, g, fg);
                        rx = br.left() - 5.0;
                    };
                    if p.is_draft {
                        chip("Draft", t.text_dim);
                    }
                    if let Some(r) = p.review() {
                        chip(r.label(), review_color(r));
                    }
                    if pr_state != crate::gh::PrState::Open {
                        chip(if pr_state == crate::gh::PrState::Merged { "Merged" } else { "Closed" }, sc);
                    }
                    let x0 = rect.left() + 30.0;
                    let title_job = one_line_job(&[(&p.title, 13.0, t.text)], (rx - x0 - 4.0).max(40.0));
                    let g = painter.layout_job(title_job);
                    painter.galley(pos2(x0, y1 - g.size().y / 2.0), g, t.text);
                    let updated = parse_iso8601(&p.updated_at).map(|ts| short_relative_time(ts, now)).unwrap_or_default();
                    let meta = format!("#{} · {} · {} › {} · {}", p.number, p.author.login, p.head_ref_name, p.base_ref_name, updated);
                    let adds = format!("+{}", p.additions);
                    let dels = format!("−{}", p.deletions);
                    let ga = painter.layout_no_wrap(dels.clone(), FontId::monospace(10.5), t.red);
                    let r_d = egui::Rect::from_min_size(pos2(rect.right() - 10.0 - ga.size().x, rect.top() + 26.0), ga.size());
                    painter.galley(r_d.min, ga, t.red);
                    let gb = painter.layout_no_wrap(adds, FontId::monospace(10.5), t.green);
                    let r_a = egui::Rect::from_min_size(pos2(r_d.left() - 6.0 - gb.size().x, rect.top() + 26.0), gb.size());
                    painter.galley(r_a.min, gb, t.green);
                    let g = painter.layout_job(one_line_job(&[(&meta, 11.0, t.text_faint)], (r_a.left() - x0 - 8.0).max(40.0)));
                    painter.galley(pos2(x0, rect.top() + 26.0), g, t.text_faint);
                    if resp.clicked() {
                        events.push(GitEvent::OpenPr(p.number));
                    }
                    resp.on_hover_text(format!("#{} {}\n{}", p.number, p.title, p.url));
                }
            },
        );
    }

    fn ui_form(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(form) = &mut self.form else { return };
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("New Pull Request").size(14.0).strong().color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.form = None;
            return;
        }
        ui.add_space(4.0);
        if let Some(e) = form.error.clone() {
            if banner(ui, BannerKind::Error, "Could not create pull request", Some(&e), true) {
                form.error = None;
            }
            ui.add_space(4.0);
        }
        if form.defaults.is_some() {
            ui.horizontal(|ui| {
                spinner(ui, 12.0);
                ui.label(dim("Preparing from branch commits…"));
            });
            return;
        }
        let submitting = form.submit.is_some();
        ui.add_enabled_ui(!submitting, |ui| {
            ui.horizontal(|ui| {
                ui.label(dim("From"));
                outline_badge(ui, if form.head.is_empty() { "?" } else { &form.head }, t.accent);
                ui.label(dim("into"));
                egui::ComboBox::from_id_salt(Id::new("pr_create_base"))
                    .selected_text(RichText::new(&form.req.base).size(12.5))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for b in &form.bases {
                            ui.selectable_value(&mut form.req.base, b.clone(), b);
                        }
                    });
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Title").size(11.5).color(t.text_dim));
            ui.add(
                egui::TextEdit::singleline(&mut form.req.title)
                    .hint_text("Pull request title")
                    .desired_width(f32::INFINITY)
                    .frame(input_frame()),
            );
            ui.add_space(4.0);
            ui.label(RichText::new("Description").size(11.5).color(t.text_dim));
            ui.add(
                egui::TextEdit::multiline(&mut form.req.body)
                    .hint_text("Describe your changes (Markdown supported)")
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .frame(input_frame()),
            );
            ui.add_space(4.0);
            checkbox_row(ui, &mut form.req.draft, "Create as draft");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can = !form.req.title.trim().is_empty() && !form.req.base.is_empty() && !submitting;
                let label = if form.req.draft { "Create Draft PR" } else { "Create Pull Request" };
                if ui.add_enabled_ui(can, |ui| primary_button(ui, label, None)).inner.clicked() {
                    let b = self.backend.clone();
                    let req = form.req.clone();
                    form.error = None;
                    form.submit = Some(Task::spawn(ui.ctx(), move || b.create(&req)));
                }
                if submitting {
                    spinner(ui, 12.0);
                    ui.label(dim("Creating…"));
                }
            });
        });
    }
}
