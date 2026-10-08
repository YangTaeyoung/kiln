//! 이슈 상세 뷰(IssueView): 헤더, 본문, 댓글 타임라인, 댓글 작성, 닫기/다시 열기, 라벨·담당자 편집.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Align, CornerRadius, Id, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use kiln_common::Task;

use super::gh_widgets::*;
use super::issue_panel::RepoMeta;
use super::markdown::Markdown;
use super::pr_view::comment_card;
use super::widgets::*;
use crate::GitEvent;
use crate::cmd::{GitError, GitResult};
use crate::gh::GhBackend;
use crate::github::{CloseReason, GithubBackend, IssueDetail, IssueEdit, IssueState, RepoRef};
use crate::util::{now_unix, parse_iso8601, relative_time};

const SIDEBAR_W: f32 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    Comment,
    Close,
    Reopen,
    Edit,
}

/// 편집 중인 목록(팝업이 닫히면 변경분을 반영한다).
struct Draft {
    before: Vec<String>,
    after: Vec<String>,
    pick: PickerState,
}

/// 이슈 상세 뷰.
pub struct IssueView {
    widget_scope: Id,
    repo: Option<RepoRef>,
    number: u64,
    backend: Arc<dyn GithubBackend>,
    load: Option<Task<GitResult<IssueDetail>>>,
    started: bool,
    detail: Option<IssueDetail>,
    body: Markdown,
    comments: Vec<Markdown>,
    error: Option<GitError>,
    comment: String,
    submitted_body: Option<String>,
    action: Option<(ActionKind, Task<GitResult<()>>)>,
    action_msg: Option<(BannerKind, String, Option<String>)>,
    meta: RepoMeta,
    labels_draft: Option<Draft>,
    users_draft: Option<Draft>,
    now_override: Option<i64>,
}

impl IssueView {
    /// `repo` 가 `None` 이면 작업 폴더의 저장소를 쓴다.
    pub fn new(root: PathBuf, repo: Option<RepoRef>, number: u64) -> Self {
        Self::with_backend(repo, number, Arc::new(GhBackend::new(root)))
    }

