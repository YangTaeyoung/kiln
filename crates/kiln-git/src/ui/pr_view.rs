//! PR 상세 뷰(PrView): 헤더, 본문, 체크, 타임라인, 변경 파일, 리뷰/병합 동작.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use kiln_common::Task;

use super::diff_view::DiffView;
use super::markdown::Markdown;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::GitResult;
use crate::gh::{ChecksState, GhBackend, MergeMethod, PrBackend, PrDetail, PrState, ReviewKind};
use crate::util::{now_unix, parse_iso8601, relative_time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrTab {
    Conversation,
    Checks,
    Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    Checkout,
    Review,
    Merge,
    Ready,
}

struct MergeDialog {
    method: MergeMethod,
    delete_branch: bool,
}

/// 타임라인 항목(설명/코멘트/리뷰).
struct TimelineItem {
    author: String,
    when: i64,
    kind: String,
    body: Markdown,
}

/// PR 상세 뷰.
pub struct PrView {
    root: PathBuf,
    number: u64,
    backend: Arc<dyn PrBackend>,
    load: Option<Task<GitResult<PrDetail>>>,
    started: bool,
    detail: Option<PrDetail>,
    body: Markdown,
    timeline: Vec<TimelineItem>,
    error: Option<String>,
    tab: PrTab,
    diff_load: Option<Task<GitResult<String>>>,
    diff: Option<DiffView>,
    diff_error: Option<String>,
    review_body: String,
    action: Option<(ActionKind, Task<GitResult<String>>)>,
    action_msg: Option<(BannerKind, String, Option<String>)>,
    merge: Option<MergeDialog>,
    now_override: Option<i64>,
}

impl PrView {
    pub fn new(root: PathBuf, number: u64) -> Self {
        let backend = Arc::new(GhBackend::new(root.clone()));
        Self::with_backend(root, number, backend)
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(root: PathBuf, number: u64, backend: Arc<dyn PrBackend>) -> Self {
        Self {
            root,
            number,
            backend,
            load: None,
            started: false,
            detail: None,
            body: Markdown::default(),
            timeline: Vec::new(),
            error: None,
            tab: PrTab::Conversation,
            diff_load: None,
            diff: None,
            diff_error: None,
            review_body: String::new(),
            action: None,
            action_msg: None,
            merge: None,
            now_override: None,
        }
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn detail(&self) -> Option<&PrDetail> {
        self.detail.as_ref()
    }

    pub fn refresh(&mut self) {
        self.started = false;
        self.diff = None;
        self.diff_error = None;
    }

    pub fn set_tab(&mut self, tab: PrTab) {
        self.tab = tab;
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started
            || self.load.as_mut().is_some_and(|t| t.is_pending())
            || self.diff_load.as_mut().is_some_and(|t| t.is_pending())
            || self.action.as_mut().is_some_and(|(_, t)| t.is_pending())
            || self.diff.as_mut().is_some_and(|d| d.is_loading())
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
        if let Some(d) = &mut self.diff {
            d.set_now(ts);
        }
    }

    fn now(&self) -> i64 {
        self.now_override.unwrap_or_else(now_unix)
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let b = self.backend.clone();
            let n = self.number;
            self.load = Some(Task::spawn(ctx, move || b.view(n)));
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            match r {
                Ok(d) => {
                    self.body = Markdown::parse(&d.body);
                    self.timeline = build_timeline(&d);
                    self.detail = Some(d);
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if self.tab == PrTab::Files && self.diff.is_none() && self.diff_load.is_none() && self.diff_error.is_none() {
            let b = self.backend.clone();
            let n = self.number;
            self.diff_load = Some(Task::spawn(ctx, move || b.diff(n)));
        }
        if let Some(t) = &mut self.diff_load
            && let Some(r) = t.take()
        {
            self.diff_load = None;
            match r {
                Ok(text) => {
                    let mut v = DiffView::from_patch(&self.root, &format!("#{} changes", self.number), text);
                    if let Some(ts) = self.now_override {
                        v.set_now(ts);
                    }
                    self.diff = Some(v);
                }
                Err(e) => self.diff_error = Some(e.to_string()),
            }
        }
        if let Some((kind, t)) = &mut self.action
            && let Some(r) = t.take()
        {
            let kind = *kind;
            self.action = None;
            match r {
                Ok(out) => {
                    let title = match kind {
                        ActionKind::Checkout => "Checked out pull request branch",
                        ActionKind::Review => "Review submitted",
                        ActionKind::Merge => "Pull request merged",
                        ActionKind::Ready => "Marked as ready for review",
                    };
                    if kind == ActionKind::Review {
                        self.review_body.clear();
                    }
                    let detail = out.trim();
                    self.action_msg =
                        Some((BannerKind::Success, title.into(), (!detail.is_empty()).then(|| detail.to_string())));
                    if kind != ActionKind::Checkout {
                        self.started = false;
                    }
                }
                Err(e) => self.action_msg = Some((BannerKind::Error, "Action failed".into(), Some(e.to_string()))),
            }
        }
    }

    fn run(&mut self, ctx: &egui::Context, kind: ActionKind, f: impl FnOnce(&dyn PrBackend) -> GitResult<String> + Send + 'static) {
        let b = self.backend.clone();
        self.action_msg = None;
        self.action = Some((kind, Task::spawn(ctx, move || f(b.as_ref()))));
    }

    /// 뷰를 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            if let Some(e) = self.error.clone() {
                egui::Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    banner(ui, BannerKind::Error, &format!("Could not load pull request #{}", self.number), Some(&e), false);
                    ui.add_space(6.0);
                    if tool_button(ui, Some(Icon::Refresh), "Retry").clicked() {
                        self.refresh();
                    }
                });
                return;
            }
            if self.detail.is_none() {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    spinner(ui, 14.0);
                    ui.label(dim(format!("Loading pull request #{}…", self.number)));
                });
                return;
            }
            self.ui_header(ui);
            ui.add(egui::Separator::default().spacing(0.0));
            match self.tab {
                PrTab::Conversation => {
                    egui::ScrollArea::vertical().id_salt(("pr_conv", self.number)).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Frame::new().inner_margin(Margin { left: 16, right: 16, top: 12, bottom: 16 }).show(ui, |ui| {
                            ui.set_max_width(ui.available_width().min(960.0));
                            self.ui_conversation(ui);
                        });
                    });
                }
                PrTab::Checks => {
                    egui::ScrollArea::vertical().id_salt(("pr_checks", self.number)).auto_shrink([false, false]).show(ui, |ui| {
                        egui::Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| self.ui_checks(ui));
                    });
                }
                PrTab::Files => {
                    if let Some(e) = self.diff_error.clone() {
                        egui::Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                            banner(ui, BannerKind::Error, "Could not load diff", Some(&e), false);
                        });
                    } else if let Some(d) = &mut self.diff {
                        d.ui(ui);
                    } else {
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            spinner(ui, 14.0);
                            ui.label(dim("Loading changes…"));
                        });
                    }
                }
            }
        });
        self.ui_merge_dialog(ui.ctx());
        events
    }

    fn ui_header(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let busy = self.action.is_some();
        let state = d.pr_state();
        egui::Frame::new().fill(t.bg_panel).inner_margin(Margin { left: 16, right: 16, top: 12, bottom: 0 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(RichText::new(&d.title).size(18.0).strong().color(t.text)).wrap());
                ui.label(RichText::new(format!("#{}", d.number)).size(18.0).color(t.text_faint));
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                pr_state_badge(ui, state, d.is_draft);
                ui.add_space(4.0);
                ui.label(RichText::new(&d.author.login).strong().size(12.5).color(t.text));
                let verb = match state {
                    PrState::Merged => "merged",
                    _ => "wants to merge",
                };
                let n = d.commits.len();
                ui.label(dim(format!("{verb} {n} commit{} into", if n == 1 { "" } else { "s" })));
                outline_badge(ui, &d.base_ref_name, t.accent);
                ui.label(dim("from"));
                outline_badge(ui, &d.head_ref_name, t.accent);
                ui.add_space(6.0);
                ui.label(RichText::new(format!("+{}", d.additions)).color(t.green).size(12.0).monospace());
                ui.label(RichText::new(format!("−{}", d.deletions)).color(t.red).size(12.0).monospace());
                for l in &d.labels {
                    let c = Color32::from_hex(&format!("#{}", l.color)).unwrap_or(t.text_dim);
                    badge(ui, &l.name, readable_on(c), c);
                }
            });
            ui.add_space(8.0);
            // 동작 버튼
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_enabled_ui(!busy, |ui| {
                    if tool_button(ui, Some(Icon::Download), "Checkout").on_hover_text("gh pr checkout").clicked() {
                        let n = d.number;
                        self.run(ui.ctx(), ActionKind::Checkout, move |b| b.checkout(n));
                    }
                    if tool_button(ui, Some(Icon::External), "Open in Browser").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&d.url));
                    }
                    if state == PrState::Open && d.is_draft && tool_button(ui, Some(Icon::Check), "Ready for Review").clicked() {
                        let n = d.number;
                        self.run(ui.ctx(), ActionKind::Ready, move |b| b.mark_ready(n).map(|_| String::new()));
                    }
                });
                if busy {
                    spinner(ui, 12.0);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if state == PrState::Open {
                        let enabled = !busy && !d.is_draft;
                        let r = ui.add_enabled_ui(enabled, |ui| primary_button(ui, "Merge…", None)).inner;
                        let r = if d.is_draft { r.on_disabled_hover_text("Draft pull requests cannot be merged") } else { r };
                        if r.clicked() {
                            self.merge = Some(MergeDialog { method: MergeMethod::Squash, delete_branch: true });
                        }
                    }
                    if icon_button(ui, Icon::Refresh, "Refresh").clicked() {
                        self.refresh();
                    }
                    if let Some(c) = d.checks() {
                        let (icon, col) = checks_icon(c);
                        let label = match c {
                            ChecksState::Pass => "Checks passing",
                            ChecksState::Fail => "Checks failing",
                            ChecksState::Pending => "Checks pending",
                        };
                        ui.label(RichText::new(label).size(12.0).color(col));
                        icon_label(ui, icon, col, 14.0);
                    }
                    if let Some(r) = d.review() {
                        ui.label(RichText::new(r.label()).size(12.0).color(review_color(r)));
                    }
                });
            });
            if let Some((k, title, detail)) = self.action_msg.clone() {
                ui.add_space(6.0);
                if banner(ui, k, &title, detail.as_deref(), true) {
                    self.action_msg = None;
                }
            }
            ui.add_space(8.0);
            // 탭
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                let comments = d.comments.len() + d.reviews.iter().filter(|r| !r.body.trim().is_empty()).count();
                let tabs = [
                    (PrTab::Conversation, "Conversation", comments),
                    (PrTab::Checks, "Checks", d.status_check_rollup.len()),
                    (PrTab::Files, "Files changed", d.changed_files as usize),
                ];
                for (tab, label, count) in tabs {
                    let sel = self.tab == tab;
                    let text = format!("{label}  {count}");
                    let g = ui.painter().layout_no_wrap(text, FontId::proportional(13.0), if sel { t.text } else { t.text_dim });
                    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x, 30.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, label));
                    let c = if sel || resp.hovered() { t.text } else { t.text_dim };
                    ui.painter().galley_with_override_text_color(pos_center(rect, g.size()), g, c);
                    if sel {
                        ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(2.0, t.orange));
                    }
                    if resp.clicked() {
                        self.tab = tab;
                    }
                }
            });
        });
    }

    fn ui_conversation(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let now = self.now();
        // 설명
        comment_card(ui, &d.author.login, parse_iso8601(&d.created_at).unwrap_or(0), now, "opened this pull request", t.accent, |ui| {
            if self.body.is_empty() {
                ui.label(faint("No description provided."));
            } else {
                self.body.show(ui);
            }
        });
        for item in &self.timeline {
            ui.add_space(10.0);
            let accent = match item.kind.as_str() {
                "approved" => t.green,
                "requested changes" => t.red,
                _ => t.border,
            };
            comment_card(ui, &item.author, item.when, now, &item.kind, accent, |ui| {
                if item.body.is_empty() {
                    ui.label(faint("No comment."));
                } else {
                    item.body.show(ui);
                }
            });
        }
        // 리뷰 입력
        if d.pr_state() == PrState::Open {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(t.bg_panel)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Add your review").size(13.0).strong().color(t.text));
                    ui.add_space(4.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut self.review_body)
                            .hint_text("Leave a comment (Markdown supported)")
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .frame(input_frame()),
                    );
                    ui.add_space(6.0);
                    let busy = self.action.is_some();
                    let has_body = !self.review_body.trim().is_empty();
                    ui.horizontal(|ui| {
                        let n = d.number;
                        if ui.add_enabled_ui(!busy && has_body, |ui| secondary_button(ui, "Comment")).inner.clicked() {
                            let body = self.review_body.clone();
                            self.run(ui.ctx(), ActionKind::Review, move |b| b.review(n, ReviewKind::Comment, &body).map(|_| String::new()));
                        }
                        if ui
                            .add_enabled_ui(!busy && has_body, |ui| secondary_button(ui, "Request Changes"))
                            .inner
                            .on_disabled_hover_text("Write a comment explaining the requested changes")
                            .clicked()
                        {
                            let body = self.review_body.clone();
                            self.run(ui.ctx(), ActionKind::Review, move |b| {
                                b.review(n, ReviewKind::RequestChanges, &body).map(|_| String::new())
                            });
                        }
                        if ui.add_enabled_ui(!busy, |ui| primary_button(ui, "Approve", None)).inner.clicked() {
                            let body = self.review_body.clone();
                            self.run(ui.ctx(), ActionKind::Review, move |b| b.review(n, ReviewKind::Approve, &body).map(|_| String::new()));
                        }
                    });
                });
        }
    }

    fn ui_checks(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = &self.detail else { return };
        if d.status_check_rollup.is_empty() {
            empty_state(ui, "No checks", "This pull request has no status checks.");
            return;
        }
        let mut items: Vec<_> = d.status_check_rollup.iter().collect();
        items.sort_by_key(|c| match c.outcome() {
            Some(ChecksState::Fail) => 0,
            Some(ChecksState::Pending) => 1,
            Some(ChecksState::Pass) => 2,
            None => 3,
        });
        let (mut pass, mut fail, mut pend, mut skip) = (0, 0, 0, 0);
        for c in &items {
            match c.outcome() {
                Some(ChecksState::Pass) => pass += 1,
                Some(ChecksState::Fail) => fail += 1,
                Some(ChecksState::Pending) => pend += 1,
                None => skip += 1,
            }
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} checks", items.len())).size(14.0).strong().color(t.text));
            ui.add_space(8.0);
            if fail > 0 {
                icon_label(ui, Icon::XCircle, t.red, 13.0);
                ui.label(RichText::new(format!("{fail} failing")).color(t.red).size(12.0));
            }
            if pend > 0 {
                icon_label(ui, Icon::PendingCircle, t.yellow, 13.0);
                ui.label(RichText::new(format!("{pend} pending")).color(t.yellow).size(12.0));
            }
            icon_label(ui, Icon::CheckCircle, t.green, 13.0);
            ui.label(RichText::new(format!("{pass} passing")).color(t.green).size(12.0));
            if skip > 0 {
                ui.label(RichText::new(format!("{skip} skipped")).color(t.text_faint).size(12.0));
            }
        });
        ui.add_space(8.0);
        egui::Frame::new()
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(6))
            .fill(t.bg_panel)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, c) in items.iter().enumerate() {
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
                    }
                    if i > 0 {
                        ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, t.border));
                    }
                    let (icon, col) = match c.outcome() {
                        Some(s) => checks_icon(s),
                        None => (Icon::Skip, t.text_faint),
                    };
                    let p = ui.painter();
                    paint_icon(p, egui::Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(15.0, 15.0)), icon, col);
                    let dur = match (c.started_at.as_deref().and_then(parse_iso8601), c.completed_at.as_deref().and_then(parse_iso8601)) {
                        (Some(s), Some(e)) if e >= s => format!(" · {}", fmt_duration(e - s)),
                        _ => String::new(),
                    };
                    let wf = c.workflow_name.clone().filter(|s| !s.is_empty()).map(|w| format!("{w} / ")).unwrap_or_default();
                    let label = format!("{wf}{}", c.display_name());
                    let status = format!("  {}{dur}", c.outcome_label());
                    let job = super::panel::one_line_job(&[(&label, 12.5, t.text), (&status, 11.5, t.text_faint)], w - 110.0);
                    let gal = p.layout_job(job);
                    p.galley(egui::pos2(rect.left() + 32.0, rect.center().y - gal.size().y / 2.0), gal, t.text);
                    if let Some(url) = c.url() {
                        p.text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, "Details", FontId::proportional(12.0), t.accent);
                        if resp.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        resp.on_hover_text(url);
                    }
                }
            });
    }

    fn ui_merge_dialog(&mut self, ctx: &egui::Context) {
        let Some(m) = &mut self.merge else { return };
        let Some(d) = &self.detail else { return };
        let t = theme();
        let mut method = m.method;
        let mut delete = m.delete_branch;
        let head = d.head_ref_name.clone();
        let msg = format!("Merge #{} \"{}\" into {}.", d.number, d.title, d.base_ref_name);
        let r = confirm_modal(ctx, Id::new(("pr_merge", self.number)), "Merge pull request", &msg, "Confirm Merge", false, |ui| {
            ui.add_space(10.0);
            for mm in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
                if radio_row(ui, method == mm, mm.label()) {
                    method = mm;
                }
            }
            ui.add_space(6.0);
            checkbox_row(ui, &mut delete, &format!("Delete branch \"{head}\" after merge"));
            if d.mergeable == "CONFLICTING" {
                ui.add_space(6.0);
                ui.label(RichText::new("⚠ This branch has conflicts that must be resolved").color(t.orange).size(12.0));
            }
        });
        m.method = method;
        m.delete_branch = delete;
        match r {
            Some(true) => {
                self.merge = None;
                let n = self.number;
                self.run(ctx, ActionKind::Merge, move |b| b.merge(n, method, delete));
            }
            Some(false) => self.merge = None,
            None => {}
        }
    }
}

