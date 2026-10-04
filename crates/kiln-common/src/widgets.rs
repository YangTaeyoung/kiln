//! 공용 위젯: 토글 스위치, 세그먼트 컨트롤, 키캡, 아바타, 버튼, 설정 행.

use crate::icons::{self, Icon};
use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use crate::Theme;
use crate::fonts;

/// Visible focus treatment for every custom control, including keyboard navigation.
pub fn focus_ring(ui: &Ui, response: &Response, radius: u8) {
    if response.has_focus() && ui.is_enabled() {
        ui.painter().rect_stroke(response.rect.expand(2.0), CornerRadius::same(radius), Stroke::new(2.0, Theme::current().accent), StrokeKind::Outside);
    }
}

/// iOS 스타일 토글 스위치.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let t = Theme::current();
    let size = vec2(36.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let label = ui.data(|d| d.get_temp::<String>(ui.id().with("setting-label"))).unwrap_or_else(|| kiln_common::i18n::tr("전환").into());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, &label));
    focus_ring(ui, &resp, 10);
    let rect = rect.shrink2(vec2(1.0, 2.0));
    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let track = lerp_color(t.border_strong, t.accent, k);
    ui.painter().rect_filled(rect, CornerRadius::same(10), track);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, k);
    ui.painter().circle_filled(pos2(x, rect.center().y), 7.5, lerp_color(Color32::WHITE, t.accent_fg, k));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 세그먼트 컨트롤. 선택이 바뀌면 true.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let t = Theme::current();
    let font = fonts::medium(12.5);
    let widths: Vec<f32> = options.iter().map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), t.text).size().x + 22.0).collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(7), t.bg_input);
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for ((v, label), w) in options.iter().zip(widths) {
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("seg", label)), Sense::click());
        let sel = *value == *v;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), sel, label));
        focus_ring(ui, &resp, 5);
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

/// 단축키 키캡 줄(예: "⌘", "K").
pub fn keycaps(ui: &mut Ui, keys: &[&str]) {
    let t = Theme::current();
    ui.spacing_mut().item_spacing.x = 3.0;
    for k in keys {
        let g = ui.painter().layout_no_wrap(k.to_string(), fonts::medium(11.0), t.text_dim);
        let w = (g.size().x + 10.0).max(18.0);
        let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(4), t.bg_hover);
        ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
        ui.painter().galley(r.center() - g.size() / 2.0, g, t.text_dim);
    }
}

