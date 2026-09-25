//! 저장소 선택 팝업(RepoPicker): 현재 저장소, 내/조직 저장소 목록, GitHub 검색, 복제.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{CornerRadius, Id, Rect, RichText, Sense, Ui, pos2, vec2};
use kiln_common::Task;

use super::gh_widgets::*;
use super::panel::one_line_job;
use super::widgets::*;
use crate::cmd::GitResult;
use crate::github::{GithubBackend, RepoInfo, RepoListItem, RepoRef, Viewer};
use crate::util::{now_unix, parse_iso8601, short_relative_time};

const POPUP_W: f32 = 440.0;
const ROW_H: f32 = 46.0;

/// 저장소 목록 작업(소유자별 또는 검색어별).
type ListTask<K> = Option<(K, Task<GitResult<Vec<RepoListItem>>>)>;

/// 저장소 선택 팝업에서 고른 동작.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoPickerAction {
    /// 허브 대상 저장소를 바꾼다.
    Select(RepoRef),
    /// 작업 폴더의 저장소로 돌아간다.
    UseWorkspace,
    /// 새 스페이스로 복제한다(`owner/name`).
    Clone(String),
}

/// 저장소 선택 팝업.
pub struct RepoPicker {
    backend: Arc<dyn GithubBackend>,
    viewer: Option<Viewer>,
    viewer_task: Option<Task<GitResult<Viewer>>>,
    /// 선택한 소유자. `None` 이면 로그인 사용자.
    owner: Option<String>,
    lists: HashMap<Option<String>, Vec<RepoListItem>>,
    list_task: ListTask<Option<String>>,
    query: String,
    search_task: ListTask<String>,
    search: Option<(String, Vec<RepoListItem>)>,
    error: Option<String>,
    focus: bool,
    now_override: Option<i64>,
}

impl RepoPicker {
    pub fn new(backend: Arc<dyn GithubBackend>) -> Self {
        Self {
            backend,
            viewer: None,
            viewer_task: None,
            owner: None,
            lists: HashMap::new(),
            list_task: None,
            query: String::new(),
            search_task: None,
            search: None,
            error: None,
            focus: false,
            now_override: None,
        }
    }

    fn popup_id() -> Id {
        Id::new("kiln_repo_picker")
    }

    /// 팝업을 열거나 닫는다. 열 때 목록을 읽기 시작한다.
    pub fn toggle(&mut self, ctx: &egui::Context) {
        egui::Popup::toggle_id(ctx, Self::popup_id());
        if egui::Popup::is_id_open(ctx, Self::popup_id()) {
            self.query.clear();
            self.search = None;
            self.focus = true;
            self.error = None;
            if self.viewer.is_none() && self.viewer_task.is_none() {
                let b = self.backend.clone();
                self.viewer_task = Some(Task::spawn(ctx, move || b.viewer()));
            }
            self.load_owner(ctx, self.owner.clone());
        }
    }

    pub fn is_open(&self, ctx: &egui::Context) -> bool {
        egui::Popup::is_id_open(ctx, Self::popup_id())
    }

    pub fn close(&self, ctx: &egui::Context) {
        egui::Popup::close_id(ctx, Self::popup_id());
    }

