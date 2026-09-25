//! diff 뷰어(DiffView): 파일/커밋/패치 diff 를 통합·좌우 분할 모드로 그린다.

use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, Rect, RichText, Sense, Stroke, TextFormat, Ui,
    pos2, text::LayoutJob, vec2,
};
use kiln_common::Task;

use super::panel::one_line_job;
use super::widgets::*;
use crate::cmd::{GitResult, Mode, git};
use crate::diff::{DiffLine, FileChange, FileDiff, LineKind, parse_diff};
use crate::repo::{self, CommitDetail, HunkAction};
use crate::util::{now_unix, relative_time};

const LINE_H: f32 = 19.0;
const HUNK_H: f32 = 26.0;
const FILE_H: f32 = 34.0;
const NOTE_H: f32 = 30.0;
const GAP_H: f32 = 12.0;
const FONT_SIZE: f32 = 12.5;
const MAX_LINE_CHARS: usize = 1200;

/// 표시 방식.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMode {
    Unified,
    SideBySide,
}

#[derive(Clone, Debug)]
enum Source {
    File { path: PathBuf, staged: bool },
    Commit(String),
    Patch { title: String, text: String },
}

struct Loaded {
    top: PathBuf,
    rel: Option<String>,
    files: Vec<FileDiff>,
    commit: Option<CommitDetail>,
}

#[derive(Clone, Copy, Debug)]
enum Row {
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    Split(usize, usize, Option<usize>, Option<usize>),
    Note(usize),
    Gap,
}

fn row_height(r: &Row) -> f32 {
    match r {
        Row::File(_) => FILE_H,
        Row::Hunk(..) => HUNK_H,
        Row::Line(..) | Row::Split(..) => LINE_H,
        Row::Note(_) => NOTE_H,
        Row::Gap => GAP_H,
    }
}

/// diff 뷰어.
pub struct DiffView {
    root: PathBuf,
    source: Source,
    load: Option<Task<GitResult<Loaded>>>,
    started: bool,
    top: PathBuf,
    rel: Option<String>,
    files: Vec<FileDiff>,
    commit: Option<CommitDetail>,
    error: Option<String>,
    mode: DiffMode,
    rows: Vec<Row>,
    row_y: Vec<f32>,
    total_h: f32,
    max_chars: usize,
    action: Option<Task<GitResult<()>>>,
    action_error: Option<String>,
    scroll_to: Option<f32>,
    now_override: Option<i64>,
    show_file_list: bool,
}

impl DiffView {
    fn new(root: &Path, source: Source) -> Self {
        Self {
            root: root.to_path_buf(),
            source,
            load: None,
            started: false,
            top: root.to_path_buf(),
            rel: None,
            files: Vec::new(),
            commit: None,
            error: None,
            mode: DiffMode::Unified,
            rows: Vec::new(),
            row_y: Vec::new(),
            total_h: 0.0,
            max_chars: 0,
            action: None,
            action_error: None,
            scroll_to: None,
            now_override: None,
            show_file_list: true,
        }
    }

    /// 작업트리(`staged=false`) 또는 인덱스(`staged=true`) 파일 diff. `path` 는 절대 경로나 루트 기준 상대 경로.
    pub fn for_file(root: &Path, path: &Path, staged: bool) -> Self {
        Self::new(root, Source::File { path: path.to_path_buf(), staged })
    }

    /// 커밋 diff.
    pub fn for_commit(root: &Path, sha: &str) -> Self {
        Self::new(root, Source::Commit(sha.to_string()))
    }

    /// 이미 가진 unified diff 텍스트(예: `gh pr diff`)를 보여준다. 헝크 조작은 비활성.
    pub fn from_patch(root: &Path, title: &str, patch: String) -> Self {
        Self::new(root, Source::Patch { title: title.to_string(), text: patch })
    }

    /// 다시 읽는다.
    pub fn reload(&mut self) {
        self.started = false;
    }

