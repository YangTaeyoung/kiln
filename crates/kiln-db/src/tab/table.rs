//! 테이블 데이터 탭: 페이지 그리드, 정렬/필터, 인라인 편집과 트랜잭션 제출, 구조·DDL 보기.

use super::viewer::{ValueViewer, ViewerCell};
use crate::driver::{ChangeError, DbResult};
use crate::edit::{ChangeSet, RowInsert, RowUpdate, TableRef, order_for, select_sql};
use crate::export::{CopyFormat, ExportFormat, format_rows};
use crate::manager::Job;
use crate::meta::TableDetails;
use crate::ui::grid::{
    CellKind, CellView, GridEvent, GridSource, GridState, HeaderView, RowState, grid_ui,
};
use crate::ui::highlight::SqlHighlighter;
use crate::ui::{self, Glyph, dim, faint, glyph_button, thousands, tool_button_icon};
use crate::value::DISPLAY_MAX_CHARS;
use crate::{ColumnInfo, ConnId, DbManager, Driver, ResultSet, TypeClass, Value};
use egui::{Key, Modifiers, RichText, Ui};
use kiln_common::icons::Icon;
use kiln_common::widgets;
use kiln_common::{Theme, fonts};
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubTab {
    Data,
    Structure,
    Ddl,
}

const PAGE_SIZES: [usize; 6] = [100, 500, 1000, 5000, 10_000, 50_000];
const DEFAULT_TEXT: &str = "<default>";
const NULL_TEXT: &str = "<null>";

/// 새 행의 셀. `None` 은 DEFAULT.
type NewCell = Option<(Value, Box<str>)>;

/// 로드한 페이지와 제출 전 변경.
pub(crate) struct TableData {
    pub rs: ResultSet,
    pub edits: HashMap<(usize, usize), (Value, Box<str>)>,
    pub deleted: BTreeSet<usize>,
    pub inserted: Vec<Vec<NewCell>>,
    pub editable: bool,
    pub pk: Vec<usize>,
    pub fk: Vec<bool>,
    pub sort: Option<(usize, bool)>,
    pub nullable: Vec<bool>,
    pub has_default: Vec<bool>,
}

impl TableData {
    fn new(rs: ResultSet) -> TableData {
        let n = rs.columns.len();
        TableData {
            rs,
            edits: HashMap::new(),
            deleted: BTreeSet::new(),
            inserted: Vec::new(),
            editable: false,
            pk: Vec::new(),
            fk: vec![false; n],
            sort: None,
            nullable: vec![true; n],
            has_default: vec![false; n],
        }
    }

    /// 상세 정보로 헤더 타입, PK/FK, 편집 가능 여부를 채운다.
    fn apply_details(&mut self, det: &TableDetails, writable_kind: bool) {
        let names: Vec<String> = self.rs.columns.iter().map(|c| c.name.clone()).collect();
        self.pk.clear();
        for (ci, name) in names.iter().enumerate() {
            if let Some(cd) = det.columns.iter().find(|c| &c.name == name) {
                self.rs.columns[ci].type_name = cd.data_type.clone();
                if self.rs.columns[ci].class != cd.class && cd.class != TypeClass::Other {
                    self.rs.columns[ci].class = cd.class;
                }
                self.fk[ci] = det.fk_target(name).is_some();
                self.nullable[ci] = cd.nullable;
                self.has_default[ci] = cd.has_default();
            }
        }
        for pi in det.pk_columns() {
            let pname = &det.columns[pi].name;
            if let Some(ci) = names.iter().position(|n| n == pname) {
                self.pk.push(ci);
            }
        }
        self.editable =
            writable_kind && !self.pk.is_empty() && self.pk.len() == det.pk_columns().len();
    }

    fn base(&self) -> usize {
        self.rs.rows.len()
    }

    fn n_rows(&self) -> usize {
        self.base() + self.inserted.len()
    }

    /// 현재 값. `None` 은 새 행의 DEFAULT.
    pub fn value_at(&self, r: usize, c: usize) -> Option<&Value> {
        if r < self.base() {
            Some(
                self.edits
                    .get(&(r, c))
                    .map(|(v, _)| v)
                    .unwrap_or(&self.rs.rows[r][c]),
            )
        } else {
            self.inserted
                .get(r - self.base())
                .and_then(|row| row[c].as_ref().map(|(v, _)| v))
        }
    }

    pub fn pending(&self) -> usize {
        let edited_rows: BTreeSet<usize> = self
            .edits
            .keys()
            .map(|(r, _)| *r)
            .filter(|r| !self.deleted.contains(r))
            .collect();
        edited_rows.len() + self.deleted.len() + self.inserted.len()
    }

    fn set_value(&mut self, r: usize, c: usize, v: Value) {
        let d = v.display(DISPLAY_MAX_CHARS).into_boxed_str();
        let base = self.base();
        if r < base {
            if self.rs.rows[r][c] == v {
                self.edits.remove(&(r, c));
            } else {
                self.edits.insert((r, c), (v, d));
            }
        } else if let Some(row) = self.inserted.get_mut(r - base) {
            row[c] = Some((v, d));
        }
    }

    fn commit_text(&mut self, r: usize, c: usize, text: &str) -> Result<(), String> {
        let class = self.rs.columns[c].class;
        let cur = self.value_at(r, c).cloned();
        let cur_text = cur.as_ref().and_then(|v| v.to_text()).unwrap_or_default();
        if text == cur_text && cur.is_some() {
            return Ok(());
        }
        let v = Value::parse_input(text, class)?;
        self.set_value(r, c, v);
        Ok(())
    }

    fn add_row(&mut self) -> usize {
        self.inserted.push(vec![None; self.rs.columns.len()]);
        self.n_rows() - 1
    }

    fn duplicate(&mut self, rows: &[usize]) {
        for &r in rows {
            let n = self.rs.columns.len();
            let row: Vec<NewCell> = (0..n)
                .map(|c| {
                    if self.pk.contains(&c) && self.has_default[c] {
                        return None;
                    }
                    self.value_at(r, c)
                        .map(|v| (v.clone(), v.display(DISPLAY_MAX_CHARS).into_boxed_str()))
                })
                .collect();
            self.inserted.push(row);
        }
    }

