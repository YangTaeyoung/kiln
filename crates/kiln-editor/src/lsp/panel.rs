//! 문제(진단) 패널: 모든 파일의 진단을 파일별로 묶어 보여 준다.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Id, Painter, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use kiln_common::Theme;

use super::{Diagnostic, LspManager, Severity};
use crate::EditorEvent;
use crate::ui_kit::{self, Icon};

const ROW_H: f32 = 22.0;
const HEADER_H: f32 = 28.0;

#[derive(Clone, Copy)]
enum Row {
    File(usize),
    Diag(usize, usize),
}

/// 심각도 색.
pub fn severity_color(s: Severity) -> Color32 {
    let t = Theme::current();
    match s {
        Severity::Error => t.red,
        Severity::Warning => t.yellow,
        Severity::Information => t.blue,
        Severity::Hint => t.text_dim,
    }
}

/// 심각도 아이콘을 `center` 에 그린다.
pub fn paint_severity(p: &Painter, center: Pos2, s: Severity, size: f32) {
    let color = severity_color(s);
    let r = size / 2.0;
    match s {
        Severity::Error => {
            p.circle_filled(center, r, color);
            let d = r * 0.42;
            let st = Stroke::new(1.4, Color32::from_black_alpha(220));
            p.line_segment([center + vec2(-d, -d), center + vec2(d, d)], st);
            p.line_segment([center + vec2(d, -d), center + vec2(-d, d)], st);
        }
        Severity::Warning => ui_kit::paint_icon(p, Rect::from_center_size(center, vec2(size, size)), Icon::Warning, color),
        Severity::Information => {
            p.circle_filled(center, r, color);
            let dark = Color32::from_black_alpha(220);
            p.circle_filled(center + vec2(0.0, -r * 0.45), 1.0, dark);
            p.line_segment([center + vec2(0.0, -r * 0.1), center + vec2(0.0, r * 0.55)], Stroke::new(1.5, dark));
        }
        Severity::Hint => {
            p.circle_stroke(center, r * 0.75, Stroke::new(1.2, color));
        }
    }
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "오류",
        Severity::Warning => "경고",
        Severity::Information => "정보",
        Severity::Hint => "힌트",
    }
}

/// 문제 패널을 그린다. 진단 행을 누르면 그 위치로 가는 `OpenAt` 을 돌려준다.
pub fn diagnostics_ui(ui: &mut Ui, lsp: &LspManager) -> Vec<EditorEvent> {
    let t = Theme::current();
    let id = Id::new(("kiln-diagnostics", lsp.root()));
    let mut events = Vec::new();
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    ui.painter().rect_filled(full, 0.0, t.bg_panel);

    let mut files = lsp.all_diagnostics();
    for (_, d) in &mut files {
        d.sort_by_key(|x| (x.severity, x.range.start));
    }
    let root = lsp.root();
    let mut collapsed: HashSet<PathBuf> = ui.data(|d| d.get_temp(id.with("collapsed"))).unwrap_or_default();

    // 요약 줄
    let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
    {
        let p = ui.painter();
        p.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, t.border));
        let mut x = header.left() + 12.0;
        let cy = header.center().y;
        let count = |s: Severity| files.iter().flat_map(|(_, d)| d).filter(|d| d.severity == s).count();
        let total: usize = files.iter().map(|(_, d)| d.len()).sum();
        if total == 0 {
            p.circle_filled(pos2(x + 6.0, cy), 6.0, t.green.gamma_multiply(0.9));
            let dark = Color32::from_black_alpha(220);
            p.add(egui::Shape::line(
                vec![pos2(x + 3.2, cy + 0.2), pos2(x + 5.3, cy + 2.3), pos2(x + 8.9, cy - 2.2)],
                Stroke::new(1.5, dark),
            ));
            p.text(pos2(x + 16.0, cy), Align2::LEFT_CENTER, "문제 없음", FontId::proportional(12.0), t.text);
            let body = Rect::from_min_max(pos2(full.left(), header.bottom()), full.max);
            p.text(
                body.center() - vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                "작업 공간에서 발견된 문제가 없습니다",
                FontId::proportional(12.5),
                t.text_faint,
            );
        } else {
            for s in [Severity::Error, Severity::Warning, Severity::Information, Severity::Hint] {
                let n = count(s);
                if n == 0 && s > Severity::Warning {
                    continue;
                }
                paint_severity(p, pos2(x + 6.0, cy), s, 12.0);
                let g = p.layout_no_wrap(format!("{} {n}", severity_label(s)), FontId::proportional(12.0), if n > 0 { t.text } else { t.text_faint });
                let w = g.size().x;
                p.galley(pos2(x + 16.0, cy - g.size().y / 2.0), g, t.text);
                x += 16.0 + w + 16.0;
            }
            let files_text = format!("파일 {}개", files.len());
            p.text(pos2(header.right() - 12.0, cy), Align2::RIGHT_CENTER, files_text, FontId::proportional(11.5), t.text_faint);
        }
    }

    let mut rows = Vec::new();
    for (fi, (path, diags)) in files.iter().enumerate() {
        rows.push(Row::File(fi));
        if !collapsed.contains(path) {
            rows.extend((0..diags.len()).map(|di| Row::Diag(fi, di)));
        }
    }

    let list_rect = Rect::from_min_max(pos2(full.left(), header.bottom()), full.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).id_salt(id.with("list")));
    child.set_clip_rect(list_rect.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing.y = 0.0;
    let mut toggle: Option<PathBuf> = None;
    ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false, false]).show_rows(
        &mut child,
        ROW_H,
        rows.len(),
        |ui, range| {
            for i in range {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                ui_kit::paint_row_bg(ui.painter(), rect, false, false, resp.hovered());
                match rows[i] {
                    Row::File(fi) => {
                        let (path, diags) = &files[fi];
                        file_row(ui, rect, path, &root, diags, collapsed.contains(path));
                        let label = path.display().to_string();
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                        if resp.clicked() {
                            toggle = Some(path.clone());
                        }
                    }
                    Row::Diag(fi, di) => {
                        let (path, diags) = &files[fi];
                        let d = &diags[di];
                        diag_row(ui, rect, d);
                        let label = d.message.clone();
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                        let resp = resp.on_hover_text(&d.message);
                        if resp.clicked() {
                            events.push(open_at(lsp, path, d));
                        }
                    }
                }
            }
        },
    );
    if let Some(p) = toggle {
        if !collapsed.remove(&p) {
            collapsed.insert(p);
        }
        ui.data_mut(|d| d.insert_temp(id.with("collapsed"), collapsed));
    }
    events
}

