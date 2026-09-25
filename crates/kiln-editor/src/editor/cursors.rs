//! 다중 커서: 커서 추가·정리, 다음 일치 추가, 위/아래 커서 추가.

use super::{Editor, Reveal, char_cols};
use crate::buffer::{Pos, Selection};

/// 보조 커서 하나. `pref` 는 세로 이동 시 유지할 표시 열.
/// `row_end_at` 이 헤드와 같으면 줄 바꿈 지점의 커서를 앞 시각 줄 끝에 둔다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub sel: Selection,
    pub pref: Option<usize>,
    pub row_end_at: Option<Pos>,
}

impl Cursor {
    pub fn new(sel: Selection) -> Self {
        Self { sel, pref: None, row_end_at: None }
    }
}

/// 사각(열) 선택. 줄 번호와 표시 열로 두 모서리를 잡는다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnSel {
    pub anchor_line: usize,
    pub anchor_col: usize,
    pub head_line: usize,
    pub head_col: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    /// 모든 커서의 선택 영역(문서 순서). 주 커서도 포함한다.
    pub fn cursors(&self) -> Vec<Selection> {
        let mut v: Vec<Selection> = self.extra.iter().map(|c| c.sel).collect();
        v.push(self.sel);
        v.sort_by_key(|s| s.range().0);
        v
    }

    /// 커서 수(주 커서 포함).
    pub fn cursor_count(&self) -> usize {
        self.extra.len() + 1
    }

    /// 주 커서를 [`Cursor`] 로 꺼낸다.
    pub(crate) fn primary_cursor(&self) -> Cursor {
        Cursor { sel: self.sel, pref: self.preferred_col, row_end_at: self.row_end_at }
    }

    /// 주 커서를 `c` 로 바꾼다.
    pub(crate) fn set_primary_cursor(&mut self, c: Cursor) {
        self.sel = c.sel;
        self.preferred_col = c.pref;
        self.row_end_at = c.row_end_at;
    }

    /// 커서 전체. 첫 항목이 주 커서이고 나머지는 보조 커서 순서.
    pub(crate) fn cursor_snapshot(&self) -> Vec<Selection> {
        std::iter::once(self.sel).chain(self.extra.iter().map(|c| c.sel)).collect()
    }

    /// [`Editor::cursor_snapshot`] 형식의 커서 전체로 바꾼다. 비어 있으면 그대로 둔다.
    pub(crate) fn restore_cursors(&mut self, all: &[Selection]) {
        let Some((&primary, others)) = all.split_first() else { return };
        let clamp = |s: Selection| Selection::new(self.buf.clamp(s.anchor), self.buf.clamp(s.head));
        self.sel = clamp(primary);
        self.extra = others.iter().map(|&s| Cursor::new(clamp(s))).collect();
        self.preferred_col = None;
        self.row_end_at = None;
        self.normalize_cursors();
    }

    /// 보조 커서를 모두 없애고 주 커서만 남긴다.
    pub fn collapse_cursors(&mut self) {
        self.extra.clear();
    }

    /// 주 커서는 그대로 두고 `p` 에 커서를 더한다. 이미 커서가 있는 곳이면 그 커서를 없앤다.
    pub fn add_cursor(&mut self, p: Pos) {
        let p = self.buf.clamp(p);
        if let Some(i) = self.extra.iter().position(|c| c.sel.head == p && c.sel.is_empty()) {
            self.extra.remove(i);
            return;
        }
        if self.sel.is_empty() && self.sel.head == p {
            if let Some(next) = self.extra.pop() {
                self.set_primary_cursor(next);
            }
            return;
        }
        self.extra.push(self.primary_cursor());
        self.sel = Selection::caret(p);
        self.preferred_col = None;
        self.row_end_at = None;
        self.normalize_cursors();
    }

    /// 주 커서를 `sel` 로 두고 나머지를 보조 커서로 지정한다.
    pub fn set_cursors(&mut self, primary: Selection, others: &[Selection]) {
        self.sel = Selection::new(self.buf.clamp(primary.anchor), self.buf.clamp(primary.head));
        self.extra = others
            .iter()
            .map(|s| Cursor::new(Selection::new(self.buf.clamp(s.anchor), self.buf.clamp(s.head))))
            .collect();
        self.preferred_col = None;
        self.normalize_cursors();
        self.reveal = Some(Reveal::Nearest);
    }

    /// 겹치거나 같은 자리의 커서를 합치고 보조 커서를 문서 순서로 정렬한다.
    pub(crate) fn normalize_cursors(&mut self) {
        if self.extra.is_empty() {
            return;
        }
        let mut all: Vec<(Cursor, bool)> = self.extra.drain(..).map(|c| (c, false)).collect();
        all.push((self.primary_cursor(), true));
        all.sort_by_key(|(c, _)| c.sel.range());
        let mut out: Vec<(Cursor, bool)> = Vec::with_capacity(all.len());
        for (c, prim) in all {
            if let Some((last, lprim)) = out.last_mut() {
                let (la, lb) = last.sel.range();
                let (a, b) = c.sel.range();
                if a < lb || (a == lb && (a == b || la == lb)) {
                    let lo = la.min(a);
                    let hi = lb.max(b);
                    let fwd = last.sel.anchor <= last.sel.head;
                    last.sel = if fwd { Selection::new(lo, hi) } else { Selection::new(hi, lo) };
                    *lprim |= prim;
                    continue;
                }
            }
            out.push((c, prim));
        }
        let pi = out.iter().position(|(_, p)| *p).unwrap_or(out.len() - 1);
        let (pc, _) = out.remove(pi);
        self.set_primary_cursor(pc);
        self.extra = out.into_iter().map(|(c, _)| c).collect();
    }

    /// Cmd+D: 선택이 없으면 커서 아래 단어를 고르고, 있으면 다음 일치를 새 커서로 더한다.
    pub fn add_next_occurrence(&mut self) {
        let (a, b) = self.sel.range();
        if a == b {
            let (wa, wb) = self.buf.word_at(a);
            if wa == wb {
                return;
            }
            self.sel = Selection::new(wa, wb);
            self.whole_word_next = true;
            self.reveal = Some(Reveal::Nearest);
            return;
        }
        if a.line != b.line {
            return;
        }
        let needle = self.buf.text_range(a, b);
        let whole = self.whole_word_next;
        let taken: Vec<(Pos, Pos)> = self.cursors().iter().map(|s| s.range()).collect();
        let n = self.buf.line_count();
        let is_free = |p: Pos, q: Pos| !taken.contains(&(p, q));
        let mut found = None;
        'outer: for k in 0..=n {
            let line_no = (b.line + k) % n;
            let text = self.buf.line(line_no);
            let from = if k == 0 { b.col } else { 0 };
            let upto = if k == n { a.col.min(text.len()) } else { text.len() };
            let mut start = from;
            while start <= upto {
                let Some(off) = text[start..].find(&needle) else { break };
                let ma = start + off;
                let mb = ma + needle.len();
                if mb > upto && k == n {
                    break;
                }
                let ok_word = !whole
                    || (!text[..ma].chars().next_back().is_some_and(is_word_char)
                        && !text[mb..].chars().next().is_some_and(is_word_char));
                let (p, q) = (Pos::new(line_no, ma), Pos::new(line_no, mb));
                if ok_word && is_free(p, q) {
                    found = Some((p, q));
                    break 'outer;
                }
                start = ma + needle.chars().next().map_or(1, char::len_utf8);
            }
        }
        if let Some((p, q)) = found {
            self.extra.push(Cursor::new(self.sel));
            self.sel = Selection::new(p, q);
            self.row_end_at = None;
            self.preferred_col = None;
            self.normalize_cursors();
            self.reveal = Some(Reveal::Nearest);
        }
    }

    /// 줄 `line` 에서 표시 열 `col` 에 가장 가까운 바이트 위치. 줄이 더 짧으면 줄 끝.
    pub(crate) fn byte_at_display_col(&self, line: usize, col: usize) -> usize {
        let tab = self.indent.width();
        let text = self.buf.line(line);
        let mut w = 0;
        for (i, c) in text.char_indices() {
            let cw = char_cols(c, tab);
            if w + cw > col {
                return if col - w > cw / 2 { i + c.len_utf8() } else { i };
            }
            w += cw;
        }
        text.len()
    }

    /// 사각 선택을 적용한다. 두 모서리 사이의 보이는 줄마다 커서와 선택을 하나씩 두고,
    /// 헤드 줄의 커서를 주 커서로 한다.
    pub(crate) fn set_column_selection(&mut self, cs: ColumnSel) {
        let n = self.buf.line_count();
        let cs = ColumnSel { anchor_line: cs.anchor_line.min(n - 1), head_line: cs.head_line.min(n - 1), ..cs };
        let (lo, hi) = (cs.anchor_line.min(cs.head_line), cs.anchor_line.max(cs.head_line));
        let mut primary = None;
        let mut others = Vec::new();
        for l in lo..=hi {
            if self.display.is_hidden(l) && l != cs.head_line {
                continue;
            }
            let s = Selection::new(
                Pos::new(l, self.byte_at_display_col(l, cs.anchor_col)),
                Pos::new(l, self.byte_at_display_col(l, cs.head_col)),
            );
            if l == cs.head_line {
                primary = Some(s);
            } else {
                others.push(s);
            }
        }
        let Some(primary) = primary else { return };
        self.sel = primary;
        self.extra = others.into_iter().map(Cursor::new).collect();
        self.preferred_col = None;
        self.row_end_at = None;
        self.normalize_cursors();
        self.reveal = Some(Reveal::Nearest);
        self.column = Some((cs, self.cursor_snapshot()));
    }

    /// 커서가 마지막 사각 선택 그대로면 그 사각 선택.
    pub(crate) fn active_column(&self) -> Option<ColumnSel> {
        self.column.as_ref().filter(|(_, snap)| *snap == self.cursor_snapshot()).map(|(cs, _)| *cs)
    }

    /// 사각 선택을 줄 `dl`, 표시 열 `dc` 만큼 넓힌다. 사각 선택이 없으면 주 커서에서 시작한다.
    pub(crate) fn extend_column(&mut self, dl: isize, dc: isize) {
        let mut cs = self.active_column().unwrap_or_else(|| ColumnSel {
            anchor_line: self.sel.anchor.line,
            anchor_col: self.display_col(self.sel.anchor),
            head_line: self.sel.head.line,
            head_col: self.display_col(self.sel.head),
        });
        if dl != 0 {
            let mut l = cs.head_line;
            for _ in 0..dl.unsigned_abs() {
                match self.display.next_visible(l, dl > 0) {
                    Some(next) => l = next,
                    None => break,
                }
            }
            if l == cs.head_line {
                return;
            }
            cs.head_line = l;
        }
        cs.head_col = cs.head_col.saturating_add_signed(dc);
        self.set_column_selection(cs);
    }

    /// 가장 위(또는 아래) 커서의 한 시각 줄 위(아래)에 커서를 더한다.
    pub fn add_cursor_vertical(&mut self, up: bool) {
        let all = self.cursors();
        let edge = if up { all[0] } else { all[all.len() - 1] };
        let head = edge.head;
        let pref = if edge == self.sel {
            self.preferred_col
        } else {
            self.extra.iter().find(|c| c.sel == edge).and_then(|c| c.pref)
        }
        .unwrap_or_else(|| self.row_display_col(head));
        let Some(p) = self.vertical_step(head, pref, if up { -1 } else { 1 }) else { return };
        if all.iter().any(|s| s.head == p) {
            return;
        }
        self.extra.push(self.primary_cursor());
        self.sel = Selection::caret(p);
        self.preferred_col = Some(pref);
        self.row_end_at = None;
        self.normalize_cursors();
        self.reveal = Some(Reveal::Nearest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_text("t.txt", text)
    }

    #[test]
    fn typing_backspace_and_delete_apply_to_every_cursor_in_one_undo_step() {
        let mut e = ed("one\ntwo\nthree");
        e.set_selection(Selection::caret(Pos::new(0, 3)));
        e.add_cursor(Pos::new(1, 3));
        e.add_cursor(Pos::new(2, 5));
        assert_eq!(e.cursor_count(), 3);
        e.insert_text("!");
        e.insert_text("?");
        assert_eq!(e.text(), "one!?\ntwo!?\nthree!?");
        e.backspace(false, false);
        assert_eq!(e.text(), "one!\ntwo!\nthree!");
        e.move_caret(super::super::Motion::Home, false);
        e.delete_forward(false);
        assert_eq!(e.text(), "ne!\nwo!\nhree!");
        e.undo();
        assert_eq!(e.text(), "one!\ntwo!\nthree!");
        e.undo();
        assert_eq!(e.text(), "one!?\ntwo!?\nthree!?");
        e.undo();
        assert_eq!(e.text(), "one\ntwo\nthree");
    }

    #[test]
    fn several_cursors_on_one_line_and_newline() {
        let mut e = ed("a,b,c");
        e.set_cursors(Selection::caret(Pos::new(0, 1)), &[Selection::caret(Pos::new(0, 3))]);
        e.newline();
        assert_eq!(e.text(), "a\n,b\n,c");
        let heads: Vec<Pos> = e.cursors().iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![Pos::new(1, 0), Pos::new(2, 0)]);
    }

    #[test]
    fn paste_distributes_lines_when_count_matches() {
        let mut e = ed("a\nb\nc");
        e.set_cursors(
            Selection::caret(Pos::new(0, 1)),
            &[Selection::caret(Pos::new(1, 1)), Selection::caret(Pos::new(2, 1))],
        );
        e.paste("1\n2\n3");
        assert_eq!(e.text(), "a1\nb2\nc3");
        e.paste("xy");
        assert_eq!(e.text(), "a1xy\nb2xy\nc3xy");
        assert_eq!(e.copy_text(), "a1xy\nb2xy\nc3xy\n");
    }

    #[test]
    fn cmd_d_selects_word_then_adds_next_whole_word_matches() {
        let mut e = ed("foo food foo\nfoo");
        e.set_selection(Selection::caret(Pos::new(0, 1)));
        e.add_next_occurrence();
        assert_eq!(e.sel.range(), (Pos::new(0, 0), Pos::new(0, 3)));
        e.add_next_occurrence();
        e.add_next_occurrence();
        let r: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| s.range()).collect();
        assert_eq!(r, vec![(Pos::new(0, 0), Pos::new(0, 3)), (Pos::new(0, 9), Pos::new(0, 12)), (Pos::new(1, 0), Pos::new(1, 3))]);
        e.add_next_occurrence();
        assert_eq!(e.cursor_count(), 3);
        e.insert_text("bar");
        assert_eq!(e.text(), "bar food bar\nbar");
    }

    #[test]
    fn add_cursor_below_keeps_column_and_esc_collapses() {
        let mut e = ed("abcdef\nab\nabcdef");
        e.set_selection(Selection::caret(Pos::new(0, 4)));
        e.add_cursor_vertical(false);
        e.add_cursor_vertical(false);
        let heads: Vec<Pos> = e.cursors().iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![Pos::new(0, 4), Pos::new(1, 2), Pos::new(2, 4)]);
        e.move_caret(super::super::Motion::Right, true);
        let sels: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| s.range()).collect();
        assert_eq!(sels[0], (Pos::new(0, 4), Pos::new(0, 5)));
        assert_eq!(sels[1], (Pos::new(1, 2), Pos::new(2, 0)));
        e.collapse_cursors();
        assert_eq!(e.cursor_count(), 1);
    }

    #[test]
    fn backspace_joining_lines_at_several_cursors() {
        let mut e = ed("a\nb\nc");
        e.set_cursors(Selection::caret(Pos::new(1, 0)), &[Selection::caret(Pos::new(2, 0))]);
        e.backspace(false, false);
        assert_eq!(e.text(), "abc");
        assert_eq!(e.display.total_rows(), 1);
        e.undo();
        assert_eq!(e.text(), "a\nb\nc");
        assert_eq!(e.display.total_rows(), 3);
    }

    #[test]
    fn overlapping_cursors_merge_after_backspace() {
        let mut e = ed("ab");
        e.set_cursors(Selection::caret(Pos::new(0, 1)), &[Selection::caret(Pos::new(0, 2))]);
        e.backspace(false, false);
        e.backspace(false, false);
        assert_eq!(e.text(), "");
        assert_eq!(e.cursor_count(), 1);
    }

    fn carets(e: &Editor) -> Vec<Pos> {
        e.cursors().iter().map(|s| s.head).collect()
    }

    fn rs(text: &str) -> Editor {
        Editor::from_text("t.rs", text)
    }

    #[test]
    fn undo_and_redo_restore_every_cursor_and_selection() {
        let mut e = ed("one\ntwo\nthree");
        e.set_cursors(
            Selection::new(Pos::new(0, 0), Pos::new(0, 3)),
            &[Selection::caret(Pos::new(1, 1)), Selection::new(Pos::new(2, 5), Pos::new(2, 2))],
        );
        let before: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| (s.anchor, s.head)).collect();
        e.insert_text("X");
        assert_eq!(e.text(), "X\ntXwo\nthX");
        let after = e.cursors();
        e.set_selection(Selection::caret(Pos::new(0, 0)));
        e.undo();
        assert_eq!(e.text(), "one\ntwo\nthree");
        assert_eq!(e.cursors().iter().map(|s| (s.anchor, s.head)).collect::<Vec<_>>(), before);
        assert_eq!(e.selection(), Selection::new(Pos::new(0, 0), Pos::new(0, 3)), "주 커서도 되살린다");
        e.redo();
        assert_eq!(e.cursors(), after);
        assert_eq!(e.cursor_count(), 3);
    }

    #[test]
    fn toggle_comment_applies_once_per_line_for_all_cursors() {
        let mut e = rs("a\nb\nc\nd");
        e.set_cursors(
            Selection::caret(Pos::new(0, 1)),
            &[Selection::caret(Pos::new(0, 0)), Selection::caret(Pos::new(2, 1))],
        );
        e.toggle_comment();
        assert_eq!(e.text(), "// a\nb\n// c\nd");
        assert_eq!(carets(&e), vec![Pos::new(0, 3), Pos::new(0, 4), Pos::new(2, 4)]);
        e.toggle_comment();
        assert_eq!(e.text(), "a\nb\nc\nd");
        e.undo();
        assert_eq!(e.text(), "// a\nb\n// c\nd");
        e.undo();
        assert_eq!(e.text(), "a\nb\nc\nd");
        assert_eq!(carets(&e), vec![Pos::new(0, 0), Pos::new(0, 1), Pos::new(2, 1)]);
    }

    #[test]
    fn toggle_comment_keeps_line_selection_start_at_column_zero() {
        let mut e = rs("a\nb\nc");
        e.set_cursors(Selection::new(Pos::new(0, 0), Pos::new(1, 1)), &[Selection::caret(Pos::new(2, 0))]);
        e.toggle_comment();
        assert_eq!(e.text(), "// a\n// b\n// c");
        assert_eq!(e.cursors()[0], Selection::new(Pos::new(0, 0), Pos::new(1, 4)));
        assert_eq!(e.cursors()[1], Selection::caret(Pos::new(2, 3)));
    }

    #[test]
    fn delete_lines_removes_each_cursor_line_once() {
        let mut e = ed("a\nb\nc\nd\ne");
        e.set_cursors(
            Selection::caret(Pos::new(1, 0)),
            &[Selection::caret(Pos::new(1, 1)), Selection::caret(Pos::new(3, 0))],
        );
        e.delete_lines();
        assert_eq!(e.text(), "a\nc\ne");
        assert_eq!(carets(&e), vec![Pos::new(1, 0), Pos::new(2, 0)]);
        e.undo();
        assert_eq!(e.text(), "a\nb\nc\nd\ne");
        assert_eq!(carets(&e), vec![Pos::new(1, 0), Pos::new(1, 1), Pos::new(3, 0)]);
    }

    #[test]
    fn delete_lines_at_end_of_document_with_several_cursors() {
        let mut e = ed("a\nb\nc");
        e.set_cursors(Selection::caret(Pos::new(2, 1)), &[Selection::caret(Pos::new(0, 0))]);
        e.delete_lines();
        assert_eq!(e.text(), "b");
        assert_eq!(e.cursor_count(), 1);
        assert_eq!(e.selection(), Selection::caret(Pos::new(0, 0)));
    }

    #[test]
    fn cut_without_selections_deletes_every_cursor_line() {
        let mut e = ed("a\nb\nc");
        e.set_cursors(Selection::caret(Pos::new(0, 0)), &[Selection::caret(Pos::new(2, 0))]);
        assert_eq!(e.cut(), "a\nc\n");
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn move_lines_moves_every_block_and_keeps_columns() {
        let mut e = ed("a\nb\nc\nd\ne");
        e.set_cursors(Selection::caret(Pos::new(0, 1)), &[Selection::caret(Pos::new(2, 0))]);
        e.move_lines(false);
        assert_eq!(e.text(), "b\na\nd\nc\ne");
        assert_eq!(carets(&e), vec![Pos::new(1, 1), Pos::new(3, 0)]);
        e.move_lines(false);
        assert_eq!(e.text(), "b\nd\na\ne\nc");
        e.move_lines(false);
        assert_eq!(e.text(), "b\nd\na\ne\nc", "맨 아래 구간이 못 내려가면 그대로 둔다");
        e.undo();
        assert_eq!(e.text(), "b\na\nd\nc\ne");
        assert_eq!(carets(&e), vec![Pos::new(1, 1), Pos::new(3, 0)]);
    }

    #[test]
    fn move_lines_merges_adjacent_cursor_lines_and_same_line_cursors() {
        let mut e = ed("a\nb\nc\nd");
        e.set_cursors(
            Selection::caret(Pos::new(1, 0)),
            &[Selection::caret(Pos::new(1, 1)), Selection::caret(Pos::new(2, 0))],
        );
        e.move_lines(true);
        assert_eq!(e.text(), "b\nc\na\nd");
        assert_eq!(carets(&e), vec![Pos::new(0, 0), Pos::new(0, 1), Pos::new(1, 0)]);
        e.move_lines(true);
        assert_eq!(e.text(), "b\nc\na\nd");
    }

    #[test]
    fn indent_and_outdent_apply_to_all_cursor_lines() {
        let mut e = ed("a\nb\nc");
        e.set_cursors(
            Selection::caret(Pos::new(0, 0)),
            &[Selection::caret(Pos::new(0, 1)), Selection::caret(Pos::new(2, 1))],
        );
        e.indent_lines(true);
        assert_eq!(e.text(), "    a\nb\n    c");
        assert_eq!(carets(&e), vec![Pos::new(0, 4), Pos::new(0, 5), Pos::new(2, 5)]);
        e.indent_lines(false);
        assert_eq!(e.text(), "a\nb\nc");
        assert_eq!(carets(&e), vec![Pos::new(0, 0), Pos::new(0, 1), Pos::new(2, 1)]);
    }

    #[test]
    fn tab_with_multi_line_selection_indents_lines_of_every_cursor() {
        let mut e = ed("a\nb\nc\nd");
        e.set_cursors(Selection::new(Pos::new(0, 0), Pos::new(1, 1)), &[Selection::caret(Pos::new(3, 1))]);
        e.indent_or_tab();
        assert_eq!(e.text(), "    a\n    b\nc\n    d");
        assert_eq!(e.cursors()[0], Selection::new(Pos::new(0, 0), Pos::new(1, 5)));
        assert_eq!(e.cursors()[1], Selection::caret(Pos::new(3, 5)));
        e.undo();
        assert_eq!(e.text(), "a\nb\nc\nd");
        assert_eq!(e.cursor_count(), 2);
    }

    #[test]
    fn insert_line_below_and_above_once_per_cursor_line() {
        let mut e = ed("  a\nb");
        e.set_cursors(
            Selection::caret(Pos::new(0, 1)),
            &[Selection::caret(Pos::new(0, 2)), Selection::caret(Pos::new(1, 0))],
        );
        e.insert_line(true);
        assert_eq!(e.text(), "  a\n  \nb\n");
        assert_eq!(carets(&e), vec![Pos::new(1, 2), Pos::new(3, 0)]);
        e.undo();
        assert_eq!(e.text(), "  a\nb");
        assert_eq!(carets(&e), vec![Pos::new(0, 1), Pos::new(0, 2), Pos::new(1, 0)]);
        e.insert_line(false);
        assert_eq!(e.text(), "  \n  a\n\nb");
        assert_eq!(carets(&e), vec![Pos::new(0, 2), Pos::new(2, 0)]);
        e.insert_text("x");
        assert_eq!(e.text(), "  x\n  a\nx\nb");
    }

    #[test]
    fn column_selection_spans_rectangle_and_short_lines_get_end_caret() {
        let mut e = ed("abcdef\nab\nabcdef");
        e.set_column_selection(ColumnSel { anchor_line: 0, anchor_col: 3, head_line: 2, head_col: 5 });
        let r: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| s.range()).collect();
        assert_eq!(
            r,
            vec![(Pos::new(0, 3), Pos::new(0, 5)), (Pos::new(1, 2), Pos::new(1, 2)), (Pos::new(2, 3), Pos::new(2, 5))]
        );
        assert_eq!(e.selection().head, Pos::new(2, 5), "헤드 줄이 주 커서");
        e.insert_text("X");
        assert_eq!(e.text(), "abcXf\nabX\nabcXf");
    }

    #[test]
    fn column_selection_uses_display_columns_for_tabs_and_wide_chars() {
        let mut e = ed("\tx\n한글ab\nabcdefgh");
        e.set_column_selection(ColumnSel { anchor_line: 0, anchor_col: 4, head_line: 2, head_col: 5 });
        let r: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| s.range()).collect();
        assert_eq!(
            r,
            vec![(Pos::new(0, 1), Pos::new(0, 2)), (Pos::new(1, 6), Pos::new(1, 7)), (Pos::new(2, 4), Pos::new(2, 5))]
        );
    }

    #[test]
    fn extend_column_grows_from_primary_and_keeps_rectangle() {
        let mut e = ed("abcd\nab\nabcd\nabcd");
        e.set_selection(Selection::caret(Pos::new(0, 2)));
        e.extend_column(1, 0);
        e.extend_column(1, 0);
        assert_eq!(carets(&e), vec![Pos::new(0, 2), Pos::new(1, 2), Pos::new(2, 2)]);
        e.extend_column(0, 1);
        let r: Vec<(Pos, Pos)> = e.cursors().iter().map(|s| s.range()).collect();
        assert_eq!(r, vec![(Pos::new(0, 2), Pos::new(0, 3)), (Pos::new(1, 2), Pos::new(1, 2)), (Pos::new(2, 2), Pos::new(2, 3))]);
        e.extend_column(-1, 0);
        assert_eq!(e.cursor_count(), 2);
        e.move_caret(super::super::Motion::Right, false);
        assert!(e.active_column().is_none(), "커서가 바뀌면 사각 선택은 끝난다");
    }
}