    pub fn is_loading(&mut self) -> bool {
        !self.started || self.load.as_mut().is_some_and(|t| t.is_pending()) || self.action.as_mut().is_some_and(|t| t.is_pending())
    }

    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: DiffMode) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild_rows();
        }
    }

    /// 파싱된 파일 diff 목록.
    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    /// 파일 목록 패널 표시 여부(커밋/패치 모드).
    pub fn set_show_file_list(&mut self, show: bool) {
        self.show_file_list = show;
    }

    fn is_worktree_file(&self) -> Option<bool> {
        match &self.source {
            Source::File { staged, .. } => Some(*staged),
            _ => None,
        }
    }

    fn start_load(&mut self, ctx: &egui::Context) {
        self.started = true;
        let root = self.root.clone();
        let source = self.source.clone();
        self.load = Some(Task::spawn(ctx, move || load(&root, &source)));
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.start_load(ctx);
        }
        if let Some(t) = &mut self.load
            && let Some(r) = t.take()
        {
            self.load = None;
            match r {
                Ok(l) => {
                    self.top = l.top;
                    self.rel = l.rel;
                    self.files = l.files;
                    self.commit = l.commit;
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    self.files.clear();
                }
            }
            self.rebuild_rows();
        }
        if let Some(t) = &mut self.action
            && let Some(r) = t.take()
        {
            self.action = None;
            if let Err(e) = r {
                self.action_error = Some(e.to_string());
            }
            self.start_load(ctx);
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let multi = self.is_worktree_file().is_none();
        let mut max_chars = 0usize;
        for (fi, f) in self.files.iter().enumerate() {
            if multi {
                if fi > 0 {
                    rows.push(Row::Gap);
                }
                rows.push(Row::File(fi));
            }
            if f.binary || f.hunks.is_empty() {
                rows.push(Row::Note(fi));
                continue;
            }
            for (hi, h) in f.hunks.iter().enumerate() {
                rows.push(Row::Hunk(fi, hi));
                for l in &h.lines {
                    max_chars = max_chars.max(display_len(&l.text));
                }
                match self.mode {
                    DiffMode::Unified => {
                        for li in 0..h.lines.len() {
                            rows.push(Row::Line(fi, hi, li));
                        }
                    }
                    DiffMode::SideBySide => split_rows(&h.lines, fi, hi, &mut rows),
                }
            }
        }
        let mut y = 0.0;
        self.row_y = rows
            .iter()
            .map(|r| {
                let v = y;
                y += row_height(r);
                v
            })
            .collect();
        self.total_h = y;
        self.rows = rows;
        self.max_chars = max_chars.min(MAX_LINE_CHARS);
    }

    fn apply(&mut self, ctx: &egui::Context, fi: usize, hi: usize, action: HunkAction) {
        let Some(f) = self.files.get(fi).cloned() else { return };
        let top = self.top.clone();
        self.action_error = None;
        self.action = Some(Task::spawn(ctx, move || repo::apply_hunk(&top, &f, hi, action)));
    }

    fn now(&self) -> i64 {
        self.now_override.unwrap_or_else(now_unix)
    }

    /// 뷰를 그린다.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.pump(ui.ctx());
        let t = theme();
        egui::Frame::new().fill(t.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            self.ui_toolbar(ui);
            if let Some(e) = self.action_error.clone() {
                egui::Frame::new().inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
                    if banner(ui, BannerKind::Error, "헝크를 적용할 수 없습니다", Some(&e), true) {
                        self.action_error = None;
                    }
                });
            }
            if let Some(e) = &self.error {
                let e = e.clone();
                egui::Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                    banner(ui, BannerKind::Error, "diff를 불러올 수 없습니다", Some(&e), false);
                });
                return;
            }
            if self.load.is_some() && self.files.is_empty() {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    spinner(ui, 14.0);
                    ui.label(dim("diff 불러오는 중…"));
                });
                return;
            }
            let multi = self.is_worktree_file().is_none();
            if multi && self.show_file_list && !self.files.is_empty() {
                egui::Panel::left(Id::new("kiln_diff_files").with(self.source_key()))
                    .resizable(true)
                    .default_size(240.0)
                    .size_range(160.0..=480.0)
                    .frame(egui::Frame::new().fill(t.bg_panel).inner_margin(Margin::ZERO))
                    .show(ui, |ui| self.ui_file_list(ui));
            }
            egui::CentralPanel::no_frame().show(ui, |ui| {
                if self.files.is_empty() {
                    empty_state(ui, "변경 사항 없음", "이 diff에 표시할 내용이 없습니다.");
                    return;
                }
                self.ui_rows(ui);
            });
        });
    }

    fn source_key(&self) -> String {
        match &self.source {
            Source::File { path, staged } => format!("{}:{staged}", path.display()),
            Source::Commit(s) => s.clone(),
            Source::Patch { title, .. } => title.clone(),
        }
    }

    fn ui_toolbar(&mut self, ui: &mut Ui) {
        let t = theme();
        let now = self.now();
        egui::Frame::new()
            .fill(t.bg_panel)
            .inner_margin(Margin { left: 12, right: 10, top: 8, bottom: 8 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(c) = self.commit.clone() {
                    commit_header(ui, &c, now, self.files.len(), self.files.iter().map(|f| (f.added(), f.removed())));
                    ui.add_space(6.0);
                }
                let row_w = ui.available_width();
                ui.allocate_ui_with_layout(vec2(row_w, 28.0), Layout::left_to_right(Align::Center), |ui| {
                    match &self.source {
                        Source::File { staged, .. } => {
                            let path = self.rel.clone().unwrap_or_default();
                            let (dir, name) = match path.rfind('/') {
                                Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
                                None => (String::new(), path.clone()),
                            };
                            ui.label(RichText::new(name).size(14.0).strong().color(t.text));
                            if !dir.is_empty() {
                                ui.label(RichText::new(dir).size(12.0).color(t.text_faint));
                            }
                            let (lbl, c) = if *staged { ("스테이징됨", t.green) } else { ("작업 트리", t.yellow) };
                            outline_badge(ui, lbl, c);
                        }
                        Source::Commit(_) => {}
                        Source::Patch { title, .. } => {
                            ui.label(RichText::new(title).size(13.0).strong().color(t.text));
                        }
                    }
                    let (a, d) = self.files.iter().fold((0, 0), |(a, d), f| (a + f.added(), d + f.removed()));
                    if !self.files.is_empty() && self.commit.is_none() {
                        ui.label(RichText::new(format!("+{a}")).color(t.green).size(12.0).monospace());
                        ui.label(RichText::new(format!("−{d}")).color(t.red).size(12.0).monospace());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut mode = self.mode;
                        if segmented(ui, &mut mode, &[(DiffMode::SideBySide, "나란히"), (DiffMode::Unified, "통합")]) {
                            self.set_mode(mode);
                        }
                        if self.load.is_some() || self.action.is_some() {
                            spinner(ui, 12.0);
                        } else if icon_button(ui, Icon::Refresh, "다시 불러오기").clicked() {
                            self.reload();
                        }
                    });
                });
            });
        ui.add(egui::Separator::default().spacing(0.0));
    }

    fn ui_file_list(&mut self, ui: &mut Ui) {
        let t = theme();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(format!("파일 {}개", self.files.len())).size(10.5).color(t.text_faint));
        });
        ui.add_space(2.0);
        egui::ScrollArea::vertical().id_salt("diff_file_list").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (fi, f) in self.files.iter().enumerate() {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover);
                }
                let p = ui.painter();
                let letter = f.change.letter();
                p.text(rect.left_center() + vec2(14.0, 0.0), Align2::CENTER_CENTER, letter, FontId::monospace(11.5), status_color(letter));
                let path = f.path();
                let (dir, name) = match path.rfind('/') {
                    Some(i) => (&path[..i], &path[i + 1..]),
                    None => ("", path),
                };
                let stats = format!("+{} −{}", f.added(), f.removed());
                let sg = p.layout_no_wrap(stats, FontId::monospace(10.5), t.text_faint);
                let sw = sg.size().x;
                p.galley(pos2(rect.right() - 8.0 - sw, rect.center().y - sg.size().y / 2.0), sg, t.text_faint);
                let d = if dir.is_empty() { String::new() } else { format!("  {dir}") };
                let g = p.layout_job(one_line_job(&[(name, 12.5, t.text), (&d, 11.0, t.text_faint)], w - 40.0 - sw));
                p.galley(pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0), g, t.text);
                if resp.clicked()
                    && let Some(i) = self.rows.iter().position(|r| matches!(r, Row::File(x) if *x == fi))
                {
                    self.scroll_to = Some(self.row_y[i]);
                }
                resp.on_hover_text(path);
            }
        });
    }

    fn ui_rows(&mut self, ui: &mut Ui) {
        let t = theme();
        let font = FontId::monospace(FONT_SIZE);
        let cw = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let max_no = self
            .files
            .iter()
            .flat_map(|f| f.hunks.iter())
            .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines))
            .max()
            .unwrap_or(1);
        let digits = max_no.to_string().len().max(3) as f32;
        let num_w = digits * cw + 14.0;
        let staged = self.is_worktree_file();
        let mut hunk_action: Option<(usize, usize, HunkAction)> = None;

        let mut sa = egui::ScrollArea::both().id_salt(("diff_rows", self.source_key())).auto_shrink([false, false]);
        if let Some(y) = self.scroll_to.take() {
            sa = sa.vertical_scroll_offset(y);
        }
        let mode = self.mode;
        let text_w = self.max_chars as f32 * cw;
        sa.show_viewport(ui, |ui, viewport| {
            let content_w = match mode {
                DiffMode::Unified => (num_w * 2.0 + 20.0 + text_w + 24.0).max(viewport.width()),
                DiffMode::SideBySide => viewport.width().max(2.0 * (num_w + 20.0 + 200.0)),
            };
            let (full, _) = ui.allocate_exact_size(vec2(content_w, self.total_h), Sense::hover());
            let origin = full.min;
            let start = self.row_y.partition_point(|&y| y + 40.0 < viewport.min.y);
            let clip = ui.clip_rect();
            for i in start..self.rows.len() {
                let y = self.row_y[i];
                if y > viewport.max.y {
                    break;
                }
                let row = self.rows[i];
                let h = row_height(&row);
                let rect = Rect::from_min_size(origin + vec2(0.0, y), vec2(content_w, h));
                // 가로 스크롤과 무관하게 보이는 영역 폭
                let vis = Rect::from_min_max(pos2(clip.left(), rect.top()), pos2(clip.right(), rect.bottom()));
                match row {
                    Row::File(fi) => self.paint_file_header(ui, vis, fi),
                    Row::Hunk(fi, hi) => {
                        let h = &self.files[fi].hunks[hi];
                        ui.painter().rect_filled(vis, CornerRadius::ZERO, alpha(t.accent, 0.08));
                        ui.painter().hline(vis.x_range(), vis.top(), Stroke::new(1.0, alpha(t.accent, 0.18)));
                        let hdr = format!(
                            "@@ -{},{} +{},{} @@",
                            h.old_start, h.old_lines, h.new_start, h.new_lines
                        );
                        let job = one_line_job(
                            &[(&hdr, 11.5, alpha(t.accent, 0.85)), (&format!("  {}", h.section()), 11.5, t.text_faint)],
                            vis.width() - 260.0,
                        );
                        let g = ui.painter().layout_job(job);
                        ui.painter().galley(pos2(vis.left() + 12.0, vis.center().y - g.size().y / 2.0), g, t.text);
                        if let Some(staged) = staged {
                            let busy = self.action.is_some();
                            let mut x = vis.right() - 10.0;
                            let mut hbtn = |ui: &mut Ui, label: &str, danger: bool| -> bool {
                                let font = FontId::proportional(11.5);
                                let gw = ui.painter().layout_no_wrap(label.to_string(), font.clone(), t.text).size().x;
                                let r = Rect::from_min_size(pos2(x - gw - 16.0, vis.top() + 3.0), vec2(gw + 16.0, HUNK_H - 6.0));
                                x = r.left() - 6.0;
                                let resp = ui.interact(r, Id::new(("hunk_btn", fi, hi, label)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !busy, format!("{label} {}", hi + 1)));
                                let enabled = !busy;
                                let (bg, fg) = if resp.hovered() && enabled {
                                    (if danger { alpha(t.red, 0.25) } else { t.bg_hover }, t.text)
                                } else {
                                    (t.bg_elevated, if enabled { t.text_dim } else { t.text_faint })
                                };
                                ui.painter().rect(r, CornerRadius::same(4), bg, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font, fg);
                                enabled && resp.clicked()
                            };
                            if staged {
                                if hbtn(ui, "헝크 스테이징 취소", false) {
                                    hunk_action = Some((fi, hi, HunkAction::Unstage));
                                }
                            } else {
                                if hbtn(ui, "헝크 스테이징", false) {
                                    hunk_action = Some((fi, hi, HunkAction::Stage));
                                }
                                if hbtn(ui, "되돌리기", true) {
                                    hunk_action = Some((fi, hi, HunkAction::Revert));
                                }
                            }
                        }
                    }
                    Row::Line(fi, hi, li) => {
                        let l = &self.files[fi].hunks[hi].lines[li];
                        paint_unified_line(ui, rect, vis, l, num_w, &font);
                    }
                    Row::Split(fi, hi, a, b) => {
                        let lines = &self.files[fi].hunks[hi].lines;
                        let half = vis.width() / 2.0;
                        let left = Rect::from_min_size(vis.min, vec2(half, h));
                        let right = Rect::from_min_size(vis.min + vec2(half, 0.0), vec2(vis.width() - half, h));
                        paint_split_side(ui, left, a.map(|i| &lines[i]), true, num_w, &font);
                        paint_split_side(ui, right, b.map(|i| &lines[i]), false, num_w, &font);
                        ui.painter().vline(right.left(), right.y_range(), Stroke::new(1.0, t.border));
                    }
                    Row::Note(fi) => {
                        let f = &self.files[fi];
                        let msg = if f.binary {
                            "바이너리 파일은 표시하지 않습니다"
                        } else if f.change == FileChange::Renamed {
                            "내용 변경 없이 이름만 바뀌었습니다"
                        } else if f.old_mode != f.new_mode {
                            "파일 모드가 바뀌었습니다"
                        } else {
                            "내용 변경 없음"
                        };
                        ui.painter().text(
                            vis.left_center() + vec2(16.0, 0.0),
                            Align2::LEFT_CENTER,
                            msg,
                            FontId::proportional(12.0),
                            t.text_faint,
                        );
                    }
                    Row::Gap => {}
                }
            }
        });
        if let Some((fi, hi, a)) = hunk_action {
            self.apply(ui.ctx(), fi, hi, a);
        }
    }

    fn paint_file_header(&self, ui: &Ui, vis: Rect, fi: usize) {
        let t = theme();
        let f = &self.files[fi];
        let r = vis.shrink2(vec2(0.0, 2.0));
        ui.painter().rect(r, CornerRadius::ZERO, t.bg_elevated, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let letter = f.change.letter();
        let c = status_color(letter);
        let p = ui.painter();
        let br = Rect::from_center_size(r.left_center() + vec2(20.0, 0.0), vec2(18.0, 18.0));
        p.rect_filled(br, CornerRadius::same(4), alpha(c, 0.18));
        p.text(br.center(), Align2::CENTER_CENTER, letter, FontId::monospace(11.5), c);
        let mut parts: Vec<(&str, f32, Color32)> = vec![(f.path(), 13.0, t.text)];
        let from;
        if f.change == FileChange::Renamed
            && let Some(o) = &f.old_path
        {
            from = format!("  {o}에서");
            parts.push((&from, 11.5, t.text_faint));
        }
        let g = p.layout_job(one_line_job(&parts, r.width() - 160.0));
        p.galley(pos2(r.left() + 36.0, r.center().y - g.size().y / 2.0), g, t.text);
        let stats_d = format!("−{}", f.removed());
        let stats_a = format!("+{}", f.added());
        let rr = p.text(r.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, stats_d, FontId::monospace(11.5), t.red);
        p.text(pos2(rr.left() - 8.0, r.center().y), Align2::RIGHT_CENTER, stats_a, FontId::monospace(11.5), t.green);
    }
}

