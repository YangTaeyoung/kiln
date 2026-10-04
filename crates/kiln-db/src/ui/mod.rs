//! 패널·탭이 함께 쓰는 UI 부품.

pub(crate) mod grid;
pub(crate) mod highlight;

use crate::ConnStatus;
use egui::{Color32, CornerRadius, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use kiln_common::Theme;
use kiln_common::fonts;
use kiln_common::icons::{self, Icon};
use kiln_common::widgets::{self, ButtonKind};

/// 연결 상태 점 색.
pub(crate) fn status_color(st: &ConnStatus) -> Color32 {
    let t = Theme::current();
    match st {
        ConnStatus::Disconnected => t.text_faint,
        ConnStatus::Connecting => t.yellow,
        ConnStatus::Connected => t.green,
        ConnStatus::Failed(_) => t.red,
    }
}

pub(crate) fn paint_dot(ui: &Ui, center: Pos2, color: Color32) {
    ui.painter().circle_filled(center, 3.5, color);
}

/// DB 화면에서 쓰는 아이콘: 공용 아이콘과 이 크레이트 전용 선 아이콘.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    Common(Icon),
    Link,
    Bolt,
    Dot,
    ChevronLeft,
    First,
    Last,
    Lock,
}

impl From<Icon> for Glyph {
    fn from(i: Icon) -> Glyph {
        Glyph::Common(i)
    }
}

/// `rect` 가운데에 아이콘을 그린다.
pub(crate) fn paint_glyph(p: &egui::Painter, rect: Rect, g: Glyph, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 18.0;
    let st = Stroke::new(1.5 * s.max(0.8), color);
    let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    match g {
        Glyph::Common(i) => icons::paint(p, rect, i, color),
        Glyph::Link => {
            p.add(Shape::line(vec![at(-1.0, -4.0), at(1.5, -6.5), at(4.5, -6.5), at(6.5, -4.5), at(6.5, -1.5), at(4.0, 1.0)], st));
            p.add(Shape::line(vec![at(1.0, 4.0), at(-1.5, 6.5), at(-4.5, 6.5), at(-6.5, 4.5), at(-6.5, 1.5), at(-4.0, -1.0)], st));
            p.line_segment([at(-2.5, 2.5), at(2.5, -2.5)], st);
        }
        Glyph::Bolt => {
            p.add(Shape::closed_line(vec![at(2.0, -8.0), at(-5.5, 1.0), at(-0.5, 1.0), at(-2.0, 8.0), at(5.5, -1.0), at(0.5, -1.0)], st));
        }
        Glyph::Dot => {
            p.circle_filled(c, 1.8 * s, color);
        }
        Glyph::ChevronLeft => {
            p.add(Shape::line(vec![at(2.0, -5.0), at(-3.0, 0.0), at(2.0, 5.0)], st));
        }
        Glyph::First => {
            p.add(Shape::line(vec![at(3.5, -5.0), at(-1.5, 0.0), at(3.5, 5.0)], st));
            p.line_segment([at(-4.5, -5.0), at(-4.5, 5.0)], st);
        }
        Glyph::Last => {
            p.add(Shape::line(vec![at(-3.5, -5.0), at(1.5, 0.0), at(-3.5, 5.0)], st));
            p.line_segment([at(4.5, -5.0), at(4.5, 5.0)], st);
        }
        Glyph::Lock => {
            p.rect_stroke(Rect::from_min_max(at(-5.5, -1.0), at(5.5, 7.0)), 1.5 * s, st, egui::StrokeKind::Middle);
            p.add(Shape::line(
                (0..=12)
                    .map(|i| {
                        let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / 12.0;
                        at(3.5 * a.cos(), -4.0 + 3.5 * a.sin())
                    })
                    .collect(),
                st,
            ));
            p.line_segment([at(-3.5, -4.0), at(-3.5, -1.0)], st);
            p.line_segment([at(3.5, -4.0), at(3.5, -1.0)], st);
        }
    }
}

/// 레이아웃에 자리를 차지하는 인라인 아이콘.
pub(crate) fn glyph_label(ui: &mut Ui, g: impl Into<Glyph>, color: Color32, size: f32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    if ui.is_rect_visible(r) {
        paint_glyph(ui.painter(), r, g.into(), color);
    }
    resp
}

/// 아이콘만 있는 둥근 버튼(24px). 툴팁이 접근성 이름이 된다.
pub(crate) fn icon_button(ui: &mut Ui, g: impl Into<Glyph>, tip: &str) -> Response {
    glyph_button(ui, g.into(), tip, true, false)
}