    fn delete_rows(&mut self, rows: &[usize]) {
        let base = self.base();
        for &r in rows.iter().rev() {
            if r >= base {
                if r - base < self.inserted.len() {
                    self.inserted.remove(r - base);
                }
            } else if !self.deleted.insert(r) {
                self.deleted.remove(&r);
            }
        }
    }

    fn revert_cells(&mut self, cells: &[(usize, usize)], rows: &[usize]) {
        for cell in cells {
            self.edits.remove(cell);
        }
        let base = self.base();
        for &r in rows.iter().rev() {
            if r < base {
                self.deleted.remove(&r);
            } else if r - base < self.inserted.len() {
                self.inserted.remove(r - base);
            }
        }
    }

    fn revert_all(&mut self) {
        self.edits.clear();
        self.deleted.clear();
        self.inserted.clear();
    }

    fn key_of(&self, r: usize) -> Vec<(usize, Value)> {
        self.pk
            .iter()
            .map(|&c| (c, self.rs.rows[r][c].clone()))
            .collect()
    }

    /// 변경 묶음. 컬럼 인덱스는 결과 집합 순서이며 `columns` 로 상세 컬럼에 대응시킨다.
    fn change_set(&self) -> ChangeSet {
        let mut by_row: HashMap<usize, Vec<(usize, Value)>> = HashMap::new();
        for ((r, c), (v, _)) in &self.edits {
            if !self.deleted.contains(r) {
                by_row.entry(*r).or_default().push((*c, v.clone()));
            }
        }
        let mut rows: Vec<usize> = by_row.keys().copied().collect();
        rows.sort_unstable();
        ChangeSet {
            deletes: self.deleted.iter().map(|&r| self.key_of(r)).collect(),
            updates: rows
                .into_iter()
                .map(|r| {
                    let mut sets = by_row.remove(&r).unwrap_or_default();
                    sets.sort_by_key(|(c, _)| *c);
                    RowUpdate {
                        key: self.key_of(r),
                        sets,
                    }
                })
                .collect(),
            inserts: self
                .inserted
                .iter()
                .map(|row| RowInsert {
                    values: row
                        .iter()
                        .map(|c| c.as_ref().map(|(v, _)| v.clone()))
                        .collect(),
                })
                .collect(),
        }
    }
}

impl GridSource for TableData {
    fn n_rows(&self) -> usize {
        TableData::n_rows(self)
    }

    fn n_cols(&self) -> usize {
        self.rs.columns.len()
    }

    fn header(&self, c: usize) -> HeaderView<'_> {
        let col = &self.rs.columns[c];
        HeaderView {
            name: &col.name,
            type_name: &col.type_name,
            pk: self.pk.contains(&c),
            fk: self.fk.get(c).copied().unwrap_or(false),
            sort: self.sort.filter(|(sc, _)| *sc == c).map(|(_, d)| d),
        }
    }

    fn cell(&self, r: usize, c: usize) -> CellView<'_> {
        let numeric = self.rs.columns[c].class.is_numeric();
        let base = self.base();
        if r < base {
            if let Some((v, d)) = self.edits.get(&(r, c)) {
                return CellView {
                    text: if v.is_null() { NULL_TEXT } else { d },
                    kind: if v.is_null() {
                        CellKind::Null
                    } else {
                        CellKind::Value
                    },
                    edited: true,
                    numeric,
                };
            }
            let v = &self.rs.rows[r][c];
            CellView {
                text: &self.rs.display[r][c],
                kind: if v.is_null() {
                    CellKind::Null
                } else {
                    CellKind::Value
                },
                edited: false,
                numeric,
            }
        } else {
            match self.inserted.get(r - base).and_then(|row| row[c].as_ref()) {
                None => CellView {
                    text: DEFAULT_TEXT,
                    kind: CellKind::Default,
                    edited: false,
                    numeric,
                },
                Some((v, d)) => CellView {
                    text: d,
                    kind: if v.is_null() {
                        CellKind::Null
                    } else {
                        CellKind::Value
                    },
                    edited: false,
                    numeric,
                },
            }
        }
    }

    fn row_state(&self, r: usize) -> RowState {
        if r >= self.base() {
            RowState::Inserted
        } else if self.deleted.contains(&r) {
            RowState::Deleted
        } else {
            RowState::Normal
        }
    }

    fn editable(&self) -> bool {
        self.editable
    }

    fn edit_text(&self, r: usize, c: usize) -> String {
        self.value_at(r, c)
            .and_then(|v| v.to_text())
            .unwrap_or_default()
    }
}

/// 테이블 탭 상태.
pub(crate) struct TableView {
    conn: ConnId,
    t: TableRef,
    driver: Driver,
    sub: SubTab,
    details: Option<TableDetails>,
    details_job: Option<Job<DbResult<TableDetails>>>,
    kind_job: Option<Job<DbResult<Option<crate::TableKind>>>>,
    ddl: Option<Result<String, String>>,
    ddl_job: Option<Job<DbResult<String>>>,
    ddl_hl: SqlHighlighter,
    data: Option<TableData>,
    load_job: Option<Job<DbResult<ResultSet>>>,
    load_started: Option<Instant>,
    last_load_ms: u64,
    count_job: Option<Job<DbResult<i64>>>,
    total: Option<i64>,
    submit_job: Option<Job<Result<u64, ChangeError>>>,
    export_job: Option<Job<DbResult<u64>>>,
    page: usize,
    page_size: usize,
    filter: String,
    order: String,
    applied_filter: String,
    applied_order: String,
    grid: GridState,
    viewer: ValueViewer,
    show_viewer: bool,
    status: Option<(String, bool)>,
    load_error: Option<String>,
    is_view: bool,
}

