//! 파일 하나를 여는 코드 편집기.

mod cursors;
mod display;
mod find;
mod fold;
mod lsp_glue;
mod view;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};

use anyhow::Context as _;

use crate::EditorEvent;
use crate::buffer::{Buffer, EditKind, Indent, Pos, Selection};
use crate::highlight::Highlighter;
use crate::syntax::{self, CommentStyle};

pub use crate::buffer::LineEnding;
pub(crate) use cursors::{ColumnSel, Cursor};
pub(crate) use display::DisplayMap;
pub(crate) use find::FindState;
pub use fold::FoldRange;
pub(crate) use fold::FoldState;

/// 이 크기를 넘는 파일은 구문 강조를 끈다.
pub const LARGE_FILE_BYTES: u64 = 5 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

/// 파일 인코딩.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// 선택된 문자 수(모든 커서 합). 선택이 없으면 0.
    pub selected: usize,
    /// 커서 수(주 커서 포함).
    pub cursors: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Separate recovery content. Restoring this never writes the original file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditorDraft {
    pub text: String,
    encoding: Encoding,
    disk: Option<DiskStamp>,
    #[serde(default)]
    conflict: bool,
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
    /// 주 커서.
    pub(crate) sel: Selection,
    /// 보조 커서(문서 순서).
    pub(crate) extra: Vec<Cursor>,
    /// 세로 이동 시 유지할 표시 열.
    pub(crate) preferred_col: Option<usize>,
    /// 주 커서 헤드가 이 위치면 줄 바꿈 지점의 커서를 앞 시각 줄 끝에 둔다.
    pub(crate) row_end_at: Option<Pos>,
    /// 마지막 사각 선택과 그때의 커서 전체.
    pub(crate) column: Option<(ColumnSel, Vec<Selection>)>,
    /// 다음 편집 그룹의 편집 전 커서 전체. 없으면 현재 커서를 쓴다.
    edit_before: Option<Vec<Selection>>,
    /// Cmd+D 가 단어 단위로 일치를 찾는지.
    pub(crate) whole_word_next: bool,
    pub(crate) display: DisplayMap,
    pub(crate) folds: FoldState,
    pub(crate) word_wrap: bool,
    pub(crate) events: Vec<EditorEvent>,
    pub(crate) lsp: Option<lsp_glue::LspState>,
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
    pub(crate) save_copy_notice: Option<String>,
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
            hl,
            sel: Selection::default(),
            extra: Vec::new(),
            preferred_col: None,
            row_end_at: None,
            column: None,
            edit_before: None,
            whole_word_next: false,
            display: DisplayMap::new(buf.line_count()),
            folds: FoldState::default(),
            word_wrap: false,
            events: Vec::new(),
            lsp: None,
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
            save_copy_notice: None,
            find: FindState::default(),
            goto: None,
            reveal: None,
            view: view::ViewState::default(),
            clock: Instant::now(),
            buf,
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
            selected: if a == b { 0 } else { self.buf.text_range(a, b).chars().count() }
                + self
                    .extra
                    .iter()
                    .map(|c| {
                        let (a, b) = c.sel.range();
                        if a == b { 0 } else { self.buf.text_range(a, b).chars().count() }
                    })
                    .sum::<usize>(),
            cursors: self.cursor_count(),
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
        self.extra.clear();
        self.sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
        self.preferred_col = None;
        self.row_end_at = None;
        self.reveal = Some(Reveal::Nearest);
    }

    /// 1부터 시작하는 줄·열로 커서를 옮기고 화면 가운데로 스크롤한다.
    pub fn goto(&mut self, line: usize, col: usize) {
        let l = line.saturating_sub(1).min(self.buf.line_count() - 1);
        let text = self.buf.line(l);
        let byte = text.char_indices().nth(col.saturating_sub(1)).map_or(text.len(), |(i, _)| i);
        self.extra.clear();
        self.sel = Selection::caret(Pos::new(l, byte));
        self.preferred_col = None;
        self.reveal = Some(Reveal::Center);
        self.unfold_around_cursors();
    }

    /// 자동 줄 바꿈을 켜거나 끈다. 줄 바꿈 폭은 다음 프레임에 화면 너비로 정해진다.
    pub fn set_word_wrap(&mut self, on: bool) {
        if self.word_wrap != on {
            self.word_wrap = on;
            if !on {
                self.set_wrap_cols(0);
            }
            self.reveal = Some(Reveal::Nearest);
        }
    }

    pub fn word_wrap(&self) -> bool {
        self.word_wrap
    }

    pub fn toggle_word_wrap(&mut self) {
        self.set_word_wrap(!self.word_wrap);
    }

    /// 줄 바꿈 폭을 열 수로 정한다. 0 이면 줄 바꿈 없음.
    pub(crate) fn set_wrap_cols(&mut self, cols: usize) {
        let tab = self.indent.width();
        self.display.set_wrap(self.buf.lines(), cols, tab);
    }

    /// 쌓인 사건(정의로 이동 등)을 꺼낸다.
    pub fn take_events(&mut self) -> Vec<EditorEvent> {
        std::mem::take(&mut self.events)
    }

    /// Capture unsaved content and the original disk identity for conflict detection.
    pub fn recovery_draft(&self) -> Option<EditorDraft> {
        self.is_dirty().then(|| EditorDraft { text: self.text(), encoding: self.encoding, disk: self.disk, conflict: self.conflict })
    }

    /// Apply a recovered draft as an undoable, dirty edit. Never silently overwrite
    /// a file changed externally since the snapshot was recorded.
    pub fn restore_draft(&mut self, draft: &EditorDraft) {
        let conflict = draft.conflict || DiskStamp::of(&self.path) != draft.disk;
        self.read_only = false;
        self.select_all();
        self.insert_text(&draft.text);
        self.encoding = draft.encoding;
        self.buf.line_ending = Buffer::from_text(&draft.text).line_ending;
        self.read_only = matches!(self.encoding, Encoding::Binary | Encoding::Utf8Lossy);
        self.conflict = conflict;
        self.deleted_on_disk = !self.path.exists();
    }

    pub fn recovery_conflict(&self) -> bool { self.conflict }

    /// Save a rescue copy without changing the original document or its dirty state.
    pub fn save_copy(&self,path:&Path)->anyhow::Result<()> {
        anyhow::ensure!(self.encoding!=Encoding::Binary,"바이너리 파일은 저장할 수 없습니다");
        anyhow::ensure!(path!=self.path,"복사본은 다른 경로에 저장하세요");
        if let (Ok(source),Ok(target))=(std::fs::canonicalize(&self.path),std::fs::canonicalize(path)){anyhow::ensure!(source!=target,"복사본은 원본과 다른 파일에 저장하세요");}
        kiln_common::safe_file::write(path,&encode(&self.buf.to_text(),self.encoding))?;Ok(())
    }
    fn choose_save_copy(&mut self) {
        let name=self.path.file_name().unwrap_or_default().to_string_lossy();
        if let Some(path)=rfd::FileDialog::new().set_file_name(format!("copy-{name}")).save_file() {
            match self.save_copy(&path){Ok(())=>{self.save_error=None;self.save_copy_notice=Some(format!("복사본 저장됨: {} · 원본의 미저장 변경은 유지됩니다",path.display()));},Err(e)=>self.save_error=Some(format!("복사본 저장 실패: {e}"))}
        }
    }

    /// 저장한다. 원래 인코딩, 줄 끝, 마지막 줄바꿈을 그대로 쓴다.
    pub fn save(&mut self) -> anyhow::Result<()> {
        if DiskStamp::of(&self.path) != self.disk { self.conflict = true; }
        if self.conflict {
            self.save_error = Some("디스크 파일이 변경되었습니다. 복원된 내용과 디스크를 검토한 뒤 ‘내 편집 유지’를 선택하세요.".into());
            anyhow::bail!("외부 변경 충돌을 먼저 해결하세요");
        }
        anyhow::ensure!(self.encoding != Encoding::Binary, "바이너리 파일은 저장할 수 없습니다");
        anyhow::ensure!(!self.read_only, "읽기 전용 파일입니다");
        let bytes = encode(&self.buf.to_text(), self.encoding);
        let res = kiln_common::safe_file::write(&self.path, &bytes).with_context(|| format!("{}에 쓸 수 없습니다", self.path.display()));
        match res {
            Ok(()) => {
                self.buf.mark_saved();
                self.disk = DiskStamp::of(&self.path);
                self.file_len = bytes.len() as u64;
                self.conflict = false;
                self.deleted_on_disk = false;
                self.save_error = None;
                self.save_copy_notice = None;
                self.lsp_did_save();
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
            let mgr = self.lsp.take().map(|s| s.manager());
            *self = Self::open(self.path.clone())?;
            if let Some(m) = mgr {
                self.set_lsp(m);
            }
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
            let snap = self.cursor_snapshot();
            self.buf.begin(EditKind::Other, &snap, self.now());
            self.buf.replace(a, b, &mid);
            self.buf.end(&snap);
            self.sync_highlighter();
        }
        self.buf.line_ending = fresh.line_ending;
        debug_assert_eq!(self.buf.to_text(), d.text.replace("\r\n", "\n").replace('\n', self.buf.line_ending.as_str()));
        self.buf.mark_saved();
        let snap = self.cursor_snapshot();
        self.restore_cursors(&snap);
        Ok(())
    }

    /// 충돌 배너에서 "내 편집 유지"를 고른 상태.
    pub fn keep_local_changes(&mut self) {
        self.disk = DiskStamp::of(&self.path);
        self.conflict = false;
    }

    pub(crate) fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    fn sync_highlighter(&mut self) {
        self.sync_line_edits();
        self.apply_folds();
        self.find.invalidate();
    }

    /// 쌓인 줄 변경을 강조·화면 줄·접기·진단 위치에 반영한다. 한 그룹 안에서 편집할 때마다 불러
    /// 화면 줄 대응이 그 시점의 버퍼 내용으로 계산되게 한다.
    pub(crate) fn sync_line_edits(&mut self) {
        for e in self.buf.take_changes() {
            self.hl.on_edit(e);
            self.display.on_edit(e, self.buf.lines());
            self.folds.on_edit(e);
            self.lsp_on_line_edit(e);
        }
    }

    // ---- 편집 명령 ----

    /// 편집 한 번을 되돌리기 한 단계로 묶는다. 커서가 여럿이면 문서 뒤쪽 커서부터 `f` 를 부르고
    /// 앞쪽 편집만큼 뒤쪽 결과 위치를 옮긴다.
    fn edit(&mut self, kind: EditKind, mut f: impl FnMut(&mut Self) -> Selection) {
        if self.read_only {
            self.edit_before = None;
            return;
        }
        let before = self.edit_before.take().unwrap_or_else(|| self.cursor_snapshot());
        let now = self.now();
        self.buf.begin(kind, &before, now);
        self.row_end_at = None;
        if self.extra.is_empty() {
            let after = f(self);
            self.sel = Selection::new(self.buf.clamp(after.anchor), self.buf.clamp(after.head));
        } else {
            let primary = self.sel;
            let mut all: Vec<(Selection, bool)> = self.extra.drain(..).map(|c| (c.sel, false)).collect();
            all.push((primary, true));
            all.sort_by_key(|(s, _)| s.range());
            let mut out: Vec<(Selection, bool)> = Vec::with_capacity(all.len());
            for &(s, prim) in all.iter().rev() {
                self.sel = s;
                let mark = self.buf.edit_mark();
                let r = f(self);
                self.sync_line_edits();
                for (o, _) in &mut out {
                    *o = Selection::new(self.buf.map_since(mark, o.anchor), self.buf.map_since(mark, o.head));
                }
                out.push((r, prim));
            }
            self.extra.clear();
            for (s, prim) in out {
                let s = Selection::new(self.buf.clamp(s.anchor), self.buf.clamp(s.head));
                if prim {
                    self.sel = s;
                } else {
                    self.extra.push(Cursor::new(s));
                }
            }
            self.preferred_col = None;
            self.normalize_cursors();
        }
        self.finish_edit();
    }

    /// 열린 편집 그룹을 지금 커서 전체로 닫고 화면 상태를 맞춘다.
    fn finish_edit(&mut self) {
        let after = self.cursor_snapshot();
        self.buf.end(&after);
        self.sync_highlighter();
        self.preferred_col = None;
        self.row_end_at = None;
        self.reveal = Some(Reveal::Nearest);
    }

    /// 여러 버퍼 편집을 되돌리기 한 단계로 묶는다. `f` 는 그룹의 편집 기준점을 받아 편집하고
    /// 편집 후 커서 전체(첫 항목이 주 커서)를 돌려준다.
    fn edit_group(&mut self, f: impl FnOnce(&mut Self, usize) -> Vec<Selection>) {
        if self.read_only {
            self.edit_before = None;
            return;
        }
        let before = self.edit_before.take().unwrap_or_else(|| self.cursor_snapshot());
        let now = self.now();
        self.buf.begin(EditKind::Other, &before, now);
        let mark = self.buf.edit_mark();
        let after = f(self, mark);
        self.restore_cursors(&after);
        self.finish_edit();
    }

    /// 편집 기준점 `mark` 이후의 편집을 거쳐 커서 전체를 옮긴 목록.
    fn mapped_cursors(&self, mark: usize) -> Vec<Selection> {
        let map = |p: Pos| self.buf.clamp(self.buf.map_since(mark, p));
        self.cursor_snapshot().into_iter().map(|s| Selection::new(map(s.anchor), map(s.head))).collect()
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
        sel_lines(self.sel)
    }

    /// 모든 커서가 걸친 줄 구간. 겹치거나 맞닿은 구간은 합친다. 문서 순서.
    pub(crate) fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut spans: Vec<(usize, usize)> = self.cursors().into_iter().map(sel_lines).collect();
        spans.sort();
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
        for (a, b) in spans {
            match out.last_mut() {
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// 모든 커서가 걸친 줄 번호(중복 없이 오름차순).
    fn cursor_lines(&self) -> Vec<usize> {
        self.line_blocks().into_iter().flat_map(|(a, b)| a..=b).collect()
    }

    pub(crate) fn display_col(&self, p: Pos) -> usize {
        let tab = self.indent.width();
        self.buf.line(p.line)[..p.col].chars().map(|c| char_cols(c, tab)).sum()
    }

    /// Tab: 여러 줄 선택이면 들여쓰기, 아니면 다음 탭 위치까지 공백(또는 탭)을 넣는다.
    pub fn indent_or_tab(&mut self) {
        if self.cursors().iter().any(|s| s.anchor.line != s.head.line) {
            self.indent_lines(true);
            return;
        }
        self.edit(EditKind::Other, |ed| {
            let (a, b) = ed.sel.range();
            let text = match ed.indent {
                Indent::Tabs => "\t".to_owned(),
                Indent::Spaces(n) => {
                    let n = n as usize;
                    " ".repeat(n - ed.display_col(a) % n)
                }
            };
            Selection::caret(ed.buf.replace(a, b, &text))
        });
    }

    /// 모든 커서가 걸친 줄들을 한 단계 들여쓰거나(`true`) 내어쓴다.
    pub fn indent_lines(&mut self, indent: bool) {
        let lines = self.cursor_lines();
        let unit = self.indent.unit();
        let width = self.indent.width();
        self.edit_group(|ed, mark| {
            let mut touched = Vec::new();
            for &l in &lines {
                if indent {
                    if ed.buf.line(l).is_empty() {
                        continue;
                    }
                    ed.buf.insert(Pos::new(l, 0), &unit);
                    touched.push(l);
                } else {
                    let line = ed.buf.line(l);
                    let n = if line.starts_with('\t') {
                        1
                    } else {
                        line.bytes().take(width).take_while(|&c| c == b' ').count()
                    };
                    if n > 0 {
                        ed.buf.replace(Pos::new(l, 0), Pos::new(l, n), "");
                    }
                }
            }
            ed.keep_line_starts(mark, &touched)
        });
    }

    /// `mark` 이후 편집으로 옮긴 커서 전체. 선택의 시작이 `lines` 중 한 줄의 맨 앞이었으면 맨 앞에 남긴다.
    fn keep_line_starts(&self, mark: usize, lines: &[usize]) -> Vec<Selection> {
        let before = self.cursor_snapshot();
        let mut after = self.mapped_cursors(mark);
        for (b, a) in before.iter().zip(after.iter_mut()) {
            if b.is_empty() {
                continue;
            }
            let fwd = b.anchor <= b.head;
            let start = if fwd { b.anchor } else { b.head };
            if start.col == 0 && lines.contains(&start.line) {
                let p = if fwd { &mut a.anchor } else { &mut a.head };
                p.col = 0;
            }
        }
        after
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

    /// 모든 커서가 걸친 줄들의 줄 주석을 한 번에 토글한다.
    pub fn toggle_comment(&mut self) {
        let Some(style) = self.comment else { return };
        let lines = self.cursor_lines();
        self.edit_group(|ed, mark| {
            let shifts = ed.buf.toggle_comment_lines(&lines, style);
            let added: Vec<usize> = shifts.iter().filter(|s| s.2 > 0 && s.1 == 0).map(|s| s.0).collect();
            ed.keep_line_starts(mark, &added)
        });
    }

    /// 모든 커서가 걸친 줄들을 지운다. 커서는 지운 자리 다음 줄의 첫 글자로 간다.
    pub fn delete_lines(&mut self) {
        let blocks = self.line_blocks();
        let primary_block = blocks.iter().position(|&(a, b)| (a..=b).contains(&sel_lines(self.sel).0)).unwrap_or(0);
        self.edit_group(|ed, _| {
            for &(first, last) in blocks.iter().rev() {
                let n = ed.buf.line_count();
                let (a, b) = if last + 1 < n {
                    (Pos::new(first, 0), Pos::new(last + 1, 0))
                } else if first > 0 {
                    (Pos::new(first - 1, ed.buf.line(first - 1).len()), Pos::new(last, ed.buf.line(last).len()))
                } else {
                    (Pos::new(0, 0), Pos::new(last, ed.buf.line(last).len()))
                };
                ed.buf.replace(a, b, "");
                ed.sync_line_edits();
            }
            let mut removed = 0;
            let mut carets = Vec::with_capacity(blocks.len());
            for &(first, last) in &blocks {
                let l = (first - removed).min(ed.buf.line_count() - 1);
                carets.push(Selection::caret(Pos::new(l, ed.buf.first_non_ws(l))));
                removed += last - first + 1;
            }
            let p = carets.remove(primary_block);
            carets.insert(0, p);
            carets
        });
    }

    /// 모든 커서가 걸친 줄들을 위(`true`)나 아래로 한 줄 옮긴다. 한 구간이라도 못 옮기면 그대로 둔다.
    pub fn move_lines(&mut self, up: bool) {
        let blocks = self.line_blocks();
        let n = self.buf.line_count();
        let (Some(&(top, _)), Some(&(_, bottom))) = (blocks.first(), blocks.last()) else { return };
        if (up && top == 0) || (!up && bottom + 1 >= n) {
            return;
        }
        self.edit_group(|ed, _| {
            for &(first, last) in &blocks {
                let block = ed.buf.text_range(Pos::new(first, 0), Pos::new(last, ed.buf.line(last).len()));
                if up {
                    let above = ed.buf.line(first - 1).to_owned();
                    let end = Pos::new(last, ed.buf.line(last).len());
                    ed.buf.replace(Pos::new(first - 1, 0), end, &format!("{block}\n{above}"));
                } else {
                    let below = ed.buf.line(last + 1).to_owned();
                    let end = Pos::new(last + 1, ed.buf.line(last + 1).len());
                    ed.buf.replace(Pos::new(first, 0), end, &format!("{below}\n{block}"));
                }
            }
            let shift = |p: Pos| Pos::new(if up { p.line - 1 } else { p.line + 1 }, p.col);
            ed.cursor_snapshot().into_iter().map(|s| Selection::new(shift(s.anchor), shift(s.head))).collect()
        });
    }

    /// 커서가 있는 줄마다 아래(`true`)나 위에 빈 줄을 넣고 그 줄로 커서를 옮긴다. 들여쓰기는 이어 간다.
    pub fn insert_line(&mut self, below: bool) {
        if self.read_only {
            return;
        }
        let before = self.cursor_snapshot();
        let mut lines: Vec<usize> = self.extra.iter().map(|c| c.sel.head.line).collect();
        let primary_line = self.sel.head.line;
        lines.retain(|&l| l != primary_line);
        lines.sort_unstable();
        lines.dedup();
        let at = |ed: &Self, l: usize| Selection::caret(Pos::new(l, if below { ed.buf.line(l).len() } else { 0 }));
        let carets: Vec<Selection> = lines.iter().map(|&l| at(self, l)).collect();
        let primary = at(self, primary_line);
        self.sel = primary;
        self.extra = carets.into_iter().map(Cursor::new).collect();
        self.normalize_cursors();
        self.edit_before = Some(before);
        if below {
            self.newline();
        } else {
            self.edit(EditKind::Other, |ed| {
                let l = ed.sel.head.line;
                let base = ed.buf.line(l)[..ed.buf.first_non_ws(l)].to_owned();
                ed.buf.insert(Pos::new(l, 0), &format!("{base}\n"));
                Selection::caret(Pos::new(l, base.len()))
            });
        }
    }

    pub fn undo(&mut self) {
        if self.read_only {
            return;
        }
        if let Some(all) = self.buf.undo() {
            self.restore_cursors(&all);
            self.sync_highlighter();
            self.reveal = Some(Reveal::Nearest);
        }
    }

    pub fn redo(&mut self) {
        if self.read_only {
            return;
        }
        if let Some(all) = self.buf.redo() {
            self.restore_cursors(&all);
            self.sync_highlighter();
            self.reveal = Some(Reveal::Nearest);
        }
    }

    pub fn select_all(&mut self) {
        self.extra.clear();
        self.sel = Selection::new(Pos::default(), self.buf.end_pos());
    }

    /// 복사할 텍스트. 선택이 없으면 현재 줄 전체(줄바꿈 포함). 커서가 여럿이면 커서마다 한 줄씩 잇는다.
    pub fn copy_text(&self) -> String {
        if !self.extra.is_empty() {
            let all = self.cursors();
            let empty = all.iter().all(Selection::is_empty);
            let parts: Vec<String> = all
                .iter()
                .map(|s| {
                    let (a, b) = s.range();
                    if a == b { self.buf.line(a.line).to_owned() } else { self.buf.text_range(a, b) }
                })
                .collect();
            let mut out = parts.join("\n");
            if empty {
                out.push('\n');
            }
            return out;
        }
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
        if self.cursors().iter().all(Selection::is_empty) {
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

    /// 붙여 넣는다. 커서 수와 줄 수가 같으면 커서마다 한 줄씩 나눠 넣는다.
    pub fn paste(&mut self, text: &str) {
        let n = self.cursor_count();
        let norm = text.replace("\r\n", "\n");
        let body = norm.strip_suffix('\n').unwrap_or(&norm);
        let pieces: Vec<&str> = body.split('\n').collect();
        let distribute = n > 1 && pieces.len() == n;
        let mut k = n;
        self.edit(EditKind::Other, |ed| {
            k -= 1;
            let piece = if distribute { pieces[k] } else { text };
            let (a, b) = ed.sel.range();
            Selection::caret(ed.buf.replace(a, b, piece))
        });
    }

    /// 모든 커서를 움직인다. `extend` 면 선택을 넓힌다.
    pub(crate) fn move_caret(&mut self, m: Motion, extend: bool) {
        if self.extra.is_empty() {
            self.move_one(m, extend);
            return;
        }
        if matches!(m, Motion::DocStart | Motion::DocEnd) && !extend {
            self.extra.clear();
            self.move_one(m, extend);
            return;
        }
        let primary = self.primary_cursor();
        let extras = std::mem::take(&mut self.extra);
        let mut moved = Vec::with_capacity(extras.len());
        for c in extras {
            self.set_primary_cursor(c);
            self.move_one(m, extend);
            moved.push(self.primary_cursor());
        }
        self.set_primary_cursor(primary);
        self.move_one(m, extend);
        self.extra = moved;
        self.normalize_cursors();
    }

    /// 위치가 속한 시각 줄의 줄 안 순번. `row_end` 면 줄 바꿈 지점을 앞 시각 줄 끝으로 본다.
    pub(crate) fn sub_of_aff(&self, p: Pos, row_end: bool) -> usize {
        let sub = self.display.sub_of(p);
        if row_end && sub > 0 && self.display.segment(p.line, sub, self.buf.line(p.line).len()).0 == p.col {
            sub - 1
        } else {
            sub
        }
    }

    /// 위치의 시각 줄 번호. `row_end` 는 [`Editor::sub_of_aff`] 와 같다.
    pub(crate) fn pos_row_aff(&mut self, p: Pos, row_end: bool) -> usize {
        let sub = self.sub_of_aff(p, row_end);
        self.display.line_row(p.line) + sub
    }

    /// 주 커서 헤드가 앞 시각 줄 끝에 붙어 있는지.
    pub(crate) fn head_at_row_end(&self) -> bool {
        self.row_end_at == Some(self.sel.head)
    }

    /// 위치가 속한 시각 줄의 바이트 범위와 줄의 마지막 시각 줄인지. `row_end` 는 [`Editor::sub_of_aff`] 와 같다.
    pub(crate) fn seg_of_aff(&self, p: Pos, row_end: bool) -> (usize, usize, bool) {
        let sub = self.sub_of_aff(p, row_end);
        let len = self.buf.line(p.line).len();
        let (a, b) = self.display.segment(p.line, sub, len);
        (a, b, sub + 1 >= self.display.rows(p.line).max(1))
    }

    /// 시각 줄 시작부터 센 표시 열.
    pub(crate) fn row_display_col(&self, p: Pos) -> usize {
        self.row_display_col_aff(p, false)
    }

    fn row_display_col_aff(&self, p: Pos, row_end: bool) -> usize {
        let (s, _, _) = self.seg_of_aff(p, row_end);
        let tab = self.indent.width();
        self.buf.line(p.line)[s.min(p.col)..p.col].chars().map(|c| char_cols(c, tab)).sum()
    }

    /// 시각 줄 `s..e` 안에서 표시 열 `target` 에 가장 가까운 바이트 위치. 마지막이 아닌 시각 줄의 끝은 피한다.
    fn col_in_seg(&self, line: usize, s: usize, e: usize, last: bool, target: usize) -> usize {
        let tab = self.indent.width();
        let text = &self.buf.line(line)[s..e];
        let before_end = s + text.char_indices().next_back().map_or(0, |(i, _)| i);
        let mut w = 0;
        for (i, c) in text.char_indices() {
            let cw = char_cols(c, tab);
            if w + cw > target {
                let at = if target - w > cw / 2 { s + i + c.len_utf8() } else { s + i };
                return if at == e && !last { before_end } else { at };
            }
            w += cw;
        }
        if last { e } else { before_end }
    }

    /// `head` 에서 시각 줄 `delta` 만큼 떨어진 줄의 표시 열 `pref` 위치. 문서 밖이면 `None`.
    pub(crate) fn vertical_step(&mut self, head: Pos, pref: usize, delta: isize) -> Option<Pos> {
        self.vertical_step_aff(head, false, pref, delta)
    }

    fn vertical_step_aff(&mut self, head: Pos, row_end: bool, pref: usize, delta: isize) -> Option<Pos> {
        let row = self.pos_row_aff(head, row_end) as isize;
        let total = self.display.total_rows() as isize;
        let t = row + delta;
        if t < 0 || t >= total {
            return None;
        }
        let (line, sub) = self.display.row_to_line(t as usize);
        let len = self.buf.line(line).len();
        let (s, e) = self.display.segment(line, sub, len);
        let last = sub + 1 >= self.display.rows(line).max(1);
        Some(Pos::new(line, self.col_in_seg(line, s, e, last, pref)))
    }

    /// 주 커서 하나를 움직인다.
    fn move_one(&mut self, m: Motion, extend: bool) {
        let (a, b) = self.sel.range();
        let head = self.sel.head;
        let aff = self.head_at_row_end();
        self.row_end_at = None;
        let collapse_to = |p: Pos| if extend { None } else { Some(p) };
        let target = match m {
            Motion::Left => collapse_to(a).filter(|_| a != b).unwrap_or_else(|| self.buf.prev_char(head)),
            Motion::Right => collapse_to(b).filter(|_| a != b).unwrap_or_else(|| self.buf.next_char(head)),
            Motion::WordLeft => self.buf.prev_word(head),
            Motion::WordRight => self.buf.next_word(head),
            Motion::Home => {
                let (s, _, _) = self.seg_of_aff(head, aff);
                if s > 0 && head.col != s {
                    Pos::new(head.line, s)
                } else {
                    let fnw = self.buf.first_non_ws(head.line);
                    Pos::new(head.line, if head.col == fnw { 0 } else { fnw })
                }
            }
            Motion::End => {
                let (_, e, last) = self.seg_of_aff(head, aff);
                if !last && !(aff && head.col == e) {
                    let p = Pos::new(head.line, e);
                    self.row_end_at = Some(p);
                    p
                } else {
                    Pos::new(head.line, self.buf.line(head.line).len())
                }
            }
            Motion::Up(n) | Motion::Down(n) => {
                let pref = self.preferred_col.unwrap_or_else(|| self.row_display_col_aff(head, aff));
                let up = matches!(m, Motion::Up(_));
                let row = self.pos_row_aff(head, aff);
                let total = self.display.total_rows();
                let p = if up && row == 0 {
                    Pos::new(0, 0)
                } else if !up && row + 1 >= total {
                    let (l, _) = self.display.row_to_line(total.saturating_sub(1));
                    Pos::new(l, self.buf.line(l).len())
                } else {
                    let d = if up { -(n.min(row) as isize) } else { n.min(total - 1 - row) as isize };
                    self.vertical_step_aff(head, aff, pref, d).unwrap_or(head)
                };
                self.sel = if extend { Selection::new(self.sel.anchor, p) } else { Selection::caret(p) };
                self.preferred_col = Some(pref);
                self.reveal = Some(Reveal::Nearest);
                return;
            }
            Motion::DocStart => Pos::default(),
            Motion::DocEnd => self.buf.end_pos(),
        };
        let target = if self.display.is_hidden(target.line) {
            let forward = target > head;
            match self.display.next_visible(target.line, forward) {
                Some(l) if forward => Pos::new(l, 0),
                Some(l) => Pos::new(l, self.buf.line(l).len()),
                None => target,
            }
        } else {
            target
        };
        self.sel = if extend { Selection::new(self.sel.anchor, target) } else { Selection::caret(target) };
        self.preferred_col = None;
        self.reveal = Some(Reveal::Nearest);
    }
}

/// 선택이 걸친 줄 번호(첫 줄, 끝 줄). 끝 위치가 줄 맨 앞이면 그 줄은 제외.
fn sel_lines(s: Selection) -> (usize, usize) {
    let (a, b) = s.range();
    let last = if b.line > a.line && b.col == 0 { b.line - 1 } else { b.line };
    (a.line, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_text("t.rs", text)
    }

    #[cfg(unix)] #[test]
    fn hardlinked_document_preserves_links_and_can_export_a_rescue_copy(){
        let dir=tempfile::tempdir().unwrap();let source=dir.path().join("source.txt");let alias=dir.path().join("alias.txt");let copy=dir.path().join("copy.txt");
        std::fs::write(&source,"original").unwrap();std::fs::hard_link(&source,&alias).unwrap();
        let mut editor=Editor::open(&source).unwrap();editor.select_all();editor.insert_text("edited");
        assert!(editor.save().is_err());assert_eq!(std::fs::read_to_string(&source).unwrap(),"original");assert_eq!(std::fs::read_to_string(&alias).unwrap(),"original");
        editor.save_copy(&copy).unwrap();assert_eq!(std::fs::read_to_string(copy).unwrap(),"edited");assert!(editor.is_dirty());
    }

    #[test]
    fn disk_conflict_reload_requires_confirmation_and_cancel_preserves_edits() {
        use egui_kittest::{Harness, kittest::Queryable};
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("draft.txt");
        std::fs::write(&path,"original").unwrap(); let mut editor=Editor::open(&path).unwrap();
        editor.select_all(); editor.insert_text("my unsaved edit");
        std::fs::write(&path,"external version").unwrap(); assert!(editor.save().is_err()); editor.save_error=None;
        let mut initialized=false;
        let mut h=Harness::builder().with_size([1100.0,600.0]).build_ui_state(|ui,editor:&mut Editor| {if !initialized {kiln_common::fonts::install(ui.ctx()); kiln_common::Theme::current().apply(ui.ctx()); initialized=true; return;} editor.ui(ui);},editor);
        h.run_steps(3);
        h.get_by_label("디스크 파일 불러오기…").click(); h.run_steps(3);
        assert_eq!(h.state().text(),"my unsaved edit");
        h.get_by_label("계속 편집").click(); h.run_steps(3);
        assert_eq!(h.state().text(),"my unsaved edit"); assert!(h.state().is_dirty());
        h.get_by_label("디스크 파일 불러오기…").click(); h.run_steps(3);
        h.get_by_label("내 편집 버리고 불러오기").click(); h.run_steps(3);
        assert_eq!(h.state().text(),"external version"); assert!(!h.state().is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(),"external version");
    }

    #[test]
    fn save_checks_disk_even_without_a_focus_change() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("draft.txt");
        std::fs::write(&path,"old").unwrap();let mut editor=Editor::open(&path).unwrap();editor.insert_text("mine");
        std::fs::write(&path,"external replacement").unwrap();
        assert!(editor.save().is_err());assert_eq!(std::fs::read_to_string(&path).unwrap(),"external replacement");
        editor.keep_local_changes();assert!(editor.save().is_ok());
    }

    #[test]
    fn editor_exposes_multiline_text_runs_and_accessible_selection() {
        let ctx=egui::Context::default(); ctx.enable_accesskit();
        let mut editor=ed("hello 한글\nsecond line");
        let input=egui::RawInput { screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(800.0,600.0))), ..Default::default() };
        let mut output=ctx.run_ui(input.clone(),|ui| editor.ui(ui));
        output.textures_delta.clear();
        let tree=output.platform_output.accesskit_update.unwrap();
        let (_,node)=tree.nodes.iter().find(|(id,_)| *id==editor.id().accesskit_id()).unwrap();
        assert_eq!(node.role(),egui::accesskit::Role::MultilineTextInput);
        assert_eq!(node.value(),Some("hello 한글\nsecond line"));
        assert!(node.text_selection().is_some());
        let (run,_)=tree.nodes.iter().find(|(_,n)| n.role()==egui::accesskit::Role::TextRun && n.value().unwrap_or("").starts_with("hello")).unwrap();
        let mut input=input;
        input.events.push(egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest { action:egui::accesskit::Action::SetTextSelection,target_tree:egui::accesskit::TreeId::ROOT,target_node:editor.id().accesskit_id(),data:Some(egui::accesskit::ActionData::SetTextSelection(egui::accesskit::TextSelection { anchor:egui::accesskit::TextPosition { node:*run,character_index:6 },focus:egui::accesskit::TextPosition { node:*run,character_index:8 } })) }));
        let mut output=ctx.run_ui(input,|ui| editor.ui(ui));
        output.textures_delta.clear();
        assert_eq!(editor.selection(),Selection::new(Pos::new(0,6),Pos::new(0,12)));
    }

    #[test]
    fn draft_roundtrip_preserves_unsaved_text_and_never_writes_on_restore() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("draft.txt");
        std::fs::write(&path, "original").unwrap();
        let mut e = Editor::open(&path).unwrap(); e.select_all(); e.insert_text("recovered 한글\n");
        let draft: EditorDraft = serde_json::from_str(&serde_json::to_string(&e.recovery_draft().unwrap()).unwrap()).unwrap();
        let mut restored = Editor::open(&path).unwrap(); restored.restore_draft(&draft);
        assert_eq!(restored.text(), "recovered 한글\n"); assert!(restored.is_dirty()); assert!(!restored.recovery_conflict());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        restored.save().unwrap(); assert!(!restored.is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "recovered 한글\n");
    }

    #[test]
    fn recovered_draft_cannot_silently_overwrite_external_change() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("draft.txt");
        std::fs::write(&path, "original").unwrap();
        let mut e = Editor::open(&path).unwrap(); e.select_all(); e.insert_text("my changes");
        let draft = e.recovery_draft().unwrap(); std::fs::write(&path, "external changed text").unwrap();
        let mut restored = Editor::open(&path).unwrap(); restored.restore_draft(&draft);
        assert!(restored.recovery_conflict()); assert!(restored.save().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external changed text");
        restored.keep_local_changes(); restored.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my changes");
    }

    #[test]
    fn empty_and_deleted_file_drafts_remain_dirty() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("draft.txt");
        std::fs::write(&path, "original").unwrap();
        let mut e = Editor::open(&path).unwrap(); e.select_all(); e.insert_text("");
        let draft=e.recovery_draft().unwrap();
        let mut restored=Editor::open(&path).unwrap(); restored.restore_draft(&draft);
        assert!(restored.is_dirty()); assert_eq!(restored.text(), "");
        e.insert_text("survives deletion"); let draft=e.recovery_draft().unwrap(); std::fs::remove_file(&path).unwrap();
        let mut restored=Editor::from_text(&path, ""); restored.restore_draft(&draft);
        assert!(restored.is_dirty()); assert!(restored.recovery_conflict());
        assert_eq!(restored.text(), "survives deletion"); assert!(!path.exists());
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
    fn wrapped_lines_move_by_visual_row_and_home_end_stay_in_row() {
        let mut e = ed("aaaa bbbb cccc\nx");
        e.set_wrap_cols(5);
        e.sel = Selection::caret(Pos::new(0, 1));
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(0, 6));
        e.move_caret(Motion::End, false);
        assert_eq!(e.sel.head, Pos::new(0, 10));
        assert!(e.head_at_row_end());
        e.move_caret(Motion::End, false);
        assert_eq!(e.sel.head, Pos::new(0, 14));
        e.move_caret(Motion::Home, false);
        assert_eq!(e.sel.head, Pos::new(0, 10));
        e.move_caret(Motion::Home, false);
        assert_eq!(e.sel.head, Pos::new(0, 0));
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(0, 5));
        e.move_caret(Motion::Down(2), false);
        assert_eq!(e.sel.head, Pos::new(1, 0));
        e.move_caret(Motion::Up(1), false);
        assert_eq!(e.sel.head, Pos::new(0, 10));
        e.insert_text("dddd ");
        assert_eq!(e.display.rows(0), 4);
        e.set_word_wrap(true);
        e.set_word_wrap(false);
        assert_eq!(e.display.rows(0), 1);
    }

    #[test]
    fn end_on_wrapped_row_puts_caret_after_last_char_of_that_row() {
        let mut e = ed("aaaa bbbb cccc\nx");
        e.set_wrap_cols(5);
        e.sel = Selection::caret(Pos::new(0, 1));
        e.move_caret(Motion::End, false);
        assert_eq!(e.sel.head, Pos::new(0, 5));
        assert!(e.head_at_row_end());
        assert_eq!(e.pos_row_aff(e.sel.head, e.head_at_row_end()), 0, "커서는 첫 시각 줄 끝에 그린다");
        assert_eq!(e.display.pos_row(e.sel.head), 1);
        e.move_caret(Motion::Home, false);
        assert_eq!(e.sel.head, Pos::new(0, 0), "Home 은 같은 시각 줄의 처음으로 간다");
        e.move_caret(Motion::End, false);
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(0, 9));
        assert!(!e.head_at_row_end());
        e.sel = Selection::caret(Pos::new(0, 1));
        e.move_caret(Motion::End, true);
        assert_eq!(e.sel, Selection::new(Pos::new(0, 1), Pos::new(0, 5)));
        e.sel = Selection::caret(Pos::new(0, 1));
        e.move_caret(Motion::End, false);
        e.insert_text("X");
        assert_eq!(e.text(), "aaaa Xbbbb cccc\nx");
        assert!(!e.head_at_row_end());
    }

    #[test]
    fn end_affinity_is_kept_per_cursor() {
        let mut e = ed("aaaa bbbb\ncccc dddd");
        e.set_wrap_cols(5);
        e.set_cursors(Selection::caret(Pos::new(0, 0)), &[Selection::caret(Pos::new(1, 0))]);
        e.move_caret(Motion::End, false);
        assert_eq!(e.cursors().iter().map(|s| s.head).collect::<Vec<_>>(), vec![Pos::new(0, 5), Pos::new(1, 5)]);
        assert!(e.head_at_row_end());
        assert!(e.extra.iter().all(|c| c.row_end_at == Some(c.sel.head)));
    }

    #[test]
    fn cursor_motion_skips_folded_lines() {
        let mut e = ed("fn a() {\n    x();\n}\nz");
        e.fold_range(FoldRange { start: 0, end: 1 });
        e.sel = Selection::caret(Pos::new(0, 8));
        e.move_caret(Motion::Right, false);
        assert_eq!(e.sel.head, Pos::new(2, 0));
        e.move_caret(Motion::Left, false);
        assert_eq!(e.sel.head, Pos::new(0, 8));
        e.sel = Selection::caret(Pos::new(0, 3));
        e.move_caret(Motion::Down(1), false);
        assert_eq!(e.sel.head, Pos::new(2, 1));
        e.move_caret(Motion::Up(1), false);
        assert_eq!(e.sel.head, Pos::new(0, 3));
        e.goto(2, 1);
        assert!(e.folded_ranges().is_empty());
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
