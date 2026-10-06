//! 편집기와 언어 서버 연결: 문서 동기화, 진단 표시, 호버, 정의·참조 이동, 자동 완성, 이름 바꾸기, 서식.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Area, Color32, FontId, Frame, Galley, Id, Key, Modifiers, Order, Pos2, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};
use kiln_common::Theme;

use super::view::RowInfo;
use super::{Editor, Reveal};
use crate::EditorEvent;
use crate::buffer::{EditKind, Pos, Selection};
use crate::lsp::{
    self, CompletionItem, Diagnostic, Location, LspManager, Pending, Position, Range, Severity, SignatureHelp,
    TextChange, TextEdit, WorkspaceEdit, position,
};
use crate::ui_kit;

const HOVER_DELAY: f64 = 0.4;
const COMPLETION_ROWS: usize = 10;
const COMPLETION_ROW_H: f32 = 26.0;
const LIST_ROWS: usize = 12;
const TOAST_SECS: f64 = 3.0;

/// 화면에 쓰는 진단(버퍼 바이트 위치).
#[derive(Clone, Debug)]
pub(crate) struct DiagView {
    a: Pos,
    b: Pos,
    severity: Severity,
    message: String,
    source: Option<String>,
}

struct HoverPopup {
    /// 호버 대상 단어의 시작 위치.
    anchor: Pos,
    diags: Vec<(Severity, String)>,
    text: Option<String>,
    from_mouse: bool,
}

struct CompletionState {
    items: Vec<CompletionItem>,
    /// 거른 결과(`items` 색인).
    shown: Vec<usize>,
    selected: usize,
    top: usize,
    /// 단어 시작 위치.
    anchor: Pos,
    incomplete: bool,
    version: u64,
    head: Pos,
}

struct ListPopup {
    title: String,
    items: Vec<ListItem>,
    selected: usize,
    top: usize,
    anchor: Pos,
}

struct ListItem {
    loc: Location,
    file_name: String,
    label: String,
    preview: String,
}

struct RenameState {
    text: String,
    anchor: Pos,
    started: bool,
}

pub(crate) struct LspState {
    mgr: LspManager,
    path: PathBuf,
    opened: bool,
    diag_version: Option<u64>,
    diags: Vec<DiagView>,
    rest: Option<(Pos, f64, Pos2)>,
    hover: Option<HoverPopup>,
    hover_req: Option<(Pending<Option<String>>, Pos, bool)>,
    completion: Option<CompletionState>,
    completion_req: Option<(Pending<lsp::CompletionList>, Pos, u64, Pos)>,
    definition_req: Option<Pending<Vec<Location>>>,
    references_req: Option<Pending<Vec<Location>>>,
    list: Option<ListPopup>,
    rename: Option<RenameState>,
    rename_req: Option<Pending<WorkspaceEdit>>,
    format_req: Option<(Pending<Vec<TextEdit>>, u64)>,
    signature: Option<(SignatureHelp, Pos)>,
    signature_req: Option<(Pending<Option<SignatureHelp>>, Pos)>,
    toast: Option<(String, f64)>,
    /// 마지막 프레임에 그린 팝업 영역.
    popup_rects: Vec<Rect>,
    now: f64,
}

impl Drop for LspState {
    fn drop(&mut self) {
        if self.opened {
            self.mgr.close_document(&self.path);
        }
    }
}

impl LspState {
    pub(crate) fn manager(&self) -> LspManager {
        self.mgr.clone()
    }

    fn any_pending(&self) -> bool {
        self.hover_req.is_some()
            || self.completion_req.is_some()
            || self.definition_req.is_some()
            || self.references_req.is_some()
            || self.rename_req.is_some()
            || self.format_req.is_some()
            || self.signature_req.is_some()
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn lsp_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| {
        if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
    })
}

impl Editor {
    /// 파일을 열고 언어 서버를 연결한다.
    pub fn open_with_lsp(path: impl Into<PathBuf>, lsp: Option<LspManager>) -> anyhow::Result<Editor> {
        let mut ed = Editor::open(path)?;
        if let Some(m) = lsp {
            ed.set_lsp(m);
        }
        Ok(ed)
    }

    /// 언어 서버 관리자를 연결한다. 이 파일 언어의 서버가 없으면 진단 표시만 한다.
    pub fn set_lsp(&mut self, mgr: LspManager) {
        self.lsp = None;
        let path = lsp_path(&self.path);
        let text=self.buf.lines().join("\n");
        let language=if self.language_override.is_some(){crate::syntax::language_id(&self.language).map(str::to_owned)}else{mgr.language_for_syntax(&path,&self.language)};
        let opened = !self.is_binary() && !self.is_large() && mgr.open_document_as(&path, &text, language.as_deref());
        self.buf.set_track_deltas(opened);
        self.lsp = Some(LspState {
            mgr,
            path,
            opened,
            diag_version: None,
            diags: Vec::new(),
            rest: None,
            hover: None,
            hover_req: None,
            completion: None,
            completion_req: None,
            definition_req: None,
            references_req: None,
            list: None,
            rename: None,
            rename_req: None,
            format_req: None,
            signature: None,
            signature_req: None,
            toast: None,
            popup_rects: Vec::new(),
            now: 0.0,
        });
    }

    /// 연결된 언어 서버 관리자.
    pub fn lsp(&self) -> Option<&LspManager> {
        self.lsp.as_ref().map(|s| &s.mgr)
    }

    /// 이 파일이 언어 서버와 동기화되고 있는지.
    pub fn lsp_attached(&self) -> bool {
        self.lsp.as_ref().is_some_and(|s| s.opened)
    }

