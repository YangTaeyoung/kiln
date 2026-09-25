//! 버퍼 줄과 화면 줄(시각 줄) 사이의 대응: 자동 줄 바꿈 위치와 접힌 줄을 반영한다.
//!
//! 줄마다 줄 바꿈 위치(바이트)와 숨김 여부를 두고, 각 줄의 첫 시각 줄 번호를 누적 배열로 유지한다.
//! 편집이 생기면 바뀐 줄만 다시 나누고 누적 배열은 바뀐 줄부터 다시 더한다.

use super::char_cols;
use crate::buffer::{LineEdit, Pos};

#[derive(Clone, Debug)]
pub(crate) struct DisplayMap {
    /// 줄 바꿈 폭(열). 0 이면 줄 바꿈 없음.
    wrap_cols: usize,
    tab: usize,
    /// `breaks[i]` = i번째 줄의 두 번째 이후 시각 줄이 시작하는 바이트 위치.
    breaks: Vec<Box<[u32]>>,
    hidden: Vec<bool>,
    /// `prefix[i]` = i번째 줄의 첫 시각 줄 번호. 길이는 줄 수 + 1.
    prefix: Vec<u32>,
    /// 이 줄부터 `prefix` 를 다시 계산해야 한다.
    dirty_from: Option<usize>,
}

impl DisplayMap {
    pub fn new(line_count: usize) -> Self {
        let n = line_count.max(1);
        Self {
            wrap_cols: 0,
            tab: 4,
            breaks: vec![Box::default(); n],
            hidden: vec![false; n],
            prefix: (0..=n as u32).collect(),
            dirty_from: None,
        }
    }

    pub fn wrap_cols(&self) -> usize {
        self.wrap_cols
    }

    /// 줄의 `sub` 번째 시각 줄 들여쓰기(열).
    pub fn row_indent(&self, line: &str, sub: usize) -> usize {
        if sub == 0 || self.wrap_cols == 0 { 0 } else { wrap_indent_cols(line, self.wrap_cols, self.tab) }
    }

    /// 줄 바꿈 폭을 바꾼다. 바뀌었으면 모든 줄을 다시 나눈다.
    pub fn set_wrap<S: AsRef<str>>(&mut self, lines: &[S], cols: usize, tab: usize) -> bool {
        if cols == self.wrap_cols && tab == self.tab {
            return false;
        }
        self.wrap_cols = cols;
        self.tab = tab;
        self.breaks.clear();
        self.breaks.extend(lines.iter().map(|l| compute_breaks(l.as_ref(), cols, tab)));
        self.mark_dirty(0);
        true
    }

    /// 숨길 줄 범위(양 끝 포함) 목록으로 숨김 상태를 다시 만든다.
    pub fn set_hidden(&mut self, ranges: &[(usize, usize)]) {
        self.hidden.iter_mut().for_each(|h| *h = false);
        let n = self.hidden.len();
        for &(a, b) in ranges {
            for h in &mut self.hidden[a.min(n)..(b + 1).min(n)] {
                *h = true;
            }
        }
        self.mark_dirty(0);
    }

    /// 줄 편집을 반영한다. 새 줄들은 숨기지 않은 상태로 들어온다.
    pub fn on_edit<S: AsRef<str>>(&mut self, e: LineEdit, lines: &[S]) {
        let LineEdit { start, old_count, new_count } = e;
        let (cols, tab) = (self.wrap_cols, self.tab);
        let fresh = lines[start..start + new_count].iter().map(|l| compute_breaks(l.as_ref(), cols, tab));
        self.breaks.splice(start..start + old_count, fresh);
        let keep_hidden = old_count == new_count && self.hidden[start..start + old_count].iter().all(|h| !h);
        if !keep_hidden {
            self.hidden.splice(start..start + old_count, std::iter::repeat_n(false, new_count));
        }
        let n = self.breaks.len();
        self.prefix.resize(n + 1, 0);
        self.mark_dirty(start);
    }

