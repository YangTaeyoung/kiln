//! 파일 하나를 여는 코드 편집기.

mod find;
mod view;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use anyhow::Context as _;

use crate::buffer::{Buffer, EditKind, Indent, Pos, Selection};
use crate::highlight::Highlighter;
use crate::syntax::{self, CommentStyle};

pub use crate::buffer::LineEnding;
pub(crate) use find::FindState;

/// 이 크기를 넘는 파일은 구문 강조를 끈다.
pub const LARGE_FILE_BYTES: u64 = 5 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

/// 파일 인코딩.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// 잘못된 UTF-8 바이트를 U+FFFD 로 바꿔 읽었다.
    Utf8Lossy,
    Binary,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 with BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Utf8Lossy => "UTF-8 (손실)",
            Encoding::Binary => "바이너리",
        }
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 상태 표시줄용 정보. `line`, `col` 은 1부터 시작한다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorStatus {
    pub line: usize,
    pub col: usize,
    pub language: String,
    pub line_ending: LineEnding,
    pub encoding: Encoding,
    pub dirty: bool,
    pub indent: Indent,
    pub read_only: bool,
    /// 선택된 문자 수. 선택이 없으면 0.
    pub selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskStamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl DiskStamp {
    fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self { mtime: m.modified().ok(), len: m.len() })
    }
}

/// 디스크에서 읽어 해석한 파일 내용.
struct Decoded {
    text: String,
    encoding: Encoding,
}

fn decode(bytes: &[u8]) -> Decoded {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Decoded { text: String::from_utf8_lossy(rest).into_owned(), encoding: Encoding::Utf8Bom };
    }
    let utf16 = |rest: &[u8], le: bool| -> String {
        let units = rest.as_chunks::<2>().0.iter().map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) });
        char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Decoded { text: utf16(rest, true), encoding: Encoding::Utf16Le };
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Decoded { text: utf16(rest, false), encoding: Encoding::Utf16Be };
    }
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Decoded { text: String::new(), encoding: Encoding::Binary };
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Decoded { text: s.to_owned(), encoding: Encoding::Utf8 },
        Err(_) => Decoded { text: String::from_utf8_lossy(bytes).into_owned(), encoding: Encoding::Utf8Lossy },
    }
}

fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 | Encoding::Utf8Lossy | Encoding::Binary => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        Encoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            v.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            v
        }
        Encoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            v.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            v
        }
    }
}

/// 문자 표시 폭(열). 탭은 `tab` 열, 동아시아 전각 문자는 2열.
pub(crate) fn char_cols(c: char, tab: usize) -> usize {
    match c {
        '\t' => tab,
        '\u{1100}'..='\u{115F}'
        | '\u{2E80}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}'
        | '\u{1F300}'..='\u{1FAFF}'
        | '\u{20000}'..='\u{3FFFD}' => 2,
        _ => 1,
    }
}

/// 커서 이동 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Up(usize),
    Down(usize),
    DocStart,
    DocEnd,
}

/// 스크롤 요청.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reveal {
    Nearest,
    Center,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 열린 파일 하나의 편집 상태.
pub struct Editor {
    id: egui::Id,
    path: PathBuf,
    pub(crate) buf: Buffer,
    pub(crate) hl: Highlighter,
    pub(crate) sel: Selection,
    /// 세로 이동 시 유지할 표시 열.
    pub(crate) preferred_col: Option<usize>,
    pub(crate) indent: Indent,
    comment: Option<CommentStyle>,
    language: String,
    encoding: Encoding,
    pub(crate) read_only: bool,
    large: bool,
    file_len: u64,
    disk: Option<DiskStamp>,
    /// 디스크 변경을 감지했지만 편집 중이라 반영하지 않은 상태.
    pub(crate) conflict: bool,
    pub(crate) deleted_on_disk: bool,
    pub(crate) large_banner: bool,
    pub(crate) save_error: Option<String>,
    pub(crate) find: FindState,
    pub(crate) goto: Option<String>,
    pub(crate) reveal: Option<Reveal>,
    pub(crate) view: view::ViewState,
    clock: Instant,
}

