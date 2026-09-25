//! 다중 커서: 커서 추가·정리, 다음 일치 추가, 위/아래 커서 추가.

use super::{Editor, Reveal};
use crate::buffer::{Pos, Selection};

/// 보조 커서 하나. `pref` 는 세로 이동 시 유지할 표시 열.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub sel: Selection,
    pub pref: Option<usize>,
}

impl Cursor {
    pub fn new(sel: Selection) -> Self {
        Self { sel, pref: None }
    }
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
                self.sel = next.sel;
                self.preferred_col = next.pref;
            }
            return;
        }
        self.extra.push(Cursor { sel: self.sel, pref: self.preferred_col });
        self.sel = Selection::caret(p);
        self.preferred_col = None;
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
        all.push((Cursor { sel: self.sel, pref: self.preferred_col }, true));
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
        self.sel = pc.sel;
        self.preferred_col = pc.pref;
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
            self.extra.push(Cursor { sel: self.sel, pref: None });
            self.sel = Selection::new(p, q);
            self.preferred_col = None;
            self.normalize_cursors();
            self.reveal = Some(Reveal::Nearest);
        }
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
        self.extra.push(Cursor { sel: self.sel, pref: self.preferred_col });
        self.sel = Selection::caret(p);
        self.preferred_col = Some(pref);
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
    fn overlapping_cursors_merge_after_backspace() {
        let mut e = ed("ab");
        e.set_cursors(Selection::caret(Pos::new(0, 1)), &[Selection::caret(Pos::new(0, 2))]);
        e.backspace(false, false);
        e.backspace(false, false);
        assert_eq!(e.text(), "");
        assert_eq!(e.cursor_count(), 1);
    }
}
