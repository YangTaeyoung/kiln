//! 코드 접기: 괄호 짝과 들여쓰기로 접을 범위를 찾고, 접힌 상태를 편집에 맞춰 유지한다.

use super::Editor;
use crate::buffer::{LineEdit, Pos, Selection};

/// 접을 수 있는 범위. `start` 줄은 보이고 `start + 1 ..= end` 줄이 숨는다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FoldRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Default)]
pub(crate) struct FoldState {
    /// 계산된 범위(시작 줄 순). `version` 은 계산 당시 버퍼 버전.
    ranges: Vec<FoldRange>,
    version: Option<u64>,
    /// 접힌 범위(시작 줄 순).
    pub folded: Vec<FoldRange>,
    /// 접힌 범위가 바뀌어 화면 대응을 다시 만들어야 한다.
    pub changed: bool,
}

impl FoldState {
    /// 줄 편집을 반영한다. 편집이 닿은 접힘(머리 줄부터 닫는 줄까지)은 풀고, 뒤쪽 접힘은 밀어 준다.
    pub fn on_edit(&mut self, e: LineEdit) {
        let delta = e.new_count as isize - e.old_count as isize;
        let last = e.start + e.old_count.max(1) - 1;
        let before = self.folded.len();
        self.folded.retain(|f| last < f.start || e.start > f.end + 1);
        if self.folded.len() != before {
            self.changed = true;
        }
        for f in &mut self.folded {
            if f.start > last {
                f.start = (f.start as isize + delta) as usize;
                f.end = (f.end as isize + delta) as usize;
                self.changed |= delta != 0;
            }
        }
        for r in &mut self.ranges {
            if r.start > last {
                r.start = (r.start as isize + delta) as usize;
                r.end = (r.end as isize + delta) as usize;
            }
        }
    }

    /// 숨길 줄 범위(양 끝 포함).
    pub fn hidden_ranges(&self) -> Vec<(usize, usize)> {
        self.folded.iter().map(|f| (f.start + 1, f.end)).collect()
    }

    pub fn is_folded_at(&self, line: usize) -> bool {
        self.folded.iter().any(|f| f.start == line)
    }
}

/// 괄호 짝과 들여쓰기로 접을 범위를 계산한다. 한 줄에서 시작하는 범위는 하나만 남긴다.
pub(crate) fn compute_fold_ranges<S: AsRef<str>>(lines: &[S], tab: usize, line_comment: Option<&str>) -> Vec<FoldRange> {
    let n = lines.len();
    let mut by_start: Vec<Option<usize>> = vec![None; n];
    let c_like = line_comment == Some("//");

    // 괄호 짝
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut in_block = false;
    for (i, line) in lines.iter().enumerate() {
        let b = line.as_ref().as_bytes();
        let mut j = 0;
        let mut quote: Option<u8> = None;
        while j < b.len() {
            let c = b[j];
            if in_block {
                if c == b'*' && b.get(j + 1) == Some(&b'/') {
                    in_block = false;
                    j += 1;
                }
                j += 1;
                continue;
            }
            if let Some(q) = quote {
                if c == b'\\' {
                    j += 1;
                } else if c == q {
                    quote = None;
                }
                j += 1;
                continue;
            }
            if let Some(tok) = line_comment
                && b[j..].starts_with(tok.as_bytes())
            {
                break;
            }
            match c {
                b'"' | b'`' => quote = Some(c),
                b'/' if c_like && b.get(j + 1) == Some(&b'*') => {
                    in_block = true;
                    j += 1;
                }
                b'{' | b'[' | b'(' => stack.push((c, i)),
                b'}' | b']' | b')' => {
                    let open = match c {
                        b'}' => b'{',
                        b']' => b'[',
                        _ => b'(',
                    };
                    if let Some(k) = stack.iter().rposition(|&(o, _)| o == open) {
                        let (_, start) = stack[k];
                        stack.truncate(k);
                        if i > start {
                            let closes_first = line.as_ref().trim_start().as_bytes().first() == Some(&c);
                            let end = if closes_first { i - 1 } else { i };
                            if end > start {
                                let e = by_start[start].get_or_insert(end);
                                *e = (*e).max(end);
                            }
                        }
                    }
                }
                _ => {}
            }
            j += 1;
        }
        // 닫히지 않은 문자열은 줄 끝에서 끝낸다.
    }

    // 들여쓰기
    let indent_of = |s: &str| -> Option<usize> {
        if s.trim().is_empty() {
            return None;
        }
        Some(s.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { tab } else { 1 }).sum())
    };
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_non_blank = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let Some(ind) = indent_of(line.as_ref()) else { continue };
        while let Some(&(start, sind)) = open.last() {
            if ind > sind {
                break;
            }
            open.pop();
            if last_non_blank > start && by_start[start].is_none() {
                by_start[start] = Some(last_non_blank);
            }
        }
        open.push((i, ind));
        last_non_blank = i;
    }
    while let Some((start, _)) = open.pop() {
        if last_non_blank > start && by_start[start].is_none() {
            by_start[start] = Some(last_non_blank);
        }
    }

    by_start
        .into_iter()
        .enumerate()
        .filter_map(|(start, end)| end.map(|end| FoldRange { start, end }))
        .collect()
}

