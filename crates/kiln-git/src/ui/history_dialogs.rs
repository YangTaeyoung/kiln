//! 이력 화면 대화상자: 확인, 리셋, 브랜치·태그 이름, 커밋 메시지, 대화형 리베이스.

use std::path::Path;

use egui::{Align, Align2, Color32, CornerRadius, Id, Key, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use kiln_common::icons::{self as kicons, Icon};
use kiln_common::widgets::{ButtonKind, button_with, tint};
use kiln_common::{Task, Theme, fonts};

use super::{Op, elide, menu_item, short};
use crate::cmd::GitResult;
use crate::history::{RebaseAction, RebasePlan, ResetMode};

/// 열린 대화상자.
pub(crate) enum Dialog {
    Confirm(ConfirmDialog),
    Reset(ResetDialog),
    Name(NameDialog),
    Message(MessageDialog),
    Rebase(Box<RebaseDialog>),
}

/// 대화상자 한 프레임의 결과.
pub(crate) enum DialogOutcome {
    Open,
    Cancel,
    Run { op: Op, autostash: bool, pushed: bool },
}

impl Dialog {
    pub(crate) fn is_loading(&mut self) -> bool {
        match self {
            Dialog::Message(m) => m.task.as_mut().is_some_and(|t| t.is_pending()),
            Dialog::Rebase(r) => r.task.as_mut().is_some_and(|t| t.is_pending()),
            _ => false,
        }
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, root: &Path) -> DialogOutcome {
        let id = Id::new(("kiln-history-dialog", root));
        match self {
            Dialog::Confirm(d) => d.show(ctx, id),
            Dialog::Reset(d) => d.show(ctx, id),
            Dialog::Name(d) => d.show(ctx, id),
            Dialog::Message(d) => d.show(ctx, id),
            Dialog::Rebase(d) => d.show(ctx, id),
        }
    }
}

fn modal_frame() -> egui::Frame {
    let t = Theme::current();
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(14))
        .shadow(t.shadow())
        .inner_margin(Margin::same(22))
}

/// 모달을 띄우고 본문 결과를 돌려준다. 바깥 클릭이나 Esc 면 `Cancel`.
fn modal(ctx: &egui::Context, id: Id, width: f32, body: impl FnOnce(&mut Ui) -> DialogOutcome) -> DialogOutcome {
    let mut out = DialogOutcome::Open;
    let resp = egui::Modal::new(id).frame(modal_frame()).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        out = body(ui);
    });
    if matches!(out, DialogOutcome::Open) && resp.should_close() {
        out = DialogOutcome::Cancel;
    }
    out
}

fn title(ui: &mut Ui, text: &str, sub: Option<&str>) {
    let t = Theme::current();
    ui.label(RichText::new(text).font(fonts::semibold(16.0)).color(t.text));
    if let Some(s) = sub {
        ui.add(egui::Label::new(RichText::new(s).font(fonts::regular(12.5)).color(t.text_faint)).truncate());
    }
    ui.add_space(8.0);
}

fn body_text(ui: &mut Ui, text: &str) {
    let t = Theme::current();
    ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(13.0)).color(t.text_dim)).wrap());
}

/// 색 테두리 안내 상자.
fn callout(ui: &mut Ui, c: Color32, icon: Icon, head: &str, text: &str) {
    let t = Theme::current();
    egui::Frame::new()
        .fill(tint(c, if t.dark { 0.10 } else { 0.07 }))
        .stroke(Stroke::new(1.0, tint(c, 0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                kicons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), icon, c);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(head).font(fonts::semibold(12.5)).color(t.text));
                    ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(12.5)).color(t.text_dim)).wrap());
                });
            });
        });
}

fn pushed_callout(ui: &mut Ui) {
    let t = Theme::current();
    callout(
        ui,
        t.yellow,
        Icon::Warning,
        "이미 푸시된 커밋이 포함되어 있습니다",
        "이력을 다시 쓰면 원격 브랜치와 달라집니다. 완료 후 강제 푸시(--force-with-lease)가 필요하며, 같은 브랜치를 쓰는 사람에게 영향을 줄 수 있습니다.",
    );
}

/// 테마 색 체크박스 행. 값이 바뀌면 true.
fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> bool {
    let t = Theme::current();
    let g = ui.painter().layout(label.to_string(), fonts::regular(13.0), t.text, ui.available_width() - 26.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, g.size().y.max(18.0) + 2.0), Sense::click());
    let v = *value;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, v, label));
    let b = Rect::from_center_size(pos2(rect.left() + 8.0, rect.top() + 10.0), vec2(15.0, 15.0));
    let border = if v { t.accent } else if resp.hovered() { t.text_faint } else { t.border_strong };
    ui.painter().rect(b, CornerRadius::same(4), if v { t.accent } else { t.bg_input }, Stroke::new(1.0, border), StrokeKind::Inside);
    if v {
        kicons::paint(ui.painter(), b.shrink(2.0), Icon::Check, t.accent_fg);
    }
    ui.painter().galley(pos2(rect.left() + 24.0, rect.top() + 1.0), g, t.text);
    if resp.clicked() {
        *value = !*value;
        return true;
    }
    false
}