impl TableView {
    pub fn new(m: &DbManager, conn: ConnId, schema: Option<String>, table: String) -> TableView {
        let driver = m.driver(conn).unwrap_or(Driver::Postgres);
        let t = TableRef::new(schema, table);
        let grid_id = egui::Id::new(("db-table-grid", conn, &t.schema, &t.table));
        let mut v = TableView {
            conn,
            t,
            driver,
            sub: SubTab::Data,
            details: None,
            details_job: None,
            kind_job: None,
            ddl: None,
            ddl_job: None,
            ddl_hl: SqlHighlighter::default(),
            data: None,
            load_job: None,
            load_started: None,
            last_load_ms: 0,
            count_job: None,
            total: None,
            submit_job: None,
            export_job: None,
            page: 0,
            page_size: 500,
            filter: String::new(),
            order: String::new(),
            applied_filter: String::new(),
            applied_order: String::new(),
            grid: GridState::new(grid_id),
            viewer: ValueViewer::default(),
            show_viewer: false,
            status: None,
            load_error: None,
            is_view: false,
        };
        v.load_details(m);
        v.reload(m, true);
        v
    }

    pub fn table_name(&self) -> String {
        self.t.table.clone()
    }

    pub fn pending_changes(&self) -> usize {
        self.data.as_ref().map(|d| d.pending()).unwrap_or(0)
    }

    fn load_details(&mut self, m: &DbManager) {
        let m2 = m.clone();
        let (id, t) = (self.conn, self.t.clone());
        self.details_job = Some(m.spawn(async move { m2.table_details(id, &t).await }));
        let m3 = m.clone();
        let (id, t) = (self.conn, self.t.clone());
        let schema = t.schema.clone().unwrap_or_default();
        let kind_job = m.spawn(async move {
            m3.list_tables(id, &schema)
                .await
                .map(|ts| ts.into_iter().find(|x| x.name == t.table).map(|x| x.kind))
        });
        self.kind_job = Some(kind_job);
    }

    fn reload(&mut self, m: &DbManager, recount: bool) {
        let (id, t) = (self.conn, self.t.clone());
        let (f, o) = (self.applied_filter.clone(), self.applied_order.clone());
        let (limit, offset) = (self.page_size, self.page * self.page_size);
        let m2 = m.clone();
        self.load_job =
            Some(m.spawn(async move { m2.fetch_page(id, &t, &f, &o, limit, offset).await }));
        self.load_started = Some(Instant::now());
        if recount {
            self.total = None;
            let m3 = m.clone();
            let (t, f) = (self.t.clone(), self.applied_filter.clone());
            self.count_job = Some(m.spawn(async move { m3.count_rows(id, &t, &f).await }));
        }
    }

    fn apply_filters(&mut self, m: &DbManager) {
        self.applied_filter = self.filter.trim().to_string();
        self.applied_order = self.order.trim().to_string();
        self.page = 0;
        self.reload(m, true);
    }

    fn poll(&mut self) {
        if let Some(j) = &mut self.details_job
            && let Some(r) = j.poll()
        {
            self.details_job = None;
            match r {
                Ok(d) => self.details = Some(d),
                Err(e) => self.status = Some((format!("구조를 불러오지 못했습니다: {e}"), true)),
            }
            self.refresh_details_on_data();
        }
        if let Some(j) = &mut self.kind_job
            && let Some(r) = j.poll()
        {
            self.kind_job = None;
            if let Ok(Some(k)) = r {
                self.is_view = k != crate::TableKind::Table;
                self.refresh_details_on_data();
            }
        }
        if let Some(j) = &mut self.load_job
            && let Some(r) = j.poll()
        {
            self.load_job = None;
            self.last_load_ms = self
                .load_started
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            match r {
                Ok(rs) => {
                    let keep_widths = self.data.as_ref().is_some_and(|d| {
                        d.rs.columns
                            .iter()
                            .map(|c| &c.name)
                            .eq(rs.columns.iter().map(|c| &c.name))
                    });
                    let mut data = TableData::new(rs);
                    data.sort = sort_from_order(&self.applied_order, &data.rs.columns, self.driver);
                    self.data = Some(data);
                    self.refresh_details_on_data();
                    if !keep_widths {
                        self.grid.col_widths.clear();
                    }
                    self.grid.sel.clear();
                    self.grid.editing = None;
                    self.load_error = None;
                }
                Err(e) => self.load_error = Some(e.to_string()),
            }
        }
        if let Some(j) = &mut self.count_job
            && let Some(r) = j.poll()
        {
            self.count_job = None;
            self.total = r.ok();
        }
        if let Some(j) = &mut self.ddl_job
            && let Some(r) = j.poll()
        {
            self.ddl_job = None;
            self.ddl = Some(r.map_err(|e| e.to_string()));
        }
        if let Some(j) = &mut self.export_job
            && let Some(r) = j.poll()
        {
            self.export_job = None;
            self.status = Some(match r {
                Ok(n) => (format!("{}행을 내보냈습니다", thousands(n as i64)), false),
                Err(e) => (format!("내보내기 실패: {e}"), true),
            });
        }
    }

    fn refresh_details_on_data(&mut self) {
        if let (Some(det), Some(data)) = (&self.details, &mut self.data) {
            data.apply_details(det, !self.is_view);
        }
    }

    fn poll_submit(&mut self, m: &DbManager) {
        let Some(j) = &mut self.submit_job else {
            return;
        };
        let Some(r) = j.poll() else {
            return;
        };
        self.submit_job = None;
        match r {
            Ok(n) => {
                self.status = Some((format!("커밋됨 · {n}행 영향"), false));
                if let Some(d) = &mut self.data {
                    d.revert_all();
                }
                self.reload(m, true);
            }
            Err(e) => {
                self.status = Some((
                    format!(
                        "트랜잭션 롤백됨 — 문 #{} 실패: {}",
                        e.index + 1,
                        e.error
                    ),
                    true,
                ));
            }
        }
    }

