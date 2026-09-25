//! 줄 벡터 기반 텍스트 버퍼. 편집, 되돌리기, 줄 끝 보존, 들여쓰기 감지, 주석 토글, 찾기.

use regex::{Regex, RegexBuilder};

/// 버퍼 안의 위치. `col` 은 줄 안의 바이트 오프셋(항상 문자 경계).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// 앵커와 헤드(커서)로 이루어진 선택 영역.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn caret(p: Pos) -> Self {
        Self { anchor: p, head: p }
    }

    pub fn new(anchor: Pos, head: Pos) -> Self {
        Self { anchor, head }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// (시작, 끝) 순서로 정렬된 범위.
    pub fn range(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

impl std::fmt::Display for LineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 들여쓰기 단위.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(u8),
    Tabs,
}

impl Indent {
    /// 한 단계 들여쓰기 문자열.
    pub fn unit(self) -> String {
        match self {
            Indent::Spaces(n) => " ".repeat(n as usize),
            Indent::Tabs => "\t".into(),
        }
    }

    /// 한 단계의 표시 폭(열 수).
    pub fn width(self) -> usize {
        match self {
            Indent::Spaces(n) => n as usize,
            Indent::Tabs => 4,
        }
    }

    pub fn label(self) -> String {
        match self {
            Indent::Spaces(n) => format!("공백: {n}"),
            Indent::Tabs => "탭".into(),
        }
    }
}

/// 줄 단위 변경 기록: `start` 부터 `old_count` 줄이 `new_count` 줄로 바뀌었다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub start: usize,
    pub old_count: usize,
    pub new_count: usize,
}

/// 되돌리기 병합 규칙을 정하는 편집 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Delete,
    Other,
}

#[derive(Clone, Debug)]
struct EditRecord {
    start: Pos,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct UndoGroup {
    id: u64,
    edits: Vec<EditRecord>,
    before: Selection,
    after: Selection,
    kind: EditKind,
    time: f64,
    sealed: bool,
}

/// LSP 증분 동기화용 텍스트 변경. 열은 변경 직전 줄 기준 UTF-16 코드 단위.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDelta {
    pub start_line: usize,
    pub start_u16: usize,
    pub end_line: usize,
    pub end_u16: usize,
    pub text: String,
}

