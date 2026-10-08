//! PR 목록 패널(PrPanel)과 PR 생성 폼.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Color32, CornerRadius, FontId, Id, Layout, Margin, RichText, Sense, Ui, pos2, vec2};
use kiln_common::Task;

use super::panel::one_line_job;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::{GhBackend, PrBackend, PrCreate, PrCreateDefaults, PrFilter, PrItem};
use crate::util::{now_unix, parse_iso8601, short_relative_time};

/// Local-only creation draft. Restoring never submits a request.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrCreationDraft {
    pub request: PrCreate,
    pub head: String,
    pub bases: Vec<String>,
}

const PR_ROW_H: f32 = 50.0;

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
    form_open: bool,
    discard_confirm: bool,
    created: Option<String>,
    embedded: bool,
    can_create: bool,
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
            form_open: false,
            discard_confirm: false,
            created: None,
            embedded: false,
            can_create: true,
            now_override: None,
        }
    }

    /// 지정 저장소(`-R owner/name`)를 대상으로 하는 패널. `repo` 가 `None` 이면 `new` 와 같다.
    pub fn for_repo(root: PathBuf, repo: Option<crate::github::RepoRef>) -> Self {
        Self::with_backend(Arc::new(GhBackend::for_repo(root, repo)))
    }

    /// 허브 안에 넣을 때 제목을 숨긴다.
    pub fn set_embedded(&mut self, on: bool) {
        self.embedded = on;
    }

    /// "새 PR" 버튼 표시 여부. 작업 폴더와 다른 저장소를 볼 때 끈다.
    pub fn set_can_create(&mut self, on: bool) {
        self.can_create = on;
        if !on { self.form_open = false; }
    }

    pub fn is_submitting(&self) -> bool { self.form.as_ref().is_some_and(|f| f.submit.is_some()) }

    pub fn creation_draft(&self) -> Option<PrCreationDraft> {
        self.form.as_ref().filter(|f| f.req != PrCreate::default()).map(|f| PrCreationDraft {
            request: f.req.clone(), head: f.head.clone(), bases: f.bases.clone(),
        })
    }

    pub fn restore_creation_draft(&mut self, draft: &PrCreationDraft) {
        self.form = Some(CreateForm { defaults: None, head: draft.head.clone(), bases: draft.bases.clone(),
            req: draft.request.clone(), submit: None, error: None });
        self.form_open = true;
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
        self.form_open = true;
        self.discard_confirm = false;
        if self.form.is_some() { return; }
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
                        if form.req.base.is_empty() { form.req.base = d.base; }
                        if form.req.title.is_empty() { form.req.title = d.title; }
                        if form.req.body.is_empty() { form.req.body = d.body; }
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
                        self.form_open = false;
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
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.embedded {
                        if self.started && self.load.is_none() && self.error.is_none() {
                            ui.label(faint(kiln_common::trf!("풀 리퀘스트 {}개", self.items.len())));
                        }
                    } else {
                        ui.label(RichText::new(kiln_common::i18n::tr("풀 리퀘스트")).font(kiln_common::fonts::semibold(13.5)).color(t.text));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.form_open
                            && self.can_create
                            && kiln_common::widgets::button_with(
                                ui,
                                Some(kiln_common::icons::Icon::Plus),
                                if self.form.is_some() { kiln_common::i18n::tr("초안 이어 쓰기") } else { kiln_common::i18n::tr("새 PR") },
                                kiln_common::widgets::ButtonKind::Primary,
                                true,
                            )
                            .on_hover_text(kiln_common::i18n::tr("풀 리퀘스트 새로 만들기"))
                            .clicked()
                        {
                            self.open_create_form();
                        }
                        if self.load.is_some() {
                            spinner(ui, 12.0);
                        } else if icon_button(ui, Icon::Refresh, kiln_common::i18n::tr("새로 고침")).clicked() {
                            self.refresh();
                        }
                    });
                });
                ui.add_space(8.0);
                let mut f = self.filter;
                let opts: Vec<(PrFilter, &str)> = PrFilter::ALL.iter().map(|f| (*f, f.label())).collect();
                if segmented(ui, &mut f, &opts) {
                    self.set_filter(f);
                }
                ui.add_space(8.0);
                let sid = ui.make_persistent_id("kiln_pr_search");
                let focused = ui.memory(|m| m.has_focus(sid));
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(sid)
                        .hint_text(kiln_common::i18n::tr("제목, 작성자, 브랜치, #번호로 필터"))
                        .desired_width(f32::INFINITY)
                        .frame(kiln_common::widgets::input_frame(focused, false)),
                );
                if let Some(url) = self.created.clone() {
                    ui.add_space(4.0);
                    if banner(ui, BannerKind::Success, kiln_common::i18n::tr("풀 리퀘스트를 만들었습니다"), Some(&url), true) {
                        self.created = None;
                    }
                }
            });
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line.shrink2(vec2(12.0, 0.0)), 0.0, t.border);
            ui.add_space(4.0);

            if self.form.is_some() && self.form_open {
                egui::ScrollArea::vertical().id_salt("pr_form").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| self.ui_form(ui));
                });
                return;
            }
            self.ui_list(ui, &mut events);
        });
        events
    }

    fn ui_list(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let t = theme();
        if let Some(e) = self.error.clone() {
            if super::gh_widgets::gh_error_state(ui, &e, kiln_common::i18n::tr("풀 리퀘스트"), events) == super::gh_widgets::ErrorAction::Retry {
                self.refresh();
            }
            return;
        }
        if self.load.is_some() && self.items.is_empty() {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                spinner(ui, 14.0);
                ui.label(dim(kiln_common::i18n::tr("풀 리퀘스트 불러오는 중…")));
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
                PrFilter::Mine => kiln_common::i18n::tr("열려 있는 내 풀 리퀘스트가 없습니다"),
                PrFilter::ReviewRequested => kiln_common::i18n::tr("나에게 요청된 리뷰가 없습니다"),
                _ => kiln_common::i18n::tr("풀 리퀘스트 없음"),
            };
            empty_state(ui, msg, if q.is_empty() { "" } else { kiln_common::i18n::tr("다른 필터를 사용해 보세요.") });
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
                    kiln_common::widgets::paint_row(ui.painter(), rect.shrink2(vec2(6.0, 1.0)), false, resp.hovered());
                    let pr_state = p.pr_state();
                    let sc = pr_state_color(pr_state, p.is_draft);
                    let painter = ui.painter();
                    // 상태 아이콘
                    paint_icon(painter, egui::Rect::from_center_size(rect.left_top() + vec2(22.0, 16.0), vec2(15.0, 15.0)), Icon::PullRequest, sc);
                    // 오른쪽 배지들
                    let mut rx = rect.right() - 16.0;
                    let y1 = rect.top() + 16.0;
                    if let Some(c) = p.checks() {
                        let (icon, c) = checks_icon(c);
                        let r = egui::Rect::from_center_size(pos2(rx - 7.0, y1), vec2(14.0, 14.0));
                        paint_icon(painter, r, icon, c);
                        rx = r.left() - 8.0;
                    }
                    let mut chip = |label: &str, fg: Color32| {
                        let g = painter.layout_no_wrap(label.to_string(), kiln_common::fonts::medium(11.0), fg);
                        let br = egui::Rect::from_min_size(pos2(rx - g.size().x - 12.0, y1 - 9.0), vec2(g.size().x + 12.0, 18.0));
                        painter.rect_filled(br, CornerRadius::same(9), alpha(fg, if t.dark { 0.16 } else { 0.12 }));
                        painter.galley(br.center() - g.size() / 2.0, g, fg);
                        rx = br.left() - 5.0;
                    };
                    if p.is_draft {
                        chip(kiln_common::i18n::tr("초안"), t.text_dim);
                    }
                    if let Some(r) = p.review() {
                        chip(r.label(), review_color(r));
                    }
                    if pr_state != crate::gh::PrState::Open {
                        chip(if pr_state == crate::gh::PrState::Merged { kiln_common::i18n::tr("병합됨") } else { kiln_common::i18n::tr("닫힘") }, sc);
                    }
                    let x0 = rect.left() + 38.0;
                    let title_job = one_line_job(&[(&p.title, 13.5, t.text)], (rx - x0 - 4.0).max(40.0));
                    let g = painter.layout_job(title_job);
                    painter.galley(pos2(x0, y1 - g.size().y / 2.0), g, t.text);
                    let updated = parse_iso8601(&p.updated_at).map(|ts| short_relative_time(ts, now)).unwrap_or_default();
                    let meta = format!("#{} · {} · {} › {} · {}", p.number, p.author.login, p.head_ref_name, p.base_ref_name, updated);
                    let adds = format!("+{}", p.additions);
                    let dels = format!("−{}", p.deletions);
                    let ga = painter.layout_no_wrap(dels.clone(), FontId::monospace(11.0), t.red);
                    let r_d = egui::Rect::from_min_size(pos2(rect.right() - 16.0 - ga.size().x, rect.top() + 29.0), ga.size());
                    painter.galley(r_d.min, ga, t.red);
                    let gb = painter.layout_no_wrap(adds, FontId::monospace(11.0), t.green);
                    let r_a = egui::Rect::from_min_size(pos2(r_d.left() - 6.0 - gb.size().x, rect.top() + 29.0), gb.size());
                    painter.galley(r_a.min, gb, t.green);
                    let g = painter.layout_job(one_line_job(&[(&meta, 11.0, t.text_faint)], (r_a.left() - x0 - 8.0).max(40.0)));
                    painter.galley(pos2(x0, rect.top() + 29.0), g, t.text_faint);
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
        if self.discard_confirm {
            ui.label(kiln_common::i18n::tr("작성한 초안을 버릴까요? GitHub에는 전송되지 않습니다."));
            let mut discard = false;
            ui.horizontal(|ui| {
                if tool_button(ui, None, kiln_common::i18n::tr("계속 작성")).clicked() { self.discard_confirm = false; }
                if tool_button(ui, None, kiln_common::i18n::tr("초안 버리기")).clicked() { discard = true; }
            });
            if discard { self.form = None; self.form_open = false; self.discard_confirm = false; }
            return;
        }
        let Some(form) = &mut self.form else { return };
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(kiln_common::i18n::tr("새 풀 리퀘스트")).font(kiln_common::fonts::semibold(14.0)).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, kiln_common::i18n::tr("초안 보관하고 닫기")).clicked() {
                    close = true;
                }
                if form.submit.is_none() && tool_button(ui, None, kiln_common::i18n::tr("초안 버리기…")).clicked() { self.discard_confirm = true; }
            });
        });
        if close {
            self.form_open = false;
            return;
        }
        ui.add_space(4.0);
        if let Some(e) = form.error.clone() {
            if banner(ui, BannerKind::Error, kiln_common::i18n::tr("풀 리퀘스트를 만들 수 없습니다"), Some(&e), true) {
                form.error = None;
            }
            ui.add_space(4.0);
        }
        if form.defaults.is_some() {
            ui.horizontal(|ui| {
                spinner(ui, 12.0);
                ui.label(dim(kiln_common::i18n::tr("브랜치 커밋으로 준비하는 중…")));
            });
            return;
        }
        let submitting = form.submit.is_some();
        ui.add_enabled_ui(!submitting, |ui| {
            let head_width=ui.painter().layout_no_wrap(form.head.clone(),kiln_common::fonts::medium(11.0),t.accent).size().x;
            let stacked=head_width+240.0>ui.available_width();
            let render_base=|ui:&mut Ui, form:&mut CreateForm, width:f32| {
                ui.label(dim(kiln_common::i18n::tr("→ 대상")));
                egui::ComboBox::from_id_salt(Id::new("pr_create_base"))
                    .selected_text(RichText::new(&form.req.base).size(12.5)).truncate()
                    .width(width)
                    .icon(|ui, rect, visuals, _open| {
                        paint_icon(ui.painter(), rect.expand(2.0), Icon::ChevronDown, visuals.fg_stroke.color);
                    })
                    .show_ui(ui, |ui| {
                        ui.set_width(width);
                        for b in &form.bases {
                            if ui.add_sized([width,24.0],egui::Button::selectable(form.req.base==*b,b).truncate()).on_hover_text(b).clicked() {
                                form.req.base=b.clone();ui.close();
                            }
                        }
                    }).response.on_hover_text(&form.req.base);
            };
            if stacked {
                ui.horizontal(|ui| {
                    ui.label(dim(kiln_common::i18n::tr("원본")));
                    ui.add(egui::Label::new(RichText::new(&form.head).font(kiln_common::fonts::medium(11.0)).color(t.accent)).truncate()).on_hover_text(&form.head);
                });
                let width=(ui.available_width()-55.0).max(80.0);
                ui.horizontal(|ui|render_base(ui,form,width));
            } else {
                ui.horizontal(|ui| {
                    ui.label(dim(kiln_common::i18n::tr("원본")));
                    outline_badge(ui, if form.head.is_empty() { "?" } else { &form.head }, t.accent);
                    render_base(ui,form,140.0);
                });
            }
            ui.add_space(10.0);
            ui.label(RichText::new(kiln_common::i18n::tr("제목")).font(kiln_common::fonts::semibold(12.0)).color(t.text_dim));
            ui.add(
                egui::TextEdit::singleline(&mut form.req.title)
                    .hint_text(kiln_common::i18n::tr("풀 리퀘스트 제목"))
                    .desired_width(f32::INFINITY)
                    .frame(input_frame()),
            );
            ui.add_space(8.0);
            ui.label(RichText::new(kiln_common::i18n::tr("설명")).font(kiln_common::fonts::semibold(12.0)).color(t.text_dim));
            ui.add(
                egui::TextEdit::multiline(&mut form.req.body)
                    .hint_text(kiln_common::i18n::tr("변경 사항을 설명하세요 (Markdown 지원)"))
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .frame(input_frame()),
            );
            ui.add_space(4.0);
            checkbox_row(ui, &mut form.req.draft, kiln_common::i18n::tr("초안으로 만들기"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can = !form.req.title.trim().is_empty() && !form.req.base.is_empty() && !submitting;
                let label = if form.req.draft { kiln_common::i18n::tr("초안 PR 만들기") } else { kiln_common::i18n::tr("풀 리퀘스트 만들기") };
                if ui.add_enabled_ui(can, |ui| primary_button(ui, label, None)).inner.clicked() {
                    let b = self.backend.clone();
                    let mut req = form.req.clone();
                    req.head = form.head.clone();
                    form.error = None;
                    form.submit = Some(Task::spawn(ui.ctx(), move || b.create(&req)));
                }
                if submitting {
                    spinner(ui, 12.0);
                    ui.label(dim(kiln_common::i18n::tr("만드는 중…")));
                }
            });
        });
    }
}
