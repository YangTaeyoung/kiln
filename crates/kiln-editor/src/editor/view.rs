//! 편집기 화면: 보이는 줄만 배치·그리기, 줄 번호, 선택, 커서, 키보드·마우스·IME 입력.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::os::OperatingSystem;
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, CursorIcon, Event, EventFilter, FontId, Galley, ImeEvent, Key, Layout,
    Modifiers, Pos2, Rect, Response, ScrollArea, Sense, Stroke, StrokeKind, TextStyle, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use kiln_common::Theme;

use super::{Editor, Motion, Reveal, char_cols};
use crate::buffer::{Pos, Selection};
use crate::ui_kit::{self, Icon};

const PAD_LEFT: f32 = 6.0;
const GUTTER_PAD_LEFT: f32 = 14.0;
const GUTTER_PAD_RIGHT: f32 = 14.0;
const BANNER_H: f32 = 30.0;
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(6);
const IDLE_HIGHLIGHT_BUDGET: Duration = Duration::from_millis(3);
const LOOKAHEAD_LINES: usize = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DragMode {
    #[default]
    None,
    Char,
    Word(Pos, Pos),
    Line(usize),
}

#[derive(Default)]
pub(crate) struct ViewState {
    pub scroll: Vec2,
    pub viewport: Vec2,
    pub row_h: f32,
    pub char_w: f32,
    pub font: Option<FontId>,
    pub last_interaction: f64,
    pub preedit: Option<String>,
    pub overlays: Vec<Rect>,
    pub drag: DragMode,
    max_cols: (u64, usize),
    /// 마지막 프레임에 그린 줄 범위.
    pub drawn: (usize, usize),
}

impl ViewState {
    fn update_metrics(&mut self, ui: &Ui) -> FontId {
        let font = TextStyle::Monospace.resolve(ui.style());
        if self.font.as_ref() != Some(&font) {
            let (w, h) = ui.ctx().fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
            self.char_w = w;
            self.row_h = (h * 1.5).round().max(h + 4.0);
            self.font = Some(font.clone());
        }
        font
    }
}

fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().count()
}

fn char_to_byte(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(i, _)| i)
}

