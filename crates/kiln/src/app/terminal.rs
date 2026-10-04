//! 터미널 화면 위젯: 렌더링, 키보드/IME/마우스 입력, 선택, 링크.

use super::conn::{Conn, Screen};
use super::keys;
use egui::{Color32, FontId, Modifiers, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};
use kiln_proto::{Cell, Color, CursorShape, Line, ScrollTo, SessionId, flags, mode};
use std::collections::HashMap;
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;

pub struct Palette {
    pub fg: Color32,
    pub bg: Color32,
    pub ansi: [Color32; 16],
    pub cursor: Color32,
    pub selection: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        Palette::from_theme(&kiln_common::Theme::current())
    }
}

impl Palette {
    pub fn from_theme(t: &kiln_common::Theme) -> Self {
        Palette { fg: t.text, bg: t.bg, ansi: t.ansi, cursor: t.accent, selection: t.accent_soft(if t.dark { 80 } else { 60 }) }
    }

    pub fn resolve(&self, c: Color, _is_fg: bool) -> Color32 {
        match c {
            Color::DefaultFg => self.fg,
            Color::DefaultBg => self.bg,
            Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
            Color::Idx(i) => self.indexed(i),
        }
    }

    pub fn indexed(&self, i: u8) -> Color32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Color32::from_rgb(lv(i / 36), lv((i / 6) % 6), lv(i % 6))
            }
            _ => {
                let g = 8 + (i - 232) * 10;
                Color32::from_rgb(g, g, g)
            }
        }
    }

    fn cell_colors(&self, c: &Cell) -> (Color32, Color32) {
        let mut fg = match c.fg {
            Color::Idx(i) if i < 8 && c.flags & flags::BOLD != 0 => self.ansi[i as usize + 8],
            other => self.resolve(other, true),
        };
        let mut bg = self.resolve(c.bg, false);
        if c.flags & flags::INVERSE != 0 { std::mem::swap(&mut fg, &mut bg); }
        if c.flags & flags::DIM != 0 { fg = lerp(bg, fg, 0.6); }
        (fg, bg)
    }
}

#[derive(Clone, Copy)]
pub struct TermSettings {
    pub font_size: f32,
    pub option_as_meta: bool,
    pub line_height: f32,
    pub copy_on_select: bool,
    pub cursor_blink: bool,
    pub close_shortcut: Option<egui::KeyboardShortcut>,
}

