//! 폰트에 의존하지 않는 벡터 아이콘, 파일 형식 배지, 공용 스타일 위젯.

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use kiln_common::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    ChevronRight,
    ChevronDown,
    ArrowUp,
    ArrowDown,
    Close,
    NewFile,
    NewFolder,
    Refresh,
    CollapseAll,
    Eye,
    EyeOff,
    Search,
    ReplaceOne,
    ReplaceAll,
    Folder,
    FolderOpen,
    Warning,
    Selection,
}

/// `rect` 가운데에 아이콘을 그린다.
pub fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 16.0;
    let st = Stroke::new(1.3 * s.max(0.85), color);
    let pt = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let poly = |pts: Vec<Pos2>| Shape::line(pts, st);
    match icon {
        Icon::ChevronRight => {
            p.add(poly(vec![pt(-2.0, -4.0), pt(2.0, 0.0), pt(-2.0, 4.0)]));
        }
        Icon::ChevronDown => {
            p.add(poly(vec![pt(-4.0, -2.0), pt(0.0, 2.0), pt(4.0, -2.0)]));
        }
        Icon::ArrowUp => {
            p.line_segment([pt(0.0, -5.0), pt(0.0, 5.0)], st);
            p.add(poly(vec![pt(-4.0, -1.0), pt(0.0, -5.0), pt(4.0, -1.0)]));
        }
        Icon::ArrowDown => {
            p.line_segment([pt(0.0, -5.0), pt(0.0, 5.0)], st);
            p.add(poly(vec![pt(-4.0, 1.0), pt(0.0, 5.0), pt(4.0, 1.0)]));
        }
        Icon::Close => {
            p.line_segment([pt(-4.0, -4.0), pt(4.0, 4.0)], st);
            p.line_segment([pt(4.0, -4.0), pt(-4.0, 4.0)], st);
        }
        Icon::NewFile => {
            p.add(poly(vec![pt(2.0, -6.0), pt(-5.0, -6.0), pt(-5.0, 6.0), pt(1.0, 6.0)]));
            p.add(poly(vec![pt(2.0, -6.0), pt(5.0, -3.0), pt(5.0, 0.0)]));
            p.line_segment([pt(4.5, 2.0), pt(4.5, 8.0)], st);
            p.line_segment([pt(1.5, 5.0), pt(7.5, 5.0)], st);
        }
        Icon::NewFolder => {
            p.add(poly(vec![pt(1.0, 5.0), pt(-7.0, 5.0), pt(-7.0, -5.0), pt(-3.0, -5.0), pt(-1.5, -3.0), pt(6.0, -3.0), pt(6.0, 0.0)]));
            p.line_segment([pt(4.5, 2.0), pt(4.5, 8.0)], st);
            p.line_segment([pt(1.5, 5.0), pt(7.5, 5.0)], st);
        }
        Icon::Refresh => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -0.6 + i as f32 / 20.0 * 4.9;
                    pt(5.0 * a.cos(), 5.0 * a.sin())
                })
                .collect();
            let tip = pts[0];
            p.add(Shape::line(pts, st));
            p.add(poly(vec![tip + vec2(-3.5 * s, -s), tip, tip + vec2(0.5 * s, -3.5 * s)]));
        }
        Icon::CollapseAll => {
            p.rect_stroke(Rect::from_min_max(pt(-4.0, -4.0), pt(6.0, 6.0)), 1.0, st, StrokeKind::Middle);
            p.add(poly(vec![pt(-6.0, 3.0), pt(-6.0, -6.0), pt(3.0, -6.0)]));
            p.line_segment([pt(-1.5, 1.0), pt(3.5, 1.0)], st);
        }
        Icon::Eye | Icon::EyeOff => {
            let top: Vec<Pos2> = (0..=16).map(|i| {
                let t = i as f32 / 16.0 * std::f32::consts::PI;
                pt(-7.0 * t.cos(), -4.5 * t.sin())
            }).collect();
            let bot: Vec<Pos2> = (0..=16).map(|i| {
                let t = i as f32 / 16.0 * std::f32::consts::PI;
                pt(-7.0 * t.cos(), 4.5 * t.sin())
            }).collect();
            p.add(Shape::line(top, st));
            p.add(Shape::line(bot, st));
            p.circle_stroke(c, 2.0 * s, st);
            if icon == Icon::EyeOff {
                p.line_segment([pt(-6.0, 6.0), pt(6.0, -6.0)], st);
            }
        }
        Icon::Search => {
            p.circle_stroke(pt(-1.5, -1.5), 4.5 * s, st);
            p.line_segment([pt(1.8, 1.8), pt(6.0, 6.0)], st);
        }
        Icon::ReplaceOne | Icon::ReplaceAll => {
            p.rect_stroke(Rect::from_min_max(pt(-7.0, -6.0), pt(-1.0, -1.0)), 1.0, st, StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(pt(1.0, 1.0), pt(7.0, 6.0)), 1.0, color);
            p.add(poly(vec![pt(1.0, -5.0), pt(5.0, -5.0), pt(5.0, -2.0)]));
            p.add(poly(vec![pt(3.5, -3.5), pt(5.0, -2.0), pt(6.5, -3.5)]));
            if icon == Icon::ReplaceAll {
                p.rect_filled(Rect::from_min_max(pt(-7.0, 2.0), pt(-2.0, 6.0)), 1.0, color);
            }
        }
        Icon::Folder | Icon::FolderOpen => {
            let body = vec![pt(-7.0, -5.0), pt(-2.5, -5.0), pt(-1.0, -3.0), pt(7.0, -3.0), pt(7.0, 5.0), pt(-7.0, 5.0)];
            p.add(Shape::convex_polygon(body, color.gamma_multiply(0.85), Stroke::NONE));
            if icon == Icon::FolderOpen {
                p.add(Shape::convex_polygon(
                    vec![pt(-5.5, -0.5), pt(8.0, -0.5), pt(6.5, 5.0), pt(-7.0, 5.0)],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Warning => {
            p.add(Shape::convex_polygon(vec![pt(0.0, -6.5), pt(7.0, 6.0), pt(-7.0, 6.0)], color, Stroke::NONE));
            let dark = Color32::from_black_alpha(220);
            p.line_segment([pt(0.0, -2.5), pt(0.0, 2.0)], Stroke::new(1.6 * s, dark));
            p.circle_filled(pt(0.0, 4.0), 0.9 * s, dark);
        }
        Icon::Selection => {
            p.line_segment([pt(-6.0, -4.0), pt(6.0, -4.0)], st);
            p.line_segment([pt(-6.0, 0.0), pt(6.0, 0.0)], st);
            p.line_segment([pt(-6.0, 4.0), pt(2.0, 4.0)], st);
        }
    }
}

/// 호버 시 배경이 생기는 정사각 아이콘 버튼.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    icon_toggle(ui, icon, tooltip, false, true)
}

/// 선택 상태를 표시할 수 있는 아이콘 버튼.
pub fn icon_toggle(ui: &mut Ui, icon: Icon, tooltip: &str, on: bool, enabled: bool) -> Response {
    let t = Theme::current();
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if on {
            p.rect_filled(rect, 4.0, t.accent.gamma_multiply(0.22));
            p.rect_stroke(rect, 4.0, Stroke::new(1.0, t.accent.gamma_multiply(0.7)), StrokeKind::Inside);
        } else if enabled && resp.hovered() {
            p.rect_filled(rect, 4.0, t.bg_hover);
        }
        let color = if !enabled {
            t.text_faint
        } else if on || resp.hovered() {
            t.text
        } else {
            t.text_dim
        };
        paint_icon(p, rect.shrink(3.0), icon, color);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, on, tooltip));
    resp.on_hover_text(tooltip)
}