    pub fn is_loading(&mut self) -> bool {
        self.viewer_task.as_mut().is_some_and(|t| t.is_pending())
            || self.list_task.as_mut().is_some_and(|(_, t)| t.is_pending())
            || self.search_task.as_mut().is_some_and(|(_, t)| t.is_pending())
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    fn load_owner(&mut self, ctx: &egui::Context, owner: Option<String>) {
        self.owner = owner.clone();
        if self.lists.contains_key(&owner) || self.list_task.as_ref().is_some_and(|(o, _)| o == &owner) {
            return;
        }
        let b = self.backend.clone();
        let o = owner.clone();
        self.list_task = Some((owner, Task::spawn(ctx, move || b.list_repos(o.as_deref()))));
    }

    fn pump(&mut self) {
        if let Some(t) = &mut self.viewer_task
            && let Some(r) = t.take()
        {
            self.viewer_task = None;
            match r {
                Ok(v) => self.viewer = Some(v),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if let Some((o, t)) = &mut self.list_task
            && let Some(r) = t.take()
        {
            let o = o.clone();
            self.list_task = None;
            match r {
                Ok(v) => {
                    self.lists.insert(o, v);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if let Some((q, t)) = &mut self.search_task
            && let Some(r) = t.take()
        {
            let q = q.clone();
            self.search_task = None;
            match r {
                Ok(v) => self.search = Some((q, v)),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    /// 팝업을 `anchor` 아래에 그린다. `current` 는 지금 허브가 보는 저장소, `workspace` 는 작업 폴더 저장소.
    pub fn show(&mut self, ui: &Ui, anchor: Rect, current: Option<&RepoInfo>, workspace: Option<&RepoRef>) -> Option<RepoPickerAction> {
        self.pump();
        let mut action = None;
        let t = theme();
        let width = POPUP_W.min(ui.ctx().content_rect().width() - 16.0);
        let now = self.now_override.unwrap_or_else(now_unix);
        egui::Popup::new(Self::popup_id(), ui.ctx().clone(), anchor, ui.layer_id())
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(width)
            .frame(popup_frame().inner_margin(egui::Margin::same(8)))
            .show(|ui| {
                ui.set_width(width - 16.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                if let Some(c) = current {
                    current_card(ui, c);
                    ui.add_space(6.0);
                }
                if let (Some(ws), Some(c)) = (workspace, current)
                    && &c.repo != ws
                {
                    let label = format!("작업 폴더 저장소로 돌아가기 ({ws})");
                    if tool_button(ui, Some(Icon::Repo), &label).clicked() {
                        action = Some(RepoPickerAction::UseWorkspace);
                    }
                    ui.add_space(4.0);
                }
                let sid = Id::new("kiln_repo_picker_search");
                let focused = ui.memory(|m| m.has_focus(sid));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(sid)
                        .hint_text("저장소 필터 · Enter로 GitHub 전체 검색")
                        .desired_width(f32::INFINITY)
                        .frame(kiln_common::widgets::input_frame(focused, false)),
                );
                if self.focus {
                    r.request_focus();
                    self.focus = false;
                }
                let q = self.query.trim().to_string();
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !q.is_empty() {
                    // `owner/name` 을 직접 입력하면 바로 고른다.
                    if let Some(rr) = RepoRef::parse(&q).filter(|_| q.contains('/') && !q.contains(' ')) {
                        action = Some(RepoPickerAction::Select(rr));
                    } else {
                        let b = self.backend.clone();
                        let qq = q.clone();
                        self.search_task = Some((q.clone(), Task::spawn(ui.ctx(), move || b.search_repos(&qq))));
                    }
                }
                if q.is_empty() && self.search.is_some() {
                    self.search = None;
                }
                ui.add_space(4.0);
                // 소유자 칩
                if let Some(v) = self.viewer.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                        let mut owners: Vec<(Option<String>, String)> = vec![(None, v.login.clone())];
                        owners.extend(v.orgs.iter().map(|o| (Some(o.clone()), o.clone())));
                        for (o, label) in owners {
                            let sel = self.owner == o && self.search.is_none();
                            if owner_chip(ui, &label, sel).clicked() {
                                self.search = None;
                                self.query.clear();
                                self.load_owner(ui.ctx(), o);
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                if let Some(e) = self.error.clone()
                    && banner(ui, BannerKind::Warning, "저장소 목록을 불러올 수 없습니다", Some(&e), true)
                {
                    self.error = None;
                }
                let (title, items, loading): (String, Vec<RepoListItem>, bool) = match &self.search {
                    Some((sq, v)) => (format!("\"{sq}\" 검색 결과"), v.clone(), self.search_task.is_some()),
                    None => {
                        let name = self.owner.clone().or_else(|| self.viewer.as_ref().map(|v| v.login.clone())).unwrap_or_else(|| "내".into());
                        let list = self.lists.get(&self.owner).cloned().unwrap_or_default();
                        let ql = q.to_lowercase();
                        let list: Vec<RepoListItem> = list
                            .into_iter()
                            .filter(|r| ql.is_empty() || r.name_with_owner.to_lowercase().contains(&ql) || r.description.to_lowercase().contains(&ql))
                            .collect();
                        (format!("{name}의 저장소"), list, self.list_task.is_some())
                    }
                };
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(title).font(kiln_common::fonts::semibold(11.5)).color(t.text_faint));
                    if loading || self.search_task.is_some() {
                        spinner(ui, 10.0);
                    }
                });
                egui::ScrollArea::vertical().max_height(340.0).min_scrolled_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if items.is_empty() && !loading {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(faint(if q.is_empty() { "저장소가 없습니다" } else { "일치하는 저장소가 없습니다 · Enter로 GitHub에서 검색" }));
                        });
                        ui.add_space(8.0);
                    }
                    for it in &items {
                        let is_current = current.is_some_and(|c| c.repo.full_name() == it.name_with_owner);
                        match repo_row(ui, it, is_current, now) {
                            RowAction::Select => {
                                if let Some(rr) = it.repo() {
                                    action = Some(RepoPickerAction::Select(rr));
                                }
                            }
                            RowAction::Clone => action = Some(RepoPickerAction::Clone(it.name_with_owner.clone())),
                            RowAction::None => {}
                        }
                    }
                });
            });
        if action.is_some() {
            self.close(ui.ctx());
        }
        action
    }
}

fn current_card(ui: &mut Ui, c: &RepoInfo) {
    let t = theme();
    egui::Frame::new()
        .fill(t.bg_hover)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icon_label(ui, if c.is_private { Icon::Lock } else { Icon::Repo }, t.text_dim, 14.0);
                ui.label(RichText::new(c.repo.full_name()).font(kiln_common::fonts::semibold(13.0)).color(t.text));
                let vis = match c.visibility.as_str() {
                    "PRIVATE" => "비공개",
                    "INTERNAL" => "내부",
                    _ => "공개",
                };
                outline_badge(ui, vis, t.text_dim);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(faint(compact_count(c.stargazer_count)));
                    icon_label(ui, Icon::Star, t.text_faint, 12.0);
                });
            });
            if !c.description.is_empty() {
                ui.add(egui::Label::new(RichText::new(&c.description).size(12.0).color(t.text_dim)).wrap());
            }
        });
}