fn load(root: &Path, source: &Source) -> GitResult<Loaded> {
    let top_out = git(root, Mode::Read, &["rev-parse", "--show-toplevel", "--show-prefix"]);
    let (top, prefix) = match top_out {
        Ok(s) => {
            let mut l = s.lines();
            (PathBuf::from(l.next().unwrap_or("").trim()), l.next().unwrap_or("").trim().to_string())
        }
        Err(e) => {
            if matches!(source, Source::Patch { .. }) {
                (root.to_path_buf(), String::new())
            } else {
                return Err(e);
            }
        }
    };
    match source {
        Source::File { path, staged } => {
            let rel = repo_relative(root, &top, &prefix, path);
            let f = repo::file_diff(&top, &rel, *staged)?;
            Ok(Loaded { top, rel: Some(rel), files: f.into_iter().collect(), commit: None })
        }
        Source::Commit(sha) => {
            let c = repo::commit_detail(&top, sha)?;
            let files = c.files.clone();
            Ok(Loaded { top, rel: None, files, commit: Some(c) })
        }
        Source::Patch { text, .. } => Ok(Loaded { top, rel: None, files: parse_diff(text), commit: None }),
    }
}

/// 절대/상대 경로를 저장소 최상위 기준 상대 경로('/' 구분)로 바꾼다.
fn repo_relative(root: &Path, top: &Path, prefix: &str, path: &Path) -> String {
    let to_slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if path.is_absolute() {
        if let Ok(r) = path.strip_prefix(root) {
            return format!("{prefix}{}", to_slash(r));
        }
        if let Ok(r) = path.strip_prefix(top) {
            return to_slash(r);
        }
        let croot = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if let Some(parent) = path.parent()
            && let Ok(cp) = parent.canonicalize()
        {
            let full = cp.join(path.file_name().unwrap_or_default());
            if let Ok(r) = full.strip_prefix(&croot) {
                return format!("{prefix}{}", to_slash(r));
            }
            if let Ok(r) = full.strip_prefix(top) {
                return to_slash(r);
            }
        }
        return to_slash(path);
    }
    format!("{prefix}{}", to_slash(path))
}

