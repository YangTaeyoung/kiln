//! Cmd+P 퍼지 파일 찾기 모달.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Id, Key, Modifiers, Order, Rect, Sense, Stroke, pos2, vec2};
use kiln_common::Theme;

use crate::fuzzy::{FileIndex, FuzzyMatch, FuzzyWorker};
use crate::ui_kit::{self, Icon};

const ROW_H: f32 = 28.0;
const MAX_ROWS: usize = 12;
const RESULT_LIMIT: usize = 200;
const REINDEX_AFTER: Duration = Duration::from_secs(10);

/// 빠른 파일 열기 모달.
pub struct QuickOpen {
    id: Id,
    open: bool,
    root: Option<PathBuf>,
    index: Option<Arc<FileIndex>>,
    refresh: Option<Arc<FileIndex>>,
    worker: Option<FuzzyWorker>,
    query: String,
    sent: Option<(String, usize, usize)>,
    results: Vec<FuzzyMatch>,
    total: usize,
    selected: usize,
    focus_input: bool,
    scroll_to_selected: bool,
    recent: Vec<PathBuf>,
}

impl Default for QuickOpen {
    fn default() -> Self {
        Self::new()
    }
}

impl QuickOpen {
    pub fn new() -> Self {
        Self {
            id: Id::new("kiln-quick-open"),
            open: false,
            root: None,
            index: None,
            refresh: None,
            worker: None,
            query: String::new(),
            sent: None,
            results: Vec::new(),
            total: 0,
            selected: 0,
            focus_input: false,
            scroll_to_selected: false,
            recent: Vec::new(),
        }
    }

    /// 모달을 연다. 같은 루트면 이전 목록을 즉시 보여 주고 백그라운드로 다시 수집한다.
    pub fn open(&mut self, root: impl Into<PathBuf>) {
        let root = root.into();
        if self.root.as_ref() != Some(&root) {
            if let Some(i) = self.index.take() {
                i.cancel();
            }
            self.refresh = None;
            self.results.clear();
            self.total = 0;
            self.root = Some(root);
        }
        self.open = true;
        self.query.clear();
        self.sent = None;
        self.selected = 0;
        self.focus_input = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 파일 수집이 진행 중인지.
    pub fn is_indexing(&self) -> bool {
        self.index.as_ref().is_some_and(|i| !i.is_done()) || self.refresh.is_some()
    }

    /// 수집된 파일 수.
    pub fn file_count(&self) -> usize {
        self.index.as_ref().map_or(0, |i| i.len())
    }

    /// 현재 결과 목록(상대 경로).
    pub fn result_paths(&self) -> Vec<String> {
        self.results.iter().map(|m| m.path.clone()).collect()
    }

    /// 최근에 연 파일로 기록한다(빈 질의일 때 위에 표시).
    pub fn note_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(20);
    }

