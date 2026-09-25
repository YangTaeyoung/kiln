//! Git UI 공용 위젯과 스타일.

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Margin, Response, RichText, Sense, Stroke, StrokeKind, Ui, vec2,
};
use kiln_common::widgets::{ButtonKind, button_with};
use kiln_common::{Theme, fonts};

pub(crate) use super::icons::{Icon, paint_icon};
use crate::gh::{ChecksState, PrState, ReviewDecision};

pub(crate) const ROW_H: f32 = 28.0;

pub(crate) fn theme() -> Theme {
    Theme::current()
}

/// 알파를 적용한 색.
pub(crate) fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

/// 상태 문자 색.
pub(crate) fn status_color(ch: char) -> Color32 {
    let t = theme();
    match ch {
        'M' => t.yellow,
        'A' | '?' => t.green,
        'D' => t.red,
        'R' => t.blue,
        'C' | 'U' => t.orange,
        'T' => t.purple,
        _ => t.text_dim,
    }
}

/// 둥근 알약 배지.
pub(crate) fn badge(ui: &mut Ui, text: impl Into<String>, fg: Color32, bg: Color32) -> Response {
    let text = text.into();
    let galley = ui.painter().layout_no_wrap(text, fonts::medium(11.0), fg);
    let size = vec2(galley.size().x + 14.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, fg);
    }
    resp
}

/// 옅은 바탕의 알약 배지(글자색으로 바탕을 물들인다).
pub(crate) fn outline_badge(ui: &mut Ui, text: impl Into<String>, fg: Color32) -> Response {
    let bg = alpha(fg, if theme().dark { 0.16 } else { 0.12 });
    badge(ui, text, fg, bg)
}

/// 테두리 없는 작은 아이콘 버튼.
pub(crate) fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    paint_icon_button(ui, rect, &resp, icon, theme().text_dim);
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    resp.on_hover_text(tooltip)
}

fn paint_icon_button(ui: &Ui, rect: egui::Rect, resp: &Response, icon: Icon, color: Color32) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let t = theme();
    let enabled = ui.is_enabled();
    if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.bg_hover);
    }
    let c = if !enabled {
        t.text_faint
    } else if resp.hovered() {
        t.text
    } else {
        color
    };
    paint_icon(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(14.0, 14.0)), icon, c);
}

/// 지정 위치에 아이콘 버튼을 둔다.
pub(crate) fn icon_button_at(ui: &mut Ui, rect: egui::Rect, id: Id, icon: Icon, tooltip: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    paint_icon_button(ui, rect, &resp, icon, theme().text_dim);
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tooltip));
    resp.on_hover_text(tooltip)
}

/// 폭을 지정할 수 있는 둥근 버튼.
fn sized_button(ui: &mut Ui, text: &str, kind: ButtonKind, width: Option<f32>) -> Response {
    match width {
        Some(w) => wide_button(ui, text, kind, w),
        None => button_with(ui, None, text, kind, false),
    }
}

/// 주어진 폭을 채우는 버튼.
fn wide_button(ui: &mut Ui, text: &str, kind: ButtonKind, w: f32) -> Response {
    let t = theme();
    let enabled = ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() && enabled;
        let (fill, fg) = match kind {
            ButtonKind::Primary => (if hovered { kiln_common::widgets::lerp_color(t.accent, Color32::WHITE, 0.08) } else { t.accent }, t.accent_fg),
            ButtonKind::Danger => (t.red, Color32::WHITE),
            _ => (if hovered { t.bg_hover } else { t.bg_elevated }, t.text),
        };
        let (fill, fg) = if enabled { (fill, fg) } else { (t.bg_hover, t.text_faint) };
        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
        if kind == ButtonKind::Secondary {
            ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
        }
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, fonts::medium(13.0), fg);
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    resp
}

/// 강조색으로 채운 기본 버튼.
pub(crate) fn primary_button(ui: &mut Ui, text: &str, width: Option<f32>) -> Response {
    sized_button(ui, text, ButtonKind::Primary, width)
}