fn pos_center(rect: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    rect.center() - size / 2.0
}

fn readable_on(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 150.0 { Color32::from_rgb(0x1a, 0x1a, 0x1a) } else { Color32::WHITE }
}

fn fmt_duration(s: i64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

fn build_timeline(d: &PrDetail) -> Vec<TimelineItem> {
    let mut v: Vec<TimelineItem> = Vec::new();
    for c in &d.comments {
        v.push(TimelineItem {
            author: c.author.login.clone(),
            when: parse_iso8601(&c.created_at).unwrap_or(0),
            kind: "commented".into(),
            body: Markdown::parse(&c.body),
        });
    }
    for r in &d.reviews {
        let kind = match r.state.as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "requested changes",
            "DISMISSED" => "review dismissed",
            _ => "reviewed",
        };
        if r.body.trim().is_empty() && kind == "reviewed" {
            continue;
        }
        v.push(TimelineItem {
            author: r.author.login.clone(),
            when: parse_iso8601(&r.submitted_at).unwrap_or(0),
            kind: kind.into(),
            body: Markdown::parse(&r.body),
        });
    }
    v.sort_by_key(|i| i.when);
    v
}

/// 작성자/시간 헤더가 있는 코멘트 카드.
fn comment_card(ui: &mut Ui, author: &str, when: i64, now: i64, verb: &str, accent: Color32, body: impl FnOnce(&mut Ui)) {
    let t = theme();
    egui::Frame::new()
        .fill(t.bg_panel)
        .stroke(Stroke::new(1.0, if accent == t.border { t.border } else { alpha(accent, 0.6) }))
        .corner_radius(CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .fill(t.bg_elevated)
                .corner_radius(CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let initial = author.chars().next().unwrap_or('?').to_uppercase().to_string();
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 10.0, alpha(t.accent, 0.3));
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, FontId::proportional(11.0), t.text);
                        ui.label(RichText::new(author).strong().size(12.5).color(t.text));
                        let vc = if accent == t.border { t.text_dim } else { accent };
                        ui.label(RichText::new(verb).size(12.0).color(vc));
                        if when > 0 {
                            ui.label(faint(relative_time(when, now)));
                        }
                    });
                });
            egui::Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
        });
}