fn autostash_row(ui: &mut Ui, dirty: bool, autostash: &mut bool) {
    if !dirty {
        return;
    }
    let t = Theme::current();
    checkbox(ui, autostash, "커밋되지 않은 변경을 자동으로 stash 후 복원 (--autostash)");
    if !*autostash {
        ui.label(RichText::new("변경 사항이 있으면 git이 작업을 거부할 수 있습니다.").font(fonts::regular(12.0)).color(t.text_faint));
    }
}

/// 오른쪽 정렬 버튼 줄. (확인 클릭, 취소 클릭).
fn footer(ui: &mut Ui, confirm: &str, kind: ButtonKind, enabled: bool) -> (bool, bool) {
    let mut ok = false;
    let mut cancel = false;
    ui.add_space(14.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if button_with(ui, None, confirm, kind, false).clicked() {
                ok = true;
            }
        });
        if button_with(ui, None, "취소", ButtonKind::Secondary, false).clicked() {
            cancel = true;
        }
    });
    (ok, cancel)
}

// ---------------------------------------------------------------- 확인

pub(crate) struct ConfirmDialog {
    pub op: Op,
    pub title: String,
    pub body: String,
    pub confirm: String,
    pub danger: bool,
    pub pushed: bool,
    pub dirty: bool,
    pub autostash: bool,
}

impl ConfirmDialog {
    pub(crate) fn plain(op: Op, title: &str, body: String, confirm: &str) -> Self {
        Self { op, title: title.into(), body, confirm: confirm.into(), danger: true, pushed: false, dirty: false, autostash: false }
    }

    /// 재작성 작업 확인(원격 커밋 경고·autostash 선택).
    pub(crate) fn rewrite(op: Op, pushed: bool, dirty: bool) -> Self {
        let (title, body, confirm) = match &op {
            Op::Move { moving, .. } => (
                "커밋 순서를 바꿀까요?",
                format!("커밋 {}개를 옮기고 이후 커밋을 다시 적용합니다.", moving.len()),
                "순서 이동",
            ),
            Op::Fixup { moving, target } => (
                "커밋을 합칠까요?",
                format!("커밋 {}개를 {}에 픽스업으로 합칩니다. 대상 커밋의 메시지를 유지합니다.", moving.len(), short(target)),
                "합치기",
            ),
            _ => ("이력을 다시 쓸까요?", "선택한 작업으로 현재 브랜치의 이력을 다시 씁니다.".to_string(), "계속"),
        };
        Self { op, title: title.into(), body, confirm: confirm.into(), danger: false, pushed, dirty, autostash: dirty }
    }

    fn show(&mut self, ctx: &egui::Context, id: Id) -> DialogOutcome {
        modal(ctx, id, 420.0, |ui| {
            title(ui, &self.title, None);
            body_text(ui, &self.body);
            if self.pushed {
                ui.add_space(8.0);
                pushed_callout(ui);
            }
            if self.dirty {
                ui.add_space(8.0);
                autostash_row(ui, true, &mut self.autostash);
            }
            let kind = if self.danger { ButtonKind::Danger } else { ButtonKind::Primary };
            let (ok, cancel) = footer(ui, &self.confirm, kind, true);
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            if ok || enter {
                DialogOutcome::Run { op: self.op.clone(), autostash: self.autostash, pushed: self.pushed }
            } else if cancel {
                DialogOutcome::Cancel
            } else {
                DialogOutcome::Open
            }
        })
    }
}

// ---------------------------------------------------------------- 리셋

pub(crate) struct ResetDialog {
    pub sha: String,
    pub subject: String,
    pub branch: Option<String>,
    pub mode: ResetMode,
    pub dirty: bool,
    pub pushed: bool,
}