/// 보조 버튼(테두리형).
pub(crate) fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    button_with(ui, None, text, ButtonKind::Secondary, false)
}

/// 위험 동작 버튼.
pub(crate) fn danger_button(ui: &mut Ui, text: &str) -> Response {
    button_with(ui, None, text, ButtonKind::Danger, false)
}

/// 작은 툴바 버튼(아이콘+텍스트). 테두리 없는 고스트 모양, 호버 시 채움.
pub(crate) fn tool_button(ui: &mut Ui, icon: Option<Icon>, text: &str) -> Response {
    let t = theme();
    let font = fonts::medium(12.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, t.text);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 18.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() && enabled;
        let fill = if hovered { t.bg_hover } else { t.bg_elevated };
        ui.painter().rect(rect, CornerRadius::same(7), fill, Stroke::new(1.0, t.border), StrokeKind::Inside);
        let fg = if enabled { t.text } else { t.text_faint };
        let mut x = rect.left() + 9.0;
        if let Some(i) = icon {
            let ir = egui::Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0));
            paint_icon(ui.painter(), ir, i, if enabled { t.text_dim } else { t.text_faint });
            x += icon_w;
        }
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    resp
}

/// 배너 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BannerKind {
    Error,
    Warning,
    Success,
}

/// 닫기 버튼이 있는 알림 배너. 닫기를 누르면 `true`.
pub(crate) fn banner(ui: &mut Ui, kind: BannerKind, title: &str, detail: Option<&str>, closable: bool) -> bool {
    let t = theme();
    let (c, icon) = match kind {
        BannerKind::Error => (t.red, Icon::Warning),
        BannerKind::Warning => (t.yellow, Icon::Warning),
        BannerKind::Success => (t.green, Icon::CheckCircle),
    };
    let mut closed = false;
    egui::Frame::new()
        .fill(alpha(c, if t.dark { 0.10 } else { 0.07 }))
        .stroke(Stroke::new(1.0, alpha(c, 0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                paint_icon(ui.painter(), ir, icon, c);
                ui.add(egui::Label::new(RichText::new(title).color(t.text).font(fonts::medium(12.5))).wrap());
                if closable {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, Icon::Close, "닫기").clicked() {
                            closed = true;
                        }
                    });
                }
            });
            if let Some(d) = detail.filter(|d| !d.trim().is_empty()) {
                let d = d.trim();
                let shown: String = d.lines().take(12).collect::<Vec<_>>().join("\n");
                ui.add(
                    egui::Label::new(RichText::new(shown).monospace().size(11.0).color(t.text_dim))
                        .wrap()
                        .selectable(true),
                );
            }
        });
    closed
}

