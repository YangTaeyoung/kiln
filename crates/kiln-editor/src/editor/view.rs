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

use super::{ColumnSel, Editor, Motion, Reveal, char_cols};
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
    /// 사각 선택 끌기. 시작 줄, 시작 표시 열, 시작점을 벗어났는지.
    Column { line: usize, col: usize, moved: bool },
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
    /// 접힌 줄 뒤 "⋯" 표시 영역과 그 줄.
    pub fold_pills: Vec<(Rect, usize)>,
    /// Cmd+K 를 누른 시각(두 번째 키를 기다리는 중).
    pub chord: Option<f64>,
    /// 텍스트 영역의 화면 사각형.
    pub text_rect: Option<Rect>,
    /// 줄 번호 여백의 화면 사각형.
    pub gutter_rect: Option<Rect>,
    /// 다음 글자 입력을 버린다.
    pub swallow_text: bool,
}

/// 화면에 그릴 시각 줄 하나.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowInfo {
    pub row: usize,
    pub line: usize,
    pub sub: usize,
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

fn text_format(font: &FontId, color: Color32, italics: bool, underline: bool) -> TextFormat {
    TextFormat {
        font_id: font.clone(),
        color,
        italics,
        underline: if underline { Stroke::new(1.0, color) } else { Stroke::NONE },
        ..Default::default()
    }
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

fn char_to_byte(s: &str, ch: usize) -> usize {
    s.char_indices().nth(ch).map_or(s.len(), |(i, _)| i)
}

impl Editor {
    /// 마지막 프레임의 (시각 줄 높이, 줄 번호 여백, 텍스트 영역). UI 테스트에서 좌표를 계산할 때 쓴다.
    #[doc(hidden)]
    pub fn layout_info(&self) -> (f32, Rect, Rect) {
        (self.view.row_h, self.view.gutter_rect.unwrap_or(Rect::NOTHING), self.view.text_rect.unwrap_or(Rect::NOTHING))
    }

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
            self.handle_keyboard(ui);
            let escape = self.wants_escape();
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    self.id(),
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape },
                )
            });
        }
        self.update_find_matches();
        self.lsp_frame(ui);
        self.unfold_around_cursors();

        let digits = (self.buf.line_count().max(1) as f32).log10().floor() as usize + 1;
        let gutter_w = (GUTTER_PAD_LEFT + digits.max(3) as f32 * self.view.char_w + GUTTER_PAD_RIGHT).round();
        let gutter = Rect::from_min_max(body.min, pos2(body.left() + gutter_w, body.bottom()));
        let text_rect = Rect::from_min_max(pos2(gutter.right(), body.top()), body.max);

        self.view.text_rect = Some(text_rect);
        self.view.gutter_rect = Some(gutter);
        let offset_req = self.reveal.take().map(|r| self.reveal_offset(ui, r, text_rect.size()));
        let mut child = ui.new_child(
            UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)).id_salt(self.id().with("text")),
        );
        child.set_clip_rect(text_rect.intersect(ui.clip_rect()));
        let mut sa = ScrollArea::new([!self.word_wrap, true])
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
        self.lsp_popups_ui(ui, out.inner_rect);
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

    /// 줄 전체를 덮는 강조 조각(바이트 범위와 서식).
    fn line_pieces(&self, i: usize, font: &FontId) -> Vec<(usize, usize, TextFormat)> {
        let t = Theme::current();
        let line = self.buf.line(i);
        let fmt = |color: Color32, italics: bool, underline: bool| text_format(font, color, italics, underline);
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
        pieces
    }

    /// 줄 `i` 의 `s..e` 바이트 구간(시각 줄 하나)을 배치할 작업. `preedit` 은 (바이트 위치, 조합 중 글자).
    fn seg_job(&self, i: usize, s: usize, e: usize, font: &FontId, preedit: Option<(usize, &str)>) -> LayoutJob {
        let t = Theme::current();
        let line = self.buf.line(i);
        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        job.break_on_newline = false;
        let push = |job: &mut LayoutJob, text: &str, f: TextFormat| {
            if !text.is_empty() {
                job.append(text, 0.0, f);
            }
        };
        let mut pre_done = false;
        for (a, b, f) in self.line_pieces(i, font) {
            let (a, b) = (a.max(s), b.min(e));
            if a >= b {
                continue;
            }
            match preedit {
                Some((caret, pre)) if !pre_done && a <= caret && caret <= b && (caret < b || b == e) => {
                    push(&mut job, &line[a..caret], f.clone());
                    push(&mut job, pre, text_format(font, t.text, false, true));
                    push(&mut job, &line[caret..b], f);
                    pre_done = true;
                }
                _ => push(&mut job, &line[a..b], f),
            }
        }
        if !pre_done && let Some((_, pre)) = preedit {
            push(&mut job, pre, text_format(font, t.text, false, true));
        }
        job
    }

    fn seg_galley(&self, ui: &Ui, i: usize, s: usize, e: usize, font: &FontId, preedit: Option<(usize, &str)>) -> Arc<Galley> {
        ui.painter().layout_job(self.seg_job(i, s, e, font, preedit))
    }

    /// 시각 줄(줄 `line` 의 `s` 부터) 갤리 안에서 바이트 `col` 의 x.
    fn seg_x(&self, galley: &Galley, line: usize, s: usize, col: usize) -> f32 {
        let text = self.buf.line(line);
        let col = col.clamp(s, text.len());
        galley.pos_from_cursor(CCursor::new(text[s..col].chars().count())).min.x
    }

    /// 위치가 속한 시각 줄의 갤리, 그 시각 줄 시작 바이트, 줄 바꿈 들여쓰기(px).
    /// `row_end` 면 줄 바꿈 지점을 앞 시각 줄 끝으로 본다.
    fn galley_for_pos(&self, ui: &Ui, p: Pos, row_end: bool, font: &FontId) -> (Arc<Galley>, usize, f32) {
        let (s, e, _) = self.seg_of_aff(p, row_end);
        let indent = self.row_indent_px(p.line, self.sub_of_aff(p, row_end));
        (self.seg_galley(ui, p.line, s, e, font, None), s, indent)
    }

    /// 줄 바꿈으로 이어진 시각 줄의 들여쓰기(px).
    fn row_indent_px(&self, line: usize, sub: usize) -> f32 {
        self.display.row_indent(self.buf.line(line), sub) as f32 * self.view.char_w
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

    /// 주 커서가 보이도록 하는 스크롤 오프셋.
    fn reveal_offset(&mut self, ui: &Ui, how: Reveal, view: Vec2) -> Vec2 {
        let row_h = self.view.row_h;
        let font = self.view.font.clone().unwrap_or_else(|| TextStyle::Monospace.resolve(ui.style()));
        let head = self.sel.head;
        let aff = self.head_at_row_end();
        let (g, s, ind) = self.galley_for_pos(ui, head, aff, &font);
        let x = PAD_LEFT + ind + self.seg_x(&g, head.line, s, head.col);
        let y = self.pos_row_aff(head, aff) as f32 * row_h;
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
        if self.word_wrap {
            off.x = 0.0;
            return off;
        }
        let xm = self.view.char_w * 4.0;
        if x - xm < off.x {
            off.x = (x - xm).max(0.0);
        } else if x + xm > off.x + vw {
            off.x = x + xm - vw;
        }
        off
    }

    /// `first..last` 시각 줄 목록. 숨은 줄은 건너뛴다.
    fn visible_rows(&mut self, first: usize, last: usize) -> Vec<RowInfo> {
        let mut out = Vec::with_capacity(last.saturating_sub(first));
        if first >= last {
            return out;
        }
        let n = self.buf.line_count();
        let (mut line, mut sub) = self.display.row_to_line(first);
        for row in first..last {
            if line >= n {
                break;
            }
            let len = self.buf.line(line).len();
            let rows = self.display.rows(line).max(1);
            let (s, e) = self.display.segment(line, sub, len);
            out.push(RowInfo { row, line, sub, start: s, end: e, last: sub + 1 >= rows });
            if sub + 1 < rows {
                sub += 1;
            } else {
                sub = 0;
                line += 1;
                while line < n && self.display.is_hidden(line) {
                    line += 1;
                }
            }
        }
        out
    }

    // ---- 본문 ----

    fn text_ui(&mut self, ui: &mut Ui, vp: Rect, font: &FontId, focused: bool) {
        let t = Theme::current();
        let row_h = self.view.row_h;
        let n = self.buf.line_count();
        let origin = ui.max_rect().min;
        let char_w = self.view.char_w.max(1.0);
        if self.word_wrap {
            let cols = ((vp.width() - PAD_LEFT - char_w * 2.0) / char_w).floor().max(8.0) as usize;
            if cols != self.display.wrap_cols() {
                self.set_wrap_cols(cols);
            }
        }
        let total = self.display.total_rows();
        let content_w = if self.word_wrap {
            vp.width()
        } else {
            PAD_LEFT + self.max_cols() as f32 * char_w + char_w * 12.0
        };
        let content_h = total as f32 * row_h + (vp.height() - row_h * 3.0).max(0.0);
        ui.set_min_size(vec2(content_w.max(vp.width()), content_h.max(vp.height())));

        let first = ((vp.min.y / row_h).floor().max(0.0) as usize).min(total);
        let last = (((vp.max.y / row_h).ceil().max(0.0)) as usize).min(total);
        let rows = self.visible_rows(first, last);
        let last_line = rows.last().map_or(0, |r| r.line + 1);
        self.view.drawn = (rows.first().map_or(0, |r| r.line), last_line);

        let deadline = Instant::now() + HIGHLIGHT_BUDGET;
        let done = self.hl.ensure(self.buf.lines(), last_line, Some(deadline));
        if !done {
            ui.ctx().request_repaint();
        } else if self.hl.valid_lines() < n {
            // 보이는 줄이 끝났으면 남은 줄을 유휴 프레임마다 조금씩 미리 계산한다.
            let idle_deadline = Instant::now() + IDLE_HIGHLIGHT_BUDGET;
            let target = if self.hl.valid_lines() < (last_line + LOOKAHEAD_LINES).min(n) { last_line + LOOKAHEAD_LINES } else { n };
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
        let base_x = origin.x + PAD_LEFT;
        let sels = self.cursors();
        let row_ends: Vec<Pos> = std::iter::once(self.primary_cursor())
            .chain(self.extra.iter().copied())
            .filter_map(|c| c.row_end_at.filter(|&p| p == c.sel.head))
            .collect();
        let primary = self.sel;
        let (psa, psb) = primary.range();
        let has_focus = ui.memory(|m| m.has_focus(self.id()));
        let multi = sels.len() > 1;

        // 들여쓰기 안내선 계산용: 빈 줄은 다음 비어 있지 않은 줄의 들여쓰기를 따른다.
        let tab = self.indent.width();
        let unit_px = self.indent.width() as f32 * char_w;
        let indent_cols = |ed: &Editor, i: usize| -> Option<usize> {
            let l = ed.buf.line(i);
            if l.trim().is_empty() {
                return None;
            }
            Some(l.chars().take_while(|c| c.is_whitespace()).map(|c| char_cols(c, tab)).sum())
        };
        let mut next_indent: Option<(usize, usize)> = None;

        let now = ui.input(|inp| inp.time);
        let since = now - self.view.last_interaction;
        let blink = ui.visuals().text_cursor.blink;
        let caret_on = !blink || since < 0.6 || ((since - 0.6) % 1.0) < 0.5;
        self.view.fold_pills.clear();

        for r in &rows {
            let i = r.line;
            let y = origin.y + r.row as f32 * row_h;
            let text_x = base_x + self.row_indent_px(i, r.sub);
            let row_rect = Rect::from_min_max(pos2(screen_vp.left(), y), pos2(screen_vp.right(), y + row_h));
            if i == primary.head.line && psa == psb && !multi {
                painter.rect_filled(row_rect, 0.0, t.bg_panel);
            }
            let preedit = match self.view.preedit.as_deref() {
                Some(pre) if i == primary.head.line && self.seg_contains(r, primary.head.col) => Some((primary.head.col, pre)),
                _ => None,
            };
            let g = self.seg_galley(ui, i, r.start, r.end, font, preedit);
            let line_len = self.buf.line(i).len();

            // 들여쓰기 안내선(줄의 첫 시각 줄에만)
            if r.sub == 0 {
                let ind = match indent_cols(self, i) {
                    Some(c) => c,
                    None => {
                        let cached = next_indent.filter(|&(from, _)| from > i).map(|(_, v)| v);
                        cached.unwrap_or_else(|| {
                            let mut j = i + 1;
                            let mut v = (n, 0);
                            while j < n && j < i + 200 {
                                if let Some(c) = indent_cols(self, j) {
                                    v = (j, c);
                                    break;
                                }
                                j += 1;
                            }
                            next_indent = Some(v);
                            v.1
                        })
                    }
                };
                if unit_px > 0.0 && tab > 0 {
                    let levels = ind / tab;
                    for lv in 1..levels.clamp(1, 40) {
                        let x = (base_x + lv as f32 * unit_px).round() + 0.5;
                        painter.line_segment([pos2(x, y), pos2(x, y + row_h)], Stroke::new(1.0, t.border.gamma_multiply(0.8)));
                    }
                }
            }

            // 찾기 일치
            let matches = &self.find.matches;
            let mut mi = matches.partition_point(|m| m.0.line < i);
            while mi < matches.len() && matches[mi].0.line == i {
                let (a, b) = matches[mi];
                let b_col = if b.line == i { b.col } else { line_len };
                let (c0, c1) = (a.col.max(r.start), b_col.min(r.end));
                if c0 < c1 || (c0 == c1 && a.col == c0 && r.last) {
                    let x0 = text_x + self.seg_x(&g, i, r.start, c0);
                    let x1 = text_x + self.seg_x(&g, i, r.start, c1);
                    let rr = Rect::from_min_max(pos2(x0, y + 2.0), pos2(x1.max(x0 + 2.0), y + row_h - 2.0));
                    if self.find.current == Some(mi) {
                        painter.rect_filled(rr, 2.0, t.orange.gamma_multiply(0.45));
                        painter.rect_stroke(rr, 2.0, Stroke::new(1.0, t.orange.gamma_multiply(0.9)), StrokeKind::Inside);
                    } else {
                        painter.rect_filled(rr, 2.0, t.yellow.gamma_multiply(0.22));
                    }
                }
                mi += 1;
            }

            // 선택 영역
            let fill = if has_focus { t.bg_selected } else { t.bg_selected.gamma_multiply(0.6) };
            let mut si = sels.partition_point(|s| s.range().1.line < i);
            while si < sels.len() && sels[si].range().0.line <= i {
                let (sa, sb) = sels[si].range();
                si += 1;
                if sa == sb {
                    continue;
                }
                let c0 = if i == sa.line { sa.col } else { 0 };
                let c1 = if i == sb.line { sb.col } else { line_len };
                let (c0, c1) = (c0.max(r.start), c1.min(r.end));
                let continues = i != sb.line && r.last;
                if c0 > c1 || (c0 == c1 && !continues) {
                    continue;
                }
                let x0 = text_x + self.seg_x(&g, i, r.start, c0);
                let mut x1 = text_x + self.seg_x(&g, i, r.start, c1);
                if continues {
                    x1 += char_w * 0.6;
                }
                painter.rect_filled(Rect::from_min_max(pos2(x0, y + 1.0), pos2(x1, y + row_h - 1.0)), 3.0, fill);
            }

            self.paint_row_diagnostics(&painter, r, &g, text_x, y);

            let gy = y + ((row_h - g.size().y) / 2.0).round();
            painter.galley(pos2(text_x, gy), g.clone(), t.text);

            // 접힌 줄 표시
            if r.last && self.folds.is_folded_at(i) {
                let x = text_x + g.size().x + char_w * 0.8;
                let pill = Rect::from_min_size(pos2(x, y + 3.0), vec2(char_w * 2.6, row_h - 6.0));
                painter.rect_filled(pill, 4.0, t.bg_hover);
                painter.rect_stroke(pill, 4.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
                painter.text(pill.center(), Align2::CENTER_CENTER, "⋯", font.clone(), t.text_dim);
                self.view.fold_pills.push((pill, i));
            }

            // 커서
            if has_focus && !self.read_only && caret_on {
                let mut ci = sels.partition_point(|s| s.head.line < i && s.range().1.line < i);
                while ci < sels.len() && sels[ci].range().0.line <= i {
                    let s = sels[ci];
                    ci += 1;
                    if s.head.line != i || !caret_in_row(r, s.head.col, row_ends.contains(&s.head)) {
                        continue;
                    }
                    let is_primary = s == primary;
                    let pre_chars = if is_primary { self.view.preedit.as_deref().map_or(0, |p| p.chars().count()) } else { 0 };
                    let cc = self.buf.line(i)[r.start..s.head.col.max(r.start)].chars().count() + pre_chars;
                    let x = text_x + g.pos_from_cursor(CCursor::new(cc)).min.x;
                    let rr = Rect::from_min_size(pos2(x.round() - 1.0, y + 3.0), vec2(2.0, row_h - 6.0));
                    painter.rect_filled(rr, 1.0, if is_primary || !multi { t.accent } else { t.accent.gamma_multiply(0.85) });
                }
            }
        }
        if has_focus && !self.read_only && blink && ui.input(|inp| inp.focused) {
            let phase = if since < 0.6 { 0.6 - since } else { 0.5 - ((since - 0.6) % 0.5) };
            ui.ctx().request_repaint_after(Duration::from_secs_f64(phase.max(0.02)));
        }
        if !focused && resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        self.hover_tracking(ui, &resp, origin, font);
    }

    /// 바이트 위치가 이 시각 줄에 속하는지. 줄 바꿈 지점은 다음 시각 줄에 속한다.
    fn seg_contains(&self, r: &RowInfo, col: usize) -> bool {
        r.start <= col && (col < r.end || (col == r.end && r.last))
    }

    /// 화면 좌표의 버퍼 위치.
    pub(crate) fn pos_at(&mut self, ui: &Ui, p: Pos2, origin: Pos2, font: &FontId) -> Pos {
        self.hit(ui, p, origin, font).0
    }

    /// 화면 좌표의 버퍼 위치와 줄 처음부터 센 표시 열. 줄 끝 너머는 빈 열까지 센다.
    fn hit(&mut self, ui: &Ui, p: Pos2, origin: Pos2, font: &FontId) -> (Pos, usize) {
        let row_h = self.view.row_h;
        let fy = (p.y - origin.y) / row_h;
        if fy < 0.0 {
            return (Pos::new(0, 0), 0);
        }
        let total = self.display.total_rows();
        let row = fy.floor() as usize;
        let (line, sub) = self.display.row_to_line(row.min(total.saturating_sub(1)));
        let len = self.buf.line(line).len();
        let (s, e) = self.display.segment(line, sub, len);
        let last = sub + 1 >= self.display.rows(line).max(1);
        let g = self.seg_galley(ui, line, s, e, font, None);
        let x = p.x - origin.x - PAD_LEFT - self.row_indent_px(line, sub);
        if row >= total {
            let end = Pos::new(line, len);
            return (end, self.display_col(end));
        }
        let cc = g.cursor_from_pos(vec2(x, g.size().y / 2.0));
        let text = self.buf.line(line);
        let mut col = s + char_to_byte(&text[s..e], cc.index.0);
        if col == e && !last {
            col = text[..e].char_indices().next_back().map_or(s, |(i, _)| i.max(s));
        }
        let pos = Pos::new(line, col);
        let mut vcol = self.display_col(pos);
        if col == e && last {
            let over = ((x - g.size().x) / self.view.char_w.max(1.0)).round();
            vcol += over.max(0.0) as usize;
        }
        (pos, vcol)
    }

    fn handle_mouse(&mut self, ui: &Ui, resp: &Response, origin: Pos2, font: &FontId) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let over_overlay = pointer.is_some_and(|p| self.view.overlays.iter().any(|r| r.contains(p)));
        if resp.hovered() && !over_overlay {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        let (pressed, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.modifiers));
        let cmd = mods.command || mods.mac_cmd;
        if resp.hovered() && !over_overlay && pressed && let Some(p) = pointer {
            ui.memory_mut(|m| m.request_focus(self.id()));
            self.view.last_interaction = ui.input(|i| i.time);
            if let Some(&(_, line)) = self.view.fold_pills.iter().find(|(r, _)| r.contains(p)) {
                self.unfold_at(line);
                self.view.drag = DragMode::None;
                return;
            }
            let (pos, vcol) = self.hit(ui, p, origin, font);
            self.preferred_col = None;
            self.row_end_at = None;
            if mods.alt && !cmd {
                if mods.shift {
                    self.set_selection(Selection::caret(pos));
                } else {
                    self.add_cursor(pos);
                }
                self.view.drag = DragMode::Column { line: pos.line, col: vcol, moved: mods.shift };
                return;
            }
            if cmd && !mods.shift && self.lsp_goto_definition_at(pos) {
                self.view.drag = DragMode::None;
                return;
            }
            self.extra.clear();
            if mods.shift {
                self.sel.head = pos;
            } else {
                self.sel = Selection::caret(pos);
            }
            self.view.drag = DragMode::Char;
        }
        if resp.double_clicked() && !over_overlay && !mods.alt && let Some(p) = pointer {
            let pos = self.pos_at(ui, p, origin, font);
            let (a, b) = self.buf.word_at(pos);
            self.sel = Selection::new(a, b);
            self.view.drag = DragMode::Word(a, b);
        }
        if resp.triple_clicked() && !over_overlay && !mods.alt && let Some(p) = pointer {
            let pos = self.pos_at(ui, p, origin, font);
            self.select_line_range(pos.line, pos.line);
            self.view.drag = DragMode::Line(pos.line);
        }
        if resp.dragged() && self.view.drag != DragMode::None && let Some(p) = pointer {
            let (pos, vcol) = self.hit(ui, p, origin, font);
            match self.view.drag {
                DragMode::Column { line, col, moved } => {
                    if moved || (pos.line, vcol) != (line, col) {
                        self.view.drag = DragMode::Column { line, col, moved: true };
                        self.set_column_selection(ColumnSel {
                            anchor_line: line,
                            anchor_col: col,
                            head_line: pos.line,
                            head_col: vcol,
                        });
                    }
                }
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
        self.extra.clear();
        let n = self.buf.line_count();
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let end = if hi + 1 < n { Pos::new(hi + 1, 0) } else { Pos::new(hi, self.buf.line(hi).len()) };
        self.sel = if b < a { Selection::new(end, Pos::new(lo, 0)) } else { Selection::new(Pos::new(lo, 0), end) };
    }

    fn context_menu(&mut self, ui: &Ui, resp: &Response) {
        let lsp_on = self.lsp.is_some();
        resp.context_menu(|ui| {
            ui.set_min_width(200.0);
            let ro = self.read_only;
            if lsp_on {
                if ui.add(egui::Button::new("정의로 이동").shortcut_text("F12")).clicked() {
                    self.lsp_goto_definition();
                    ui.close();
                }
                if ui.add(egui::Button::new("참조 찾기").shortcut_text("Shift+F12")).clicked() {
                    self.lsp_find_references();
                    ui.close();
                }
                if ui.add_enabled(!ro, egui::Button::new("기호 이름 바꾸기").shortcut_text("F2")).clicked() {
                    self.lsp_start_rename();
                    ui.close();
                }
                if ui.add_enabled(!ro, egui::Button::new("문서 서식")).clicked() {
                    self.lsp_format();
                    ui.close();
                }
                ui.separator();
            }
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
            let wrap_label = if self.word_wrap { "자동 줄 바꿈 끄기" } else { "자동 줄 바꿈" };
            if ui.add(egui::Button::new(wrap_label).shortcut_text("Alt+Z")).clicked() {
                self.toggle_word_wrap();
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
        let total = self.display.total_rows();
        let first = ((self.view.scroll.y / row_h).floor().max(0.0) as usize).min(total);
        let last = (((self.view.scroll.y + gutter.height()) / row_h).ceil() as usize).min(total);
        let rows = self.visible_rows(first, last);
        let (sa, sb) = self.sel.range();
        let head = self.sel.head.line;
        let multi = !self.extra.is_empty();
        let hovered = ui.rect_contains_pointer(gutter);
        let fold_starts: Vec<usize> = if hovered {
            let (a, b) = (rows.first().map_or(0, |r| r.line), rows.last().map_or(0, |r| r.line));
            self.fold_ranges().iter().filter(|r| r.start >= a && r.start <= b).map(|r| r.start).collect()
        } else {
            Vec::new()
        };
        let chevron_x = gutter.right() - GUTTER_PAD_RIGHT / 2.0 - 1.0;
        let diag_lines = self.gutter_diagnostics(rows.first().map_or(0, |r| r.line), rows.last().map_or(0, |r| r.line));
        for r in &rows {
            let i = r.line;
            let y = top + r.row as f32 * row_h;
            if i == head && sa == sb && !multi {
                painter.rect_filled(Rect::from_min_max(pos2(gutter.left(), y), pos2(gutter.right(), y + row_h)), 0.0, t.bg_panel);
            }
            if r.sub != 0 {
                continue;
            }
            let in_sel = sa != sb && sa.line <= i && i <= sb.line;
            let color = if i == head { t.text } else if in_sel { t.text_dim } else { t.text_faint };
            painter.text(
                pos2(gutter.right() - GUTTER_PAD_RIGHT, y + row_h / 2.0),
                Align2::RIGHT_CENTER,
                (i + 1).to_string(),
                font.clone(),
                color,
            );
            if let Some(&(_, sev_color)) = diag_lines.iter().find(|(l, _)| *l == i) {
                let c = pos2(gutter.left() + 6.0, y + row_h / 2.0);
                painter.circle_filled(c, 3.0, sev_color);
            }
            let folded = self.folds.is_folded_at(i);
            if folded || fold_starts.contains(&i) {
                let rect = Rect::from_center_size(pos2(chevron_x, y + row_h / 2.0), vec2(11.0, 11.0));
                let icon = if folded { Icon::ChevronRight } else { Icon::ChevronDown };
                ui_kit::paint_icon(&painter, rect, icon, if folded { t.text_dim } else { t.text_faint });
            }
        }
        let resp = ui.interact(gutter, self.id().with("gutter"), Sense::click_and_drag());
        if let Some(p) = resp.interact_pointer_pos() {
            let row = ((p.y - top) / row_h).floor().max(0.0) as usize;
            let (line, _) = self.display.row_to_line(row.min(total.saturating_sub(1)));
            let line = line.min(n - 1);
            let on_chevron = p.x >= chevron_x - 8.0;
            let press = resp.drag_started() || resp.clicked() || ui.input(|i| i.pointer.primary_pressed()) && resp.hovered();
            if press && on_chevron && (self.folds.is_folded_at(line) || self.fold_ranges().iter().any(|r| r.start == line)) {
                if ui.input(|i| i.pointer.primary_pressed()) {
                    self.toggle_fold_at(line);
                }
                self.view.drag = DragMode::None;
            } else if press {
                ui.memory_mut(|m| m.request_focus(self.id()));
                self.view.drag = DragMode::Line(line);
                self.select_line_range(line, line);
            } else if resp.dragged() && let DragMode::Line(a) = self.view.drag {
                self.select_line_range(a, line);
            }
        }
    }

    fn ime_output(&mut self, ui: &Ui, inner: Rect) {
        let row_h = self.view.row_h;
        let head = self.sel.head;
        let aff = self.head_at_row_end();
        let font = match &self.view.font {
            Some(f) => f.clone(),
            None => return,
        };
        let (g, s, ind) = self.galley_for_pos(ui, head, aff, &font);
        let x = inner.left() + PAD_LEFT + ind + self.seg_x(&g, head.line, s, head.col) - self.view.scroll.x;
        let y = inner.top() + self.pos_row_aff(head, aff) as f32 * row_h - self.view.scroll.y;
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

    /// 버퍼 위치의 화면 좌표(시각 줄 왼쪽 위). 텍스트 영역 `inner` 기준.
    pub(crate) fn screen_pos_of(&mut self, ui: &Ui, p: Pos, inner: Rect) -> Pos2 {
        let font = self.view.font.clone().unwrap_or_else(|| TextStyle::Monospace.resolve(ui.style()));
        let (g, s, ind) = self.galley_for_pos(ui, p, false, &font);
        let x = inner.left() + PAD_LEFT + ind + self.seg_x(&g, p.line, s, p.col) - self.view.scroll.x;
        let y = inner.top() + self.display.pos_row(p) as f32 * self.view.row_h - self.view.scroll.y;
        pos2(x, y)
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
                Event::Text(_) if self.view.swallow_text => {
                    self.view.swallow_text = false;
                    true
                }
                Event::Text(s) => {
                    let s: String = s.chars().filter(|c| !c.is_control()).collect();
                    if !s.is_empty() && !self.read_only {
                        self.insert_text(&s);
                        self.lsp_after_typed(&s);
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
                Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
                    if self.view.preedit.is_some() {
                        false
                    } else {
                        // Option 을 누른 키 입력은 물리 키로 해석한다.
                        let key = if modifiers.alt { physical_key.unwrap_or(*key) } else { *key };
                        self.view.swallow_text = false;
                        let handled = self.handle_key(key, *modifiers, is_mac);
                        // Option 조합 단축키가 macOS 에서 함께 보내는 글자는 넣지 않는다. 화살표 키는 글자를 보내지 않는다.
                        let arrow = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight);
                        self.view.swallow_text = handled && modifiers.alt && !modifiers.command && !modifiers.ctrl && !arrow;
                        handled
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

    /// Escape 로 닫거나 풀 것이 있는지. 없으면 Escape 는 포커스를 놓게 둔다.
    fn wants_escape(&self) -> bool {
        !self.extra.is_empty() || !self.sel.is_empty() || self.view.chord.is_some() || self.lsp_has_popup()
    }

    fn page_lines(&self) -> usize {
        ((self.view.viewport.y / self.view.row_h.max(1.0)).floor() as usize).saturating_sub(1).max(1)
    }

    fn handle_key(&mut self, key: Key, m: Modifiers, is_mac: bool) -> bool {
        let cmd = m.command || m.mac_cmd;
        let word = if is_mac { m.alt } else { m.ctrl };
        let mac_line = is_mac && cmd;
        let ext = m.shift;
        if self.lsp_popup_key(key, m) {
            return true;
        }
        if self.view.chord.take().is_some() {
            match key {
                Key::Num0 => self.fold_all(),
                Key::J => self.unfold_all(),
                Key::I => self.lsp_hover_at_cursor(),
                _ => return self.handle_key(key, m, is_mac),
            }
            return true;
        }
        match key {
            Key::K if cmd && !m.shift && !m.alt => self.view.chord = Some(0.0),
            Key::Z if m.alt && !cmd && !m.ctrl => self.toggle_word_wrap(),
            Key::D if cmd && !m.shift && !m.alt => self.add_next_occurrence(),
            Key::ArrowUp if m.alt && cmd => self.add_cursor_vertical(true),
            Key::ArrowDown if m.alt && cmd => self.add_cursor_vertical(false),
            Key::OpenBracket if cmd && m.alt => {
                self.fold_at(self.sel.head.line);
            }
            Key::CloseBracket if cmd && m.alt => {
                self.unfold_at(self.sel.head.line);
            }
            Key::Escape if !self.extra.is_empty() => self.collapse_cursors(),
            Key::F12 if m.shift => self.lsp_find_references(),
            Key::F12 => self.lsp_goto_definition(),
            Key::F2 => self.lsp_start_rename(),
            Key::F if m.shift && m.alt && !cmd => self.lsp_format(),
            Key::Space if m.ctrl => self.lsp_trigger_completion(None),
            Key::ArrowUp if m.shift && m.alt && !cmd && !m.ctrl => self.extend_column(-1, 0),
            Key::ArrowDown if m.shift && m.alt && !cmd && !m.ctrl => self.extend_column(1, 0),
            Key::ArrowLeft if m.shift && m.alt && !cmd && !m.ctrl && self.active_column().is_some() => {
                self.extend_column(0, -1)
            }
            Key::ArrowRight if m.shift && m.alt && !cmd && !m.ctrl && self.active_column().is_some() => {
                self.extend_column(0, 1)
            }
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
            Key::Enter if cmd => self.insert_line(!m.shift),
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

/// 커서가 이 시각 줄에 그려지는지. `row_end` 면 줄 바꿈 지점의 커서를 앞 시각 줄 끝에 둔다.
fn caret_in_row(r: &RowInfo, col: usize, row_end: bool) -> bool {
    if row_end && !r.last && col == r.end {
        return true;
    }
    if row_end && r.sub > 0 && col == r.start {
        return false;
    }
    r.start <= col && (col < r.end || (col == r.end && r.last))
}