impl ResetDialog {
    fn show(&mut self, ctx: &egui::Context, id: Id) -> DialogOutcome {
        let t = Theme::current();
        modal(ctx, id, 460.0, |ui| {
            let who = self.branch.as_deref().map(|b| format!("'{b}'을(를)")).unwrap_or_else(|| "HEAD를".into());
            title(ui, &format!("{who} 여기로 리셋"), Some(&format!("{} · {}", short(&self.sha), self.subject)));
            let options = [
                (ResetMode::Soft, "Soft", "커밋만 되돌립니다. 이후 커밋의 변경 내용은 스테이지된 상태로 남습니다."),
                (ResetMode::Mixed, "Mixed", "커밋과 스테이지를 되돌립니다. 변경 내용은 작업 트리에 그대로 남습니다."),
                (ResetMode::Hard, "Hard", "커밋·스테이지·작업 트리를 모두 이 커밋 상태로 맞춥니다."),
            ];
            for (m, name, desc) in options {
                if mode_card(ui, self.mode == m, name, desc, m == ResetMode::Hard) {
                    self.mode = m;
                }
                ui.add_space(2.0);
            }
            if self.mode == ResetMode::Hard {
                ui.add_space(6.0);
                let extra = if self.dirty { " 지금 작업 트리에 커밋되지 않은 변경이 있습니다." } else { "" };
                callout(
                    ui,
                    t.red,
                    Icon::Warning,
                    "데이터가 사라질 수 있습니다",
                    &format!(
                        "커밋되지 않은 변경은 복구할 수 없습니다. 이후 커밋은 되돌리기나 reflog로만 되살릴 수 있습니다.{extra}"
                    ),
                );
            }
            if self.pushed {
                ui.add_space(6.0);
                pushed_callout(ui);
            }
            let kind = if self.mode == ResetMode::Hard { ButtonKind::Danger } else { ButtonKind::Primary };
            let label = match self.mode {
                ResetMode::Soft => "Soft 리셋",
                ResetMode::Mixed => "Mixed 리셋",
                ResetMode::Hard => "Hard 리셋",
            };
            let (ok, cancel) = footer(ui, label, kind, true);
            if ok {
                DialogOutcome::Run { op: Op::Reset(self.sha.clone(), self.mode), autostash: false, pushed: self.pushed }
            } else if cancel {
                DialogOutcome::Cancel
            } else {
                DialogOutcome::Open
            }
        })
    }
}

/// 라디오 카드. 클릭되면 true.
fn mode_card(ui: &mut Ui, selected: bool, name: &str, desc: &str, danger: bool) -> bool {
    let t = Theme::current();
    let w = ui.available_width();
    let dg = ui.painter().layout(desc.to_string(), fonts::regular(12.5), t.text_dim, w - 48.0);
    let h = dg.size().y + 34.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, name));
    let accent = if danger { t.red } else { t.accent };
    let (fill, stroke) = if selected {
        (tint(accent, if t.dark { 0.10 } else { 0.06 }), tint(accent, 0.6))
    } else if resp.hovered() {
        (t.bg_hover, t.border_strong)
    } else {
        (Color32::TRANSPARENT, t.border)
    };
    ui.painter().rect(r, CornerRadius::same(9), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let c = pos2(r.left() + 18.0, r.top() + 17.0);
    ui.painter().circle(c, 7.0, t.bg_input, Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { accent } else { t.border_strong }));
    if selected {
        ui.painter().circle_filled(c, 3.8, accent);
    }
    ui.painter().text(pos2(r.left() + 34.0, c.y), Align2::LEFT_CENTER, name, fonts::semibold(13.5), if danger && selected { t.red } else { t.text });
    ui.painter().galley(pos2(r.left() + 34.0, r.top() + 27.0), dg, t.text_dim);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

// ---------------------------------------------------------------- 이름 입력

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameKind {
    Branch,
    Tag,
}

pub(crate) struct NameDialog {
    kind: NameKind,
    sha: String,
    name: String,
    message: String,
    switch: bool,
    focused: bool,
}

impl NameDialog {
    pub(crate) fn new(kind: NameKind, sha: String) -> Self {
        Self { kind, sha, name: String::new(), message: String::new(), switch: true, focused: false }
    }

    fn show(&mut self, ctx: &egui::Context, id: Id) -> DialogOutcome {
        let t = Theme::current();
        modal(ctx, id, 420.0, |ui| {
            let (head, hint) = match self.kind {
                NameKind::Branch => ("새 브랜치", "feature/my-change"),
                NameKind::Tag => ("새 태그", "v1.2.0"),
            };
            title(ui, head, Some(&format!("{}에서 만듭니다", short(&self.sha))));
            let name_id = id.with("name");
            let focused = ui.memory(|m| m.has_focus(name_id));
            let bad = self.name.contains(char::is_whitespace);
            kiln_common::widgets::input_frame(focused, bad).show(ui, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .id(name_id)
                        .frame(egui::Frame::NONE)
                        .hint_text(RichText::new(hint).color(t.text_faint))
                        .font(fonts::regular(13.5))
                        .desired_width(f32::INFINITY),
                );
                if !self.focused {
                    r.request_focus();
                    self.focused = true;
                }
            });
            if bad {
                ui.label(RichText::new("이름에 공백을 쓸 수 없습니다").font(fonts::regular(12.0)).color(t.red));
            }
            ui.add_space(4.0);
            match self.kind {
                NameKind::Branch => {
                    checkbox(ui, &mut self.switch, "만든 뒤 체크아웃");
                }
                NameKind::Tag => {
                    ui.label(RichText::new("메시지 (입력하면 주석 태그)").font(fonts::medium(12.0)).color(t.text_faint));
                    kiln_common::widgets::input_frame(false, false).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.message)
                                .frame(egui::Frame::NONE)
                                .font(fonts::regular(13.0))
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                    });
                }
            }
            let valid = !self.name.trim().is_empty() && !bad;
            let label = if self.kind == NameKind::Branch { "브랜치 만들기" } else { "태그 만들기" };
            let (ok, cancel) = footer(ui, label, ButtonKind::Primary, valid);
            let enter = valid && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift) && self.kind == NameKind::Branch;
            if ok || enter {
                let op = match self.kind {
                    NameKind::Branch => Op::Branch { name: self.name.trim().into(), sha: self.sha.clone(), switch: self.switch },
                    NameKind::Tag => Op::Tag { name: self.name.trim().into(), sha: self.sha.clone(), message: self.message.clone() },
                };
                DialogOutcome::Run { op, autostash: false, pushed: false }
            } else if cancel {
                DialogOutcome::Cancel
            } else {
                DialogOutcome::Open
            }
        })
    }
}