fn display_len(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn split_rows(lines: &[DiffLine], fi: usize, hi: usize, rows: &mut Vec<Row>) {
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => {
                rows.push(Row::Split(fi, hi, Some(i), Some(i)));
                i += 1;
            }
            LineKind::NoNewline => i += 1,
            LineKind::Remove | LineKind::Add => {
                let mut rem = Vec::new();
                let mut add = Vec::new();
                while i < lines.len() && matches!(lines[i].kind, LineKind::Remove | LineKind::NoNewline) {
                    if lines[i].kind == LineKind::Remove {
                        rem.push(i);
                    }
                    i += 1;
                }
                while i < lines.len() && matches!(lines[i].kind, LineKind::Add | LineKind::NoNewline) {
                    if lines[i].kind == LineKind::Add {
                        add.push(i);
                    }
                    i += 1;
                }
                for k in 0..rem.len().max(add.len()) {
                    rows.push(Row::Split(fi, hi, rem.get(k).copied(), add.get(k).copied()));
                }
            }
        }
    }
}

fn line_colors(kind: LineKind) -> (Color32, Color32, Color32, &'static str) {
    let t = theme();
    match kind {
        LineKind::Add => (alpha(t.green, 0.10), alpha(t.green, 0.20), alpha(t.green, 0.38), "+"),
        LineKind::Remove => (alpha(t.red, 0.10), alpha(t.red, 0.20), alpha(t.red, 0.40), "−"),
        LineKind::Context | LineKind::NoNewline => (Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT, " "),
    }
}

