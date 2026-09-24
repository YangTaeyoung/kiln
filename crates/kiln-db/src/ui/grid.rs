//! 가상화된 데이터 그리드. 보이는 행·열만 그리고, 헤더와 행 번호는 스크롤해도 고정된다.

use crate::export::CopyFormat;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CursorIcon, EventFilter, FontId, Id, Key, Pos2, Rect, Sense, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};
use kiln_common::Theme;

pub(crate) const ROW_H: f32 = 22.0;
pub(crate) const HEADER_H: f32 = 26.0;
const PAD_X: f32 = 6.0;
const MIN_COL_W: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CellKind {
    Value,
    Null,
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowState {
    Normal,
    Inserted,
    Deleted,
}

/// 그리드가 그릴 셀 하나.
pub(crate) struct CellView<'a> {
    pub text: &'a str,
    pub kind: CellKind,
    pub edited: bool,
    pub numeric: bool,
}

/// 헤더 한 칸.
pub(crate) struct HeaderView<'a> {
    pub name: &'a str,
    pub type_name: &'a str,
    pub pk: bool,
    pub fk: bool,
    /// `Some(true)` 내림차순, `Some(false)` 오름차순.
    pub sort: Option<bool>,
}

/// 그리드 데이터 공급자.
pub(crate) trait GridSource {
    fn n_rows(&self) -> usize;
    fn n_cols(&self) -> usize;
    fn header(&self, c: usize) -> HeaderView<'_>;
    fn cell(&self, r: usize, c: usize) -> CellView<'_>;
    fn row_state(&self, _r: usize) -> RowState {
        RowState::Normal
    }
    fn editable(&self) -> bool {
        false
    }
    /// 인라인 편집 시작 텍스트.
    fn edit_text(&self, r: usize, c: usize) -> String;
}

/// 그리드가 소유자에게 알리는 사용자 동작.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GridEvent {
    SortBy(usize),
    Commit {
        row: usize,
        col: usize,
        text: String,
    },
    SetNull,
    SetDefault,
    DeleteRows,
    DuplicateRow,
    AddRow,
    RevertSelection,
    Copy(CopyFormat, bool),
    ViewValue,
    SelectionChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellRange {
    pub r0: usize,
    pub r1: usize,
    pub c0: usize,
    pub c1: usize,
}

impl CellRange {
    fn new(a: (usize, usize), b: (usize, usize)) -> CellRange {
        CellRange {
            r0: a.0.min(b.0),
            r1: a.0.max(b.0),
            c0: a.1.min(b.1),
            c1: a.1.max(b.1),
        }
    }

    fn contains(&self, r: usize, c: usize) -> bool {
        r >= self.r0 && r <= self.r1 && c >= self.c0 && c <= self.c1
    }
}

/// 선택 상태: 여러 사각 범위 + 커서 + 기준점.
#[derive(Clone, Debug, Default)]
pub(crate) struct Selection {
    pub ranges: Vec<CellRange>,
    pub cursor: Option<(usize, usize)>,
    anchor: Option<(usize, usize)>,
}

impl Selection {
    pub fn contains(&self, r: usize, c: usize) -> bool {
        self.ranges.iter().any(|g| g.contains(r, c))
    }

    fn row_touched(&self, r: usize) -> bool {
        self.ranges.iter().any(|g| r >= g.r0 && r <= g.r1)
    }

    pub fn set_single(&mut self, cell: (usize, usize)) {
        self.ranges = vec![CellRange::new(cell, cell)];
        self.cursor = Some(cell);
        self.anchor = Some(cell);
    }

    fn extend_to(&mut self, cell: (usize, usize)) {
        let anchor = self.anchor.unwrap_or(cell);
        let r = CellRange::new(anchor, cell);
        match self.ranges.last_mut() {
            Some(last) => *last = r,
            None => self.ranges.push(r),
        }
        self.cursor = Some(cell);
    }

    fn add(&mut self, cell: (usize, usize)) {
        self.ranges.push(CellRange::new(cell, cell));
        self.cursor = Some(cell);
        self.anchor = Some(cell);
    }