impl Default for TermSettings {
    fn default() -> Self {
        TermSettings { font_size: 13.5, option_as_meta: true, line_height: 1.2, copy_on_select: false, cursor_blink: false, close_shortcut: None }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Point {
    /// 절대 줄 번호(화면 행 - display_offset).
    line: i64,
    col: u16,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SelMode {
    Cell,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: Point,
    head: Point,
    mode: SelMode,
}

impl Selection {
    fn ordered(&self) -> (Point, Point) {
        let (a, b) = if (self.anchor.line, self.anchor.col) <= (self.head.line, self.head.col) { (self.anchor, self.head) } else { (self.head, self.anchor) };
        (a, b)
    }
}

struct RowCache {
    version: u64,
    key: u64,
    shapes: Vec<Shape>,
}

pub enum LinkTarget {
    Url(String),
    File { path: std::path::PathBuf, line: Option<usize>, col: Option<usize> },
}

#[derive(Default)]
pub struct TermOutput {
    pub focused: bool,
    pub clicked: bool,
    pub open: Option<LinkTarget>,
    /// 종료된 세션에서 Enter 를 눌러 재시작을 요청했다.
    pub restart: bool,
    /// Explicitly reviewed command to launch in a fresh terminal; never silently injected.
    pub command_to_run: Option<(String, Option<String>)>,
}

struct SearchBar {
    query: String,
    focus: bool,
    last_found: Option<bool>,
}

pub struct TermView {
    pub session: SessionId,
    rows: Vec<RowCache>,
    fit_cache: HashMap<char, bool>,
    metrics: Option<((f32, f32, f32), Vec2)>,
    selection: Option<Selection>,
    selecting: bool,
    preedit: String,
    scroll_accum: f32,
    last_mouse_cell: Option<(u16, u16)>,
    mouse_capture: MouseCapture,
    search: Option<SearchBar>,
    inspector: bool,
    integration_help: bool,
    inspected_command: Option<u64>,
    inspector_record_expired: bool,
    command_draft: String,
    accessible_input: String,
    pub palette: Arc<Palette>,
    pending_size: Option<((u16, u16), std::time::Instant)>,
    theme_name: &'static str,
    /// false 면 바탕을 칠하지 않는다(카드가 둥근 바탕을 그린다).
    pub fill_background: bool,
}

// Text events follow their key event, but InputState.modifiers contains the
// final state of the whole frame. Do not re-send text already encoded as Meta
// or Ctrl when the modifier was released later in that same frame.
fn terminal_input_events(events: Vec<egui::Event>, frame_mods: egui::Modifiers, option_as_meta: bool, is_mac: bool) -> Vec<egui::Event> {
    let mut text_mods = frame_mods;
    events.into_iter().filter(|event| match event {
        egui::Event::Key { pressed:true, modifiers, .. } => {text_mods=*modifiers;true}
        egui::Event::Text(_) => {
            let include=!(is_mac && option_as_meta && text_mods.alt || !is_mac && text_mods.ctrl);
            text_mods=frame_mods;
            include
        }
        _=>true,
    }).collect()
}

/// Each split owns only presses delivered to its terminal, including an outside release.
#[derive(Default)]
struct MouseCapture(u8);
impl MouseCapture {
    fn button(&mut self, button: u8, pressed: bool, shift: bool, owns_pointer: bool) -> bool {
        let mask = 1 << button;
        if pressed {
            if shift || !owns_pointer { return false; }
            self.0 |= mask;
            true
        } else {
            let captured = self.0 & mask != 0;
            self.0 &= !mask;
            captured
        }
    }
    fn dragged_button(&self) -> Option<u8> { (0..3).find(|button| self.0 & (1 << button) != 0) }
}

impl TermView {
    /// Cached galleys reference the current font atlas; reinstalling fonts
    /// invalidates their texture coordinates even when cell dimensions match.
    pub fn invalidate_fonts(&mut self) {
        self.metrics = None;
        self.rows.clear();
        self.fit_cache.clear();
    }

    pub fn new(session: SessionId) -> Self {
        TermView {
            session,
            rows: Vec::new(),
            fit_cache: HashMap::new(),
            metrics: None,
            selection: None,
            selecting: false,
            preedit: String::new(),
            scroll_accum: 0.0,
            last_mouse_cell: None,
            mouse_capture: MouseCapture::default(),
            search: None,
            inspector: false,
            integration_help: false,
            inspected_command: None,
            inspector_record_expired: false,
            command_draft: String::new(),
            accessible_input: String::new(),
            palette: Arc::new(Palette::default()),
            pending_size: None,
            theme_name: kiln_common::Theme::current().name,
            fill_background: true,
        }
    }

    pub fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(SearchBar { query: String::new(), focus: true, last_found: None });
        } else if let Some(s) = &mut self.search {
            s.focus = true;
        }
    }

    /// Open command history and selectable output from the terminal header.
    pub fn open_history(&mut self) {
        self.inspector = true;
        self.integration_help = false;
    }

    /// Setup is secondary to terminal work and lives in the terminal menu.
    pub fn open_integration_help(&mut self) {
        self.inspector = true;
        self.integration_help = true;
    }

    pub fn inspector_open(&self) -> bool {
        self.inspector
    }

    pub fn set_search_result(&mut self, found: bool) {
        if let Some(s) = &mut self.search {
            s.last_found = Some(found);
        }
    }

    fn cell_size(&mut self, ctx: &egui::Context, s: &TermSettings) -> Vec2 {
        let key = (s.font_size, s.line_height, ctx.pixels_per_point());
        if let Some((cached_key, v)) = self.metrics
            && cached_key == key {
                return v;
            }
        let font = FontId::monospace(s.font_size);
        let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let v = vec2(w, (h * s.line_height).round());
        self.metrics = Some((key, v));
        self.rows.clear();
        self.fit_cache.clear();
        v
    }

    /// 선택 영역의 텍스트(보이는 범위만).
    pub fn selection_text(&self, screen: &Screen) -> Option<String> {
        let sel = self.selection?;
        let (a, b) = sel.ordered();
        let off = screen.display_offset as i64;
        let mut out = String::new();
        for line in a.line..=b.line {
            let row = line + off;
            if row < 0 || row >= screen.rows as i64 {
                continue;
            }
            let l = &screen.lines[row as usize];
            let cols = l.cells.len() as u16;
            if cols == 0 { continue; }
            let (c0, c1) = match sel.mode {
                SelMode::Line => (0, cols.saturating_sub(1)),
                _ => (if line == a.line { a.col } else { 0 }, if line == b.line { b.col } else { cols.saturating_sub(1) }),
            };
            let mut s = String::new();
            for c in c0..=c1.min(cols.saturating_sub(1)) {
                let cell = &l.cells[c as usize];
                if cell.flags & flags::SPACER != 0 {
                    continue;
                }
                s.push(cell.c);
                for (_, combining) in l.combining.iter().filter(|(col, _)| *col == c) { s.push_str(combining); }
            }
            let wrapped = l.cells.last().is_some_and(|c| c.flags & flags::WRAP != 0);
            if line != b.line && wrapped && c1 + 1 >= cols {
                out.push_str(&s);
            } else {
                out.push_str(s.trim_end());
                if line != b.line {
                    out.push('\n');
                }
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    /// 선택 영역을 복사한다. 화면 밖(스크롤백)까지 걸치면 데몬에서 텍스트를 받아 복사한다.
    pub fn copy_selection(&mut self, ctx: &egui::Context, conn: &mut Conn) {
        let Some(sel) = self.selection else { return };
        let Some(screen) = conn.screens.get(&self.session) else { return };
        let (a, b) = sel.ordered();
        let off = screen.display_offset as i64;
        let visible = a.line + off >= 0 && b.line + off < screen.rows as i64;
        if visible {
            if let Some(t) = self.selection_text(screen) {
                ctx.copy_text(t);
            }
            return;
        }
        let last = screen.cols.saturating_sub(1);
        let (start, end) = match sel.mode {
            SelMode::Line => ((a.line as i32, 0), (b.line as i32, last)),
            _ => ((a.line as i32, a.col), (b.line as i32, b.col)),
        };
        conn.copy_range(self.session, start, end);
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| s.anchor != s.head || s.mode != SelMode::Cell)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, conn: &mut Conn, settings: &TermSettings, focus_request: bool, cwd: Option<&str>) -> TermOutput {
        let mut out = TermOutput::default();
        if self.inspector {
            out.clicked = ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i|i.pointer.any_pressed());
            self.inspector_ui(ui, conn, cwd, &mut out);
            return out;
        }
        let rect = ui.available_rect_before_wrap();
        let id = ui.id().with(("term", self.session));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        ui.advance_cursor_after_rect(rect);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, kiln_common::i18n::tr("터미널 화면")));
        if !self.inspector && (focus_request || resp.clicked() || resp.drag_started()) {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        out.focused = focused;
        out.clicked = resp.clicked() || resp.drag_started();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
            });
        }

        let cell = self.cell_size(ui.ctx(), settings);
        conn.set_cell_px(cell.x.round().max(1.0) as u16, cell.y.round().max(1.0) as u16);
        let pad = vec2(10.0, 6.0);
        let inner = rect.shrink2(pad);
        let cols = ((inner.width() / cell.x).floor() as u16).max(2);
        let rows = ((inner.height() / cell.y).floor() as u16).max(1);
        self.request_size(ui.ctx(), conn, cols, rows);

        let theme = kiln_common::Theme::current();
        if theme.name != self.theme_name {
            self.theme_name = theme.name;
            self.palette = Arc::new(Palette::from_theme(&theme));
            self.rows.clear();
        }
        let painter = ui.painter_at(rect);
        if self.fill_background {
            painter.rect_filled(rect, 0.0, self.palette.bg);
        }

        let screen_exists = conn.screens.contains_key(&self.session);
        if !screen_exists {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, kiln_common::i18n::tr("연결 중…"), FontId::proportional(13.0), Color32::GRAY);
            return out;
        }

        // 입력 처리 (화면 캐시를 빌리기 전에).
        let term_mode = conn.screens.get(&self.session).map(|s| s.mode).unwrap_or(0);
        // Preserve application mouse reporting. Shift-right-click always opens Kiln's menu.
        let app_handles_mouse = term_mode & (mode::MOUSE_CLICK | mode::MOUSE_DRAG | mode::MOUSE_MOTION) != 0;
        if !app_handles_mouse || ui.input(|i| i.modifiers.shift) || resp.context_menu_opened() {
            resp.context_menu(|ui| {
                if ui.add_enabled(self.has_selection(), egui::Button::new(kiln_common::i18n::tr("복사"))).clicked() {
                    self.copy_selection(ui.ctx(), conn);
                    ui.close();
                }
                if ui.button(kiln_common::i18n::tr("출력 검색")).clicked() {
                    self.open_search();
                    ui.close();
                }
                if ui.button(kiln_common::i18n::tr("명령 기록")).clicked() {
                    self.open_history();
                    ui.close();
                }
                ui.separator();
                if ui.button(kiln_common::i18n::tr("셸·에이전트 연동")).clicked() {
                    self.open_integration_help();
                    ui.close();
                }
            });
        }
        if self.inspector {
            out.clicked = true;
            return out;
        }
        let exited = conn.infos.get(&self.session).and_then(|i| i.exited);
        if focused && !resp.context_menu_opened() && !self.inspector && self.search.as_ref().is_none_or(|s| !s.focus) {
            self.handle_keyboard(ui, conn, term_mode, settings, exited, &mut out);
        }
        self.handle_mouse(ui, &resp, conn, inner.min, cell, term_mode, &mut out, cwd);
        if settings.copy_on_select && !self.inspector && self.has_selection() && (resp.drag_stopped() || resp.double_clicked() || resp.triple_clicked()) {
            self.copy_selection(ui.ctx(),conn);
        }

        let screen = conn.screens.get(&self.session).unwrap();
        // 행 캐시 갱신 후 그리기.
        if self.rows.len() != screen.rows as usize {
            self.rows.clear();
            for _ in 0..screen.rows {
                self.rows.push(RowCache { version: u64::MAX, key: 0, shapes: Vec::new() });
            }
        }
        let font = FontId::monospace(settings.font_size);
        let key = (settings.font_size.to_bits() as u64) << 32 | cell.y.to_bits() as u64;
        let mut shapes: Vec<Shape> = Vec::with_capacity(screen.rows as usize * 4);
        for r in 0..screen.rows as usize {
            let rc = &mut self.rows[r];
            let ver = screen.row_versions.get(r).copied().unwrap_or(0);
            if rc.version != ver || rc.key != key {
                rc.shapes = build_row(ui.ctx(), &screen.lines[r], cell, &font, &self.palette, &mut self.fit_cache);
                rc.version = ver;
                rc.key = key;
            }
            let origin = vec2(inner.min.x, inner.min.y + r as f32 * cell.y);
            for s in &rc.shapes {
                let mut s = s.clone();
                s.translate(origin);
                shapes.push(s);
            }
        }
        painter.extend(shapes);

        // 인라인 이미지(비율 유지, 셀 영역 안에 왼쪽 위 정렬).
        for im in &screen.images {
            let Some(tex) = conn.textures.get(&(self.session, im.id)) else { continue };
            let area = Rect::from_min_size(
                pos2(inner.min.x + im.col as f32 * cell.x, inner.min.y + im.row as f32 * cell.y),
                vec2(im.cols as f32 * cell.x, im.rows as f32 * cell.y),
            );
            let size = tex.size_vec2();
            let scale = (area.width() / size.x.max(1.0)).min(area.height() / size.y.max(1.0));
            let r = Rect::from_min_size(area.min, size * scale);
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // 선택 영역.
        if let Some(sel) = self.selection {
            let (a, b) = sel.ordered();
            let off = screen.display_offset as i64;
            for line in a.line..=b.line {
                let row = line + off;
                if row < 0 || row >= screen.rows as i64 {
                    continue;
                }
                let (c0, c1) = match sel.mode {
                    SelMode::Line => (0, screen.cols - 1),
                    _ => (if line == a.line { a.col } else { 0 }, if line == b.line { b.col } else { screen.cols - 1 }),
                };
                if sel.anchor == sel.head && sel.mode == SelMode::Cell {
                    continue;
                }
                let r = Rect::from_min_size(pos2(inner.min.x + c0 as f32 * cell.x, inner.min.y + row as f32 * cell.y), vec2((c1 - c0 + 1) as f32 * cell.x, cell.y));
                painter.rect_filled(r, 0.0, self.palette.selection);
            }
        }

        if settings.cursor_blink && focused { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
        // 커서와 IME 조합 문자열.
        if let Some(c) = screen.cursor {
            let pos = pos2(inner.min.x + c.col as f32 * cell.x, inner.min.y + c.row as f32 * cell.y);
            let wide = screen.lines.get(c.row as usize).and_then(|l| l.cells.get(c.col as usize)).is_some_and(|x| x.flags & flags::WIDE != 0);
            let w = if wide { cell.x * 2.0 } else { cell.x };
            let crect = Rect::from_min_size(pos, vec2(w, cell.y));
            if focused {
                ui.ctx().output_mut(|o| {
                    o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Terminal, rect, cursor_rect: crect, should_interrupt_composition: false })
                });
            }
            if !self.preedit.is_empty() {
                let (fg, bg) = screen.lines.get(c.row as usize).and_then(|l| l.cells.get(c.col as usize))
                    .map(|c| self.palette.cell_colors(c)).unwrap_or((self.palette.fg, self.palette.bg));
                let line = preedit_line(&self.preedit, fg);
                let r = Rect::from_min_size(pos, vec2((line.cells.len() as f32 * cell.x).max(cell.x), cell.y));
                painter.rect_filled(r, 0.0, bg);
                for mut shape in build_row(ui.ctx(), &line, cell, &font, &self.palette, &mut self.fit_cache) {
                    shape.translate(pos.to_vec2());
                    painter.add(shape);
                }
                painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, fg));
            } else if screen.display_offset == 0 && (!settings.cursor_blink || !focused || ui.input(|i| (i.time * 2.0) as u64 % 2 == 0)) {
                let color = self.palette.cursor;
                match (c.shape, focused) {
                    (CursorShape::Block, true) => {
                        painter.rect_filled(crect, 1.0, color);
                        if let Some(line) = screen.lines.get(c.row as usize) {
                            let line = cursor_line(line, c.col, self.palette.bg);
                            for mut shape in build_row(ui.ctx(), &line, cell, &font, &self.palette, &mut self.fit_cache) {
                                shape.translate(pos.to_vec2());
                                painter.add(shape);
                            }
                        }
                    }
                    (CursorShape::Beam, _) => {
                        painter.rect_filled(Rect::from_min_size(pos, vec2(2.0, cell.y)), 0.0, color);
                    }
                    (CursorShape::Underline, _) => {
                        painter.rect_filled(Rect::from_min_size(pos2(pos.x, pos.y + cell.y - 2.0), vec2(w, 2.0)), 0.0, color);
                    }
                    _ => {
                        painter.rect_stroke(crect.shrink(0.5), 1.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                    }
                }
            }
        }

        // 스크롤백 위치 표시.
        if screen.display_offset > 0 && screen.history > 0 {
            let t = kiln_common::Theme::current();
            let total = (screen.history + screen.rows as u32) as f32;
            let h = (rect.height() * screen.rows as f32 / total).max(24.0);
            let top_frac = (screen.history - screen.display_offset) as f32 / (screen.history as f32).max(1.0);
            let y = rect.top() + top_frac * (rect.height() - h);
            painter.rect_filled(Rect::from_min_size(pos2(rect.right() - 6.0, y), vec2(3.0, h)), 2.0, t.text_faint);
            let label = kiln_common::trf!("{} 줄 위", screen.display_offset);
            let g = painter.layout_no_wrap(label, kiln_common::fonts::medium(11.5), t.text);
            let br = Rect::from_min_size(pos2(rect.right() - g.size().x - 34.0, rect.top() + 10.0), g.size() + vec2(16.0, 8.0));
            painter.rect_filled(br, 8.0, t.bg_elevated);
            painter.rect_stroke(br, 8.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
            painter.galley(br.min + vec2(8.0, 4.0), g, t.text);
        }

        if let Some(code) = exited {
            let t = kiln_common::Theme::current();
            let msg = kiln_common::trf!("프로세스가 종료됐습니다 (코드 {code})");
            let g = painter.layout_no_wrap(msg, kiln_common::fonts::medium(13.0), t.text);
            let hint_text = settings.close_shortcut.as_ref().map(|shortcut| kiln_common::trf!("↩ 새 셸   ·   {} 닫기", ui.ctx().format_shortcut(shortcut))).unwrap_or_else(|| kiln_common::i18n::tr("↩ 새 셸 시작").into());
            let hint = painter.layout_no_wrap(hint_text, kiln_common::fonts::regular(12.0), t.text_dim);
            let w = g.size().x.max(hint.size().x) + 36.0;
            let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 46.0), vec2(w, 56.0));
            painter.rect_filled(r, 12.0, t.bg_elevated);
            painter.rect_stroke(r, 12.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
            painter.galley(pos2(r.center().x - g.size().x / 2.0, r.top() + 10.0), g, t.text);
            painter.galley(pos2(r.center().x - hint.size().x / 2.0, r.top() + 31.0), hint, t.text_dim);
        }

        let searching = self.search.is_some();
        self.search_ui(ui, conn, rect);
        if searching && self.search.is_none() { resp.request_focus(); }
        if resp.has_focus() && ui.input(|i| i.focused) && ui.memory(|m| m.allows_interaction(ui.layer_id())) && !resp.context_menu_opened() {
            conn.note_terminal_focus(self.session, ui.ctx(), id);
        }
        out
    }

    fn request_size(&mut self, ctx: &egui::Context, conn: &mut Conn, cols: u16, rows: u16) {
        let now = std::time::Instant::now();
        match self.pending_size {
            Some((sz, _)) if sz == (cols, rows) => {}
            _ => self.pending_size = Some(((cols, rows), now)),
        }
        let ((c, r), since) = self.pending_size.unwrap();
        // 첫 attach 는 즉시, 이후 크기 변경은 40ms 동안 안정되면 보낸다.
        let attached_before = conn.screens.contains_key(&self.session);
        if !attached_before || since.elapsed().as_millis() >= 40 {
            conn.attach(self.session, c, r);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(40).saturating_sub(since.elapsed()));
        }
    }

    fn handle_keyboard(&mut self, ui: &mut egui::Ui, conn: &mut Conn, term_mode: u32, s: &TermSettings, exited: Option<i32>, out: &mut TermOutput) {
        let mods = ui.input(|i| i.modifiers);
        let events = terminal_input_events(ui.input(|i| i.events.clone()), mods, s.option_as_meta, cfg!(target_os = "macos"));
        let sid = self.session;
        let mut bytes: Vec<u8> = Vec::new();
        let is_mac = cfg!(target_os = "macos");
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    bytes.extend_from_slice(t.as_bytes());
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    if modifiers.mac_cmd || (!is_mac && modifiers.ctrl && modifiers.shift && matches!(key, egui::Key::C | egui::Key::V)) {
                        continue;
                    }
                    if exited.is_some() {
                        if key == egui::Key::Enter {
                            out.restart = true;
                        }
                        continue;
                    }
                    if let Some(b) = keys::encode_key(key, modifiers, term_mode, s.option_as_meta) {
                        bytes.extend(b);
                        self.selection = None;
                    }
                }
                egui::Event::Paste(p) => {
                    bytes.extend(keys::paste_bytes(&p, term_mode));
                }
                egui::Event::Copy => {
                    if is_mac || mods.shift {
                        self.copy_selection(ui.ctx(), conn);
                    } else if self.has_selection() {
                        self.copy_selection(ui.ctx(), conn);
                        self.selection = None;
                    } else {
                        bytes.push(0x03);
                    }
                }
                egui::Event::Cut => {
                    if !is_mac {
                        bytes.push(0x18);
                    }
                }
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    self.preedit = text;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                    self.preedit.clear();
                    bytes.extend_from_slice(text.as_bytes());
                }
                _ => {}
            }
        }
        if !bytes.is_empty() && exited.is_none() {
            conn.input(sid, bytes);
            conn.send(kiln_proto::ClientMsg::ClearAttention { session: sid });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_mouse(&mut self, ui: &mut egui::Ui, resp: &egui::Response, conn: &mut Conn, origin: Pos2, cell: Vec2, term_mode: u32, out: &mut TermOutput, cwd: Option<&str>) {
        let Some(screen) = conn.screens.get(&self.session) else { return };
        let (cols, rows, offset) = (screen.cols, screen.rows, screen.display_offset as i64);
        let to_cell = |p: Pos2| -> (u16, u16) {
            let c = ((p.x - origin.x) / cell.x).floor().clamp(0.0, cols.saturating_sub(1) as f32) as u16;
            let r = ((p.y - origin.y) / cell.y).floor().clamp(0.0, rows.saturating_sub(1) as f32) as u16;
            (c, r)
        };
        let mods = ui.input(|i| i.modifiers);
        let mouse_mode = term_mode & (mode::MOUSE_CLICK | mode::MOUSE_DRAG | mode::MOUSE_MOTION) != 0 && !mods.shift;
        let sid = self.session;

        // 휠.
        if resp.hovered() {
            let mut dy = 0.0;
            for ev in ui.input(|i| i.events.clone()) {
                if let egui::Event::MouseWheel { unit, delta, .. } = ev {
                    dy += match unit {
                        egui::MouseWheelUnit::Point => delta.y / cell.y,
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Page => delta.y * rows as f32,
                    };
                }
            }
            self.scroll_accum += dy;
            let lines = self.scroll_accum.trunc() as i32;
            if lines != 0 {
                self.scroll_accum -= lines as f32;
                if mouse_mode {
                    if let Some(p) = resp.hover_pos() {
                        let (c, r) = to_cell(p);
                        let btn = if lines > 0 { 64 } else { 65 };
                        let mut b = Vec::new();
                        for _ in 0..lines.unsigned_abs().min(10) {
                            b.extend(keys::mouse_report(btn, c, r, true, mods, term_mode));
                        }
                        conn.input(sid, b);
                    }
                } else if term_mode & mode::ALT_SCREEN != 0 && term_mode & mode::ALT_SCROLL != 0 {
                    let seq: &[u8] = if lines > 0 {
                        if term_mode & mode::APP_CURSOR != 0 { b"\x1bOA" } else { b"\x1b[A" }
                    } else if term_mode & mode::APP_CURSOR != 0 {
                        b"\x1bOB"
                    } else {
                        b"\x1b[B"
                    };
                    conn.input(sid, seq.repeat(lines.unsigned_abs().min(10) as usize));
                } else if term_mode & mode::ALT_SCREEN == 0 {
                    conn.send(kiln_proto::ClientMsg::Scroll { session: sid, scroll: ScrollTo::Delta(lines) });
                }
            }
        }

        let app_handles_mouse = term_mode & (mode::MOUSE_CLICK | mode::MOUSE_DRAG | mode::MOUSE_MOTION) != 0;
        if app_handles_mouse {
            for ev in ui.input(|i| i.events.clone()) {
                match ev {
                    egui::Event::PointerButton { pos, button, pressed, modifiers } => {
                        let b = match button {
                            egui::PointerButton::Primary => 0,
                            egui::PointerButton::Middle => 1,
                            egui::PointerButton::Secondary => 2,
                            _ => continue,
                        };
                        let owns_pointer = resp.rect.contains(pos) && resp.contains_pointer() && !resp.context_menu_opened();
                        if self.mouse_capture.button(b, pressed, modifiers.shift, owns_pointer) {
                            let (c, r) = to_cell(pos);
                            conn.input(sid, keys::mouse_report(b, c, r, pressed, modifiers, term_mode));
                        }
                    }
                    egui::Event::PointerMoved(pos) if mouse_mode && !resp.context_menu_opened()
                        && (resp.contains_pointer() || self.mouse_capture.dragged_button().is_some()) => {
                        let (c, r) = to_cell(pos);
                        if self.last_mouse_cell != Some((c, r)) {
                            self.last_mouse_cell = Some((c, r));
                            let dragged = self.mouse_capture.dragged_button();
                            if (dragged.is_some() && term_mode & (mode::MOUSE_DRAG | mode::MOUSE_MOTION) != 0) || term_mode & mode::MOUSE_MOTION != 0 {
                                conn.input(sid, keys::mouse_report(32 + dragged.unwrap_or(3), c, r, true, mods, term_mode));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if mouse_mode { return; }
        } else {
            self.mouse_capture = MouseCapture::default();
        }

        // 선택.
        let abs = |(c, r): (u16, u16)| Point { line: r as i64 - offset, col: c };
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (c, r) = to_cell(p);
                let line = &screen.lines[r as usize];
                let (a, b) = word_bounds(line, c);
                self.selection = Some(Selection { anchor: abs((a, r)), head: abs((b, r)), mode: SelMode::Word });
            }
        } else if resp.triple_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (_, r) = to_cell(p);
                self.selection = Some(Selection { anchor: abs((0, r)), head: abs((cols - 1, r)), mode: SelMode::Line });
            }
        } else if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let pt = abs(to_cell(p));
                self.selection = Some(Selection { anchor: pt, head: pt, mode: SelMode::Cell });
                self.selecting = true;
            }
        } else if self.selecting && resp.dragged() {
            if let (Some(p), Some(sel)) = (resp.interact_pointer_pos(), self.selection.as_mut()) {
                sel.head = abs(to_cell(p));
                // 화면 밖으로 끌면 스크롤한다.
                if p.y < resp.rect.top() {
                    conn.send(kiln_proto::ClientMsg::Scroll { session: sid, scroll: ScrollTo::Delta(1) });
                } else if p.y > resp.rect.bottom() {
                    conn.send(kiln_proto::ClientMsg::Scroll { session: sid, scroll: ScrollTo::Delta(-1) });
                }
            }
        } else if resp.drag_stopped() {
            self.selecting = false;
        } else if resp.clicked() && !(mods.command) {
            self.selection = None;
        }

        // Cmd(Ctrl)+클릭 링크.
        if mods.command
            && let Some(p) = resp.hover_pos() {
                let (c, r) = to_cell(p);
                let line = &screen.lines[r as usize];
                let osc8 = line.links.iter().find(|(s, e, _)| (*s..=*e).contains(&c)).map(|(s, e, u)| (*s as usize, *e as usize + 1, link_target(u)));
                let text = line.text();
                if let Some((start, end, target)) = osc8.or_else(|| find_link(&text, c as usize, cwd)) {
                    let painter = ui.painter();
                    let y = origin.y + (r as f32 + 1.0) * cell.y - 1.0;
                    painter.line_segment([pos2(origin.x + start as f32 * cell.x, y), pos2(origin.x + end as f32 * cell.x, y)], Stroke::new(1.0, kiln_common::Theme::current().accent));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        out.open = Some(target);
                    }
                }
            }
    }

    /// A selectable text alternative to the GPU terminal canvas, plus explicit command review.
    fn inspector_ui(&mut self, host: &mut egui::Ui, conn: &mut Conn, cwd: Option<&str>, out: &mut TermOutput) {
        if !self.inspector { return; }
        let ctx = host.ctx().clone();
        let t = kiln_common::Theme::current();
        let telemetry = conn.telemetry.get(&self.session).cloned().unwrap_or_default();
        if self.inspected_command.is_some_and(|id| !telemetry.commands.iter().any(|record|record.id==id)) {
            self.inspected_command = None;
            self.inspector_record_expired = true;
        }
        let screen_text = conn.screens.get(&self.session).map(|s| s.lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        let mut close = false;
        let available_height = host.available_height();
        let frame = egui::Frame::new().fill(t.bg_panel).inner_margin(8);
        frame.show(host, |ui| {
            ui.set_width((ui.available_width()-16.0).max(100.0));
            ui.spacing_mut().scroll.floating = false;
            ui.spacing_mut().scroll.dormant_handle_opacity = 0.65;
            ui.horizontal_wrapped(|ui| {
                close = ui.button(kiln_common::i18n::tr("← 터미널")).clicked();
                ui.label(egui::RichText::new(kiln_common::i18n::tr("명령 기록")).color(t.text).strong()).on_hover_text(kiln_common::i18n::tr("최근 40개 명령 · 출력은 명령당 앞부분 128KiB. 데몬 재시작·업그레이드 시 기록이 사라집니다."));
                if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::Plug, 28.0, self.integration_help, kiln_common::i18n::tr("셸·에이전트 연동")).clicked() {
                    self.integration_help = !self.integration_help;
                }
            });
            let body_height=(available_height-116.0).max(32.0);
            egui::ScrollArea::vertical().id_salt(("command-inspector",self.session)).max_height(body_height).auto_shrink([false,false]).show(ui, |ui| {
                if self.integration_help {
                    ui.label(if telemetry.shell_integration {kiln_common::i18n::tr("셸: 연결됨")} else {kiln_common::i18n::tr("셸: 명령 경계를 받지 못했습니다")});
                    ui.label(kiln_common::trf!("에이전트: {}",kiln_common::i18n::tr(telemetry.activity.label())));
                    ui.label(kiln_common::i18n::tr("설정 → 터미널에서 기본 셸을 /bin/zsh로 지정하고, Kiln의 + 버튼으로 새 작업을 여세요. 새 터미널은 자동 연결됩니다. 기존 zsh에는 그다음 아래 명령을 직접 실행하세요. 셸 안에서 zsh만 실행하면 연결 파일이 생성되지 않습니다."));
                    if ui.button(kiln_common::i18n::tr("zsh 연결 명령 복사")).clicked(){ctx.copy_text("source ~/.local/share/kiln/shell-integration-v1/integration.zsh".into());}
                    ui.label(kiln_common::i18n::tr("에이전트 상태는 훅에서 kiln activity running / waiting / done / failed로 알립니다. 명령을 실행할 수 있는 셸 프롬프트에서 테스트하세요."));
                    if ui.button(kiln_common::i18n::tr("연결 테스트 명령 복사")).clicked(){ctx.copy_text(r"printf '\033]777;kiln-agent;waiting\007'".into());}
                    ui.label(kiln_common::i18n::tr("테스트 후 에이전트 상태가 ‘입력 필요’로 바뀌고 알림에 표시됩니다. kiln activity unknown으로 초기화할 수 있습니다."));
                    return;
                } else if !telemetry.shell_integration {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(kiln_common::i18n::tr("셸을 연결하면 명령 기록이 표시됩니다."));
                        if ui.link(kiln_common::i18n::tr("연결 방법")).clicked() { self.integration_help = true; }
                    });
                }
                if self.inspector_record_expired {
                    ui.label(kiln_common::i18n::tr("선택한 명령 기록이 만료되어 현재 출력을 표시합니다."));
                    if !self.command_draft.is_empty() {
                        ui.label(kiln_common::i18n::tr("편집하던 명령은 아래에 남겨두었습니다. 복사한 뒤 다른 기록을 선택하세요."));
                        ui.add(egui::TextEdit::multiline(&mut self.command_draft).desired_rows(2).desired_width(f32::INFINITY));
                        if ui.button(kiln_common::i18n::tr("명령 초안 복사")).clicked(){ctx.copy_text(self.command_draft.clone());}
                    }
                }
                let old = self.inspected_command;
                egui::ComboBox::from_id_salt(("command-picker",self.session)).width(ui.available_width()-16.0)
                    .selected_text(self.inspected_command.and_then(|id| telemetry.commands.iter().find(|c|c.id==id)).map(|c| command_label(c)).unwrap_or_else(|| kiln_common::i18n::tr("현재 보이는 출력").into()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.inspected_command,None,kiln_common::i18n::tr("현재 보이는 출력"));
                        for c in telemetry.commands.iter().rev() { ui.selectable_value(&mut self.inspected_command,Some(c.id),command_label(c)); }
                    });
                ui.horizontal_wrapped(|ui| {
                    let index=self.inspected_command.and_then(|id|telemetry.commands.iter().position(|c|c.id==id));
                    let previous=index.map(|i|i.checked_sub(1)).unwrap_or_else(||telemetry.commands.len().checked_sub(1));
                    ui.add_enabled_ui(previous.is_some(), |ui| {
                        if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::ArrowUp, 28.0, false, kiln_common::i18n::tr("이전 명령")).clicked() { self.inspected_command=previous.map(|i|telemetry.commands[i].id); }
                    });
                    let next=index.and_then(|i|telemetry.commands.get(i+1)).map(|c|c.id);
                    ui.add_enabled_ui(next.is_some(), |ui| {
                        if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::ArrowDown, 28.0, false, kiln_common::i18n::tr("다음 명령")).clicked() { self.inspected_command=next; }
                    });
                    if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::Terminal, 28.0, self.inspected_command.is_none(), kiln_common::i18n::tr("현재 출력")).clicked() { self.inspected_command=None; }
                    if let Some(record) = self.inspected_command.and_then(|id| telemetry.commands.iter().find(|c| c.id == id)) {
                        ui.add_enabled_ui(record.output_available, |ui| {
                            if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::Refresh, 28.0, false, kiln_common::i18n::tr("명령 출력 다시 읽기")).clicked() { conn.read_command_output(self.session, record.id); }
                        });
                    }
                });
                if old != self.inspected_command {
                    self.inspector_record_expired = false;
                    self.command_draft = self.inspected_command.and_then(|id|telemetry.commands.iter().find(|c|c.id==id)).map(|c|c.command.clone()).unwrap_or_default();
                    if let Some(id)=self.inspected_command.filter(|id|telemetry.commands.iter().any(|c|c.id==*id && c.output_available)) { conn.read_command_output(self.session,id); }
                }
                let command = self.inspected_command.and_then(|id|telemetry.commands.iter().find(|c|c.id==id));
                let output = self.inspected_command.and_then(|id|conn.command_outputs.get(&(self.session,id)));
                let display = if self.inspected_command.is_none() { screen_text.as_str() }
                    else if let Some((text, _)) = output { text.as_str() }
                    else if command.is_some_and(|c| c.finished_unix.is_none()) { kiln_common::i18n::tr("실행 중인 명령입니다. ‘현재 출력’에서 진행 상황을 확인하세요.") }
                    else if command.is_some_and(|c| !c.output_available) { kiln_common::i18n::tr("보관된 출력이 없습니다. 명령 내용은 아래에서 확인할 수 있습니다.") }
                    else { kiln_common::i18n::tr("출력을 불러오는 중… 응답이 없으면 ‘명령 출력 다시 읽기’를 누르세요.") };
                if output.is_some_and(|x|x.1) { ui.label(egui::RichText::new(kiln_common::i18n::tr("출력이 기록 제한(128KiB)을 초과해 앞부분만 보관했습니다.")).color(t.orange)); }
                let label = ui.label(if self.inspected_command.is_none() { kiln_common::i18n::tr("보이는 터미널 출력 · 읽기 전용") } else { kiln_common::i18n::tr("선택한 명령 출력 · 읽기 전용") });
                let mut readonly: &str = display;
                ui.add(egui::TextEdit::multiline(&mut readonly).font(egui::TextStyle::Monospace).desired_rows(8).desired_width(f32::INFINITY)).labelled_by(label.id);
                if let Some(command)=command {
                    ui.label(kiln_common::trf!("작업 폴더: {}",command.cwd.as_deref().or(cwd).unwrap_or(kiln_common::i18n::tr("확인 안 됨"))));
                    let label=ui.label(kiln_common::i18n::tr("명령 편집 · 아래 실행 버튼을 눌러야 실행됩니다"));
                    ui.add(egui::TextEdit::multiline(&mut self.command_draft).desired_rows(2).desired_width(f32::INFINITY).code_editor()).labelled_by(label.id);
                } else {
                    let label=ui.label(kiln_common::i18n::tr("터미널 입력 · 전송 후 터미널에서 Enter로 실행"));
                    ui.add(egui::TextEdit::singleline(&mut self.accessible_input).desired_width(f32::INFINITY)).labelled_by(label.id);
                    if ui.button(kiln_common::i18n::tr("입력만 보내기")).clicked() {
                        let text=self.accessible_input.replace(['\r','\n']," ");
                        conn.input(self.session,text.into_bytes()); self.accessible_input.clear();
                    }
                    if ui.button(kiln_common::i18n::tr("터미널에 Enter 보내기")).clicked() { conn.input(self.session,vec![b'\r']); }
                }
            });
            if !self.integration_help { ui.horizontal_wrapped(|ui| {
                let output = if self.inspected_command.is_none(){Some(screen_text.as_str())}else{self.inspected_command.and_then(|id|conn.command_outputs.get(&(self.session,id))).map(|o|o.0.as_str())};
                if ui.add_enabled(output.is_some(),egui::Button::new(kiln_common::i18n::tr("출력 복사"))).clicked(){ctx.copy_text(output.unwrap_or_default().to_owned());}
            if let Some(command)=self.inspected_command.and_then(|id|telemetry.commands.iter().find(|c|c.id==id)) {
                if ui.add_enabled(!self.command_draft.trim().is_empty(),egui::Button::new(kiln_common::i18n::tr("새 터미널에서 실행"))).clicked() {
                    out.command_to_run=Some((self.command_draft.clone(),command.cwd.clone())); close=true;
                }
            }
            }); }
        });
        if close { self.inspector=false; }
    }

    fn search_ui(&mut self, ui: &mut egui::Ui, conn: &mut Conn, rect: Rect) {
        use kiln_common::icons::Icon;
        use kiln_common::widgets;
        let Some(sb) = &mut self.search else { return };
        let t = kiln_common::Theme::current();
        let mut close = false;
        let width = 340.0f32.min(rect.width() - 20.0);
        let area = egui::Area::new(ui.id().with(("term-search", self.session))).fixed_pos(pos2(rect.right() - width - 12.0, rect.top() + 10.0)).order(egui::Order::Foreground);
        area.show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(t.bg_elevated)
                .stroke(Stroke::new(1.0, t.border_strong))
                .corner_radius(egui::CornerRadius::same(10))
                .shadow(t.shadow())
                .inner_margin(egui::Margin { left: 10, right: 6, top: 5, bottom: 5 })
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let (ir, _) = ui.allocate_exact_size(vec2(16.0, 26.0), Sense::hover());
                        kiln_common::icons::paint(ui.painter(), Rect::from_center_size(ir.center(), vec2(13.0, 13.0)), Icon::Search, t.text_faint);
                        ui.add_space(4.0);
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut sb.query)
                                .hint_text(kiln_common::i18n::tr("스크롤백에서 찾기"))
                                .frame(egui::Frame::NONE)
                                .font(kiln_common::fonts::regular(13.0))
                                .desired_width(width - 150.0),
                        );
                        if sb.focus {
                            te.request_focus();
                            sb.focus = false;
                        }
                        let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let shift = ui.input(|i| i.modifiers.shift);
                        let go = |backward: bool| {
                            let req = conn.next_req();
                            conn.send(kiln_proto::ClientMsg::Search { req, session: self.session, query: sb.query.clone(), backward });
                        };
                        if enter {
                            go(!shift);
                            te.request_focus();
                        }
                        if sb.last_found == Some(false) {
                            ui.label(egui::RichText::new(kiln_common::i18n::tr("없음")).size(12.0).color(t.red));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::Close, 24.0, false, kiln_common::i18n::tr("닫기 (Esc)")).clicked() || ((te.has_focus() || te.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
                                close = true;
                            }
                            if widgets::icon_button(ui, Icon::ArrowDown, 24.0, false, kiln_common::i18n::tr("다음 (⇧↩)")).clicked() {
                                go(false);
                            }
                            let up = widgets::icon_button(ui, Icon::ArrowUp, 24.0, false, kiln_common::i18n::tr("이전 (↩)"));
                            if up.clicked() {
                                go(true);
                            }
                        });
                    });
                });
        });
        if close {
            self.search = None;
            conn.send(kiln_proto::ClientMsg::Scroll { session: self.session, scroll: ScrollTo::Bottom });
        }
    }
}