    fn mark_dirty(&mut self, from: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(from, |d| d.min(from)));
    }

    fn ensure(&mut self) {
        let Some(from) = self.dirty_from.take() else { return };
        let n = self.breaks.len();
        self.prefix.resize(n + 1, 0);
        let mut acc = if from == 0 { 0 } else { self.prefix[from] };
        for i in from..n {
            self.prefix[i] = acc;
            acc += self.rows_of(i) as u32;
        }
        self.prefix[n] = acc;
    }

    fn rows_of(&self, i: usize) -> usize {
        if self.hidden[i] { 0 } else { self.breaks[i].len() + 1 }
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden.get(line).copied().unwrap_or(false)
    }

    /// 줄이 차지하는 시각 줄 수. 숨긴 줄은 0.
    pub fn rows(&self, line: usize) -> usize {
        self.rows_of(line)
    }

    pub fn total_rows(&mut self) -> usize {
        self.ensure();
        self.prefix[self.breaks.len()] as usize
    }

    /// 줄의 첫 시각 줄 번호.
    pub fn line_row(&mut self, line: usize) -> usize {
        self.ensure();
        self.prefix[line.min(self.breaks.len())] as usize
    }

    /// 시각 줄 번호 → (줄, 줄 안 몇 번째 시각 줄).
    pub fn row_to_line(&mut self, row: usize) -> (usize, usize) {
        self.ensure();
        let n = self.breaks.len();
        let total = self.prefix[n] as usize;
        if total == 0 {
            return (0, 0);
        }
        let row = row.min(total - 1) as u32;
        let line = self.prefix[..n].partition_point(|&p| p <= row) - 1;
        (line, (row - self.prefix[line]) as usize)
    }

    /// 줄의 `sub` 번째 시각 줄이 차지하는 바이트 범위.
    pub fn segment(&self, line: usize, sub: usize, line_len: usize) -> (usize, usize) {
        let br = &self.breaks[line];
        let a = if sub == 0 { 0 } else { br.get(sub - 1).map_or(line_len, |&b| b as usize) };
        let b = br.get(sub).map_or(line_len, |&b| b as usize);
        (a.min(line_len), b.min(line_len))
    }

    /// 위치가 속한 시각 줄의 줄 안 순번. 줄 바꿈 지점은 다음 시각 줄에 속한다.
    pub fn sub_of(&self, p: Pos) -> usize {
        self.breaks.get(p.line).map_or(0, |br| br.partition_point(|&b| b as usize <= p.col))
    }

    /// 위치의 시각 줄 번호.
    pub fn pos_row(&mut self, p: Pos) -> usize {
        let sub = self.sub_of(p);
        self.line_row(p.line) + sub
    }

    /// 다음(또는 이전) 숨기지 않은 줄.
    pub fn next_visible(&self, line: usize, forward: bool) -> Option<usize> {
        if forward {
            (line + 1..self.hidden.len()).find(|&l| !self.hidden[l])
        } else {
            (0..line).rev().find(|&l| !self.hidden[l])
        }
    }
}

/// 줄 바꿈으로 이어지는 시각 줄의 들여쓰기(열). 줄의 앞 공백 폭을 따르되 폭의 절반을 넘지 않는다.
pub(crate) fn wrap_indent_cols(line: &str, cols: usize, tab: usize) -> usize {
    let lead: usize = line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| char_cols(c, tab)).sum();
    lead.min(cols / 2)
}