impl Editor {
    /// 편집기를 그리고 입력을 처리한다. 사용 가능한 영역 전체를 차지한다.
    pub fn ui(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        let font = self.view.update_metrics(ui);
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        ui.painter().rect_filled(full, 0.0, t.bg);
        if self.is_binary() {
            self.binary_placeholder(ui, full);
            return;
        }
        let body = self.banners_ui(ui, full);

        let focused = ui.memory(|m| m.has_focus(self.id()));
        let overlay_focus = ui.memory(|m| {
            m.has_focus(self.find_query_id()) || m.has_focus(self.find_replace_id()) || m.has_focus(self.goto_id())
        });
        if focused || overlay_focus {
            self.handle_shortcuts(ui);
        }
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    self.id(),
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false },
                )
            });
            self.handle_keyboard(ui);
        }
        self.update_find_matches();

        let digits = (self.buf.line_count().max(1) as f32).log10().floor() as usize + 1;
        let gutter_w = (GUTTER_PAD_LEFT + digits.max(3) as f32 * self.view.char_w + GUTTER_PAD_RIGHT).round();
        let gutter = Rect::from_min_max(body.min, pos2(body.left() + gutter_w, body.bottom()));
        let text_rect = Rect::from_min_max(pos2(gutter.right(), body.top()), body.max);

        let offset_req = self.reveal.take().map(|r| self.reveal_offset(ui, r, text_rect.size()));
        let mut child = ui.new_child(
            UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)).id_salt(self.id().with("text")),
        );
        child.set_clip_rect(text_rect.intersect(ui.clip_rect()));
        let mut sa = ScrollArea::both()
            .id_salt(self.id().with("scroll"))
            .auto_shrink([false, false])
            .content_margin(egui::Margin::ZERO);
        if let Some(o) = offset_req {
            sa = sa.scroll_offset(o);
        }
        let out = sa.show_viewport(&mut child, |ui, vp| self.text_ui(ui, vp, &font, focused));
        self.view.scroll = out.state.offset;
        self.view.viewport = out.inner_rect.size();

        self.gutter_ui(ui, gutter, &font);
        if ui.memory(|m| m.has_focus(self.id())) {
            self.ime_output(ui, out.inner_rect);
        }

        self.view.overlays.clear();
        if self.find.open {
            let r = self.find_bar_ui(ui, text_rect);
            self.view.overlays.push(r);
        }
        if let Some(r) = self.goto_ui(ui, text_rect) {
            self.view.overlays.push(r);
        }
    }

    fn binary_placeholder(&self, ui: &mut Ui, rect: Rect) {
        let t = Theme::current();
        let p = ui.painter_at(rect);
        let c = rect.center();
        ui_kit::paint_icon(&p, Rect::from_center_size(c - vec2(0.0, 34.0), vec2(28.0, 28.0)), Icon::Warning, t.yellow);
        p.text(c, Align2::CENTER_CENTER, "바이너리 파일은 표시하지 않습니다", egui::FontId::proportional(15.0), t.text);
        p.text(
            c + vec2(0.0, 22.0),
            Align2::CENTER_CENTER,
            format!("{} · {}", self.title(), ui_kit::size_label(self.file_len)),
            egui::FontId::proportional(12.0),
            t.text_dim,
        );
    }

    /// 상단 알림 막대들을 그리고 남은 영역을 돌려준다.
    fn banners_ui(&mut self, ui: &mut Ui, full: Rect) -> Rect {
        let t = Theme::current();
        let mut top = full.top();
        #[derive(PartialEq)]
        enum B {
            Conflict,
            Deleted,
            Lossy,
            Large,
            SaveError,
        }
        let mut list = Vec::new();
        if self.conflict {
            list.push(B::Conflict);
        }
        if self.deleted_on_disk {
            list.push(B::Deleted);
        }
        if self.encoding == super::Encoding::Utf8Lossy && self.read_only {
            list.push(B::Lossy);
        }
        if self.large_banner {
            list.push(B::Large);
        }
        if self.save_error.is_some() {
            list.push(B::SaveError);
        }
        for b in list {
            let rect = Rect::from_min_size(pos2(full.left(), top), vec2(full.width(), BANNER_H));
            top += BANNER_H;
            let (accent, msg) = match b {
                B::Conflict => (t.yellow, "저장하지 않은 편집 내용이 있는 동안 디스크의 파일이 변경되었습니다.".to_owned()),
                B::Deleted => (t.red, "디스크에서 파일이 삭제되었습니다. 저장하면 다시 만들어집니다.".to_owned()),
                B::Lossy => (t.yellow, "올바른 UTF-8 파일이 아닙니다 — 손상을 막기 위해 읽기 전용으로 열었습니다.".to_owned()),
                B::Large => (
                    t.blue,
                    format!("큰 파일({}) — 구문 강조를 끕니다.", ui_kit::size_label(self.file_len)),
                ),
                B::SaveError => (t.red, format!("저장 실패: {}", self.save_error.clone().unwrap_or_default())),
            };
            let p = ui.painter();
            p.rect_filled(rect, 0.0, accent.gamma_multiply(0.13));
            p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, accent.gamma_multiply(0.35)));
            p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, accent);
            let icon_rect = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(15.0, 15.0));
            ui_kit::paint_icon(p, icon_rect, Icon::Warning, accent);
            let inner = Rect::from_min_max(pos2(rect.left() + 34.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
            ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| {
                ui.label(egui::RichText::new(msg).size(12.5).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match b {
                    B::Conflict => {
                        if ui_kit::flat_button(ui, "내 변경 유지", false).clicked() {
                            self.keep_local_changes();
                        }
                        if ui_kit::flat_button(ui, "다시 불러오기", true).clicked()
                            && let Err(e) = self.reload_from_disk()
                        {
                            self.save_error = Some(format!("{e:#}"));
                        }
                    }
                    B::Lossy => {
                        if ui_kit::flat_button(ui, "그래도 편집", false).clicked() {
                            self.read_only = false;
                        }
                    }
                    B::Large => {
                        if ui_kit::icon_button(ui, Icon::Close, "닫기").clicked() {
                            self.large_banner = false;
                        }
                    }
                    B::SaveError => {
                        if ui_kit::icon_button(ui, Icon::Close, "닫기").clicked() {
                            self.save_error = None;
                        }
                    }
                    B::Deleted => {}
                });
            });
        }
        Rect::from_min_max(pos2(full.left(), top), full.max)
    }

    // ---- 레이아웃 ----

    fn line_job(&self, i: usize, font: &FontId, with_preedit: bool) -> LayoutJob {
        let t = Theme::current();
        let line = self.buf.line(i);
        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        job.break_on_newline = false;
        let fmt = |color: Color32, italics: bool, underline: bool| TextFormat {
            font_id: font.clone(),
            color,
            italics,
            underline: if underline { Stroke::new(1.0, color) } else { Stroke::NONE },
            ..Default::default()
        };
        let preedit = if with_preedit && i == self.sel.head.line { self.view.preedit.as_deref() } else { None };
        let push = |job: &mut LayoutJob, text: &str, f: TextFormat| {
            if !text.is_empty() {
                job.append(text, 0.0, f);
            }
        };
        let mut pieces: Vec<(usize, usize, TextFormat)> = Vec::new();
        match self.hl.line_spans(i) {
            Some(spans) => {
                let mut pos = 0usize;
                for s in spans {
                    let (a, b) = (s.start as usize, s.end as usize);
                    if a > pos {
                        pieces.push((pos, a, fmt(t.text, false, false)));
                    }
                    pieces.push((a, b, fmt(s.color, s.italic, s.underline)));
                    pos = b;
                }
                if pos < line.len() {
                    pieces.push((pos, line.len(), fmt(t.text, false, false)));
                }
            }
            None => pieces.push((0, line.len(), fmt(t.text, false, false))),
        }
        let caret = self.sel.head.col;
        for (a, b, f) in pieces {
            match preedit {
                Some(pre) if a <= caret && caret <= b && (caret < b || b == line.len()) => {
                    push(&mut job, &line[a..caret], f.clone());
                    push(&mut job, pre, fmt(t.text, false, true));
                    push(&mut job, &line[caret..b], f);
                }
                _ => push(&mut job, &line[a..b], f),
            }
        }
        if line.is_empty()
            && let Some(pre) = preedit
        {
            push(&mut job, pre, fmt(t.text, false, true));
        }
        job
    }

    fn line_galley(&self, ui: &Ui, i: usize, font: &FontId, with_preedit: bool) -> Arc<Galley> {
        ui.painter().layout_job(self.line_job(i, font, with_preedit))
    }

    fn x_of(&self, galley: &Galley, line: usize, col: usize) -> f32 {
        galley.pos_from_cursor(CCursor::new(byte_to_char(self.buf.line(line), col))).min.x
    }

    fn max_cols(&mut self) -> usize {
        let v = self.buf.version();
        if self.view.max_cols.0 != v || self.view.max_cols.1 == 0 {
            let tab = self.indent.width();
            let m = self
                .buf
                .lines()
                .iter()
                .map(|l| if l.is_ascii() && !l.contains('\t') { l.len() } else { l.chars().map(|c| char_cols(c, tab)).sum() })
                .max()
                .unwrap_or(0);
            self.view.max_cols = (v, m.max(1));
        }
        self.view.max_cols.1
    }

    /// 커서가 보이도록 하는 스크롤 오프셋.
    fn reveal_offset(&self, ui: &Ui, how: Reveal, view: Vec2) -> Vec2 {
        let row_h = self.view.row_h;
        let font = self.view.font.clone().unwrap_or_else(|| TextStyle::Monospace.resolve(ui.style()));
        let head = self.sel.head;
        let g = self.line_galley(ui, head.line, &font, false);
        let x = PAD_LEFT + self.x_of(&g, head.line, head.col);
        let y = head.line as f32 * row_h;
        let mut off = self.view.scroll;
        let vw = if self.view.viewport.x > 0.0 { self.view.viewport.x } else { view.x };
        let vh = if self.view.viewport.y > 0.0 { self.view.viewport.y } else { view.y };
        match how {
            Reveal::Center => {
                if y < off.y || y + row_h > off.y + vh {
                    off.y = (y - vh / 2.0 + row_h / 2.0).max(0.0);
                }
            }
            Reveal::Nearest => {
                let margin = (row_h * 2.0).min(vh / 3.0);
                if y - margin < off.y {
                    off.y = (y - margin).max(0.0);
                } else if y + row_h + margin > off.y + vh {
                    off.y = y + row_h + margin - vh;
                }
            }
        }
        let xm = self.view.char_w * 4.0;
        if x - xm < off.x {
            off.x = (x - xm).max(0.0);
        } else if x + xm > off.x + vw {
            off.x = x + xm - vw;
        }
        off
    }

    // ---- 본문 ----

    fn text_ui(&mut self, ui: &mut Ui, vp: Rect, font: &FontId, focused: bool) {
        let t = Theme::current();
        let row_h = self.view.row_h;
        let n = self.buf.line_count();
        let origin = ui.max_rect().min;
        let content_w = PAD_LEFT + self.max_cols() as f32 * self.view.char_w + self.view.char_w * 12.0;
        let content_h = n as f32 * row_h + (vp.height() - row_h * 3.0).max(0.0);
        ui.set_min_size(vec2(content_w.max(vp.width()), content_h.max(vp.height())));

        let first = ((vp.min.y / row_h).floor().max(0.0) as usize).min(n);
        let last = (((vp.max.y / row_h).ceil().max(0.0)) as usize).min(n);
        self.view.drawn = (first, last);

        let deadline = Instant::now() + HIGHLIGHT_BUDGET;
        let done = self.hl.ensure(self.buf.lines(), last, Some(deadline));
        if !done {
            ui.ctx().request_repaint();
        } else if self.hl.valid_lines() < n {
            // 보이는 줄이 끝났으면 남은 줄을 유휴 프레임마다 조금씩 미리 계산한다.
            let idle_deadline = Instant::now() + IDLE_HIGHLIGHT_BUDGET;
            let target = if self.hl.valid_lines() < (last + LOOKAHEAD_LINES).min(n) { last + LOOKAHEAD_LINES } else { n };
            self.hl.ensure(self.buf.lines(), target, Some(idle_deadline));
            if self.hl.valid_lines() < n {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }

        let screen_vp = vp.translate(origin.to_vec2());
        let resp = ui.interact(screen_vp, self.id(), Sense::click_and_drag());
        self.handle_mouse(ui, &resp, origin, font);
        self.context_menu(ui, &resp);

        let painter = ui.painter_at(screen_vp);
        let text_x = origin.x + PAD_LEFT;
        let (sa, sb) = self.sel.range();
        let head = self.sel.head;
        let has_focus = ui.memory(|m| m.has_focus(self.id()));

        // 들여쓰기 안내선 계산용: 빈 줄은 다음 비어 있지 않은 줄의 들여쓰기를 따른다.
        let tab = self.indent.width();
        let unit_px = self.indent.width() as f32 * self.view.char_w;
        let indent_cols = |ed: &Editor, i: usize| -> Option<usize> {
            let l = ed.buf.line(i);
            if l.trim().is_empty() {
                return None;
            }
            Some(l.chars().take_while(|c| c.is_whitespace()).map(|c| char_cols(c, tab)).sum())
        };
        let mut next_indent: Option<usize> = None;

        let matches = &self.find.matches;
        let first_match = matches.partition_point(|m| m.0.line < first);

        let mut mi = first_match;
        for i in first..last {
            let y = origin.y + i as f32 * row_h;
            let row = Rect::from_min_max(pos2(screen_vp.left(), y), pos2(screen_vp.right(), y + row_h));
            if i == head.line && sa == sb {
                painter.rect_filled(row, 0.0, t.bg_panel);
            }
            let g = self.line_galley(ui, i, font, true);
            let line_len = self.buf.line(i).len();

            // 들여쓰기 안내선
            let ind = match indent_cols(self, i) {
                Some(c) => {
                    next_indent = None;
                    c
                }
                None => {
                    if next_indent.is_none() {
                        let mut j = i + 1;
                        let mut v = 0;
                        while j < n && j < i + 200 {
                            if let Some(c) = indent_cols(self, j) {
                                v = c;
                                break;
                            }
                            j += 1;
                        }
                        next_indent = Some(v);
                    }
                    next_indent.unwrap_or(0)
                }
            };
            if unit_px > 0.0 && tab > 0 {
                let levels = ind / tab;
                for lv in 1..levels.clamp(1, 40) {
                    let x = (text_x + lv as f32 * unit_px).round() + 0.5;
                    painter.line_segment([pos2(x, y), pos2(x, y + row_h)], Stroke::new(1.0, t.border.gamma_multiply(0.8)));
                }
            }

            // 찾기 일치
            while mi < matches.len() && matches[mi].0.line == i {
                let (a, b) = matches[mi];
                let x0 = text_x + self.x_of(&g, i, a.col);
                let x1 = text_x + self.x_of(&g, i, if b.line == i { b.col } else { line_len });
                let r = Rect::from_min_max(pos2(x0, y + 2.0), pos2(x1.max(x0 + 2.0), y + row_h - 2.0));
                if self.find.current == Some(mi) {
                    painter.rect_filled(r, 2.0, t.orange.gamma_multiply(0.45));
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0, t.orange.gamma_multiply(0.9)), StrokeKind::Inside);
                } else {
                    painter.rect_filled(r, 2.0, t.yellow.gamma_multiply(0.22));
                }
                mi += 1;
            }

            // 선택 영역
            if sa != sb && sa.line <= i && i <= sb.line {
                let c0 = if i == sa.line { sa.col } else { 0 };
                let c1 = if i == sb.line { sb.col } else { line_len };
                let x0 = text_x + self.x_of(&g, i, c0);
                let mut x1 = text_x + self.x_of(&g, i, c1);
                if i != sb.line {
                    x1 += self.view.char_w * 0.6;
                }
                let fill = if has_focus { t.bg_selected } else { t.bg_selected.gamma_multiply(0.6) };
                painter.rect_filled(Rect::from_min_max(pos2(x0, y + 1.0), pos2(x1, y + row_h - 1.0)), 3.0, fill);
            }

            let gy = y + ((row_h - g.size().y) / 2.0).round();
            painter.galley(pos2(text_x, gy), g.clone(), t.text);

            // 커서
            if i == head.line && has_focus && !self.read_only {
                let pre_chars = self.view.preedit.as_deref().map_or(0, |p| p.chars().count());
                let cc = byte_to_char(self.buf.line(i), head.col) + pre_chars;
                let x = text_x + g.pos_from_cursor(CCursor::new(cc)).min.x;
                let now = ui.input(|inp| inp.time);
                let since = now - self.view.last_interaction;
                let blink = ui.visuals().text_cursor.blink;
                let on = !blink || since < 0.6 || ((since - 0.6) % 1.0) < 0.5;
                if on {
                    let r = Rect::from_min_size(pos2(x.round() - 1.0, y + 3.0), vec2(2.0, row_h - 6.0));
                    painter.rect_filled(r, 1.0, t.accent);
                }
                if blink && ui.input(|inp| inp.focused) {
                    let phase = if since < 0.6 { 0.6 - since } else { 0.5 - ((since - 0.6) % 0.5) };
                    ui.ctx().request_repaint_after(Duration::from_secs_f64(phase.max(0.02)));
                }
            }
        }
        if !focused && resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
    }

    fn pos_at(&self, ui: &Ui, p: Pos2, origin: Pos2, font: &FontId) -> Pos {
        let row_h = self.view.row_h;
        let n = self.buf.line_count();
        let fy = (p.y - origin.y) / row_h;
        if fy < 0.0 {
            return Pos::new(0, 0);
        }
        let line = fy.floor() as usize;
        if line >= n {
            return self.buf.end_pos();
        }
        let g = self.line_galley(ui, line, font, false);
        let x = p.x - origin.x - PAD_LEFT;
        let cc = g.cursor_from_pos(vec2(x, g.size().y / 2.0));
        Pos::new(line, char_to_byte(self.buf.line(line), cc.index.0))
    }

    fn handle_mouse(&mut self, ui: &Ui, resp: &Response, origin: Pos2, font: &FontId) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let over_overlay = pointer.is_some_and(|p| self.view.overlays.iter().any(|r| r.contains(p)));
        if resp.hovered() && !over_overlay {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        let (pressed, shift, clicks) = ui.input(|i| {
            let clicks = if i.pointer.button_triple_clicked(egui::PointerButton::Primary) {
                3
            } else if i.pointer.button_double_clicked(egui::PointerButton::Primary) {
                2
            } else {
                1
            };
            (i.pointer.primary_pressed(), i.modifiers.shift, clicks)
        });
        if resp.hovered() && !over_overlay && pressed && let Some(p) = pointer {
            ui.memory_mut(|m| m.request_focus(self.id()));
            let pos = self.pos_at(ui, p, origin, font);
            self.preferred_col = None;
            self.view.last_interaction = ui.input(|i| i.time);
            if shift {
                self.sel.head = pos;
                self.view.drag = DragMode::Char;
            } else {
                self.sel = Selection::caret(pos);
                self.view.drag = DragMode::Char;
            }
        }
        if resp.double_clicked() && !over_overlay && let Some(p) = pointer {
            let pos = self.pos_at(ui, p, origin, font);
            let (a, b) = self.buf.word_at(pos);
            self.sel = Selection::new(a, b);
            self.view.drag = DragMode::Word(a, b);
            let _ = clicks;
        }
        if resp.triple_clicked() && !over_overlay && let Some(p) = pointer {
            let pos = self.pos_at(ui, p, origin, font);
            self.select_line_range(pos.line, pos.line);
            self.view.drag = DragMode::Line(pos.line);
        }
        if resp.dragged() && self.view.drag != DragMode::None && let Some(p) = pointer {
            let pos = self.pos_at(ui, p, origin, font);
            match self.view.drag {
                DragMode::Char => self.sel.head = pos,
                DragMode::Word(a, b) => {
                    let (wa, wb) = self.buf.word_at(pos);
                    self.sel = if pos < a { Selection::new(b, wa) } else { Selection::new(a, wb.max(b)) };
                }
                DragMode::Line(l) => self.select_line_range(l, pos.line),
                DragMode::None => {}
            }
            // 가장자리 밖으로 끌면 자동 스크롤
            let clip = resp.rect;
            let mut d = Vec2::ZERO;
            if p.y < clip.top() {
                d.y = (clip.top() - p.y).min(60.0);
            } else if p.y > clip.bottom() {
                d.y = -(p.y - clip.bottom()).min(60.0);
            }
            if p.x < clip.left() {
                d.x = (clip.left() - p.x).min(60.0);
            } else if p.x > clip.right() {
                d.x = -(p.x - clip.right()).min(60.0);
            }
            if d != Vec2::ZERO {
                ui.scroll_with_delta(d * 0.5);
                ui.ctx().request_repaint();
            }
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            self.view.drag = DragMode::None;
        }
    }

    /// `a..=b` 줄 전체를 선택한다(마지막 줄바꿈 포함).
    fn select_line_range(&mut self, a: usize, b: usize) {
        let n = self.buf.line_count();
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let end = if hi + 1 < n { Pos::new(hi + 1, 0) } else { Pos::new(hi, self.buf.line(hi).len()) };
        self.sel = if b < a { Selection::new(end, Pos::new(lo, 0)) } else { Selection::new(Pos::new(lo, 0), end) };
    }

    fn context_menu(&mut self, ui: &Ui, resp: &Response) {
        resp.context_menu(|ui| {
            ui.set_min_width(180.0);
            let ro = self.read_only;
            if ui.add_enabled(!ro, egui::Button::new("잘라내기")).clicked() {
                let text = self.cut();
                ui.ctx().copy_text(text);
                ui.close();
            }
            if ui.button("복사").clicked() {
                ui.ctx().copy_text(self.copy_text());
                ui.close();
            }
            ui.separator();
            if ui.button("모두 선택").clicked() {
                self.select_all();
                ui.close();
            }
            if ui.add_enabled(!ro && self.comment.is_some(), egui::Button::new("줄 주석 전환")).clicked() {
                self.toggle_comment();
                ui.close();
            }
            ui.separator();
            if ui.button("찾기…").clicked() {
                self.open_find(false);
                ui.close();
            }
            if ui.button("줄로 이동…").clicked() {
                self.goto = Some(String::new());
                ui.close();
            }
        });
        let _ = ui;
    }

    // ---- 줄 번호 ----

    fn gutter_ui(&mut self, ui: &mut Ui, gutter: Rect, font: &FontId) {
        let t = Theme::current();
        let row_h = self.view.row_h;
        let n = self.buf.line_count();
        let clip = gutter.intersect(ui.clip_rect());
        let painter = ui.painter_at(clip);
        painter.rect_filled(gutter, 0.0, t.bg);
        let top = gutter.top() - self.view.scroll.y;
        let first = ((self.view.scroll.y / row_h).floor().max(0.0) as usize).min(n);
        let last = (((self.view.scroll.y + gutter.height()) / row_h).ceil() as usize).min(n);
        let (sa, sb) = self.sel.range();
        let head = self.sel.head.line;
        for i in first..last {
            let y = top + i as f32 * row_h;
            let in_sel = sa != sb && sa.line <= i && i <= sb.line;
            let color = if i == head { t.text } else if in_sel { t.text_dim } else { t.text_faint };
            if i == head && sa == sb {
                painter.rect_filled(Rect::from_min_max(pos2(gutter.left(), y), pos2(gutter.right(), y + row_h)), 0.0, t.bg_panel);
            }
            painter.text(
                pos2(gutter.right() - GUTTER_PAD_RIGHT, y + row_h / 2.0),
                Align2::RIGHT_CENTER,
                (i + 1).to_string(),
                font.clone(),
                color,
            );
        }
        let resp = ui.interact(gutter, self.id().with("gutter"), Sense::click_and_drag());
        if let Some(p) = resp.interact_pointer_pos() {
            let line = (((p.y - top) / row_h).floor().max(0.0) as usize).min(n - 1);
            if resp.drag_started() || resp.clicked() || ui.input(|i| i.pointer.primary_pressed()) && resp.hovered() {
                ui.memory_mut(|m| m.request_focus(self.id()));
                self.view.drag = DragMode::Line(line);
                self.select_line_range(line, line);
            } else if resp.dragged() && let DragMode::Line(a) = self.view.drag {
                self.select_line_range(a, line);
            }
        }
    }

    fn ime_output(&self, ui: &Ui, inner: Rect) {
        let row_h = self.view.row_h;
        let head = self.sel.head;
        let font = match &self.view.font {
            Some(f) => f.clone(),
            None => return,
        };
        let g = self.line_galley(ui, head.line, &font, false);
        let x = inner.left() + PAD_LEFT + self.x_of(&g, head.line, head.col) - self.view.scroll.x;
        let y = inner.top() + head.line as f32 * row_h - self.view.scroll.y;
        let cursor = Rect::from_min_size(pos2(x, y), vec2(2.0, row_h));
        let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
        ui.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect: to_global * inner,
                cursor_rect: to_global * cursor,
                should_interrupt_composition: false,
            });
        });
    }

    // ---- 키보드 ----

    fn handle_shortcuts(&mut self, ui: &mut Ui) {
        let is_mac = ui.ctx().os() == OperatingSystem::Mac;
        let events = ui.input(|i| i.events.clone());
        let mut consumed = Vec::new();
        for (idx, ev) in events.iter().enumerate() {
            let Event::Key { key, pressed: true, modifiers: m, .. } = ev else { continue };
            let cmd = m.command || m.mac_cmd;
            let handled = match key {
                Key::S if cmd && !m.shift && !m.alt => {
                    let _ = self.save();
                    true
                }
                Key::F if cmd && m.alt => {
                    self.open_find(true);
                    true
                }
                Key::F if cmd && !m.shift => {
                    self.open_find(false);
                    true
                }
                Key::H if cmd && (m.shift || !is_mac) => {
                    self.open_find(true);
                    true
                }
                Key::G if is_mac && m.ctrl && !cmd => {
                    self.goto = Some(String::new());
                    true
                }
                Key::G if !is_mac && m.ctrl => {
                    self.goto = Some(String::new());
                    true
                }
                Key::G if is_mac && cmd => {
                    self.find_next(!m.shift);
                    true
                }
                Key::F3 => {
                    self.find_next(!m.shift);
                    true
                }
                Key::Escape if self.goto.is_some() => {
                    self.goto = None;
                    ui.memory_mut(|mm| mm.request_focus(self.id()));
                    true
                }
                Key::Escape if self.find.open => {
                    self.close_find();
                    ui.memory_mut(|mm| mm.request_focus(self.id()));
                    true
                }
                _ => false,
            };
            if handled {
                consumed.push(idx);
            }
        }
        remove_events(ui, &consumed);
    }

    fn handle_keyboard(&mut self, ui: &mut Ui) {
        let is_mac = ui.ctx().os() == OperatingSystem::Mac;
        let events = ui.input(|i| i.events.clone());
        let now = ui.input(|i| i.time);
        let mut consumed = Vec::new();
        for (idx, ev) in events.iter().enumerate() {
            let handled = match ev {
                Event::Text(s) => {
                    let s: String = s.chars().filter(|c| !c.is_control()).collect();
                    if !s.is_empty() && !self.read_only {
                        self.insert_text(&s);
                    }
                    true
                }
                Event::Copy => {
                    ui.ctx().copy_text(self.copy_text());
                    true
                }
                Event::Cut => {
                    if !self.read_only {
                        let text = self.cut();
                        ui.ctx().copy_text(text);
                    }
                    true
                }
                Event::Paste(s) => {
                    self.paste(s);
                    true
                }
                Event::Ime(ime) => {
                    self.handle_ime(ime);
                    true
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if self.view.preedit.is_some() {
                        false
                    } else {
                        self.handle_key(*key, *modifiers, is_mac)
                    }
                }
                _ => false,
            };
            if handled {
                consumed.push(idx);
                self.view.last_interaction = now;
            }
        }
        remove_events(ui, &consumed);
    }

    fn handle_ime(&mut self, ime: &ImeEvent) {
        match ime {
            ImeEvent::Preedit { text, .. } => {
                if text.is_empty() {
                    self.view.preedit = None;
                } else {
                    if !self.sel.is_empty() {
                        self.paste("");
                    }
                    self.view.preedit = Some(text.clone());
                }
            }
            ImeEvent::Commit(text) => {
                self.view.preedit = None;
                if !text.is_empty() && text != "\n" && text != "\r" {
                    self.insert_text(text);
                }
            }
            _ => {}
        }
    }

    fn page_lines(&self) -> usize {
        ((self.view.viewport.y / self.view.row_h.max(1.0)).floor() as usize).saturating_sub(1).max(1)
    }

    fn handle_key(&mut self, key: Key, m: Modifiers, is_mac: bool) -> bool {
        let cmd = m.command || m.mac_cmd;
        let word = if is_mac { m.alt } else { m.ctrl };
        let mac_line = is_mac && cmd;
        let ext = m.shift;
        match key {
            Key::ArrowLeft => self.move_caret(if mac_line { Motion::Home } else if word { Motion::WordLeft } else { Motion::Left }, ext),
            Key::ArrowRight => self.move_caret(if mac_line { Motion::End } else if word { Motion::WordRight } else { Motion::Right }, ext),
            Key::ArrowUp if m.alt && !cmd && !m.ctrl => self.move_lines(true),
            Key::ArrowDown if m.alt && !cmd && !m.ctrl => self.move_lines(false),
            Key::ArrowUp => self.move_caret(if mac_line { Motion::DocStart } else { Motion::Up(1) }, ext),
            Key::ArrowDown => self.move_caret(if mac_line { Motion::DocEnd } else { Motion::Down(1) }, ext),
            Key::Home => self.move_caret(if cmd || m.ctrl { Motion::DocStart } else { Motion::Home }, ext),
            Key::End => self.move_caret(if cmd || m.ctrl { Motion::DocEnd } else { Motion::End }, ext),
            Key::PageUp => {
                let n = self.page_lines();
                self.move_caret(Motion::Up(n), ext);
                self.view.scroll.y = (self.view.scroll.y - n as f32 * self.view.row_h).max(0.0);
            }
            Key::PageDown => {
                let n = self.page_lines();
                self.move_caret(Motion::Down(n), ext);
                self.view.scroll.y += n as f32 * self.view.row_h;
            }
            Key::Enter if cmd => {
                let l = self.sel.head.line;
                let (at, below) = if m.shift { (Pos::new(l, 0), false) } else { (Pos::new(l, self.buf.line(l).len()), true) };
                self.sel = Selection::caret(at);
                if below {
                    self.newline();
                } else {
                    let base = self.buf.line(l)[..self.buf.first_non_ws(l)].to_owned();
                    self.paste(&format!("{base}\n"));
                    self.sel = Selection::caret(Pos::new(l, base.len()));
                }
            }
            Key::Enter => self.newline(),
            Key::Tab if m.shift => self.indent_lines(false),
            Key::Tab if !cmd && !m.ctrl => self.indent_or_tab(),
            Key::Backspace => self.backspace(word, mac_line),
            Key::Delete => self.delete_forward(word),
            Key::Escape if !self.sel.is_empty() => self.sel = Selection::caret(self.sel.head),
            Key::A if cmd => self.select_all(),
            Key::Z if cmd && m.shift => self.redo(),
            Key::Z if cmd => self.undo(),
            Key::Y if cmd && !is_mac => self.redo(),
            Key::Slash if cmd => self.toggle_comment(),
            Key::K if cmd && m.shift => self.delete_lines(),
            Key::CloseBracket if cmd => self.indent_lines(true),
            Key::OpenBracket if cmd => self.indent_lines(false),
            Key::L if cmd => {
                let (a, b) = self.selected_lines();
                self.select_line_range(a, b);
            }
            _ => return false,
        }
        true
    }
}

fn remove_events(ui: &mut Ui, consumed: &[usize]) {
    if consumed.is_empty() {
        return;
    }
    ui.input_mut(|i| {
        let mut k = 0usize;
        i.events.retain(|_| {
            let keep = !consumed.contains(&k);
            k += 1;
            keep
        });
    });
}