    fn submit(&mut self, m: &DbManager) {
        let (Some(data), Some(det)) = (&self.data, &self.details) else {
            return;
        };
        if data.pending() == 0 || self.submit_job.is_some() {
            return;
        }
        // 결과 집합 컬럼 순서를 상세 컬럼 순서로 옮긴다.
        let map: Vec<usize> = data
            .rs
            .columns
            .iter()
            .map(|c| {
                det.columns
                    .iter()
                    .position(|d| d.name == c.name)
                    .unwrap_or(usize::MAX)
            })
            .collect();
        let remap = |v: Vec<(usize, Value)>| -> Vec<(usize, Value)> {
            v.into_iter()
                .filter_map(|(c, val)| {
                    map.get(c)
                        .copied()
                        .filter(|x| *x != usize::MAX)
                        .map(|x| (x, val))
                })
                .collect()
        };
        let cs = data.change_set();
        let n_det = det.columns.len();
        let cs = ChangeSet {
            deletes: cs.deletes.into_iter().map(remap).collect(),
            updates: cs
                .updates
                .into_iter()
                .map(|u| RowUpdate {
                    key: remap(u.key),
                    sets: remap(u.sets),
                })
                .collect(),
            inserts: cs
                .inserts
                .into_iter()
                .map(|ins| {
                    let mut values = vec![None; n_det];
                    for (c, v) in ins.values.into_iter().enumerate() {
                        if let Some(&x) = map.get(c)
                            && x != usize::MAX
                        {
                            values[x] = v;
                        }
                    }
                    RowInsert { values }
                })
                .collect(),
        };
        let m2 = m.clone();
        let (id, t, cols) = (self.conn, self.t.clone(), det.columns.clone());
        self.status = Some(("제출 중…".into(), false));
        self.submit_job = Some(m.spawn(async move { m2.submit_changes(id, &t, &cols, &cs).await }));
    }