/// 줄을 `cols` 열 폭으로 나눌 바이트 위치들. 공백 뒤에서 끊고, 끊을 곳이 없으면 글자 단위로 끊는다.
/// 두 번째 시각 줄부터는 [`wrap_indent_cols`] 만큼 좁은 폭을 쓴다.
pub(crate) fn compute_breaks(line: &str, cols: usize, tab: usize) -> Box<[u32]> {
    if cols == 0 || (line.len() <= cols && line.is_ascii() && !line.contains('\t')) {
        return Box::default();
    }
    let rest_cols = cols - wrap_indent_cols(line, cols, tab);
    let mut limit = cols;
    let mut out = Vec::new();
    let mut seg_start = 0usize;
    let mut w = 0usize;
    // 현재 시각 줄에서 마지막으로 끊을 수 있는 위치와 그 앞까지의 폭.
    let mut soft: Option<(usize, usize)> = None;
    let mut prev_space = false;
    for (i, c) in line.char_indices() {
        let cw = char_cols(c, tab);
        if prev_space && !c.is_whitespace() && i > seg_start {
            soft = Some((i, w));
        }
        if w + cw > limit && i > seg_start {
            let (at, used) = match soft {
                Some((at, used)) if at > seg_start => (at, used),
                _ => (i, w),
            };
            out.push(at as u32);
            limit = rest_cols;
            seg_start = at;
            w -= used;
            soft = None;
        }
        w += cw;
        prev_space = c.is_whitespace();
    }
    out.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(line: &str, cols: usize) -> Vec<&str> {
        let br = compute_breaks(line, cols, 4);
        let mut out = Vec::new();
        let mut a = 0;
        for &b in br.iter() {
            out.push(&line[a..b as usize]);
            a = b as usize;
        }
        out.push(&line[a..]);
        out
    }

    #[test]
    fn breaks_after_spaces_and_hard_breaks_long_words() {
        assert_eq!(segs("hello world foo", 11), vec!["hello ", "world foo"]);
        assert_eq!(segs("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(segs("short", 10), vec!["short"]);
        assert_eq!(segs("aa bbbbbbbbbb", 5), vec!["aa ", "bbbbb", "bbbbb"]);
        // 이어지는 시각 줄은 앞 공백만큼 좁다.
        assert_eq!(segs("  aaaa bbbb cc", 8), vec!["  aaaa ", "bbbb ", "cc"]);
        // 한글은 2열.
        assert_eq!(segs("가나다라마", 4), vec!["가나", "다라", "마"]);
        let long = "x ".repeat(100);
        for s in segs(&long, 13) {
            assert!(s.len() <= 13, "{s:?}");
        }
    }

    #[test]
    fn rows_and_lookup_with_wrap_and_hidden_lines() {
        let lines = ["aaaa bbbb cccc", "x", "y", "z", "dddd eeee"];
        let mut m = DisplayMap::new(lines.len());
        m.set_wrap(&lines, 5, 4);
        assert_eq!(m.total_rows(), 3 + 1 + 1 + 1 + 2);
        assert_eq!(m.row_to_line(2), (0, 2));
        assert_eq!(m.row_to_line(3), (1, 0));
        m.set_hidden(&[(2, 3)]);
        assert_eq!(m.total_rows(), 3 + 1 + 2);
        assert_eq!(m.row_to_line(4), (4, 0));
        assert_eq!(m.row_to_line(5), (4, 1));
        assert_eq!(m.line_row(4), 4);
        assert_eq!(m.segment(0, 1, lines[0].len()), (5, 10));
        assert_eq!(m.sub_of(Pos::new(0, 5)), 1);
        assert_eq!(m.sub_of(Pos::new(0, 4)), 0);
        assert_eq!(m.next_visible(1, true), Some(4));
        assert_eq!(m.next_visible(4, false), Some(1));
    }

    #[test]
    fn incremental_edit_matches_full_rebuild() {
        let mut lines: Vec<String> = (0..50).map(|i| format!("line {i} {}", "word ".repeat(i % 7))).collect();
        let mut m = DisplayMap::new(lines.len());
        m.set_wrap(&lines, 12, 4);
        let _ = m.total_rows();
        lines.splice(10..13, ["a very long replacement line that wraps".to_owned()]);
        m.on_edit(LineEdit { start: 10, old_count: 3, new_count: 1 }, &lines);
        let mut full = DisplayMap::new(lines.len());
        full.set_wrap(&lines, 12, 4);
        assert_eq!(m.total_rows(), full.total_rows());
        for r in 0..full.total_rows() {
            assert_eq!(m.row_to_line(r), full.row_to_line(r));
        }
    }
}
