//! 프로젝트 검색 사이드 패널.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, FontId, Id, Key, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use kiln_common::{Task, Theme};

use crate::EditorEvent;
use crate::buffer::{FindOptions, build_regex, expand_replacement};
use crate::search::{FileMatches, SearchHandle, SearchMsg, SearchQuery, SearchSummary, replace_in_files, start_search};
use crate::ui_kit::{self, Icon};

const DEBOUNCE: Duration = Duration::from_millis(150);
const ROW_H: f32 = 26.0;
const DEFAULT_MAX_RESULTS: usize = 10_000;
const MSGS_PER_FRAME: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    File(usize),
    Line(usize, usize),
}

/// 프로젝트 전체 검색 패널.
pub struct SearchPanel {
    root: PathBuf,
    id: Id,
    query: String,
    replacement: String,
    include: String,
    exclude: String,
    opts: FindOptions,
    show_replace: bool,
    show_filters: bool,
    handle: Option<SearchHandle>,
    results: Vec<FileMatches>,
    collapsed: HashSet<usize>,
    summary: Option<SearchSummary>,
    error: Option<String>,
    /// 입력이 바뀐 egui 시각(초). 디바운스 기준.
    changed_at: Option<f64>,
    search_now: bool,
    last_query: Option<SearchQuery>,
    rows: Vec<Row>,
    rows_dirty: bool,
    selected: Option<Row>,
    confirm_replace: bool,
    replace_task: Option<Task<Result<(usize, usize), String>>>,
    replace_status: Option<String>,
    focus_query: bool,
    max_results: usize,
    preview_regex: Option<regex::Regex>,
}