    pub fn select_rows(&mut self, r0: usize, r1: usize, n_cols: usize) {
        if n_cols == 0 {
            return;
        }
        self.ranges = vec![CellRange::new((r0, 0), (r1, n_cols - 1))];
        self.cursor = Some((r1, 0));
        self.anchor = Some((r0, 0));
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
        self.cursor = None;
        self.anchor = None;
    }

    /// 선택된 셀 전체(행, 열 순으로 정렬, 중복 제거). 범위를 `n_rows`/`n_cols` 로 자른다.
    pub fn cells(&self, n_rows: usize, n_cols: usize) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for g in &self.ranges {
            for r in g.r0..=g.r1.min(n_rows.saturating_sub(1)) {
                for c in g.c0..=g.c1.min(n_cols.saturating_sub(1)) {
                    if r < n_rows && c < n_cols {
                        v.push((r, c));
                    }
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    /// 선택이 걸친 행 번호(오름차순).
    pub fn rows(&self, n_rows: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .ranges
            .iter()
            .flat_map(|g| g.r0..=g.r1)
            .filter(|r| *r < n_rows)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// 선택이 걸친 열 번호(오름차순).
    pub fn cols(&self, n_cols: usize) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .ranges
            .iter()
            .flat_map(|g| g.c0..=g.c1)
            .filter(|c| *c < n_cols)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// 인라인 편집 상태.
#[derive(Clone, Debug)]
pub(crate) struct EditCell {
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub focus_requested: bool,
}

/// 그리드 UI 상태.
pub(crate) struct GridState {
    pub id: Id,
    pub col_widths: Vec<f32>,
    pub sel: Selection,
    pub editing: Option<EditCell>,
    scroll_to_cursor: bool,
    drag_select: bool,
    resize_origin: Option<(usize, f32)>,
}

impl GridState {
    pub fn new(id: Id) -> GridState {
        GridState {
            id,
            col_widths: Vec::new(),
            sel: Selection::default(),
            editing: None,
            scroll_to_cursor: false,
            drag_select: false,
            resize_origin: None,
        }
    }

    /// 헤더와 앞쪽 행 표본으로 열 너비를 정한다.
    pub fn auto_size(&mut self, src: &dyn GridSource) {
        let n = src.n_cols();
        let sample = src.n_rows().min(200);
        self.col_widths = (0..n)
            .map(|c| {
                let h = src.header(c);
                let mut chars = h.name.chars().count() as f32 * 7.2
                    + h.type_name.chars().count().min(14) as f32 * 6.0
                    + 30.0;
                if h.pk || h.fk {
                    chars += 16.0;
                }
                let mut w = chars;
                for r in 0..sample {
                    let t = src.cell(r, c).text;
                    let cw = t.chars().count().min(60) as f32 * 7.3 + 2.0 * PAD_X + 4.0;
                    w = w.max(cw);
                }
                w.clamp(60.0, 360.0)
            })
            .collect();
    }

    pub fn begin_edit(&mut self, src: &dyn GridSource, r: usize, c: usize) {
        if !src.editable() || r >= src.n_rows() || c >= src.n_cols() {
            return;
        }
        if src.row_state(r) == RowState::Deleted {
            return;
        }
        self.sel.set_single((r, c));
        self.editing = Some(EditCell {
            row: r,
            col: c,
            text: src.edit_text(r, c),
            focus_requested: false,
        });
    }

    pub fn scroll_to_cursor(&mut self) {
        self.scroll_to_cursor = true;
    }
}

fn gutter_width(n_rows: usize) -> f32 {
    let digits = (n_rows.max(1) as f64).log10().floor() as usize + 1;
    (digits.max(2) as f32) * 7.5 + 14.0
}

fn cell_job(text: &str, width: f32, font: FontId, color: Color32, italics: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            italics,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

/// 그리드를 그리고 사용자 동작을 돌려준다.
pub(crate) fn grid_ui(ui: &mut Ui, st: &mut GridState, src: &dyn GridSource) -> Vec<GridEvent> {
    let theme = Theme::current();
    let mut events = Vec::new();
    let n_rows = src.n_rows();
    let n_cols = src.n_cols();
    if st.col_widths.len() != n_cols {
        st.auto_size(src);
    }
    let gutter = gutter_width(n_rows);
    let mut xs = Vec::with_capacity(n_cols + 1);
    let mut acc = gutter;
    for w in &st.col_widths {
        xs.push(acc);
        acc += *w;
    }
    xs.push(acc);
    let content_w = acc + 40.0;
    let widths = st.col_widths.clone();
    let content_h = HEADER_H + n_rows as f32 * ROW_H + 4.0;
    let mono = FontId::monospace(12.0);
    let head_font = FontId::proportional(12.5);
    let small = FontId::proportional(10.5);
    let editable = src.editable();

    // 키보드 처리(포커스가 그리드에 있을 때).
    let body_id = st.id.with("body");
    let has_focus = ui.memory(|m| m.has_focus(body_id));
    if has_focus && st.editing.is_none() {
        handle_keys(ui, st, src, &mut events);
    }

    let out = egui::ScrollArea::both()
        .id_salt(st.id.with("scroll"))
        .auto_shrink([false, false])
        .content_margin(0.0)
        .show_viewport(ui, |ui, vp| {
            let (rect, _) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::hover());
            let resp = ui.interact(rect, body_id, Sense::click_and_drag());
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "data grid")
            });
            let origin = rect.min;
            let painter = ui.painter_at(ui.clip_rect());
            let screen_vp = vp.translate(origin.to_vec2());

            let first = ((vp.min.y - HEADER_H) / ROW_H).floor().max(0.0) as usize;
            let last = (((vp.max.y - HEADER_H) / ROW_H).ceil().max(0.0) as usize + 1).min(n_rows);
            let c_first = xs
                .partition_point(|x| *x <= vp.min.x)
                .saturating_sub(1)
                .min(n_cols);
            let c_last = xs.partition_point(|x| *x < vp.max.x).min(n_cols);

            // 본문 배경.
            painter.rect_filled(screen_vp, 0.0, theme.bg);

            let cell_rect = |r: usize, c: usize| {
                Rect::from_min_size(
                    pos2(origin.x + xs[c], origin.y + HEADER_H + r as f32 * ROW_H),
                    vec2(widths[c], ROW_H),
                )
            };

            for r in first..last {
                let y = origin.y + HEADER_H + r as f32 * ROW_H;
                let row_rect =
                    Rect::from_min_size(pos2(screen_vp.min.x, y), vec2(screen_vp.width(), ROW_H));
                let rs = src.row_state(r);
                let base = if r % 2 == 1 { zebra(theme) } else { theme.bg };
                painter.rect_filled(row_rect, 0.0, base);
                match rs {
                    RowState::Inserted => {
                        painter.rect_filled(row_rect, 0.0, theme.green.gamma_multiply(0.16));
                    }
                    RowState::Deleted => {
                        painter.rect_filled(row_rect, 0.0, theme.red.gamma_multiply(0.16));
                    }
                    RowState::Normal => {}
                }
                for c in c_first..c_last {
                    let cr = cell_rect(r, c);
                    let cell = src.cell(r, c);
                    if cell.edited && rs != RowState::Deleted {
                        painter.rect_filled(cr, 0.0, theme.yellow.gamma_multiply(0.22));
                    }
                    if st.sel.contains(r, c) {
                        painter.rect_filled(
                            cr,
                            0.0,
                            theme
                                .bg_selected
                                .gamma_multiply(if has_focus { 1.0 } else { 0.7 }),
                        );
                    }
                    let (color, italics) = match cell.kind {
                        CellKind::Value => (
                            if rs == RowState::Deleted {
                                theme.text_dim
                            } else {
                                theme.text
                            },
                            false,
                        ),
                        CellKind::Null | CellKind::Default => (theme.text_faint, true),
                    };
                    let inner_w = cr.width() - 2.0 * PAD_X;
                    let galley = painter.layout_job(cell_job(
                        cell.text,
                        inner_w,
                        mono.clone(),
                        color,
                        italics,
                    ));
                    let x = if cell.numeric && cell.kind == CellKind::Value {
                        cr.max.x - PAD_X - galley.size().x
                    } else {
                        cr.min.x + PAD_X
                    };
                    let gy = cr.center().y - galley.size().y / 2.0;
                    let text_rect = Rect::from_min_size(pos2(x, gy), galley.size());
                    painter.galley(pos2(x, gy), galley, color);
                    if rs == RowState::Deleted {
                        painter.hline(
                            text_rect.x_range(),
                            cr.center().y,
                            Stroke::new(1.0, theme.red.gamma_multiply(0.8)),
                        );
                    }
                }
                painter.hline(
                    screen_vp.x_range(),
                    y + ROW_H - 0.5,
                    Stroke::new(1.0, theme.border.gamma_multiply(0.45)),
                );
            }
            // 열 구분선.
            for x in &xs[c_first..=c_last.min(n_cols)] {
                let x = origin.x + x - 0.5;
                painter.vline(
                    x,
                    screen_vp.y_range(),
                    Stroke::new(1.0, theme.border.gamma_multiply(0.45)),
                );
            }
            // 커서 셀 테두리.
            if let Some((r, c)) = st.sel.cursor
                && r < n_rows
                && c < n_cols
                && r >= first
                && r < last
            {
                painter.rect_stroke(
                    cell_rect(r, c).shrink(0.5),
                    0.0,
                    Stroke::new(
                        1.5,
                        if has_focus {
                            theme.accent
                        } else {
                            theme.text_faint
                        },
                    ),
                    StrokeKind::Inside,
                );
            }

            // 셀 좌표 계산.
            let hit = |p: Pos2| -> Option<(usize, usize)> {
                let lx = p.x - origin.x;
                let ly = p.y - origin.y - HEADER_H;
                if ly < 0.0 || lx < gutter {
                    return None;
                }
                let r = (ly / ROW_H) as usize;
                let c = xs.partition_point(|x| *x <= lx).checked_sub(1)?;
                (r < n_rows && c < n_cols).then_some((r, c))
            };

            // 본문 마우스.
            let (pressed, modifiers, ptr) = ui.input(|i| {
                (
                    i.pointer.primary_pressed(),
                    i.modifiers,
                    i.pointer.interact_pos(),
                )
            });
            let header_band = Rect::from_min_size(screen_vp.min, vec2(screen_vp.width(), HEADER_H));
            let gutter_band = Rect::from_min_size(screen_vp.min, vec2(gutter, screen_vp.height()));
            let in_body = |p: Pos2| {
                !header_band.contains(p) && !gutter_band.contains(p) && screen_vp.contains(p)
            };
            if pressed
                && resp.hovered()
                && let Some(p) = ptr
                && in_body(p)
                && let Some(cell) = hit(p)
            {
                resp.request_focus();
                if modifiers.command {
                    st.sel.add(cell);
                } else if modifiers.shift {
                    st.sel.extend_to(cell);
                } else {
                    st.sel.set_single(cell);
                }
                st.drag_select = true;
                events.push(GridEvent::SelectionChanged);
            }
            if st.drag_select {
                if resp.dragged() {
                    if let Some(p) = ptr
                        && let Some(cell) = hit(p.clamp(
                            screen_vp.min + vec2(gutter + 1.0, HEADER_H + 1.0),
                            screen_vp.max - vec2(1.0, 1.0),
                        ))
                        && st.sel.cursor != Some(cell)
                    {
                        st.sel.extend_to(cell);
                        events.push(GridEvent::SelectionChanged);
                    }
                    // 가장자리로 끌면 자동 스크롤.
                    if let Some(p) = ptr {
                        let mut d = Vec2::ZERO;
                        if p.y > screen_vp.max.y - 8.0 {
                            d.y = -ROW_H;
                        } else if p.y < screen_vp.min.y + HEADER_H + 4.0 {
                            d.y = ROW_H;
                        }
                        if d != Vec2::ZERO {
                            ui.scroll_with_delta(d);
                        }
                    }
                } else if !ui.input(|i| i.pointer.primary_down()) {
                    st.drag_select = false;
                }
            }
            if resp.double_clicked()
                && let Some(p) = ptr
                && in_body(p)
                && let Some((r, c)) = hit(p)
            {
                if editable {
                    st.begin_edit(src, r, c);
                } else {
                    events.push(GridEvent::ViewValue);
                }
            }
            if resp.secondary_clicked()
                && let Some(p) = ptr
                && let Some(cell) = hit(p)
                && !st.sel.contains(cell.0, cell.1)
            {
                st.sel.set_single(cell);
                events.push(GridEvent::SelectionChanged);
            }
            if has_focus || resp.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        body_id,
                        EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    )
                });
            }
            resp.context_menu(|ui| context_menu(ui, st, src, &mut events));

