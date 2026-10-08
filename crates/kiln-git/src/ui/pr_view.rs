//! PR 상세 뷰(PrView): 헤더, 본문, 체크, 타임라인, 변경 파일, 리뷰/병합 동작.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Align2, Color32, CornerRadius, Id, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
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
    submitted_body: Option<String>,
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

    /// 지정 저장소(`-R owner/name`)의 PR 뷰. `repo` 가 `None` 이면 `new` 와 같다.
    pub fn for_repo(root: PathBuf, repo: Option<crate::github::RepoRef>, number: u64) -> Self {
        let backend = Arc::new(GhBackend::for_repo(root.clone(), repo));
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
            submitted_body: None,
            action: None,
            action_msg: None,
            merge: None,
            now_override: None,
        }
    }

    pub fn review_draft(&self) -> &str { &self.review_body }

    pub fn restore_review_draft(&mut self, body: &str) { self.review_body = body.to_owned(); }

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
                    let mut v = DiffView::from_patch(&self.root, &kiln_common::trf!("#{} 변경 사항", self.number), text);
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
                        ActionKind::Checkout => kiln_common::i18n::tr("풀 리퀘스트 브랜치를 체크아웃했습니다"),
                        ActionKind::Review => kiln_common::i18n::tr("리뷰를 제출했습니다"),
                        ActionKind::Merge => kiln_common::i18n::tr("풀 리퀘스트를 병합했습니다"),
                        ActionKind::Ready => kiln_common::i18n::tr("리뷰 준비 완료로 표시했습니다"),
                    };
                    if kind == ActionKind::Review && self.submitted_body.take().as_deref() == Some(self.review_body.as_str()) {
                        self.review_body.clear();
                    }
                    let detail = out.trim();
                    self.action_msg =
                        Some((BannerKind::Success, title.into(), (!detail.is_empty()).then(|| detail.to_string())));
                    if kind != ActionKind::Checkout {
                        self.started = false;
                    }
                }
                Err(e) => self.action_msg = Some((BannerKind::Error, match kind { ActionKind::Checkout => kiln_common::i18n::tr("브랜치 체크아웃 실패"), ActionKind::Review => kiln_common::i18n::tr("리뷰 제출 실패"), ActionKind::Merge => kiln_common::i18n::tr("풀 리퀘스트 병합 실패"), ActionKind::Ready => kiln_common::i18n::tr("리뷰 준비 상태 변경 실패") }.into(), Some(e.to_string()))),
            }
        }
    }

    fn run(&mut self, ctx: &egui::Context, kind: ActionKind, f: impl FnOnce(&dyn PrBackend) -> GitResult<String> + Send + 'static) {
        let b = self.backend.clone();
        self.submitted_body = (kind == ActionKind::Review).then(|| self.review_body.clone());
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
                    banner(ui, BannerKind::Error, &kiln_common::trf!("풀 리퀘스트 #{} 불러오기 실패", self.number), Some(&e), false);
                    ui.add_space(6.0);
                    if tool_button(ui, Some(Icon::Refresh), kiln_common::i18n::tr("다시 시도")).clicked() {
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
                    ui.label(dim(kiln_common::trf!("풀 리퀘스트 #{} 불러오는 중…", self.number)));
                });
                return;
            }
            self.ui_header(ui);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, t.border);
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
                            banner(ui, BannerKind::Error, kiln_common::i18n::tr("변경 비교를 불러올 수 없습니다"), Some(&e), false);
                        });
                    } else if let Some(d) = &mut self.diff {
                        d.ui(ui);
                    } else {
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            spinner(ui, 14.0);
                            ui.label(dim(kiln_common::i18n::tr("변경 사항 불러오는 중…")));
                        });
                    }
                }
            }
        });
        self.ui_merge_dialog(ui.ctx(), ui.make_persistent_id(("pr_merge", &self.root, self.number)));
        events
    }

    fn ui_header(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let busy = self.action.is_some();
        let state = d.pr_state();
        egui::Frame::new().fill(t.bg).inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 0 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(RichText::new(&d.title).font(kiln_common::fonts::semibold(20.0)).color(t.text)).wrap());
                ui.label(RichText::new(format!("#{}", d.number)).size(20.0).color(t.text_faint));
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                pr_state_badge(ui, state, d.is_draft);
                ui.add_space(4.0);
                ui.label(RichText::new(&d.author.login).font(kiln_common::fonts::medium(13.0)).color(t.text));
                let n = d.commits.len();
                outline_badge(ui, &d.head_ref_name, t.accent);
                ui.label(dim("→"));
                outline_badge(ui, &d.base_ref_name, t.accent);
                ui.label(dim(kiln_common::trf!("{n}개 커밋")));
                ui.add_space(6.0);
                ui.label(RichText::new(format!("+{}", d.additions)).color(t.green).size(12.0).monospace());
                ui.label(RichText::new(format!("−{}", d.deletions)).color(t.red).size(12.0).monospace());
                ui.add_space(4.0);
                for l in &d.labels {
                    let c = Color32::from_hex(&format!("#{}", l.color)).unwrap_or(t.text_dim);
                    badge(ui, &l.name, readable_on(c), c);
                }
            });
            ui.add_space(12.0);
            // 동작 버튼
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_enabled_ui(!busy, |ui| {
                    if tool_button(ui, Some(Icon::Download), kiln_common::i18n::tr("체크아웃")).on_hover_text("gh pr checkout").clicked() {
                        let n = d.number;
                        self.run(ui.ctx(), ActionKind::Checkout, move |b| b.checkout(n));
                    }
                    if tool_button(ui, Some(Icon::External), kiln_common::i18n::tr("브라우저에서 열기")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&d.url));
                    }
                    if state == PrState::Open && d.is_draft && tool_button(ui, Some(Icon::Check), kiln_common::i18n::tr("리뷰 준비 완료")).clicked() {
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
                        let r = ui.add_enabled_ui(enabled, |ui| primary_button(ui, kiln_common::i18n::tr("병합…"), None)).inner;
                        let r = if d.is_draft { r.on_disabled_hover_text(kiln_common::i18n::tr("초안 풀 리퀘스트는 병합할 수 없습니다")) } else { r };
                        if r.clicked() {
                            self.merge = Some(MergeDialog { method: MergeMethod::Squash, delete_branch: true });
                        }
                    }
                    if icon_button(ui, Icon::Refresh, kiln_common::i18n::tr("새로 고침")).clicked() {
                        self.refresh();
                    }
                    if let Some(c) = d.checks() {
                        let (icon, col) = checks_icon(c);
                        let label = match c {
                            ChecksState::Pass => kiln_common::i18n::tr("검사 통과"),
                            ChecksState::Fail => kiln_common::i18n::tr("검사 실패"),
                            ChecksState::Pending => kiln_common::i18n::tr("검사 진행 중"),
                        };
                        ui.label(RichText::new(label).font(kiln_common::fonts::medium(12.5)).color(col));
                        icon_label(ui, icon, col, 14.0);
                    }
                    if let Some(r) = d.review() {
                        outline_badge(ui, r.label(), review_color(r));
                    }
                });
            });
            if let Some((k, title, detail)) = self.action_msg.clone() {
                ui.add_space(6.0);
                if banner(ui, k, &title, detail.as_deref(), true) {
                    self.action_msg = None;
                }
            }
            ui.add_space(10.0);
            // 탭
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                let comments = d.comments.len() + d.reviews.iter().filter(|r| !r.body.trim().is_empty()).count();
                let tabs = [
                    (PrTab::Conversation, kiln_common::i18n::tr("대화"), comments),
                    (PrTab::Checks, kiln_common::i18n::tr("검사"), d.status_check_rollup.len()),
                    (PrTab::Files, kiln_common::i18n::tr("변경된 파일"), d.changed_files as usize),
                ];
                for (tab, label, count) in tabs {
                    let sel = self.tab == tab;
                    let g = ui.painter().layout_no_wrap(label.to_string(), kiln_common::fonts::medium(13.0), t.text);
                    let cg = ui.painter().layout_no_wrap(count.to_string(), kiln_common::fonts::medium(11.0), t.text_dim);
                    let pill_w = (cg.size().x + 12.0).max(20.0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 6.0 + pill_w, 36.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, label));
                    let c = if sel || resp.hovered() { t.text } else { t.text_dim };
                    let ty = rect.center().y - 1.0;
                    ui.painter().galley_with_override_text_color(egui::pos2(rect.left(), ty - g.size().y / 2.0), g.clone(), c);
                    let pr = egui::Rect::from_min_size(egui::pos2(rect.left() + g.size().x + 6.0, ty - 9.0), vec2(pill_w, 18.0));
                    ui.painter().rect_filled(pr, CornerRadius::same(9), if sel { alpha(t.accent, 0.16) } else { t.bg_hover });
                    ui.painter().galley_with_override_text_color(pos_center(pr, cg.size()), cg, if sel { t.accent } else { t.text_dim });
                    if sel {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom()),
                            CornerRadius::same(1),
                            t.accent,
                        );
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
        comment_card(ui, &d.author.login, parse_iso8601(&d.created_at).unwrap_or(0), now, kiln_common::i18n::tr("님이 이 풀 리퀘스트를 열었습니다"), t.accent, |ui| {
            if self.body.is_empty() {
                ui.label(faint(kiln_common::i18n::tr("설명이 없습니다.")));
            } else {
                self.body.show(ui);
            }
        });
        for item in &self.timeline {
            ui.add_space(10.0);
            let accent = match item.kind.as_str() {
                "님이 승인했습니다" => t.green,
                "님이 변경을 요청했습니다" => t.red,
                _ => t.border,
            };
            comment_card(ui, &item.author, item.when, now, kiln_common::i18n::tr(&item.kind), accent, |ui| {
                if item.body.is_empty() {
                    ui.label(faint(kiln_common::i18n::tr("댓글이 없습니다.")));
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
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(kiln_common::i18n::tr("리뷰 작성")).font(kiln_common::fonts::semibold(13.5)).color(t.text));
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut self.review_body)
                            .hint_text(kiln_common::i18n::tr("댓글 남기기 (Markdown 지원)"))
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .frame(input_frame()),
                    );
                    ui.add_space(10.0);
                    let busy = self.action.is_some();
                    let has_body = !self.review_body.trim().is_empty();
                    ui.horizontal(|ui| {
                        let n = d.number;
                        if ui.add_enabled_ui(!busy && has_body, |ui| secondary_button(ui, kiln_common::i18n::tr("댓글"))).inner.clicked() {
                            let body = self.review_body.clone();
                            self.run(ui.ctx(), ActionKind::Review, move |b| b.review(n, ReviewKind::Comment, &body).map(|_| String::new()));
                        }
                        if ui
                            .add_enabled_ui(!busy && has_body, |ui| secondary_button(ui, kiln_common::i18n::tr("변경 요청")))
                            .inner
                            .on_disabled_hover_text(kiln_common::i18n::tr("요청하는 변경 사항을 설명하는 댓글을 작성하세요"))
                            .clicked()
                        {
                            let body = self.review_body.clone();
                            self.run(ui.ctx(), ActionKind::Review, move |b| {
                                b.review(n, ReviewKind::RequestChanges, &body).map(|_| String::new())
                            });
                        }
                        if ui.add_enabled_ui(!busy, |ui| primary_button(ui, kiln_common::i18n::tr("승인"), None)).inner.clicked() {
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
            empty_state(ui, kiln_common::i18n::tr("검사 없음"), kiln_common::i18n::tr("이 풀 리퀘스트에는 상태 검사가 없습니다."));
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
            ui.label(RichText::new(kiln_common::trf!("검사 {}개", items.len())).font(kiln_common::fonts::semibold(14.0)).color(t.text));
            ui.add_space(8.0);
            if fail > 0 {
                icon_label(ui, Icon::XCircle, t.red, 13.0);
                ui.label(RichText::new(kiln_common::trf!("실패 {fail}")).color(t.red).size(12.0));
            }
            if pend > 0 {
                icon_label(ui, Icon::PendingCircle, t.yellow, 13.0);
                ui.label(RichText::new(kiln_common::trf!("진행 중 {pend}")).color(t.yellow).size(12.0));
            }
            icon_label(ui, Icon::CheckCircle, t.green, 13.0);
            ui.label(RichText::new(kiln_common::trf!("통과 {pass}")).color(t.green).size(12.0));
            if skip > 0 {
                ui.label(RichText::new(kiln_common::trf!("건너뜀 {skip}")).color(t.text_faint).size(12.0));
            }
        });
        ui.add_space(8.0);
        egui::Frame::new()
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(10))
            .fill(t.bg_panel)
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, c) in items.iter().enumerate() {
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
                    if i > 0 && !resp.hovered() {
                        ui.painter().hline(rect.shrink2(vec2(10.0, 0.0)).x_range(), rect.top(), Stroke::new(1.0, t.border));
                    }
                    kiln_common::widgets::paint_row(ui.painter(), rect, false, resp.hovered());
                    let (icon, col) = match c.outcome() {
                        Some(s) => checks_icon(s),
                        None => (Icon::Skip, t.text_faint),
                    };
                    let p = ui.painter();
                    paint_icon(p, egui::Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), vec2(16.0, 16.0)), icon, col);
                    let dur = match (c.started_at.as_deref().and_then(parse_iso8601), c.completed_at.as_deref().and_then(parse_iso8601)) {
                        (Some(s), Some(e)) if e >= s => format!(" · {}", fmt_duration(e - s)),
                        _ => String::new(),
                    };
                    let wf = c.workflow_name.clone().filter(|s| !s.is_empty()).map(|w| format!("{w} / ")).unwrap_or_default();
                    let label = format!("{wf}{}", c.display_name());
                    let status = format!("  {}{dur}", c.outcome_label());
                    let job = super::panel::one_line_job(&[(&label, 13.0, t.text), (&status, 12.0, t.text_faint)], w - 120.0);
                    let gal = p.layout_job(job);
                    p.galley(egui::pos2(rect.left() + 36.0, rect.center().y - gal.size().y / 2.0), gal, t.text);
                    if let Some(url) = c.url() {
                        p.text(rect.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, kiln_common::i18n::tr("세부 정보"), kiln_common::fonts::medium(12.5), t.accent);
                        if resp.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        resp.on_hover_text(url);
                    }
                }
            });
    }

    fn ui_merge_dialog(&mut self, ctx: &egui::Context, id: Id) {
        let Some(m) = &mut self.merge else { return };
        let Some(d) = &self.detail else { return };
        let t = theme();
        let mut method = m.method;
        let mut delete = m.delete_branch;
        let head = d.head_ref_name.clone();
        let msg = kiln_common::trf!("풀 리퀘스트 #{} · {}\n대상 브랜치: {}", d.number, d.title, d.base_ref_name);
        let r = confirm_modal(ctx, id, kiln_common::i18n::tr("풀 리퀘스트 병합"), &msg, kiln_common::i18n::tr("병합 확인"), false, |ui| {
            ui.add_space(10.0);
            for mm in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
                if radio_row(ui, method == mm, mm.label()) {
                    method = mm;
                }
            }
            ui.add_space(6.0);
            checkbox_row(ui, &mut delete, &kiln_common::trf!("병합 후 \"{head}\" 브랜치 삭제"));
            if d.mergeable == "CONFLICTING" {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    icon_label(ui, Icon::Warning, t.orange, 14.0);
                    ui.label(RichText::new(kiln_common::i18n::tr("이 브랜치에 해결해야 할 충돌이 있습니다")).color(t.orange).size(12.5));
                });
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

pub(crate) fn readable_on(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 150.0 { kiln_common::Theme::KILN_LIGHT.text } else { kiln_common::Theme::KILN_DARK.text }
}

fn fmt_duration(s: i64) -> String {
    if s < 60 {
        kiln_common::trf!("{s}초")
    } else if s < 3600 {
        kiln_common::trf!("{}분 {}초", s / 60, s % 60)
    } else {
        kiln_common::trf!("{}시간 {}분", s / 3600, (s % 3600) / 60)
    }
}

fn build_timeline(d: &PrDetail) -> Vec<TimelineItem> {
    let mut v: Vec<TimelineItem> = Vec::new();
    for c in &d.comments {
        v.push(TimelineItem {
            author: c.author.login.clone(),
            when: parse_iso8601(&c.created_at).unwrap_or(0),
            kind: "님이 댓글을 남겼습니다".into(),
            body: Markdown::parse(&c.body),
        });
    }
    for r in &d.reviews {
        let kind = match r.state.as_str() {
            "APPROVED" => "님이 승인했습니다",
            "CHANGES_REQUESTED" => "님이 변경을 요청했습니다",
            "DISMISSED" => "님의 리뷰가 취소되었습니다",
            _ => "님이 리뷰했습니다",
        };
        if r.body.trim().is_empty() && kind == "님이 리뷰했습니다" {
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
pub(crate) fn comment_card(ui: &mut Ui, author: &str, when: i64, now: i64, verb: &str, accent: Color32, body: impl FnOnce(&mut Ui)) {
    let t = theme();
    egui::Frame::new()
        .fill(if t.dark { t.bg_panel } else { t.bg })
        .stroke(Stroke::new(1.0, if accent == t.border { t.border } else { alpha(accent, 0.5) }))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .fill(if t.dark { t.bg_elevated } else { t.bg_panel })
                .corner_radius(CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 })
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let initial = author.chars().next().unwrap_or('?').to_uppercase().to_string();
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        let ac = kiln_common::widgets::hue_color(author);
                        ui.painter().circle_filled(r.center(), 10.0, alpha(ac, if t.dark { 0.28 } else { 0.2 }));
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, kiln_common::fonts::semibold(11.0), t.text);
                        ui.label(RichText::new(author).font(kiln_common::fonts::medium(13.0)).color(t.text));
                        let vc = if accent == t.border { t.text_dim } else { accent };
                        ui.label(RichText::new(verb).size(12.0).color(vc));
                        if when > 0 {
                            ui.label(faint(relative_time(when, now)));
                        }
                    });
                });
            egui::Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
        });
}

#[cfg(test)]
mod pending_draft_tests {
    use super::*;
    #[test]
    fn successful_delayed_submission_preserves_newer_text() {
        for edit_while_sending in [false,true] {
            let ctx=egui::Context::default(); let mut view=PrView::new(PathBuf::from("."), 1);
            view.started=true;view.review_body="submitted body".into();
            let (tx,rx)=std::sync::mpsc::channel();
            view.run(&ctx,ActionKind::Review,move |_|{rx.recv().unwrap();Ok(String::new())});
            if edit_while_sending {view.review_body="submitted body plus next draft".into();}
            tx.send(()).unwrap();
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
            while view.action.is_some() && std::time::Instant::now()<deadline {view.pump(&ctx);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert!(view.action.is_none());
            assert_eq!(view.review_body,if edit_while_sending {"submitted body plus next draft"}else{""});
        }
    }
}
