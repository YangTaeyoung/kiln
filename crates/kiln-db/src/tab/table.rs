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

/// A local-only snapshot. Restoration never submits database writes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableDraft {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<Value>>,
    edits: Vec<(usize, usize, Value)>,
    deleted: Vec<usize>,
    inserted: Vec<Vec<Option<Value>>>,
    page: usize,
    page_size: usize,
    filter: String,
    order: String,
    #[serde(default)]
    pending_cell: Option<(usize, usize, String)>,
}

struct RecoveryConflict {
    row: usize,
    current: Option<Vec<Value>>,
    description: String,
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
    recovery_review: bool,
    recovery_conflicts: Vec<RecoveryConflict>,
    recovery_check: Option<Job<DbResult<ResultSet>>>,
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
            recovery_review: false,
            recovery_conflicts: Vec::new(),
            recovery_check: None,
        };
        v.load_details(m);
        v.reload(m, true);
        v
    }

    pub fn table_name(&self) -> String {
        self.t.table.clone()
    }

    pub fn pending_changes(&self) -> usize {
        self.data.as_ref().map(|d| d.pending()).unwrap_or(0) + usize::from(self.pending_cell().is_some())
    }

    fn pending_cell(&self) -> Option<(usize,usize,String)> {
        let edit=self.grid.editing.as_ref()?;
        let data=self.data.as_ref()?;
        (edit.row < data.n_rows() && edit.col < data.rs.columns.len() && edit.text != data.edit_text(edit.row,edit.col)).then(|| (edit.row,edit.col,edit.text.clone()))
    }

    pub fn recovery_draft(&self) -> Option<TableDraft> {
        if self.pending_changes() == 0 { return None; }
        let data = self.data.as_ref()?;
        let mut edits: Vec<_> = data.edits.iter().map(|(&(r,c),(v,_))| (r,c,v.clone())).collect();
        edits.sort_by_key(|(r,c,_)| (*r,*c));
        Some(TableDraft { columns: data.rs.columns.clone(), rows: data.rs.rows.clone(), edits,
            deleted: data.deleted.iter().copied().collect(), inserted: data.inserted.iter().map(|r| r.iter().map(|v| v.as_ref().map(|(v,_)| v.clone())).collect()).collect(),
            page: self.page, page_size: self.page_size, filter: self.applied_filter.clone(), order: self.applied_order.clone(), pending_cell: self.pending_cell() })
    }

    pub fn restore_draft(&mut self, draft: &TableDraft) {
        self.load_job = None;
        self.count_job = None;
        self.page = draft.page;
        self.page_size = draft.page_size.clamp(1, 50_000);
        self.applied_filter = draft.filter.clone(); self.filter = draft.filter.clone();
        self.applied_order = draft.order.clone(); self.order = draft.order.clone();
        let mut data = TableData::new(ResultSet::new(draft.columns.clone(), draft.rows.clone()));
        for (r,c,v) in &draft.edits {
            if *r < data.rs.rows.len() && *c < data.rs.columns.len() { data.edits.insert((*r,*c), (v.clone(), v.display(DISPLAY_MAX_CHARS).into_boxed_str())); }
        }
        data.deleted = draft.deleted.iter().copied().filter(|r| *r < data.rs.rows.len()).collect();
        data.inserted = draft.inserted.iter().filter(|r| r.len() == data.rs.columns.len()).map(|r| r.iter().map(|v| v.as_ref().map(|v| (v.clone(), v.display(DISPLAY_MAX_CHARS).into_boxed_str()))).collect()).collect();
        if let Some((row,col,text))=&draft.pending_cell && *row < data.n_rows() && *col < data.rs.columns.len() {
            self.grid.editing=Some(crate::ui::grid::EditCell {row:*row,col:*col,text:text.clone(),focus_requested:false});
        }
        self.data = Some(data);
        self.recovery_review = true;
        self.refresh_details_on_data();
    }

    fn check_recovered_draft(&mut self, m: &DbManager) {
        if self.recovery_check.is_some() { return; }
        let Some(data)=&self.data else{return};
        if data.pk.is_empty() {self.status=Some((kiln_common::i18n::tr("기본 키 정보를 읽은 뒤 비교할 수 있습니다.").into(),true));return;}
        let rows=affected_rows(data,self.pending_cell().as_ref().map(|(r,_,_)|*r));
        let Some(details)=&self.details else{return};
        let columns=details.columns.clone();
        let mapping:Option<Vec<usize>>=data.rs.columns.iter().map(|col|columns.iter().position(|c|c.name==col.name)).collect();
        let Some(mapping)=mapping else{self.status=Some((kiln_common::i18n::tr("테이블 구조가 변경되었습니다. 초안을 내보내세요.").into(),true));return};
        let keys:Vec<Vec<(usize,Value)>>=rows.iter().map(|&row|data.pk.iter().map(|&col|(mapping[col],data.rs.rows[row][col].clone())).collect()).collect();
        let (id,t)=(self.conn,self.t.clone());let m2=m.clone();self.recovery_conflicts.clear();
        self.recovery_check=Some(m.spawn(async move { m2.fetch_original_rows(id,&t,&columns,&keys).await }));
    }

    fn export_recovery(&mut self) {
        let Some(draft)=self.recovery_draft() else{return};
        let Some(path)=rfd::FileDialog::new().set_file_name(format!("{}-draft.json",self.t.table)).add_filter(kiln_common::i18n::tr("Kiln DB 초안"),&["json"]).save_file() else{return};
        let result=self.write_recovery(&path,&draft);
        self.status=Some(match result{Ok(())=>(kiln_common::trf!("초안 내보냄: {} · DB에 적용되지 않았습니다",path.display()),false),Err(e)=>(kiln_common::trf!("초안 내보내기 실패: {e}"),true)});
    }

    fn write_recovery(&self,path:&std::path::Path,draft:&TableDraft)->Result<(),String>{
        let bytes=serde_json::to_vec_pretty(&serde_json::json!({"version":1,"schema":self.t.schema,"table":self.t.table,"draft":draft})).map_err(|e|e.to_string())?;
        kiln_common::safe_file::write(path,&bytes).map_err(|e|e.to_string())
    }

    fn resolve_recovery_row(&mut self,index:usize,keep_edits:bool) {
        if index>=self.recovery_conflicts.len(){return;}
        let conflict=self.recovery_conflicts.remove(index);
        let Some(data)=&mut self.data else{return};
        if !keep_edits || conflict.current.is_none() {
            data.edits.retain(|(row,_),_|*row!=conflict.row);data.deleted.remove(&conflict.row);
            if self.grid.editing.as_ref().is_some_and(|e|e.row==conflict.row){self.grid.editing=None;}
        }
        if let Some(current)=conflict.current {
            data.rs.display[conflict.row]=current.iter().map(|v|v.display(DISPLAY_MAX_CHARS).into_boxed_str()).collect();
            data.rs.rows[conflict.row]=current;
        }
        self.status=Some((kiln_common::i18n::tr("선택한 행을 정리했습니다. 최신 DB와 다시 비교한 뒤 제출하세요.").into(),false));
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
                Err(e) => self.status = Some((kiln_common::trf!("구조를 불러오지 못했습니다: {e}"), true)),
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
                Ok(n) => (kiln_common::trf!("{}행을 내보냈습니다", thousands(n as i64)), false),
                Err(e) => (kiln_common::trf!("내보내기 실패: {e}"), true),
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
                self.status = Some((kiln_common::trf!("커밋됨 · {n}행 영향"), false));
                if let Some(d) = &mut self.data {
                    d.revert_all();
                }
                self.reload(m, true);
            }
            Err(e) => {
                self.status = Some((
                    kiln_common::trf!(
                        "{}번째 SQL 실행 실패. 이 트랜잭션의 변경은 되돌렸습니다: {}",
                        e.index + 1,
                        e.error
                    ),
                    true,
                ));
            }
        }
    }

    fn submit(&mut self, m: &DbManager) {
        if let Some(edit) = self.grid.editing.take() {
            self.commit_edit(edit.row, edit.col, &edit.text);
            if self.grid.editing.is_some() { return; }
        }
        if self.recovery_review {
            self.status = Some((kiln_common::i18n::tr("복원된 초안은 먼저 최신 DB와 비교해야 제출할 수 있습니다.").into(), true));
            return;
        }
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
        self.status = Some((kiln_common::i18n::tr("DB에 변경 적용 중…").into(), false));
        self.submit_job = Some(m.spawn(async move { m2.submit_changes(id, &t, &cols, &cs).await }));
    }

    pub fn ui(&mut self, ui: &mut Ui, m: &DbManager) {
        self.poll();
        self.poll_submit(m);
        if let Some(job) = &mut self.recovery_check && let Some(result) = job.poll() {
            self.recovery_check = None;
            match result {
                Ok(fresh) => {
                    let pending=self.pending_cell().as_ref().map(|(r,_,_)|*r);
                    match self.data.as_ref().map(|data|find_recovery_conflicts(data,&fresh,pending)) {
                        Some(Ok(conflicts)) if conflicts.is_empty()=>{self.recovery_review=false;self.status=Some((kiln_common::i18n::tr("변경할 원본 행이 최신 DB와 일치합니다. 검토 후 직접 제출하세요.").into(),false));}
                        Some(Ok(conflicts))=>{self.status=Some((kiln_common::trf!("DB의 원본 {}개 행이 변경되었습니다. 행별로 해결하거나 초안을 내보내세요.",conflicts.len()),true));self.recovery_conflicts=conflicts;}
                        Some(Err(e))=>self.status=Some((e,true)),None=>{},
                    }
                }
                Err(e) => self.status = Some((kiln_common::trf!("초안 비교 실패: {e}"), true)),
            }
        }
        if self.recovery_check.is_some() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(60)); }
        if self.recovery_review && self.pending_changes() > 0 {
            ui.horizontal_wrapped(|ui| {
                ui.label(kiln_common::i18n::tr("복원된 DB 초안 · 아직 DB에 적용되지 않았습니다."));
                if ui.add_enabled(self.recovery_check.is_none(), egui::Button::new(kiln_common::i18n::tr("최신 DB와 비교"))).clicked() { self.check_recovered_draft(m); }
                if ui.button(kiln_common::i18n::tr("초안 내보내기…")).clicked(){self.export_recovery();}
            });
        } else if self.pending_changes() == 0 { self.recovery_review = false; }
        if !self.recovery_conflicts.is_empty() {
            let mut resolve=None;
            egui::ScrollArea::vertical().id_salt(self.grid.id.with("recovery-conflicts")).max_height(150.0).show(ui,|ui|{
                for (index,conflict) in self.recovery_conflicts.iter().enumerate(){
                    ui.group(|ui|{ui.label(&conflict.description);ui.horizontal_wrapped(|ui|{
                        if ui.button(kiln_common::i18n::tr("이 행의 초안 버리기")).clicked(){resolve=Some((index,false));}
                        if conflict.current.is_some() && ui.button(kiln_common::i18n::tr("내 편집을 최신 행에 재적용")).clicked(){resolve=Some((index,true));}
                    });});
                }
            });
            if let Some((index,keep))=resolve{self.resolve_recovery_row(index,keep);}
        }
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
                        &[(SubTab::Data, kiln_common::i18n::tr("데이터")), (SubTab::Structure, kiln_common::i18n::tr("구조")), (SubTab::Ddl, "DDL")],
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
                        widgets::pill(ui, kiln_common::i18n::tr("뷰"), theme.green);
                    }
                    if self.sub == SubTab::Data {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if ui::toggle_button_icon(ui, Some(Icon::Eye), kiln_common::i18n::tr("셀 내용"), self.show_viewer)
                                .on_hover_text(kiln_common::i18n::tr("선택한 셀의 전체 내용 보기"))
                                .clicked()
                            {
                                self.show_viewer = !self.show_viewer;
                            }
                            ui::menu_button(ui, Some(Icon::Download), kiln_common::i18n::tr("내보내기"), |ui| {
                                for f in [ExportFormat::Csv, ExportFormat::Json] {
                                    if ui
                                        .button(kiln_common::trf!("모든 행을 {}로…", f.extension().to_uppercase()))
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
                kiln_common::i18n::tr("읽기 전용: 뷰입니다.")
            } else {
                kiln_common::i18n::tr("읽기 전용: 기본 키가 없어 편집할 행을 식별할 수 없습니다.")
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
                    ui.label(dim(kiln_common::i18n::tr("데이터 없음")));
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
            self.grid.editing = Some(crate::ui::grid::EditCell { row:r, col:c, text:text.to_owned(), focus_requested:false });
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
                        kiln_common::i18n::tr("정렬하기 전에 보류 중인 변경 사항을 제출하거나 되돌리세요").into(),
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
                        kiln_common::i18n::tr("페이지 다시 불러오기"),
                        self.load_job.is_none() && pending == 0,
                        false,
                    )
                    .clicked()
                    {
                        self.reload(m, true);
                    }
                    toolbar_sep(ui);
                    if tool_button_icon(ui, Some(Icon::Plus), kiln_common::i18n::tr("행 추가"), editable, false).clicked() {
                        self.add_row();
                    }
                    if tool_button_icon(ui, Some(Icon::Minus), kiln_common::i18n::tr("행 삭제"), editable && has_sel, false)
                        .on_hover_text(kiln_common::i18n::tr("선택한 행 삭제 (⌘⌫)"))
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        let rows = self.grid.sel.rows(d.n_rows());
                        d.delete_rows(&rows);
                    }
                    if tool_button_icon(ui, Some(Icon::Copy), kiln_common::i18n::tr("복제"), editable && has_sel, false)
                        .on_hover_text(kiln_common::i18n::tr("선택한 행 복제 (⌘D)"))
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        let rows = self.grid.sel.rows(d.n_rows());
                        d.duplicate(&rows);
                    }
                    toolbar_sep(ui);
                    let submit_label = if pending > 0 {
                        kiln_common::trf!("제출 ({pending})")
                    } else {
                        kiln_common::i18n::tr("제출").into()
                    };
                    if tool_button_icon(
                        ui,
                        Some(Icon::Check),
                        &submit_label,
                        pending > 0 && self.submit_job.is_none(),
                        true,
                    )
                    .on_hover_text(kiln_common::i18n::tr("모든 변경 사항을 하나의 트랜잭션으로 커밋 (⌘↩)"))
                    .clicked()
                    {
                        self.submit(m);
                    }
                    if tool_button_icon(ui, Some(Icon::Undo), kiln_common::i18n::tr("되돌리기"), pending > 0, false)
                        .on_hover_text(kiln_common::i18n::tr("보류 중인 변경 사항 모두 취소"))
                        .clicked()
                        && let Some(d) = &mut self.data
                    {
                        d.revert_all();
                        self.grid.editing = None;
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
        let last = glyph_button(ui, Glyph::Last, kiln_common::i18n::tr("마지막 페이지"), can_nav && has_next && total_pages.is_some(), false);
        let next = glyph_button(ui, Glyph::Common(Icon::ChevronRight), kiln_common::i18n::tr("다음 페이지"), can_nav && has_next, false);
        let start = self.page * self.page_size;
        let range = if loaded == 0 {
            kiln_common::i18n::tr("0행").to_string()
        } else {
            format!(
                "{}–{}",
                thousands(start as i64 + 1),
                thousands((start + loaded) as i64)
            )
        };
        let of = match (self.total, self.count_job.is_some()) {
            (Some(t), _) => kiln_common::trf!(" / 총 {}", thousands(t)),
            (None, true) => kiln_common::i18n::tr(" / 총 …").into(),
            _ => String::new(),
        };
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("{range}{of}"))
                .font(fonts::medium(12.0))
                .color(theme.text_dim),
        );
        ui.add_space(4.0);
        let prev = glyph_button(ui, Glyph::ChevronLeft, kiln_common::i18n::tr("이전 페이지"), can_nav && self.page > 0, false);
        let first = glyph_button(ui, Glyph::First, kiln_common::i18n::tr("첫 페이지"), can_nav && self.page > 0, false);
        ui.add_space(6.0);
        let mut size = self.page_size;
        egui::ComboBox::from_id_salt(("db-page-size", self.conn, &self.t.table))
            .selected_text(RichText::new(kiln_common::trf!("페이지당 {size}")).size(12.0))
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
                let narrow=ui.available_width()<600.0;
                let mut render=|ui:&mut Ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let available=ui.available_width();
                    let w=if narrow {available}else{((available-90.0)/2.0).max(120.0)};
                    let mut apply = false;
                    for (label, text, hint, id) in [
                        (
                            "WHERE",
                            &mut self.filter,
                            kiln_common::i18n::tr("예: id > 100 AND name LIKE 'a%'"),
                            "where",
                        ),
                        ("ORDER BY", &mut self.order, kiln_common::i18n::tr("예: created_at DESC"), "order"),
                    ] {
                        let fid = egui::Id::new((id, self.conn, &self.t.table));
                        let focused = ui.memory(|mm| mm.has_focus(fid));
                        let r = widgets::input_frame(focused, false)
                            .inner_margin(egui::Margin { left: 8, right: 6, top: 2, bottom: 2 })
                            .show(ui, |ui| {
                                ui.set_width((w-16.0).max(100.0));
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    ui.label(RichText::new(label).font(fonts::mono(11.0)).color(theme.purple));
                                    ui.add(
                                        egui::TextEdit::singleline(text)
                                            .id(fid)
                                            .font(egui::TextStyle::Monospace)
                                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 4)))
                                            .hint_text(RichText::new(hint).size(12.0).color(theme.text_faint))
                                            .desired_width(ui.available_width().max(50.0)),
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
                        tool_button_icon(ui, Some(Icon::Filter), kiln_common::i18n::tr("적용"), true, true)
                    } else {
                        ui::secondary_button(ui, Some(Icon::Filter), kiln_common::i18n::tr("적용"), false)
                    };
                    if r.clicked() {
                        apply = true;
                    }
                    if apply {
                        if self.pending_changes() > 0 {
                            self.status = Some((
                                kiln_common::i18n::tr("필터링하기 전에 보류 중인 변경 사항을 제출하거나 되돌리세요").into(),
                                true,
                            ));
                        } else {
                            self.apply_filters(m);
                        }
                    }
                };
                if narrow {ui.vertical(&mut render);}else{ui.horizontal(&mut render);}
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
                        ui.label(faint(kiln_common::trf!(
                            "{}행 · {} ms",
                            thousands(d.rs.len() as i64),
                            self.last_load_ms
                        )));
                        if let Some((r, c)) = self.grid.sel.cursor
                            && c < d.rs.columns.len()
                        {
                            ui.label(faint(kiln_common::trf!("{}행 · {}", r + 1, d.rs.columns[c].name)));
                        }
                        let p = d.pending();
                        if p > 0 {
                            widgets::pill(ui, &kiln_common::trf!("보류 {p}건"), theme.yellow);
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
        self.status = Some((kiln_common::i18n::tr("내보내는 중…").into(), false));
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
                    section(ui, kiln_common::i18n::tr("컬럼"), det.columns.len());
                    card(ui, "cols", &["", kiln_common::i18n::tr("이름"), kiln_common::i18n::tr("타입"), kiln_common::i18n::tr("NULL 허용"), kiln_common::i18n::tr("기본값"), kiln_common::i18n::tr("참조")], |ui| {
                        for c in &det.columns {
                            let fk = det.fk_target(&c.name);
                            if c.is_pk() {
                                ui::glyph_label(ui, Icon::Key, theme.yellow, 13.0).on_hover_text(kiln_common::i18n::tr("기본 키"));
                            } else if fk.is_some() {
                                ui::glyph_label(ui, Glyph::Link, theme.blue, 13.0).on_hover_text(kiln_common::i18n::tr("외래 키"));
                            } else {
                                ui.label("");
                            }
                            ui.label(RichText::new(&c.name).font(fonts::mono(12.5)).color(theme.text));
                            ui.label(RichText::new(&c.data_type).font(fonts::mono(12.0)).color(theme.purple));
                            if c.nullable {
                                ui.label(faint(kiln_common::i18n::tr("예")));
                            } else {
                                widgets::pill(ui, "NOT NULL", theme.orange);
                            }
                            let def = if c.auto_increment && c.default.is_none() {
                                kiln_common::i18n::tr("자동").to_string()
                            } else {
                                c.default.clone().unwrap_or_default()
                            };
                            ui.label(RichText::new(def).font(fonts::mono(12.0)).color(theme.text_dim));
                            ui.label(RichText::new(fk.unwrap_or_default()).font(fonts::mono(12.0)).color(theme.blue));
                            ui.end_row();
                        }
                    });
                    ui.add_space(20.0);
                    section(ui, kiln_common::i18n::tr("인덱스"), det.indexes.len());
                    card(ui, "idx", &[kiln_common::i18n::tr("이름"), kiln_common::i18n::tr("컬럼"), kiln_common::i18n::tr("종류")], |ui| {
                        for ix in &det.indexes {
                            ui.label(RichText::new(&ix.name).font(fonts::mono(12.5)).color(theme.text));
                            ui.label(RichText::new(ix.columns.join(", ")).font(fonts::mono(12.0)).color(theme.text_dim));
                            if ix.primary {
                                widgets::pill(ui, kiln_common::i18n::tr("기본 키"), theme.yellow);
                            } else if ix.unique {
                                widgets::pill(ui, kiln_common::i18n::tr("고유"), theme.blue);
                            } else {
                                ui.label(faint(kiln_common::i18n::tr("인덱스")));
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(20.0);
                    section(ui, kiln_common::i18n::tr("외래 키"), det.foreign_keys.len());
                    card(ui, "fks", &[kiln_common::i18n::tr("이름"), kiln_common::i18n::tr("컬럼"), kiln_common::i18n::tr("참조"), "ON UPDATE", "ON DELETE"], |ui| {
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
                                            ui.label(RichText::new(kiln_common::i18n::tr("CREATE 문")).font(fonts::semibold(12.5)).color(theme.text_dim));
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                if glyph_button(ui, Glyph::Common(Icon::Refresh), kiln_common::i18n::tr("새로 고침"), self.ddl_job.is_none(), false)
                                                    .clicked()
                                                {
                                                    let m2 = m.clone();
                                                    let (id, t) = (self.conn, self.t.clone());
                                                    self.ddl_job = Some(m.spawn(async move { m2.table_ddl(id, &t).await }));
                                                }
                                                if ui::secondary_button(ui, Some(Icon::Copy), kiln_common::i18n::tr("복사"), true).clicked() {
                                                    ui.ctx().copy_text(ddl.clone());
                                                    self.status = Some((kiln_common::i18n::tr("DDL을 복사했습니다").into(), false));
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

fn affected_rows(data:&TableData,pending:Option<usize>)->BTreeSet<usize>{
    data.edits.keys().map(|(row,_)|*row).chain(data.deleted.iter().copied()).chain(pending).filter(|row|*row<data.rs.rows.len()).collect()
}
fn find_recovery_conflicts(data:&TableData,fresh:&ResultSet,pending:Option<usize>)->Result<Vec<RecoveryConflict>,String>{
    if data.pk.is_empty(){return Err(kiln_common::i18n::tr("기본 키가 없어 행을 안전하게 비교할 수 없습니다. 초안을 내보내세요.").into());}
    let map:Vec<usize>=data.rs.columns.iter().map(|col|fresh.columns.iter().position(|c|c.name==col.name).ok_or_else(||kiln_common::trf!("컬럼 {}이 없어졌습니다. 초안을 내보내세요.",col.name))).collect::<Result<_,_>>()?;
    let mut conflicts=Vec::new();
    for row in affected_rows(data,pending){
        let original=&data.rs.rows[row];
        let matches:Vec<_>=fresh.rows.iter().filter(|values|data.pk.iter().all(|&col|values.get(map[col])==original.get(col))).collect();
        let key=data.pk.iter().map(|&col|format!("{}={}",data.rs.columns[col].name,original[col].display(80))).collect::<Vec<_>>().join(", ");
        if matches.len()>1{return Err(kiln_common::trf!("기본 키가 중복된 행: {key}. 안전한 비교를 중단했습니다."));}
        let current=matches.first().map(|values|map.iter().map(|&col|values.get(col).cloned().unwrap_or(Value::Null)).collect::<Vec<_>>());
        if let Some(current)=&current {
            let differences=data.rs.columns.iter().enumerate().filter(|(col,_)|original[*col]!=current[*col]).map(|(col,c)|format!("{}: {} → {}",c.name,original[col].display(80),current[col].display(80))).collect::<Vec<_>>();
            if differences.is_empty(){continue;}
            conflicts.push(RecoveryConflict{row,current:Some(current.clone()),description:format!("{key} · {}",differences.join("; "))});
        }else{conflicts.push(RecoveryConflict{row,current:None,description:kiln_common::trf!("{key} · DB에서 삭제된 행")});}
    }
    Ok(conflicts)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn recovery_compares_only_changed_primary_keys_independent_of_row_order(){
        let columns=vec![ColumnInfo::new("id","INTEGER"),ColumnInfo::new("name","TEXT")];
        let mut data=TableData::new(ResultSet::new(columns.clone(),vec![vec![Value::Int(1),Value::Text("a".into())],vec![Value::Int(2),Value::Text("b".into())]]));
        data.pk=vec![0];data.edits.insert((0,1),(Value::Text("mine".into()),"mine".into()));
        let fresh=ResultSet::new(columns.clone(),vec![vec![Value::Int(2),Value::Text("unrelated changed".into())],vec![Value::Int(1),Value::Text("a".into())]]);
        assert!(find_recovery_conflicts(&data,&fresh,None).unwrap().is_empty());
        let fresh=ResultSet::new(columns,vec![vec![Value::Int(1),Value::Text("theirs".into())]]);
        let conflicts=find_recovery_conflicts(&data,&fresh,None).unwrap();assert_eq!(conflicts.len(),1);assert!(conflicts[0].description.contains("name: a → theirs"));
        let manager=DbManager::in_memory();let mut view=TableView::new(&manager,ConnId(999),None,"fixture".into());view.load_job=None;view.data=Some(data);view.recovery_review=true;view.recovery_conflicts=conflicts;
        view.resolve_recovery_row(0,true);assert!(view.recovery_review);assert!(view.submit_job.is_none());
        assert!(find_recovery_conflicts(view.data.as_ref().unwrap(),&fresh,None).unwrap().is_empty());
        assert_eq!(view.data.as_ref().unwrap().edits[&(0,1)].0,Value::Text("mine".into()));
    }
    #[test]
    fn restored_table_edits_never_submit_before_review() {
        let manager=DbManager::in_memory();
        let mut tab=TableView::new(&manager,ConnId(999),None,"fixture".into());
        let draft=TableDraft { columns:vec![ColumnInfo::new("id","INTEGER")], rows:vec![vec![Value::Int(1)]], edits:vec![(0,0,Value::Int(2))],deleted:vec![],inserted:vec![],page:0,page_size:500,filter:String::new(),order:String::new(),pending_cell:None };
        let draft:TableDraft=serde_json::from_str(&serde_json::to_string(&draft).unwrap()).unwrap();
        tab.restore_draft(&draft); assert_eq!(tab.pending_changes(),1); assert!(tab.load_job.is_none());
        tab.submit(&manager); assert!(tab.submit_job.is_none()); assert!(tab.recovery_review);
        assert_eq!(tab.recovery_draft().unwrap(),draft);
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("draft.json");tab.write_recovery(&path,&draft).unwrap();
        let exported:serde_json::Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(exported["table"],"fixture");let exported_draft:TableDraft=serde_json::from_value(exported["draft"].clone()).unwrap();assert_eq!(draft,exported_draft);
        assert!(tab.submit_job.is_none());
        let source=ResultSet::new(draft.columns.clone(),draft.rows.clone());
        tab.data.as_mut().unwrap().pk=vec![0];
        assert!(find_recovery_conflicts(tab.data.as_ref().unwrap(),&source,None).unwrap().is_empty());
        assert_eq!(find_recovery_conflicts(tab.data.as_ref().unwrap(),&ResultSet::new(draft.columns,vec![vec![Value::Int(3)]]),None).unwrap().len(),1);
    }
    #[test]
    fn in_progress_invalid_cell_text_is_recoverable_without_submitting() {
        let manager=DbManager::in_memory();
        let mut tab=TableView::new(&manager,ConnId(999),None,"fixture".into());
        tab.load_job=None;
        tab.data=Some(TableData::new(ResultSet::new(vec![ColumnInfo::new("id","INTEGER")],vec![vec![Value::Int(1)]])));
        tab.grid.editing=Some(crate::ui::grid::EditCell {row:0,col:0,text:"unfinished invalid integer".into(),focus_requested:true});
        assert_eq!(tab.pending_changes(),1);
        let draft=tab.recovery_draft().unwrap();
        let mut restored=TableView::new(&manager,ConnId(999),None,"fixture".into());
        restored.restore_draft(&draft);
        assert_eq!(restored.pending_cell(),Some((0,0,"unfinished invalid integer".into())));
        assert!(restored.submit_job.is_none());
        restored.recovery_review=false;
        restored.submit(&manager);
        assert!(restored.submit_job.is_none());
        assert_eq!(restored.pending_cell(),Some((0,0,"unfinished invalid integer".into())));
    }

    #[test]
    fn recovery_float_encoding_preserves_nonfinite_values() {
        for value in [f64::INFINITY,f64::NEG_INFINITY,f64::NAN,-0.0] {
            let encoded=serde_json::to_string(&Value::Float(value)).unwrap();
            let Value::Float(decoded)=serde_json::from_str::<Value>(&encoded).unwrap() else { panic!() };
            assert_eq!(value.to_bits(),decoded.to_bits());
        }
    }
}

#[cfg(test)]
mod narrow_filter_tests {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    #[test]
    fn narrow_filter_fields_and_apply_remain_inside_panel() {
        let manager=DbManager::in_memory();
        let mut view=TableView::new(&manager,ConnId(999),None,"fixture".into());
        view.load_job=None;view.filter="id > 2".into();view.order="created_at DESC".into();
        let mut initialized=false;
        let mut h=Harness::builder().with_size([320.0,300.0]).wgpu().build_ui_state(|ui,view:&mut TableView| {
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            view.filter_bar(ui,&manager);
        },view);
        h.run_steps(4);
        for label in ["WHERE","ORDER BY","적용"]{assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}");}
        assert!(h.get_by_label("ORDER BY").rect().top()>h.get_by_label("WHERE").rect().bottom());
        h.render().unwrap().save("/tmp/kiln-db-filter-320.png").unwrap();
    }
}
