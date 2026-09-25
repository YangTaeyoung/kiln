//! 터미널 화면 위젯: 렌더링, 키보드/IME/마우스 입력, 선택, 링크.

use super::conn::{Conn, Screen};
use super::keys;
use egui::{Color32, FontId, Modifiers, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};
use kiln_proto::{Cell, Color, CursorShape, Line, ScrollTo, SessionId, flags, mode};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Palette {
    pub fg: Color32,
    pub bg: Color32,
    pub ansi: [Color32; 16],
    pub cursor: Color32,
    pub selection: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        let h = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        Palette {
            fg: h(0xdcdee6),
            bg: h(0x16171c),
            ansi: [
                h(0x3b3f4c), h(0xe06c75), h(0x98c379), h(0xe5c07b), h(0x61afef), h(0xc678dd), h(0x56b6c2), h(0xabb2bf),
                h(0x5c6370), h(0xff7a85), h(0xb5e890), h(0xffd68a), h(0x7cc4ff), h(0xde8fff), h(0x6fd3df), h(0xf0f2f6),
            ],
            cursor: h(0xdfe3ec),
            selection: Color32::from_rgba_unmultiplied(0x6c, 0x9e, 0xff, 0x55),
        }
    }
}

impl Palette {
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
}

#[derive(Clone, Copy)]
pub struct TermSettings {
    pub font_size: f32,
    pub option_as_meta: bool,
    pub line_height: f32,
}