/// 활성/켜짐 상태를 지정하는 아이콘 버튼.
pub(crate) fn glyph_button(ui: &mut Ui, g: Glyph, tip: &str, enabled: bool, active: bool) -> Response {
    if let Glyph::Common(i) = g {
        return ui.add_enabled_ui(enabled, |ui| widgets::icon_button(ui, i, 24.0, active, tip)).inner;
    }
    let t = Theme::current();
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, tip));
    if ui.is_rect_visible(rect) {
        let (bg, fg) = if !enabled {
            (Color32::TRANSPARENT, t.text_faint)
        } else if active {
            (t.accent_soft(if t.dark { 46 } else { 32 }), t.accent)
        } else if resp.hovered() {
            (t.bg_hover, t.text)
        } else {
            (Color32::TRANSPARENT, t.text_dim)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
        paint_glyph(ui.painter(), Rect::from_center_size(rect.center(), vec2(13.5, 13.5)), g, fg);
    }
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// 아이콘이 붙은 툴바 버튼. 기본은 테두리 없는 고스트, `primary` 면 강조색.
pub(crate) fn tool_button_icon(ui: &mut Ui, icon: Option<Icon>, label: &str, enabled: bool, primary: bool) -> Response {
    let kind = if primary && enabled { ButtonKind::Primary } else { ButtonKind::Ghost };
    ui.add_enabled_ui(enabled, |ui| widgets::button_with(ui, icon, label, kind, true)).inner
}

/// 테두리 있는 보조 버튼(높이 26).
pub(crate) fn secondary_button(ui: &mut Ui, icon: Option<Icon>, label: &str, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| widgets::button_with(ui, icon, label, ButtonKind::Secondary, true)).inner
}

/// 아이콘이 붙은 켜고 끄는 버튼.
pub(crate) fn toggle_button_icon(ui: &mut Ui, icon: Option<Icon>, label: &str, on: bool) -> Response {
    let t = Theme::current();
    let font = fonts::medium(12.5);
    let g = ui.painter().layout_no_wrap(label.to_string(), font, t.text);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + icon_w + 20.0, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    if ui.is_rect_visible(rect) {
        let (bg, fg) = if on {
            (t.accent_soft(if t.dark { 40 } else { 28 }), t.accent)
        } else if resp.hovered() {
            (t.bg_hover, t.text)
        } else {
            (Color32::TRANSPARENT, t.text_dim)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
        let mut x = rect.center().x - (g.size().x + icon_w) / 2.0;
        if let Some(i) = icon {
            icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)), i, fg);
            x += icon_w;
        }
        ui.painter().galley_with_override_text_color(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}

/// 세그먼트 컨트롤. 각 칸은 접근성 이름을 가진 버튼이다. 선택이 바뀌면 `true`.
pub(crate) fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let t = Theme::current();
    let font = fonts::medium(12.5);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), t.text).size().x + 22.0)
        .collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), t.bg_hover);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for ((v, label), w) in options.iter().zip(widths) {
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("db-seg", *label)), Sense::click());
        let sel = *value == *v;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, *label));
        if sel {
            ui.painter().rect_filled(r, CornerRadius::same(6), if t.dark { t.bg_selected } else { t.bg_elevated });
            if !t.dark {
                ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            }
        }
        let fg = if sel || resp.hovered() { t.text } else { t.text_dim };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *label, font.clone(), fg);
        if resp.clicked() && !sel {
            *value = *v;
            changed = true;
        }
        x += w;
    }
    changed
}

/// 결과 탭 같은 작은 탭 칩. 접근성 이름은 `label`.
pub(crate) fn tab_chip(ui: &mut Ui, label: &str, selected: bool, color: Color32) -> Response {
    let t = Theme::current();
    let font = fonts::medium(12.0);
    let g = ui.painter().layout_no_wrap(label.to_string(), font, color);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter().rect_filled(rect, CornerRadius::same(6), t.bg_selected);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), t.bg_hover);
        }
        let fg = if selected { color } else { widgets::lerp_color(color, t.bg, 0.25) };
        ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    }
    resp
}

/// 드롭다운을 여는 고스트 버튼. 누르면 `content` 가 메뉴로 열린다.
pub(crate) fn menu_button(ui: &mut Ui, icon: Option<Icon>, label: &str, content: impl FnOnce(&mut Ui)) -> Response {
    let resp = widgets::button_with(ui, icon, label, ButtonKind::Ghost, true);
    egui::Popup::menu(&resp).gap(4.0).show(|ui| {
        ui.set_min_width(180.0);
        content(ui);
    });
    resp
}