/// 섹션 헤더(접기/펴기). 헤더 클릭 시 `open` 을 토글한다. `actions` 는 오른쪽 버튼 영역.
pub(crate) fn section_header(
    ui: &mut Ui,
    open: &mut bool,
    title: &str,
    count: Option<usize>,
    title_color: Option<Color32>,
    actions: impl FnOnce(&mut Ui),
) {
    let t = theme();
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, true, title));
    if resp.clicked() {
        *open = !*open;
    }
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    let bg = rect.shrink2(vec2(4.0, 1.0));
    if hovered {
        ui.painter().rect_filled(bg, CornerRadius::same(6), t.bg_hover);
    }
    let p = ui.painter();
    let chevron = if *open { Icon::ChevronDown } else { Icon::ChevronRight };
    paint_icon(p, egui::Rect::from_center_size(rect.left_center() + vec2(14.0, 0.0), vec2(12.0, 12.0)), chevron, t.text_faint);
    let title_rect = p.text(
        rect.left_center() + vec2(24.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        fonts::semibold(12.5),
        title_color.unwrap_or(t.text_dim),
    );
    if let Some(n) = count {
        let s = n.to_string();
        let g = p.layout_no_wrap(s, fonts::medium(11.0), t.text_dim);
        let br = egui::Rect::from_min_size(
            egui::pos2(title_rect.right() + 7.0, rect.center().y - 9.0),
            vec2((g.size().x + 12.0).max(20.0), 18.0),
        );
        p.rect_filled(br, CornerRadius::same(9), t.bg_hover);
        p.galley(br.center() - g.size() / 2.0, g, t.text_dim);
    }
    let actions_rect = egui::Rect::from_min_max(egui::pos2(rect.right() - 120.0, rect.top()), rect.max - vec2(8.0, 0.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(actions_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 2.0;
    actions(&mut child);
}

/// 체크 상태 아이콘과 색.
pub(crate) fn checks_icon(c: ChecksState) -> (Icon, Color32) {
    let t = theme();
    match c {
        ChecksState::Pass => (Icon::CheckCircle, t.green),
        ChecksState::Fail => (Icon::XCircle, t.red),
        ChecksState::Pending => (Icon::PendingCircle, t.yellow),
    }
}

/// 인라인 아이콘(레이아웃에 자리를 차지한다).
pub(crate) fn icon_label(ui: &mut Ui, icon: Icon, color: Color32, size: f32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if ui.is_rect_visible(r) {
        paint_icon(ui.painter(), r, icon, color);
    }
    resp
}

pub(crate) fn review_color(r: ReviewDecision) -> Color32 {
    let t = theme();
    match r {
        ReviewDecision::Approved => t.green,
        ReviewDecision::ChangesRequested => t.red,
        ReviewDecision::ReviewRequired => t.text_dim,
    }
}

/// PR 상태 배지(Open/Draft/Merged/Closed).
pub(crate) fn pr_state_badge(ui: &mut Ui, state: PrState, draft: bool) -> Response {
    let t = theme();
    let (label, c) = match (state, draft) {
        (PrState::Open, true) => ("초안", t.text_dim),
        (PrState::Open, false) => ("열림", t.green),
        (PrState::Merged, _) => ("병합됨", t.purple),
        (PrState::Closed, _) => ("닫힘", t.red),
    };
    outline_badge(ui, label, c)
}

/// PR 상태 아이콘 색(목록용).
pub(crate) fn pr_state_color(state: PrState, draft: bool) -> Color32 {
    let t = theme();
    match (state, draft) {
        (PrState::Open, true) => t.text_dim,
        (PrState::Open, false) => t.green,
        (PrState::Merged, _) => t.purple,
        (PrState::Closed, _) => t.red,
    }
}

/// 세그먼트 탭(필터 등). 선택이 바뀌면 `true`.
pub(crate) fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let t = theme();
    let font = fonts::medium(12.5);
    let widths: Vec<f32> =
        options.iter().map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), t.text).size().x + 20.0).collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(7), t.bg_input);
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for ((v, label), w) in options.iter().zip(widths) {
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("seg", *label)), Sense::click());
        let sel = *value == *v;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, *label));
        if sel {
            ui.painter().rect_filled(r, CornerRadius::same(5), t.bg_selected);
            ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(5), t.bg_hover);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, font.clone(), if sel { t.text } else { t.text_dim });
        if resp.clicked() && !sel {
            *value = *v;
            changed = true;
        }
        x += w;
    }
    changed
}

/// 작은 회색 보조 텍스트.
pub(crate) fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme().text_dim).size(12.5)
}

pub(crate) fn faint(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(theme().text_faint).size(11.5)
}