/// 입력칸 안에 들어가는 작은 옵션 토글(`Aa`, `ab`, `.*`).
pub fn option_chip(ui: &mut Ui, label: &str, tooltip: &str, on: bool) -> Response {
    let t = Theme::current();
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if on {
            p.rect_filled(rect, 3.0, t.accent.gamma_multiply(0.25));
            p.rect_stroke(rect, 3.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(rect, 3.0, t.bg_hover);
        }
        let color = if on || resp.hovered() { t.text } else { t.text_dim };
        p.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(10.5), color);
        if label == "ab" {
            let y = rect.center().y + 6.0;
            p.line_segment([pos2(rect.left() + 5.0, y), pos2(rect.right() - 5.0, y)], Stroke::new(1.0, color));
        }
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, tooltip));
    resp.on_hover_text(tooltip)
}

/// 둥근 테두리 입력칸 프레임. 포커스가 있으면 강조색 테두리.
pub fn field_frame(focused: bool) -> egui::Frame {
    let t = Theme::current();
    egui::Frame::new()
        .fill(t.bg)
        .stroke(Stroke::new(1.0, if focused { t.accent.gamma_multiply(0.85) } else { t.border }))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin { left: 6, right: 2, top: 2, bottom: 2 })
}

/// 테두리 없는 한 줄 입력칸. 에러 상태면 붉은 글자.
pub fn bare_text_edit<'t>(text: &'t mut String, id: egui::Id, hint: &str, error: bool) -> egui::TextEdit<'t> {
    let t = Theme::current();
    egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(egui::RichText::new(hint).color(t.text_faint))
        .text_color(if error { t.red } else { t.text })
        .margin(vec2(0.0, 3.0))
        .return_key(None)
}