            // 인라인 편집기.
            if let Some(ed) = &mut st.editing {
                if ed.row < n_rows && ed.col < n_cols {
                    let cr = cell_rect(ed.row, ed.col);
                    painter.rect_filled(cr, 0.0, theme.bg_elevated);
                    let te_id = st.id.with("editor");
                    let te = egui::TextEdit::singleline(&mut ed.text)
                        .id(te_id)
                        .font(mono.clone())
                        .frame(egui::Frame::NONE)
                        .margin(vec2(PAD_X - 1.0, 3.0))
                        .desired_width(cr.width());
                    let r = ui.put(cr, te);
                    painter.rect_stroke(
                        cr,
                        0.0,
                        Stroke::new(1.5, theme.accent),
                        StrokeKind::Inside,
                    );
                    if !ed.focus_requested {
                        r.request_focus();
                        ed.focus_requested = true;
                    }
                    let (enter, esc, tab) = ui.input(|i| {
                        (
                            i.key_pressed(Key::Enter),
                            i.key_pressed(Key::Escape),
                            i.key_pressed(Key::Tab),
                        )
                    });
                    if esc {
                        st.editing = None;
                        ui.memory_mut(|m| m.request_focus(body_id));
                    } else if r.lost_focus() || enter || tab {
                        let ed = st.editing.take().expect("editing");
                        events.push(GridEvent::Commit {
                            row: ed.row,
                            col: ed.col,
                            text: ed.text,
                        });
                        if enter || tab {
                            ui.memory_mut(|m| m.request_focus(body_id));
                            if tab && ed.col + 1 < n_cols {
                                st.sel.set_single((ed.row, ed.col + 1));
                            } else if enter && ed.row + 1 < n_rows {
                                st.sel.set_single((ed.row + 1, ed.col));
                            }
                            st.scroll_to_cursor = true;
                        }
                    }
                } else {
                    st.editing = None;
                }
            }

