//! alacritty_terminal 기반 에뮬레이터 래퍼. 화면을 프로토콜 `Line` 으로 변환하고
//! 업그레이드용 덤프/복원을 제공한다.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid, Scroll};
use alacritty_terminal::index::{Column, Line as GLine, Point};
use alacritty_terminal::term::cell::{Cell as TCell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as TColor, CursorShape as TShape, NamedColor, Processor};
use kiln_proto::{Cell, Color, Cursor, CursorShape, Line, ScrollTo, flags, mode};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const HISTORY: usize = 10_000;

#[derive(Clone, Default)]
pub struct Proxy(Arc<Mutex<Vec<Event>>>);

impl EventListener for Proxy {
    fn send_event(&self, e: Event) {
        self.0.lock().push(e);
    }
}

#[derive(Clone, Copy)]
struct Dims {
    cols: usize,
    rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Emu {
    term: Term<Proxy>,
    parser: Processor,
    proxy: Proxy,
}

fn config() -> Config {
    Config { scrolling_history: HISTORY, ..Default::default() }
}

impl Emu {
    pub fn new(cols: u16, rows: u16) -> Self {
        let proxy = Proxy::default();
        let d = Dims { cols: cols.max(2) as usize, rows: rows.max(1) as usize };
        let term = Term::new(config(), &d, proxy.clone());
        Emu { term, parser: Processor::new(), proxy }
    }

    /// 바이트를 처리하고 발생한 이벤트를 돌려준다.
    pub fn advance(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.parser.advance(&mut self.term, bytes);
        std::mem::take(&mut *self.proxy.0.lock())
    }

    pub fn size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let d = Dims { cols: cols.max(2) as usize, rows: rows.max(1) as usize };
        if (d.cols, d.rows) != (self.term.columns(), self.term.screen_lines()) {
            self.term.resize(d);
        }
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn scroll(&mut self, s: ScrollTo) {
        let s = match s {
            ScrollTo::Delta(d) => Scroll::Delta(d),
            ScrollTo::PageUp => Scroll::PageUp,
            ScrollTo::PageDown => Scroll::PageDown,
            ScrollTo::Top => Scroll::Top,
            ScrollTo::Bottom => Scroll::Bottom,
        };
        self.term.scroll_display(s);
    }

    /// 화면의 `row` 번째 줄(스크롤 오프셋 반영).
    pub fn visible_line(&self, row: usize) -> Line {
        let grid = self.term.grid();
        let line = GLine(row as i32 - grid.display_offset() as i32);
        convert_row(grid, line, self.term.columns())
    }

    pub fn cursor(&self) -> Option<Cursor> {
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let grid = self.term.grid();
        let p = grid.cursor.point;
        let row = p.line.0 + grid.display_offset() as i32;
        if row < 0 || row >= self.term.screen_lines() as i32 {
            return None;
        }
        let shape = match self.term.cursor_style().shape {
            TShape::Block | TShape::HollowBlock => CursorShape::Block,
            TShape::Underline => CursorShape::Underline,
            TShape::Beam => CursorShape::Beam,
            TShape::Hidden => return None,
        };
        Some(Cursor { col: p.column.0 as u16, row: row as u16, shape })
    }

    pub fn mode_bits(&self) -> u32 {
        let m = self.term.mode();
        let mut b = 0;
        let map = [
            (TermMode::APP_CURSOR, mode::APP_CURSOR),
            (TermMode::APP_KEYPAD, mode::APP_KEYPAD),
            (TermMode::MOUSE_REPORT_CLICK, mode::MOUSE_CLICK),
            (TermMode::MOUSE_DRAG, mode::MOUSE_DRAG),
            (TermMode::MOUSE_MOTION, mode::MOUSE_MOTION),
            (TermMode::SGR_MOUSE, mode::SGR_MOUSE),
            (TermMode::BRACKETED_PASTE, mode::BRACKETED_PASTE),
            (TermMode::FOCUS_IN_OUT, mode::FOCUS_EVENTS),
            (TermMode::ALT_SCREEN, mode::ALT_SCREEN),
            (TermMode::ALTERNATE_SCROLL, mode::ALT_SCROLL),
            (TermMode::UTF8_MOUSE, mode::UTF8_MOUSE),
        ];
        for (t, p) in map {
            if m.contains(t) {
                b |= p;
            }
        }
        if m.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
            b |= mode::KITTY_KEYBOARD;
        }
        b
    }

    /// 스크롤백 마지막 `history` 줄과 화면을 평문으로 돌려준다.
    pub fn text(&self, history: usize) -> String {
        let grid = self.term.grid();
        let top = -(grid.history_size().min(history) as i32);
        let bottom = self.term.screen_lines() as i32;
        let mut out = String::new();
        for l in top..bottom {
            let line = convert_row(grid, GLine(l), self.term.columns());
            let wrapped = line.cells.last().is_some_and(|c| c.flags & flags::WRAP != 0);
            let t = line.text();
            if wrapped {
                out.push_str(&t);
            } else {
                out.push_str(t.trim_end());
                out.push('\n');
            }
        }
        while out.ends_with("\n\n") {
            out.pop();
        }
        out
    }