// ---------------------------------------------------------------- 메시지

pub(crate) enum MessageKind {
    Reword(String),
    /// 표 순서(새 커밋 먼저).
    Squash(Vec<String>),
}

pub(crate) struct MessageDialog {
    kind: MessageKind,
    pub text: String,
    task: Option<Task<GitResult<String>>>,
    error: Option<String>,
    pushed: bool,
    dirty: bool,
    autostash: bool,
}

impl MessageDialog {
    pub(crate) fn new(kind: MessageKind, task: Task<GitResult<String>>, pushed: bool, dirty: bool) -> Self {
        Self { kind, text: String::new(), task: Some(task), error: None, pushed, dirty, autostash: dirty }
    }

    fn show(&mut self, ctx: &egui::Context, id: Id) -> DialogOutcome {
        let t = Theme::current();
        if let Some(task) = &mut self.task
            && let Some(r) = task.take()
        {
            self.task = None;
            match r {
                Ok(s) => self.text = s,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        modal(ctx, id, 540.0, |ui| {
            let (head, sub, confirm) = match &self.kind {
                MessageKind::Reword(sha) => ("커밋 메시지 수정", short(sha).to_string(), "메시지 수정"),
                MessageKind::Squash(shas) => (
                    "하나로 스쿼시",
                    format!("커밋 {}개 · {} … {}", shas.len(), short(shas.last().map(String::as_str).unwrap_or("")), short(&shas[0])),
                    "스쿼시",
                ),
            };
            title(ui, head, Some(&sub));
            if self.task.is_some() {
                ui.add(egui::Spinner::new().size(16.0).color(t.text_faint));
                return DialogOutcome::Open;
            }
            if let Some(e) = &self.error {
                callout(ui, t.red, Icon::Warning, "메시지를 읽지 못했습니다", e);
            }
            let focused = ui.memory(|m| m.has_focus(id.with("msg")));
            kiln_common::widgets::input_frame(focused, false).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.text)
                            .id(id.with("msg"))
                            .frame(egui::Frame::NONE)
                            .font(fonts::regular(13.5))
                            .desired_rows(9)
                            .desired_width(f32::INFINITY)
                            .lock_focus(true),
                    );
                });
            });
            let subject_len = self.text.lines().next().map(|l| l.chars().count()).unwrap_or(0);
            ui.horizontal(|ui| {
                let c = if subject_len > 72 { t.yellow } else { t.text_faint };
                ui.label(RichText::new(format!("제목 {subject_len}자")).font(fonts::regular(12.0)).color(c));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("첫 줄이 제목, 빈 줄 뒤가 본문입니다").font(fonts::regular(12.0)).color(t.text_faint));
                });
            });
            if self.pushed {
                ui.add_space(4.0);
                pushed_callout(ui);
            }
            autostash_row(ui, self.dirty, &mut self.autostash);
            let valid = !self.text.trim().is_empty();
            let (ok, cancel) = footer(ui, confirm, ButtonKind::Primary, valid);
            let submit = valid && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command);
            if ok || submit {
                let op = match &self.kind {
                    MessageKind::Reword(sha) => Op::Reword { sha: sha.clone(), message: self.text.clone() },
                    MessageKind::Squash(shas) => Op::Squash { shas: shas.clone(), message: self.text.clone() },
                };
                DialogOutcome::Run { op, autostash: self.autostash, pushed: self.pushed }
            } else if cancel {
                DialogOutcome::Cancel
            } else {
                DialogOutcome::Open
            }
        })
    }
}

// ---------------------------------------------------------------- 대화형 리베이스