    pub fn ui(&mut self, ui: &mut Ui, m: &DbManager) {
        self.poll();
        self.poll_submit(m);
        let theme = Theme::current();
        if self.load_job.is_some()
            || self.submit_job.is_some()
            || self.count_job.is_some()
            || self.details_job.is_some()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
        // 전역 단축키.
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            if let Some(ed) = self.grid.editing.take() {
                self.commit_edit(ed.row, ed.col, &ed.text);
            }
            self.submit(m);
        }
        egui::Frame::new().fill(theme.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
            self.sub_tabs(ui, m);
            match self.sub {
                SubTab::Data => self.data_ui(ui, m),
                SubTab::Structure => self.structure_ui(ui),
                SubTab::Ddl => self.ddl_ui(ui, m),
            }
        });
    }

    fn sub_tabs(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        egui::Frame::new()
            .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 4 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let mut sub = self.sub;
                    if ui::segmented(
                        ui,
                        &mut sub,
                        &[(SubTab::Data, "데이터"), (SubTab::Structure, "구조"), (SubTab::Ddl, "DDL")],
                    ) {
                        self.sub = sub;
                        if sub == SubTab::Ddl && self.ddl.is_none() && self.ddl_job.is_none() {
                            let m2 = m.clone();
                            let (id, t) = (self.conn, self.t.clone());
                            self.ddl_job = Some(m.spawn(async move { m2.table_ddl(id, &t).await }));
                        }
                    }
                    ui.add_space(4.0);
                    ui::glyph_label(
                        ui,
                        if self.is_view { Icon::Eye } else { Icon::Table },
                        if self.is_view { theme.green } else { theme.blue },
                        14.0,
                    );
                    ui.label(RichText::new(self.t.sql_name(self.driver)).font(fonts::mono(12.5)).color(theme.text));
                    if self.is_view {
                        widgets::pill(ui, "뷰", theme.green);
                    }
                    if self.sub == SubTab::Data {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if ui::toggle_button_icon(ui, Some(Icon::Eye), "값", self.show_viewer)
                                .on_hover_text("값 뷰어 전환")
                                .clicked()
                            {
                                self.show_viewer = !self.show_viewer;
                            }
                            ui::menu_button(ui, Some(Icon::Download), "내보내기", |ui| {
                                for f in [ExportFormat::Csv, ExportFormat::Json] {
                                    if ui
                                        .button(format!("모든 행을 {}로…", f.extension().to_uppercase()))
                                        .clicked()
                                    {
                                        ui.close();
                                        self.export(m, f);
                                    }
                                }
                            });
                        });
                    }
                });
            });
    }

    fn data_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        self.toolbar(ui, m);
        self.filter_bar(ui, m);
        if let Some(data) = &self.data
            && !data.editable
            && self.details.is_some()
        {
            let why = if self.is_view {
                "읽기 전용: 뷰입니다."
            } else {
                "읽기 전용: 기본 키가 없어 편집할 행을 식별할 수 없습니다."
            };
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 12, right: 10, top: 0, bottom: 6 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui::glyph_label(ui, Glyph::Lock, theme.text_faint, 12.0);
                        ui.label(faint(why));
                    });
                });
        }
        if let Some(e) = &self.load_error {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 10, right: 10, top: 2, bottom: 8 })
                .show(ui, |ui| ui::banner(ui, e, true));
        }
        // 상태 줄.
        let status_h = 28.0;
        let avail = ui.available_rect_before_wrap();
        let grid_rect =
            egui::Rect::from_min_max(avail.min, egui::pos2(avail.max.x, avail.max.y - status_h));
        let status_rect =
            egui::Rect::from_min_max(egui::pos2(avail.min.x, avail.max.y - status_h), avail.max);
        ui.allocate_rect(avail, egui::Sense::hover());
        let mut grid_ui_area = ui.new_child(egui::UiBuilder::new().max_rect(grid_rect));
        self.grid_area(&mut grid_ui_area, m);
        let mut status_ui = ui.new_child(egui::UiBuilder::new().max_rect(status_rect));
        self.status_bar(&mut status_ui);
    }

    fn grid_area(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        if self.show_viewer {
            let cell_info = self.viewer_cell_owned();
            let mut applied = None;
            egui::Panel::right(egui::Id::new(("db-viewer", self.conn, &self.t.table)))
                .resizable(true)
                .default_size(320.0)
                .min_size(200.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme.bg_panel)
                        .stroke(egui::Stroke::new(1.0, theme.border))
                        .inner_margin(12),
                )
                .show(ui, |ui| {
                    let cell = cell_info.as_ref().map(|c| ViewerCell {
                        row: c.0,
                        col: c.1,
                        name: &c.2,
                        type_name: &c.3,
                        class: c.4,
                        value: c.5.as_ref(),
                        editable: c.6,
                    });
                    applied = self.viewer.ui(ui, cell);
                });
            if let (Some(text), Some((r, c, ..))) = (applied, cell_info) {
                self.commit_edit(r, c, &text);
            }
        }
        let Some(data) = &self.data else {
            ui.centered_and_justified(|ui| {
                if self.load_job.is_some() {
                    ui.add(egui::Spinner::new().size(18.0).color(theme.text_dim));
                } else {
                    ui.label(dim("데이터 없음"));
                }
            });
            return;
        };
        let events = grid_ui(ui, &mut self.grid, data);
        if self.load_job.is_some() {
            let r = ui.max_rect();
            ui.put(
                egui::Rect::from_center_size(r.center(), egui::vec2(24.0, 24.0)),
                egui::Spinner::new().size(18.0).color(theme.text_dim),
            );
        }
        for ev in events {
            self.handle_grid_event(ui, m, ev);
        }
    }

    /// 뷰어용 셀 정보(행, 열, 이름, 타입, 분류, 값, 편집 가능).
    #[allow(clippy::type_complexity)]
    fn viewer_cell_owned(
        &self,
    ) -> Option<(usize, usize, String, String, TypeClass, Option<Value>, bool)> {
        let data = self.data.as_ref()?;
        let (r, c) = self.grid.sel.cursor?;
        if r >= data.n_rows() || c >= data.rs.columns.len() {
            return None;
        }
        let col = &data.rs.columns[c];
        Some((
            r,
            c,
            col.name.clone(),
            col.type_name.clone(),
            col.class,
            data.value_at(r, c).cloned(),
            data.editable && data.row_state(r) != RowState::Deleted,
        ))
    }

    fn commit_edit(&mut self, r: usize, c: usize, text: &str) {
        if let Some(data) = &mut self.data
            && let Err(e) = data.commit_text(r, c, text)
        {
            let name = data.rs.columns[c].name.clone();
            self.status = Some((format!("{name}: {e}"), true));
        }
    }

    fn handle_grid_event(&mut self, ui: &mut Ui, m: &DbManager, ev: GridEvent) {
        let Some(data) = &mut self.data else {
            return;
        };
        let n_rows = data.n_rows();
        let n_cols = data.rs.columns.len();
        match ev {
            GridEvent::SortBy(c) => {
                if data.pending() > 0 {
                    self.status = Some((
                        "정렬하기 전에 보류 중인 변경 사항을 제출하거나 되돌리세요".into(),
                        true,
                    ));
                    return;
                }
                let next = match data.sort {
                    Some((sc, false)) if sc == c => Some((c, true)),
                    Some((sc, true)) if sc == c => None,
                    _ => Some((c, false)),
                };
                data.sort = next;
                self.order = match next {
                    Some((c, desc)) => order_for(self.driver, &data.rs.columns[c].name, desc),
                    None => String::new(),
                };
                self.applied_order = self.order.clone();
                self.page = 0;
                self.reload(m, false);
            }
            GridEvent::Commit { row, col, text } => self.commit_edit(row, col, &text),
            GridEvent::SetNull => {
                let cells = self.grid.sel.cells(n_rows, n_cols);
                for (r, c) in cells {
                    if data.row_state(r) != RowState::Deleted {
                        data.set_value(r, c, Value::Null);
                    }
                }
            }
            GridEvent::SetDefault => {
                let base = data.base();
                for (r, c) in self.grid.sel.cells(n_rows, n_cols) {
                    if r >= base {
                        data.inserted[r - base][c] = None;
                    }
                }
            }
            GridEvent::DeleteRows => {
                let rows = self.grid.sel.rows(n_rows);
                data.delete_rows(&rows);
                if data.n_rows() < n_rows {
                    self.grid.sel.clear();
                }
            }
            GridEvent::DuplicateRow => {
                let rows = self.grid.sel.rows(n_rows);
                data.duplicate(&rows);
                let last = data.n_rows() - 1;
                self.grid.sel.set_single((last, 0));
                self.grid.scroll_to_cursor();
            }
            GridEvent::AddRow => self.add_row(),
            GridEvent::RevertSelection => {
                let cells = self.grid.sel.cells(n_rows, n_cols);
                let rows: Vec<usize> = self
                    .grid
                    .sel
                    .rows(n_rows)
                    .into_iter()
                    .filter(|r| *r >= data.base() || data.deleted.contains(r))
                    .collect();
                data.revert_cells(&cells, &rows);
                self.grid.sel.clear();
            }
            GridEvent::Copy(fmt, header) => {
                let text =
                    copy_selection(data, &self.grid, fmt, header, self.driver, &self.t.table);
                ui.ctx().copy_text(text);
            }
            GridEvent::ViewValue => self.show_viewer = true,
            GridEvent::SelectionChanged => {}
        }
    }

    fn add_row(&mut self) {
        let Some(data) = &mut self.data else {
            return;
        };
        if !data.editable {
            return;
        }
        let r = data.add_row();
        self.grid.sel.set_single((r, 0));
        self.grid.scroll_to_cursor();
    }

    fn toolbar(&mut self, ui: &mut Ui, m: &DbManager) {
        let pending = self.pending_changes();
        let editable = self.data.as_ref().is_some_and(|d| d.editable);
        let has_sel = !self.grid.sel.ranges.is_empty();
        egui::Frame::new()
            .inner_margin(egui::Margin { left: 10, right: 10, top: 4, bottom: 4 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if glyph_button(
                        ui,
                        Glyph::Common(Icon::Refresh),
                        "페이지 다시 불러오기",
                        self.load_job.is_none() && pending == 0,
                        false,
                    )
                    .clicked()
                    {
                        self.reload(m, true);
                    }
                    toolbar_sep(ui);
                    if tool_button_icon(ui, Some(Icon::Plus), "행 추가", editable, false).clicked() {
                        self.add_row();
                    }
                    if tool_button_icon(ui, Some(Icon::Minus), "행 삭제", editable && has_sel, false)
                        .on_hover_text("선택한 행 삭제 (⌘⌫)")
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        let rows = self.grid.sel.rows(d.n_rows());
                        d.delete_rows(&rows);
                    }
                    if tool_button_icon(ui, Some(Icon::Copy), "복제", editable && has_sel, false)
                        .on_hover_text("선택한 행 복제 (⌘D)")
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        let rows = self.grid.sel.rows(d.n_rows());
                        d.duplicate(&rows);
                    }
                    toolbar_sep(ui);
                    let submit_label = if pending > 0 {
                        format!("제출 ({pending})")
                    } else {
                        "제출".into()
                    };
                    if tool_button_icon(
                        ui,
                        Some(Icon::Check),
                        &submit_label,
                        pending > 0 && self.submit_job.is_none(),
                        true,
                    )
                    .on_hover_text("모든 변경 사항을 하나의 트랜잭션으로 커밋 (⌘↩)")
                    .clicked()
                    {
                        self.submit(m);
                    }
                    if tool_button_icon(ui, Some(Icon::Undo), "되돌리기", pending > 0, false)
                        .on_hover_text("보류 중인 변경 사항 모두 취소")
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        d.revert_all();
                        self.status = None;
                    }
                    if self.submit_job.is_some() {
                        ui::spinner(ui);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.pager(ui, m, pending);
                    });
                });
            });
    }

    fn pager(&mut self, ui: &mut Ui, m: &DbManager, pending: usize) {
        let theme = Theme::current();
        ui.spacing_mut().item_spacing.x = 2.0;
        let can_nav = pending == 0 && self.load_job.is_none();
        let total_pages = self
            .total
            .map(|t| ((t.max(0) as usize).div_ceil(self.page_size)).max(1));
        let loaded = self.data.as_ref().map(|d| d.rs.len()).unwrap_or(0);
        let has_next = match total_pages {
            Some(tp) => self.page + 1 < tp,
            None => loaded == self.page_size,
        };
        // 오른쪽에서 왼쪽 순서로 배치된다.
        let last = glyph_button(ui, Glyph::Last, "마지막 페이지", can_nav && has_next && total_pages.is_some(), false);
        let next = glyph_button(ui, Glyph::Common(Icon::ChevronRight), "다음 페이지", can_nav && has_next, false);
        let start = self.page * self.page_size;
        let range = if loaded == 0 {
            "0행".to_string()
        } else {
            format!(
                "{}–{}",
                thousands(start as i64 + 1),
                thousands((start + loaded) as i64)
            )
        };
        let of = match (self.total, self.count_job.is_some()) {
            (Some(t), _) => format!(" / 총 {}", thousands(t)),
            (None, true) => " / 총 …".into(),
            _ => String::new(),
        };
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{range}{of}"))
                .font(fonts::medium(12.0))
                .color(theme.text_dim),
        );
        ui.add_space(4.0);
        let prev = glyph_button(ui, Glyph::ChevronLeft, "이전 페이지", can_nav && self.page > 0, false);
        let first = glyph_button(ui, Glyph::First, "첫 페이지", can_nav && self.page > 0, false);
        ui.add_space(6.0);
        let mut size = self.page_size;
        egui::ComboBox::from_id_salt(("db-page-size", self.conn, &self.t.table))
            .selected_text(RichText::new(format!("페이지당 {size}")).size(12.0))
            .width(104.0)
            .show_ui(ui, |ui| {
                for s in PAGE_SIZES {
                    ui.selectable_value(&mut size, s, s.to_string());
                }
            });
        if size != self.page_size && pending == 0 {
            self.page_size = size;
            self.page = 0;
            self.reload(m, false);
        }
        let mut target = None;
        if first.clicked() {
            target = Some(0);
        }
        if prev.clicked() {
            target = Some(self.page.saturating_sub(1));
        }
        if next.clicked() {
            target = Some(self.page + 1);
        }
        if last.clicked()
            && let Some(tp) = total_pages
        {
            target = Some(tp - 1);
        }
        if let Some(p) = target {
            self.page = p;
            self.reload(m, false);
        }
    }

    fn filter_bar(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        egui::Frame::new()
            .inner_margin(egui::Margin { left: 10, right: 10, top: 2, bottom: 8 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let w = ((ui.available_width() - 190.0) / 2.0).max(120.0);
                    let mut apply = false;
                    for (label, text, hint, id) in [
                        (
                            "WHERE",
                            &mut self.filter,
                            "예: id > 100 AND name LIKE 'a%'",
                            "where",
                        ),
                        ("ORDER BY", &mut self.order, "예: created_at DESC", "order"),
                    ] {
                        let fid = egui::Id::new((id, self.conn, &self.t.table));
                        let focused = ui.memory(|mm| mm.has_focus(fid));
                        let r = widgets::input_frame(focused, false)
                            .inner_margin(egui::Margin { left: 8, right: 6, top: 2, bottom: 2 })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    ui.label(RichText::new(label).font(fonts::mono(11.0)).color(theme.purple));
                                    ui.add(
                                        egui::TextEdit::singleline(text)
                                            .id(fid)
                                            .font(egui::TextStyle::Monospace)
                                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 4)))
                                            .hint_text(RichText::new(hint).size(12.0).color(theme.text_faint))
                                            .desired_width(ui.available_width().min(w - 16.0 - if label == "WHERE" { 52.0 } else { 70.0 })),
                                    )
                                })
                                .inner
                            })
                            .inner;
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            apply = true;
                        }
                    }
                    let dirty = self.filter.trim() != self.applied_filter
                        || self.order.trim() != self.applied_order;
                    let r = if dirty {
                        tool_button_icon(ui, Some(Icon::Filter), "적용", true, true)
                    } else {
                        ui::secondary_button(ui, Some(Icon::Filter), "적용", false)
                    };
                    if r.clicked() {
                        apply = true;
                    }
                    if apply {
                        if self.pending_changes() > 0 {
                            self.status = Some((
                                "필터링하기 전에 보류 중인 변경 사항을 제출하거나 되돌리세요".into(),
                                true,
                            ));
                        } else {
                            self.apply_filters(m);
                        }
                    }
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.min_rect().max.y - 0.5,
            egui::Stroke::new(1.0, theme.border),
        );
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let r = ui.max_rect();
        ui.painter().rect_filled(r, 0.0, theme.bg);
        ui.painter()
            .hline(r.x_range(), r.min.y + 0.5, egui::Stroke::new(1.0, theme.border));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(r.shrink2(egui::vec2(12.0, 2.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(d) = &self.data {
                        ui.label(faint(format!(
                            "{}행 · {} ms",
                            thousands(d.rs.len() as i64),
                            self.last_load_ms
                        )));
                        if let Some((r, c)) = self.grid.sel.cursor
                            && c < d.rs.columns.len()
                        {
                            ui.label(faint(format!("{}행 · {}", r + 1, d.rs.columns[c].name)));
                        }
                        let p = d.pending();
                        if p > 0 {
                            widgets::pill(ui, &format!("보류 {p}건"), theme.yellow);
                        }
                    }
                    if let Some((msg, err)) = &self.status {
                        let c = if *err { theme.red } else { theme.green };
                        let (dr, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(dr.center(), 3.0, c);
                        ui.label(
                            RichText::new(crate::value::one_line(msg, 240))
                                .size(12.0)
                                .color(if *err { theme.red } else { theme.text_dim }),
                        )
                        .on_hover_text(msg.as_str());
                    }
                });
            },
        );
    }

    fn export(&mut self, m: &DbManager, fmt: ExportFormat) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.{}", self.t.table, fmt.extension()))
            .add_filter(fmt.extension().to_uppercase(), &[fmt.extension()])
            .save_file()
        else {
            return;
        };
        self.export_to(m, fmt, path);
    }

    /// 현재 필터·정렬로 모든 행을 파일에 쓴다.
    pub(crate) fn export_to(&mut self, m: &DbManager, fmt: ExportFormat, path: std::path::PathBuf) {
        let sql = select_sql(
            self.driver,
            &self.t,
            &self.applied_filter,
            &self.applied_order,
        );
        let m2 = m.clone();
        let id = self.conn;
        self.status = Some(("내보내는 중…".into(), false));
        self.export_job = Some(m.spawn(async move { m2.export_query(id, &sql, &path, fmt).await }));
    }

    fn structure_ui(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let Some(det) = &self.details else {
            ui.centered_and_justified(|ui| {
                ui.add(egui::Spinner::new().color(theme.text_dim));
            });
            return;
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme.border);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    section(ui, "컬럼", det.columns.len());
                    card(ui, "cols", &["", "이름", "타입", "NULL 허용", "기본값", "참조"], |ui| {
                        for c in &det.columns {
                            let fk = det.fk_target(&c.name);
                            if c.is_pk() {
                                ui::glyph_label(ui, Icon::Key, theme.yellow, 13.0).on_hover_text("기본 키");
                            } else if fk.is_some() {
                                ui::glyph_label(ui, Glyph::Link, theme.blue, 13.0).on_hover_text("외래 키");
                            } else {
                                ui.label("");
                            }
                            ui.label(RichText::new(&c.name).font(fonts::mono(12.5)).color(theme.text));
                            ui.label(RichText::new(&c.data_type).font(fonts::mono(12.0)).color(theme.purple));
                            if c.nullable {
                                ui.label(faint("예"));
                            } else {
                                widgets::pill(ui, "NOT NULL", theme.orange);
                            }
                            let def = if c.auto_increment && c.default.is_none() {
                                "자동".to_string()
                            } else {
                                c.default.clone().unwrap_or_default()
                            };
                            ui.label(RichText::new(def).font(fonts::mono(12.0)).color(theme.text_dim));
                            ui.label(RichText::new(fk.unwrap_or_default()).font(fonts::mono(12.0)).color(theme.blue));
                            ui.end_row();
                        }
                    });
                    ui.add_space(20.0);
                    section(ui, "인덱스", det.indexes.len());
                    card(ui, "idx", &["이름", "컬럼", "종류"], |ui| {
                        for ix in &det.indexes {
                            ui.label(RichText::new(&ix.name).font(fonts::mono(12.5)).color(theme.text));
                            ui.label(RichText::new(ix.columns.join(", ")).font(fonts::mono(12.0)).color(theme.text_dim));
                            if ix.primary {
                                widgets::pill(ui, "기본 키", theme.yellow);
                            } else if ix.unique {
                                widgets::pill(ui, "고유", theme.blue);
                            } else {
                                ui.label(faint("인덱스"));
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(20.0);
                    section(ui, "외래 키", det.foreign_keys.len());
                    card(ui, "fks", &["이름", "컬럼", "참조", "ON UPDATE", "ON DELETE"], |ui| {
                        for fk in &det.foreign_keys {
                            ui.label(RichText::new(&fk.name).font(fonts::mono(12.5)).color(theme.text));
                            ui.label(RichText::new(fk.columns.join(", ")).font(fonts::mono(12.0)).color(theme.text_dim));
                            ui.label(
                                RichText::new(format!("{}({})", fk.ref_table, fk.ref_columns.join(", ")))
                                    .font(fonts::mono(12.0))
                                    .color(theme.blue),
                            );
                            ui.label(faint(&fk.on_update));
                            ui.label(faint(&fk.on_delete));
                            ui.end_row();
                        }
                    });
                });
            });
    }

    fn ddl_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        match &self.ddl {
            None => {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().color(theme.text_dim));
                });
            }
            Some(Err(e)) => {
                let e = e.clone();
                egui::Frame::new().inner_margin(12).show(ui, |ui| ui::banner(ui, &e, true));
            }
            Some(Ok(ddl)) => {
                let ddl = ddl.clone();
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, theme.border);
                egui::Frame::new()
                    .inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 16 })
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        egui::Frame::new()
                            .fill(theme.bg_panel)
                            .stroke(egui::Stroke::new(1.0, theme.border))
                            .corner_radius(10.0)
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                egui::Frame::new()
                                    .inner_margin(egui::Margin { left: 12, right: 8, top: 6, bottom: 6 })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("CREATE 문").font(fonts::semibold(12.5)).color(theme.text_dim));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                if glyph_button(ui, Glyph::Common(Icon::Refresh), "새로 고침", self.ddl_job.is_none(), false)
                                                    .clicked()
                                                {
                                                    let m2 = m.clone();
                                                    let (id, t) = (self.conn, self.t.clone());
                                                    self.ddl_job = Some(m.spawn(async move { m2.table_ddl(id, &t).await }));
                                                }
                                                if ui::secondary_button(ui, Some(Icon::Copy), "복사", true).clicked() {
                                                    ui.ctx().copy_text(ddl.clone());
                                                    self.status = Some(("DDL을 복사했습니다".into(), false));
                                                }
                                            });
                                        });
                                    });
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 0.0, theme.border);
                                let driver = self.driver;
                                let hl = &mut self.ddl_hl;
                                egui::ScrollArea::both()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                                            let job = hl.job(buf.as_str(), wrap, driver, 12.5);
                                            ui.fonts_mut(|f| f.layout_job(job))
                                        };
                                        let mut s = ddl.as_str();
                                        ui.add(
                                            egui::TextEdit::multiline(&mut s)
                                                .code_editor()
                                                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(14, 12)))
                                                .desired_width(f32::INFINITY)
                                                .layouter(&mut layouter),
                                        );
                                    });
                            });
                    });
            }
        }
    }
}