    /// 이 파일의 진단(언어 서버가 보낸 그대로, UTF-16 위치).
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.lsp.as_ref().map(|s| s.mgr.diagnostics_for(&s.path)).unwrap_or_default()
    }

    /// 열려 있는 언어 서버 팝업 종류. UI 테스트에서 쓴다.
    #[doc(hidden)]
    pub fn lsp_popup_kind(&self) -> Option<&'static str> {
        let st = self.lsp.as_ref()?;
        if st.rename.is_some() {
            Some("rename")
        } else if st.completion.as_ref().is_some_and(|c| !c.shown.is_empty()) {
            Some("completion")
        } else if st.list.is_some() {
            Some("list")
        } else if let Some(h) = &st.hover {
            Some(if h.text.is_some() { "hover" } else { "hover-loading" })
        } else if st.signature.is_some() {
            Some("signature")
        } else {
            None
        }
    }

    /// 화면에 그리지 않을 때도 언어 서버 쪽 대기 편집과 응답을 처리한다. 백그라운드 탭에서 부르면 된다.
    pub fn poll_lsp(&mut self) {
        self.lsp_poll(None);
    }

    // ---- 위치 변환 ----

    fn to_lsp(&self, p: Pos) -> Position {
        let p = self.buf.clamp(p);
        Position::new(p.line as u32, position::utf16_col(self.buf.line(p.line), p.col))
    }

    fn pos_from_lsp(&self, p: Position) -> Pos {
        let n = self.buf.line_count();
        if p.line as usize >= n {
            return self.buf.end_pos();
        }
        let line = self.buf.line(p.line as usize);
        Pos::new(p.line as usize, position::byte_col(line, p.character))
    }

    /// 쌓인 텍스트 변경을 서버에 보낸다.
    fn lsp_flush(&mut self) {
        let Some(st) = &self.lsp else { return };
        if !st.opened {
            return;
        }
        let deltas = self.buf.take_deltas();
        if deltas.is_empty() {
            return;
        }
        let changes: Vec<TextChange> = deltas
            .into_iter()
            .map(|d| TextChange {
                range: Some(Range {
                    start: Position::new(d.start_line as u32, d.start_u16 as u32),
                    end: Position::new(d.end_line as u32, d.end_u16 as u32),
                }),
                text: d.text,
            })
            .collect();
        st.mgr.change_document(&st.path, &changes);
    }

    pub(crate) fn lsp_did_save(&mut self) {
        self.lsp_flush();
        if let Some(st) = &self.lsp
            && st.opened
        {
            st.mgr.save_document(&st.path);
        }
    }

    /// 줄 편집에 맞춰 진단 위치를 옮긴다.
    pub(crate) fn lsp_on_line_edit(&mut self, e: crate::buffer::LineEdit) {
        let Some(st) = &mut self.lsp else { return };
        let delta = e.new_count as isize - e.old_count as isize;
        let old_end = e.start + e.old_count;
        let new_last = e.start + e.new_count.max(1) - 1;
        for d in &mut st.diags {
            for p in [&mut d.a, &mut d.b] {
                if p.line >= old_end {
                    p.line = (p.line as isize + delta) as usize;
                } else if p.line > new_last {
                    p.line = new_last;
                }
            }
        }
    }

    // ---- 프레임마다 ----

    pub(crate) fn lsp_frame(&mut self, ui: &Ui) {
        if self.lsp.is_none() {
            return;
        }
        self.lsp_poll(Some(ui));
    }

    fn lsp_poll(&mut self, ui: Option<&Ui>) {
        let Some(st) = &mut self.lsp else { return };
        if let Some(ui) = ui {
            st.mgr.set_repaint_ctx(ui.ctx());
            st.now = ui.input(|i| i.time);
        }
        self.lsp_flush();
        let Some(st) = &mut self.lsp else { return };
        let edits = st.mgr.take_pending_edits(&st.path);
        if !edits.is_empty() {
            self.apply_lsp_edits(&edits);
            self.lsp_flush();
        }
        self.refresh_diagnostics();
        self.poll_requests();
        self.update_completion_filter();
        let Some(st) = &mut self.lsp else { return };
        if let Some((_, t0)) = &st.toast
            && st.now - t0 > TOAST_SECS
        {
            st.toast = None;
        }
        if let Some(ui) = ui {
            if st.any_pending() {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            if st.toast.is_some() {
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    fn refresh_diagnostics(&mut self) {
        let Some(st) = &self.lsp else { return };
        let v = st.mgr.diagnostics_version();
        if st.diag_version == Some(v) {
            return;
        }
        let raw = st.mgr.diagnostics_for(&st.path);
        let mut views: Vec<DiagView> = raw
            .into_iter()
            .map(|d| DiagView {
                a: self.pos_from_lsp(d.range.start),
                b: self.pos_from_lsp(d.range.end),
                severity: d.severity,
                message: d.message,
                source: d.source,
            })
            .collect();
        views.sort_by_key(|d| (d.a, Reverse(d.severity)));
        let st = self.lsp.as_mut().expect("lsp");
        st.diags = views;
        st.diag_version = Some(v);
    }

    fn toast(&mut self, msg: impl Into<String>) {
        if let Some(st) = &mut self.lsp {
            st.toast = Some((msg.into(), st.now));
        }
    }

    fn poll_requests(&mut self) {
        let Some(st) = &mut self.lsp else { return };

        if let Some((req, anchor, from_mouse)) = &mut st.hover_req
            && let Some(res) = req.poll()
        {
            let (anchor, from_mouse) = (*anchor, *from_mouse);
            st.hover_req = None;
            let text = res.ok().flatten().filter(|t| !t.trim().is_empty());
            match &mut st.hover {
                Some(h) if h.anchor == anchor => h.text = text,
                _ => st.hover = Some(HoverPopup { anchor, diags: Vec::new(), text, from_mouse }),
            }
            if st.hover.as_ref().is_some_and(|h| h.text.is_none() && h.diags.is_empty()) {
                st.hover = None;
                if !from_mouse {
                    st.toast = Some((kiln_common::i18n::tr("표시할 정보가 없습니다").into(), st.now));
                }
            }
        }

        if let Some((req, anchor, version, requested_head)) = &mut st.completion_req
            && let Some(res) = req.poll()
        {
            let anchor = *anchor;
            let valid = *version == self.buf.version() && *requested_head == self.sel.head;
            st.completion_req = None;
            match res {
                Ok(list) => {
                    let head = self.sel.head;
                    if valid && head.line == anchor.line && head.col >= anchor.col {
                        let st = self.lsp.as_mut().expect("lsp");
                        st.completion = Some(CompletionState {
                            items: list.items,
                            shown: Vec::new(),
                            selected: 0,
                            top: 0,
                            anchor,
                            incomplete: list.is_incomplete,
                            version: u64::MAX,
                            head,
                        });
                        self.update_completion_filter();
                    }
                }
                Err(_) => self.local_completion(),
            }
        }

        let Some(st) = &mut self.lsp else { return };
        if let Some(req) = &mut st.definition_req
            && let Some(res) = req.poll()
        {
            st.definition_req = None;
            match res {
                Ok(locs) if locs.len() == 1 => self.navigate(&locs[0]),
                Ok(locs) if locs.is_empty() => self.toast(kiln_common::i18n::tr("정의를 찾을 수 없습니다")),
                Ok(locs) => {
                    let title = kiln_common::trf!("정의 {}개", locs.len());
                    self.show_list(title, locs);
                }
                Err(e) => self.toast(kiln_common::trf!("정의로 이동 실패: {e}")),
            }
        }

        let Some(st) = &mut self.lsp else { return };
        if let Some(req) = &mut st.references_req
            && let Some(res) = req.poll()
        {
            st.references_req = None;
            match res {
                Ok(locs) if locs.is_empty() => self.toast(kiln_common::i18n::tr("참조가 없습니다")),
                Ok(locs) => {
                    let title = kiln_common::trf!("참조 {}개", locs.len());
                    self.show_list(title, locs);
                }
                Err(e) => self.toast(kiln_common::trf!("참조 찾기 실패: {e}")),
            }
        }

        let Some(st) = &mut self.lsp else { return };
        if let Some(req) = &mut st.rename_req
            && let Some(res) = req.poll()
        {
            st.rename_req = None;
            match res {
                Ok(edit) if edit.changes.is_empty() => self.toast(kiln_common::i18n::tr("바꿀 항목이 없습니다")),
                Ok(edit) => {
                    let mgr = st.mgr.clone();
                    let path = st.path.clone();
                    match mgr.apply_workspace_edit(&edit) {
                        Ok(()) => {
                            let mine = mgr.take_pending_edits(&path);
                            self.apply_lsp_edits(&mine);
                            self.lsp_flush();
                            let files = edit.changes.len();
                            let n: usize = edit.changes.iter().map(|(_, e)| e.len()).sum();
                            self.toast(kiln_common::trf!("{files}개 파일에서 {n}곳을 바꿨습니다"));
                        }
                        Err(e) => self.toast(kiln_common::trf!("이름 바꾸기 실패: {e:#}")),
                    }
                }
                Err(e) => self.toast(kiln_common::trf!("이름 바꾸기 실패: {e}")),
            }
        }

        let Some(st) = &mut self.lsp else { return };
        if let Some((req, version)) = &mut st.format_req
            && let Some(res) = req.poll()
        {
            let version = *version;
            st.format_req = None;
            match res {
                Ok(edits) if self.buf.version() == version => {
                    self.apply_lsp_edits(&edits);
                    self.lsp_flush();
                }
                Ok(_) => self.toast(kiln_common::i18n::tr("문서가 바뀌어 서식을 적용하지 않았습니다")),
                Err(e) => self.toast(kiln_common::trf!("서식 실패: {e}")),
            }
        }

        let Some(st) = &mut self.lsp else { return };
        if let Some((req, at)) = &mut st.signature_req
            && let Some(res) = req.poll()
        {
            let at = *at;
            st.signature_req = None;
            st.signature = res.ok().flatten().map(|s| (s, at));
        }
    }

    /// 편집 목록을 되돌리기 한 단계로 적용한다. 범위는 현재 버퍼 기준 UTF-16 위치.
    /// 모든 커서는 편집을 거쳐 옮긴다.
    pub(crate) fn apply_lsp_edits(&mut self, edits: &[TextEdit]) {
        if edits.is_empty() || self.read_only {
            return;
        }
        let mut conv: Vec<(usize, Pos, Pos, &str)> = edits
            .iter()
            .enumerate()
            .map(|(i, e)| (i, self.pos_from_lsp(e.range.start), self.pos_from_lsp(e.range.end), e.new_text.as_str()))
            .collect();
        conv.sort_by_key(|&(i, a, _, _)| Reverse((a, i)));
        let before = self.cursor_snapshot();
        let now = self.now();
        self.buf.begin(EditKind::Other, &before, now);
        let mark = self.buf.edit_mark();
        for (_, a, b, text) in conv {
            let (a, b) = (self.buf.clamp(a), self.buf.clamp(b));
            self.buf.replace(a, b, text);
            self.sync_line_edits();
        }
        let after = self.mapped_cursors(mark);
        self.restore_cursors(&after);
        self.buf.end(&self.cursor_snapshot());
        self.sync_highlighter();
        self.preferred_col = None;
    }

    // ---- 명령 ----

    fn lsp_ready_for_requests(&mut self) -> bool {
        match &self.lsp {
            Some(st) if st.opened => true,
            Some(_) => {
                self.toast(kiln_common::i18n::tr("이 파일에 연결된 언어 서버가 없습니다"));
                false
            }
            None => false,
        }
    }

    pub(crate) fn lsp_hover_at_cursor(&mut self) {
        let head = self.sel.head;
        self.request_hover(head, false);
    }

    fn request_hover(&mut self, at: Pos, from_mouse: bool) {
        let at=self.buf.clamp(at);
        let Some(st) = &self.lsp else { return };
        let (wa, _) = self.buf.word_at(at);
        let anchor = if wa.col <= at.col { wa } else { at };
        let diags: Vec<(Severity, String)> = st
            .diags
            .iter()
            .filter(|d| d.a <= at && at <= d.b.max(self.buf.next_char(d.a)))
            .map(|d| {
                let msg = match &d.source {
                    Some(s) => format!("{} ({s})", d.message),
                    None => d.message.clone(),
                };
                (d.severity, msg)
            })
            .collect();
        let pos = self.to_lsp(at);
        self.lsp_flush();
        let st = self.lsp.as_mut().expect("lsp");
        st.hover = (!diags.is_empty()).then_some(HoverPopup { anchor, diags, text: None, from_mouse });
        if st.opened {
            st.hover_req = Some((st.mgr.hover(&st.path, pos), anchor, from_mouse));
        } else if st.hover.is_none() && !from_mouse {
            st.toast = Some((kiln_common::i18n::tr("이 파일에 연결된 언어 서버가 없습니다").into(), st.now));
        }
    }

    pub(crate) fn lsp_goto_definition(&mut self) {
        let head = self.sel.head;
        self.request_definition(head);
    }

    /// Cmd+클릭. 언어 서버가 연결돼 있으면 커서를 옮기고 정의를 요청한다.
    pub(crate) fn lsp_goto_definition_at(&mut self, p: Pos) -> bool {
        if !self.lsp_attached() {
            return false;
        }
        self.extra.clear();
        self.sel = Selection::caret(p);
        self.request_definition(p);
        true
    }

    fn request_definition(&mut self, at: Pos) {
        if !self.lsp_ready_for_requests() {
            return;
        }
        self.lsp_flush();
        let pos = self.to_lsp(at);
        let st = self.lsp.as_mut().expect("lsp");
        st.definition_req = Some(st.mgr.definition(&st.path, pos));
    }

    pub(crate) fn lsp_find_references(&mut self) {
        if !self.lsp_ready_for_requests() {
            return;
        }
        self.lsp_flush();
        let pos = self.to_lsp(self.sel.head);
        let st = self.lsp.as_mut().expect("lsp");
        st.references_req = Some(st.mgr.references(&st.path, pos));
    }

    pub(crate) fn lsp_start_rename(&mut self) {
        if self.read_only || !self.lsp_ready_for_requests() {
            return;
        }
        let (a, b) = self.buf.word_at(self.sel.head);
        let word = self.buf.text_range(a, b);
        if word.is_empty() || !word.chars().all(is_ident) {
            self.toast(kiln_common::i18n::tr("이름을 바꿀 기호 위에 커서를 두세요"));
            return;
        }
        let st = self.lsp.as_mut().expect("lsp");
        st.rename = Some(RenameState { text: word, anchor: a, started: false });
    }

    fn submit_rename(&mut self, new_name: String, at: Pos) {
        if new_name.trim().is_empty() {
            return;
        }
        self.lsp_flush();
        let pos = self.to_lsp(at);
        let st = self.lsp.as_mut().expect("lsp");
        st.rename_req = Some(st.mgr.rename(&st.path, pos, new_name.trim()));
    }

    pub(crate) fn lsp_format(&mut self) {
        if self.read_only || !self.lsp_ready_for_requests() {
            return;
        }
        self.lsp_flush();
        let (tab, spaces) = match self.indent {
            crate::buffer::Indent::Spaces(n) => (n as u32, true),
            crate::buffer::Indent::Tabs => (4, false),
        };
        let v = self.buf.version();
        let st = self.lsp.as_mut().expect("lsp");
        st.format_req = Some((st.mgr.format(&st.path, tab, spaces), v));
    }

    /// 커서 앞 단어의 시작 위치.
    fn word_start_before(&self, p: Pos) -> Pos {
        let line = self.buf.line(p.line);
        let start = line[..p.col].char_indices().rev().take_while(|(_, c)| is_ident(*c)).last().map_or(p.col, |(i, _)| i);
        Pos::new(p.line, start)
    }

    pub(crate) fn lsp_dismiss_completion(&mut self){if let Some(st)=&mut self.lsp {st.completion=None;st.completion_req=None;}}
    fn local_completion(&mut self) {
        if self.is_binary() || self.is_large() || self.read_only || self.view.preedit.is_some() || !self.sel.is_empty(){return;}
        if self.lsp.is_none(){
            self.set_lsp(LspManager::with_config(self.path.parent().unwrap_or(Path::new(".")).to_path_buf(),lsp::LspConfig::default()));
        }
        let anchor=self.word_start_before(self.sel.head);
        let prefix=&self.buf.line(self.sel.head.line)[anchor.col..self.sel.head.col];
        let mut items=super::local_completion::items(&self.language,self.buf.lines(),prefix);
        let line=self.buf.line(self.sel.head.line);
        let end=self.sel.head.col+line[self.sel.head.col..].char_indices().take_while(|(_,c)|is_ident(*c)).map(|(i,c)|i+c.len_utf8()).last().unwrap_or(0);
        let range=Range{start:self.to_lsp(anchor),end:self.to_lsp(Pos::new(self.sel.head.line,end))};
        for item in &mut items {item.edit=Some(TextEdit{range,new_text:item.insert_text.clone()});}
        self.lsp.as_mut().unwrap().completion=Some(CompletionState{items,shown:vec![],selected:0,top:0,anchor,incomplete:false,version:u64::MAX,head:self.sel.head});
        self.update_completion_filter();
    }
    pub(crate) fn lsp_trigger_completion(&mut self, trigger: Option<String>) {
        if self.view.preedit.is_some() || !self.sel.is_empty(){return;}
        if !self.lsp.as_ref().is_some_and(|s| s.opened) {
            self.local_completion();
            return;
        }
        self.lsp_flush();
        let head = self.sel.head;
        let anchor = self.word_start_before(head);
        let pos = self.to_lsp(head);
        let st = self.lsp.as_mut().expect("lsp");
        st.completion_req = Some((st.mgr.completion(&st.path, pos, trigger), anchor, self.buf.version(), head));
    }

    /// 글자를 입력한 뒤: 완성·서명 도움말을 띄우거나 거른다.
    pub(crate) fn lsp_after_typed(&mut self, s: &str) {
        self.refresh_language();
        if !self.lsp.as_ref().is_some_and(|s|s.opened){
            let head=self.sel.head;
            if s.chars().last().is_some_and(is_ident) && self.buf.line(head.line)[self.word_start_before(head).col..head.col].chars().count()>=2 {self.local_completion();}
            return;
        }
        let st=self.lsp.as_ref().unwrap();
        let Some(last) = s.chars().last() else { return };
        let ch = last.to_string();
        let triggers = st.mgr.completion_triggers(&st.path);
        let sig_triggers = st.mgr.signature_triggers(&st.path);
        let open = st.completion.is_some();
        let st = self.lsp.as_mut().expect("lsp");
        st.hover = None;
        if last == ')' {
            st.signature = None;
        }
        self.lsp_flush();
        if triggers.contains(&ch) {
            self.lsp_trigger_completion(Some(ch.clone()));
        } else if is_ident(last) {
            let head = self.sel.head;
            let prefix_len = head.col - self.word_start_before(head).col;
            let incomplete = self.lsp.as_ref().and_then(|s| s.completion.as_ref()).is_some_and(|c| c.incomplete);
            let stale_edits=self.lsp.as_ref().and_then(|s|s.completion.as_ref()).is_some_and(|c|c.items.iter().any(|i|i.edit.is_some()||!i.additional_edits.is_empty()));
            if (!open && prefix_len >= 2) || (open && (incomplete||stale_edits)) {
                self.lsp_trigger_completion(None);
            }
        } else if open {
            self.lsp.as_mut().expect("lsp").completion = None;
        }
        if sig_triggers.contains(&ch) {
            let pos = self.to_lsp(self.sel.head);
            let head = self.sel.head;
            let st = self.lsp.as_mut().expect("lsp");
            st.signature = None;
            st.signature_req = Some((st.mgr.signature_help(&st.path, pos), head));
        }
    }

    /// 완성 목록을 커서 앞 글자로 다시 거른다. 커서가 단어를 벗어나면 닫는다.
    fn update_completion_filter(&mut self) {
        let v = self.buf.version();
        let head = self.sel.head;
        let Some(st) = &self.lsp else { return };
        let Some(c) = &st.completion else { return };
        if c.version != u64::MAX && c.version != v && c.items.iter().any(|i| i.edit.is_some() || !i.additional_edits.is_empty()) {
            self.lsp.as_mut().expect("lsp").completion = None;
            return;
        }
        if c.version == v && c.head != head {
            self.lsp.as_mut().expect("lsp").completion=None;return;
        }
        if c.version == v && !c.shown.is_empty() {
            return;
        }
        if head.line != c.anchor.line || head.col < c.anchor.col {
            self.lsp.as_mut().expect("lsp").completion = None;
            return;
        }
        let prefix = self.buf.line(head.line)[c.anchor.col..head.col].to_lowercase();
        let mut scored: Vec<(u8, &str, &str, usize)> = c
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let key = if it.filter_text.is_empty() { it.label.as_str() } else { it.filter_text.as_str() };
                let lk = key.to_lowercase();
                let tier = if lk.starts_with(&prefix) {
                    0
                } else if is_subsequence(&prefix, &lk) {
                    1
                } else {
                    return None;
                };
                Some((tier, it.sort_text.as_str(), it.label.as_str(), i))
            })
            .collect();
        scored.sort();
        let shown: Vec<usize> = scored.into_iter().map(|s| s.3).collect();
        let st = self.lsp.as_mut().expect("lsp");
        let c = st.completion.as_mut().expect("completion");
        if shown.is_empty() && !c.incomplete {
            st.completion = None;
            return;
        }
        c.shown = shown;
        c.selected = 0;
        c.top = 0;
        c.version = v;
        c.head = head;
    }

    /// 고른 완성 항목을 모든 커서에 넣는다. 주 커서는 항목의 편집 범위(없으면 단어 앞부분)를 바꾸고,
    /// 다른 커서는 커서 앞 같은 글자 수만큼을 바꾼다.
    fn accept_completion(&mut self) {
        let Some(st) = &mut self.lsp else { return };
        let Some(c) = st.completion.take() else { return };
        let Some(&idx) = c.shown.get(c.selected) else { return };
        let item = c.items[idx].clone();
        let head = self.sel.head;
        if c.version != self.buf.version() || c.head != head { return; }
        let (start, end, text) = match &item.edit {
            Some(e) => (self.pos_from_lsp(e.range.start), self.pos_from_lsp(e.range.end), e.new_text.clone()),
            None => (c.anchor, head, item.insert_text.clone()),
        };
        if start > end || start.line != head.line || start > head { return; }
        let prefix_chars = self.buf.line(head.line)[start.col.min(head.col)..head.col].chars().count();
        let mut edits: Vec<TextEdit> = item.additional_edits.clone();
        edits.push(TextEdit { range: Range { start: self.to_lsp(start), end: self.to_lsp(end) }, new_text: text.clone() });
        let mut floor = Pos::default();
        for s in self.cursors() {
            let h = s.head;
            if h == head {
                floor = h;
                continue;
            }
            let line = self.buf.line(h.line);
            let from = line[..h.col].char_indices().rev().take(prefix_chars).last().map_or(h.col, |(i, _)| i);
            let from = if floor.line == h.line { from.max(floor.col) } else { from };
            floor = h;
            let range = Range { start: self.to_lsp(Pos::new(h.line, from)), end: self.to_lsp(h) };
            edits.push(TextEdit { range, new_text: text.clone() });
        }
        self.apply_lsp_edits(&edits);
        // 커서는 삽입 끝으로 옮겨져 있다. 스니펫 첫 탭 정지가 있으면 그만큼 되돌린다.
        let caret_off = item.cursor_offset.unwrap_or(text.len()).min(text.len());
        let back = crate::buffer::advance(Pos::new(0, 0), &text[caret_off..]);
        let step_back = |p: Pos| if back.line == 0 { Pos::new(p.line, p.col.saturating_sub(back.col)) } else { p };
        let carets: Vec<Selection> =
            self.cursor_snapshot().into_iter().map(|s| Selection::caret(self.buf.clamp(step_back(s.head)))).collect();
        self.restore_cursors(&carets);
        self.reveal = Some(Reveal::Nearest);
        self.lsp_flush();
    }

    fn show_list(&mut self, title: String, locs: Vec<Location>) {
        let Some(st) = &self.lsp else { return };
        let root = st.mgr.root();
        let own = st.path.clone();
        let mgr = st.mgr.clone();
        let mut items = Vec::with_capacity(locs.len());
        for loc in locs {
            let line_no = loc.range.start.line as usize;
            let text = if loc.path == own {
                (line_no < self.buf.line_count()).then(|| self.buf.line(line_no).to_owned())
            } else {
                mgr.line_text(&loc.path, line_no)
            }
            .unwrap_or_default();
            let rel = loc.path.strip_prefix(&root).unwrap_or(&loc.path).display().to_string();
            let file_name = loc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            items.push(ListItem { label: format!("{rel}:{}", line_no + 1), preview: text.trim().to_owned(), file_name, loc });
        }
        let anchor = self.sel.head;
        let st = self.lsp.as_mut().expect("lsp");
        st.list = Some(ListPopup { title, items, selected: 0, top: 0, anchor });
    }

    /// 위치로 이동한다. 이 파일이면 커서를 옮기고, 다른 파일이면 `OpenAt` 사건을 쌓는다.
    fn navigate(&mut self, loc: &Location) {
        let Some(st) = &self.lsp else { return };
        if loc.path == st.path {
            let p = self.pos_from_lsp(loc.range.start);
            self.extra.clear();
            self.sel = Selection::caret(p);
            self.preferred_col = None;
            self.reveal = Some(Reveal::Center);
            self.unfold_around_cursors();
            return;
        }
        let line = st.mgr.line_text(&loc.path, loc.range.start.line as usize).unwrap_or_default();
        let col = position::char_col(&line, loc.range.start.character);
        self.events.push(EditorEvent::OpenAt { path: loc.path.clone(), line: loc.range.start.line as usize + 1, col: col + 1 });
    }

    pub(crate) fn lsp_has_popup(&self) -> bool {
        self.lsp.as_ref().is_some_and(|s| {
            s.hover.is_some() || s.completion.is_some() || s.list.is_some() || s.signature.is_some() || s.rename.is_some()
        })
    }

    /// 팝업이 열려 있을 때의 키 처리. 처리했으면 `true`.
    pub(crate) fn lsp_popup_key(&mut self, key: Key, m: Modifiers) -> bool {
        if key==Key::Escape && m.alt && !m.command && !m.ctrl {self.lsp_trigger_completion(None);return true;}
        let Some(st) = &mut self.lsp else { return false };
        if let Some(c) = &mut st.completion
            && !c.shown.is_empty()
        {
            let n = c.shown.len();
            let plain = !m.any();
            match key {
                Key::ArrowDown if plain => c.selected = (c.selected + 1) % n,
                Key::ArrowUp if plain => c.selected = (c.selected + n - 1) % n,
                Key::PageDown => c.selected = (c.selected + COMPLETION_ROWS).min(n - 1),
                Key::PageUp => c.selected = c.selected.saturating_sub(COMPLETION_ROWS),
                Key::Enter | Key::Tab if !m.any() => {
                    self.accept_completion();
                    return true;
                }
                Key::Escape => st.completion = None,
                _ => {
                    if matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::ArrowUp | Key::ArrowDown) {
                        st.completion = None;
                    }
                    return false;
                }
            }
            if let Some(c) = &mut st.completion {
                keep_visible(&mut c.top, c.selected, COMPLETION_ROWS);
            }
            return true;
        }
        if let Some(l) = &mut st.list {
            let n = l.items.len();
            match key {
                Key::ArrowDown => l.selected = (l.selected + 1) % n,
                Key::ArrowUp => l.selected = (l.selected + n - 1) % n,
                Key::Enter => {
                    let loc = l.items[l.selected].loc.clone();
                    st.list = None;
                    self.navigate(&loc);
                    return true;
                }
                Key::Escape => st.list = None,
                _ => return false,
            }
            if let Some(l) = &mut st.list {
                keep_visible(&mut l.top, l.selected, LIST_ROWS);
            }
            return true;
        }
        if key == Key::Escape && (st.hover.is_some() || st.signature.is_some()) {
            st.hover = None;
            st.signature = None;
            return true;
        }
        if st.hover.is_some() && !matches!(key, Key::K | Key::I) {
            st.hover = None;
        }
        false
    }

    // ---- 그리기 ----

    /// 한 시각 줄의 진단 물결 밑줄.
    pub(crate) fn paint_row_diagnostics(&self, p: &egui::Painter, r: &RowInfo, g: &Galley, text_x: f32, y: f32) {
        let Some(st) = &self.lsp else { return };
        if st.diags.is_empty() {
            return;
        }
        let i = r.line;
        let line = self.buf.line(i);
        let row_h = self.view.row_h;
        for d in st.diags.iter().take_while(|d| d.a.line <= i) {
            if d.b.line < i {
                continue;
            }
            let mut c0 = if d.a.line == i { d.a.col.min(line.len()) } else { 0 };
            let mut c1 = if d.b.line == i { d.b.col.min(line.len()) } else { line.len() };
            if c0 >= c1 {
                // 폭 없는 범위는 글자 하나(줄 끝이면 앞 글자)를 표시한다.
                if c0 < line.len() {
                    c1 = self.buf.next_char(Pos::new(i, c0)).col;
                } else if c0 > 0 {
                    c0 = self.buf.prev_char(Pos::new(i, c0)).col;
                } else {
                    c1 = c0;
                }
            }
            while !line.is_char_boundary(c0) {
                c0 -= 1;
            }
            while !line.is_char_boundary(c1) {
                c1 -= 1;
            }
            let (s0, s1) = (c0.max(r.start), c1.min(r.end));
            if s0 > s1 || (s0 == s1 && !(c0 == c1 && r.start <= c0 && c0 <= r.end)) {
                continue;
            }
            let text = &line[r.start..];
            let x_at = |c: usize| g.pos_from_cursor(egui::text::CCursor::new(text[..c - r.start].chars().count())).min.x;
            let x0 = text_x + x_at(s0);
            let mut x1 = text_x + x_at(s1);
            if x1 - x0 < 6.0 {
                x1 = x0 + 6.0;
            }
            let color = lsp::severity_color(d.severity);
            paint_squiggle(p, x0, x1, y + row_h - 4.0, color, d.severity == Severity::Hint);
        }
    }

    /// `a..=b` 줄 가운데 진단이 있는 줄과 가장 심한 진단 색.
    pub(crate) fn gutter_diagnostics(&self, a: usize, b: usize) -> Vec<(usize, Color32)> {
        let Some(st) = &self.lsp else { return Vec::new() };
        let mut out: Vec<(usize, Severity)> = Vec::new();
        for d in &st.diags {
            let l = d.a.line;
            if l < a || l > b || d.severity == Severity::Hint {
                continue;
            }
            match out.iter_mut().find(|(ll, _)| *ll == l) {
                Some((_, s)) => *s = (*s).min(d.severity),
                None => out.push((l, d.severity)),
            }
        }
        out.into_iter().map(|(l, s)| (l, lsp::severity_color(s))).collect()
    }

    /// 마우스가 한 자리에 머무르면 호버를 요청한다.
    pub(crate) fn hover_tracking(&mut self, ui: &Ui, resp: &Response, origin: Pos2, font: &FontId) {
        if self.lsp.is_none() {
            return;
        }
        let now = ui.input(|i| i.time);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let any_down = ui.input(|i| i.pointer.any_down());
        let over_popup = pointer.is_some_and(|p| {
            self.lsp.as_ref().is_some_and(|s| s.popup_rects.iter().any(|r| r.expand(6.0).contains(p)))
        });
        if over_popup {
            return;
        }
        let target = match pointer {
            Some(p) if resp.hovered() && !any_down => {
                let pos = self.pos_at(ui, p, origin, font);
                let pos = self.buf.clamp(pos);
                let (wa, wb) = self.buf.word_at(pos);
                let on_diag = self.lsp.as_ref().is_some_and(|s| s.diags.iter().any(|d| d.a <= pos && pos <= d.b));
                ((wa != wb && self.buf.line(pos.line)[wa.col..wb.col].chars().any(is_ident)) || on_diag).then_some((pos, wa, p))
            }
            _ => None,
        };
        let st = self.lsp.as_mut().expect("lsp");
        match target {
            None => {
                st.rest = None;
                if st.hover.as_ref().is_some_and(|h| h.from_mouse) {
                    st.hover = None;
                }
            }
            Some((pos, word_start, p)) => {
                let same_word = st.rest.as_ref().is_some_and(|(q, _, _)| self.buf.word_at(self.buf.clamp(*q)).0 == word_start);
                if !same_word {
                    st.rest = Some((pos, now, p));
                    if st.hover.as_ref().is_some_and(|h| h.from_mouse && h.anchor != word_start) {
                        st.hover = None;
                    }
                    ui.ctx().request_repaint_after(Duration::from_secs_f64(HOVER_DELAY + 0.02));
                } else if let Some((q, t0, _)) = st.rest {
                    let shown = st.hover.as_ref().is_some_and(|h| h.anchor == word_start)
                        || st.hover_req.as_ref().is_some_and(|(_, a, _)| *a == word_start);
                    if !shown && now - t0 >= HOVER_DELAY {
                        self.request_hover(q, true);
                    } else if !shown {
                        ui.ctx().request_repaint_after(Duration::from_secs_f64((HOVER_DELAY - (now - t0)).max(0.02)));
                    }
                }
            }
        }
    }

    /// 호버·완성·목록·이름 바꾸기·서명 도움말 팝업과 알림을 그린다.
    pub(crate) fn lsp_popups_ui(&mut self, ui: &mut Ui, inner: Rect) {
        if self.lsp.is_none() {
            return;
        }
        let mut rects = Vec::new();
        self.rename_ui(ui, inner, &mut rects);
        self.hover_ui(ui, inner, &mut rects);
        self.signature_ui(ui, inner, &mut rects);
        self.completion_ui(ui, inner, &mut rects);
        self.list_ui(ui, inner, &mut rects);
        self.toast_ui(ui, inner);
        self.view.overlays.extend(rects.iter().copied());
        if let Some(st) = &mut self.lsp {
            st.popup_rects = rects;
        }
    }

    /// 위치의 시각 줄 사각형(화면 좌표).
    fn row_rect_of(&mut self, ui: &Ui, p: Pos, inner: Rect) -> Rect {
        let top_left = self.screen_pos_of(ui, p, inner);
        Rect::from_min_size(top_left, vec2(1.0, self.view.row_h))
    }

    fn hover_ui(&mut self, ui: &mut Ui, inner: Rect, rects: &mut Vec<Rect>) {
        let Some(anchor) = self.lsp.as_ref().and_then(|s| s.hover.as_ref()).map(|h| h.anchor) else { return };
        let row = self.row_rect_of(ui, anchor, inner);
        if !inner.intersects(row) {
            return;
        }
        let t = Theme::current();
        let st = self.lsp.as_ref().expect("lsp");
        let h = st.hover.as_ref().expect("hover");
        let above = row.top() - inner.top() > 160.0;
        let (pivot, at) = if above { (Align2::LEFT_BOTTOM, row.left_top() - vec2(4.0, 2.0)) } else { (Align2::LEFT_TOP, row.left_bottom() + vec2(-4.0, 2.0)) };
        let diags = h.diags.clone();
        let text = h.text.clone();
        let resp = Area::new(self.id().with("lsp-hover"))
            .order(Order::Foreground)
            .pivot(pivot)
            .fixed_pos(at)
            .constrain_to(ui.ctx().content_rect())
            .show(ui.ctx(), |ui| {
                popup_frame().show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ScrollArea::vertical().max_height(320.0).auto_shrink([true, true]).show(ui, |ui| {
                        for (sev, msg) in &diags {
                            ui.horizontal_top(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                                lsp::paint_severity(ui.painter(), r.center(), *sev, 11.0);
                                ui.add(egui::Label::new(inline_job(msg, t.text, 12.5)).wrap());
                            });
                        }
                        if !diags.is_empty() && text.is_some() {
                            ui.add_space(2.0);
                            ui.separator();
                        }
                        if let Some(md) = &text {
                            markdown_ui(ui, md);
                        } else if diags.is_empty() {
                            ui.label(RichText::new(kiln_common::i18n::tr("불러오는 중…")).size(12.0).color(t.text_dim));
                        }
                    });
                });
            });
        rects.push(resp.response.rect);
    }

    fn signature_ui(&mut self, ui: &mut Ui, inner: Rect, rects: &mut Vec<Rect>) {
        let Some((sig, at)) = self.lsp.as_ref().and_then(|s| s.signature.clone()) else { return };
        if self.sel.head.line != at.line || self.lsp.as_ref().is_some_and(|s| s.completion.is_some()) {
            if self.sel.head.line != at.line {
                self.lsp.as_mut().expect("lsp").signature = None;
            }
            return;
        }
        let t = Theme::current();
        let row = self.row_rect_of(ui, self.sel.head, inner);
        let resp = Area::new(self.id().with("lsp-signature"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(row.left_top() - vec2(8.0, 2.0))
            .constrain_to(ui.ctx().content_rect())
            .show(ui.ctx(), |ui| {
                popup_frame().show(ui, |ui| {
                    ui.set_max_width(560.0);
                    let font = FontId::monospace(12.5);
                    let mut job = LayoutJob::default();
                    let normal = TextFormat { font_id: font.clone(), color: t.text, ..Default::default() };
                    match sig.active_param {
                        Some((a, b)) if b <= sig.label.len() && sig.label.is_char_boundary(a) && sig.label.is_char_boundary(b) => {
                            job.append(&sig.label[..a], 0.0, normal.clone());
                            job.append(
                                &sig.label[a..b],
                                0.0,
                                TextFormat { font_id: font.clone(), color: t.accent, underline: Stroke::new(1.0, t.accent), ..Default::default() },
                            );
                            job.append(&sig.label[b..], 0.0, normal);
                        }
                        _ => job.append(&sig.label, 0.0, normal),
                    }
                    ui.label(job);
                    if let Some(doc) = &sig.doc {
                        ui.add(egui::Label::new(RichText::new(doc).size(12.0).color(t.text_dim)).wrap());
                    }
                });
            });
        rects.push(resp.response.rect);
    }

    fn completion_ui(&mut self, ui: &mut Ui, inner: Rect, rects: &mut Vec<Rect>) {
        let Some(anchor) = self.lsp.as_ref().and_then(|s| s.completion.as_ref()).filter(|c| !c.shown.is_empty()).map(|c| c.anchor) else {
            return;
        };
        let row = self.row_rect_of(ui, anchor, inner);
        let t = Theme::current();
        let st = self.lsp.as_mut().expect("lsp");
        let c = st.completion.as_mut().expect("completion");
        let n = c.shown.len();
        let visible = n.min(COMPLETION_ROWS).min(((inner.height()-20.0)/COMPLETION_ROW_H).max(1.0) as usize);
        // Keyboard navigation uses a maximum page size; a short viewport can
        // display fewer rows. Keep the accepted candidate visible at that size.
        keep_visible(&mut c.top, c.selected, visible);
        c.top = c.top.min(n.saturating_sub(visible));
        let height = visible as f32 * COMPLETION_ROW_H + 10.0;
        let below = inner.bottom() - row.bottom() > height + 8.0 || row.top() - inner.top() < height + 8.0;
        let (pivot, at) = if below { (Align2::LEFT_TOP, row.left_bottom() + vec2(-26.0, 2.0)) } else { (Align2::LEFT_BOTTOM, row.left_top() - vec2(26.0, 2.0)) };
        let rows: Vec<(usize, CompletionItem)> =
            (c.top..(c.top + visible).min(n)).map(|k| (k, c.items[c.shown[k]].clone())).collect();
        let selected = c.selected;
        let mut clicked: Option<usize> = None;
        let mut scroll: f32 = 0.0;
        let width=(inner.width()-16.0).clamp(80.0,440.0);
        let resp = Area::new(self.id().with("lsp-completion"))
            .order(Order::Foreground)
            .pivot(pivot)
            .fixed_pos(at)
            .constrain_to(inner.intersect(ui.ctx().content_rect()))
            .show(ui.ctx(), |ui| {
                popup_frame().inner_margin(5).show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    for (k, it) in &rows {
                        let (r, resp) = ui.allocate_exact_size(vec2(width, COMPLETION_ROW_H), Sense::click());
                        resp.widget_info(||egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel,true,*k==selected,format!("{} · {}",it.label,it.detail.as_deref().unwrap_or("LSP"))));
                        resp.clone().on_hover_text(format!("{} · {}",it.label,it.detail.as_deref().unwrap_or("LSP")));
                        let p = ui.painter();
                        if *k == selected {
                            p.rect_filled(r, 6.0, t.accent_soft(if t.dark { 44 } else { 30 }));
                        } else if resp.hovered() {
                            p.rect_filled(r, 6.0, t.bg_hover);
                        }
                        let (badge, color) = kind_badge(it.kind);
                        let br = Rect::from_center_size(pos2(r.left() + 15.0, r.center().y), vec2(20.0, 17.0));
                        p.rect_filled(br, 5.0, kiln_common::widgets::tint(color, if t.dark { 0.18 } else { 0.12 }));
                        p.text(br.center(), Align2::CENTER_CENTER, badge, kiln_common::fonts::semibold(10.0), color);
                        let label_font = FontId::monospace(12.5);
                        let label = p.layout_no_wrap(elide(&it.label,((width-48.0)/8.0).max(2.0) as usize), label_font, t.text);
                        let lw = label.size().x;
                        p.galley(pos2(r.left() + 32.0, r.center().y - label.size().y / 2.0), label, t.text);
                        if let Some(d) = &it.detail {
                            let room = width - 32.0 - lw - 18.0;
                            if room > 40.0 {
                                let d = elide(d, (room / 6.6) as usize);
                                p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, d, FontId::proportional(12.0), t.text_faint);
                            }
                        }
                        if resp.clicked() {
                            clicked = Some(*k);
                        }
                    }
                    if ui.rect_contains_pointer(ui.min_rect()) {
                        scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    }
                });
            });
        rects.push(resp.response.rect);
        let st = self.lsp.as_mut().expect("lsp");
        if let Some(c) = &mut st.completion {
            if scroll != 0.0 {
                let steps = (scroll / COMPLETION_ROW_H).round() as isize;
                c.top = (c.top as isize - steps).clamp(0, n.saturating_sub(visible) as isize) as usize;
                // Scrolling must not snap back to an off-screen selection on
                // the next frame, or allow Enter to accept an invisible row.
                c.selected = c.selected.clamp(c.top, (c.top + visible - 1).min(n - 1));
            }
            if let Some(k) = clicked {
                c.selected = k;
            }
        }
        if clicked.is_some() {
            self.accept_completion();
            ui.memory_mut(|m| m.request_focus(self.id()));
        }
    }

    fn list_ui(&mut self, ui: &mut Ui, inner: Rect, rects: &mut Vec<Rect>) {
        let Some(anchor) = self.lsp.as_ref().and_then(|s| s.list.as_ref()).map(|l| l.anchor) else { return };
        let row = self.row_rect_of(ui, anchor, inner);
        let t = Theme::current();
        let st = self.lsp.as_ref().expect("lsp");
        let l = st.list.as_ref().expect("list");
        let n = l.items.len();
        let visible = n.min(LIST_ROWS);
        let rows: Vec<(usize, String, String, String)> = (l.top..(l.top + visible).min(n))
            .map(|k| (k, l.items[k].label.clone(), l.items[k].preview.clone(), l.items[k].file_name.clone()))
            .collect();
        let (title, selected) = (l.title.clone(), l.selected);
        let width = (inner.width() - 40.0).clamp(320.0, 640.0);
        let at = pos2(inner.left() + 24.0, row.bottom() + 4.0);
        let mut clicked = None;
        let mut close = false;
        let resp = Area::new(self.id().with("lsp-list"))
            .order(Order::Foreground)
            .fixed_pos(at)
            .constrain_to(ui.ctx().content_rect())
            .show(ui.ctx(), |ui| {
                popup_frame().inner_margin(8).show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&title).font(kiln_common::fonts::semibold(13.0)).color(t.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_kit::icon_button(ui, ui_kit::Icon::Close, kiln_common::i18n::tr("닫기 (Escape)")).clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    for (k, label, preview, name) in &rows {
                        let (r, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
                        let p = ui.painter();
                        if *k == selected {
                            p.rect_filled(r, 6.0, t.accent_soft(if t.dark { 44 } else { 30 }));
                        } else if resp.hovered() {
                            p.rect_filled(r, 6.0, t.bg_hover);
                        }
                        ui_kit::paint_file_badge(p, Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(18.0, 14.0)), name);
                        let lg = p.layout_no_wrap(label.clone(), FontId::proportional(12.0), t.text_dim);
                        let lw = lg.size().x;
                        p.galley(pos2(r.left() + 28.0, r.center().y - lg.size().y / 2.0), lg, t.text_dim);
                        let room = width - 28.0 - lw - 20.0;
                        if room > 30.0 {
                            let pv = elide(preview, (room / 7.2) as usize);
                            p.text(pos2(r.left() + 28.0 + lw + 12.0, r.center().y), Align2::LEFT_CENTER, pv, FontId::monospace(12.0), t.text);
                        }
                        if resp.clicked() {
                            clicked = Some(*k);
                        }
                    }
                    if n > visible {
                        ui.add_space(3.0);
                        ui.label(RichText::new(kiln_common::trf!("↑↓ 로 더 보기 ({}/{n})", selected + 1)).size(11.5).color(t.text_faint));
                    }
                });
            });
        rects.push(resp.response.rect);
        if close {
            self.lsp.as_mut().expect("lsp").list = None;
        } else if let Some(k) = clicked {
            let st = self.lsp.as_mut().expect("lsp");
            let loc = st.list.as_ref().expect("list").items[k].loc.clone();
            st.list = None;
            self.navigate(&loc);
            ui.memory_mut(|m| m.request_focus(self.id()));
        }
    }

    fn rename_id(&self) -> Id {
        self.id().with("lsp-rename")
    }

    fn rename_ui(&mut self, ui: &mut Ui, inner: Rect, rects: &mut Vec<Rect>) {
        let Some(anchor) = self.lsp.as_ref().and_then(|s| s.rename.as_ref()).map(|r| r.anchor) else { return };
        let row = self.row_rect_of(ui, anchor, inner);
        let t = Theme::current();
        let id = self.rename_id();
        let st = self.lsp.as_mut().expect("lsp");
        let r = st.rename.as_mut().expect("rename");
        let first = !r.started;
        r.started = true;
        let mut submit = false;
        let mut cancel = false;
        let resp = Area::new(self.id.with("lsp-rename-area"))
            .order(Order::Foreground)
            .fixed_pos(row.left_bottom() + vec2(-6.0, 2.0))
            .constrain_to(ui.ctx().content_rect())
            .show(ui.ctx(), |ui| {
                popup_frame().inner_margin(10).show(ui, |ui| {
                    ui.set_width(260.0);
                    ui.label(RichText::new(kiln_common::i18n::tr("이름 바꾸기")).font(kiln_common::fonts::semibold(12.5)).color(t.text));
                    ui.add_space(4.0);
                    let focused = ui.memory(|m| m.has_focus(id));
                    ui_kit::field_frame(focused).show(ui, |ui| {
                        ui.set_width(248.0);
                        let out = ui_kit::bare_text_edit(&mut r.text, id, kiln_common::i18n::tr("새 이름"), false).desired_width(240.0).show(ui);
                        if first {
                            out.response.request_focus();
                            let mut state = out.state.clone();
                            let n = r.text.chars().count();
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                            state.store(ui.ctx(), id);
                        } else if out.response.lost_focus() && !ui.input(|i| i.key_pressed(Key::Enter)) {
                            cancel = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new(kiln_common::i18n::tr("Enter 로 바꾸기, Esc 로 취소")).size(11.5).color(t.text_faint));
                    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    if enter {
                        submit = true;
                    }
                    if esc {
                        cancel = true;
                    }
                });
            });
        rects.push(resp.response.rect);
        if submit || cancel {
            let r = self.lsp.as_mut().expect("lsp").rename.take().expect("rename");
            if submit {
                self.submit_rename(r.text, r.anchor);
            }
            ui.memory_mut(|m| m.request_focus(self.id()));
        }
    }

    fn toast_ui(&mut self, ui: &mut Ui, inner: Rect) {
        let Some((msg, _)) = self.lsp.as_ref().and_then(|s| s.toast.clone()) else { return };
        let t = Theme::current();
        Area::new(self.id().with("lsp-toast"))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(inner.right_bottom() - vec2(16.0, 12.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                popup_frame().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.label(RichText::new(msg).font(kiln_common::fonts::medium(12.5)).color(t.text));
                });
            });
    }
}

