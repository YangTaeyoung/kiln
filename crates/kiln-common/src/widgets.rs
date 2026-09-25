//! 공용 위젯: 토글 스위치, 세그먼트 컨트롤, 키캡, 아바타, 버튼, 설정 행.

use crate::icons::{self, Icon};
use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use crate::Theme;
use crate::fonts;

/// iOS 스타일 토글 스위치.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let t = Theme::current();
    let size = vec2(34.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let track = lerp_color(t.border_strong, t.accent, k);
    ui.painter().rect_filled(rect, CornerRadius::same(10), track);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, k);
    ui.painter().circle_filled(pos2(x, rect.center().y), 7.5, Color32::WHITE);
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
    let t = Theme::current();
    let font = fonts::medium(13.0);
    let g = ui.painter().layout_no_wrap(label.to_string(), font, t.text);
    let size = vec2(g.size().x + 26.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let (fill, stroke, fg) = match kind {
        ButtonKind::Primary => (if hovered { lerp_color(t.accent, Color32::WHITE, 0.08) } else { t.accent }, t.accent, t.accent_fg),
        ButtonKind::Danger => (if hovered { lerp_color(t.red, Color32::WHITE, 0.08) } else { t.red }, t.red, Color32::WHITE),
        ButtonKind::Secondary => (if hovered { t.bg_hover } else { t.bg_elevated }, t.border_strong, t.text),
        ButtonKind::Ghost => (if hovered { t.bg_hover } else { Color32::TRANSPARENT }, Color32::TRANSPARENT, t.text_dim),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    if stroke != Color32::TRANSPARENT && kind == ButtonKind::Secondary {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, stroke), StrokeKind::Inside);
    }
    ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 아이콘만 있는 버튼. `active` 면 강조 배경.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool, tip: &str) -> Response {
    let t = Theme::current();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let (bg, fg) = if active {
        (t.accent_soft(if t.dark { 46 } else { 32 }), t.accent)
    } else if resp.hovered() {
        (t.bg_hover, t.text)
    } else {
        (Color32::TRANSPARENT, t.text_dim)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(size * 0.56)), icon, fg);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
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
    if t.dark { lerp_color(t.bg, Color32::BLACK, 0.45) } else { t.bg_panel }
}

/// 설정 화면의 한 줄: 제목·설명 왼쪽, 컨트롤 오른쪽.
pub fn setting_row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    let t = Theme::current();
    ui.horizontal(|ui| {
        ui.set_min_height(if desc.is_empty() { 36.0 } else { 46.0 });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(egui::RichText::new(title).font(fonts::medium(13.5)).color(t.text));
            if !desc.is_empty() {
                ui.label(egui::RichText::new(desc).size(12.0).color(t.text_faint));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
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