    /// 현재 보이는 영역 위쪽(또는 아래쪽)에서 `query` 를 찾아 화면에 보이도록 스크롤한다.
    pub fn search(&mut self, query: &str, backward: bool) -> bool {
        if query.is_empty() {
            return false;
        }
        let q = query.to_lowercase();
        let grid = self.term.grid();
        let rows = self.term.screen_lines() as i32;
        let offset = grid.display_offset() as i32;
        let top = -(grid.history_size() as i32);
        let cols = self.term.columns();
        let view_top = -offset;
        let found = if backward {
            (top..view_top).rev().find(|&l| convert_row(grid, GLine(l), cols).text().to_lowercase().contains(&q))
        } else {
            (view_top + rows..rows).find(|&l| convert_row(grid, GLine(l), cols).text().to_lowercase().contains(&q))
        };
        match found {
            Some(l) => {
                // 매치 줄이 화면 가운데쯤 오도록 오프셋을 정한다.
                let target = (-(l) + rows / 2).clamp(0, grid.history_size() as i32);
                let delta = target - offset;
                self.term.scroll_display(Scroll::Delta(delta));
                true
            }
            None => false,
        }
    }

    pub fn dump(&mut self) -> Dump {
        let (cols, rows) = self.size();
        let mode = self.term.mode().bits();
        let alt_active = self.term.mode().contains(TermMode::ALT_SCREEN);
        let mut alt = None;
        let mut alt_cursor = (0, 0);
        if alt_active {
            alt = Some(self.term.grid().clone());
            let p = self.term.grid().cursor.point;
            alt_cursor = (p.line.0, p.column.0);
            self.term.swap_alt();
        }
        let p = self.term.grid().cursor.point;
        let primary = self.term.grid().clone();
        let dump = Dump { cols, rows, mode, primary, primary_cursor: (p.line.0, p.column.0), alt, alt_cursor };
        if alt_active {
            self.term.swap_alt();
        }
        dump
    }

    pub fn restore(d: Dump) -> Emu {
        let mut emu = Emu::new(d.cols, d.rows);
        *emu.term.grid_mut() = d.primary;
        emu.term.grid_mut().cursor.point = Point::new(GLine(d.primary_cursor.0), Column(d.primary_cursor.1));
        let saved = TermMode::from_bits_truncate(d.mode);
        if let Some(alt) = d.alt {
            emu.advance(b"\x1b[?1049h");
            *emu.term.grid_mut() = alt;
            emu.term.grid_mut().cursor.point = Point::new(GLine(d.alt_cursor.0), Column(d.alt_cursor.1));
        }
        emu.advance(mode_sequences(saved).as_bytes());
        emu
    }
}

fn mode_sequences(m: TermMode) -> String {
    let mut s = String::new();
    let decset = [
        (TermMode::APP_CURSOR, 1),
        (TermMode::MOUSE_REPORT_CLICK, 1000),
        (TermMode::MOUSE_DRAG, 1002),
        (TermMode::MOUSE_MOTION, 1003),
        (TermMode::FOCUS_IN_OUT, 1004),
        (TermMode::UTF8_MOUSE, 1005),
        (TermMode::SGR_MOUSE, 1006),
        (TermMode::ALTERNATE_SCROLL, 1007),
        (TermMode::BRACKETED_PASTE, 2004),
    ];
    for (flag, n) in decset {
        if m.contains(flag) {
            s.push_str(&format!("\x1b[?{n}h"));
        }
    }
    if m.contains(TermMode::APP_KEYPAD) {
        s.push_str("\x1b=");
    }
    if !m.contains(TermMode::SHOW_CURSOR) {
        s.push_str("\x1b[?25l");
    }
    if !m.contains(TermMode::LINE_WRAP) {
        s.push_str("\x1b[?7l");
    }
    if m.contains(TermMode::INSERT) {
        s.push_str("\x1b[4h");
    }
    let kitty = [
        (TermMode::DISAMBIGUATE_ESC_CODES, 1),
        (TermMode::REPORT_EVENT_TYPES, 2),
        (TermMode::REPORT_ALTERNATE_KEYS, 4),
        (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
        (TermMode::REPORT_ASSOCIATED_TEXT, 16),
    ];
    let k: u32 = kitty.iter().filter(|(f, _)| m.contains(*f)).map(|(_, n)| n).sum();
    if k != 0 {
        s.push_str(&format!("\x1b[>{k}u"));
    }
    s
}

#[derive(Serialize, Deserialize)]
pub struct Dump {
    pub cols: u16,
    pub rows: u16,
    pub mode: u32,
    pub primary: Grid<TCell>,
    pub primary_cursor: (i32, usize),
    pub alt: Option<Grid<TCell>>,
    pub alt_cursor: (i32, usize),
}

fn convert_color(c: TColor) -> Color {
    match c {
        TColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        TColor::Indexed(i) => Color::Idx(i),
        TColor::Named(n) => {
            let v = n as usize;
            if v < 16 {
                Color::Idx(v as u8)
            } else {
                match n {
                    NamedColor::Background => Color::DefaultBg,
                    NamedColor::DimBlack
                    | NamedColor::DimRed
                    | NamedColor::DimGreen
                    | NamedColor::DimYellow
                    | NamedColor::DimBlue
                    | NamedColor::DimMagenta
                    | NamedColor::DimCyan
                    | NamedColor::DimWhite => Color::Idx((v - NamedColor::DimBlack as usize) as u8),
                    _ => Color::DefaultFg,
                }
            }
        }
    }
}

fn convert_flags(f: Flags) -> u16 {
    let mut o = 0;
    if f.contains(Flags::BOLD) {
        o |= flags::BOLD;
    }
    if f.contains(Flags::ITALIC) {
        o |= flags::ITALIC;
    }
    if f.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE) {
        o |= flags::UNDERLINE;
    }
    if f.contains(Flags::UNDERCURL) {
        o |= flags::UNDERCURL;
    }
    if f.contains(Flags::INVERSE) {
        o |= flags::INVERSE;
    }
    if f.contains(Flags::DIM) {
        o |= flags::DIM;
    }
    if f.contains(Flags::HIDDEN) {
        o |= flags::HIDDEN;
    }
    if f.contains(Flags::STRIKEOUT) {
        o |= flags::STRIKE;
    }
    if f.contains(Flags::WIDE_CHAR) {
        o |= flags::WIDE;
    }
    if f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        o |= flags::SPACER;
    }
    if f.contains(Flags::WRAPLINE) {
        o |= flags::WRAP;
    }
    o
}