    fn ensure_index(&mut self, ctx: &egui::Context) {
        let Some(root) = self.root.clone() else { return };
        let repaint = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match &self.index {
            None => self.index = Some(FileIndex::spawn(root, repaint)),
            Some(i) if i.is_done() && self.refresh.is_none() && i.started.elapsed() > REINDEX_AFTER && self.open && self.sent.is_none() => {
                self.refresh = Some(FileIndex::spawn(root, repaint));
            }
            _ => {}
        }
        if self.refresh.as_ref().is_some_and(|r| r.is_done()) {
            self.index = self.refresh.take();
            self.sent = None;
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        let Some(index) = self.index.clone() else { return };
        let worker = self.worker.get_or_insert_with(|| {
            let ctx = ctx.clone();
            FuzzyWorker::new(move || ctx.request_repaint())
        });
        let len = index.len();
        let key = (self.query.clone(), len, Arc::as_ptr(&index) as usize);
        if self.sent.as_ref() != Some(&key) {
            // 수집 중에는 목록이 충분히 늘었을 때만 다시 계산한다.
            let growing_only = self.sent.as_ref().is_some_and(|s| s.0 == key.0 && s.2 == key.2) && !index.is_done();
            let len_changed_enough = self.sent.as_ref().is_none_or(|s| len >= s.1 + s.1 / 4 + 256);
            if !growing_only || len_changed_enough {
                worker.submit(index.clone(), &self.query, RESULT_LIMIT);
                self.sent = Some(key);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(r) = worker.poll() {
            let query_changed = self.results.is_empty() || r.set.query != self.query.trim();
            self.results = r.set.matches;
            self.total = r.set.total;
            if self.query.trim().is_empty() {
                self.prepend_recent(&index);
            }
            if query_changed || self.selected >= self.results.len() {
                self.selected = 0;
            }
        }
    }

    fn prepend_recent(&mut self, index: &FileIndex) {
        let mut front = Vec::new();
        for p in &self.recent {
            if let Ok(rel) = p.strip_prefix(&index.root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                self.results.retain(|m| m.path != rel);
                front.push(FuzzyMatch { index: u32::MAX, score: 0, path: rel, indices: Vec::new() });
            }
        }
        front.append(&mut self.results);
        self.results = front;
        self.results.truncate(RESULT_LIMIT);
    }

    /// 모달을 그린다. 파일을 고르면 절대 경로를 돌려주고 닫힌다.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        self.ensure_index(ctx);
        let t = Theme::current();
        let mut chosen: Option<usize> = None;

        // 목록 탐색 키는 입력칸보다 먼저 가져간다.
        let (up, down, pgup, pgdn, enter, esc) = ctx.input_mut(|i| {
            (
                i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp) + i.count_and_consume_key(Modifiers::CTRL, Key::P),
                i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown) + i.count_and_consume_key(Modifiers::CTRL, Key::N),
                i.count_and_consume_key(Modifiers::NONE, Key::PageUp),
                i.count_and_consume_key(Modifiers::NONE, Key::PageDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            self.open = false;
            return None;
        }
        let n = self.results.len();
        if n > 0 {
            let mut s = self.selected as isize;
            s -= up as isize;
            s += down as isize;
            s -= (pgup * MAX_ROWS) as isize;
            s += (pgdn * MAX_ROWS) as isize;
            let s = s.clamp(0, n as isize - 1) as usize;
            if s != self.selected {
                self.selected = s;
                self.scroll_to_selected = true;
            }
        }
        if enter && n > 0 {
            chosen = Some(self.selected);
        }

        let screen = ctx.content_rect();
        let width = (screen.width() - 40.0).clamp(280.0, 640.0);
        let area = egui::Area::new(self.id)
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, (screen.height() * 0.08).clamp(12.0, 80.0)))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(8)
                    .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) })
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_width(width - 12.0);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        self.input_ui(ui);
                        ui.add_space(6.0);
                        self.pump(ui.ctx());
                        if let Some(i) = self.list_ui(ui) {
                            chosen = Some(i);
                        }
                        self.footer_ui(ui);
                    });
            });

        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !area.response.rect.contains(p));
        if clicked_outside {
            self.open = false;
        }
        let root = self.index.as_ref().map(|i| i.root.clone())?;
        let i = chosen?;
        let rel = self.results.get(i)?.path.clone();
        let abs = root.join(rel);
        self.note_recent(&abs);
        self.open = false;
        Some(abs)
    }

    fn input_ui(&mut self, ui: &mut egui::Ui) {
        let t = Theme::current();
        let id = self.id.with("input");
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
        ui.painter().rect_filled(rect, 5.0, t.bg);
        let focused = ui.memory(|m| m.has_focus(id));
        let stroke = if focused { t.accent.gamma_multiply(0.8) } else { t.border };
        ui.painter().rect_stroke(rect, 5.0, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        ui_kit::paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(15.0, 15.0)), Icon::Search, t.text_dim);
        let inner = Rect::from_min_max(pos2(rect.left() + 34.0, rect.top() + 4.0), pos2(rect.right() - 8.0, rect.bottom() - 4.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            let out = egui::TextEdit::singleline(&mut self.query)
                .id(id)
                .frame(egui::Frame::NONE)
                .hint_text(egui::RichText::new("Search files by name").color(t.text_faint))
                .font(FontId::proportional(14.5))
                .text_color(t.text)
                .desired_width(inner.width())
                .margin(vec2(0.0, 4.0))
                .return_key(None)
                .show(ui);
            if self.focus_input || !out.response.has_focus() {
                out.response.request_focus();
                self.focus_input = false;
            }
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        let t = Theme::current();
        let n = self.results.len();
        if n == 0 {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            let msg = if self.index.as_ref().is_some_and(|i| !i.is_done()) && self.file_count() == 0 {
                "Indexing files…"
            } else if self.query.trim().is_empty() {
                "No files"
            } else {
                "No matching files"
            };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), t.text_dim);
            return None;
        }
        let mut clicked = None;
        let visible = n.min(MAX_ROWS);
        let mut sa = egui::ScrollArea::vertical()
            .id_salt(self.id.with("list"))
            .max_height(visible as f32 * ROW_H)
            .auto_shrink([false, true]);
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            let off = ui.ctx().data(|d| d.get_temp::<f32>(self.id.with("off"))).unwrap_or(0.0);
            let y = self.selected as f32 * ROW_H;
            let vh = visible as f32 * ROW_H;
            if y < off {
                sa = sa.vertical_scroll_offset(y);
            } else if y + ROW_H > off + vh {
                sa = sa.vertical_scroll_offset(y + ROW_H - vh);
            }
        }
        let out = sa.show_rows(ui, ROW_H, n, |ui, range| {
            for i in range {
                let m = &self.results[i];
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let selected = i == self.selected;
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 5.0, t.bg_selected);
                } else if resp.hovered() {
                    p.rect_filled(rect, 5.0, t.bg_hover);
                }
                let name_start = m.path.rfind('/').map_or(0, |k| k + 1);
                let name = &m.path[name_start..];
                ui_kit::paint_file_badge(p, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(16.0, 16.0)), name);
                let name_char_start = m.path[..name_start].chars().count() as u32;
                let mut job = LayoutJob::default();
                let hl = |job: &mut LayoutJob, text: &str, offset: u32, base: Color32, size: f32, indices: &[u32]| {
                    for (k, ch) in text.chars().enumerate() {
                        let hit = indices.binary_search(&(offset + k as u32)).is_ok();
                        let mut buf = [0u8; 4];
                        job.append(
                            ch.encode_utf8(&mut buf),
                            0.0,
                            TextFormat {
                                font_id: FontId::proportional(size),
                                color: if hit { t.accent } else { base },
                                underline: if hit { Stroke::new(1.0, t.accent.gamma_multiply(0.6)) } else { Stroke::NONE },
                                ..Default::default()
                            },
                        );
                    }
                };
                hl(&mut job, name, name_char_start, t.text, 13.5, &m.indices);
                let dir = m.path[..name_start].trim_end_matches('/');
                if !dir.is_empty() {
                    job.append("   ", 0.0, TextFormat { font_id: FontId::proportional(12.0), ..Default::default() });
                    hl(&mut job, dir, 0, t.text_dim, 12.0, &m.indices);
                }
                if m.index == u32::MAX && self.query.trim().is_empty() {
                    job.append("   recently opened", 0.0, TextFormat { font_id: FontId::proportional(11.0), color: t.text_faint, ..Default::default() });
                }
                job.wrap.max_width = rect.width() - 44.0;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &m.path));
                if resp.clicked() {
                    clicked = Some(i);
                }
                if resp.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                    self.selected = i;
                }
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(self.id.with("off"), out.state.offset.y));
        clicked
    }

    fn footer_ui(&self, ui: &mut egui::Ui) {
        let t = Theme::current();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        let p = ui.painter();
        p.line_segment([pos2(rect.left(), rect.top() + 2.0), pos2(rect.right(), rect.top() + 2.0)], Stroke::new(1.0, t.border));
        let count = self.file_count();
        let left = if self.query.trim().is_empty() {
            format!("{} files", fmt_count(count))
        } else {
            format!("{} of {} files", fmt_count(self.total), fmt_count(count))
        };
        let left = if self.is_indexing() { format!("{left} · indexing…") } else { left };
        p.text(pos2(rect.left() + 8.0, rect.center().y + 2.0), Align2::LEFT_CENTER, left, FontId::proportional(11.0), t.text_faint);
        let mut x = rect.right() - 8.0;
        for (keys, label) in [(&["Esc"][..], "close"), (&["Enter"][..], "open"), (&["↓", "↑"][..], "navigate")] {
            let g = p.layout_no_wrap(label.to_owned(), FontId::proportional(11.0), t.text_faint);
            x -= g.size().x;
            p.galley(pos2(x, rect.center().y + 2.0 - g.size().y / 2.0), g, t.text_faint);
            x -= 5.0;
            for k in keys {
                let w = if k.len() > 1 { 8.0 + k.len() as f32 * 5.5 } else { 16.0 };
                let r = Rect::from_min_max(pos2(x - w, rect.center().y - 6.0), pos2(x, rect.center().y + 10.0));
                p.rect_filled(r, 3.0, t.bg_hover);
                p.rect_stroke(r, 3.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                match *k {
                    "↑" => ui_kit::paint_icon(p, r.shrink(3.0), Icon::ArrowUp, t.text_dim),
                    "↓" => ui_kit::paint_icon(p, r.shrink(3.0), Icon::ArrowDown, t.text_dim),
                    _ => {
                        p.text(r.center(), Align2::CENTER_CENTER, *k, FontId::proportional(10.0), t.text_dim);
                    }
                }
                x = r.left() - 3.0;
            }
            x -= 10.0;
        }
    }
}

fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn count_formatting() {
        assert_eq!(super::fmt_count(0), "0");
        assert_eq!(super::fmt_count(1234), "1,234");
        assert_eq!(super::fmt_count(100000), "100,000");
    }
}