impl Editor {
    /// 접을 수 있는 범위(버퍼가 바뀌었으면 다시 계산한다).
    pub fn fold_ranges(&mut self) -> &[FoldRange] {
        let v = self.buf.version();
        if self.folds.version != Some(v) {
            let tok = match self.comment {
                Some(crate::syntax::CommentStyle::Line(t)) => Some(t),
                _ => None,
            };
            self.folds.ranges = compute_fold_ranges(self.buf.lines(), self.indent.width(), tok);
            self.folds.version = Some(v);
        }
        &self.folds.ranges
    }

    /// 현재 접힌 범위.
    pub fn folded_ranges(&self) -> Vec<FoldRange> {
        self.folds.folded.clone()
    }

    /// 계산된 범위가 아니어도 `start` 줄부터 `end` 줄까지를 접는다(`start` 는 보인다).
    pub fn fold_range(&mut self, r: FoldRange) {
        if r.end <= r.start || r.end >= self.buf.line_count() || self.folds.folded.contains(&r) {
            return;
        }
        let pos = self.folds.folded.partition_point(|f| *f < r);
        self.folds.folded.insert(pos, r);
        self.folds.changed = true;
        self.move_cursors_out_of(r);
        self.apply_folds();
    }

    /// `line` 을 포함하는 가장 안쪽 범위를 접는다. 이미 접혀 있으면 한 단계 바깥을 접는다.
    pub fn fold_at(&mut self, line: usize) -> bool {
        let ranges = self.fold_ranges().to_vec();
        let folded = self.folds.folded.clone();
        let cand = ranges
            .iter()
            .filter(|r| r.start <= line && line <= r.end && !folded.contains(r))
            .max_by_key(|r| r.start)
            .copied();
        match cand {
            Some(r) => {
                self.fold_range(r);
                true
            }
            None => false,
        }
    }

    /// `line` 에서 시작하거나 `line` 을 품은 접힘을 푼다.
    pub fn unfold_at(&mut self, line: usize) -> bool {
        let before = self.folds.folded.len();
        let hit = self.folds.folded.iter().rposition(|f| f.start == line).or_else(|| {
            self.folds.folded.iter().rposition(|f| f.start <= line && line <= f.end)
        });
        if let Some(i) = hit {
            self.folds.folded.remove(i);
        }
        self.folds.changed |= self.folds.folded.len() != before;
        self.apply_folds();
        hit.is_some()
    }

    pub fn toggle_fold_at(&mut self, line: usize) {
        if self.folds.is_folded_at(line) {
            self.unfold_at(line);
        } else if let Some(r) = self.fold_ranges().iter().find(|r| r.start == line).copied() {
            self.fold_range(r);
        }
    }

    /// 계산된 모든 범위를 접는다.
    pub fn fold_all(&mut self) {
        let ranges = self.fold_ranges().to_vec();
        self.folds.folded = ranges;
        self.folds.changed = true;
        let all = self.folds.folded.clone();
        for r in all {
            self.move_cursors_out_of(r);
        }
        self.apply_folds();
    }

    pub fn unfold_all(&mut self) {
        if !self.folds.folded.is_empty() {
            self.folds.folded.clear();
            self.folds.changed = true;
        }
        self.apply_folds();
    }