fn keep_visible(top: &mut usize, selected: usize, rows: usize) {
    if selected < *top {
        *top = selected;
    } else if selected >= *top + rows {
        *top = selected + 1 - rows;
    }
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn popup_frame() -> Frame {
    let t = Theme::current();
    Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(10)
        .shadow(t.shadow())
        .inner_margin(10)
}

/// 완성 항목 종류 표시(글자, 색).
fn kind_badge(kind: Option<u32>) -> (&'static str, Color32) {
    let t = Theme::current();
    match kind.unwrap_or(1) {
        2..=4 => ("fn", t.purple),
        5 | 10 => ("fd", t.blue),
        6 => ("v", t.blue),
        7 | 22 => ("S", t.yellow),
        8 | 25 => ("T", t.yellow),
        9 => ("md", t.text_dim),
        13 => ("E", t.yellow),
        20 | 21 => ("c", t.orange),
        14 => ("kw", t.purple),
        15 => ("sn", t.green),
        17 | 19 => ("f", t.text_dim),
        _ => ("ab", t.text_dim),
    }
}

/// 물결 밑줄. `dotted` 면 점선으로 그린다.
fn paint_squiggle(p: &egui::Painter, x0: f32, x1: f32, y: f32, color: Color32, dotted: bool) {
    if dotted {
        let mut x = x0;
        while x < x1 {
            p.circle_filled(pos2(x + 0.75, y + 1.0), 0.75, color);
            x += 3.0;
        }
        return;
    }
    let step = 2.0;
    let amp = 1.4;
    let mut pts = Vec::new();
    let mut x = x0;
    let mut up = false;
    while x <= x1 {
        pts.push(pos2(x, if up { y - amp } else { y + amp }));
        up = !up;
        x += step;
    }
    if pts.len() >= 2 {
        p.add(egui::Shape::line(pts, Stroke::new(1.1, color)));
    }
}

/// 마크다운 비슷한 호버 텍스트를 그린다: ``` 코드 블록, `코드`, **굵게**, 제목, 목록.
fn markdown_ui(ui: &mut Ui, md: &str) {
    let t = Theme::current();
    let mut in_code = false;
    let mut code = String::new();
    let mut para: Vec<String> = Vec::new();
    let flush_para = |ui: &mut Ui, para: &mut Vec<String>| {
        if para.is_empty() {
            return;
        }
        let text = para.join(" ");
        para.clear();
        ui.add(egui::Label::new(inline_job(&text, t.text, 12.5)).wrap());
    };
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_code {
                let body = code.trim_end_matches('\n').to_owned();
                code.clear();
                Frame::new().fill(t.bg_hover).corner_radius(6).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(body).monospace().size(12.5).color(t.text)).wrap());
                });
                in_code = false;
            } else {
                flush_para(ui, &mut para);
                in_code = true;
            }
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            flush_para(ui, &mut para);
            continue;
        }
        if trimmed.chars().all(|c| c == '-' || c == '_' || c == '*') && trimmed.len() >= 3 {
            flush_para(ui, &mut para);
            ui.separator();
            continue;
        }
        if let Some(h) = trimmed.strip_prefix('#') {
            flush_para(ui, &mut para);
            ui.label(RichText::new(h.trim_start_matches('#').trim()).font(kiln_common::fonts::semibold(13.5)).color(t.text));
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush_para(ui, &mut para);
            ui.add(egui::Label::new(inline_job(&format!("• {item}"), t.text, 12.5)).wrap());
            continue;
        }
        para.push(trimmed.to_owned());
    }
    if in_code && !code.is_empty() {
        ui.label(RichText::new(code.trim_end()).monospace().size(12.5).color(t.text));
    }
    flush_para(ui, &mut para);
}