impl SearchPanel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            id: Id::new(("kiln-search-panel", root.clone())),
            root,
            query: String::new(),
            replacement: String::new(),
            include: String::new(),
            exclude: String::new(),
            opts: FindOptions::default(),
            show_replace: false,
            show_filters: false,
            handle: None,
            results: Vec::new(),
            collapsed: HashSet::new(),
            summary: None,
            error: None,
            changed_at: None,
            search_now: false,
            last_query: None,
            rows: Vec::new(),
            rows_dirty: true,
            selected: None,
            confirm_replace: false,
            replace_task: None,
            replace_status: None,
            focus_query: false,
            max_results: DEFAULT_MAX_RESULTS,
            preview_regex: None,
        }
    }

    /// 검색 루트를 바꾸고 결과를 지운다(입력값은 유지).
    pub fn set_root(&mut self, root: PathBuf) {
        self.root = root;
        self.clear_results();
        self.search_now = true;
    }

    /// 다음 프레임에 검색어 입력칸에 포커스를 준다.
    pub fn focus(&mut self) {
        self.focus_query = true;
    }

    /// 검색어를 설정하고 곧바로 검색한다.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
        self.search_now = true;
    }

    /// 결과 상한(기본 10,000).
    pub fn set_max_results(&mut self, max: usize) {
        self.max_results = max.max(1);
    }

    /// 검색 진행 중인지.
    pub fn is_searching(&self) -> bool {
        self.handle.is_some() || self.changed_at.is_some() || self.search_now
    }

    /// 현재 결과(상대 경로 순).
    pub fn results(&self) -> &[FileMatches] {
        &self.results
    }

    /// 검색어 오류(잘못된 정규식 등).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn summary(&self) -> Option<SearchSummary> {
        self.summary
    }

    fn clear_results(&mut self) {
        self.handle = None;
        self.results.clear();
        self.collapsed.clear();
        self.summary = None;
        self.error = None;
        self.selected = None;
        self.rows_dirty = true;
    }

    fn current_query(&self) -> SearchQuery {
        SearchQuery { pattern: self.query.clone(), opts: self.opts, include: self.include.clone(), exclude: self.exclude.clone() }
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.changed_at = None;
        self.search_now = false;
        let q = self.current_query();
        self.clear_results();
        self.preview_regex = build_regex(&q.pattern, q.opts).ok();
        self.last_query = Some(q.clone());
        if q.pattern.is_empty() {
            return;
        }
        let ctx = ctx.clone();
        self.handle = Some(start_search(self.root.clone(), q, self.max_results, move || ctx.request_repaint()));
    }

    fn pump(&mut self) {
        let Some(h) = &self.handle else { return };
        let mut done = false;
        for _ in 0..MSGS_PER_FRAME {
            match h.rx.try_recv() {
                Ok(SearchMsg::File(f)) => {
                    let at = self.results.partition_point(|x| x.rel < f.rel);
                    // 삽입 위치 이후의 접힘 인덱스를 한 칸씩 민다.
                    self.collapsed = self.collapsed.iter().map(|&i| if i >= at { i + 1 } else { i }).collect();
                    self.results.insert(at, f);
                    self.rows_dirty = true;
                }
                Ok(SearchMsg::Done(s)) => {
                    self.summary = Some(s);
                    done = true;
                    break;
                }
                Ok(SearchMsg::Error(e)) => {
                    self.error = Some(e);
                    done = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        if done {
            self.handle = None;
        }
    }

    fn rebuild_rows(&mut self) {
        if !self.rows_dirty {
            return;
        }
        self.rows_dirty = false;
        self.rows.clear();
        for (fi, f) in self.results.iter().enumerate() {
            self.rows.push(Row::File(fi));
            if !self.collapsed.contains(&fi) {
                self.rows.extend((0..f.lines.len()).map(|li| Row::Line(fi, li)));
            }
        }
    }

    /// 패널을 그리고 사건 목록을 돌려준다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<EditorEvent> {
        let t = Theme::current();
        let mut events = Vec::new();
        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, t.bg_panel);

        let before = self.current_query();
        let inputs_h = self.inputs_ui(ui, full);
        let now = ui.input(|i| i.time);
        if self.current_query() != before {
            self.changed_at = Some(now);
        }
        if self.search_now {
            self.start(ui.ctx());
        } else if let Some(at) = self.changed_at {
            let el = now - at;
            if el >= DEBOUNCE.as_secs_f64() {
                self.start(ui.ctx());
            } else {
                ui.ctx().request_repaint_after(DEBOUNCE - Duration::from_secs_f64(el));
            }
        }
        self.pump();
        self.poll_replace(ui.ctx());
        self.rebuild_rows();

        let status_rect = Rect::from_min_size(pos2(full.left(), full.top() + inputs_h), vec2(full.width(), 28.0));
        self.status_ui(ui, status_rect);

        let list_rect = Rect::from_min_max(pos2(full.left(), status_rect.bottom()), full.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).id_salt(self.id.with("list")));
        child.set_clip_rect(list_rect.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing.y = 0.0;
        let n = self.rows.len();
        ScrollArea::vertical().id_salt(self.id.with("scroll")).auto_shrink([false, false]).show_rows(&mut child, ROW_H, n, |ui, range| {
            for i in range {
                if let Some(ev) = self.row_ui(ui, i) {
                    events.push(ev);
                }
            }
        });
        self.replace_modal(ui);
        events
    }

    /// 입력 영역을 그리고 높이를 돌려준다.
    fn inputs_ui(&mut self, ui: &mut Ui, full: Rect) -> f32 {
        let t = Theme::current();
        let pad = 10.0;
        let field_h = 30.0;
        let gap = 6.0;
        let mut y = full.top() + pad;
        let left = full.left() + pad;
        let right = full.right() - pad;
        let qid = self.id.with("query");
        let rid = self.id.with("replace");

        // 바꾸기 토글
        let chev = Rect::from_min_size(pos2(left, y + 5.0), vec2(18.0, field_h - 10.0));
        let resp = ui.interact(chev, self.id.with("toggle-replace"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(chev, 5.0, t.bg_hover);
        }
        ui_kit::paint_icon(ui.painter(), chev.shrink(2.0), if self.show_replace { Icon::ChevronDown } else { Icon::ChevronRight }, if resp.hovered() { t.text } else { t.text_faint });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "바꾸기 전환"));
        if resp.on_hover_text("바꾸기 전환").clicked() {
            self.show_replace = !self.show_replace;
        }
        let fx = left + 22.0;
        let q_rect = Rect::from_min_max(pos2(fx, y), pos2(right - 30.0, y + field_h));
        let submit = self.field(ui, q_rect, qid, FieldKind::Query);
        if submit {
            self.search_now = true;
        }
        let more = Rect::from_min_size(pos2(right - 25.0, y + 3.0), vec2(24.0, 24.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(more), |ui| {
            if ui_kit::icon_toggle(ui, Icon::Selection, "검색 세부 정보 전환", self.show_filters, true).clicked() {
                self.show_filters = !self.show_filters;
            }
        });
        y += field_h + gap;
        if self.show_replace {
            let r_rect = Rect::from_min_max(pos2(fx, y), pos2(right - 30.0, y + field_h));
            self.field(ui, r_rect, rid, FieldKind::Replace);
            let btn = Rect::from_min_size(pos2(right - 25.0, y + 3.0), vec2(24.0, 24.0));
            let can = !self.results.is_empty() && self.replace_task.is_none() && self.handle.is_none();
            ui.scope_builder(egui::UiBuilder::new().max_rect(btn), |ui| {
                if ui_kit::icon_toggle(ui, Icon::ReplaceAll, "모두 바꾸기", false, can).clicked() {
                    self.confirm_replace = true;
                }
            });
            y += field_h + gap;
        }
        if self.show_filters {
            for (label, kind) in [("포함할 파일", FieldKind::Include), ("제외할 파일", FieldKind::Exclude)] {
                ui.painter().text(pos2(fx + 1.0, y + 8.0), Align2::LEFT_CENTER, label, kiln_common::fonts::medium(12.0), t.text_dim);
                y += 19.0;
                let r = Rect::from_min_max(pos2(fx, y), pos2(right, y + field_h));
                let id = self.id.with(label);
                self.field(ui, r, id, kind);
                y += field_h + gap;
            }
        }
        y - full.top() + 2.0
    }

    fn field(&mut self, ui: &mut Ui, rect: Rect, id: Id, kind: FieldKind) -> bool {
        let t = Theme::current();
        let focused = ui.memory(|m| m.has_focus(id));
        let error = kind == FieldKind::Query && self.error.is_some();
        let stroke = if error { t.red } else if focused { t.accent } else { t.border_strong };
        ui.painter().rect_filled(rect, 7.0, t.bg_input);
        ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let chips_w = if kind == FieldKind::Query { 3.0 * 25.0 + 2.0 } else { 0.0 };
        let lead = if kind == FieldKind::Query || kind == FieldKind::Replace { 26.0 } else { 9.0 };
        if lead > 10.0 {
            let icon = if kind == FieldKind::Query { Icon::Search } else { Icon::ReplaceOne };
            ui_kit::paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(14.0, 14.0)), icon, t.text_faint);
        }
        let inner = Rect::from_min_max(pos2(rect.left() + lead, rect.top() + 1.0), pos2(rect.right() - 4.0 - chips_w, rect.bottom() - 1.0));
        let (text, hint) = match kind {
            FieldKind::Query => (&mut self.query, "검색"),
            FieldKind::Replace => (&mut self.replacement, "바꾸기"),
            FieldKind::Include => (&mut self.include, "예: *.rs, src/**"),
            FieldKind::Exclude => (&mut self.exclude, "예: *.lock, tests"),
        };
        let mut submit = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            let out = ui.add(ui_kit::bare_text_edit(text, id, hint, false).desired_width(inner.width()));
            if kind == FieldKind::Query && self.focus_query {
                out.request_focus();
                self.focus_query = false;
            }
            if out.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
        });
        if kind == FieldKind::Query {
            let chips = Rect::from_min_max(pos2(rect.right() - 4.0 - chips_w, rect.top() + 4.0), pos2(rect.right() - 4.0, rect.bottom() - 4.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(chips).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                let o = &mut self.opts;
                if ui_kit::option_chip(ui, "Aa", "대/소문자 구분", o.case_sensitive).clicked() {
                    o.case_sensitive = !o.case_sensitive;
                }
                if ui_kit::option_chip(ui, "ab", "단어 단위로", o.whole_word).clicked() {
                    o.whole_word = !o.whole_word;
                }
                if ui_kit::option_chip(ui, ".*", "정규식 사용", o.regex).clicked() {
                    o.regex = !o.regex;
                }
            });
        }
        submit
    }

    fn status_ui(&self, ui: &Ui, rect: Rect) {
        let t = Theme::current();
        let p = ui.painter();
        let x = rect.left() + 14.0;
        let (text, color) = if let Some(e) = &self.error {
            let msg = e.lines().last().unwrap_or(e).trim();
            (format!("잘못된 패턴: {}", msg.trim_start_matches("error: ")), t.red)
        } else if let Some(s) = &self.replace_status {
            (s.clone(), t.green)
        } else if self.handle.is_some() || self.changed_at.is_some() && !self.query.is_empty() {
            let found: usize = self.results.iter().map(|f| f.match_count()).sum();
            if found > 0 { (format!("검색 중… 지금까지 결과 {}개", fmt_count(found)), t.text_dim) } else { ("검색 중…".to_owned(), t.text_dim) }
        } else if let Some(s) = &self.summary {
            if s.matches == 0 {
                ("일치하는 내용이 없습니다 · 검색어와 파일 필터를 확인하세요".to_owned(), t.text_dim)
            } else {
                let mut msg = format!("파일 {}개에서 결과 {}개", fmt_count(s.files), fmt_count(s.matches));
                if s.truncated {
                    msg.push_str(" — 앞부분 결과만 표시");
                }
                (msg, if s.truncated { t.yellow } else { t.text_dim })
            }
        } else {
            (String::new(), t.text_dim)
        };
        p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, text, kiln_common::fonts::medium(12.0), color);
    }

    fn row_ui(&mut self, ui: &mut Ui, i: usize) -> Option<EditorEvent> {
        let t = Theme::current();
        let row = self.rows[i];
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected == Some(row);
        ui_kit::paint_row_bg(ui.painter(), rect, selected, false, resp.hovered());
        match row {
            Row::File(fi) => {
                let f = &self.results[fi];
                let collapsed = self.collapsed.contains(&fi);
                let p = ui.painter();
                ui_kit::paint_icon(p, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(12.0, 12.0)), if collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, t.text_faint);
                let name_start = f.rel.rfind('/').map_or(0, |k| k + 1);
                let name = &f.rel[name_start..];
                ui_kit::paint_file_badge(p, Rect::from_center_size(pos2(rect.left() + 36.0, rect.center().y), vec2(16.0, 16.0)), name);
                let count = f.match_count();
                let pill_text = fmt_count(count);
                let pill_w = 12.0 + pill_text.len() as f32 * 6.5;
                let pill = Rect::from_center_size(pos2(rect.right() - 14.0 - pill_w / 2.0, rect.center().y), vec2(pill_w, 17.0));
                let mut job = LayoutJob::default();
                job.append(name, 0.0, TextFormat { font_id: kiln_common::fonts::medium(13.0), color: t.text, ..Default::default() });
                let dir = f.rel[..name_start].trim_end_matches('/');
                if !dir.is_empty() {
                    job.append(&format!("  {dir}"), 0.0, TextFormat { font_id: FontId::proportional(12.0), color: t.text_faint, ..Default::default() });
                }
                job.wrap.max_width = (pill.left() - rect.left() - 54.0).max(20.0);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(rect.left() + 50.0, rect.center().y - g.size().y / 2.0), g, t.text);
                let p = ui.painter();
                p.rect_filled(pill, 8.5, t.bg_hover);
                p.text(pill.center(), Align2::CENTER_CENTER, pill_text, kiln_common::fonts::medium(11.0), t.text_dim);
                let rel = f.rel.clone();
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &rel));
                if resp.clicked() {
                    self.selected = Some(row);
                    if !self.collapsed.remove(&fi) {
                        self.collapsed.insert(fi);
                    }
                    self.rows_dirty = true;
                }
                None
            }
            Row::Line(fi, li) => {
                let f = &self.results[fi];
                let m = &f.lines[li];
                let lnum = m.line.to_string();
                let num_w = 12.0 + lnum.len() as f32 * 6.5;
                let mut job = LayoutJob::default();
                let base = TextFormat { font_id: FontId::proportional(12.5), color: t.text_dim, ..Default::default() };
                let hit = TextFormat {
                    font_id: FontId::proportional(12.5),
                    color: t.text,
                    background: kiln_common::widgets::tint(t.yellow, if t.dark { 0.24 } else { 0.18 }),
                    ..Default::default()
                };
                let replacing = self.show_replace && !self.replacement.is_empty();
                let mut pos = 0;
                for r in &m.ranges {
                    if r.start < pos || r.end > m.preview.len() {
                        continue;
                    }
                    job.append(&m.preview[pos..r.start], 0.0, base.clone());
                    let matched = &m.preview[r.clone()];
                    if replacing {
                        job.append(matched, 0.0, TextFormat { background: kiln_common::widgets::tint(t.red, 0.18), strikethrough: Stroke::new(1.0, t.red), color: t.text_dim, ..hit.clone() });
                        let rep = match &self.preview_regex {
                            Some(re) => expand_replacement(re, matched, &self.replacement, self.opts.regex),
                            None => self.replacement.clone(),
                        };
                        job.append(&rep, 0.0, TextFormat { background: kiln_common::widgets::tint(t.green, 0.18), ..hit.clone() });
                    } else {
                        job.append(matched, 0.0, hit.clone());
                    }
                    pos = r.end;
                }
                job.append(&m.preview[pos.min(m.preview.len())..], 0.0, base);
                job.wrap.max_width = (rect.width() - 50.0 - num_w).max(20.0);
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(rect.left() + 50.0, rect.center().y - g.size().y / 2.0), g, t.text);
                if resp.hovered() || selected {
                    ui.painter().text(pos2(rect.right() - 16.0, rect.center().y), Align2::RIGHT_CENTER, lnum, FontId::monospace(11.0), t.text_faint);
                }
                let label = format!("{}:{}", f.rel, m.line);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                if resp.clicked() {
                    self.selected = Some(row);
                    return Some(EditorEvent::OpenAt { path: f.path.clone(), line: m.line, col: m.col });
                }
                None
            }
        }
    }

    fn replace_modal(&mut self, ui: &mut Ui) {
        if !self.confirm_replace {
            return;
        }
        let t = Theme::current();
        let matches: usize = self.results.iter().map(|f| f.match_count()).sum();
        let files = self.results.len();
        let mut confirm = false;
        let mut cancel = false;
        let modal = egui::Modal::new(self.id.with("confirm-replace"))
            .backdrop_color(ui_kit::backdrop())
            .frame(ui_kit::modal_frame())
            .show(ui.ctx(), |ui| {
                ui.set_width((ui.ctx().content_rect().width() - 72.0).clamp(200.0, 360.0));
                ui.label(
                    egui::RichText::new(format!(
                        "파일 {}개에서 {}개 항목을 바꿀까요?",
                        fmt_count(files),
                        fmt_count(matches)
                    ))
                    .font(kiln_common::fonts::semibold(15.0))
                    .color(t.text),
                );
                ui.add_space(6.0);
                ui.add(egui::Label::new(egui::RichText::new(format!("“{}”(으)로 바꿉니다. 파일은 즉시 디스크에 기록됩니다.", self.replacement)).size(13.0).color(t.text_dim)).wrap());
                ui.add_space(18.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui_kit::flat_button(ui, "바꾸기", true).clicked() {
                        confirm = true;
                    }
                    if ui_kit::flat_button(ui, "취소", false).clicked() {
                        cancel = true;
                    }
                });
            });
        if modal.should_close() {
            cancel = true;
        }
        if confirm {
            self.confirm_replace = false;
            let files: Vec<PathBuf> = self.results.iter().map(|f| f.path.clone()).collect();
            let q = self.current_query();
            let rep = self.replacement.clone();
            self.replace_task = Some(Task::spawn(ui.ctx(), move || replace_in_files(&files, &q, &rep).map_err(|e| format!("{e:#}"))));
        } else if cancel {
            self.confirm_replace = false;
        }
    }

    fn poll_replace(&mut self, ctx: &egui::Context) {
        let Some(task) = &mut self.replace_task else { return };
        let Some(res) = task.take() else { return };
        self.replace_task = None;
        match res {
            Ok((files, n)) => {
                self.start(ctx);
                self.replace_status = Some(format!("파일 {}개에서 {}개 항목을 바꿨습니다", files, fmt_count(n)));
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// 확인 없이 모든 결과를 바로 바꾼다(동기). (바뀐 파일 수, 바꾼 개수).
    pub fn replace_all_now(&mut self, replacement: &str) -> anyhow::Result<(usize, usize)> {
        let files: Vec<PathBuf> = self.results.iter().map(|f| f.path.clone()).collect();
        let r = replace_in_files(&files, &self.current_query(), replacement)?;
        self.search_now = true;
        Ok(r)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Query,
    Replace,
    Include,
    Exclude,
}

fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