fn text_job(l: &DiffLine, font: &FontId, emph_bg: Color32) -> LayoutJob {
    let t = theme();
    let mut job = LayoutJob::default();
    let color = if l.kind == LineKind::NoNewline { t.text_faint } else { t.text };
    let text: &str = if l.text.len() > MAX_LINE_CHARS * 4 { &l.text[..floor_boundary(&l.text, MAX_LINE_CHARS * 4)] } else { &l.text };
    let fmt = |bg: Color32| TextFormat { font_id: font.clone(), color, background: bg, valign: Align::Center, ..Default::default() };
    match l.emph {
        Some((s, e)) if e <= text.len() && s <= e => {
            job.append(&expand_tabs(&text[..s]), 0.0, fmt(Color32::TRANSPARENT));
            job.append(&expand_tabs(&text[s..e]), 0.0, fmt(emph_bg));
            job.append(&expand_tabs(&text[e..]), 0.0, fmt(Color32::TRANSPARENT));
        }
        _ => job.append(&expand_tabs(text), 0.0, fmt(Color32::TRANSPARENT)),
    }
    job.wrap.max_rows = 1;
    job
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn expand_tabs(s: &str) -> String {
    if s.contains('\t') { s.replace('\t', "    ") } else { s.to_string() }
}

fn paint_unified_line(ui: &Ui, rect: Rect, vis: Rect, l: &DiffLine, num_w: f32, font: &FontId) {
    let t = theme();
    let (bg, gutter_bg, emph, sign) = line_colors(l.kind);
    let p = ui.painter();
    if bg != Color32::TRANSPARENT {
        p.rect_filled(vis, CornerRadius::ZERO, bg);
    }
    // 줄 번호는 가로 스크롤과 무관하게 왼쪽에 고정한다.
    let gutter = Rect::from_min_size(vis.min, vec2(num_w * 2.0, rect.height()));
    p.rect_filled(gutter, CornerRadius::ZERO, if gutter_bg == Color32::TRANSPARENT { t.bg_panel } else { gutter_bg });
    let small = FontId::monospace(11.5);
    if let Some(n) = l.old_no {
        p.text(pos2(gutter.left() + num_w - 6.0, rect.center().y), Align2::RIGHT_CENTER, n, small.clone(), t.text_faint);
    }
    if let Some(n) = l.new_no {
        p.text(pos2(gutter.left() + num_w * 2.0 - 6.0, rect.center().y), Align2::RIGHT_CENTER, n, small, t.text_faint);
    }
    let sign_c = match l.kind {
        LineKind::Add => t.green,
        LineKind::Remove => t.red,
        _ => t.text_faint,
    };
    let text_x = rect.left() + num_w * 2.0 + 20.0;
    let clip = Rect::from_min_max(pos2(gutter.right(), vis.top()), vis.max);
    let pc = p.with_clip_rect(clip.intersect(p.clip_rect()));
    pc.text(pos2(text_x - 12.0, rect.center().y), Align2::CENTER_CENTER, sign, font.clone(), sign_c);
    let g = pc.layout_job(text_job(l, font, emph));
    pc.galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g, t.text);
}