impl Default for TermSettings {
    fn default() -> Self {
        TermSettings { font_size: 13.5, option_as_meta: true, line_height: 1.2 }
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
    metrics: Option<(f32, Vec2)>,
    selection: Option<Selection>,
    selecting: bool,
    preedit: String,
    scroll_accum: f32,
    last_mouse_cell: Option<(u16, u16)>,
    search: Option<SearchBar>,
    pub palette: Arc<Palette>,
    pending_size: Option<((u16, u16), std::time::Instant)>,
}

impl TermView {
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
            search: None,
            palette: Arc::new(Palette::default()),
            pending_size: None,
        }
    }

    pub fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(SearchBar { query: String::new(), focus: true, last_found: None });
        } else if let Some(s) = &mut self.search {
            s.focus = true;
        }
    }

    pub fn set_search_result(&mut self, found: bool) {
        if let Some(s) = &mut self.search {
            s.last_found = Some(found);
        }
    }

    fn cell_size(&mut self, ctx: &egui::Context, s: &TermSettings) -> Vec2 {
        if let Some((fs, v)) = self.metrics
            && fs == s.font_size * s.line_height {
                return v;
            }
        let font = FontId::monospace(s.font_size);
        let (w, h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let v = vec2(w, (h * s.line_height).round());
        self.metrics = Some((s.font_size * s.line_height, v));
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
        let rect = ui.available_rect_before_wrap();
        let id = ui.id().with(("term", self.session));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        ui.advance_cursor_after_rect(rect);
        if focus_request || resp.clicked() || resp.drag_started() {
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
        let pad = vec2(8.0, 4.0);
        let inner = rect.shrink2(pad);
        let cols = ((inner.width() / cell.x).floor() as u16).max(2);
        let rows = ((inner.height() / cell.y).floor() as u16).max(1);
        self.request_size(conn, cols, rows);

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, self.palette.bg);

        let screen_exists = conn.screens.contains_key(&self.session);
        if !screen_exists {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "연결 중…", FontId::proportional(13.0), Color32::GRAY);
            return out;
        }

        // 입력 처리 (화면 캐시를 빌리기 전에).
        let term_mode = conn.screens.get(&self.session).map(|s| s.mode).unwrap_or(0);
        let exited = conn.infos.get(&self.session).and_then(|i| i.exited);
        if focused && self.search.as_ref().is_none_or(|s| !s.focus) {
            self.handle_keyboard(ui, conn, term_mode, settings, exited, &mut out);
        }
        self.handle_mouse(ui, &resp, conn, inner.min, cell, term_mode, &mut out, cwd);

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
                let g = painter.layout_no_wrap(self.preedit.clone(), font.clone(), self.palette.fg);
                let r = Rect::from_min_size(pos, vec2(g.size().x.max(cell.x), cell.y));
                painter.rect_filled(r, 0.0, self.palette.bg);
                painter.galley(pos2(pos.x, pos.y + (cell.y - g.size().y) / 2.0), g, self.palette.fg);
                painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, self.palette.fg));
            } else if screen.display_offset == 0 {
                let color = self.palette.cursor;
                match (c.shape, focused) {
                    (CursorShape::Block, true) => {
                        painter.rect_filled(crect, 1.0, color);
                        if let Some(ch) = screen.lines.get(c.row as usize).and_then(|l| l.cells.get(c.col as usize))
                            && ch.c != ' ' {
                                let g = painter.layout_no_wrap(ch.c.to_string(), font.clone(), self.palette.bg);
                                painter.galley(pos2(pos.x + (w - g.size().x) / 2.0, pos.y + (cell.y - g.size().y) / 2.0), g, self.palette.bg);
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
            let total = (screen.history + screen.rows as u32) as f32;
            let h = (rect.height() * screen.rows as f32 / total).max(24.0);
            let top_frac = (screen.history - screen.display_offset) as f32 / total;
            let y = rect.top() + top_frac * (rect.height() - h) / (1.0 - screen.rows as f32 / total).max(0.01) * (1.0 - screen.rows as f32 / total);
            painter.rect_filled(Rect::from_min_size(pos2(rect.right() - 5.0, y), vec2(3.0, h)), 2.0, Color32::from_white_alpha(70));
            let label = format!("↑ {} 줄", screen.display_offset);
            let g = painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::from_gray(200));
            let br = Rect::from_min_size(pos2(rect.right() - g.size().x - 22.0, rect.top() + 6.0), g.size() + vec2(10.0, 4.0));
            painter.rect_filled(br, 4.0, Color32::from_black_alpha(160));
            painter.galley(br.min + vec2(5.0, 2.0), g, Color32::WHITE);
        }

        if let Some(code) = exited {
            let msg = format!("프로세스 종료 (코드 {code}) — Enter: 새 셸   ⌘W: 닫기");
            let g = painter.layout_no_wrap(msg, FontId::proportional(12.5), Color32::from_gray(230));
            let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 26.0), g.size() + vec2(24.0, 12.0));
            painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(40, 42, 52, 235));
            painter.galley(r.min + vec2(12.0, 6.0), g, Color32::WHITE);
        }

        self.search_ui(ui, conn, rect);
        out
    }

    fn request_size(&mut self, conn: &mut Conn, cols: u16, rows: u16) {
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
            conn_repaint(conn);
        }
    }

    fn handle_keyboard(&mut self, ui: &mut egui::Ui, conn: &mut Conn, term_mode: u32, s: &TermSettings, exited: Option<i32>, out: &mut TermOutput) {
        let events = ui.input(|i| i.events.clone());
        let mods = ui.input(|i| i.modifiers);
        let sid = self.session;
        let mut bytes: Vec<u8> = Vec::new();
        let is_mac = cfg!(target_os = "macos");
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    if mods.alt && s.option_as_meta && is_mac {
                        continue;
                    }
                    if mods.ctrl && !is_mac {
                        continue;
                    }
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
                egui::Event::WindowFocused(f) if term_mode & mode::FOCUS_EVENTS != 0 => {
                    bytes.extend_from_slice(if f { b"\x1b[I" } else { b"\x1b[O" });
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

        if mouse_mode {
            for ev in ui.input(|i| i.events.clone()) {
                match ev {
                    egui::Event::PointerButton { pos, button, pressed, modifiers } if resp.rect.contains(pos) || !pressed => {
                        let b = match button {
                            egui::PointerButton::Primary => 0,
                            egui::PointerButton::Middle => 1,
                            egui::PointerButton::Secondary => 2,
                            _ => continue,
                        };
                        if !resp.rect.contains(pos) && pressed {
                            continue;
                        }
                        let (c, r) = to_cell(pos);
                        conn.input(sid, keys::mouse_report(b, c, r, pressed, modifiers, term_mode));
                    }
                    egui::Event::PointerMoved(pos) if resp.rect.contains(pos) => {
                        let (c, r) = to_cell(pos);
                        if self.last_mouse_cell != Some((c, r)) {
                            self.last_mouse_cell = Some((c, r));
                            let down = ui.input(|i| i.pointer.primary_down());
                            if (down && term_mode & (mode::MOUSE_DRAG | mode::MOUSE_MOTION) != 0) || term_mode & mode::MOUSE_MOTION != 0 {
                                let btn = if down { 32 } else { 35 };
                                conn.input(sid, keys::mouse_report(btn, c, r, true, mods, term_mode));
                            }
                        }
                    }
                    _ => {}
                }
            }
            return;
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
                    painter.line_segment([pos2(origin.x + start as f32 * cell.x, y), pos2(origin.x + end as f32 * cell.x, y)], Stroke::new(1.0, Color32::from_rgb(0x6c, 0x9e, 0xff)));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        out.open = Some(target);
                    }
                }
            }
    }

    fn search_ui(&mut self, ui: &mut egui::Ui, conn: &mut Conn, rect: Rect) {
        let Some(sb) = &mut self.search else { return };
        let mut close = false;
        let area = egui::Area::new(ui.id().with(("term-search", self.session)))
            .fixed_pos(pos2(rect.right() - 330.0, rect.top() + 8.0))
            .order(egui::Order::Foreground);
        area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut sb.query).hint_text("스크롤백 검색").desired_width(180.0));
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
                    if ui.small_button("↑").on_hover_text("이전 (Enter)").clicked() {
                        go(true);
                    }
                    if ui.small_button("↓").on_hover_text("다음 (Shift+Enter)").clicked() {
                        go(false);
                    }
                    if sb.last_found == Some(false) {
                        ui.colored_label(Color32::from_rgb(0xf0, 0x6c, 0x75), "없음");
                    }
                    if ui.small_button("✕").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
            });
        });
        if close {
            self.search = None;
            conn.send(kiln_proto::ClientMsg::Scroll { session: self.session, scroll: ScrollTo::Bottom });
        }
    }
}