/// 단축키 문자열("⇧⌘P")을 키캡 단위로 나눈다.
pub fn split_keys(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if "⌘⇧⌥⌃".contains(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(c.to_string());
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 이름에서 안정적인 색을 만든다.
pub fn hue_color(name: &str) -> Color32 {
    let h = name.bytes().fold(2166136261u32, |a, b| (a ^ b as u32).wrapping_mul(16777619));
    let palette = [0x7c8cff, 0x5fd08c, 0xff9f5a, 0xc49bff, 0x5eb1ff, 0xf2c46d, 0xff6b9a, 0x5ad4d4];
    let v = palette[(h as usize) % palette.len()];
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// 둥근 사각형 안에 첫 글자를 넣은 아바타.
pub fn avatar(ui: &Ui, rect: Rect, name: &str, active: bool) {
    let c = hue_color(name);
    let fill = if active { c } else { Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 60) };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let fg = if active { Color32::from_rgb(20, 20, 24) } else { c };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, letter, fonts::semibold(rect.height() * 0.5), fg);
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// 모서리가 둥근 버튼.
pub fn button(ui: &mut Ui, label: &str, kind: ButtonKind) -> Response {
    button_with(ui, None, label, kind, false)
}

/// 아이콘을 앞에 붙일 수 있는 둥근 버튼. `compact` 면 높이 26, 아니면 30.
/// 비활성 UI 안에서는 흐리게 그리고 클릭되지 않는다.
pub fn button_with(ui: &mut Ui, icon: Option<Icon>, label: &str, kind: ButtonKind, compact: bool) -> Response {
    let t = Theme::current();
    let enabled = ui.is_enabled();
    let font = fonts::medium(if compact { 12.5 } else { 13.0 });
    let g = ui.painter().layout_no_wrap(label.to_string(), font, t.text);
    let icon_w = if icon.is_some() { if label.is_empty() { 14.0 } else { 20.0 } } else { 0.0 };
    let pad = if compact { 20.0 } else { 26.0 };
    let size = vec2(g.size().x + icon_w + pad, if compact { 28.0 } else { 32.0 });
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hovered = resp.hovered() && enabled;
    let pressed = resp.is_pointer_button_down_on() && enabled;
    let (fill, stroke, fg) = match kind {
        ButtonKind::Primary => (if hovered { lerp_color(t.accent, Color32::WHITE, 0.08) } else { t.accent }, t.accent, t.accent_fg),
        ButtonKind::Danger => (if hovered { lerp_color(t.red, if t.dark { Color32::WHITE } else { Color32::BLACK }, 0.08) } else { t.red }, t.red, if t.dark { t.accent_fg } else { Color32::WHITE }),
        ButtonKind::Secondary => (if pressed { t.bg_selected } else if hovered { t.bg_hover } else { t.bg_elevated }, t.border_strong, t.text),
        ButtonKind::Ghost => (if pressed { t.bg_selected } else if hovered { t.bg_hover } else { Color32::TRANSPARENT }, Color32::TRANSPARENT, if hovered { t.text } else { t.text_dim }),
    };
    // Disabled Ui already attenuates its painter once; do not fade colors again.
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    if kind == ButtonKind::Secondary {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, stroke), StrokeKind::Inside);
    }
    let content_w = g.size().x + icon_w;
    let mut x = rect.center().x - content_w / 2.0;
    if let Some(i) = icon {
        let ir = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
        icons::paint(ui.painter(), ir, i, fg);
        x += icon_w;
    }
    ui.painter().galley_with_override_text_color(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    focus_ring(ui, &resp, 7);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 아이콘만 있는 버튼. `active` 면 강조 배경. 툴팁 문자열이 접근성 이름이 된다.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool, tip: &str) -> Response {
    let t = Theme::current();
    let enabled = ui.is_enabled();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, tip));
    if ui.is_rect_visible(rect) {
        let (bg, fg) = if !enabled {
            (Color32::TRANSPARENT, t.text_dim)
        } else if active {
            (t.accent_soft(if t.dark { 46 } else { 32 }), t.accent)
        } else if resp.hovered() {
            (t.bg_hover, t.text)
        } else {
            (Color32::TRANSPARENT, t.text_dim)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(size * 0.56)), icon, fg);
    }
    focus_ring(ui, &resp, 7);
    if resp.hovered() && enabled { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// 입력칸 프레임: bg_input 바탕, 1px 테두리, 모서리 7. 포커스면 강조색, 오류면 빨간 테두리.
pub fn input_frame(focused: bool, error: bool) -> egui::Frame {
    let t = Theme::current();
    let stroke = if error {
        t.red
    } else if focused {
        t.accent
    } else {
        t.border_strong
    };
    egui::Frame::new()
        .fill(t.bg_input)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(8, 4))
}

/// 색을 불투명도 `a`(0~1)로 섞는다.
pub fn tint(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// 둥근 알약 배지. 글자색 `fg`, 바탕은 `fg` 를 옅게 깐다.
pub fn pill(ui: &mut Ui, text: &str, fg: Color32) -> Response {
    let t = Theme::current();
    let g = ui.painter().layout_no_wrap(text.to_string(), fonts::medium(11.0), fg);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 18.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(9), tint(fg, if t.dark { 0.16 } else { 0.12 }));
        ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    }
    resp
}

/// 작은 섹션 제목(세미볼드 12.5, 흐린 글자).
pub fn section_title(ui: &mut Ui, text: &str) -> Response {
    let t = Theme::current();
    ui.label(egui::RichText::new(text).font(fonts::semibold(12.5)).color(t.text_dim))
}