/// `코드` 와 **굵게** 를 서식으로 바꾼 한 문단.
fn inline_job(text: &str, color: Color32, size: f32) -> LayoutJob {
    let t = Theme::current();
    let mut job = LayoutJob::default();
    let normal = TextFormat { font_id: FontId::proportional(size), color, ..Default::default() };
    let code = TextFormat { font_id: FontId::monospace(size - 0.5), color: t.orange, background: t.bg_hover, ..Default::default() };
    let bold = TextFormat { font_id: kiln_common::fonts::semibold(size), color: t.text, ..Default::default() };
    let mut rest = text;
    while !rest.is_empty() {
        let next_code = rest.find('`');
        let next_bold = rest.find("**");
        match (next_code, next_bold) {
            (Some(c), b) if b.is_none_or(|b| c < b) => {
                job.append(&rest[..c], 0.0, normal.clone());
                let after = &rest[c + 1..];
                match after.find('`') {
                    Some(e) => {
                        job.append(&after[..e], 0.0, code.clone());
                        rest = &after[e + 1..];
                    }
                    None => {
                        job.append(after, 0.0, normal.clone());
                        rest = "";
                    }
                }
            }
            (_, Some(b)) => {
                job.append(&rest[..b], 0.0, normal.clone());
                let after = &rest[b + 2..];
                match after.find("**") {
                    Some(e) => {
                        job.append(&after[..e], 0.0, bold.clone());
                        rest = &after[e + 2..];
                    }
                    None => {
                        job.append(after, 0.0, normal.clone());
                        rest = "";
                    }
                }
            }
            _ => {
                job.append(rest, 0.0, normal.clone());
                rest = "";
            }
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::Range;

    fn r(l0: u32, c0: u32, l1: u32, c1: u32) -> Range {
        Range { start: Position::new(l0, c0), end: Position::new(l1, c1) }
    }

    #[test]
    fn lsp_edits_apply_in_one_undo_step_with_utf16_columns() {
        let mut e = Editor::from_text("t.rs", "let 한글 = 1;\nfoo(한글);\n");
        e.set_selection(Selection::caret(Pos::new(1, 4)));
        e.apply_lsp_edits(&[
            TextEdit { range: r(0, 4, 0, 6), new_text: "name".into() },
            TextEdit { range: r(1, 4, 1, 6), new_text: "name".into() },
            TextEdit { range: r(0, 0, 0, 0), new_text: "// x\n".into() },
        ]);
        assert_eq!(e.text(), "// x\nlet name = 1;\nfoo(name);\n");
        assert_eq!(e.sel.head, Pos::new(2, 4));
        e.undo();
        assert_eq!(e.text(), "let 한글 = 1;\nfoo(한글);\n");
    }

    #[test]
    fn inserts_at_same_position_keep_array_order() {
        let mut e = Editor::from_text("t.rs", "x");
        e.apply_lsp_edits(&[
            TextEdit { range: r(0, 0, 0, 0), new_text: "a".into() },
            TextEdit { range: r(0, 0, 0, 0), new_text: "b".into() },
        ]);
        assert_eq!(e.text(), "abx");
    }

    #[test]
    fn subsequence_and_elide() {
        assert!(is_subsequence("pln", "println"));
        assert!(!is_subsequence("xyz", "println"));
        assert_eq!(elide("abcdef", 4), "abc…");
        assert_eq!(elide("ab", 4), "ab");
    }
}

#[cfg(test)]
mod completion_safety_tests {
    use super::*;
    #[test]
    fn delayed_completion_is_rejected_after_edits_or_caret_movement(){
        for move_only in [false,true]{
            let mut ed=Editor::from_text("fixture.rs","someprefix other");ed.goto(1,5);ed.local_completion();
            let (tx,pending)=Pending::channel();
            let version=ed.buf.version();let head=ed.sel.head;
            ed.lsp.as_mut().unwrap().completion=None;
            ed.lsp.as_mut().unwrap().completion_req=Some((pending,Pos::new(0,0),version,head));
            if move_only {ed.goto(1,7);}else{ed.insert_text("x");}
            let before=ed.text();
            let item=super::super::local_completion::items("Rust",&[],"ret").remove(0);
            tx.send(Ok(lsp::CompletionList{is_incomplete:false,items:vec![item]})).unwrap();
            ed.poll_lsp();assert!(ed.lsp.as_ref().unwrap().completion.is_none());assert_eq!(ed.text(),before);
        }
    }
    #[test]
    fn full_replacement_and_additional_import_are_one_undo() {
        let mut ed=Editor::from_text("fixture.rs","// 한글🙂\nfoobar");
        ed.set_selection(Selection::caret(Pos::new(1,3)));
        ed.local_completion();
        let item=CompletionItem{label:"finished".into(),detail:None,kind:None,filter_text:String::new(),sort_text:String::new(),insert_text:"finished".into(),cursor_offset:None,
            edit:Some(TextEdit{range:Range{start:Position{line:1,character:0},end:Position{line:1,character:6}},new_text:"finished".into()}),
            additional_edits:vec![TextEdit{range:Range{start:Position{line:0,character:0},end:Position{line:0,character:0}},new_text:"use thing;\n".into()}]};
        ed.lsp.as_mut().unwrap().completion=Some(CompletionState{items:vec![item],shown:vec![0],selected:0,top:0,anchor:Pos::new(1,0),incomplete:false,version:ed.buf.version(),head:ed.sel.head});
        ed.accept_completion();assert_eq!(ed.text(),"use thing;\n// 한글🙂\nfinished");
        ed.undo();assert_eq!(ed.text(),"// 한글🙂\nfoobar");
    }
    #[test]
    fn local_keywords_and_words_work_without_a_server_and_keep_undo() {
        let mut ed=Editor::from_text("script.py","example_word\nret");ed.goto(2,4);ed.local_completion();
        let c=ed.lsp.as_ref().unwrap().completion.as_ref().unwrap();assert!(c.items.iter().any(|i|i.label=="return"));
        ed.accept_completion();assert_eq!(ed.text(),"example_word\nreturn");ed.undo();assert_eq!(ed.text(),"example_word\nret");
    }
}
