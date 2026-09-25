//! 줄 단위 상태 체크포인트를 둔 증분 구문 강조.
//!
//! 각 줄 시작의 파서·하이라이터 상태를 저장한다. 편집이 생기면 변경된 첫 줄부터 다시
//! 계산하고, 변경 구간을 지난 뒤 계산한 상태가 기존에 저장된 상태와 같아지면 멈춘다.

use std::time::Instant;

use egui::Color32;
use syntect::highlighting::{FontStyle, HighlightIterator, HighlightState};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference};

use crate::buffer::LineEdit;
use crate::syntax;

/// 강조 구간. `start..end` 는 줄 안의 바이트 범위.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub color: Color32,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct LineState {
    parse: ParseState,
    hl: HighlightState,
}

/// 이 길이보다 긴 줄은 강조하지 않고 상태를 그대로 넘긴다.
const MAX_HIGHLIGHT_LINE: usize = 20_000;

pub struct Highlighter {
    syntax: Option<&'static SyntaxReference>,
    /// `starts[i]` = i번째 줄 시작 상태.
    starts: Vec<Option<Box<LineState>>>,
    spans: Vec<Vec<Span>>,
    /// `[0, valid)` 줄은 확정.
    valid: usize,
    /// `[valid, dirty_end)` 줄은 반드시 다시 계산해야 한다.
    dirty_end: usize,
    /// `[dirty_end, data_end)` 줄은 이전 계산 결과가 남아 있어 수렴 비교에 쓰인다.
    data_end: usize,
    scratch: String,
    /// 캐시를 계산할 때 쓴 앱 테마 이름. 테마가 바뀌면 전체를 다시 계산한다.
    theme: &'static str,
    /// 마지막 `ensure` 호출에서 실제로 계산한 줄 수.
    pub last_work: usize,
}

impl Highlighter {
    pub fn new(syntax: Option<&'static SyntaxReference>, line_count: usize) -> Self {
        let mut h = Self {
            syntax,
            starts: Vec::new(),
            spans: Vec::new(),
            valid: 0,
            dirty_end: 0,
            data_end: 0,
            scratch: String::new(),
            theme: kiln_common::Theme::current().name,
            last_work: 0,
        };
        h.reset(line_count);
        h
    }

