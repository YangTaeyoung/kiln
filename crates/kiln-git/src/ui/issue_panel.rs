//! 이슈 목록 패널(IssuePanel)과 이슈 생성 폼.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, Id, Layout, Margin, RichText, Sense, Ui, pos2, vec2};
use kiln_common::Task;

use super::gh_widgets::*;
use super::panel::one_line_job;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::GhBackend;
use crate::github::{GithubBackend, IssueCreate, IssueFilter, IssueItem, Label, RepoRef};
use crate::util::{now_unix, parse_iso8601, short_relative_time};

const ROW_H: f32 = 54.0;

/// 라벨/담당자 선택지(저장소에서 한 번 읽는다).
#[derive(Default)]
pub(crate) struct RepoMeta {
    pub labels: Vec<Label>,
    pub users: Vec<String>,
    labels_task: Option<Task<GitResult<Vec<Label>>>>,
    users_task: Option<Task<GitResult<Vec<String>>>>,
    pub error: Option<String>,
    started: bool,
}

impl RepoMeta {
    /// 처음 호출될 때 라벨과 담당자 후보를 읽기 시작하고, 끝난 결과를 반영한다.
    pub fn pump(&mut self, ctx: &egui::Context, backend: &Arc<dyn GithubBackend>, repo: Option<&RepoRef>) {
        if !self.started {
            self.started = true;
            let (b, r) = (backend.clone(), repo.cloned());
            self.labels_task = Some(Task::spawn(ctx, move || b.labels(r.as_ref())));
            let (b, r) = (backend.clone(), repo.cloned());
            self.users_task = Some(Task::spawn(ctx, move || b.assignable_users(r.as_ref())));
        }
        if let Some(t) = &mut self.labels_task
            && let Some(r) = t.take()
        {
            self.labels_task = None;
            match r {
                Ok(v) => self.labels = v,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if let Some(t) = &mut self.users_task
            && let Some(r) = t.take()
        {
            self.users_task = None;
            match r {
                Ok(v) => self.users = v,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    pub fn is_loading(&self) -> bool {
        self.labels_task.is_some() || self.users_task.is_some()
    }

    pub fn labels_loading(&self) -> bool {
        self.labels_task.is_some()
    }

    pub fn users_loading(&self) -> bool {
        self.users_task.is_some()
    }

    /// 이름으로 라벨 색을 찾는다.
    pub fn label_color(&self, name: &str) -> egui::Color32 {
        self.labels.iter().find(|l| l.name == name).map(|l| hex_color(&l.color)).unwrap_or(theme().text_dim)
    }
}

struct CreateForm {
    req: IssueCreate,
    submit: Option<Task<GitResult<u64>>>,
    error: Option<String>,
    label_pick: PickerState,
    user_pick: PickerState,
}

/// 이슈 목록 패널.
pub struct IssuePanel {
    backend: Arc<dyn GithubBackend>,
    repo: Option<RepoRef>,
    filter: IssueFilter,
    search: String,
    /// 서버 검색에 쓴 질의(Enter 로 확정).
    server_query: String,
    label_filter: Option<String>,
    load: Option<Task<GitResult<Vec<IssueItem>>>>,
    started: bool,
    items: Vec<IssueItem>,
    error: Option<GitError>,
    form: Option<CreateForm>,
    form_open: bool,
    discard_confirm: bool,
    meta: RepoMeta,
    created: Option<u64>,
    embedded: bool,
    now_override: Option<i64>,
}

impl IssuePanel {
    /// 작업 폴더 저장소(`repo` 가 `None`)나 지정 저장소의 이슈 패널.
    pub fn new(root: PathBuf, repo: Option<RepoRef>) -> Self {
        Self::with_backend(Arc::new(GhBackend::new(root)), repo)
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(backend: Arc<dyn GithubBackend>, repo: Option<RepoRef>) -> Self {
        Self {
            backend,
            repo,
            filter: IssueFilter::Open,
            search: String::new(),
            server_query: String::new(),
            label_filter: None,
            load: None,
            started: false,
            items: Vec::new(),
            error: None,
            form: None,
            form_open: false,
            discard_confirm: false,
            meta: RepoMeta::default(),
            created: None,
            embedded: false,
            now_override: None,
        }
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        self.repo.as_ref()
    }

    pub fn refresh(&mut self) {
        self.started = false;
    }

    pub fn set_filter(&mut self, f: IssueFilter) {
        if self.filter != f {
            self.filter = f;
            self.refresh();
        }
    }

    pub fn filter(&self) -> IssueFilter {
        self.filter
    }

    /// 불러온 전체 이슈(검색어·라벨 필터 적용 전).
    pub fn items(&self) -> &[IssueItem] {
        &self.items
    }

    /// 검색어·라벨 필터를 적용한 이슈.
    pub fn visible_items(&self) -> Vec<&IssueItem> {
        let q = self.search.trim().to_lowercase();
        let q_num = q.trim_start_matches('#').parse::<u64>().ok();
        let server = self.server_query.trim().to_lowercase();
        self.items
            .iter()
            .filter(|i| self.label_filter.as_ref().is_none_or(|l| i.labels.iter().any(|x| &x.name == l)))
            .filter(|i| {
                q.is_empty()
                    || q == server
                    || q_num == Some(i.number)
                    || i.title.to_lowercase().contains(&q)
                    || i.author.login.to_lowercase().contains(&q)
                    || i.labels.iter().any(|l| l.name.to_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn set_search(&mut self, q: &str) {
        self.search = q.to_string();
    }

    pub fn set_label_filter(&mut self, l: Option<String>) {
        self.label_filter = l;
    }

    /// 허브 안에 넣을 때 제목을 숨긴다.
    pub fn set_embedded(&mut self, on: bool) {
        self.embedded = on;
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started
            || self.load.as_mut().is_some_and(|t| t.is_pending())
            || self.form.as_mut().is_some_and(|f| f.submit.as_mut().is_some_and(|t| t.is_pending()))
            || (self.form.is_some() && self.meta.is_loading())
    }

    /// 이슈 생성 폼을 연다.
    pub fn open_create_form(&mut self) {
        self.form_open = true;
        self.discard_confirm = false;
        if self.form.is_some() { return; }
        self.form = Some(CreateForm {
            req: IssueCreate::default(),
            submit: None,
            error: None,
            label_pick: PickerState::default(),
            user_pick: PickerState::default(),
        });
        self.created = None;
    }

    pub fn is_submitting(&self) -> bool { self.form.as_ref().is_some_and(|f| f.submit.is_some()) }

    pub fn creation_draft(&self) -> Option<IssueCreate> {
        self.form.as_ref().filter(|f| f.req != IssueCreate::default()).map(|f| f.req.clone())
    }

    pub fn restore_creation_draft(&mut self, request: &IssueCreate) {
        self.form = None;
        self.open_create_form();
        self.form.as_mut().unwrap().req = request.clone();
    }

    pub fn is_form_open(&self) -> bool {
        self.form.is_some() && self.form_open
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    fn pump(&mut self, ctx: &egui::Context, events: &mut Vec<GitEvent>) {
        if !self.started {
            self.started = true;
            let b = self.backend.clone();
            let (f, r, q) = (self.filter, self.repo.clone(), self.server_query.clone());
            self.load = Some(Task::spawn(ctx, move || b.issues(r.as_ref(), f, &q)));
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
        if self.form.is_some() {
            self.meta.pump(ctx, &self.backend, self.repo.as_ref());
        }
        if let Some(form) = &mut self.form
            && let Some(t) = &mut form.submit
            && let Some(r) = t.take()
        {
            form.submit = None;
            match r {
                Ok(n) => {
                    events.push(GitEvent::OpenIssue(n));
                    self.created = Some(n);
                    self.form = None;
                    self.form_open = false;
                    self.refresh();
                }
                Err(e) => form.error = Some(e.to_string()),
            }
        }
    }

    /// 패널을 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        let mut events = Vec::new();
        self.pump(ui.ctx(), &mut events);
        let t = theme();
        egui::Frame::new().fill(t.bg_panel).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            if self.form.is_some() && self.form_open {
                egui::ScrollArea::vertical().id_salt("issue_form").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| self.ui_form(ui));
                });
                return;
            }
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 8 }).show(ui, |ui| {
                self.ui_toolbar(ui);
            });
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
                let n = self.visible_items().len();
                if self.started && self.load.is_none() && self.error.is_none() {
                    ui.label(faint(format!("이슈 {n}개")));
                }
            } else {
                ui.label(RichText::new("이슈").font(kiln_common::fonts::semibold(13.5)).color(t.text));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if kiln_common::widgets::button_with(
                    ui,
                    Some(kiln_common::icons::Icon::Plus),
                    if self.form.is_some() { "초안 이어 쓰기" } else { "새 이슈" },
                    kiln_common::widgets::ButtonKind::Primary,
                    true,
                )
                .on_hover_text("이슈 새로 만들기")
                .clicked()
                {
                    self.open_create_form();
                }
                if self.load.is_some() {
                    spinner(ui, 12.0);
                } else if icon_button(ui, Icon::Refresh, "새로 고침").clicked() {
                    self.refresh();
                }
            });
        });
        ui.add_space(8.0);
        let mut f = self.filter;
        let opts: Vec<(IssueFilter, &str)> = IssueFilter::ALL.iter().map(|f| (*f, f.label())).collect();
        if segmented(ui, &mut f, &opts) {
            self.set_filter(f);
        }
        ui.add_space(8.0);
        let sid = Id::new("kiln_issue_search");
        let focused = ui.memory(|m| m.has_focus(sid));
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .id(sid)
                .hint_text("제목, 작성자, 라벨, #번호 · Enter로 GitHub 검색")
                .desired_width(f32::INFINITY)
                .frame(kiln_common::widgets::input_frame(focused, false)),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && self.search.trim() != self.server_query.trim() {
            self.server_query = self.search.trim().to_string();
            self.refresh();
        }
        // 라벨 칩: 불러온 이슈에 많이 붙은 라벨
        let mut counts: Vec<(String, String, usize)> = Vec::new();
        for i in &self.items {
            for l in &i.labels {
                match counts.iter_mut().find(|c| c.0 == l.name) {
                    Some(c) => c.2 += 1,
                    None => counts.push((l.name.clone(), l.color.clone(), 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        counts.truncate(6);
        if !counts.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                for (name, color, n) in &counts {
                    let sel = self.label_filter.as_deref() == Some(name.as_str());
                    if label_filter_chip(ui, name, hex_color(color), *n, sel).clicked() {
                        self.label_filter = if sel { None } else { Some(name.clone()) };
                    }
                }
            });
        }
        if let Some(n) = self.created {
            ui.add_space(6.0);
            if banner(ui, BannerKind::Success, &format!("이슈 #{n} 생성 완료"), None, true) {
                self.created = None;
            }
        }
    }

    fn ui_list(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let t = theme();
        if let Some(e) = self.error.clone() {
            if gh_error_state(ui, &e, "이슈", events) == ErrorAction::Retry {
                self.refresh();
            }
            return;
        }
        if self.load.is_some() && self.items.is_empty() {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                spinner(ui, 14.0);
                ui.label(dim("이슈 불러오는 중…"));
            });
            return;
        }
        let items: Vec<IssueItem> = self.visible_items().into_iter().cloned().collect();
        if items.is_empty() {
            let msg = match self.filter {
                IssueFilter::Mine => "열려 있는 내 이슈가 없습니다",
                IssueFilter::Assigned => "나에게 할당된 이슈가 없습니다",
                IssueFilter::Closed => "닫힌 이슈가 없습니다",
                IssueFilter::Open => "열린 이슈가 없습니다",
            };
            let filtered = !self.search.trim().is_empty() || self.label_filter.is_some();
            empty_state_icon(ui, Icon::Issue, msg, if filtered { "다른 검색어나 라벨을 사용해 보세요." } else { "" });
            return;
        }
        let now = self.now_override.unwrap_or_else(now_unix);
        egui::ScrollArea::vertical().id_salt("issue_list").auto_shrink([false, false]).show_rows(ui, ROW_H, items.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for it in &items[range] {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("#{} {}", it.number, it.title)));
                kiln_common::widgets::paint_row(ui.painter(), rect.shrink2(vec2(6.0, 1.0)), false, resp.hovered());
                let painter = ui.painter();
                let (icon, sc) = issue_state_style(it.issue_state());
                let y1 = rect.top() + 17.0;
                let y2 = rect.top() + 37.0;
                paint_icon(painter, egui::Rect::from_center_size(pos2(rect.left() + 22.0, y1), vec2(15.0, 15.0)), icon, sc);
                // 오른쪽: 담당자 아바타, 댓글 수
                let mut rx = rect.right() - 16.0;
                for a in it.assignees.iter().take(3) {
                    paint_avatar(painter, pos2(rx - 9.0, y1), 9.0, &a.login);
                    rx -= 14.0;
                }
                if !it.assignees.is_empty() {
                    rx -= 8.0;
                }
                if it.comments > 0 {
                    let g = painter.layout_no_wrap(it.comments.to_string(), kiln_common::fonts::medium(11.5), t.text_dim);
                    let gx = rx - g.size().x;
                    painter.galley(pos2(gx, y1 - g.size().y / 2.0), g, t.text_dim);
                    paint_icon(painter, egui::Rect::from_center_size(pos2(gx - 9.0, y1), vec2(13.0, 13.0)), Icon::Comment, t.text_faint);
                    rx = gx - 20.0;
                }
                let x0 = rect.left() + 38.0;
                let g = painter.layout_job(one_line_job(&[(&it.title, 13.5, t.text)], (rx - x0 - 4.0).max(40.0)));
                painter.galley(pos2(x0, y1 - g.size().y / 2.0), g, t.text);
                let updated = parse_iso8601(&it.updated_at).map(|ts| short_relative_time(ts, now)).unwrap_or_default();
                let meta = format!("#{} · {} · {}", it.number, it.author.login, updated);
                let g = painter.layout_job(one_line_job(&[(&meta, 11.0, t.text_faint)], (rect.right() - x0 - 16.0).max(40.0)));
                let mut lx = x0 + g.size().x + 8.0;
                painter.galley(pos2(x0, y2 - g.size().y / 2.0), g, t.text_faint);
                for l in &it.labels {
                    match paint_label_chip(painter, pos2(lx, y2), &l.name, hex_color(&l.color), rect.right() - 12.0) {
                        Some(r) => lx = r + 4.0,
                        None => break,
                    }
                }
                if resp.clicked() {
                    events.push(GitEvent::OpenIssue(it.number));
                }
                resp.on_hover_text(format!("#{} {}\n{}", it.number, it.title, it.url));
            }
        });
    }

    fn ui_form(&mut self, ui: &mut Ui) {
        let t = theme();
        if self.discard_confirm {
            ui.label("작성한 초안을 버릴까요? GitHub에는 전송되지 않습니다.");
            let mut discard = false;
            ui.horizontal(|ui| {
                if tool_button(ui, None, "계속 작성").clicked() { self.discard_confirm = false; }
                if tool_button(ui, None, "초안 버리기").clicked() { discard = true; }
            });
            if discard { self.form = None; self.form_open = false; self.discard_confirm = false; }
            return;
        }
        let Some(form) = &mut self.form else { return };
        let mut close = false;
        ui.horizontal(|ui| {
            icon_label(ui, Icon::Issue, t.green, 16.0);
            ui.label(RichText::new("새 이슈").font(kiln_common::fonts::semibold(14.0)).color(t.text));
            if let Some(r) = &self.repo {
                let width=(ui.available_width()-140.0).max(20.0);
                let name=r.full_name();
                let natural_width=ui.painter().layout_no_wrap(name.clone(),kiln_common::fonts::regular(12.0),t.text_faint).size().x;
                if natural_width>width { ui.add_sized([width,18.0],egui::Label::new(faint(&name)).truncate()).on_hover_text(&name); }
                else { ui.label(faint(&name)); }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, "초안 보관하고 닫기").clicked() {
                    close = true;
                }
                if form.submit.is_none() && tool_button(ui, None, "초안 버리기…").clicked() { self.discard_confirm = true; }
            });
        });
        if close {
            self.form_open = false;
            return;
        }
        ui.add_space(8.0);
        if let Some(e) = form.error.clone() {
            if banner(ui, BannerKind::Error, "이슈를 만들 수 없습니다", Some(&e), true) {
                form.error = None;
            }
            ui.add_space(6.0);
        }
        let submitting = form.submit.is_some();
        ui.add_enabled_ui(!submitting, |ui| {
            field_label(ui, "제목");
            let tid = Id::new("kiln_issue_create_title");
            let focused = ui.memory(|m| m.has_focus(tid));
            ui.add(
                egui::TextEdit::singleline(&mut form.req.title)
                    .id(tid)
                    .hint_text("이슈 제목")
                    .desired_width(f32::INFINITY)
                    .frame(kiln_common::widgets::input_frame(focused, false)),
            );
            ui.add_space(10.0);
            field_label(ui, "설명");
            let bid = Id::new("kiln_issue_create_body");
            let focused = ui.memory(|m| m.has_focus(bid));
            ui.add(
                egui::TextEdit::multiline(&mut form.req.body)
                    .id(bid)
                    .hint_text("무엇이 문제인지, 어떻게 재현하는지 적어 주세요 (Markdown 지원)")
                    .desired_rows(9)
                    .desired_width(f32::INFINITY)
                    .frame(kiln_common::widgets::input_frame(focused, false)),
            );
            ui.add_space(12.0);
            // 라벨
            let meta = &self.meta;
            picker_field(
                ui,
                "라벨",
                "라벨 선택",
                Id::new("issue_create_labels"),
                &mut form.label_pick,
                &label_options(&meta.labels),
                &mut form.req.labels,
                meta.labels_loading(),
                meta.error.as_deref(),
                |ui, name| {
                    label_chip(ui, name, meta.label_color(name));
                },
            );
            ui.add_space(10.0);
            picker_field(
                ui,
                "담당자",
                "담당자 선택",
                Id::new("issue_create_assignees"),
                &mut form.user_pick,
                &user_options(&meta.users),
                &mut form.req.assignees,
                meta.users_loading(),
                meta.error.as_deref(),
                |ui, name| {
                    person_row(ui, name);
                },
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let can = !form.req.title.trim().is_empty() && !submitting;
                let r = ui.add_enabled_ui(can, |ui| primary_button(ui, "생성", Some(96.0))).inner;
                if r.clicked() {
                    let b = self.backend.clone();
                    let (req, repo) = (form.req.clone(), self.repo.clone());
                    form.error = None;
                    form.submit = Some(Task::spawn(ui.ctx(), move || b.create_issue(repo.as_ref(), &req)));
                }
                if submitting {
                    spinner(ui, 12.0);
                    ui.label(dim("만드는 중…"));
                } else if form.req.title.trim().is_empty() {
                    ui.label(faint("제목을 입력하세요"));
                }
            });
        });
    }
}