/// 줄 안 바이트 위치까지의 UTF-16 코드 단위 수.
pub fn utf16_len(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

const UNDO_LIMIT: usize = 2000;
const MERGE_WINDOW_SECS: f64 = 1.0;

pub struct Buffer {
    lines: Vec<String>,
    pub line_ending: LineEnding,
    version: u64,
    changes: Vec<LineEdit>,
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    open: bool,
    next_group_id: u64,
    saved_group: Option<u64>,
    track_deltas: bool,
    deltas: Vec<TextDelta>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl Buffer {
    /// 텍스트로 버퍼를 만든다. CRLF 가 LF 보다 많으면 CRLF 파일로 취급한다.
    pub fn from_text(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        let line_ending = if crlf > 0 && crlf * 2 >= lf { LineEnding::CrLf } else { LineEnding::Lf };
        let lines: Vec<String> = match line_ending {
            LineEnding::Lf => text.split('\n').map(str::to_owned).collect(),
            LineEnding::CrLf => text
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
                .collect(),
        };
        Self {
            lines,
            line_ending,
            version: 0,
            changes: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            next_group_id: 1,
            saved_group: None,
            track_deltas: false,
            deltas: Vec::new(),
        }
    }

    /// 원래 줄 끝 문자로 이어 붙인 전체 텍스트.
    pub fn to_text(&self) -> String {
        self.lines.join(self.line_ending.as_str())
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// 편집마다 증가하는 버전.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 마지막 저장 이후 내용이 바뀌었는지.
    pub fn is_dirty(&self) -> bool {
        self.undo.last().map(|g| g.id) != self.saved_group
    }

    /// 현재 상태를 저장된 상태로 표시한다.
    pub fn mark_saved(&mut self) {
        if let Some(g) = self.undo.last_mut() {
            g.sealed = true;
        }
        self.open = false;
        self.saved_group = self.undo.last().map(|g| g.id);
    }

    /// 누적된 줄 변경 기록을 꺼낸다.
    pub fn take_changes(&mut self) -> Vec<LineEdit> {
        std::mem::take(&mut self.changes)
    }

    /// 켜 두면 모든 변경을 [`TextDelta`] 로 기록한다.
    pub fn set_track_deltas(&mut self, on: bool) {
        self.track_deltas = on;
        self.deltas.clear();
    }

    /// 누적된 텍스트 변경을 꺼낸다.
    pub fn take_deltas(&mut self) -> Vec<TextDelta> {
        std::mem::take(&mut self.deltas)
    }

    /// 열린 편집 그룹에 지금까지 쌓인 편집 수. [`Buffer::map_since`] 의 기준점.
    pub fn edit_mark(&self) -> usize {
        if self.open { self.undo.last().map_or(0, |g| g.edits.len()) } else { 0 }
    }

    /// `mark` 이후 열린 그룹에서 일어난 편집을 거쳐 위치를 옮긴다. 편집 뒤쪽 위치만 밀린다.
    pub fn map_since(&self, mark: usize, mut p: Pos) -> Pos {
        let Some(g) = self.undo.last() else { return p };
        for e in g.edits.iter().skip(mark) {
            p = map_pos(p, e.start, advance(e.start, &e.removed), advance(e.start, &e.inserted));
        }
        p
    }

    pub fn end_pos(&self) -> Pos {
        let l = self.lines.len() - 1;
        Pos::new(l, self.lines[l].len())
    }

    /// 위치를 버퍼 범위와 문자 경계 안으로 맞춘다.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        let s = &self.lines[line];
        let mut col = p.col.min(s.len());
        while !s.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    pub fn text_range(&self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        if a.line == b.line {
            return self.lines[a.line][a.col..b.col].to_owned();
        }
        let mut out = String::new();
        out.push_str(&self.lines[a.line][a.col..]);
        for l in &self.lines[a.line + 1..b.line] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[b.line][..b.col]);
        out
    }

    /// 편집 그룹을 연다. 연속 타이핑은 직전 그룹에 합친다.
    pub fn begin(&mut self, kind: EditKind, before: Selection, time: f64) {
        self.redo.clear();
        if let Some(last) = self.undo.last_mut()
            && !last.sealed
            && last.kind == kind
            && kind != EditKind::Other
            && last.after == before
            && time - last.time < MERGE_WINDOW_SECS
        {
            last.time = time;
            self.open = true;
            return;
        }
        if let Some(last) = self.undo.last_mut() {
            last.sealed = true;
        }
        let id = self.next_group_id;
        self.next_group_id += 1;
        self.undo.push(UndoGroup {
            id,
            edits: Vec::new(),
            before,
            after: before,
            kind,
            time,
            sealed: false,
        });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.open = true;
    }

    /// 편집 그룹을 닫는다. 편집이 없었던 그룹은 버린다.
    pub fn end(&mut self, after: Selection) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(last) = self.undo.last_mut() {
            if last.edits.is_empty() {
                self.undo.pop();
            } else {
                last.after = after;
            }
        }
    }

    /// 다음 편집이 이전 그룹과 합쳐지지 않게 한다.
    pub fn seal(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.sealed = true;
        }
    }

    /// `a..b` 를 `text` 로 바꾸고 삽입된 텍스트의 끝 위치를 돌려준다.
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let text = normalize_newlines(text);
        let (removed, end) = self.raw_replace(a, b, &text);
        if !self.open {
            self.begin(EditKind::Other, Selection::caret(a), 0.0);
        }
        if let Some(g) = self.undo.last_mut() {
            g.edits.push(EditRecord { start: a, removed, inserted: text.into_owned() });
        }
        end
    }

    pub fn insert(&mut self, p: Pos, text: &str) -> Pos {
        self.replace(p, p, text)
    }

    fn raw_replace(&mut self, a: Pos, b: Pos, text: &str) -> (String, Pos) {
        let removed = self.text_range(a, b);
        self.version += 1;
        if self.track_deltas {
            self.deltas.push(TextDelta {
                start_line: a.line,
                start_u16: utf16_len(&self.lines[a.line][..a.col]),
                end_line: b.line,
                end_u16: utf16_len(&self.lines[b.line][..b.col]),
                text: text.to_owned(),
            });
        }
        if a.line == b.line && !text.contains('\n') {
            self.lines[a.line].replace_range(a.col..b.col, text);
            self.changes.push(LineEdit { start: a.line, old_count: 1, new_count: 1 });
            return (removed, Pos::new(a.line, a.col + text.len()));
        }
        let suffix = self.lines[b.line][b.col..].to_owned();
        let mut new_lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        new_lines[0].insert_str(0, &self.lines[a.line][..a.col]);
        let last = new_lines.len() - 1;
        let end = Pos::new(a.line + last, new_lines[last].len());
        new_lines[last].push_str(&suffix);
        let old_count = b.line - a.line + 1;
        let new_count = new_lines.len();
        self.lines.splice(a.line..=b.line, new_lines);
        self.changes.push(LineEdit { start: a.line, old_count, new_count });
        (removed, end)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 마지막 그룹을 되돌리고 그 이전 선택 영역을 돌려준다.
    pub fn undo(&mut self) -> Option<Selection> {
        self.open = false;
        let mut g = self.undo.pop()?;
        for e in g.edits.iter().rev() {
            let end = advance(e.start, &e.inserted);
            self.raw_replace(e.start, end, &e.removed);
        }
        g.sealed = true;
        let sel = g.before;
        self.redo.push(g);
        Some(sel)
    }

    /// 되돌린 그룹을 다시 적용하고 그 이후 선택 영역을 돌려준다.
    pub fn redo(&mut self) -> Option<Selection> {
        self.open = false;
        let g = self.redo.pop()?;
        for e in &g.edits {
            let end = advance(e.start, &e.removed);
            self.raw_replace(e.start, end, &e.inserted);
        }
        let sel = g.after;
        self.undo.push(g);
        Some(sel)
    }

    // ---- 커서 이동 보조 ----

    pub fn next_char(&self, p: Pos) -> Pos {
        let s = &self.lines[p.line];
        if p.col < s.len() {
            let n = s[p.col..].chars().next().map_or(1, char::len_utf8);
            Pos::new(p.line, p.col + n)
        } else if p.line + 1 < self.lines.len() {
            Pos::new(p.line + 1, 0)
        } else {
            p
        }
    }

    pub fn prev_char(&self, p: Pos) -> Pos {
        if p.col > 0 {
            let s = &self.lines[p.line];
            let n = s[..p.col].chars().next_back().map_or(1, char::len_utf8);
            Pos::new(p.line, p.col - n)
        } else if p.line > 0 {
            Pos::new(p.line - 1, self.lines[p.line - 1].len())
        } else {
            p
        }
    }

    /// 다음 단어 끝으로 이동한 위치.
    pub fn next_word(&self, p: Pos) -> Pos {
        let s = &self.lines[p.line];
        if p.col >= s.len() {
            return self.next_char(p);
        }
        let rest = &s[p.col..];
        let mut it = rest.char_indices().peekable();
        while let Some(&(_, c)) = it.peek() {
            if c.is_whitespace() {
                it.next();
            } else {
                break;
            }
        }
        let Some(&(_, first)) = it.peek() else { return Pos::new(p.line, s.len()) };
        let class = char_class(first);
        let mut end = rest.len();
        for (i, c) in it {
            if char_class(c) != class {
                end = i;
                break;
            }
        }
        Pos::new(p.line, p.col + end)
    }

    /// 이전 단어 시작으로 이동한 위치.
    pub fn prev_word(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return self.prev_char(p);
        }
        let s = &self.lines[p.line][..p.col];
        let mut it = s.char_indices().rev().peekable();
        while let Some(&(_, c)) = it.peek() {
            if c.is_whitespace() {
                it.next();
            } else {
                break;
            }
        }
        let Some(&(_, first)) = it.peek() else { return Pos::new(p.line, 0) };
        let class = char_class(first);
        let mut start = 0;
        for (i, c) in it {
            if char_class(c) != class {
                start = i + c.len_utf8();
                break;
            }
        }
        Pos::new(p.line, start)
    }

    /// 위치를 둘러싼 단어 범위.
    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let s = &self.lines[p.line];
        let at = s[p.col..].chars().next().or_else(|| s[..p.col].chars().next_back());
        let Some(c) = at else { return (p, p) };
        let class = char_class(c);
        let mut start = p.col;
        for (i, ch) in s[..p.col].char_indices().rev() {
            if char_class(ch) != class {
                break;
            }
            start = i;
        }
        let mut end = p.col;
        for (i, ch) in s[p.col..].char_indices() {
            if char_class(ch) != class {
                break;
            }
            end = p.col + i + ch.len_utf8();
        }
        (Pos::new(p.line, start), Pos::new(p.line, end))
    }

    /// 줄의 첫 비공백 문자 위치(바이트).
    pub fn first_non_ws(&self, line: usize) -> usize {
        let s = &self.lines[line];
        s.len() - s.trim_start().len()
    }

    // ---- 찾기 ----

    /// 모든 일치 범위를 줄 단위로 찾는다. 빈 일치는 제외한다.
    pub fn find_all(&self, re: &Regex, limit: usize) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue;
                }
                out.push((Pos::new(i, m.start()), Pos::new(i, m.end())));
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }

    // ---- 들여쓰기와 주석 ----

    /// 앞쪽 일부 줄을 보고 들여쓰기 단위를 추정한다.
    pub fn detect_indent(&self) -> Indent {
        detect_indent(self.lines.iter().map(String::as_str))
    }

    /// `first..=last` 줄의 줄 주석을 토글한다. 줄마다 (삽입·삭제 열, 변화량)을 돌려준다.
    pub fn toggle_comment(
        &mut self,
        first: usize,
        last: usize,
        style: crate::syntax::CommentStyle,
    ) -> Vec<(usize, usize, isize)> {
        use crate::syntax::CommentStyle;
        let rows: Vec<usize> = (first..=last).filter(|&l| !self.lines[l].trim().is_empty()).collect();
        if rows.is_empty() {
            return Vec::new();
        }
        let mut shifts = Vec::new();
        match style {
            CommentStyle::Line(tok) => {
                let all_commented = rows.iter().all(|&l| self.lines[l].trim_start().starts_with(tok));
                if all_commented {
                    for &l in &rows {
                        let ws = self.first_non_ws(l);
                        let after = &self.lines[l][ws + tok.len()..];
                        let n = tok.len() + usize::from(after.starts_with(' '));
                        self.replace(Pos::new(l, ws), Pos::new(l, ws + n), "");
                        shifts.push((l, ws, -(n as isize)));
                    }
                } else {
                    let col = rows.iter().map(|&l| self.first_non_ws(l)).min().unwrap_or(0);
                    let ins = format!("{tok} ");
                    for &l in &rows {
                        self.insert(Pos::new(l, col), &ins);
                        shifts.push((l, col, ins.len() as isize));
                    }
                }
            }
            CommentStyle::Block(open, close) => {
                let is_wrapped = |s: &str| {
                    let t = s.trim();
                    t.starts_with(open) && t.ends_with(close) && t.len() >= open.len() + close.len()
                };
                let all_commented = rows.iter().all(|&l| is_wrapped(&self.lines[l]));
                for &l in &rows {
                    let ws = self.first_non_ws(l);
                    if all_commented {
                        let s = &self.lines[l];
                        let end_trim = s.trim_end().len();
                        let close_start = end_trim - close.len();
                        let pre_close = close_start - usize::from(s[..close_start].ends_with(' ') && close_start > ws + open.len());
                        self.replace(Pos::new(l, pre_close), Pos::new(l, end_trim), "");
                        let after = &self.lines[l][ws + open.len()..];
                        let n = open.len() + usize::from(after.starts_with(' '));
                        self.replace(Pos::new(l, ws), Pos::new(l, ws + n), "");
                        shifts.push((l, ws, -(n as isize)));
                    } else {
                        let end = self.lines[l].trim_end().len();
                        self.insert(Pos::new(l, end), &format!(" {close}"));
                        let ins = format!("{open} ");
                        self.insert(Pos::new(l, ws), &ins);
                        shifts.push((l, ws, ins.len() as isize));
                    }
                }
            }
        }
        shifts
    }
}