    /// 데이터 소스를 주입해 만든다.
    pub fn with_backend(repo: Option<RepoRef>, number: u64, backend: Arc<dyn GithubBackend>) -> Self {
        Self {
            widget_scope: Id::NULL,
            repo,
            number,
            backend,
            load: None,
            started: false,
            detail: None,
            body: Markdown::default(),
            comments: Vec::new(),
            error: None,
            comment: String::new(),
            submitted_body: None,
            action: None,
            action_msg: None,
            meta: RepoMeta::default(),
            labels_draft: None,
            users_draft: None,
            now_override: None,
        }
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn repo(&self) -> Option<&RepoRef> {
        self.repo.as_ref()
    }

    pub fn detail(&self) -> Option<&IssueDetail> {
        self.detail.as_ref()
    }

    /// 탭 제목용 문구(`#123 제목`).
    pub fn title(&self) -> String {
        match &self.detail {
            Some(d) => format!("#{} {}", d.number, d.title),
            None => kiln_common::trf!("이슈 #{}", self.number),
        }
    }

    pub fn refresh(&mut self) {
        self.started = false;
    }

    pub fn comment_draft(&self) -> &str { &self.comment }

    pub fn comment_mut(&mut self) -> &mut String {
        &mut self.comment
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started
            || self.load.as_mut().is_some_and(|t| t.is_pending())
            || self.action.as_mut().is_some_and(|(_, t)| t.is_pending())
            || self.meta.is_loading()
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    fn now(&self) -> i64 {
        self.now_override.unwrap_or_else(now_unix)
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            let (b, r, n) = (self.backend.clone(), self.repo.clone(), self.number);
            self.load = Some(Task::spawn(ctx, move || b.issue(r.as_ref(), n)));
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            match r {
                Ok(d) => {
                    self.body = Markdown::parse(&d.body);
                    self.comments = d.comments.iter().map(|c| Markdown::parse(&c.body)).collect();
                    self.detail = Some(d);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some((kind, t)) = &mut self.action
            && let Some(r) = t.take()
        {
            let kind = *kind;
            self.action = None;
            match r {
                Ok(()) => {
                    let title = match kind {
                        ActionKind::Comment => kiln_common::i18n::tr("댓글을 남겼습니다"),
                        ActionKind::Close => kiln_common::i18n::tr("이슈를 닫았습니다"),
                        ActionKind::Reopen => kiln_common::i18n::tr("이슈를 다시 열었습니다"),
                        ActionKind::Edit => kiln_common::i18n::tr("이슈를 수정했습니다"),
                    };
                    if kind == ActionKind::Comment && self.submitted_body.take().as_deref() == Some(self.comment.as_str()) {
                        self.comment.clear();
                    }
                    self.action_msg = Some((BannerKind::Success, title.into(), None));
                    self.started = false;
                }
                Err(e) => self.action_msg = Some((BannerKind::Error, match kind { ActionKind::Comment => kiln_common::i18n::tr("댓글 전송 실패"), ActionKind::Close => kiln_common::i18n::tr("이슈 닫기 실패"), ActionKind::Reopen => kiln_common::i18n::tr("이슈 다시 열기 실패"), ActionKind::Edit => kiln_common::i18n::tr("이슈 수정 실패") }.into(), Some(e.to_string()))),
            }
        }
    }

    fn run(&mut self, ctx: &egui::Context, kind: ActionKind, f: impl FnOnce(&dyn GithubBackend, Option<&RepoRef>) -> GitResult<()> + Send + 'static) {
        let (b, r) = (self.backend.clone(), self.repo.clone());
        self.submitted_body = (kind == ActionKind::Comment).then(|| self.comment.clone());
        self.action_msg = None;
        self.action = Some((kind, Task::spawn(ctx, move || f(b.as_ref(), r.as_ref()))));
    }

    /// 뷰를 그린다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<GitEvent> {
        self.widget_scope = ui.make_persistent_id(("issue-view", &self.repo, self.number));
        let mut events = Vec::new();
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            if let Some(e) = self.error.clone() {
                egui::Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    if gh_error_state(ui, &e, &kiln_common::trf!("이슈 #{}", self.number), &mut events) == ErrorAction::Retry {
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
                    ui.label(dim(kiln_common::trf!("이슈 #{} 불러오는 중…", self.number)));
                });
                return;
            }
            self.ui_header(ui, &mut events);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, t.border);
            egui::ScrollArea::vertical().id_salt(("issue_conv", self.number)).auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 20 }).show(ui, |ui| {
                    let w = ui.available_width().min(1100.0);
                    if w >= 720.0 {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(w - SIDEBAR_W - 24.0);
                                self.ui_conversation(ui);
                            });
                            ui.add_space(18.0);
                            ui.vertical(|ui| {
                                ui.set_width(SIDEBAR_W);
                                self.ui_sidebar(ui);
                            });
                        });
                    } else {
                        ui.set_max_width(w);
                        self.ui_sidebar(ui);
                        ui.add_space(14.0);
                        self.ui_conversation(ui);
                    }
                });
            });
        });
        self.finish_drafts(ui.ctx());
        events
    }

    fn ui_header(&mut self, ui: &mut Ui, events: &mut Vec<GitEvent>) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let busy = self.action.is_some();
        let state = d.issue_state();
        let now = self.now();
        egui::Frame::new().fill(t.bg).inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 14 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(RichText::new(&d.title).font(kiln_common::fonts::semibold(20.0)).color(t.text)).wrap());
                ui.label(RichText::new(format!("#{}", d.number)).size(20.0).color(t.text_faint));
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let (icon, c) = issue_state_style(state);
                state_pill(ui, icon, state.label(), c);
                ui.add_space(4.0);
                ui.label(RichText::new(&d.author.login).font(kiln_common::fonts::medium(13.0)).color(t.text));
                let when = parse_iso8601(&d.created_at).map(|ts| relative_time(ts, now)).unwrap_or_default();
                ui.label(dim(kiln_common::trf!("님이 {when}에 열었습니다 · 댓글 {}개", d.comments.len())));
                if let Some(r) = &self.repo {
                    ui.label(faint(format!("· {}", r.full_name())));
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if tool_button(ui, Some(Icon::External), kiln_common::i18n::tr("브라우저에서 열기")).clicked() && !d.url.is_empty() {
                    events.push(GitEvent::OpenUrl(d.url.clone()));
                }
                if icon_button(ui, Icon::Refresh, kiln_common::i18n::tr("새로 고침")).clicked() {
                    self.refresh();
                }
                if busy {
                    spinner(ui, 12.0);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if state.is_open() {
                            if tool_button(ui, Some(Icon::IssueClosed), kiln_common::i18n::tr("이슈 닫기")).on_hover_text(kiln_common::i18n::tr("완료로 닫기")).clicked() {
                                self.run(ui.ctx(), ActionKind::Close, move |b, repo| b.close_issue(repo, d.number, CloseReason::Completed));
                            }
                            if tool_button(ui, Some(Icon::Skip), kiln_common::i18n::tr("계획 없음으로 닫기")).clicked() {
                                self.run(ui.ctx(), ActionKind::Close, move |b, repo| b.close_issue(repo, d.number, CloseReason::NotPlanned));
                            }
                        } else if tool_button(ui, Some(Icon::Issue), kiln_common::i18n::tr("다시 열기")).clicked() {
                            self.run(ui.ctx(), ActionKind::Reopen, move |b, repo| b.reopen_issue(repo, d.number));
                        }
                    });
                });
            });
            if let Some((k, title, detail)) = self.action_msg.clone() {
                ui.add_space(8.0);
                if banner(ui, k, &title, detail.as_deref(), true) {
                    self.action_msg = None;
                }
            }
        });
    }

    fn ui_conversation(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let now = self.now();
        comment_card(ui, &d.author.login, parse_iso8601(&d.created_at).unwrap_or(0), now, kiln_common::i18n::tr("님이 이 이슈를 열었습니다"), t.accent, |ui| {
            if self.body.is_empty() {
                ui.label(faint(kiln_common::i18n::tr("설명이 없습니다.")));
            } else {
                self.body.show(ui);
            }
        });
        for (c, md) in d.comments.iter().zip(&self.comments) {
            ui.add_space(10.0);
            comment_card(ui, &c.author.login, parse_iso8601(&c.created_at).unwrap_or(0), now, kiln_common::i18n::tr("님이 댓글을 남겼습니다"), t.border, |ui| {
                if md.is_empty() {
                    ui.label(faint(kiln_common::i18n::tr("내용이 없습니다.")));
                } else {
                    md.show(ui);
                }
            });
        }
        if !d.issue_state().is_open() && !d.closed_at.is_empty() {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let (icon, c) = issue_state_style(d.issue_state());
                icon_label(ui, icon, c, 14.0);
                let when = parse_iso8601(&d.closed_at).map(|ts| relative_time(ts, now)).unwrap_or_default();
                ui.label(dim(kiln_common::trf!("{when}에 {}(으)로 닫혔습니다", d.issue_state().label())));
            });
        }
        // 댓글 입력
        ui.add_space(16.0);
        egui::Frame::new()
            .fill(t.bg_panel)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    icon_label(ui, Icon::Comment, t.text_dim, 14.0);
                    ui.label(RichText::new(kiln_common::i18n::tr("댓글 작성")).font(kiln_common::fonts::semibold(13.5)).color(t.text));
                });
                ui.add_space(8.0);
                let cid = self.widget_scope.with("kiln_issue_comment");
                let focused = ui.memory(|m| m.has_focus(cid));
                ui.add(
                    egui::TextEdit::multiline(&mut self.comment)
                        .id(cid)
                        .hint_text(kiln_common::i18n::tr("댓글 남기기 (Markdown 지원)"))
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .frame(kiln_common::widgets::input_frame(focused, false)),
                );
                ui.add_space(10.0);
                let busy = self.action.is_some();
                let has_body = !self.comment.trim().is_empty();
                let cmd_enter = focused && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = d.number;
                    let r = ui.add_enabled_ui(!busy && has_body, |ui| primary_button(ui, kiln_common::i18n::tr("댓글"), Some(80.0))).inner;
                    if (r.clicked() || (cmd_enter && has_body && !busy)) && has_body {
                        let body = self.comment.clone();
                        self.run(ui.ctx(), ActionKind::Comment, move |b, repo| b.comment_issue(repo, n, &body));
                    }
                    ui.label(faint("⌘ Enter"));
                });
            });
    }

    fn ui_sidebar(&mut self, ui: &mut Ui) {
        let t = theme();
        let Some(d) = self.detail.clone() else { return };
        let busy = self.action.is_some();
        // 담당자
        let r = ui.add_enabled_ui(!busy, |ui| sidebar_heading(ui, kiln_common::i18n::tr("담당자"), true)).inner;
        if let Some(r) = r {
            let pid = self.widget_scope.with("issue_assignees_pick");
            if r.clicked() {
                egui::Popup::toggle_id(ui.ctx(), pid);
                if self.users_draft.is_none() {
                    let cur: Vec<String> = d.assignees.iter().map(|u| u.login.clone()).collect();
                    self.users_draft = Some(Draft { before: cur.clone(), after: cur, pick: PickerState { filter: String::new(), focus: true } });
                }
            }
            self.meta_pump_if_needed(ui.ctx());
            if let Some(dr) = &mut self.users_draft {
                multi_pick_popup(
                    ui,
                    pid,
                    r.rect.with_min_x(r.rect.right() - 280.0),
                    &mut dr.pick,
                    kiln_common::i18n::tr("담당자 검색…"),
                    &user_options(&self.meta.users),
                    &mut dr.after,
                    self.meta.users_loading(),
                    self.meta.error.as_deref(),
                );
            }
        }
        ui.add_space(4.0);
        let shown: Vec<String> = match &self.users_draft {
            Some(dr) => dr.after.clone(),
            None => d.assignees.iter().map(|u| u.login.clone()).collect(),
        };
        if shown.is_empty() {
            ui.label(faint(kiln_common::i18n::tr("아무도 없음")));
        }
        for a in &shown {
            person_row(ui, a);
        }
        sidebar_divider(ui);
        // 라벨
        let r = ui.add_enabled_ui(!busy, |ui| sidebar_heading(ui, kiln_common::i18n::tr("라벨"), true)).inner;
        if let Some(r) = r {
            let pid = self.widget_scope.with("issue_labels_pick");
            if r.clicked() {
                egui::Popup::toggle_id(ui.ctx(), pid);
                if self.labels_draft.is_none() {
                    let cur: Vec<String> = d.labels.iter().map(|l| l.name.clone()).collect();
                    self.labels_draft = Some(Draft { before: cur.clone(), after: cur, pick: PickerState { filter: String::new(), focus: true } });
                }
            }
            if let Some(dr) = &mut self.labels_draft {
                multi_pick_popup(
                    ui,
                    pid,
                    r.rect.with_min_x(r.rect.right() - 280.0),
                    &mut dr.pick,
                    kiln_common::i18n::tr("라벨 검색…"),
                    &label_options(&self.meta.labels),
                    &mut dr.after,
                    self.meta.labels_loading(),
                    self.meta.error.as_deref(),
                );
            }
        }
        ui.add_space(4.0);
        let shown: Vec<(String, egui::Color32)> = match &self.labels_draft {
            Some(dr) => dr.after.iter().map(|n| (n.clone(), self.label_color(&d, n))).collect(),
            None => d.labels.iter().map(|l| (l.name.clone(), hex_color(&l.color))).collect(),
        };
        if shown.is_empty() {
            ui.label(faint(kiln_common::i18n::tr("없음")));
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                for (n, c) in &shown {
                    label_chip(ui, n, *c);
                }
            });
        }
        sidebar_divider(ui);
        ui.label(RichText::new(kiln_common::i18n::tr("참여자")).font(kiln_common::fonts::semibold(12.0)).color(t.text_dim));
        ui.add_space(4.0);
        let mut people: Vec<&str> = vec![d.author.login.as_str()];
        for c in &d.comments {
            if !people.contains(&c.author.login.as_str()) {
                people.push(&c.author.login);
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for p in people.iter().take(12) {
                let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                paint_avatar(ui.painter(), r.center(), 11.0, p);
                resp.on_hover_text(*p);
            }
        });
    }

    fn label_color(&self, d: &IssueDetail, name: &str) -> egui::Color32 {
        d.labels.iter().find(|l| l.name == name).map(|l| hex_color(&l.color)).unwrap_or_else(|| self.meta.label_color(name))
    }

    fn meta_pump_if_needed(&mut self, ctx: &egui::Context) {
        if self.labels_draft.is_some() || self.users_draft.is_some() {
            self.meta.pump(ctx, &self.backend, self.repo.as_ref());
        }
    }

    /// 편집 팝업이 닫혔으면 변경분을 `gh issue edit` 으로 반영한다.
    fn finish_drafts(&mut self, ctx: &egui::Context) {
        self.meta_pump_if_needed(ctx);
        let lid = self.widget_scope.with("issue_labels_pick");
        let uid = self.widget_scope.with("issue_assignees_pick");
        let mut edit = IssueEdit::default();
        if self.labels_draft.is_some() && !egui::Popup::is_id_open(ctx, lid) {
            let dr = self.labels_draft.take().unwrap();
            (edit.add_labels, edit.remove_labels) = IssueEdit::diff_labels(&dr.before, &dr.after);
        }
        if self.users_draft.is_some() && !egui::Popup::is_id_open(ctx, uid) {
            let dr = self.users_draft.take().unwrap();
            (edit.add_assignees, edit.remove_assignees) = IssueEdit::diff_labels(&dr.before, &dr.after);
        }
        if !edit.is_empty() {
            let n = self.number;
            self.run(ctx, ActionKind::Edit, move |b, repo| b.edit_issue(repo, n, &edit));
        }
    }

    /// 상태를 바꾸는 동작이 가능한지(테스트용 요약).
    #[doc(hidden)]
    pub fn state(&self) -> Option<IssueState> {
        self.detail.as_ref().map(|d| d.issue_state())
    }
}

fn sidebar_divider(ui: &mut Ui) {
    ui.add_space(12.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, theme().border);
    ui.add_space(12.0);
}

#[cfg(test)]
mod pending_draft_tests {
    use super::*;
    #[test]
    fn successful_delayed_submission_preserves_newer_text() {
        for edit_while_sending in [false,true] {
            let ctx=egui::Context::default(); let mut view=IssueView::new(PathBuf::from("."), None, 1);
            view.started=true;view.comment="submitted body".into();
            let (tx,rx)=std::sync::mpsc::channel();
            view.run(&ctx,ActionKind::Comment,move |_, _|{rx.recv().unwrap();Ok(())});
            if edit_while_sending {view.comment="submitted body plus next draft".into();}
            tx.send(()).unwrap();
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
            while view.action.is_some() && std::time::Instant::now()<deadline {view.pump(&ctx);std::thread::sleep(std::time::Duration::from_millis(1));}
            assert!(view.action.is_none());
            assert_eq!(view.comment,if edit_while_sending {"submitted body plus next draft"}else{""});
        }
    }
}