            // 고정 행 번호.
            let gx = screen_vp.min.x;
            let g_rect =
                Rect::from_min_size(pos2(gx, screen_vp.min.y), vec2(gutter, screen_vp.height()));
            painter.rect_filled(g_rect, 0.0, theme.bg_panel);
            for r in first..last {
                let y = origin.y + HEADER_H + r as f32 * ROW_H;
                let rr = Rect::from_min_size(pos2(gx, y), vec2(gutter, ROW_H));
                let rs = src.row_state(r);
                let touched = st.sel.row_touched(r);
                if touched {
                    painter.rect_filled(rr, 0.0, theme.bg_hover);
                }
                let (label, color) = match rs {
                    RowState::Inserted => ("+".to_string(), theme.green),
                    RowState::Deleted => ((r + 1).to_string(), theme.red),
                    RowState::Normal => (
                        (r + 1).to_string(),
                        if touched {
                            theme.text_dim
                        } else {
                            theme.text_faint
                        },
                    ),
                };
                painter.text(
                    pos2(rr.max.x - 6.0, rr.center().y),
                    Align2::RIGHT_CENTER,
                    label,
                    FontId::monospace(10.5),
                    color,
                );
            }
            painter.vline(
                g_rect.max.x - 0.5,
                g_rect.y_range(),
                Stroke::new(1.0, theme.border),
            );
            let g_resp = ui.interact(g_rect, st.id.with("gutter"), Sense::click_and_drag());
            if (g_resp.clicked() || g_resp.drag_started() || g_resp.dragged())
                && let Some(p) = ptr
            {
                let r = ((p.y - origin.y - HEADER_H) / ROW_H).floor();
                if r >= 0.0 && (r as usize) < n_rows {
                    let r = r as usize;
                    let anchor = if (modifiers.shift || g_resp.dragged()) && !g_resp.drag_started()
                    {
                        st.sel.anchor.map(|a| a.0).unwrap_or(r)
                    } else {
                        r
                    };
                    st.sel.select_rows(anchor, r, n_cols);
                    ui.memory_mut(|m| m.request_focus(body_id));
                    events.push(GridEvent::SelectionChanged);
                }
            }
            g_resp.context_menu(|ui| context_menu(ui, st, src, &mut events));