/// `a..b` 가 `a..e` 로 바뀐 뒤의 위치. 범위 안의 위치는 `e` 로 간다.
pub fn map_pos(p: Pos, a: Pos, b: Pos, e: Pos) -> Pos {
    if p >= b {
        if p.line == b.line { Pos::new(e.line, e.col + (p.col - b.col)) } else { Pos::new(p.line + e.line - b.line, p.col) }
    } else if p > a {
        e
    } else {
        p
    }
}

/// 텍스트 삽입 후 끝 위치를 계산한다.
pub fn advance(start: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos::new(start.line, start.col + text.len()),
        Some(i) => Pos::new(start.line + text.matches('\n').count(), text.len() - i - 1),
    }
}

fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Word,
    Space,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punct
    }
}

/// 줄들의 들여쓰기 변화량 분포로 들여쓰기 단위를 고른다.
pub fn detect_indent<'a>(lines: impl Iterator<Item = &'a str>) -> Indent {
    let mut tabs = 0usize;
    let mut spaced = 0usize;
    let mut hist = [0usize; 9];
    let mut prev = 0usize;
    for line in lines.take(3000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = line.len() - line.trim_start_matches(' ').len();
        if n > 0 {
            spaced += 1;
        }
        let d = n.abs_diff(prev);
        if (2..=8).contains(&d) {
            hist[d] += 1;
        }
        prev = n;
    }
    if tabs > spaced {
        return Indent::Tabs;
    }
    if spaced == 0 {
        return Indent::Spaces(4);
    }
    let mut best = 4usize;
    for w in [2usize, 3, 8] {
        if hist[w] > hist[best] {
            best = w;
        }
    }
    Indent::Spaces(best as u8)
}