/// ORDER BY 텍스트가 단일 컬럼 정렬이면 (열, 내림차순 여부)를 돌려준다.
fn sort_from_order(order: &str, cols: &[ColumnInfo], driver: Driver) -> Option<(usize, bool)> {
    let o = order.trim();
    if o.is_empty() {
        return None;
    }
    cols.iter().enumerate().find_map(|(i, c)| {
        [false, true].into_iter().find_map(|desc| {
            let quoted = order_for(driver, &c.name, desc);
            let plain = format!("{} {}", c.name, if desc { "DESC" } else { "ASC" });
            (o.eq_ignore_ascii_case(&quoted)
                || o.eq_ignore_ascii_case(&plain)
                || (!desc && o == c.name))
                .then_some((i, desc))
        })
    })
}

/// 구조 화면 섹션 제목과 개수.
fn section(ui: &mut Ui, title: &str, count: usize) {
    let theme = Theme::current();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(title).font(fonts::semibold(13.0)).color(theme.text));
        ui.label(RichText::new(count.to_string()).font(fonts::medium(12.0)).color(theme.text_faint));
    });
    ui.add_space(8.0);
}

/// 둥근 테두리 카드 안의 표. 첫 줄은 흐린 세미볼드 머리글.
fn card(ui: &mut Ui, id: &str, headers: &[&str], body: impl FnOnce(&mut Ui)) {
    let theme = Theme::current();
    egui::Frame::new()
        .fill(theme.bg_panel)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(10.0)
        .inner_margin(egui::Margin { left: 14, right: 14, top: 10, bottom: 10 })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::Grid::new(id)
                .spacing(egui::vec2(22.0, 10.0))
                .min_row_height(20.0)
                .show(ui, |ui| {
                    for h in headers {
                        ui.label(RichText::new(*h).font(fonts::semibold(12.0)).color(theme.text_faint));
                    }
                    ui.end_row();
                    body(ui);
                });
        });
}