fn open_at(lsp: &LspManager, path: &Path, d: &Diagnostic) -> EditorEvent {
    let line = d.range.start.line as usize;
    let col = lsp
        .line_text(path, line)
        .map_or(d.range.start.character as usize, |l| super::position::char_col(&l, d.range.start.character));
    EditorEvent::OpenAt { path: path.to_path_buf(), line: line + 1, col: col + 1 }
}

fn file_row(ui: &Ui, rect: Rect, path: &Path, root: &Path, diags: &[Diagnostic], collapsed: bool) {
    let t = Theme::current();
    let p = ui.painter();
    let chevron = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
    ui_kit::paint_icon(p, Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(12.0, 12.0)), chevron, t.text_dim);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ui_kit::paint_file_badge(p, Rect::from_center_size(pos2(rect.left() + 32.0, rect.center().y), vec2(16.0, 16.0)), &name);
    let rel = path.strip_prefix(root).unwrap_or(path);
    let dir = rel.parent().map(|d| d.to_string_lossy().replace('\\', "/")).unwrap_or_default();

    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = diags.iter().filter(|d| d.severity == Severity::Warning).count();
    let others = diags.len() - errors - warnings;
    let other_sev = diags.iter().map(|d| d.severity).filter(|s| *s > Severity::Warning).min().unwrap_or(Severity::Information);
    let mut x = rect.right() - 10.0;
    for (n, s) in [(others, other_sev), (warnings, Severity::Warning), (errors, Severity::Error)] {
        if n == 0 {
            continue;
        }
        let text = n.to_string();
        let w = 20.0 + text.len() as f32 * 6.5;
        let pill = Rect::from_min_max(pos2(x - w, rect.center().y - 8.0), pos2(x, rect.center().y + 8.0));
        let c = severity_color(s);
        p.rect_filled(pill, 8.0, c.gamma_multiply(0.16));
        p.circle_filled(pos2(pill.left() + 8.0, pill.center().y), 3.0, c);
        p.text(pos2(pill.left() + 14.0, pill.center().y), Align2::LEFT_CENTER, text, FontId::proportional(10.5), t.text);
        x = pill.left() - 4.0;
    }

    let mut job = LayoutJob::default();
    job.append(&name, 0.0, TextFormat { font_id: FontId::proportional(13.0), color: t.text, ..Default::default() });
    if !dir.is_empty() {
        job.append(&format!("  {dir}"), 0.0, TextFormat { font_id: FontId::proportional(11.5), color: t.text_faint, ..Default::default() });
    }
    job.wrap.max_width = (x - rect.left() - 50.0).max(20.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(rect.left() + 44.0, rect.center().y - g.size().y / 2.0), g, t.text);
}

fn diag_row(ui: &Ui, rect: Rect, d: &Diagnostic) {
    let t = Theme::current();
    let p = ui.painter();
    paint_severity(p, pos2(rect.left() + 42.0, rect.center().y), d.severity, 12.0);
    let loc = format!("{}:{}", d.range.start.line + 1, d.range.start.character + 1);
    let loc_g = p.layout_no_wrap(loc, FontId::monospace(11.0), t.text_faint);
    let loc_w = loc_g.size().x;
    p.galley(pos2(rect.right() - 10.0 - loc_w, rect.center().y - loc_g.size().y / 2.0), loc_g, t.text_faint);

    let first_line = d.message.lines().next().unwrap_or("");
    let mut job = LayoutJob::default();
    job.append(first_line, 0.0, TextFormat { font_id: FontId::proportional(12.5), color: t.text, ..Default::default() });
    let tail = match (&d.source, &d.code) {
        (Some(s), Some(c)) => format!("  {s}({c})"),
        (Some(s), None) => format!("  {s}"),
        (None, Some(c)) => format!("  {c}"),
        (None, None) => String::new(),
    };
    if !tail.is_empty() {
        job.append(&tail, 0.0, TextFormat { font_id: FontId::proportional(11.5), color: t.text_faint, ..Default::default() });
    }
    job.wrap.max_width = (rect.width() - 56.0 - loc_w - 20.0).max(20.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let g = p.layout_job(job);
    p.galley(pos2(rect.left() + 54.0, rect.center().y - g.size().y / 2.0), g, t.text);
}