fn owner_chip(ui: &mut Ui, label: &str, sel: bool) -> egui::Response {
    let t = theme();
    let g = ui.painter().layout_no_wrap(label.to_string(), kiln_common::fonts::medium(11.5), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, format!("소유자 {label}")));
    if ui.is_rect_visible(rect) {
        let fill = if sel { alpha(t.accent, 0.16) } else if resp.hovered() { t.bg_hover } else { egui::Color32::TRANSPARENT };
        let stroke = if sel { alpha(t.accent, 0.6) } else { t.border };
        ui.painter().rect(rect, CornerRadius::same(11), fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        paint_avatar(ui.painter(), pos2(rect.left() + 11.0, rect.center().y), 7.0, label);
        ui.painter().galley_with_override_text_color(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, if sel { t.text } else { t.text_dim });
    }
    resp
}

enum RowAction {
    None,
    Select,
    Clone,
}

fn repo_row(ui: &mut Ui, it: &RepoListItem, is_current: bool, now: i64) -> RowAction {
    let t = theme();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, is_current, &it.name_with_owner));
    let clone_rect = Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(24.0, 24.0));
    let clone = icon_button_at(ui, clone_rect, resp.id.with("clone"), Icon::Download, &format!("{} 새 스페이스로 복제", it.name_with_owner));
    if ui.is_rect_visible(rect) {
        kiln_common::widgets::paint_row(ui.painter(), rect, is_current, resp.hovered() || clone.hovered());
        let p = ui.painter();
        paint_icon(p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.top() + 15.0), vec2(13.0, 13.0)), if it.is_private { Icon::Lock } else { Icon::Repo }, t.text_faint);
        let (owner, name) = it.name_with_owner.split_once('/').unwrap_or(("", &it.name_with_owner));
        let when = parse_iso8601(&it.updated_at).map(|ts| short_relative_time(ts, now)).unwrap_or_default();
        let tg = p.layout_no_wrap(when, kiln_common::fonts::regular(11.0), t.text_faint);
        let tx = clone_rect.left() - 6.0 - tg.size().x;
        p.galley(pos2(tx, rect.top() + 15.0 - tg.size().y / 2.0), tg, t.text_faint);
        let x0 = rect.left() + 30.0;
        let owner_s = format!("{owner}/");
        let job = one_line_job(&[(&owner_s, 13.0, t.text_dim), (name, 13.0, t.text)], (tx - x0 - 8.0).max(40.0));
        let g = p.layout_job(job);
        let ny = if it.description.is_empty() { rect.center().y } else { rect.top() + 15.0 };
        p.galley(pos2(x0, ny - g.size().y / 2.0), g, t.text);
        if !it.description.is_empty() {
            let g = p.layout_job(one_line_job(&[(&it.description, 11.0, t.text_faint)], (clone_rect.left() - x0 - 8.0).max(40.0)));
            p.galley(pos2(x0, rect.top() + 32.0 - g.size().y / 2.0), g, t.text_faint);
        }
    }
    if clone.clicked() {
        RowAction::Clone
    } else if resp.clicked() {
        RowAction::Select
    } else {
        RowAction::None
    }
}