fn paint_split_side(ui: &Ui, rect: Rect, l: Option<&DiffLine>, left: bool, num_w: f32, font: &FontId) {
    let t = theme();
    let p = ui.painter();
    let Some(l) = l else {
        p.rect_filled(rect, CornerRadius::ZERO, alpha(t.bg_panel, 0.7));
        return;
    };
    let (bg, gutter_bg, emph, sign) = line_colors(l.kind);
    if bg != Color32::TRANSPARENT {
        p.rect_filled(rect, CornerRadius::ZERO, bg);
    }
    let gutter = Rect::from_min_size(rect.min, vec2(num_w, rect.height()));
    p.rect_filled(gutter, CornerRadius::ZERO, if gutter_bg == Color32::TRANSPARENT { t.bg_panel } else { gutter_bg });
    let no = if left { l.old_no } else { l.new_no };
    if let Some(n) = no {
        p.text(pos2(gutter.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, n, FontId::monospace(11.5), t.text_faint);
    }
    let sign_c = match l.kind {
        LineKind::Add => t.green,
        LineKind::Remove => t.red,
        _ => t.text_faint,
    };
    let clip = Rect::from_min_max(pos2(gutter.right(), rect.top()), rect.max);
    let pc = p.with_clip_rect(clip.intersect(p.clip_rect()));
    let text_x = gutter.right() + 18.0;
    pc.text(pos2(text_x - 10.0, rect.center().y), Align2::CENTER_CENTER, sign, font.clone(), sign_c);
    let g = pc.layout_job(text_job(l, font, emph));
    pc.galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g, t.text);
}