pub(crate) struct RebaseDialog {
    task: Option<Task<GitResult<RebasePlan>>>,
    plan: Option<RebasePlan>,
    original: Vec<String>,
    error: Option<String>,
    pushed: bool,
    dirty: bool,
    autostash: bool,
    selected: Option<usize>,
    drag: Option<usize>,
    drop_at: Option<usize>,
    /// 불러올 때의 커밋 메시지.
    messages: std::collections::HashMap<String, String>,
    /// 사용자가 직접 고친 메시지의 커밋.
    edited: std::collections::HashSet<String>,
}

fn action_color(a: RebaseAction) -> Color32 {
    let t = Theme::current();
    match a {
        RebaseAction::Pick => t.text_dim,
        RebaseAction::Reword => t.blue,
        RebaseAction::Edit => t.orange,
        RebaseAction::Squash | RebaseAction::Fixup => t.purple,
        RebaseAction::Drop => t.red,
    }
}

fn action_key(a: RebaseAction) -> &'static str {
    match a {
        RebaseAction::Pick => "P",
        RebaseAction::Reword => "R",
        RebaseAction::Edit => "E",
        RebaseAction::Squash => "S",
        RebaseAction::Fixup => "F",
        RebaseAction::Drop => "D",
    }
}

impl RebaseDialog {
    pub(crate) fn new(task: Task<GitResult<RebasePlan>>, pushed: bool, dirty: bool) -> Self {
        Self {
            task: Some(task),
            plan: None,
            original: Vec::new(),
            error: None,
            pushed,
            dirty,
            autostash: dirty,
            selected: None,
            drag: None,
            drop_at: None,
            messages: Default::default(),
            edited: Default::default(),
        }
    }

    fn set_action(&mut self, i: usize, a: RebaseAction) {
        if let Some(s) = self.plan.as_mut().and_then(|p| p.steps.get_mut(i)) {
            s.action = a;
        }
    }