fn word_bounds(line: &Line, col: u16) -> (u16, u16) {
    let is_word = |c: &Cell| !(c.c.is_whitespace() || "\"'`()[]{}<>|,;".contains(c.c));
    let cells = &line.cells;
    if cells.is_empty() {
        return (0, 0);
    }
    let col = (col as usize).min(cells.len() - 1);
    if !is_word(&cells[col]) {
        return (col as u16, col as u16);
    }
    let mut a = col;
    while a > 0 && is_word(&cells[a - 1]) {
        a -= 1;
    }
    let mut b = col;
    while b + 1 < cells.len() && is_word(&cells[b + 1]) {
        b += 1;
    }
    (a as u16, b as u16)
}

fn command_label(command: &kiln_proto::CommandRecord) -> String {
    let status=match (command.finished_unix,command.exit_code) { (None,_)=>kiln_common::i18n::tr("실행 중").into(),(_,Some(0))=>kiln_common::i18n::tr("성공").into(),(_,Some(code))=>kiln_common::trf!("종료 {code}"),_=>kiln_common::i18n::tr("종료 코드 확인 안 됨").into() };
    let text=if command.command.is_empty(){kiln_common::i18n::tr("명령 텍스트 확인 안 됨")}else{&command.command};
    format!("{} · {} · {}",command.id,status,text.chars().take(80).collect::<String>())
}