fn conn_repaint(conn: &Conn) {
    let _ = conn;
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

/// 한 줄을 셰이프로 만든다. 좌표는 줄 원점 기준.
fn build_row(ctx: &egui::Context, line: &Line, cell: Vec2, font: &FontId, pal: &Palette, fit: &mut HashMap<char, bool>) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let resolve = |c: &Cell| -> (Color32, Color32) {
        let mut fg = match c.fg {
            Color::Idx(i) if i < 8 && c.flags & flags::BOLD != 0 => pal.ansi[i as usize + 8],
            other => pal.resolve(other, true),
        };
        let mut bg = pal.resolve(c.bg, false);
        if c.flags & flags::INVERSE != 0 {
            std::mem::swap(&mut fg, &mut bg);
        }
        if c.flags & flags::DIM != 0 {
            fg = lerp(bg, fg, 0.6);
        }
        (fg, bg)
    };

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
            // 두 칸 문자는 칸 폭을 채우도록 최대 1.2배까지 키운다.
            if span == 2.0 && g.size().x > 0.0 && g.size().x < cell.x * 1.8 {
                let scale = (cell.x * 1.9 / g.size().x).min(1.2);
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
        let line = Line { cells: "foo bar.baz(x)".chars().map(|c| Cell { c, ..Default::default() }).collect(), combining: vec![] };
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