    /// 숨은 줄에 들어간 커서를 머리 줄 끝으로 옮긴다.
    fn move_cursors_out_of(&mut self, r: FoldRange) {
        let inside = |p: Pos| p.line > r.start && p.line <= r.end;
        let to = Pos::new(r.start, self.buf.line(r.start).len());
        let fix = |s: &mut Selection| {
            if inside(s.head) || inside(s.anchor) {
                *s = Selection::caret(to);
            }
        };
        fix(&mut self.sel);
        for c in &mut self.extra {
            fix(&mut c.sel);
        }
        self.normalize_cursors();
    }

    /// 접힘 변화를 화면 대응에 반영한다.
    pub(crate) fn apply_folds(&mut self) {
        if self.folds.changed {
            self.folds.changed = false;
            let hidden = self.folds.hidden_ranges();
            self.display.set_hidden(&hidden);
        }
    }

    /// 커서가 숨은 줄에 있으면 그 줄을 품은 접힘을 푼다.
    pub(crate) fn unfold_around_cursors(&mut self) {
        if self.folds.folded.is_empty() {
            return;
        }
        let mut heads: Vec<usize> = vec![self.sel.head.line];
        heads.extend(self.extra.iter().map(|c| c.sel.head.line));
        let before = self.folds.folded.len();
        self.folds.folded.retain(|f| !heads.iter().any(|&l| l > f.start && l <= f.end));
        if self.folds.folded.len() != before {
            self.folds.changed = true;
            self.apply_folds();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(src: &str, tok: Option<&str>) -> Vec<(usize, usize)> {
        let lines: Vec<&str> = src.split('\n').collect();
        compute_fold_ranges(&lines, 4, tok).into_iter().map(|r| (r.start, r.end)).collect()
    }

    #[test]
    fn bracket_ranges_keep_closing_line_visible() {
        let src = "fn a() {\n    let x = [\n        1,\n    ];\n    if x {\n        y();\n    }\n}\n";
        assert_eq!(ranges(src, Some("//")), vec![(0, 6), (1, 2), (4, 5)]);
    }

    #[test]
    fn brackets_in_strings_and_comments_are_ignored() {
        let src = "let s = \"{\";\n// {\n/* { */\nfn b() {\n    x\n}";
        assert_eq!(ranges(src, Some("//")), vec![(3, 4)]);
    }

    #[test]
    fn indentation_ranges_for_python() {
        let src = "def f():\n    a = 1\n\n    if a:\n        b()\n\nprint(1)";
        assert_eq!(ranges(src, Some("#")), vec![(0, 4), (3, 4)]);
    }

    #[test]
    fn editing_inside_a_fold_unfolds_it_and_later_folds_shift() {
        let src = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n";
        let mut e = Editor::from_text("t.rs", src);
        e.fold_all();
        assert_eq!(e.folded_ranges().len(), 2);
        assert!(e.display.is_hidden(1) && e.display.is_hidden(4));
        e.set_selection(Selection::caret(Pos::new(0, 0)));
        e.insert_text("\n");
        assert_eq!(e.folded_ranges(), vec![FoldRange { start: 4, end: 5 }]);
        assert!(!e.display.is_hidden(2) && e.display.is_hidden(5));
        e.set_selection(Selection::caret(Pos::new(4, 0)));
        e.insert_text("z");
        assert!(e.folded_ranges().is_empty());
    }

    #[test]
    fn fold_at_cursor_moves_hidden_cursor_to_header_and_unfold_restores() {
        let src = "fn a() {\n    if x {\n        y();\n    }\n}\n";
        let mut e = Editor::from_text("t.rs", src);
        e.set_selection(Selection::caret(Pos::new(2, 4)));
        assert!(e.fold_at(2));
        assert_eq!(e.folded_ranges(), vec![FoldRange { start: 1, end: 2 }]);
        assert_eq!(e.sel.head, Pos::new(1, 10));
        assert!(e.fold_at(1));
        assert_eq!(e.folded_ranges().len(), 2);
        assert_eq!(e.sel.head, Pos::new(0, 8));
        assert!(e.unfold_at(0));
        e.unfold_all();
        assert!(e.folded_ranges().is_empty());
        assert!(!e.display.is_hidden(2));
    }
}