    /// 사용자가 고치지 않은 메시지를 현재 동작·순서에 맞는 기본값으로 다시 채운다.
    /// 스쿼시는 합쳐질 앞 커밋들(픽스업·삭제 제외)과 자기 메시지를 이어 붙인다.
    fn refresh_defaults(&mut self) {
        let Some(plan) = &mut self.plan else { return };
        let orig = |sha: &str| self.messages.get(sha).cloned().unwrap_or_default();
        for i in 0..plan.steps.len() {
            if self.edited.contains(&plan.steps[i].sha) {
                continue;
            }
            let msg = if plan.steps[i].action == RebaseAction::Squash {
                let mut msgs: Vec<String> = Vec::new();
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    let p = &plan.steps[k];
                    if p.action == RebaseAction::Drop {
                        continue;
                    }
                    if p.action != RebaseAction::Fixup {
                        msgs.insert(0, orig(&p.sha));
                    }
                    if !p.action.melds() {
                        break;
                    }
                }
                msgs.push(orig(&plan.steps[i].sha));
                msgs.join("\n\n")
            } else {
                orig(&plan.steps[i].sha)
            };
            plan.steps[i].message = msg;
        }
    }

    fn show(&mut self, ctx: &egui::Context, id: Id) -> DialogOutcome {
        let t = Theme::current();
        if let Some(task) = &mut self.task
            && let Some(r) = task.take()
        {
            self.task = None;
            match r {
                Ok(p) => {
                    self.original = p.steps.iter().map(|s| s.sha.clone()).collect();
                    self.messages = p.steps.iter().map(|s| (s.sha.clone(), s.message.clone())).collect();
                    self.selected = (!p.steps.is_empty()).then_some(0);
                    self.plan = Some(p);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.handle_keys(ctx);
        self.refresh_defaults();
        let width = (ctx.content_rect().width() - 80.0).clamp(560.0, 900.0);
        modal(ctx, id, width, |ui| {
            let n = self.plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
            let base = self.plan.as_ref().map(|p| p.base.as_deref().map(short).unwrap_or("루트").to_string()).unwrap_or_default();
            title(ui, "대화형 리베이스", Some(&format!("{base} 위에 커밋 {n}개 · 위에서부터 차례로 적용")));
            if self.task.is_some() {
                ui.add(egui::Spinner::new().size(16.0).color(t.text_faint));
                return DialogOutcome::Open;
            }
            if let Some(e) = self.error.clone() {
                callout(ui, t.red, Icon::Warning, "리베이스를 준비하지 못했습니다", &e);
                let (_, cancel) = footer(ui, "리베이스 시작", ButtonKind::Primary, false);
                return if cancel { DialogOutcome::Cancel } else { DialogOutcome::Open };
            }
            let preview_w = 250.0;
            let max_h = (ctx.content_rect().height() - 330.0).clamp(180.0, 460.0);
            let editor_h = if self.selected.and_then(|i| self.plan.as_ref()?.steps.get(i)).is_some_and(|s| {
                matches!(s.action, RebaseAction::Reword | RebaseAction::Squash)
            }) {
                96.0
            } else {
                0.0
            };
            let preview_rows = self.plan.as_ref().map(|p| p.preview().len()).unwrap_or(0);
            let list_h = (n as f32 * 38.0 + 12.0 + editor_h).max(84.0 + preview_rows as f32 * 30.0).clamp(180.0, max_h);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let list_w = ui.available_width() - preview_w - 16.0;
                ui.allocate_ui(vec2(list_w, list_h), |ui| {
                    ui.set_width(list_w);
                    self.ui_list(ui, list_h);
                });
                ui.allocate_ui(vec2(preview_w, list_h), |ui| {
                    ui.set_width(preview_w);
                    self.ui_preview(ui, list_h);
                });
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new("손잡이를 끌어 순서를 바꾸세요. 단축키: P 유지 · R 메시지 수정 · E 편집 · S 스쿼시 · F 픽스업 · D 삭제 · Alt+↑↓ 이동")
                    .font(fonts::regular(11.5))
                    .color(t.text_faint),
            );
            let plan = self.plan.clone();
            let invalid = plan.as_ref().and_then(|p| p.validate().err());
            if let Some(e) = &invalid {
                ui.add_space(4.0);
                ui.label(RichText::new(e).font(fonts::medium(12.5)).color(t.red));
            }
            if self.pushed {
                ui.add_space(6.0);
                pushed_callout(ui);
            }
            if self.dirty {
                ui.add_space(4.0);
                autostash_row(ui, true, &mut self.autostash);
            }
            let (ok, cancel) = footer(ui, "리베이스 시작", ButtonKind::Primary, invalid.is_none() && plan.is_some());
            if ok && let Some(p) = plan {
                DialogOutcome::Run { op: Op::Rebase(p), autostash: self.autostash, pushed: self.pushed }
            } else if cancel {
                DialogOutcome::Cancel
            } else {
                DialogOutcome::Open
            }
        })
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let Some(sel) = self.selected else { return };
        let n = self.plan.as_ref().map(|p| p.steps.len()).unwrap_or(0);
        let keys = ctx.input(|i| {
            let mut v = Vec::new();
            for (k, a) in [
                (Key::P, RebaseAction::Pick),
                (Key::R, RebaseAction::Reword),
                (Key::E, RebaseAction::Edit),
                (Key::S, RebaseAction::Squash),
                (Key::F, RebaseAction::Fixup),
                (Key::D, RebaseAction::Drop),
            ] {
                if i.key_pressed(k) && !i.modifiers.command {
                    v.push(Some(a));
                }
            }
            (v, i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.modifiers.alt)
        });
        for a in keys.0.into_iter().flatten() {
            self.set_action(sel, a);
        }
        let (up, down, alt) = (keys.1, keys.2, keys.3);
        if alt && up && sel > 0 {
            self.move_row(sel, sel - 1);
        } else if alt && down && sel + 1 < n {
            self.move_row(sel, sel + 2);
        } else if up && sel > 0 {
            self.selected = Some(sel - 1);
        } else if down && sel + 1 < n {
            self.selected = Some(sel + 1);
        }
    }

    /// `from` 행을 `to`(삽입 위치, 0..=n) 앞으로 옮긴다.
    fn move_row(&mut self, from: usize, to: usize) {
        let Some(plan) = &mut self.plan else { return };
        if from >= plan.steps.len() || to > plan.steps.len() || to == from || to == from + 1 {
            return;
        }
        let s = plan.steps.remove(from);
        let at = if to > from { to - 1 } else { to };
        plan.steps.insert(at, s);
        self.selected = Some(at);
    }

    fn ui_list(&mut self, ui: &mut Ui, height: f32) {
        let t = Theme::current();
        let Some(plan) = self.plan.clone() else { return };
        let frame_rect = ui.available_rect_before_wrap();
        let frame_rect = Rect::from_min_size(frame_rect.min, vec2(ui.available_width(), height));
        ui.painter().rect(frame_rect, CornerRadius::same(10), t.bg, Stroke::new(1.0, t.border), StrokeKind::Inside);
        let mut rows: Vec<(usize, Rect)> = Vec::new();
        let mut new_action: Option<(usize, RebaseAction)> = None;
        let mut msg_edit: Option<(usize, String)> = None;
        let mut drag_start: Option<usize> = None;
        let mut click: Option<usize> = None;
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(frame_rect.shrink(4.0)).layout(Layout::top_down(Align::Min)));
        egui::ScrollArea::vertical().id_salt("rebase-list").auto_shrink([false, false]).max_height(height - 8.0).show(&mut inner, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let w = ui.available_width();
            for (i, s) in plan.steps.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, self.selected == Some(i), format!("리베이스: {}", s.subject)));
                rows.push((i, r));
                if resp.clicked() {
                    click = Some(i);
                }
                let sel = self.selected == Some(i);
                let dragging = self.drag == Some(i);
                if sel || dragging {
                    ui.painter().rect_filled(r, CornerRadius::same(7), t.accent_soft(if t.dark { 36 } else { 26 }));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(7), t.bg_hover);
                }
                // 손잡이
                let hr = Rect::from_min_size(r.min, vec2(24.0, r.height()));
                let handle = ui.interact(hr, Id::new(("rebase-handle", &s.sha)), Sense::drag());
                handle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("순서 변경: {}", s.subject)));
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                if handle.drag_started() {
                    drag_start = Some(i);
                }
                let gc = if handle.hovered() { t.text_dim } else { t.text_faint };
                for dx in [-2.5f32, 2.5] {
                    for dy in [-5.0f32, 0.0, 5.0] {
                        ui.painter().circle_filled(pos2(hr.center().x + dx, hr.center().y + dy), 1.3, gc);
                    }
                }
                // 동작 드롭다운
                let ar = Rect::from_min_size(pos2(r.left() + 28.0, r.center().y - 12.0), vec2(112.0, 24.0));
                let aresp = ui.interact(ar, Id::new(("rebase-action", &s.sha)), Sense::click());
                aresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, format!("동작: {}", s.subject)));
                let ac = action_color(s.action);
                let fill = if aresp.hovered() { tint(ac, 0.22) } else { tint(ac, if t.dark { 0.14 } else { 0.10 }) };
                ui.painter().rect(ar, CornerRadius::same(6), fill, Stroke::new(1.0, tint(ac, 0.35)), StrokeKind::Inside);
                ui.painter().text(pos2(ar.left() + 9.0, ar.center().y), Align2::LEFT_CENTER, s.action.label(), fonts::medium(12.0), if s.action == RebaseAction::Pick { t.text } else { ac });
                kicons::paint(ui.painter(), Rect::from_center_size(pos2(ar.right() - 11.0, ar.center().y), vec2(9.0, 9.0)), Icon::ChevronDown, t.text_faint);
                egui::Popup::menu(&aresp).width(190.0).show(|ui| {
                    ui.set_min_width(190.0);
                    for a in RebaseAction::ALL {
                        if menu_item(ui, a.label(), Some(action_key(a)), s.action == a, true).clicked() {
                            new_action = Some((i, a));
                            ui.close();
                        }
                    }
                });
                // 해시 + 제목
                let mut x = ar.right() + 12.0;
                ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, short(&s.sha), fonts::mono(11.5), t.text_faint);
                x += 72.0;
                if s.action.melds() {
                    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, "↳", fonts::regular(13.0), t.purple);
                    x += 16.0;
                }
                let dropped = s.action == RebaseAction::Drop;
                let shown = if s.action == RebaseAction::Reword {
                    s.message.lines().next().unwrap_or("").to_string()
                } else {
                    s.subject.clone()
                };
                let col = if dropped { t.text_faint } else { t.text };
                let g = elide(ui.painter(), &shown, fonts::regular(13.0), col, r.right() - x - 90.0);
                let gy = r.center().y - g.size().y / 2.0;
                let gw = g.size().x;
                ui.painter().galley(pos2(x, gy), g, col);
                if dropped {
                    ui.painter().hline(x..=x + gw, r.center().y, Stroke::new(1.2, t.text_faint));
                }
                let who = s.author.split_whitespace().next().unwrap_or("").to_string();
                let g = elide(ui.painter(), &who, fonts::regular(12.0), t.text_faint, 80.0);
                ui.painter().galley(pos2(r.right() - 10.0 - g.size().x, r.center().y - g.size().y / 2.0), g, t.text_faint);

                if sel && matches!(s.action, RebaseAction::Reword | RebaseAction::Squash) {
                    let mut m = s.message.clone();
                    let focused = ui.memory(|mm| mm.has_focus(Id::new(("rebase-msg", &s.sha))));
                    egui::Frame::new().inner_margin(Margin { left: 120, right: 8, top: 2, bottom: 6 }).show(ui, |ui| {
                        kiln_common::widgets::input_frame(focused, m.trim().is_empty()).show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut m)
                                    .id(Id::new(("rebase-msg", &s.sha)))
                                    .frame(egui::Frame::NONE)
                                    .font(fonts::regular(13.0))
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    });
                    if m != s.message {
                        msg_edit = Some((i, m));
                    }
                }
            }
        });
        if let Some(i) = click {
            self.selected = Some(i);
        }
        if let Some((i, a)) = new_action {
            self.selected = Some(i);
            self.set_action(i, a);
        }
        if let Some((i, m)) = msg_edit
            && let Some(p) = &mut self.plan
        {
            self.edited.insert(p.steps[i].sha.clone());
            p.steps[i].message = m;
        }
        if let Some(i) = drag_start {
            self.drag = Some(i);
            self.selected = Some(i);
        }
        if let Some(from) = self.drag {
            let (pos, down) = ui.input(|i| (i.pointer.hover_pos().or(i.pointer.interact_pos()), i.pointer.primary_down()));
            let mut at = None;
            if let Some(p) = pos {
                for &(i, r) in &rows {
                    if p.y < r.center().y {
                        at = Some(i);
                        break;
                    }
                }
                if at.is_none() && !rows.is_empty() {
                    at = Some(rows.len());
                }
            }
            self.drop_at = at;
            if let Some(a) = at
                && a != from
                && a != from + 1
            {
                let y = rows.get(a).map(|(_, r)| r.top() - 1.0).or_else(|| rows.last().map(|(_, r)| r.bottom() + 1.0));
                if let Some(y) = y {
                    let x0 = frame_rect.left() + 8.0;
                    ui.painter().line_segment([pos2(x0 + 4.0, y), pos2(frame_rect.right() - 8.0, y)], Stroke::new(2.0, t.accent));
                    ui.painter().circle(pos2(x0 + 2.0, y), 3.5, t.bg, Stroke::new(2.0, t.accent));
                }
            }
            if !down {
                self.drag = None;
                self.drop_at = None;
                if let Some(a) = at {
                    self.move_row(from, a);
                }
            } else {
                ui.ctx().request_repaint();
            }
        }
        ui.allocate_rect(frame_rect, Sense::hover());
    }

    fn ui_preview(&mut self, ui: &mut Ui, height: f32) {
        let t = Theme::current();
        let Some(plan) = &self.plan else { return };
        let r = ui.available_rect_before_wrap();
        let r = Rect::from_min_size(r.min, vec2(ui.available_width(), height));
        ui.painter().rect(r, CornerRadius::same(10), t.bg_panel, Stroke::new(1.0, t.border), StrokeKind::Inside);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(14.0, 12.0))).layout(Layout::top_down(Align::Min)));
        let pv = plan.preview();
        let dropped = plan.steps.iter().filter(|s| s.action == RebaseAction::Drop).count();
        let reordered = plan.steps.iter().map(|s| &s.sha).ne(self.original.iter());
        inner.label(RichText::new("결과 미리보기").font(fonts::semibold(12.5)).color(t.text_dim));
        let mut summary = format!("커밋 {}개 → {}개", plan.steps.len(), pv.len());
        if dropped > 0 {
            summary.push_str(&format!(" · 삭제 {dropped}"));
        }
        if reordered {
            summary.push_str(" · 순서 변경");
        }
        inner.label(RichText::new(summary).font(fonts::regular(12.0)).color(t.text_faint));
        inner.add_space(8.0);
        egui::ScrollArea::vertical().id_salt("rebase-preview").auto_shrink([false, false]).show(&mut inner, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            let count = pv.len();
            for (k, (lead, melded, subject)) in pv.iter().rev().enumerate() {
                let (row, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
                let cx = row.left() + 7.0;
                let s = &plan.steps[*lead];
                let c = if !melded.is_empty() { t.purple } else { action_color(s.action) };
                let c = if s.action == RebaseAction::Pick && melded.is_empty() { t.accent } else { c };
                if k + 1 < count {
                    ui.painter().vline(cx, row.center().y..=row.bottom(), Stroke::new(1.6, tint(t.accent, 0.45)));
                }
                if k > 0 {
                    ui.painter().vline(cx, row.top()..=row.center().y, Stroke::new(1.6, tint(t.accent, 0.45)));
                }
                ui.painter().circle(pos2(cx, row.center().y), 4.0, c, Stroke::new(1.5, t.bg_panel));
                let mut x = cx + 12.0;
                let tag = if !melded.is_empty() {
                    Some(format!("+{}", melded.len()))
                } else if s.action == RebaseAction::Edit {
                    Some("멈춤".to_string())
                } else if s.action == RebaseAction::Reword {
                    Some("수정".to_string())
                } else {
                    None
                };
                let tag_w = if let Some(tg) = &tag {
                    let g = ui.painter().layout_no_wrap(tg.clone(), fonts::medium(10.5), c);
                    let br = Rect::from_min_size(pos2(row.right() - g.size().x - 12.0, row.center().y - 8.0), vec2(g.size().x + 10.0, 16.0));
                    ui.painter().rect_filled(br, CornerRadius::same(8), tint(c, 0.16));
                    let gs = g.size();
                    ui.painter().galley(br.center() - gs / 2.0, g, c);
                    br.width() + 6.0
                } else {
                    0.0
                };
                let g = elide(ui.painter(), subject, fonts::regular(12.5), t.text, row.right() - x - tag_w);
                ui.painter().galley(pos2(x, row.center().y - g.size().y / 2.0), g, t.text);
                x += 0.0;
                let _ = x;
            }
            if count == 0 {
                ui.label(RichText::new("모든 커밋이 삭제됩니다").font(fonts::regular(12.5)).color(t.red));
            }
        });
        ui.allocate_rect(r, Sense::hover());
    }
}