/// 목록 행 배경: 선택은 bg_selected, 호버는 bg_hover, 모서리 6.
pub fn paint_row(painter: &egui::Painter, rect: Rect, selected: bool, hovered: bool) {
    let t = Theme::current();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), t.bg_selected);
    } else if hovered {
        painter.rect_filled(rect, CornerRadius::same(6), t.bg_hover);
    }
}

/// 빈 상태: 가운데 아이콘, 한 줄 안내, 선택적 동작 버튼. 버튼이 눌리면 true.
pub fn empty_state(ui: &mut Ui, icon: Icon, text: &str, action: Option<&str>) -> bool {
    let t = Theme::current();
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(28.0);
        let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(10), t.bg_hover);
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), Vec2::splat(20.0)), icon, t.text_faint);
        ui.add_space(10.0);
        ui.add(egui::Label::new(egui::RichText::new(text).font(fonts::medium(13.0)).color(t.text_dim)).wrap());
        if let Some(a) = action {
            ui.add_space(10.0);
            clicked = button_with(ui, None, a, ButtonKind::Secondary, true).clicked();
        }
        ui.add_space(20.0);
    });
    clicked
}

/// 상태 점(주의가 필요하면 부드럽게 깜박인다).
pub fn status_dot(ui: &Ui, center: egui::Pos2, color: Color32, pulse: bool) {
    if pulse {
        let time = ui.input(|i| i.time);
        let a = ((time * 3.0).sin() * 0.5 + 0.5) as f32;
        ui.painter().circle_filled(center, 3.5 + 3.0 * a, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (70.0 * (1.0 - a)) as u8));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    ui.painter().circle_filled(center, 3.5, color);
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// 카드·시트 바탕(창 바탕보다 조금 어둡거나 밝은 캔버스 색).
pub fn canvas_color(t: &Theme) -> Color32 {
    if t.dark { lerp_color(t.bg_panel, t.bg, 0.5) } else { t.bg_panel }
}

/// 설정 화면의 한 줄: 제목·설명 왼쪽, 컨트롤 오른쪽.
pub fn setting_row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    setting_row_with_control_width(ui,title,desc,230.0,control);
}

/// A switch needs only its own width; keep it aligned with its setting at narrow sizes.
pub fn setting_toggle(ui: &mut Ui, title: &str, desc: &str, value: &mut bool) {
    setting_row_with_control_width(ui,title,desc,52.0,|ui|{toggle(ui,value);});
}

fn setting_row_with_control_width(ui: &mut Ui, title: &str, desc: &str, control_width: f32, control: impl FnOnce(&mut Ui)) {
    let t = Theme::current();
    let width = ui.available_width();
    // Stack descriptive settings at narrow widths so controls never cover the explanation.
    ui.push_id(title, |ui| {
        let copy = |ui: &mut Ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.add(egui::Label::new(egui::RichText::new(title).font(fonts::medium(13.5)).color(t.text)).wrap());
            if !desc.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(desc).size(12.0).color(t.text_dim)).wrap());
            }
        };
        if width < control_width + 250.0 && !desc.is_empty() {
            ui.add_space(8.0);
            copy(ui);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.data_mut(|d| d.insert_temp(ui.id().with("setting-label"), title.to_owned()));
                control(ui);
            });
            ui.add_space(8.0);
        } else {
            ui.horizontal(|ui| {
                ui.set_min_height(if desc.is_empty() { 38.0 } else { 54.0 });
                ui.allocate_ui_with_layout(vec2((width - control_width).max(100.0), if desc.is_empty() { 24.0 } else { 42.0 }), egui::Layout::top_down(egui::Align::Min), copy);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.data_mut(|d| d.insert_temp(ui.id().with("setting-label"), title.to_owned()));
                    control(ui);
                });
            });
        }
    });
}

/// 설정 그룹 카드.
pub fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    let t = Theme::current();
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).font(fonts::semibold(12.0)).color(t.text_faint));
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin { left: 16, right: 14, top: 6, bottom: 6 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            body(ui);
        });
    ui.add_space(14.0);
}

/// 그룹 안 구분선.
pub fn divider(ui: &mut Ui) {
    let t = Theme::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.border);
}