/// 툴바 구분선.
fn toolbar_sep(ui: &mut Ui) {
    let theme = Theme::current();
    let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 18.0), egui::Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, theme.border_strong));
}

/// 선택 영역을 지정 형식 문자열로 만든다.
pub(crate) fn copy_selection(
    data: &TableData,
    grid: &GridState,
    fmt: CopyFormat,
    header: bool,
    driver: Driver,
    table: &str,
) -> String {
    let n_rows = data.n_rows();
    let n_cols = data.rs.columns.len();
    let rows = grid.sel.rows(n_rows);
    let cols = grid.sel.cols(n_cols);
    let col_refs: Vec<&ColumnInfo> = cols.iter().map(|&c| &data.rs.columns[c]).collect();
    let null = Value::Null;
    let vals: Vec<Vec<&Value>> = rows
        .iter()
        .map(|&r| {
            cols.iter()
                .map(|&c| data.value_at(r, c).unwrap_or(&null))
                .collect()
        })
        .collect();
    format_rows(fmt, driver, Some(table), &col_refs, &vals, header)
}

/// 콘솔 결과 등 편집 불가 결과 집합을 그리드로 보여주는 공급자.
pub(crate) struct ReadOnlyGrid<'a> {
    pub rs: &'a ResultSet,
}

impl GridSource for ReadOnlyGrid<'_> {
    fn n_rows(&self) -> usize {
        self.rs.rows.len()
    }

    fn n_cols(&self) -> usize {
        self.rs.columns.len()
    }

    fn header(&self, c: usize) -> HeaderView<'_> {
        let col = &self.rs.columns[c];
        HeaderView {
            name: &col.name,
            type_name: &col.type_name,
            pk: false,
            fk: false,
            sort: None,
        }
    }

    fn cell(&self, r: usize, c: usize) -> CellView<'_> {
        let v = &self.rs.rows[r][c];
        CellView {
            text: &self.rs.display[r][c],
            kind: if v.is_null() {
                CellKind::Null
            } else {
                CellKind::Value
            },
            edited: false,
            numeric: self.rs.columns[c].class.is_numeric()
                || matches!(v, Value::Int(_) | Value::UInt(_) | Value::Float(_)),
        }
    }

    fn edit_text(&self, r: usize, c: usize) -> String {
        self.rs.rows[r][c].to_text().unwrap_or_default()
    }
}

/// 결과 집합 선택 영역을 지정 형식으로 만든다.
pub(crate) fn copy_result_selection(
    rs: &ResultSet,
    grid: &GridState,
    fmt: CopyFormat,
    header: bool,
    driver: Driver,
) -> String {
    let rows = grid.sel.rows(rs.rows.len());
    let cols = grid.sel.cols(rs.columns.len());
    let col_refs: Vec<&ColumnInfo> = cols.iter().map(|&c| &rs.columns[c]).collect();
    let vals: Vec<Vec<&Value>> = rows
        .iter()
        .map(|&r| cols.iter().map(|&c| &rs.rows[r][c]).collect())
        .collect();
    format_rows(fmt, driver, None, &col_refs, &vals, header)
}