            // 고정 헤더.
            let hy = screen_vp.min.y;
            let h_rect =
                Rect::from_min_size(pos2(screen_vp.min.x, hy), vec2(screen_vp.width(), HEADER_H));
            painter.rect_filled(h_rect, 0.0, theme.bg_panel);
            for c in c_first..c_last {
                let x0 = origin.x + xs[c];
                let hr = Rect::from_min_size(pos2(x0, hy), vec2(widths[c], HEADER_H));
                if hr.max.x <= gx + gutter {
                    continue;
                }
                let h = src.header(c);
                let hp = painter.with_clip_rect(
                    hr.intersect(Rect::from_min_max(pos2(gx + gutter, hy), h_rect.max)),
                );
                let hresp = ui.interact(hr, st.id.with(("hdr", c)), Sense::click());
                hresp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, h.name)
                });
                if hresp.hovered() {
                    hp.rect_filled(hr, 0.0, theme.bg_hover);
                }
                let mut x = hr.min.x + PAD_X;
                if h.pk || h.fk {
                    let (glyph, col) = if h.pk {
                        ("🔑", theme.yellow)
                    } else {
                        ("🔗", theme.blue)
                    };
                    let g = hp.text(
                        pos2(x, hr.center().y),
                        Align2::LEFT_CENTER,
                        glyph,
                        FontId::proportional(12.0),
                        col,
                    );
                    x = g.max.x + 4.0;
                }
                let right_reserve = if h.sort.is_some() { 16.0 } else { 4.0 };
                let name_g = hp.layout_job(cell_job(
                    h.name,
                    (hr.max.x - x - right_reserve).max(8.0),
                    head_font.clone(),
                    theme.text,
                    false,
                ));
                let name_w = name_g.size().x;
                hp.galley(
                    pos2(x, hr.center().y - name_g.size().y / 2.0),
                    name_g,
                    theme.text,
                );
                let tx = x + name_w + 6.0;
                let avail = hr.max.x - tx - right_reserve;
                if avail > 18.0
                    && !h.type_name.is_empty()
                    && !h.type_name.eq_ignore_ascii_case("null")
                {
                    let tg = hp.layout_job(cell_job(
                        &h.type_name.to_ascii_lowercase(),
                        avail,
                        small.clone(),
                        theme.text_faint,
                        false,
                    ));
                    hp.galley(
                        pos2(tx, hr.center().y - tg.size().y / 2.0 + 0.5),
                        tg,
                        theme.text_faint,
                    );
                }
                if let Some(desc) = h.sort {
                    hp.text(
                        pos2(hr.max.x - 6.0, hr.center().y),
                        Align2::RIGHT_CENTER,
                        if desc { "▼" } else { "▲" },
                        FontId::monospace(9.0),
                        theme.accent,
                    );
                }
                hp.vline(hr.max.x - 0.5, hr.y_range(), Stroke::new(1.0, theme.border));
                if hresp.clicked() {
                    events.push(GridEvent::SortBy(c));
                }
                let tip = format!(
                    "{}{}{}",
                    h.name,
                    if h.type_name.is_empty() {
                        String::new()
                    } else {
                        format!(" : {}", h.type_name)
                    },
                    if h.pk {
                        "  (primary key)"
                    } else if h.fk {
                        "  (foreign key)"
                    } else {
                        ""
                    }
                );
                hresp.on_hover_text(tip);
                // 열 너비 조절 핸들.
                let handle =
                    Rect::from_center_size(pos2(hr.max.x, hr.center().y), vec2(8.0, HEADER_H));
                let rresp = ui.interact(handle, st.id.with(("resize", c)), Sense::drag());
                if rresp.hovered() || rresp.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
                if rresp.drag_started() {
                    st.resize_origin = Some((c, st.col_widths[c]));
                }
                if rresp.dragged()
                    && let Some((rc, _)) = st.resize_origin
                    && rc == c
                {
                    st.col_widths[c] = (st.col_widths[c] + rresp.drag_delta().x).max(MIN_COL_W);
                }
                if rresp.double_clicked() {
                    let mut w: f32 = h.name.chars().count() as f32 * 7.5 + 30.0;
                    for r in first..last {
                        w = w.max(
                            src.cell(r, c).text.chars().count() as f32 * 7.3 + 2.0 * PAD_X + 4.0,
                        );
                    }
                    st.col_widths[c] = w.clamp(MIN_COL_W, 800.0);
                }
                if rresp.drag_stopped() {
                    st.resize_origin = None;
                }
            }
            // 헤더 왼쪽 모서리.
            let corner = Rect::from_min_size(pos2(gx, hy), vec2(gutter, HEADER_H));
            painter.rect_filled(corner, 0.0, theme.bg_panel);
            painter.vline(
                corner.max.x - 0.5,
                corner.y_range(),
                Stroke::new(1.0, theme.border),
            );
            painter.hline(
                h_rect.x_range(),
                h_rect.max.y - 0.5,
                Stroke::new(1.0, theme.border),
            );
            let corner_resp = ui.interact(corner, st.id.with("corner"), Sense::click());
            if corner_resp.clicked() && n_rows > 0 {
                st.sel.select_rows(0, n_rows - 1, n_cols);
                ui.memory_mut(|m| m.request_focus(body_id));
                events.push(GridEvent::SelectionChanged);
            }

            // 커서 따라 스크롤.
            if st.scroll_to_cursor {
                st.scroll_to_cursor = false;
                if let Some((r, c)) = st.sel.cursor
                    && r < n_rows
                    && c < n_cols
                {
                    let cr = cell_rect(r, c);
                    let mut target = cr;
                    target.min.y -= HEADER_H;
                    target.min.x -= gutter;
                    ui.scroll_to_rect(target, None);
                }
            }
        });
    let _ = out;
    events
}