    pub fn syntax(&self) -> Option<&'static SyntaxReference> {
        self.syntax
    }

    pub fn is_plain(&self) -> bool {
        self.syntax.is_none()
    }

    /// 문법을 바꾸고 전체를 무효화한다.
    pub fn set_syntax(&mut self, syntax: Option<&'static SyntaxReference>, line_count: usize) {
        self.syntax = syntax;
        self.reset(line_count);
    }

    /// 모든 캐시를 비운다.
    pub fn reset(&mut self, line_count: usize) {
        self.theme = kiln_common::Theme::current().name;
        self.starts = vec![None; line_count.max(1)];
        self.spans = vec![Vec::new(); line_count.max(1)];
        if let Some(s) = self.syntax {
            let hl = HighlightState::new(syntax::highlighter(), ScopeStack::new());
            self.starts[0] = Some(Box::new(LineState { parse: ParseState::new(s), hl }));
        }
        self.valid = 0;
        self.dirty_end = 0;
        self.data_end = 0;
    }

    /// 확정된 줄 수.
    pub fn valid_lines(&self) -> usize {
        self.valid
    }

    /// 줄 변경을 반영한다.
    pub fn on_edit(&mut self, e: LineEdit) {
        let LineEdit { start, old_count, new_count } = e;
        let delta = new_count as isize - old_count as isize;
        let shift = |x: usize| (x as isize + delta) as usize;
        let old_end = start + old_count;

        self.spans.splice(start..old_end, std::iter::repeat_n(Vec::new(), new_count));
        self.starts
            .splice(start + 1..old_end, std::iter::repeat_n(None, new_count - 1));

        let had_dirty = self.dirty_end > self.valid;
        self.dirty_end = if had_dirty && self.dirty_end > old_end {
            shift(self.dirty_end)
        } else {
            start + new_count
        };
        self.data_end = if self.data_end > old_end { shift(self.data_end) } else { self.data_end.min(start) };
        self.valid = self.valid.min(start);
        self.data_end = self.data_end.max(self.dirty_end).min(self.spans.len());
        self.dirty_end = self.dirty_end.min(self.spans.len());
    }

    /// `upto` 줄(미포함)까지 강조를 계산한다. 시간 예산을 넘기면 멈추고 `false` 를 돌려준다.
    pub fn ensure<S: AsRef<str>>(&mut self, lines: &[S], upto: usize, deadline: Option<Instant>) -> bool {
        self.last_work = 0;
        if self.syntax.is_some() && self.theme != kiln_common::Theme::current().name {
            self.reset(lines.len());
        }
        let upto = upto.min(lines.len());
        let Some(_) = self.syntax else {
            self.valid = lines.len();
            return true;
        };
        debug_assert_eq!(lines.len(), self.spans.len());
        let hl = syntax::highlighter();
        let set = &syntax::assets().set;
        while self.valid < upto {
            let i = self.valid;
            let mut st = self.starts[i].as_deref().cloned().expect("start state of first invalid line");
            let line = lines[i].as_ref();
            let mut out = Vec::new();
            if line.len() <= MAX_HIGHLIGHT_LINE {
                self.scratch.clear();
                self.scratch.push_str(line);
                self.scratch.push('\n');
                if let Ok(ops) = st.parse.parse_line(&self.scratch, set) {
                    let mut pos = 0u32;
                    for (style, piece) in HighlightIterator::new(&mut st.hl, &ops, &self.scratch, hl) {
                        let len = piece.len() as u32;
                        let end = (pos + len).min(line.len() as u32);
                        if end > pos {
                            let color = Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                            let italic = style.font_style.contains(FontStyle::ITALIC);
                            let underline = style.font_style.contains(FontStyle::UNDERLINE);
                            match out.last_mut() {
                                Some(Span { end: e, color: c, italic: it, underline: u, .. })
                                    if *e == pos && *c == color && *it == italic && *u == underline =>
                                {
                                    *e = end
                                }
                                _ => out.push(Span { start: pos, end, color, italic, underline }),
                            }
                        }
                        pos += len;
                    }
                }
            }
            self.spans[i] = out;
            self.last_work += 1;
            let next = i + 1;
            if next < self.starts.len() {
                let converged = next >= self.dirty_end
                    && next < self.data_end
                    && self.starts[next].as_deref() == Some(&st);
                if converged {
                    self.valid = self.data_end;
                    self.dirty_end = self.data_end;
                    continue;
                }
                self.starts[next] = Some(Box::new(st));
            }
            self.valid = next;
            self.dirty_end = self.dirty_end.max(self.valid);
            self.data_end = self.data_end.max(self.valid);
            if let Some(d) = deadline
                && self.last_work.is_multiple_of(16)
                && Instant::now() >= d
            {
                return self.valid >= upto;
            }
        }
        true
    }

    /// 확정된 줄의 강조 구간. 아직 계산되지 않았으면 `None`.
    pub fn line_spans(&self, line: usize) -> Option<&[Span]> {
        if self.syntax.is_none() || line >= self.valid {
            return None;
        }
        self.spans.get(line).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Pos};
    use std::path::Path;

    fn rust() -> Option<&'static SyntaxReference> {
        syntax::detect(Path::new("x.rs"), "")
    }

    fn full(b: &Buffer) -> Vec<Vec<Span>> {
        let mut h = Highlighter::new(rust(), b.line_count());
        h.ensure(b.lines(), b.line_count(), None);
        (0..b.line_count()).map(|i| h.line_spans(i).unwrap().to_vec()).collect()
    }

    fn incremental(h: &mut Highlighter, b: &Buffer) -> Vec<Vec<Span>> {
        h.ensure(b.lines(), b.line_count(), None);
        (0..b.line_count()).map(|i| h.line_spans(i).unwrap().to_vec()).collect()
    }

    const SRC: &str = "/// doc\nfn main() {\n    let s = \"hi\";\n    // c\n    let n = 42;\n}\n\nstruct A { x: u32 }\n";

    #[test]
    fn incremental_matches_full_rehighlight_after_edits() {
        let mut b = Buffer::from_text(&SRC.repeat(20));
        let mut h = Highlighter::new(rust(), b.line_count());
        h.ensure(b.lines(), b.line_count(), None);
        // 문자열/주석을 열어 뒤쪽 전체 상태를 바꾸는 편집과 되돌리는 편집을 섞는다.
        let edits: &[(Pos, Pos, &str)] = &[
            (Pos::new(2, 12), Pos::new(2, 12), "\""),
            (Pos::new(2, 12), Pos::new(2, 13), ""),
            (Pos::new(0, 0), Pos::new(0, 0), "/*"),
            (Pos::new(30, 0), Pos::new(30, 0), "*/\n"),
            (Pos::new(5, 0), Pos::new(9, 0), ""),
            (Pos::new(10, 4), Pos::new(10, 4), "let q = 1;\nlet r = 2;\n"),
            (Pos::new(0, 0), Pos::new(0, 2), ""),
        ];
        for &(a, e, t) in edits {
            let (a, e) = (b.clamp(a), b.clamp(e));
            b.replace(a, e, t);
            for c in b.take_changes() {
                h.on_edit(c);
            }
            assert_eq!(incremental(&mut h, &b), full(&b), "after inserting {t:?} at {a:?}");
        }
    }

    #[test]
    fn edits_before_catching_up_still_match_full() {
        let mut b = Buffer::from_text(&SRC.repeat(10));
        let mut h = Highlighter::new(rust(), b.line_count());
        h.ensure(b.lines(), 30, None);
        b.insert(Pos::new(3, 0), "\"");
        b.insert(Pos::new(50, 0), "/* x\n");
        b.replace(Pos::new(10, 0), Pos::new(12, 0), "");
        for c in b.take_changes() {
            h.on_edit(c);
        }
        h.ensure(b.lines(), 5, None);
        b.insert(Pos::new(3, 0), "x");
        for c in b.take_changes() {
            h.on_edit(c);
        }
        assert_eq!(incremental(&mut h, &b), full(&b));
    }

    #[test]
    fn local_edit_converges_quickly() {
        let mut b = Buffer::from_text(&SRC.repeat(200));
        let mut h = Highlighter::new(rust(), b.line_count());
        h.ensure(b.lines(), b.line_count(), None);
        b.insert(Pos::new(800, 4), "x");
        for c in b.take_changes() {
            h.on_edit(c);
        }
        h.ensure(b.lines(), b.line_count(), None);
        assert!(h.last_work <= 2, "recomputed {} lines", h.last_work);
    }

    #[test]
    fn keyword_and_string_get_distinct_colors() {
        let b = Buffer::from_text("fn x() { \"s\" }");
        let spans = &full(&b)[0];
        let color_at = |col: u32| spans.iter().find(|s| s.start <= col && col < s.end).map(|s| s.color);
        assert_ne!(color_at(0), color_at(9));
        assert_ne!(color_at(0), color_at(3));
    }
}