fn convert_row(grid: &Grid<TCell>, line: GLine, cols: usize) -> Line {
    let row = &grid[line];
    let mut cells = Vec::with_capacity(cols);
    let mut combining = Vec::new();
    for c in 0..cols {
        let tc = &row[Column(c)];
        cells.push(Cell { c: tc.c, fg: convert_color(tc.fg), bg: convert_color(tc.bg), flags: convert_flags(tc.flags) });
        if let Some(z) = tc.zerowidth()
            && !z.is_empty() {
                combining.push((c as u16, z.iter().collect()));
            }
    }
    Line { cells, combining }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_and_colors() {
        let mut e = Emu::new(20, 3);
        e.advance(b"\x1b[31mred\x1b[0m \xed\x95\x9c\xea\xb8\x80");
        let l = e.visible_line(0);
        assert_eq!(l.cells[0].fg, Color::Idx(1));
        assert!(l.text().starts_with("red 한글"));
        assert_ne!(l.cells[4].flags & flags::WIDE, 0);
        assert_ne!(l.cells[5].flags & flags::SPACER, 0);
        assert_eq!(e.cursor().unwrap().col, 8);
    }

    #[test]
    fn dump_restore_preserves_screen_history_and_modes() {
        let mut e = Emu::new(30, 5);
        for i in 0..40 {
            e.advance(format!("line {i}\r\n").as_bytes());
        }
        e.advance(b"\x1b[?2004h\x1b[?1h");
        let before = e.text(100);
        let dump = e.dump();
        let bytes = postcard::to_stdvec(&dump).unwrap();
        let back: Dump = postcard::from_bytes(&bytes).unwrap();
        let r = Emu::restore(back);
        assert_eq!(r.text(100), before);
        assert_ne!(r.mode_bits() & mode::BRACKETED_PASTE, 0);
        assert_ne!(r.mode_bits() & mode::APP_CURSOR, 0);
        assert_eq!(r.cursor().map(|c| (c.row, c.col)), e.cursor().map(|c| (c.row, c.col)));
    }

    #[test]
    fn dump_restore_keeps_alt_screen_and_primary() {
        let mut e = Emu::new(20, 4);
        e.advance(b"primary\r\n");
        e.advance(b"\x1b[?1049h\x1b[Halt-ui");
        let dump = e.dump();
        let mut r = Emu::restore(dump);
        assert_ne!(r.mode_bits() & mode::ALT_SCREEN, 0);
        assert!(r.visible_line(0).text().starts_with("alt-ui"));
        r.advance(b"\x1b[?1049l");
        assert!(r.visible_line(0).text().starts_with("primary"));
    }

    #[test]
    fn search_scrolls_to_match_in_history() {
        let mut e = Emu::new(20, 5);
        for i in 0..100 {
            e.advance(format!("row {i}\r\n").as_bytes());
        }
        assert!(e.search("row 10", true));
        assert!(e.display_offset() > 0);
        let visible: Vec<String> = (0..5).map(|r| e.visible_line(r).text()).collect();
        assert!(visible.iter().any(|t| t.contains("row 10")), "{visible:?}");
    }
}