/// 폼 필드 제목.
pub(crate) fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(kiln_common::fonts::semibold(12.0)).color(theme().text_dim));
    ui.add_space(2.0);
}

/// 선택된 항목 칩과 "선택" 버튼, 다중 선택 팝업을 묶은 폼 필드.
#[allow(clippy::too_many_arguments)]
pub(crate) fn picker_field(
    ui: &mut Ui,
    title: &str,
    button: &str,
    popup_id: Id,
    st: &mut PickerState,
    options: &[PickOption],
    selected: &mut Vec<String>,
    loading: bool,
    error: Option<&str>,
    show_item: impl Fn(&mut Ui, &str),
) {
    field_label(ui, title);
    let anchor = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let r = tool_button(ui, Some(if title == "라벨" { Icon::Tag } else { Icon::Person }), button);
            if r.clicked() {
                egui::Popup::toggle_id(ui.ctx(), popup_id);
                st.filter.clear();
                st.focus = true;
            }
            for s in selected.iter() {
                show_item(ui, s);
            }
            if selected.is_empty() {
                ui.label(faint("없음"));
            }
            r.rect
        })
        .inner;
    multi_pick_popup(ui, popup_id, anchor.with_max_x(anchor.left() + 300.0), st, &format!("{title} 검색…"), options, selected, loading, error);
}

/// 라벨 필터 칩(클릭 가능, 선택되면 강조).
fn label_filter_chip(ui: &mut Ui, name: &str, color: egui::Color32, count: usize, sel: bool) -> egui::Response {
    let t = theme();
    let text = format!("{name}  {count}");
    let g = ui.painter().layout_no_wrap(text, kiln_common::fonts::medium(11.0), t.text_dim);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, format!("라벨 {name}")));
    if ui.is_rect_visible(rect) {
        let fill = if sel { alpha(t.accent, 0.16) } else if resp.hovered() { t.bg_hover } else { t.bg_elevated };
        let stroke = if sel { alpha(t.accent, 0.6) } else { t.border };
        ui.painter().rect(rect, egui::CornerRadius::same(11), fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        ui.painter().circle_filled(pos2(rect.left() + 11.0, rect.center().y), 4.0, color);
        ui.painter().galley_with_override_text_color(
            pos2(rect.left() + 19.0, rect.center().y - g.size().y / 2.0),
            g,
            if sel { t.text } else { t.text_dim },
        );
    }
    resp
}