/// 커밋 헤더(제목, 본문, 작성자, SHA).
fn commit_header(
    ui: &mut Ui,
    c: &CommitDetail,
    now: i64,
    nfiles: usize,
    stats: impl Iterator<Item = (usize, usize)>,
) {
    let t = theme();
    let (subject, body) = c.message.split_once('\n').unwrap_or((&c.message, ""));
    ui.add(egui::Label::new(RichText::new(subject).size(16.0).strong().color(t.text)).wrap());
    let body = body.trim();
    if !body.is_empty() {
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(body).size(12.5).color(t.text_dim)).wrap().selectable(true));
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let initial = c.author.chars().next().unwrap_or('?').to_uppercase().to_string();
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 10.0, alpha(t.accent, 0.3));
        ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, FontId::proportional(11.0), t.text);
        ui.label(RichText::new(&c.author).strong().size(12.5).color(t.text));
        ui.label(dim(format!("{} 커밋함", relative_time(c.date, now))));
        if c.committer != c.author && !c.committer.is_empty() {
            ui.label(faint(format!("(커미터 {})", c.committer)));
        }
        ui.add_space(8.0);
        let short = &c.sha[..c.sha.len().min(10)];
        if ui
            .add(egui::Button::new(RichText::new(short).monospace().size(11.5).color(t.accent)).frame(false))
            .on_hover_text("전체 SHA 복사")
            .clicked()
        {
            ui.ctx().copy_text(c.sha.clone());
        }
        if !c.parents.is_empty() {
            let ps: Vec<&str> = c.parents.iter().map(|p| &p[..p.len().min(7)]).collect();
            ui.label(faint(format!("부모 {}", ps.join(" + "))));
        }
        for r in &c.refs {
            let r = r.strip_prefix("HEAD -> ").unwrap_or(r);
            outline_badge(ui, r, t.purple);
        }
        let (a, d) = stats.fold((0, 0), |(a, d), (x, y)| (a + x, d + y));
        ui.label(faint(format!("파일 {nfiles}개 변경됨")));
        ui.label(RichText::new(format!("+{a}")).color(t.green).size(11.5).monospace());
        ui.label(RichText::new(format!("−{d}")).color(t.red).size(11.5).monospace());
    });
}