/// 입력칸 프레임 안에 테두리 없는 한 줄 입력을 그린다.
pub(crate) fn text_field(ui: &mut Ui, edit: egui::TextEdit<'_>, id: egui::Id, width: f32) -> Response {
    let focused = ui.memory(|m| m.has_focus(id));
    widgets::input_frame(focused, false)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.add(edit.id(id).frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 2))).desired_width(width - 18.0))
        })
        .inner
}

/// 회전하는 로딩 표시.
pub(crate) fn spinner(ui: &mut Ui) {
    ui.add(egui::Spinner::new().size(12.0).color(Theme::current().text_dim));
}

/// 흐린 보조 텍스트.
pub(crate) fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(Theme::current().text_dim).size(12.0)
}

/// 더 흐린 메타 텍스트.
pub(crate) fn faint(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(Theme::current().text_faint).size(11.5)
}

/// 알림 줄(오류/정보). 아이콘과 옅은 색 바탕, 모서리 8.
pub(crate) fn banner(ui: &mut Ui, text: &str, error: bool) {
    let t = Theme::current();
    let color = if error { t.red } else { t.blue };
    egui::Frame::new()
        .fill(widgets::tint(color, if t.dark { 0.10 } else { 0.07 }))
        .stroke(Stroke::new(1.0, widgets::tint(color, 0.30)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                glyph_label(ui, if error { Icon::Warning } else { Icon::Info }, color, 15.0);
                ui.add(egui::Label::new(RichText::new(text).color(t.text).size(12.5)).wrap());
            });
        });
}

/// 모달 대화상자 프레임.
pub(crate) fn modal_frame() -> egui::Frame {
    let t = Theme::current();
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(12))
        .shadow(t.shadow())
        .inner_margin(egui::Margin::same(20))
}

/// 입력한 이름이 일치해야 확인되는 위험 작업 대화상자.
pub(crate) struct TypedConfirm {
    pub title: String,
    pub message: String,
    pub expected: String,
    pub input: String,
    pub action_label: String,
}

impl TypedConfirm {
    /// 모달을 그린다. `Some(true)` 확인, `Some(false)` 취소.
    pub fn show(&mut self, ctx: &egui::Context, id: egui::Id) -> Option<bool> {
        let t = Theme::current();
        let mut result = None;
        let resp = egui::Modal::new(id).frame(modal_frame()).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(8), widgets::tint(t.red, 0.14));
                icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), Icon::Warning, t.red);
                ui.label(RichText::new(&self.title).font(fonts::semibold(15.0)).color(t.text));
            });
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(&self.message).size(13.0).color(t.text_dim)).wrap());
            ui.add_space(12.0);
            ui.label(faint(format!("확인하려면 \"{}\"을(를) 입력하세요", self.expected)));
            ui.add_space(4.0);
            let fid = id.with("typed");
            let hint = RichText::new(self.expected.as_str()).color(t.text_faint);
            let r = text_field(ui, egui::TextEdit::singleline(&mut self.input).hint_text(hint), fid, ui.available_width());
            r.request_focus();
            ui.add_space(16.0);
            let ok = self.input == self.expected;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let danger = ui.add_enabled_ui(ok, |ui| widgets::button_with(ui, None, &self.action_label, ButtonKind::Danger, false)).inner;
                if danger.clicked() || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    result = Some(true);
                }
                if widgets::button(ui, "취소", ButtonKind::Secondary).clicked() {
                    result = Some(false);
                }
            });
        });
        if resp.should_close() && result.is_none() {
            result = Some(false);
        }
        result
    }
}

/// 트리 한 줄을 할당한다(클릭·더블클릭·우클릭 감지). 배경은 둥근 호버·선택 채움.
pub(crate) fn tree_row(ui: &mut Ui, height: f32, selected: bool, label: &str) -> (egui::Rect, Response) {
    let w = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        widgets::paint_row(ui.painter(), rect.shrink2(vec2(0.0, 1.0)), selected, resp.hovered());
    }
    (rect, resp)
}

/// 위젯 위에서 기본 버튼 더블클릭이 끝났는지.
pub(crate) fn double_clicked(ui: &Ui, resp: &Response) -> bool {
    resp.hovered() && ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
}

/// 펼침 화살표.
pub(crate) fn chevron(ui: &Ui, pos: Pos2, open: bool, visible: bool) {
    if !visible {
        return;
    }
    let t = Theme::current();
    let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
    icons::paint(ui.painter(), Rect::from_center_size(pos, vec2(11.0, 11.0)), icon, t.text_faint);
}

/// 숫자에 천 단위 구분자를 넣는다.
pub(crate) fn thousands(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}