fn zebra(theme: Theme) -> Color32 {
    let b = theme.bg;
    Color32::from_rgb(
        b.r().saturating_add(5),
        b.g().saturating_add(5),
        b.b().saturating_add(7),
    )
}

fn handle_keys(ui: &mut Ui, st: &mut GridState, src: &dyn GridSource, events: &mut Vec<GridEvent>) {
    let n_rows = src.n_rows();
    let n_cols = src.n_cols();
    if n_rows == 0 || n_cols == 0 {
        return;
    }
    let (r, c) = st.sel.cursor.unwrap_or((0, 0));
    let page = 20usize;
    let mut moved: Option<(usize, usize)> = None;
    let mut extend = false;
    ui.input_mut(|i| {
        let m = i.modifiers;
        extend = m.shift;
        let mv = |i: &mut egui::InputState, k: Key| i.consume_key(m, k);
        if mv(i, Key::ArrowDown) {
            moved = Some(if m.command {
                (n_rows - 1, c)
            } else {
                ((r + 1).min(n_rows - 1), c)
            });
        } else if mv(i, Key::ArrowUp) {
            moved = Some(if m.command {
                (0, c)
            } else {
                (r.saturating_sub(1), c)
            });
        } else if mv(i, Key::ArrowRight) {
            moved = Some(if m.command {
                (r, n_cols - 1)
            } else {
                (r, (c + 1).min(n_cols - 1))
            });
        } else if mv(i, Key::ArrowLeft) {
            moved = Some(if m.command {
                (r, 0)
            } else {
                (r, c.saturating_sub(1))
            });
        } else if mv(i, Key::PageDown) {
            moved = Some(((r + page).min(n_rows - 1), c));
        } else if mv(i, Key::PageUp) {
            moved = Some((r.saturating_sub(page), c));
        } else if mv(i, Key::Home) {
            moved = Some(if m.command { (0, 0) } else { (r, 0) });
        } else if mv(i, Key::End) {
            moved = Some(if m.command {
                (n_rows - 1, n_cols - 1)
            } else {
                (r, n_cols - 1)
            });
        } else if i.consume_key(egui::Modifiers::NONE, Key::Tab) {
            moved = Some((r, (c + 1).min(n_cols - 1)));
            extend = false;
        }
    });
    if let Some(cell) = moved {
        if extend {
            st.sel.extend_to(cell);
        } else {
            st.sel.set_single(cell);
        }
        st.scroll_to_cursor = true;
        events.push(GridEvent::SelectionChanged);
        return;
    }
    let (enter, f2, copy, select_all, del_rows, null_key, dup) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::Enter),
            i.consume_key(egui::Modifiers::NONE, Key::F2),
            i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            i.consume_key(egui::Modifiers::COMMAND, Key::A),
            i.consume_key(egui::Modifiers::COMMAND, Key::Backspace)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Delete),
            i.consume_key(egui::Modifiers::NONE, Key::Delete)
                || i.consume_key(egui::Modifiers::NONE, Key::Backspace),
            i.consume_key(egui::Modifiers::COMMAND, Key::D),
        )
    });
    if (enter || f2) && st.sel.cursor.is_some() {
        if src.editable() {
            st.begin_edit(src, r, c);
        } else {
            events.push(GridEvent::ViewValue);
        }
    }
    if copy {
        events.push(GridEvent::Copy(CopyFormat::Tsv, false));
    }
    if select_all {
        st.sel.select_rows(0, n_rows - 1, n_cols);
        events.push(GridEvent::SelectionChanged);
    }
    if src.editable() {
        if del_rows {
            events.push(GridEvent::DeleteRows);
        }
        if null_key {
            events.push(GridEvent::SetNull);
        }
        if dup {
            events.push(GridEvent::DuplicateRow);
        }
    }
}