/// 찾기 옵션.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// 찾기 옵션으로 정규식을 만든다.
pub fn build_regex(query: &str, opts: FindOptions) -> Result<Regex, regex::Error> {
    let mut pat = if opts.regex { query.to_owned() } else { regex::escape(query) };
    if opts.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    RegexBuilder::new(&pat).case_insensitive(!opts.case_sensitive).build()
}

/// 일치 텍스트에 대한 치환 문자열. 정규식 모드면 `$1` 같은 캡처 참조를 확장한다.
pub fn expand_replacement(re: &Regex, matched: &str, replacement: &str, regex_mode: bool) -> String {
    if !regex_mode {
        return replacement.to_owned();
    }
    match re.captures(matched) {
        Some(caps) => {
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            dst
        }
        None => replacement.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::CommentStyle;

    #[test]
    fn round_trips_lf_crlf_and_trailing_newline() {
        for text in ["a\nb\n", "a\nb", "a\r\nb\r\n", "a\r\nb", "", "\n", "x\r\n\r\n"] {
            let b = Buffer::from_text(text);
            assert_eq!(b.to_text(), text, "{text:?}");
        }
        assert_eq!(Buffer::from_text("a\r\nb\r\n").line_ending, LineEnding::CrLf);
        assert_eq!(Buffer::from_text("a\nb\n").line_ending, LineEnding::Lf);
    }

    #[test]
    fn crlf_file_keeps_crlf_after_editing() {
        let mut b = Buffer::from_text("one\r\ntwo\r\n");
        b.insert(Pos::new(0, 3), "\nnew");
        assert_eq!(b.to_text(), "one\r\nnew\r\ntwo\r\n");
        b.insert(Pos::new(0, 0), "x\r\ny");
        assert_eq!(b.to_text(), "x\r\nyone\r\nnew\r\ntwo\r\n");
    }

    #[test]
    fn replace_across_lines_and_undo_redo() {
        let mut b = Buffer::from_text("hello\nworld\nfoo");
        b.begin(EditKind::Other, Selection::default(), 0.0);
        let end = b.replace(Pos::new(0, 2), Pos::new(1, 3), "XY\nZ");
        b.end(Selection::caret(end));
        assert_eq!(b.to_text(), "heXY\nZld\nfoo");
        assert_eq!(end, Pos::new(1, 1));
        assert!(b.is_dirty());
        b.undo();
        assert_eq!(b.to_text(), "hello\nworld\nfoo");
        assert!(!b.is_dirty());
        b.redo();
        assert_eq!(b.to_text(), "heXY\nZld\nfoo");
    }

    #[test]
    fn consecutive_typing_merges_into_one_undo_group() {
        let mut b = Buffer::from_text("");
        let mut p = Pos::default();
        for (i, ch) in "abc".chars().enumerate() {
            b.begin(EditKind::Typing, Selection::caret(p), i as f64 * 0.1);
            p = b.insert(p, &ch.to_string());
            b.end(Selection::caret(p));
        }
        assert_eq!(b.to_text(), "abc");
        b.undo();
        assert_eq!(b.to_text(), "");
    }

    #[test]
    fn saved_state_tracks_dirty_through_undo() {
        let mut b = Buffer::from_text("x");
        b.begin(EditKind::Typing, Selection::default(), 0.0);
        let p = b.insert(Pos::default(), "a");
        b.end(Selection::caret(p));
        b.mark_saved();
        assert!(!b.is_dirty());
        b.begin(EditKind::Typing, Selection::caret(p), 0.1);
        let p2 = b.insert(p, "b");
        b.end(Selection::caret(p2));
        assert!(b.is_dirty());
        b.undo();
        assert!(!b.is_dirty());
        assert_eq!(b.to_text(), "ax");
    }

    #[test]
    fn indentation_detection() {
        let four = "fn a() {\n    let x = 1;\n    if x {\n        y();\n    }\n}\n";
        assert_eq!(Buffer::from_text(four).detect_indent(), Indent::Spaces(4));
        let two = "a:\n  b:\n    c: 1\n  d: 2\n";
        assert_eq!(Buffer::from_text(two).detect_indent(), Indent::Spaces(2));
        let tabs = "func a() {\n\tx := 1\n\tif x {\n\t\ty()\n\t}\n}\n";
        assert_eq!(Buffer::from_text(tabs).detect_indent(), Indent::Tabs);
        assert_eq!(Buffer::from_text("no indent\nat all").detect_indent(), Indent::Spaces(4));
    }

    #[test]
    fn toggle_line_comment_adds_at_min_indent_and_removes() {
        let mut b = Buffer::from_text("fn a() {\n    x();\n\n        y();\n}");
        b.toggle_comment(1, 3, CommentStyle::Line("//"));
        assert_eq!(b.to_text(), "fn a() {\n    // x();\n\n    //     y();\n}");
        b.toggle_comment(1, 3, CommentStyle::Line("//"));
        assert_eq!(b.to_text(), "fn a() {\n    x();\n\n        y();\n}");
    }

    #[test]
    fn toggle_comment_on_mixed_lines_comments_all() {
        let mut b = Buffer::from_text("# a\nb");
        b.toggle_comment(0, 1, CommentStyle::Line("#"));
        assert_eq!(b.to_text(), "# # a\n# b");
    }

    #[test]
    fn toggle_block_comment_wraps_each_line() {
        let mut b = Buffer::from_text("  <div>\n  </div>");
        b.toggle_comment(0, 1, CommentStyle::Block("<!--", "-->"));
        assert_eq!(b.to_text(), "  <!-- <div> -->\n  <!-- </div> -->");
        b.toggle_comment(0, 1, CommentStyle::Block("<!--", "-->"));
        assert_eq!(b.to_text(), "  <div>\n  </div>");
    }

    #[test]
    fn find_with_case_word_and_regex_options() {
        let b = Buffer::from_text("Foo foo food\nFOO_bar foo");
        let n = |q: &str, o: FindOptions| b.find_all(&build_regex(q, o).unwrap(), 100).len();
        assert_eq!(n("foo", FindOptions::default()), 5);
        assert_eq!(n("foo", FindOptions { case_sensitive: true, ..Default::default() }), 3);
        assert_eq!(n("foo", FindOptions { whole_word: true, ..Default::default() }), 3);
        assert_eq!(n("fo+d?", FindOptions { regex: true, ..Default::default() }), 5);
        assert_eq!(n("a.b", FindOptions::default()), 0);
        assert!(build_regex("(", FindOptions { regex: true, ..Default::default() }).is_err());
    }

    #[test]
    fn regex_replacement_expands_captures() {
        let re = build_regex(r"(\w+)=(\d+)", FindOptions { regex: true, ..Default::default() }).unwrap();
        assert_eq!(expand_replacement(&re, "x=1", "$2=$1", true), "1=x");
        assert_eq!(expand_replacement(&re, "x=1", "$2=$1", false), "$2=$1");
    }

    #[test]
    fn word_motion_and_word_at() {
        let b = Buffer::from_text("let foo_bar = baz(1);");
        assert_eq!(b.next_word(Pos::new(0, 0)), Pos::new(0, 3));
        assert_eq!(b.next_word(Pos::new(0, 3)), Pos::new(0, 11));
        assert_eq!(b.prev_word(Pos::new(0, 11)), Pos::new(0, 4));
        assert_eq!(b.word_at(Pos::new(0, 6)), (Pos::new(0, 4), Pos::new(0, 11)));
    }

    #[test]
    fn map_since_shifts_positions_after_edits() {
        let mut b = Buffer::from_text("abc\ndef\nghi");
        b.begin(EditKind::Other, Selection::default(), 0.0);
        let m = b.edit_mark();
        b.replace(Pos::new(0, 1), Pos::new(1, 1), "XY\nZ\nW");
        assert_eq!(b.to_text(), "aXY\nZ\nWef\nghi");
        assert_eq!(b.map_since(m, Pos::new(1, 2)), Pos::new(2, 2));
        assert_eq!(b.map_since(m, Pos::new(2, 1)), Pos::new(3, 1));
        assert_eq!(b.map_since(m, Pos::new(0, 0)), Pos::new(0, 0));
        assert_eq!(b.map_since(m, Pos::new(0, 2)), Pos::new(2, 1));
        b.end(Selection::default());
    }

    #[test]
    fn deltas_record_utf16_columns_before_each_change() {
        let mut b = Buffer::from_text("한a😀b\nx");
        b.set_track_deltas(true);
        b.replace(Pos::new(0, 4), Pos::new(0, 8), "Q");
        b.replace(Pos::new(0, 5), Pos::new(1, 0), "");
        let d = b.take_deltas();
        assert_eq!(d[0], TextDelta { start_line: 0, start_u16: 2, end_line: 0, end_u16: 4, text: "Q".into() });
        assert_eq!(d[1], TextDelta { start_line: 0, start_u16: 3, end_line: 1, end_u16: 0, text: String::new() });
        b.undo();
        assert_eq!(b.take_deltas().len(), 2);
    }

    #[test]
    fn multibyte_char_navigation_stays_on_boundaries() {
        let b = Buffer::from_text("한글a");
        let p = b.next_char(Pos::new(0, 0));
        assert_eq!(p, Pos::new(0, 3));
        assert_eq!(b.prev_char(Pos::new(0, 6)), Pos::new(0, 3));
        assert_eq!(b.clamp(Pos::new(0, 4)), Pos::new(0, 3));
    }
}