/// 확인 대화상자. 확인 시 `Some(true)`, 취소 시 `Some(false)`.
pub(crate) fn confirm_modal(
    ctx: &egui::Context,
    id: Id,
    title: &str,
    message: &str,
    confirm_label: &str,
    danger: bool,
    extra: impl FnOnce(&mut Ui),
) -> Option<bool> {
    let t = theme();
    let mut result = None;
    let modal = egui::Modal::new(id)
        .frame(
            egui::Frame::new()
                .fill(t.bg_elevated)
                .stroke(Stroke::new(1.0, t.border_strong))
                .corner_radius(CornerRadius::same(12))
                .shadow(t.shadow())
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(title).font(fonts::semibold(15.0)).color(t.text));
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(message).color(t.text_dim).size(13.0)).wrap());
            extra(ui);
            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = if danger { danger_button(ui, confirm_label) } else { primary_button(ui, confirm_label, None) };
                if ok.clicked() {
                    result = Some(true);
                }
                if secondary_button(ui, "취소").clicked() {
                    result = Some(false);
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && result.is_none() {
                result = Some(true);
            }
        });
    if result.is_none() && modal.should_close() {
        result = Some(false);
    }
    result
}

/// 텍스트 입력의 배경 프레임을 테마에 맞춘다.
pub(crate) fn input_frame() -> egui::Frame {
    let t = theme();
    egui::Frame::new()
        .fill(t.bg_input)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(8, 5))
}

/// 회전하는 로딩 표시.
pub(crate) fn spinner(ui: &mut Ui, size: f32) -> Response {
    ui.add(egui::Spinner::new().size(size).color(theme().text_dim))
}

/// 빈 상태 안내: 가운데 아이콘, 제목 한 줄, 흐린 설명.
pub(crate) fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    empty_state_icon(ui, Icon::Check, title, detail);
}

/// 아이콘을 고를 수 있는 빈 상태 안내.
pub(crate) fn empty_state_icon(ui: &mut Ui, icon: Icon, title: &str, detail: &str) {
    let t = theme();
    ui.add_space(20.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(10), t.bg_hover);
        paint_icon(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(18.0, 18.0)), icon, t.text_faint);
        ui.add_space(10.0);
        ui.label(RichText::new(title).color(t.text_dim).font(fonts::medium(13.0)));
        if !detail.is_empty() {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(detail).color(t.text_faint).size(12.0)).wrap());
        }
    });
    ui.add_space(20.0);
}

/// 테마 색으로 그린 라디오 버튼 행. 클릭되면 `true`.
pub(crate) fn radio_row(ui: &mut Ui, selected: bool, label: &str) -> bool {
    choice_row(ui, selected, label, false)
}

/// 테마 색으로 그린 체크박스 행. 값이 바뀌면 `true`.
pub(crate) fn checkbox_row(ui: &mut Ui, value: &mut bool, label: &str) -> bool {
    let clicked = choice_row(ui, *value, label, true);
    if clicked {
        *value = !*value;
    }
    clicked
}

fn choice_row(ui: &mut Ui, selected: bool, label: &str, square: bool) -> bool {
    let t = theme();
    let font = FontId::proportional(13.0);
    let wrap = (ui.available_width() - 26.0).max(40.0);
    let galley = ui.painter().layout(label.to_string(), font, t.text, wrap);
    let size = vec2(galley.size().x + 26.0, galley.size().y.max(18.0) + 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let typ = if square { egui::WidgetType::Checkbox } else { egui::WidgetType::RadioButton };
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(typ, enabled, selected, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let c = egui::pos2(rect.left() + 8.0, rect.top() + 11.0);
        let hovered = resp.hovered() && enabled;
        let border = if selected { t.accent } else if hovered { t.text_faint } else { t.border_strong };
        let box_r = egui::Rect::from_center_size(c, vec2(14.0, 14.0));
        if square {
            p.rect(box_r, CornerRadius::same(4), if selected { t.accent } else { t.bg_input }, Stroke::new(1.0, border), StrokeKind::Inside);
            if selected {
                paint_icon(p, box_r.shrink(1.5), Icon::Check, t.accent_fg);
            }
        } else {
            p.circle(c, 7.0, t.bg_input, Stroke::new(if selected { 1.5 } else { 1.0 }, border));
            if selected {
                p.circle_filled(c, 3.8, t.accent);
            }
        }
        p.galley(egui::pos2(rect.left() + 24.0, rect.top() + 2.0), galley, t.text);
    }
    resp.clicked() && enabled
}