impl Editor {
    /// 파일을 연다. 이진 파일은 읽기 전용 자리표시로 연다.
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Editor> {
        let path: PathBuf = path.into();
        let meta = std::fs::metadata(&path).with_context(|| format!("{}을(를) 열 수 없습니다", path.display()))?;
        anyhow::ensure!(!meta.is_dir(), "{}은(는) 디렉터리입니다", path.display());
        let bytes = std::fs::read(&path).with_context(|| format!("{}을(를) 읽을 수 없습니다", path.display()))?;
        let decoded = decode(&bytes);
        let large = bytes.len() as u64 > LARGE_FILE_BYTES;
        let mut ed = Self::from_decoded(path.clone(), decoded, large);
        ed.file_len = bytes.len() as u64;
        ed.disk = DiskStamp::of(&path);
        Ok(ed)
    }

    /// 디스크 없이 텍스트로 편집기를 만든다. 경로는 언어 감지와 저장에 쓰인다.
    pub fn from_text(path: impl Into<PathBuf>, text: &str) -> Editor {
        let large = text.len() as u64 > LARGE_FILE_BYTES;
        Self::from_decoded(path.into(), Decoded { text: text.to_owned(), encoding: Encoding::Utf8 }, large)
    }

    fn from_decoded(path: PathBuf, d: Decoded, large: bool) -> Editor {
        let buf = Buffer::from_text(&d.text);
        let first = buf.line(0).to_owned();
        let syntax_ref = if large || d.encoding == Encoding::Binary { None } else { syntax::detect(&path, &first) };
        let language = syntax_ref.map_or_else(|| "Plain Text".to_owned(), |s| s.name.clone());
        let comment = syntax::comment_style(&language);
        let hl = Highlighter::new(syntax_ref, buf.line_count());
        let indent = buf.detect_indent();
        Editor {
            id: egui::Id::new(("kiln-editor", NEXT_ID.fetch_add(1, Ordering::Relaxed))),
            read_only: matches!(d.encoding, Encoding::Binary | Encoding::Utf8Lossy),
            path,
            buf,
            hl,
            sel: Selection::default(),
            preferred_col: None,
            indent,
            comment,
            language,
            encoding: d.encoding,
            large,
            file_len: d.text.len() as u64,
            disk: None,
            conflict: false,
            deleted_on_disk: false,
            large_banner: large,
            save_error: None,
            find: FindState::default(),
            goto: None,
            reveal: None,
            view: view::ViewState::default(),
            clock: Instant::now(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 탭 제목(파일 이름).
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn is_dirty(&self) -> bool {
        self.buf.is_dirty()
    }

    pub fn is_binary(&self) -> bool {
        self.encoding == Encoding::Binary
    }

    /// 큰 파일이라 구문 강조를 끈 상태인지.
    pub fn is_large(&self) -> bool {
        self.large
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// 현재 버퍼 전체 텍스트(원래 줄 끝 포함).
    pub fn text(&self) -> String {
        self.buf.to_text()
    }

    pub fn status(&self) -> EditorStatus {
        let head = self.sel.head;
        let line_text = self.buf.line(head.line);
        let (a, b) = self.sel.range();
        EditorStatus {
            line: head.line + 1,
            col: line_text[..head.col].chars().count() + 1,
            language: self.language.clone(),
            line_ending: self.buf.line_ending,
            encoding: self.encoding,
            dirty: self.is_dirty(),
            indent: self.indent,
            read_only: self.read_only,
            selected: if a == b { 0 } else { self.buf.text_range(a, b).chars().count() },
        }
    }

    /// 구문 강조가 확정된 줄 수.
    pub fn highlighted_lines(&self) -> usize {
        self.hl.valid_lines()
    }

    /// 다음 프레임부터 키보드 입력을 받도록 포커스를 요청한다.
    pub fn request_focus(&self, ctx: &egui::Context) {
        ctx.memory_mut(|m| m.request_focus(self.id));
    }

    /// 편집기 본문에 키보드 포커스가 있는지.
    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id))
    }

    /// 편집기 위젯의 egui Id.
    pub fn id(&self) -> egui::Id {
        self.id
    }

    /// 현재 선택 영역.
    pub fn selection(&self) -> Selection {
        self.sel
    }

    pub fn set_selection(&mut self, sel: Selection) {
        self.sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
        self.preferred_col = None;
        self.reveal = Some(Reveal::Nearest);
    }

    /// 1부터 시작하는 줄·열로 커서를 옮기고 화면 가운데로 스크롤한다.
    pub fn goto(&mut self, line: usize, col: usize) {
        let l = line.saturating_sub(1).min(self.buf.line_count() - 1);
        let text = self.buf.line(l);
        let byte = text.char_indices().nth(col.saturating_sub(1)).map_or(text.len(), |(i, _)| i);
        self.sel = Selection::caret(Pos::new(l, byte));
        self.preferred_col = None;
        self.reveal = Some(Reveal::Center);
    }

    /// 저장한다. 원래 인코딩, 줄 끝, 마지막 줄바꿈을 그대로 쓴다.
    pub fn save(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(self.encoding != Encoding::Binary, "바이너리 파일은 저장할 수 없습니다");
        anyhow::ensure!(!self.read_only, "읽기 전용 파일입니다");
        let bytes = encode(&self.buf.to_text(), self.encoding);
        let res = std::fs::write(&self.path, &bytes).with_context(|| format!("{}에 쓸 수 없습니다", self.path.display()));
        match res {
            Ok(()) => {
                self.buf.mark_saved();
                self.disk = DiskStamp::of(&self.path);
                self.file_len = bytes.len() as u64;
                self.conflict = false;
                self.deleted_on_disk = false;
                self.save_error = None;
                Ok(())
            }
            Err(e) => {
                self.save_error = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// 디스크의 파일이 바뀌었으면 편집 중이 아닐 때 다시 읽는다. 편집 중이면 충돌 배너를 띄운다.
    /// 다시 읽었거나 충돌 상태가 되었으면 `true`.
    pub fn reload_if_changed_on_disk(&mut self) -> bool {
        let now = DiskStamp::of(&self.path);
        if now.is_none() {
            let changed = !self.deleted_on_disk && self.disk.is_some();
            self.deleted_on_disk = self.disk.is_some();
            return changed;
        }
        self.deleted_on_disk = false;
        if now == self.disk {
            return false;
        }
        if self.is_dirty() {
            self.conflict = true;
            self.disk = now;
            return true;
        }
        self.reload_from_disk().is_ok()
    }

    /// 디스크 내용으로 버퍼를 바꾼다. 달라진 줄 구간만 교체해 되돌리기와 강조 캐시를 유지한다.
    pub fn reload_from_disk(&mut self) -> anyhow::Result<()> {
        let bytes = std::fs::read(&self.path)?;
        let d = decode(&bytes);
        self.disk = DiskStamp::of(&self.path);
        self.file_len = bytes.len() as u64;
        self.conflict = false;
        if d.encoding == Encoding::Binary || self.encoding == Encoding::Binary {
            *self = Self::open(self.path.clone())?;
            return Ok(());
        }
        self.encoding = d.encoding;
        self.read_only = d.encoding == Encoding::Utf8Lossy;
        let fresh = Buffer::from_text(&d.text);
        let old = self.buf.lines();
        let new = fresh.lines();
        let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
        let sel = self.sel;
        if prefix + suffix < old.len() || old.len() != new.len() {
            let a = Pos::new(prefix, 0);
            let old_last = old.len() - suffix;
            let b = if old_last > prefix { Pos::new(old_last - 1, old[old_last - 1].len()) } else { a };
            let mid = new[prefix..new.len() - suffix].join("\n");
            let (a, b, mid) = if old_last > prefix && new.len() - suffix > prefix {
                (a, b, mid)
            } else if old_last > prefix {
                // 줄 삭제만 있는 경우 앞/뒤 줄바꿈까지 함께 지운다.
                if prefix > 0 {
                    (Pos::new(prefix - 1, old[prefix - 1].len()), b, String::new())
                } else {
                    (a, Pos::new(old_last.min(old.len() - 1), 0), String::new())
                }
            } else if prefix < old.len() {
                (a, a, format!("{mid}\n"))
            } else {
                (Pos::new(prefix - 1, old[prefix - 1].len()), Pos::new(prefix - 1, old[prefix - 1].len()), format!("\n{mid}"))
            };
            self.buf.begin(EditKind::Other, sel, self.now());
            self.buf.replace(a, b, &mid);
            self.buf.end(sel);
            self.sync_highlighter();
        }
        self.buf.line_ending = fresh.line_ending;
        debug_assert_eq!(self.buf.to_text(), d.text.replace("\r\n", "\n").replace('\n', self.buf.line_ending.as_str()));
        self.buf.mark_saved();
        self.sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
        Ok(())
    }

    /// 충돌 배너에서 "내 변경 유지"를 고른 상태.
    pub fn keep_local_changes(&mut self) {
        self.conflict = false;
    }

    pub(crate) fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    fn sync_highlighter(&mut self) {
        for e in self.buf.take_changes() {
            self.hl.on_edit(e);
        }
        self.find.invalidate();
    }

    // ---- 편집 명령 ----

    fn edit(&mut self, kind: EditKind, f: impl FnOnce(&mut Self) -> Selection) {
        if self.read_only {
            return;
        }
        let before = self.sel;
        let now = self.now();
        self.buf.begin(kind, before, now);
        let after = f(self);
        self.sel = Selection::new(self.buf.clamp(after.anchor), self.buf.clamp(after.head));
        self.buf.end(self.sel);
        self.sync_highlighter();
        self.preferred_col = None;
        self.reveal = Some(Reveal::Nearest);
    }

    /// 선택 영역을 텍스트로 바꾼다(타이핑).
    pub fn insert_text(&mut self, text: &str) {
        let kind = if text.chars().count() == 1 && !text.contains('\n') { EditKind::Typing } else { EditKind::Other };
        self.edit(kind, |ed| {
            let (a, b) = ed.sel.range();
            let end = ed.buf.replace(a, b, text);
            Selection::caret(end)
        });
    }

    /// 줄을 나누고 들여쓰기를 이어 간다. 여는 괄호 뒤면 한 단계 더 들여쓴다.
    pub fn newline(&mut self) {
        let unit = self.indent.unit();
        let colon_indents = matches!(self.language.as_str(), "Python" | "YAML" | "Nim");
        self.edit(EditKind::Other, |ed| {
            let (a, b) = ed.sel.range();
            let line = ed.buf.line(a.line).to_owned();
            let base = line[..ed.buf.first_non_ws(a.line).min(a.col)].to_owned();
            let before = line[..a.col].trim_end();
            let after_full = ed.buf.line(b.line)[b.col..].to_owned();
            let after = after_full.trim_start();
            let last = before.chars().last();
            let opens = matches!(last, Some('{' | '[' | '('))
                || (colon_indents && last == Some(':'))
                || (before.ends_with("=>") && ed.language.contains("Script"));
            let closes = matches!(
                (last, after.chars().next()),
                (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
            );
            // 커서 앞 공백과 커서 뒤 선행 공백은 지운다.
            let a = Pos::new(a.line, before.len());
            let b = Pos::new(b.line, b.col + (after_full.len() - after.len()));
            let inner = if opens { format!("{base}{unit}") } else { base.clone() };
            if closes {
                ed.buf.replace(a, b, &format!("\n{inner}\n{base}"));
                Selection::caret(Pos::new(a.line + 1, inner.len()))
            } else {
                Selection::caret(ed.buf.replace(a, b, &format!("\n{inner}")))
            }
        });
    }

    /// 선택된 줄들 번호(첫 줄, 끝 줄). 끝 위치가 줄 맨 앞이면 그 줄은 제외.
    pub(crate) fn selected_lines(&self) -> (usize, usize) {
        let (a, b) = self.sel.range();
        let last = if b.line > a.line && b.col == 0 { b.line - 1 } else { b.line };
        (a.line, last)
    }

    fn display_col(&self, p: Pos) -> usize {
        let tab = self.indent.width();
        self.buf.line(p.line)[..p.col].chars().map(|c| char_cols(c, tab)).sum()
    }

    fn col_for_display(&self, line: usize, target: usize) -> usize {
        let tab = self.indent.width();
        let mut w = 0;
        for (i, c) in self.buf.line(line).char_indices() {
            let cw = char_cols(c, tab);
            if w + cw > target {
                return if target - w > cw / 2 { i + c.len_utf8() } else { i };
            }
            w += cw;
        }
        self.buf.line(line).len()
    }

    /// Tab: 여러 줄 선택이면 들여쓰기, 아니면 다음 탭 위치까지 공백(또는 탭)을 넣는다.
    pub fn indent_or_tab(&mut self) {
        let (a, b) = self.sel.range();
        if a.line != b.line {
            self.indent_lines(true);
            return;
        }
        let text = match self.indent {
            Indent::Tabs => "\t".to_owned(),
            Indent::Spaces(n) => {
                let n = n as usize;
                " ".repeat(n - self.display_col(a) % n)
            }
        };
        self.edit(EditKind::Other, |ed| {
            let (a, b) = ed.sel.range();
            Selection::caret(ed.buf.replace(a, b, &text))
        });
    }

    /// 선택된 줄들을 한 단계 들여쓰거나(`true`) 내어쓴다.
    pub fn indent_lines(&mut self, indent: bool) {
        let (first, last) = self.selected_lines();
        let unit = self.indent.unit();
        let width = self.indent.width();
        self.edit(EditKind::Other, |ed| {
            let mut sel = ed.sel;
            for l in first..=last {
                let shift = |p: &mut Pos, at: usize, d: isize| {
                    if p.line == l && p.col >= at {
                        p.col = (p.col as isize + d).max(at as isize) as usize;
                    }
                };
                if indent {
                    if ed.buf.line(l).is_empty() {
                        continue;
                    }
                    ed.buf.insert(Pos::new(l, 0), &unit);
                    shift(&mut sel.anchor, 0, unit.len() as isize);
                    shift(&mut sel.head, 0, unit.len() as isize);
                    if sel.anchor.line == l && sel.anchor.col == unit.len() && sel.anchor != sel.head && ed.sel.anchor.col == 0 {
                        sel.anchor.col = 0;
                    }
                } else {
                    let line = ed.buf.line(l);
                    let n = if line.starts_with('\t') {
                        1
                    } else {
                        line.bytes().take(width).take_while(|&c| c == b' ').count()
                    };
                    if n > 0 {
                        ed.buf.replace(Pos::new(l, 0), Pos::new(l, n), "");
                        shift(&mut sel.anchor, 0, -(n as isize));
                        shift(&mut sel.head, 0, -(n as isize));
                    }
                }
            }
            sel
        });
    }

    /// Backspace. `word` 면 이전 단어까지, `line` 이면 줄 처음까지 지운다.
    pub fn backspace(&mut self, word: bool, line: bool) {
        let width = self.indent.width();
        let spaces = matches!(self.indent, Indent::Spaces(_));
        self.edit(EditKind::Delete, |ed| {
            let (a, b) = ed.sel.range();
            if a != b {
                ed.buf.replace(a, b, "");
                return Selection::caret(a);
            }
            let from = if line {
                if a.col == 0 { ed.buf.prev_char(a) } else { Pos::new(a.line, 0) }
            } else if word {
                ed.buf.prev_word(a)
            } else {
                let text = ed.buf.line(a.line);
                let lead = &text[..a.col];
                if spaces && a.col > 0 && lead.bytes().all(|c| c == b' ') {
                    let n = a.col % width;
                    Pos::new(a.line, a.col - if n == 0 { width } else { n })
                } else {
                    ed.buf.prev_char(a)
                }
            };
            ed.buf.replace(from, a, "");
            Selection::caret(from)
        });
    }

    /// Delete. `word` 면 다음 단어 끝까지 지운다.
    pub fn delete_forward(&mut self, word: bool) {
        self.edit(EditKind::Delete, |ed| {
            let (a, b) = ed.sel.range();
            if a != b {
                ed.buf.replace(a, b, "");
                return Selection::caret(a);
            }
            let to = if word { ed.buf.next_word(a) } else { ed.buf.next_char(a) };
            ed.buf.replace(a, to, "");
            Selection::caret(a)
        });
    }

    /// 선택된 줄들의 줄 주석을 토글한다.
    pub fn toggle_comment(&mut self) {
        let Some(style) = self.comment else { return };
        let (first, last) = self.selected_lines();
        self.edit(EditKind::Other, |ed| {
            let shifts = ed.buf.toggle_comment(first, last, style);
            let mut sel = ed.sel;
            for (l, at, d) in shifts {
                for p in [&mut sel.anchor, &mut sel.head] {
                    if p.line == l && p.col >= at {
                        p.col = (p.col as isize + d).max(at as isize) as usize;
                    }
                }
            }
            sel
        });
    }

    /// 현재 줄(또는 선택된 줄들)을 지운다.
    pub fn delete_lines(&mut self) {
        let (first, last) = self.selected_lines();
        self.edit(EditKind::Other, |ed| {
            let n = ed.buf.line_count();
            let (a, b) = if last + 1 < n {
                (Pos::new(first, 0), Pos::new(last + 1, 0))
            } else if first > 0 {
                (Pos::new(first - 1, ed.buf.line(first - 1).len()), Pos::new(last, ed.buf.line(last).len()))
            } else {
                (Pos::new(0, 0), Pos::new(last, ed.buf.line(last).len()))
            };
            ed.buf.replace(a, b, "");
            let l = first.min(ed.buf.line_count() - 1);
            Selection::caret(Pos::new(l, ed.buf.first_non_ws(l)))
        });
    }

    /// 선택된 줄들을 위(`true`)나 아래로 한 줄 옮긴다.
    pub fn move_lines(&mut self, up: bool) {
        let (first, last) = self.selected_lines();
        let n = self.buf.line_count();
        if (up && first == 0) || (!up && last + 1 >= n) {
            return;
        }
        self.edit(EditKind::Other, |ed| {
            let block = ed.buf.text_range(Pos::new(first, 0), Pos::new(last, ed.buf.line(last).len()));
            let mut sel = ed.sel;
            if up {
                let above = ed.buf.line(first - 1).to_owned();
                ed.buf.replace(Pos::new(first - 1, 0), Pos::new(last, ed.buf.line(last).len()), &format!("{block}\n{above}"));
                sel.anchor.line -= 1;
                sel.head.line -= 1;
            } else {
                let below = ed.buf.line(last + 1).to_owned();
                ed.buf.replace(Pos::new(first, 0), Pos::new(last + 1, ed.buf.line(last + 1).len()), &format!("{below}\n{block}"));
                sel.anchor.line += 1;
                sel.head.line += 1;
            }
            sel
        });
    }

    pub fn undo(&mut self) {
        if self.read_only {
            return;
        }
        if let Some(sel) = self.buf.undo() {
            self.sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
            self.sync_highlighter();
            self.reveal = Some(Reveal::Nearest);
        }
    }

    pub fn redo(&mut self) {
        if self.read_only {
            return;
        }
        if let Some(sel) = self.buf.redo() {
            self.sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
            self.sync_highlighter();
            self.reveal = Some(Reveal::Nearest);
        }
    }

    pub fn select_all(&mut self) {
        self.sel = Selection::new(Pos::default(), self.buf.end_pos());
    }

    /// 복사할 텍스트. 선택이 없으면 현재 줄 전체(줄바꿈 포함).
    pub fn copy_text(&self) -> String {
        let (a, b) = self.sel.range();
        if a == b {
            format!("{}\n", self.buf.line(a.line))
        } else {
            self.buf.text_range(a, b)
        }
    }

    /// 잘라내고 잘라낸 텍스트를 돌려준다. 선택이 없으면 현재 줄을 잘라낸다.
    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.sel.is_empty() {
            self.delete_lines();
        } else {
            self.edit(EditKind::Other, |ed| {
                let (a, b) = ed.sel.range();
                ed.buf.replace(a, b, "");
                Selection::caret(a)
            });
        }
        text
    }

    pub fn paste(&mut self, text: &str) {
        self.edit(EditKind::Other, |ed| {
            let (a, b) = ed.sel.range();
            Selection::caret(ed.buf.replace(a, b, text))
        });
    }

    /// 커서를 움직인다. `extend` 면 선택을 넓힌다.
    pub(crate) fn move_caret(&mut self, m: Motion, extend: bool) {
        let (a, b) = self.sel.range();
        let head = self.sel.head;
        let collapse_to = |p: Pos| if extend { None } else { Some(p) };
        let target = match m {
            Motion::Left => collapse_to(a).filter(|_| a != b).unwrap_or_else(|| self.buf.prev_char(head)),
            Motion::Right => collapse_to(b).filter(|_| a != b).unwrap_or_else(|| self.buf.next_char(head)),
            Motion::WordLeft => self.buf.prev_word(head),
            Motion::WordRight => self.buf.next_word(head),
            Motion::Home => {
                let fnw = self.buf.first_non_ws(head.line);
                Pos::new(head.line, if head.col == fnw { 0 } else { fnw })
            }
            Motion::End => Pos::new(head.line, self.buf.line(head.line).len()),
            Motion::Up(n) | Motion::Down(n) => {
                let pref = self.preferred_col.unwrap_or_else(|| self.display_col(head));
                let last = self.buf.line_count() - 1;
                let line = if matches!(m, Motion::Up(_)) { head.line.saturating_sub(n) } else { (head.line + n).min(last) };
                let p = if matches!(m, Motion::Up(_)) && head.line == 0 {
                    Pos::new(0, 0)
                } else if matches!(m, Motion::Down(_)) && head.line == last {
                    Pos::new(last, self.buf.line(last).len())
                } else {
                    Pos::new(line, self.col_for_display(line, pref))
                };
                self.sel = if extend { Selection::new(self.sel.anchor, p) } else { Selection::caret(p) };
                self.preferred_col = Some(pref);
                self.reveal = Some(Reveal::Nearest);
                return;
            }
            Motion::DocStart => Pos::default(),
            Motion::DocEnd => self.buf.end_pos(),
        };
        self.sel = if extend { Selection::new(self.sel.anchor, target) } else { Selection::caret(target) };
        self.preferred_col = None;
        self.reveal = Some(Reveal::Nearest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_text("t.rs", text)
    }

    #[test]
    fn newline_keeps_indent_and_indents_after_open_brace() {
        let mut e = ed("fn a() {}");
        e.sel = Selection::caret(Pos::new(0, 8));
        e.newline();
        assert_eq!(e.text(), "fn a() {\n    \n}");
        assert_eq!(e.sel.head, Pos::new(1, 4));
        e.insert_text("x");
        e.newline();
        assert_eq!(e.text(), "fn a() {\n    x\n    \n}");
    }

    #[test]
    fn newline_in_plain_indented_line() {
        let mut e = ed("    let a = 1;");
        e.sel = Selection::caret(Pos::new(0, 14));
        e.newline();
        assert_eq!(e.text(), "    let a = 1;\n    ");
    }

    #[test]
    fn tab_inserts_spaces_to_next_stop_and_indents_selection() {
        let mut e = ed("ab\ncd");
        e.sel = Selection::caret(Pos::new(0, 1));
        e.indent_or_tab();
        assert_eq!(e.text(), "a   b\ncd");
        e.sel = Selection::new(Pos::new(0, 0), Pos::new(1, 1));
        e.indent_or_tab();
        assert_eq!(e.text(), "    a   b\n    cd");
        e.indent_lines(false);
        assert_eq!(e.text(), "a   b\ncd");
    }

    #[test]
    fn tab_uses_tab_char_for_tab_indented_files() {
        let mut e = ed("a {\n\tb\n\tc\n}");
        e.sel = Selection::caret(Pos::new(1, 1));
        e.indent_or_tab();
        assert_eq!(e.buf.line(1), "\t\tb");
    }

    #[test]
    fn backspace_in_leading_spaces_removes_one_indent_level() {
        let mut e = ed("        x");
        e.indent = Indent::Spaces(4);
        e.sel = Selection::caret(Pos::new(0, 8));
        e.backspace(false, false);
        assert_eq!(e.text(), "    x");
        e.sel = Selection::caret(Pos::new(0, 5));
        e.backspace(false, false);
        assert_eq!(e.text(), "    ");
    }

    #[test]
    fn toggle_comment_uses_language_token_and_keeps_caret() {
        let mut e = ed("let a = 1;\nlet b = 2;");
        e.sel = Selection::caret(Pos::new(0, 4));
        e.toggle_comment();
        assert_eq!(e.text(), "// let a = 1;\nlet b = 2;");
        assert_eq!(e.sel.head, Pos::new(0, 7));
        e.toggle_comment();
        assert_eq!(e.text(), "let a = 1;\nlet b = 2;");
        let mut py = Editor::from_text("x.py", "a = 1");
        py.toggle_comment();
        assert_eq!(py.text(), "# a = 1");
    }

    #[test]
    fn vertical_motion_keeps_preferred_column() {
        let mut e = ed("abcdef\nab\nabcdef");
        e.sel = Selection::caret(Pos::new(0, 5));
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(1, 2));
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(2, 5));
    }

    #[test]
    fn move_and_delete_lines() {
        let mut e = ed("a\nb\nc");
        e.sel = Selection::caret(Pos::new(1, 0));
        e.move_lines(true);
        assert_eq!(e.text(), "b\na\nc");
        e.move_lines(false);
        assert_eq!(e.text(), "a\nb\nc");
        e.delete_lines();
        assert_eq!(e.text(), "a\nc");
        e.undo();
        assert_eq!(e.text(), "a\nb\nc");
    }

    #[test]
    fn copy_without_selection_copies_line() {
        let mut e = ed("one\ntwo");
        e.sel = Selection::caret(Pos::new(1, 1));
        assert_eq!(e.copy_text(), "two\n");
        let cut = e.cut();
        assert_eq!(cut, "two\n");
        assert_eq!(e.text(), "one");
    }

    #[test]
    fn save_preserves_crlf_trailing_newline_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        let original = b"\xEF\xBB\xBFone\r\ntwo\r\n".to_vec();
        std::fs::write(&p, &original).unwrap();
        let mut e = Editor::open(&p).unwrap();
        assert_eq!(e.status().line_ending, LineEnding::CrLf);
        assert_eq!(e.status().encoding, Encoding::Utf8Bom);
        e.save().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), original);
        e.sel = Selection::caret(Pos::new(0, 3));
        e.insert_text("!");
        e.save().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"\xEF\xBB\xBFone!\r\ntwo\r\n");
        assert!(!e.is_dirty());
    }

    #[test]
    fn utf16_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("u.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("héllo\n".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&p, &bytes).unwrap();
        let mut e = Editor::open(&p).unwrap();
        assert_eq!(e.text(), "héllo\n");
        e.save().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
    }

    #[test]
    fn binary_and_invalid_utf8_detection() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2, 3, 0, 5]).unwrap();
        let e = Editor::open(&bin).unwrap();
        assert!(e.is_binary());
        assert!(e.is_read_only());
        let bad = dir.path().join("bad.txt");
        std::fs::write(&bad, b"ok \xFF\xFE bytes").unwrap();
        let mut e = Editor::open(&bad).unwrap();
        assert_eq!(e.status().encoding, Encoding::Utf8Lossy);
        assert!(e.is_read_only());
        e.insert_text("x");
        assert!(!e.is_dirty());
        assert!(e.save().is_err());
    }

    #[test]
    fn large_file_disables_highlighting() {
        let big = "x\n".repeat((LARGE_FILE_BYTES as usize) / 2 + 10);
        let e = Editor::from_text("big.rs", &big);
        assert!(e.hl.is_plain());
        assert_eq!(e.status().language, "Plain Text");
    }

    #[test]
    fn reload_when_clean_and_conflict_when_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.rs");
        std::fs::write(&p, "a\nb\nc\n").unwrap();
        let mut e = Editor::open(&p).unwrap();
        e.goto(3, 1);
        // 길이가 다른 내용으로 덮어쓴다.
        std::fs::write(&p, "a\nB changed\nc\nd\n").unwrap();
        assert!(e.reload_if_changed_on_disk());
        assert_eq!(e.text(), "a\nB changed\nc\nd\n");
        assert!(!e.is_dirty());
        assert_eq!(e.status().line, 3);
        e.insert_text("x");
        std::fs::write(&p, "totally different\n").unwrap();
        assert!(e.reload_if_changed_on_disk());
        assert!(e.conflict);
        assert!(e.text().contains('x'));
        e.reload_from_disk().unwrap();
        assert_eq!(e.text(), "totally different\n");
        assert!(!e.conflict);
    }

    #[test]
    fn reload_handles_pure_line_deletions_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.txt");
        for (from, to) in [
            ("a\nb\nc\n", "a\nc\n"),
            ("a\nb\nc\n", "b\nc\n"),
            ("a\nb\nc", "a\nb"),
            ("a\nb", "a\nb\nc\nd"),
            ("a\nb", "x\na\nb"),
            ("a\nb\n", "a\nb\n\n"),
            ("", "hello"),
            ("hello\n", ""),
        ] {
            std::fs::write(&p, from).unwrap();
            let mut e = Editor::open(&p).unwrap();
            std::fs::write(&p, to).unwrap();
            e.reload_from_disk().unwrap();
            assert_eq!(e.text(), to, "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn goto_is_one_based_and_clamped() {
        let mut e = ed("abc\ndef");
        e.goto(2, 3);
        assert_eq!(e.status().line, 2);
        assert_eq!(e.status().col, 3);
        e.goto(99, 99);
        assert_eq!(e.sel.head, Pos::new(1, 3));
    }
}