fn context_menu(
    ui: &mut Ui,
    st: &mut GridState,
    src: &dyn GridSource,
    events: &mut Vec<GridEvent>,
) {
    ui.set_min_width(190.0);
    let editable = src.editable();
    let has_sel = !st.sel.ranges.is_empty();
    if editable {
        if ui
            .add_enabled(
                st.sel.cursor.is_some(),
                egui::Button::new("Edit cell").shortcut_text("Enter"),
            )
            .clicked()
            && let Some((r, c)) = st.sel.cursor
        {
            st.begin_edit(src, r, c);
            ui.close();
        }
        if ui
            .add_enabled(has_sel, egui::Button::new("Set NULL").shortcut_text("Del"))
            .clicked()
        {
            events.push(GridEvent::SetNull);
            ui.close();
        }
        if ui
            .add_enabled(has_sel, egui::Button::new("Set DEFAULT"))
            .clicked()
        {
            events.push(GridEvent::SetDefault);
            ui.close();
        }
        ui.separator();
        if ui.button("Add row").clicked() {
            events.push(GridEvent::AddRow);
            ui.close();
        }
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new("Duplicate row").shortcut_text("⌘D"),
            )
            .clicked()
        {
            events.push(GridEvent::DuplicateRow);
            ui.close();
        }
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new("Delete rows").shortcut_text("⌘⌫"),
            )
            .clicked()
        {
            events.push(GridEvent::DeleteRows);
            ui.close();
        }
        if ui
            .add_enabled(has_sel, egui::Button::new("Revert selected"))
            .clicked()
        {
            events.push(GridEvent::RevertSelection);
            ui.close();
        }
        ui.separator();
    }
    ui.menu_button("Copy as", |ui| {
        for f in CopyFormat::ALL {
            if ui.button(f.label()).clicked() {
                events.push(GridEvent::Copy(f, false));
                ui.close();
            }
        }
        ui.separator();
        for f in [CopyFormat::Tsv, CopyFormat::Csv] {
            if ui.button(format!("{} with header", f.label())).clicked() {
                events.push(GridEvent::Copy(f, true));
                ui.close();
            }
        }
    });
    if ui
        .add_enabled(has_sel, egui::Button::new("Copy").shortcut_text("⌘C"))
        .clicked()
    {
        events.push(GridEvent::Copy(CopyFormat::Tsv, false));
        ui.close();
    }
    if ui
        .add_enabled(st.sel.cursor.is_some(), egui::Button::new("View value"))
        .clicked()
    {
        events.push(GridEvent::ViewValue);
        ui.close();
    }
}