/// 작은 글꼴의 평평한 텍스트 버튼.
pub fn flat_button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let t = Theme::current();
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, t.text);
    let size = vec2(galley.size().x + 20.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let (fill, text) = if primary {
            (if resp.hovered() { t.accent } else { t.accent.gamma_multiply(0.85) }, Color32::from_rgb(0x10, 0x14, 0x20))
        } else {
            (if resp.hovered() { t.bg_hover } else { t.bg_elevated }, t.text)
        };
        p.rect_filled(rect, 4.0, fill);
        if !primary {
            p.rect_stroke(rect, 4.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        let pos = rect.center() - galley.size() / 2.0;
        p.galley_with_override_text_color(pos, galley, text);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp
}

/// 파일 이름으로 고른 형식 배지(글자, 색).
pub fn file_badge(name: &str) -> (&'static str, Color32) {
    let t = Theme::current();
    let cyan = Color32::from_rgb(0x56, 0xb6, 0xc2);
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match lower.as_str() {
        "dockerfile" | "containerfile" => return ("DK", t.blue),
        "makefile" | "justfile" => return ("MK", t.orange),
        "cargo.toml" | "cargo.lock" => return ("RS", t.orange),
        "package.json" => return ("NP", t.red),
        "license" | "licence" | "license.md" => return ("LC", t.yellow),
        ".gitignore" | ".gitattributes" | ".gitmodules" => return ("GI", t.orange),
        _ => {}
    }
    match ext {
        "rs" => ("RS", t.orange),
        "ts" | "mts" | "cts" => ("TS", t.blue),
        "tsx" | "jsx" => ("TX", cyan),
        "js" | "mjs" | "cjs" => ("JS", t.yellow),
        "json" | "jsonc" | "json5" => ("{}", t.yellow),
        "toml" => ("TM", t.text_dim),
        "yaml" | "yml" => ("YM", t.purple),
        "md" | "mdx" | "markdown" => ("MD", t.blue),
        "py" | "pyi" => ("PY", t.blue),
        "go" => ("GO", cyan),
        "html" | "htm" => ("<>", t.orange),
        "css" | "scss" | "sass" | "less" => ("#", t.blue),
        "sh" | "bash" | "zsh" | "fish" => ("$_", t.green),
        "c" | "h" => ("C", t.blue),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => ("C+", t.blue),
        "cs" => ("C#", t.purple),
        "java" => ("JV", t.red),
        "kt" | "kts" => ("KT", t.purple),
        "swift" => ("SW", t.orange),
        "rb" => ("RB", t.red),
        "php" => ("PH", t.purple),
        "lua" => ("LU", t.blue),
        "sql" => ("SQ", t.yellow),
        "zig" => ("ZG", t.orange),
        "vue" => ("VU", t.green),
        "svelte" => ("SV", t.orange),
        "lock" => ("LK", t.text_faint),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "svg" => ("IM", t.purple),
        "txt" | "log" => ("TX", t.text_faint),
        "env" => ("EN", t.yellow),
        "xml" => ("<>", t.orange),
        "proto" => ("PB", t.blue),
        "wasm" => ("WA", t.purple),
        "pdf" => ("PD", t.red),
        "zip" | "gz" | "tar" | "xz" | "zst" => ("ZP", t.text_faint),
        _ if lower.starts_with(".env") => ("EN", t.yellow),
        _ => ("", t.text_faint),
    }
}

/// 파일 형식 배지를 `rect` 에 그린다. 알 수 없는 형식은 문서 모양 아이콘.
pub fn paint_file_badge(p: &egui::Painter, rect: Rect, name: &str) {
    let (label, color) = file_badge(name);
    if label.is_empty() {
        let c = rect.center();
        let r = Rect::from_center_size(c, vec2(9.0, 11.0));
        p.rect_stroke(r, 1.5, Stroke::new(1.1, color), StrokeKind::Middle);
        p.line_segment([pos2(r.left() + 2.5, c.y - 1.0), pos2(r.right() - 2.5, c.y - 1.0)], Stroke::new(1.0, color));
        p.line_segment([pos2(r.left() + 2.5, c.y + 2.0), pos2(r.right() - 2.5, c.y + 2.0)], Stroke::new(1.0, color));
        return;
    }
    let size = if label.chars().count() >= 2 { 8.5 } else { 10.0 };
    p.text(rect.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, label, FontId::monospace(size), color);
}

/// 행 배경(호버/선택)을 칠한다.
pub fn paint_row_bg(p: &egui::Painter, rect: Rect, selected: bool, focused: bool, hovered: bool) {
    let t = Theme::current();
    if selected {
        let fill = if focused { t.bg_selected } else { t.bg_hover };
        p.rect_filled(rect, 0.0, fill);
        if focused {
            p.rect_stroke(rect, 0.0, Stroke::new(1.0, t.accent.gamma_multiply(0.55)), StrokeKind::Inside);
        }
    } else if hovered {
        p.rect_filled(rect, 0.0, t.bg_hover.gamma_multiply(0.8));
    }
}

pub fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.1} KB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB", b / K / K)
    } else {
        format!("{:.1} GB", b / K / K / K)
    }
}