/// OSC 8 URI 를 열 대상으로 바꾼다(`file://` 는 로컬 파일).
fn link_target(uri: &str) -> LinkTarget {
    if let Some(rest) = uri.strip_prefix("file://") {
        let path = &rest[rest.find('/').unwrap_or(0)..];
        return LinkTarget::File { path: std::path::PathBuf::from(path), line: None, col: None };
    }
    LinkTarget::Url(uri.to_string())
}

/// `text` 의 `col` 위치에 있는 URL 또는 파일 경로(존재하는 파일만)를 찾는다.
/// 반환값의 열 범위는 문자 단위다.
pub fn find_link(text: &str, col: usize, cwd: Option<&str>) -> Option<(usize, usize, LinkTarget)> {
    use std::sync::OnceLock;
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    static PATH: OnceLock<regex::Regex> = OnceLock::new();
    let url = URL.get_or_init(|| regex::Regex::new(r#"(?:https?|file)://[^\s<>"'`]+[^\s<>"'`.,;:)\]]"#).unwrap());
    let path = PATH.get_or_init(|| regex::Regex::new(r"(?:~|\.{1,2})?/?[\w@.\-+]+(?:/[\w@.\-+]+)*(?::(\d+))?(?::(\d+))?").unwrap());
    // 바이트 위치를 문자 열로 변환.
    let char_col = |byte: usize| text[..byte].chars().count();
    for m in url.find_iter(text) {
        let (s, e) = (char_col(m.start()), char_col(m.end()));
        if (s..e).contains(&col) {
            return Some((s, e, LinkTarget::Url(m.as_str().to_string())));
        }
    }
    for caps in path.captures_iter(text) {
        let m = caps.get(0).unwrap();
        let (s, e) = (char_col(m.start()), char_col(m.end()));
        if !(s..e).contains(&col) {
            continue;
        }
        let raw = m.as_str();
        let file_part = raw.split(':').next().unwrap_or(raw);
        if !file_part.contains('/') && !file_part.contains('.') {
            return None;
        }
        let expanded = if let Some(rest) = file_part.strip_prefix("~/") {
            std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(rest)).ok()?
        } else {
            let p = std::path::PathBuf::from(file_part);
            if p.is_absolute() { p } else { std::path::PathBuf::from(cwd?).join(p) }
        };
        if expanded.exists() {
            let line = caps.get(1).and_then(|x| x.as_str().parse().ok());
            let c = caps.get(2).and_then(|x| x.as_str().parse().ok());
            return Some((s, e, LinkTarget::File { path: expanded, line, col: c }));
        }
        return None;
    }
    None
}

/// Use the emulator's Unicode column widths so composition has the same glyph
/// sizing, fallback fonts, and alignment as the text after it is committed.
fn preedit_line(text: &str, fg: Color32) -> Line {
    let mut line = Line::default();
    let mut base: Option<u16> = None;
    for c in text.chars() {
        match c.width() {
            Some(0) => {
                if let Some(col) = base { line.combining.push((col, c.to_string())); }
            }
            Some(width) => {
                base = Some(line.cells.len() as u16);
                line.cells.push(Cell { c, fg: Color::Rgb(fg.r(), fg.g(), fg.b()), flags: if width == 2 { flags::WIDE } else { 0 }, ..Default::default() });
                if width == 2 { line.cells.push(Cell { flags: flags::SPACER, ..Default::default() }); }
            }
            None => {}
        }
    }
    line
}

/// Repaint only the glyph on a block cursor, without its original background or
/// style colors. Hidden text must never be revealed by moving the cursor over it.
fn cursor_line(line: &Line, col: u16, fg: Color32) -> Line {
    let Some(c) = line.cells.get(col as usize).filter(|c| c.flags & (flags::HIDDEN | flags::SPACER) == 0) else { return Line::default() };
    Line {
        cells: vec![Cell { c: c.c, fg: Color::Rgb(fg.r(), fg.g(), fg.b()), flags: c.flags & flags::WIDE, ..Default::default() }],
        combining: line.combining.iter().filter(|(i, _)| *i == col).map(|(_, text)| (0, text.clone())).collect(),
        links: vec![],
    }
}

/// 한 줄을 셰이프로 만든다. 좌표는 줄 원점 기준.
fn build_row(ctx: &egui::Context, line: &Line, cell: Vec2, font: &FontId, pal: &Palette, fit: &mut HashMap<char, bool>) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let resolve = |c: &Cell| pal.cell_colors(c);

    // 배경.
    let mut run: Option<(usize, Color32)> = None;
    let n = line.cells.len();
    for (i, c) in line.cells.iter().enumerate() {
        let (_, bg) = resolve(c);
        let bg = if bg == pal.bg { None } else { Some(bg) };
        match (run, bg) {
            (Some((_, rc)), Some(b)) if rc == b => {}
            (Some((start, rc)), _) => {
                shapes.push(Shape::rect_filled(Rect::from_min_size(pos2(start as f32 * cell.x, 0.0), vec2((i - start) as f32 * cell.x, cell.y)), 0.0, rc));
                run = bg.map(|b| (i, b));
            }
            (None, Some(b)) => run = Some((i, b)),
            (None, None) => {}
        }
    }
    if let Some((start, rc)) = run {
        shapes.push(Shape::rect_filled(Rect::from_min_size(pos2(start as f32 * cell.x, 0.0), vec2((n - start) as f32 * cell.x, cell.y)), 0.0, rc));
    }

    // 글자.
    let row_h = ctx.fonts_mut(|f| f.row_height(font));
    let y_text = ((cell.y - row_h) / 2.0).round();
    let mut text_run = String::new();
    let mut run_start = 0usize;
    let mut run_style: Option<(Color32, u16)> = None;
    let deco_mask = flags::UNDERLINE | flags::STRIKE | flags::UNDERCURL;
    let flush = |shapes: &mut Vec<Shape>, text: &mut String, start: usize, style: Option<(Color32, u16)>| {
        if let Some((fg, fl)) = style {
            let trimmed_len = text.trim_end().chars().count();
            let full_len = text.chars().count();
            if trimmed_len > 0 {
                let t: String = text.chars().take(trimmed_len).collect();
                let g = ctx.fonts_mut(|f| f.layout_no_wrap(t, font.clone(), fg));
                shapes.push(Shape::galley(pos2(start as f32 * cell.x, y_text), g, fg));
            }
            let len = if fl & deco_mask != 0 { full_len } else { 0 };
            if len > 0 {
                let x0 = start as f32 * cell.x;
                let x1 = x0 + len as f32 * cell.x;
                if fl & (flags::UNDERLINE | flags::UNDERCURL) != 0 {
                    shapes.push(Shape::line_segment([pos2(x0, cell.y - 1.5), pos2(x1, cell.y - 1.5)], Stroke::new(1.0, fg)));
                }
                if fl & flags::STRIKE != 0 {
                    shapes.push(Shape::line_segment([pos2(x0, cell.y / 2.0), pos2(x1, cell.y / 2.0)], Stroke::new(1.0, fg)));
                }
            }
        }
        text.clear();
    };
    for (i, c) in line.cells.iter().enumerate() {
        if c.flags & flags::SPACER != 0 {
            continue;
        }
        let (fg, _) = resolve(c);
        let hidden = c.flags & flags::HIDDEN != 0;
        let ch = if hidden { ' ' } else { c.c };
        let deco = c.flags & deco_mask;
        let fits = c.flags & flags::WIDE == 0 && (ch.is_ascii() || *fit.entry(ch).or_insert_with(|| {
            let w = ctx.fonts_mut(|f| f.glyph_width(font, ch));
            (w - cell.x).abs() < 0.6
        }));
        let has_combining = line.combining.iter().any(|(col, _)| *col as usize == i);
        if fits && !has_combining {
            let style = Some((fg, deco));
            if run_style != style || text_run.is_empty() {
                if !text_run.is_empty() {
                    flush(&mut shapes, &mut text_run, run_start, run_style);
                }
                run_start = i;
                run_style = style;
            }
            text_run.push(ch);
        } else {
            if !text_run.is_empty() {
                flush(&mut shapes, &mut text_run, run_start, run_style);
            }
            run_style = None;
            let mut s = ch.to_string();
            for (col, z) in &line.combining {
                if *col as usize == i {
                    s.push_str(z);
                }
            }
            let span = if c.flags & flags::WIDE != 0 { 2.0 } else { 1.0 };
            let mut g = ctx.fonts_mut(|f| f.layout_no_wrap(s.clone(), font.clone(), fg));
            // 두 칸 문자는 칸 폭을 채우도록 최대 1.4배까지 키운다.
            if span == 2.0 && g.size().x > 0.0 && g.size().x < cell.x * 1.8 {
                let scale = (cell.x * 1.92 / g.size().x).min(1.4);
                let f2 = FontId::new(font.size * scale, font.family.clone());
                g = ctx.fonts_mut(|f| f.layout_no_wrap(s, f2, fg));
            }
            let x = i as f32 * cell.x + (span * cell.x - g.size().x) / 2.0;
            let y = ((cell.y - g.size().y) / 2.0).round();
            shapes.push(Shape::galley(pos2(x, y), g, fg));
            if deco != 0 {
                flush(&mut shapes, &mut " ".repeat(span as usize), i, Some((fg, deco)));
            }
        }
    }
    if !text_run.is_empty() {
        flush(&mut shapes, &mut text_run, run_start, run_style);
    }
    for (a, b, _) in &line.links {
        let fg = line.cells.get(*a as usize).map(|c| resolve(c).0).unwrap_or(pal.fg).gamma_multiply(0.6);
        let (x0, x1) = (*a as f32 * cell.x, (*b as f32 + 1.0) * cell.x);
        let y = cell.y - 1.5;
        let mut x = x0;
        while x < x1 {
            shapes.push(Shape::line_segment([pos2(x, y), pos2((x + 2.0).min(x1), y)], Stroke::new(1.0, fg)));
            x += 4.0;
        }
    }
    shapes
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[allow(dead_code)]
pub fn modifiers_none() -> Modifiers {
    Modifiers::NONE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_typography_matches_committed_terminal_cells() {
        use egui_kittest::Harness;
        let mut emu = kiln_daemon::emu::Emu::new(80, 3);
        emu.advance("\x1b[48;2;47;49;55m\x1b[2J한으a한b e\u{301}".as_bytes());
        let mut conn = Conn::offline(egui::Context::default());
        conn.screens.insert(1, Screen {
            cols: 80, rows: 3, lines: (0..3).map(|r| emu.visible_line(r)).collect(), row_versions: vec![1; 3],
            cursor: Some(kiln_proto::Cursor { col: 0, row: 1, shape: CursorShape::Block }), ..Default::default()
        });
        let mut installed = false;
        let mut h = Harness::builder().with_size([760.0, 200.0]).build_ui_state(|ui, s: &mut (TermView, Conn, TermSettings)| {
            if !installed { kiln_common::fonts::install(ui.ctx()); installed = true; return; }
            s.0.ui(ui, &mut s.1, &s.2, true, None);
        }, (TermView::new(1), conn, TermSettings::default()));
        h.run_steps(3);
        for (size, scale) in [(13.5, 1.0), (18.0, 2.0), (24.0, 1.3)] {
            h.state_mut().2.font_size = size;
            h.set_pixels_per_point(scale);
            h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "한으a한b e\u{301}".into(), active_range_chars: None }));
            h.run_steps(3);
            let texts: Vec<_> = h.output().shapes.iter().filter_map(|s| match &s.shape { Shape::Text(t) => Some(t), _ => None }).collect();
            let cell = h.state().0.metrics.unwrap().1;
            // Compare actual terminal rendering with the overlay, including fallback font,
            // CJK enlargement, combining marks, and each glyph's grid-relative position.
            let split = texts.iter().position(|t| t.pos.y >= 6.0 + cell.y).unwrap();
            let (committed, composing) = texts.split_at(split);
            assert_eq!(committed.len(), composing.len(), "composition must use terminal cell layout");
            for (a, b) in committed.iter().zip(composing) {
                assert_eq!(a.galley.job.text, b.galley.job.text);
                assert_eq!(a.galley.job.sections[0].format.font_id, b.galley.job.sections[0].format.font_id);
                assert_eq!(a.galley.size(), b.galley.size());
                assert!((a.pos.x - b.pos.x).abs() < 0.01);
                assert!((b.pos.y - a.pos.y - cell.y).abs() < 0.01);
            }
            if size == 18.0 {
                h.render().unwrap().save("/tmp/kiln-ime-composition-2x.png").unwrap();
            }
            h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
            h.run_steps(2);
            assert!(h.state().0.preedit.is_empty());
            h.state_mut().1.screens.get_mut(&1).unwrap().cursor.as_mut().unwrap().row = 0;
            h.run_steps(2);
            let texts: Vec<_> = h.output().shapes.iter().filter_map(|s| match &s.shape { Shape::Text(t) => Some(t), _ => None }).collect();
            let (a, b) = (texts[0], *texts.last().unwrap());
            assert_eq!(a.galley.job.text, b.galley.job.text);
            assert_eq!(a.galley.job.sections[0].format.font_id, b.galley.job.sections[0].format.font_id);
            assert_eq!(a.galley.size(), b.galley.size());
            assert_eq!(a.pos, b.pos, "block cursor must not move or resize its underlying glyph");
            if size == 24.0 {
                h.render().unwrap().save("/tmp/kiln-ime-cursor-130.png").unwrap();
            }
            h.state_mut().1.screens.get_mut(&1).unwrap().cursor.as_mut().unwrap().row = 1;
        }
        h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "으".into(), active_range_chars: None }));
        h.run_steps(2);
        h.event(egui::Event::Ime(egui::ImeEvent::Commit("으".into())));
        h.run_steps(2);
        assert!(h.state().0.preedit.is_empty(), "commit removes the composition overlay");
    }

    #[test]
    fn ime_columns_and_cursor_styles_preserve_terminal_semantics() {
        for text in ["으", "ㄱ", "a한b", "e\u{301}", "한\u{301}字", "かな", "中🙂"] {
            let mut emu = kiln_daemon::emu::Emu::new(80, 1);
            emu.advance(text.as_bytes());
            let rendered = emu.visible_line(0);
            let composing = preedit_line(text, Color32::WHITE);
            for (a, b) in rendered.cells.iter().zip(&composing.cells) {
                assert_eq!(a.c, b.c, "{text}");
                assert_eq!(a.flags & (flags::WIDE | flags::SPACER), b.flags, "{text}");
            }
            assert_eq!(rendered.combining, composing.combining, "{text}");
        }
        let mut line = Line { cells: vec![Cell { c: '한', flags: flags::WIDE | flags::INVERSE | flags::BOLD, bg: Color::Idx(1), ..Default::default() }], combining: vec![(0, "\u{301}".into())], links: vec![] };
        let cursor = cursor_line(&line, 0, Color32::BLACK);
        assert_eq!(cursor.cells[0].flags, flags::WIDE);
        assert_eq!(cursor.cells[0].bg, Color::DefaultBg);
        assert_eq!(cursor.combining, line.combining);
        line.cells[0].flags |= flags::HIDDEN;
        assert!(cursor_line(&line, 0, Color32::BLACK).cells.is_empty());
        assert!(cursor_line(&line, 0, Color32::BLACK).combining.is_empty());
    }

    #[test]
    fn tui_mouse_capture_keeps_releases_in_their_split_and_excludes_shift_selection() {
        let mut left = MouseCapture::default();
        let mut right = MouseCapture::default();
        assert!(left.button(0, true, false, true));
        assert!(!right.button(0, true, false, false));
        // Drag into the other split and release with Shift newly pressed.
        assert!(left.button(0, false, true, false));
        assert!(!right.button(0, false, true, true));
        assert_eq!(left.dragged_button(), None);
        // A popup-covered press or Shift selection never enters TUI mouse reporting.
        assert!(!left.button(2, true, false, false));
        assert!(!left.button(2, false, false, true));
        assert!(!left.button(0, true, true, true));
        assert!(!left.button(0, false, false, true));
        assert!(left.button(1, true, false, true));
        assert_eq!(left.dragged_button(), Some(1));
    }

    #[test]
    fn copied_selection_preserves_combining_unicode_and_soft_wrapped_lines() {
        let mut first = Line { cells: "cafe".chars().map(|c| Cell { c, ..Default::default() }).collect(), combining: vec![(3, "\u{301}".into())], links: vec![] };
        first.cells[3].flags |= flags::WRAP;
        let second = Line { cells: " noir".chars().map(|c| Cell { c, ..Default::default() }).collect(), combining: vec![], links: vec![] };
        let screen = Screen { cols: 5, rows: 2, lines: vec![first, second], ..Default::default() };
        let mut view = TermView::new(1);
        view.selection = Some(Selection { anchor: Point { line: 0, col: 0 }, head: Point { line: 1, col: 4 }, mode: SelMode::Cell });
        assert_eq!(view.selection_text(&screen).as_deref(), Some("cafe\u{301} noir"));
    }

    #[test]
    fn closing_terminal_search_returns_keyboard_focus_to_terminal() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut view = TermView::new(1);
        view.open_search();
        let mut conn = Conn::offline(egui::Context::default());
        conn.screens.insert(1, Screen { cols: 2, rows: 1, lines: vec![Line { cells: vec![Cell::default(); 2], combining: vec![], links: vec![] }], row_versions: vec![1], ..Default::default() });
        let mut initialized = false;
        let mut h = Harness::builder().with_size([720.0, 440.0]).build_ui_state(|ui, s: &mut (TermView, Conn)| {
            if !initialized { kiln_common::fonts::install(ui.ctx()); initialized = true; return; }
            s.0.ui(ui, &mut s.1, &TermSettings::default(), false, None);
        }, (view, conn));
        h.run_steps(3);
        h.get_by_label("닫기 (Esc)").click(); h.run_steps(2);
        assert!(h.state().0.search.is_none());
        assert!(h.get_by_label("터미널 화면").is_focused());
    }

    #[test]
    fn modifier_release_in_one_frame_does_not_duplicate_meta_text() {
        use egui::{Event,Key,Modifiers};
        let key=|key,modifiers|Event::Key{key,physical_key:None,pressed:true,repeat:false,modifiers};
        let events=vec![key(Key::B,Modifiers::ALT),Event::Text("b".into()),key(Key::C,Modifiers::ALT),Event::Text("c".into())];
        let filtered=terminal_input_events(events.clone(),Modifiers::NONE,true,true);
        let mut bytes=vec![];
        for event in filtered {match event {Event::Key{key,modifiers,..}=>bytes.extend(keys::encode_key(key,modifiers,0,true).unwrap_or_default()),Event::Text(text)=>bytes.extend(text.bytes()),_=>{}}}
        assert_eq!(bytes,b"\x1bb\x1bc");
        assert_eq!(terminal_input_events(events,Modifiers::NONE,false,true).iter().filter(|e|matches!(e,Event::Text(_))).count(),2,"Option-as-Meta off preserves composed text");
        let mixed=vec![key(Key::B,Modifiers::NONE),Event::Text("b".into()),key(Key::C,Modifiers::ALT),Event::Text("c".into())];
        assert_eq!(terminal_input_events(mixed,Modifiers::ALT,true,true).iter().filter(|e|matches!(e,Event::Text(_))).count(),1,"plain text before Option press must not be lost");
    }

    #[test]
    fn inspector_expired_record_returns_to_live_output_and_preserves_edited_command() {
        use egui_kittest::{Harness,kittest::Queryable};
        let conn=Conn::offline(egui::Context::default());
        let mut view=TermView::new(1); view.inspector=true; view.inspected_command=Some(7); view.command_draft="echo edited".into();
        let mut installed=false;
        let mut h=Harness::builder().with_size([720.0,600.0]).build_ui_state(|ui,state:&mut(TermView,Conn,TermOutput)|{
            if !installed{kiln_common::fonts::install(ui.ctx());kiln_common::Theme::current().apply(ui.ctx());installed=true;return;}
            state.0.inspector_ui(ui,&mut state.1,None,&mut state.2);
        },(view,conn,TermOutput::default()));
        h.run_steps(3);
        assert_eq!(h.state().0.inspected_command,None);
        assert_eq!(h.state().0.command_draft,"echo edited");
        assert!(h.query_by_label("명령 초안 복사").is_some());
        assert!(h.query_by_label("명령 출력 다시 읽기").is_none());
        assert!(h.query_by_label("보이는 터미널 출력 · 읽기 전용").is_some());
        assert!(h.state().2.command_to_run.is_none());
    }

    #[test]
    fn cell_metrics_track_font_spacing_and_display_scale_independently() {
        let ctx=egui::Context::default(); kiln_common::fonts::install(&ctx);
        let mut view=TermView::new(1);
        let mut sizes=Vec::new();
        let first=TermSettings{font_size:12.0,line_height:1.5,..Default::default()};
        let second=TermSettings{font_size:15.0,line_height:1.2,..Default::default()};
        assert_eq!(first.font_size*first.line_height,second.font_size*second.line_height);
        let mut output=ctx.run_ui(egui::RawInput::default(),|ui| {
            sizes.push(view.cell_size(ui.ctx(),&first));
            sizes.push(view.cell_size(ui.ctx(),&second));
        });output.textures_delta.clear();
        assert!(sizes[1].x>sizes[0].x,"font size must update column width even with equal font × line spacing");
        ctx.set_pixels_per_point(1.3);
        let mut output=ctx.run_ui(egui::RawInput::default(),|ui| {
            let measured=view.cell_size(ui.ctx(),&second);
            let expected=ui.fonts_mut(|fonts|fonts.glyph_width(&FontId::monospace(15.0),'M'));
            assert_eq!(measured.x,expected);
            assert_eq!(view.metrics.unwrap().0.2,ui.ctx().pixels_per_point());
        });output.textures_delta.clear();
    }


    #[test]
    fn command_inspector_small_window_is_bounded_and_execution_is_explicit() {
        use egui_kittest::{Harness, kittest::Queryable};
        let ctx=egui::Context::default();
        let mut conn=Conn::offline(ctx);
        conn.telemetry.insert(1,kiln_proto::SessionTelemetry { shell_integration:true, commands:vec![kiln_proto::CommandRecord {id:7,command:"echo original".into(),cwd:Some("/tmp".into()),exit_code:Some(0),finished_unix:Some(1),output_available:true,..Default::default()}],..Default::default() });
        conn.command_outputs.insert((1,7),("original".into(),false));
        let mut view=TermView::new(1); view.inspector=true;view.inspected_command=Some(7);view.command_draft="echo edited".into();
        let mut installed=false;
        let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).build_ui_state(|ui,state:&mut (TermView,Conn,TermOutput)| {
            if !installed { kiln_common::fonts::install(ui.ctx()); kiln_common::Theme::current().apply(ui.ctx()); installed=true;return; }
            state.0.inspector_ui(ui,&mut state.1,None,&mut state.2);
        },(view,conn,TermOutput::default()));
        h.run_steps(3);
        assert!(h.state().2.command_to_run.is_none());
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("← 터미널").rect()));
        h.render().unwrap().save("/tmp/kiln-command-inspector-small.png").unwrap();
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("새 터미널에서 실행").rect()));
        // The execution action remains visible outside the scrollable editor.
        h.get_by_label("새 터미널에서 실행").click();
        h.run_steps(2);
        assert_eq!(h.state().2.command_to_run,Some(("echo edited".into(),Some("/tmp".into()))));
    }

    #[test]
    fn inline_history_keeps_other_panels_interactive_and_selection_does_not_execute() {
        use egui_kittest::{Harness,kittest::Queryable};
        let mut conn=Conn::offline(egui::Context::default());
        conn.telemetry.insert(1,kiln_proto::SessionTelemetry {shell_integration:true,commands:(1..=2).map(|id|kiln_proto::CommandRecord{id,command:format!("echo {id}"),exit_code:Some(0),finished_unix:Some(1),output_available:true,..Default::default()}).collect(),..Default::default()});
        let mut view=TermView::new(1);view.inspector=true;view.inspected_command=Some(2);view.command_draft="echo 2".into();
        let mut initialized=false;
        let mut h=Harness::builder().with_size([1000.0,600.0]).build_ui_state(|ui,s:&mut (TermView,Conn,TermOutput,bool)| {
            if !initialized {kiln_common::fonts::install(ui.ctx());initialized=true;return;}
            ui.columns(2,|cols|{s.0.inspector_ui(&mut cols[0],&mut s.1,None,&mut s.2);if cols[1].button("다른 패널 작업").clicked(){s.3=true;}});
        },(view,conn,TermOutput::default(),false));
        h.run_steps(3);h.get_by_label("이전 명령").click();h.run_steps(2);
        assert_eq!(h.state().0.command_draft,"echo 1");assert!(h.state().2.command_to_run.is_none());
        h.get_by_label("다른 패널 작업").click();h.run_steps(2);assert!(h.state().3);
        h.get_by_label("← 터미널").click();h.run_steps(2);assert!(!h.state().0.inspector);
    }

    #[test]
    fn header_history_action_leaves_setup_and_keeps_command_draft() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut view = TermView::new(1);
        view.open_integration_help();
        view.inspected_command = Some(7);
        view.command_draft = "echo edited".into();
        let mut conn = Conn::offline(egui::Context::default());
        conn.telemetry.insert(1, kiln_proto::SessionTelemetry {
            shell_integration: true,
            commands: vec![kiln_proto::CommandRecord {
                id: 7, command: "echo original".into(), exit_code: Some(0),
                finished_unix: Some(1), output_available: true, ..Default::default()
            }], ..Default::default()
        });
        let mut initialized = false;
        let mut h = Harness::builder().with_size([720.0, 600.0]).build_ui_state(|ui, s: &mut (TermView, Conn, TermOutput)| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                kiln_common::Theme::current().apply(ui.ctx());
                initialized = true;
                return;
            }
            if kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::History, 28.0, s.0.inspector_open(), "명령 기록 열기").clicked() {
                s.0.open_history();
            }
            s.0.inspector_ui(ui, &mut s.1, None, &mut s.2);
        }, (view, conn, TermOutput::default()));
        h.run_steps(3);
        assert!(h.query_by_label("zsh 연결 명령 복사").is_some());
        h.get_by_label("명령 기록 열기").click();
        h.run_steps(2);
        assert!(h.state().0.inspector_open());
        assert!(h.query_by_label("zsh 연결 명령 복사").is_none());
        assert_eq!(h.state().0.command_draft, "echo edited");
        assert_eq!(h.state().0.inspected_command, Some(7));
        assert!(h.state().2.command_to_run.is_none());
    }

    #[test]
    fn integration_help_exposes_status_and_copy_only_setup() {
        use egui_kittest::{Harness,kittest::Queryable};
        let mut view=TermView::new(1);view.inspector=true;view.integration_help=true;
        let mut initialized=false;
        let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).build_ui_state(|ui,s:&mut (TermView,Conn,TermOutput)|{
            if !initialized {kiln_common::fonts::install(ui.ctx());kiln_common::Theme::current().apply(ui.ctx());initialized=true;return;}
            s.0.inspector_ui(ui,&mut s.1,None,&mut s.2);
        },(view,Conn::offline(egui::Context::default()),TermOutput::default()));
        h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("← 터미널").rect()));
        h.get_by_label("zsh 연결 명령 복사").click();h.run_steps(2);
        assert!(h.state().2.command_to_run.is_none());
        h.render().unwrap().save("/tmp/kiln-terminal-integration-help-small.png").unwrap();
    }

    #[test]
    fn finds_url_under_column() {
        let t = "see https://example.com/a?b=1. ok";
        let (s, e, target) = find_link(t, 10, None).unwrap();
        assert_eq!((s, e), (4, 29));
        assert!(matches!(target, LinkTarget::Url(u) if u == "https://example.com/a?b=1"));
    }

    #[test]
    fn finds_existing_file_with_line_number() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main(){}").unwrap();
        let t = "error at src/main.rs:12:5 here";
        let (_, _, target) = find_link(t, 12, Some(dir.path().to_str().unwrap())).unwrap();
        match target {
            LinkTarget::File { path, line, col } => {
                assert!(path.ends_with("src/main.rs"));
                assert_eq!((line, col), (Some(12), Some(5)));
            }
            _ => panic!(),
        }
        assert!(find_link("missing/file.rs", 3, Some(dir.path().to_str().unwrap())).is_none());
    }

    #[test]
    fn word_bounds_stop_at_separators() {
        let line = Line { cells: "foo bar.baz(x)".chars().map(|c| Cell { c, ..Default::default() }).collect(), combining: vec![], links: vec![] };
        assert_eq!(word_bounds(&line, 5), (4, 10));
    }

    #[test]
    fn palette_cube_and_gray() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(p.indexed(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(p.indexed(232), Color32::from_rgb(8, 8, 8));
    }
}
