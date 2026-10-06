//! SQL 콘솔 탭: 강조 편집기, 문장 단위 실행, 결과 하위 탭, 취소, 기록, EXPLAIN, 자동 LIMIT.

use super::table::{ReadOnlyGrid, copy_result_selection};
use super::viewer::{ValueViewer, ViewerCell};
use crate::driver::{DbError, StmtOutcome};
use crate::export::ExportFormat;
use crate::manager::{HistoryEntry, Job};
use crate::sql::{apply_auto_limit, returns_rows, split_statements, statement_at};
use crate::ui::grid::{GridEvent, GridState, grid_ui};
use crate::ui::highlight::SqlHighlighter;
use crate::ui::{self, dim, faint, thousands, toggle_button_icon, tool_button_icon};
use crate::{ConnId, ConsoleSession, DbManager, Driver, ResultSet};
use egui::text::{CCursor, CCursorRange};
use egui::{Key, Modifiers, RichText, Ui};
use kiln_common::icons::Icon;
use kiln_common::widgets;
use kiln_common::{Theme, fonts};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod completion_ui;

/// 실행한 문장 하나의 결과.
struct StmtRun {
    sql: String,
    /// 편집기 안 시작 바이트 오프셋.
    offset: usize,
    outcome: Result<StmtOutcome, DbError>,
    elapsed_ms: u64,
}

struct ResultTab {
    run: StmtRun,
    grid: GridState,
}

struct Running {
    job: Job<()>,
    done: Arc<Mutex<Vec<StmtRun>>>,
    total: usize,
    started: Instant,
    cancel_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConsoleDocument {
    pub path: Option<std::path::PathBuf>,
    pub text: String,
    pub saved_text: Option<String>,
}

pub(crate) struct ConsoleView {
    conn: ConnId,
    editor_id: egui::Id,
    focus_pending: bool,
    driver: Driver,
    sql: String,
    document_path: Option<std::path::PathBuf>,
    saved_text: Option<String>,
    document_error: Option<String>,
    confirm_open: bool,
    pending_history: Option<String>,
    hl: SqlHighlighter,
    session: Arc<Mutex<Option<ConsoleSession>>>,
    running: Option<Running>,
    results: Vec<ResultTab>,
    active: usize,
    auto_limit: bool,
    limit: usize,
    show_history: bool,
    history_filter: String,
    cursor: Option<(usize, usize)>,
    set_cursor: Option<usize>,
    messages: Vec<(String, bool)>,
    viewer: ValueViewer,
    show_viewer: bool,
    run_seq: u64,
    export_job: Option<Job<crate::DbResult<u64>>>,
    metadata: crate::completion::Metadata,
    completion: Option<completion_ui::CompletionState>,
    completion_requested: bool,
    completion_dismissed: Option<(String, usize)>,
    ime_composing: bool,
}

impl ConsoleView {
    pub fn new(m: &DbManager, conn: ConnId) -> ConsoleView {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        ConsoleView {
            editor_id: egui::Id::new(("db-console", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))),
            focus_pending: true,
            conn,
            driver: m.driver(conn).unwrap_or(Driver::Postgres),
            sql: String::new(),
            document_path: None,
            saved_text: None,
            document_error: None,
            confirm_open: false,
            pending_history: None,
            hl: SqlHighlighter::default(),
            session: Arc::new(Mutex::new(None)),
            running: None,
            results: Vec::new(),
            active: 0,
            auto_limit: true,
            limit: 1000,
            show_history: false,
            history_filter: String::new(),
            cursor: None,
            set_cursor: None,
            messages: Vec::new(),
            viewer: ValueViewer::default(),
            show_viewer: false,
            run_seq: 0,
            export_job: None,
            metadata: Default::default(),
            completion: None,
            completion_requested: false,
            completion_dismissed: None,
            ime_composing: false,
        }
    }

    pub fn set_text(&mut self, sql: &str) {
        self.completion = None;
        self.completion_requested = false;
        self.completion_dismissed = None;
        self.sql = sql.to_string();
        self.set_cursor = Some(sql.chars().count());
    }

    pub fn text(&self) -> &str { &self.sql }
    pub fn request_focus(&mut self) { self.focus_pending = true; }

    pub fn has_draft(&self) -> bool {
        self.saved_text.as_ref().map_or_else(|| self.document_path.is_some() || !self.sql.trim().is_empty(), |saved| saved != &self.sql)
    }
    pub fn document(&self) -> ConsoleDocument { ConsoleDocument {path:self.document_path.clone(),text:self.sql.clone(),saved_text:self.saved_text.clone()} }
    pub fn restore_document(&mut self, doc:&ConsoleDocument) {
        self.set_text(&doc.text);self.document_path=doc.path.clone();self.saved_text=doc.saved_text.clone();
        if let Some(path)=&doc.path {
            match std::fs::read_to_string(path) {
                Ok(text) if doc.saved_text.as_deref()==Some(&text)=>{},
                Ok(_)=>{self.saved_text=None;self.document_error=Some(kiln_common::i18n::tr("저장 파일이 외부에서 변경되었습니다. 다른 이름으로 저장하거나 파일을 다시 여세요.").into());}
                Err(e)=>{self.saved_text=None;self.document_error=Some(kiln_common::trf!("저장 파일을 읽지 못했습니다. 복원된 텍스트는 보존됩니다: {e}"));}
            }
        }
    }
    pub fn document_name(&self)->Option<String>{self.document_path.as_ref().and_then(|p|p.file_name()).map(|s|s.to_string_lossy().into_owned())}
    fn save_document(&mut self,path:std::path::PathBuf)->Result<(),String>{
        if self.document_path.as_ref()==Some(&path) {
            match std::fs::read_to_string(&path) {
                Ok(text) if self.saved_text.as_ref()==Some(&text)=>{},
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                _=>return Err(kiln_common::i18n::tr("파일이 외부에서 변경되어 덮어쓰기를 중단했습니다. 다른 이름으로 저장하거나 다시 여세요.").into()),
            }
        }
        kiln_common::safe_file::write(&path,self.sql.as_bytes()).map_err(|e|e.to_string())?;
        self.document_path=Some(path);self.saved_text=Some(self.sql.clone());self.document_error=None;Ok(())
    }
    fn open_document(&mut self,path:std::path::PathBuf)->Result<(),String>{
        let text=std::fs::read_to_string(&path).map_err(|e|e.to_string())?;
        self.set_text(&text);self.saved_text=Some(text);self.document_path=Some(path);self.document_error=None;Ok(())
    }
    fn choose_open(&mut self){
        if let Some(path)=rfd::FileDialog::new().add_filter("SQL",&["sql"]).pick_file() {
            if let Err(e)=self.open_document(path){self.document_error=Some(e);}
        }
    }
    fn choose_save(&mut self,save_as:bool){
        let path=if !save_as {self.document_path.clone()} else {None}.or_else(||rfd::FileDialog::new().set_file_name(self.document_name().unwrap_or_else(||"query.sql".into())).add_filter("SQL",&["sql"]).save_file());
        if let Some(path)=path && let Err(e)=self.save_document(path){self.document_error=Some(e);}
    }
    fn document_ui(&mut self,ui:&mut Ui){
        let theme=Theme::current();
        egui::Frame::new().fill(theme.bg_panel).inner_margin(egui::Margin::symmetric(12,8)).show(ui,|ui|{
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing=egui::vec2(8.0,6.0);
        ui.horizontal_wrapped(|ui|{
            ui.strong(self.document_name().unwrap_or_else(||kiln_common::i18n::tr("이름 없는 쿼리").into()));
            ui.label(RichText::new(if self.has_draft(){kiln_common::i18n::tr("저장하지 않은 변경")}else if self.document_path.is_some(){kiln_common::i18n::tr("저장됨")}else{kiln_common::i18n::tr("임시 쿼리")}).size(12.0).color(theme.text_dim));
            if ui.button(kiln_common::i18n::tr("열기…")).clicked(){if self.has_draft(){self.confirm_open=true;}else{self.choose_open();}}
            if ui.button(kiln_common::i18n::tr("저장")).clicked(){self.choose_save(false);}
            if ui.button(kiln_common::i18n::tr("다른 이름으로 저장…")).clicked(){self.choose_save(true);}
        });
        if let Some(error)=&self.document_error{ui.colored_label(Theme::current().red,error);}
        if self.pending_history.is_some(){
            ui.horizontal_wrapped(|ui|{ui.label(kiln_common::i18n::tr("수정 중인 쿼리를 기록의 SQL로 바꿀까요?"));if ui.button(kiln_common::i18n::tr("기록 불러오기 취소")).clicked(){self.pending_history=None;}if ui.button(kiln_common::i18n::tr("변경 버리고 기록 불러오기")).clicked(){if let Some(sql)=self.pending_history.take(){self.set_text(&sql);}}});
        }
        if self.confirm_open {
            ui.horizontal_wrapped(|ui|{ui.label(kiln_common::i18n::tr("현재 수정 내용을 버리고 파일을 열까요?"));if ui.button(kiln_common::i18n::tr("취소")).clicked(){self.confirm_open=false;}if ui.button(kiln_common::i18n::tr("변경 버리고 열기")).clicked(){self.confirm_open=false;self.choose_open();}});
        }
        });
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// 이미 받은 결과를 결과 탭으로 추가한다.
    pub fn push_result(&mut self, sql: &str, rs: ResultSet) {
        let n = rs.len() as u64;
        self.run_seq += 1;
        self.results.push(ResultTab {
            run: StmtRun {
                sql: sql.to_string(),
                offset: 0,
                outcome: Ok(StmtOutcome {
                    result: rs,
                    has_rows: true,
                    affected: n,
                    truncated: false,
                }),
                elapsed_ms: 0,
            },
            grid: GridState::new(self.editor_id.with(("result-grid", self.run_seq))),
        });
        self.active = self.results.len() - 1;
    }

    fn byte_of_char(&self, ci: usize) -> usize {
        self.sql
            .char_indices()
            .nth(ci)
            .map(|(b, _)| b)
            .unwrap_or(self.sql.len())
    }

    /// 선택 영역이 있으면 그 텍스트, 없으면 커서 아래 문장.
    fn current_statements(&self) -> Vec<(usize, String)> {
        if let Some((a, b)) = self.cursor {
            let (s, e) = (self.byte_of_char(a.min(b)), self.byte_of_char(a.max(b)));
            if e > s {
                let sel = &self.sql[s..e];
                return split_statements(sel, self.driver)
                    .into_iter()
                    .map(|r| (s + r.start, sel[r].to_string()))
                    .collect();
            }
            let cur = self.byte_of_char(a);
            if let Some(r) = statement_at(&self.sql, self.driver, cur) {
                return vec![(r.start, self.sql[r].to_string())];
            }
        }
        split_statements(&self.sql, self.driver)
            .into_iter()
            .next()
            .map(|r| vec![(r.start, self.sql[r].to_string())])
            .unwrap_or_default()
    }

    fn all_statements(&self) -> Vec<(usize, String)> {
        split_statements(&self.sql, self.driver)
            .into_iter()
            .map(|r| (r.start, self.sql[r].to_string()))
            .collect()
    }

    /// 문장들을 순서대로 실행한다. 오류가 나면 거기서 멈춘다.
    pub(crate) fn run(&mut self, m: &DbManager, stmts: Vec<(usize, String)>, explain: bool) {
        if stmts.is_empty() || self.running.is_some() {
            return;
        }
        let driver = self.driver;
        let limit = self.auto_limit.then_some(self.limit);
        let stmts: Vec<(usize, String, String)> = stmts
            .into_iter()
            .map(|(off, s)| {
                let exec = if explain {
                    match driver {
                        Driver::Sqlite => format!("EXPLAIN QUERY PLAN {s}"),
                        _ => format!("EXPLAIN {s}"),
                    }
                } else if let Some(n) = limit {
                    apply_auto_limit(&s, driver, n).unwrap_or_else(|| s.clone())
                } else {
                    s.clone()
                };
                (off, s, exec)
            })
            .collect();
        let done = Arc::new(Mutex::new(Vec::new()));
        let done2 = done.clone();
        let session = self.session.clone();
        let m2 = m.clone();
        let id = self.conn;
        let total = stmts.len();
        let job = m.spawn(async move {
            let sess = {
                let existing = session.lock().clone();
                match existing {
                    Some(s) => s,
                    None => match m2.open_session(id).await {
                        Ok(s) => {
                            *session.lock() = Some(s.clone());
                            s
                        }
                        Err(e) => {
                            if let Some((off, sql, _)) = stmts.first() {
                                done2.lock().push(StmtRun {
                                    sql: sql.clone(),
                                    offset: *off,
                                    outcome: Err(e),
                                    elapsed_ms: 0,
                                });
                            }
                            return;
                        }
                    },
                }
            };
            for (off, sql, exec) in stmts {
                let t0 = Instant::now();
                let cap = limit
                    .filter(|_| returns_rows(&exec, driver))
                    .map(|n| n.max(1));
                let outcome = sess.run(&exec, cap).await;
                let failed = outcome.is_err();
                done2.lock().push(StmtRun {
                    sql,
                    offset: off,
                    outcome,
                    elapsed_ms: t0.elapsed().as_millis() as u64,
                });
                if failed {
                    break;
                }
            }
        });
        self.running = Some(Running {
            job,
            done,
            total,
            started: Instant::now(),
            cancel_at: None,
        });
        // 이전 결과를 비운다.
        self.results.clear();
        self.active = 0;
    }

    fn cancel(&mut self, m: &DbManager) {
        let Some(r) = &mut self.running else {
            return;
        };
        if r.cancel_at.is_some() {
            return;
        }
        r.cancel_at = Some(Instant::now());
        if let Some(s) = self.session.lock().clone() {
            let _ = m.spawn(async move { s.cancel().await });
        }
        self.messages.push((kiln_common::i18n::tr("취소를 요청했습니다").into(), false));
    }

    fn poll(&mut self, m: &DbManager) {
        if let Some(j) = &mut self.export_job
            && let Some(r) = j.poll()
        {
            self.export_job = None;
            self.messages.push(match r {
                Ok(n) => (kiln_common::trf!("{}행을 내보냈습니다", thousands(n as i64)), false),
                Err(e) => (kiln_common::trf!("내보내기 실패: {e}"), true),
            });
        }
        let Some(r) = &mut self.running else {
            return;
        };
        let new: Vec<StmtRun> = std::mem::take(&mut *r.done.lock());
        let finished = r.job.poll().is_some() || !r.job.is_running();
        for run in new {
            let ok = run.outcome.is_ok();
            if ok && crate::sql::tokenize(&run.sql, self.driver)
                .iter()
                .find(|t| t.kind == crate::sql::TokKind::Word)
                .is_some_and(|t| matches!(
                    run.sql[t.start..t.end].to_ascii_uppercase().as_str(),
                    "CREATE" | "ALTER" | "DROP" | "RENAME" | "TRUNCATE"
                ))
            {
                self.metadata.reset();
            }
            m.push_history(
                self.conn,
                HistoryEntry {
                    sql: run.sql.clone(),
                    at: chrono::Utc::now().timestamp(),
                    elapsed_ms: run.elapsed_ms,
                    ok,
                },
            );
            let msg = match &run.outcome {
                Ok(o) if o.has_rows => (
                    kiln_common::trf!(
                        "{}행 조회 · {} ms{}",
                        thousands(o.result.len() as i64),
                        run.elapsed_ms,
                        if o.truncated { kiln_common::i18n::tr(" · 조회 한도에 도달해 일부 결과만 표시합니다") } else { "" }
                    ),
                    false,
                ),
                Ok(o) => (
                    kiln_common::trf!("{}행 영향 · {} ms", o.affected, run.elapsed_ms),
                    false,
                ),
                Err(e) => (format!("{e}"), true),
            };
            self.messages.push(msg);
            if let Err(e) = &run.outcome
                && let Some(p) = e.position
            {
                let char_off = self.sql[..run.offset].chars().count() + p.saturating_sub(1);
                self.set_cursor = Some(char_off);
            }
            self.run_seq += 1;
            self.results.push(ResultTab {
                run,
                grid: GridState::new(self.editor_id.with(("result-grid", self.run_seq))),
            });
            // 오류 탭, 또는 결과 집합이 있는 첫 탭을 보여준다.
            let idx = self.results.len() - 1;
            let has_rows = |t: &ResultTab| matches!(&t.run.outcome, Ok(o) if o.has_rows);
            if idx == 0 || !ok || self.results.get(self.active).is_none_or(|t| !has_rows(t)) {
                self.active = idx;
            }
        }
        let r = self.running.as_mut().expect("running");
        let stop_waiting = r
            .cancel_at
            .is_some_and(|t| t.elapsed() > Duration::from_secs(2) || self.driver == Driver::Sqlite);
        if stop_waiting && !finished {
            r.job.abort();
            self.messages
                .push((kiln_common::i18n::tr("쿼리 대기를 중단했습니다").into(), true));
            *self.session.lock() = None;
        }
        if finished || stop_waiting {
            let n = self.results.len();
            let total = r.total;
            let ms = r.started.elapsed().as_millis();
            self.running = None;
            if total > 1 {
                self.messages.push((
                    kiln_common::trf!("문 {total}개 중 {n}개 실행 · {ms} ms"),
                    false,
                ));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, m: &DbManager) {
        self.poll(m);
        if let Some(driver) = m.driver(self.conn) {
            self.driver = driver;
        }
        if !self.metadata.matches_connection(m, self.conn) {
            self.completion = None;
        }
        self.metadata.poll(m, self.conn, &[]);
        self.completion_keys(ui);
        let focused=ui.memory(|memory|memory.has_focus(self.editor_id));
        if focused && ui.input_mut(|i|i.consume_key(Modifiers::COMMAND|Modifiers::SHIFT,Key::S)){self.choose_save(true);}
        else if focused && ui.input_mut(|i|i.consume_key(Modifiers::COMMAND,Key::S)){self.choose_save(false);}
        let theme = Theme::current();
        if self.running.is_some() || self.export_job.is_some() || self.metadata.loading() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        let (run_all, run_cur) = ui.input_mut(|i| {
            let all = focused && i.consume_key(Modifiers::COMMAND | Modifiers::ALT, Key::Enter);
            let cur = focused && i.consume_key(Modifiers::COMMAND, Key::Enter);
            (all, cur)
        });
        if run_all {
            self.completion = None;
            let s = self.all_statements();
            self.run(m, s, false);
        } else if run_cur {
            self.completion = None;
            let s = self.current_statements();
            self.run(m, s, false);
        }
        let completion_bounds = ui.max_rect().intersect(ui.ctx().content_rect());
        egui::Frame::new().fill(theme.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.document_ui(ui);
            self.toolbar(ui, m);
            if self.show_history {
                egui::Panel::right(self.editor_id.with("history"))
                    .resizable(true)
                    .default_size(280.0)
                    .frame(
                        egui::Frame::new()
                            .fill(theme.bg_panel)
                            .stroke(egui::Stroke::new(1.0, theme.border))
                            .inner_margin(10),
                    )
                    .show(ui, |ui| self.history_ui(ui, m));
            }
            let editor_h = (ui.available_height() * 0.42).clamp(90.0, 600.0);
            egui::Panel::top(self.editor_id.with("editor"))
                .resizable(true)
                .default_size(editor_h)
                .min_size(60.0)
                .frame(egui::Frame::new().fill(theme.bg))
                .show(ui, |ui| self.editor_ui(ui, m, completion_bounds));
            self.results_ui(ui, m);
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        egui::Frame::new()
            .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 8 })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let running = self.running.is_some();
                    if tool_button_icon(ui, Some(Icon::Play), kiln_common::i18n::tr("실행"), !running, true)
                        .on_hover_text(kiln_common::i18n::tr("커서 위치 또는 선택 영역의 문 실행 (⌘↩)"))
                        .clicked()
                    {
                        let s = self.current_statements();
                        self.run(m, s, false);
                    }
                    if ui::secondary_button(ui, None, kiln_common::i18n::tr("모두 실행"), !running)
                        .on_hover_text(kiln_common::i18n::tr("모든 문 실행 (⌘⌥↩)"))
                        .clicked()
                    {
                        let s = self.all_statements();
                        self.run(m, s, false);
                    }
                    if tool_button_icon(ui, Some(Icon::Stop), kiln_common::i18n::tr("취소"), running, false).clicked() {
                        self.cancel(m);
                    }
                    if tool_button_icon(ui, Some(Icon::Sparkle), kiln_common::i18n::tr("실행 계획"), !running, false)
                        .on_hover_text(kiln_common::i18n::tr("현재 문의 쿼리 실행 계획 표시"))
                        .clicked()
                    {
                        let s = self.current_statements();
                        self.run(m, s, true);
                    }
                    if crate::ui::icon_button(ui, Icon::Code, kiln_common::i18n::tr("SQL 자동완성 (⌃Space · ⌥Esc)")).clicked() {
                        self.completion_requested = true;
                        ui.memory_mut(|memory| memory.request_focus(self.editor_id));
                    }
                    let (r, _) = ui.allocate_exact_size(egui::vec2(13.0, 18.0), egui::Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, theme.border_strong));
                    widgets::toggle(ui, &mut self.auto_limit)
                        .on_hover_text(kiln_common::i18n::tr("LIMIT이 없는 SELECT 문에 LIMIT 추가"));
                    ui.add_space(2.0);
                    ui.label(RichText::new(kiln_common::i18n::tr("행 제한")).size(12.5).color(theme.text_dim));
                    ui.add_enabled(
                        self.auto_limit,
                        egui::DragValue::new(&mut self.limit)
                            .range(1..=1_000_000)
                            .speed(10.0),
                    );
                    if running && let Some(r) = &self.running {
                        ui.add_space(6.0);
                        ui::spinner(ui);
                        ui.label(dim(format!(
                            "{:.1}s · {}/{}",
                            r.started.elapsed().as_secs_f32(),
                            self.results.len(),
                            r.total
                        )));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if toggle_button_icon(ui, Some(Icon::History), kiln_common::i18n::tr("기록"), self.show_history).clicked() {
                            self.show_history = !self.show_history;
                        }
                        if toggle_button_icon(ui, Some(Icon::Eye), kiln_common::i18n::tr("값"), self.show_viewer).clicked() {
                            self.show_viewer = !self.show_viewer;
                        }
                        ui.add_space(6.0);
                        let name = m
                            .get(self.conn)
                            .map(|c| c.display_name())
                            .unwrap_or_default();
                        let status = m.status(self.conn);
                        let g = ui.painter().layout_no_wrap(name, fonts::medium(12.0), theme.text);
                        let (pr, _) = ui.allocate_exact_size(egui::vec2(g.size().x + 32.0, 26.0), egui::Sense::hover());
                        ui.painter().rect_filled(pr, 13.0, theme.bg_hover);
                        ui::paint_dot(ui, egui::pos2(pr.left() + 13.0, pr.center().y), ui::status_color(&status));
                        ui.painter().galley(egui::pos2(pr.left() + 22.0, pr.center().y - g.size().y / 2.0), g, theme.text);
                    });
                });
            });
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, theme.border);
    }

    fn editor_ui(&mut self, ui: &mut Ui, m: &DbManager, completion_bounds: egui::Rect) {
        let theme = Theme::current();
        let driver = self.driver;
        let te_id = self.editor_id;
        if let Some(ci) = self.set_cursor.take()
            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id)
        {
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), te_id);
            ui.memory_mut(|m| m.request_focus(te_id));
        }
        egui::ScrollArea::vertical()
            .id_salt(self.editor_id.with("scroll"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let hl = &mut self.hl;
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let job = hl.job(buf.as_str(), wrap, driver, 13.0);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let out = egui::TextEdit::multiline(&mut self.sql)
                    .id(te_id)
                    .code_editor()
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(14, 10)))
                    .desired_width(f32::INFINITY)
                    .desired_rows(8)
                    .lock_focus(true)
                    .event_filter(egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: self.completion.is_some(),
                    })
                    .hint_text(RichText::new(kiln_common::i18n::tr("-- 여기에 SQL을 작성하세요. ⌘↩는 커서 위치의 문을, ⌘⌥↩는 전체를 실행합니다.")).monospace().color(theme.text_faint))
                    .layouter(&mut layouter)
                    .show(ui);
                if std::mem::take(&mut self.focus_pending) { out.response.request_focus(); }
                if let Some(cr) = out.cursor_range {
                    let r = cr.as_sorted_char_range();
                    self.cursor = Some((r.start.0, r.end.0));
                }
                self.completion_overlay(ui, &out, completion_bounds, m);
                // 편집기 아래 남는 공간 클릭 시 포커스.
                let rest = ui.available_rect_before_wrap();
                if rest.height() > 0.0 && ui.interact(rest, te_id.with("rest"), egui::Sense::click()).clicked() {
                    ui.memory_mut(|m| m.request_focus(te_id));
                }
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.max_rect().max.y - 0.5,
            egui::Stroke::new(1.0, theme.border),
        );
    }

    fn results_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        // 결과 탭 막대.
        egui::Frame::new()
            .inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 6 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let n = self.results.len();
                    for i in 0..n {
                        let t = &self.results[i];
                        let (label, color) = match &t.run.outcome {
                            Ok(o) if o.has_rows => (kiln_common::trf!("결과 {} · {}", i + 1, thousands(o.result.len() as i64)), theme.text),
                            Ok(o) => (kiln_common::trf!("#{} · {}행 영향", i + 1, o.affected), theme.text_dim),
                            Err(_) => (kiln_common::trf!("#{} · 오류", i + 1), theme.red),
                        };
                        let sel = self.active == i;
                        let r = ui::tab_chip(ui, &label, sel, color);
                        if r.clicked() {
                            self.active = i;
                        }
                        r.on_hover_text(crate::value::one_line(&t.run.sql, 300));
                    }
                    let out_sel = self.active == usize::MAX;
                    if ui::tab_chip(ui, kiln_common::i18n::tr("출력"), out_sel, theme.text).clicked() {
                        self.active = usize::MAX;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let exportable = self
                            .results
                            .get(self.active)
                            .is_some_and(|t| matches!(&t.run.outcome, Ok(o) if o.has_rows));
                        ui.add_enabled_ui(exportable && self.export_job.is_none(), |ui| {
                            ui::menu_button(ui, Some(Icon::Download), kiln_common::i18n::tr("내보내기"), |ui| {
                                for f in [ExportFormat::Csv, ExportFormat::Json] {
                                    if ui.button(kiln_common::trf!("모든 행을 {}로…", f.extension().to_uppercase())).clicked() {
                                        ui.close();
                                        self.export(m, f);
                                    }
                                }
                            });
                        });
                        if let Some(t) = self.results.get(self.active)
                            && let Ok(o) = &t.run.outcome
                        {
                            ui.label(faint(format!("{} ms", t.run.elapsed_ms)));
                            if o.truncated {
                                widgets::pill(ui, kiln_common::i18n::tr("제한됨"), theme.yellow)
                                    .on_hover_text(kiln_common::i18n::tr("행이 더 있습니다. 가져오려면 제한을 늘리거나 끄세요"));
                            }
                        }
                    });
                });
            });
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, theme.border);
        if self.active == usize::MAX || self.results.is_empty() {
            self.output_ui(ui);
            return;
        }
        let Some(tab) = self.results.get_mut(self.active) else {
            return;
        };
        match &tab.run.outcome {
            Err(e) => {
                let e = e.clone();
                let offset = tab.run.offset;
                let sql = tab.run.sql.clone();
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    ui::banner(ui, &e.to_string(), true);
                    if let Some(p) = e.position {
                        let char_off = self.sql[..offset.min(self.sql.len())].chars().count()
                            + p.saturating_sub(1);
                        let before: String = self.sql.chars().take(char_off).collect();
                        let line = before.matches('\n').count() + 1;
                        let col = before
                            .rsplit('\n')
                            .next()
                            .map(|l| l.chars().count())
                            .unwrap_or(0)
                            + 1;
                        ui.add_space(8.0);
                        widgets::pill(ui, &kiln_common::trf!("{line}줄, {col}열"), theme.red);
                    }
                    ui.add_space(10.0);
                    code_block(ui, &crate::value::one_line(&sql, 500));
                });
            }
            Ok(o) if !o.has_rows => {
                let (aff, ms, sql) = (o.affected, tab.run.elapsed_ms, tab.run.sql.clone());
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui::glyph_label(ui, Icon::Check, theme.green, 16.0);
                        ui.label(RichText::new(kiln_common::trf!("{aff}행 영향받음")).font(fonts::semibold(14.0)).color(theme.text));
                        ui.label(faint(format!("{ms} ms")));
                    });
                    ui.add_space(10.0);
                    code_block(ui, &crate::value::one_line(&sql, 500));
                });
            }
            Ok(_) => {
                self.grid_ui(ui);
            }
        }
    }

    fn grid_ui(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let driver = self.driver;
        let Some(tab) = self.results.get_mut(self.active) else {
            return;
        };
        let Ok(o) = &tab.run.outcome else {
            return;
        };
        if self.show_viewer {
            let rs = &o.result;
            let cell = tab
                .grid
                .sel
                .cursor
                .filter(|(r, c)| *r < rs.rows.len() && *c < rs.columns.len());
            egui::Panel::right(self.editor_id.with("viewer"))
                .resizable(true)
                .default_size(300.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme.bg_panel)
                        .stroke(egui::Stroke::new(1.0, theme.border))
                        .inner_margin(12),
                )
                .show(ui, |ui| {
                    let vc = cell.map(|(r, c)| ViewerCell {
                        row: r,
                        col: c,
                        name: &rs.columns[c].name,
                        type_name: &rs.columns[c].type_name,
                        class: rs.columns[c].class,
                        value: Some(&rs.rows[r][c]),
                        editable: false,
                    });
                    self.viewer.ui(ui, vc);
                });
        }
        let src = ReadOnlyGrid { rs: &o.result };
        let events = grid_ui(ui, &mut tab.grid, &src);
        for ev in events {
            match ev {
                GridEvent::Copy(fmt, header) => {
                    let text = copy_result_selection(&o.result, &tab.grid, fmt, header, driver);
                    ui.ctx().copy_text(text);
                }
                GridEvent::ViewValue => self.show_viewer = true,
                _ => {}
            }
        }
    }

    fn output_ui(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        egui::ScrollArea::vertical()
            .id_salt(self.editor_id.with("output"))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(14).show(ui, |ui| {
                    if self.messages.is_empty() {
                        widgets::empty_state(ui, Icon::Terminal, kiln_common::i18n::tr("아직 출력이 없습니다"), None);
                    }
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (msg, err) in &self.messages {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 14.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 2.5, if *err { theme.red } else { theme.text_faint });
                            ui.label(RichText::new(msg).font(fonts::mono(12.0)).color(if *err {
                                theme.red
                            } else {
                                theme.text_dim
                            }));
                        });
                    }
                });
            });
    }

    fn history_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        ui.label(RichText::new(kiln_common::i18n::tr("기록")).font(fonts::semibold(13.0)).color(theme.text));
        ui.add_space(6.0);
        let w = ui.available_width();
        ui::text_field(
            ui,
            egui::TextEdit::singleline(&mut self.history_filter).hint_text(RichText::new(kiln_common::i18n::tr("검색")).color(theme.text_faint)),
            self.editor_id.with("history-filter"),
            w,
        );
        ui.add_space(6.0);
        let filter = self.history_filter.to_lowercase();
        let entries = m.history(self.conn);
        let mut pick: Option<(String, bool)> = None;
        egui::ScrollArea::vertical()
            .id_salt(self.editor_id.with("history-scroll"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for e in entries
                    .iter()
                    .filter(|e| filter.is_empty() || e.sql.to_lowercase().contains(&filter))
                    .take(300)
                {
                    let when = chrono::DateTime::from_timestamp(e.at, 0)
                        .map(|d| {
                            d.with_timezone(&chrono::Local)
                                .format("%m-%d %H:%M")
                                .to_string()
                        })
                        .unwrap_or_default();
                    let (rect, resp) = ui::tree_row(ui, 44.0, false, &e.sql);
                    let p = ui.painter();
                    p.circle_filled(rect.min + egui::vec2(10.0, 14.0), 3.0, if e.ok { theme.green } else { theme.red });
                    p.text(
                        rect.min + egui::vec2(20.0, 6.0),
                        egui::Align2::LEFT_TOP,
                        crate::value::one_line(&e.sql, 60),
                        fonts::mono(12.0),
                        theme.text,
                    );
                    p.text(
                        rect.min + egui::vec2(20.0, 25.0),
                        egui::Align2::LEFT_TOP,
                        format!("{when} · {} ms", e.elapsed_ms),
                        fonts::regular(11.0),
                        theme.text_faint,
                    );
                    let resp = resp.on_hover_text(&e.sql);
                    if ui::double_clicked(ui, &resp) {
                        pick = Some((e.sql.clone(), true));
                    } else if resp.clicked() {
                        pick = Some((e.sql.clone(), false));
                    }
                }
            });
        if let Some((sql, run)) = pick {
            if self.has_draft(){self.pending_history=Some(sql);}else{
                self.set_text(&sql);
                if run { self.run(m, vec![(0, sql)], false); }
            }
        }
    }

    fn export(&mut self, m: &DbManager, fmt: ExportFormat) {
        let Some(tab) = self.results.get(self.active) else {
            return;
        };
        let sql = tab.run.sql.clone();
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("result.{}", fmt.extension()))
            .add_filter(fmt.extension().to_uppercase(), &[fmt.extension()])
            .save_file()
        else {
            return;
        };
        let m2 = m.clone();
        let id = self.conn;
        self.export_job = Some(m.spawn(async move { m2.export_query(id, &sql, &path, fmt).await }));
    }
}

/// 둥근 테두리 안의 SQL 한 덩어리.
fn code_block(ui: &mut Ui, sql: &str) {
    let theme = Theme::current();
    egui::Frame::new()
        .fill(theme.bg_panel)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(sql).font(fonts::mono(12.0)).color(theme.text_dim)).wrap());
        });
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn command_enter_only_runs_the_focused_console(){
        let manager=DbManager::in_memory();let ctx=egui::Context::default();kiln_common::fonts::install(&ctx);
        let mut first=ConsoleView::new(&manager,ConnId(999));let mut second=ConsoleView::new(&manager,ConnId(999));
        first.set_text("select 1;");second.set_text("select 2;");first.focus_pending=false;second.focus_pending=false;
        let input=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(900.0,900.0))),..Default::default()};
        let mut output=ctx.run_ui(input.clone(),|ui|{ui.columns(2,|cols|{first.ui(&mut cols[0],&manager);second.ui(&mut cols[1],&manager);});});output.textures_delta.clear();
        ctx.memory_mut(|memory|memory.request_focus(second.editor_id));
        let mut input=input;input.events.push(egui::Event::Key{key:Key::Enter,physical_key:None,pressed:true,repeat:false,modifiers:Modifiers::COMMAND});
        let mut output=ctx.run_ui(input,|ui|{ui.columns(2,|cols|{first.ui(&mut cols[0],&manager);second.ui(&mut cols[1],&manager);});});output.textures_delta.clear();
        assert!(!first.is_running());assert!(second.is_running());
    }
    #[test]
    fn saved_query_has_a_clean_baseline_and_edits_survive_restart(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("query.sql");let manager=DbManager::in_memory();
        let mut console=ConsoleView::new(&manager,ConnId(1));console.set_text("select 1;");assert!(console.has_draft());
        console.save_document(path.clone()).unwrap();assert!(!console.has_draft());assert_eq!(std::fs::read_to_string(&path).unwrap(),"select 1;");
        console.set_text("select 2;");assert!(console.has_draft());let doc=console.document();
        let mut restored=ConsoleView::new(&manager,ConnId(1));restored.restore_document(&doc);assert!(restored.has_draft());assert_eq!(restored.text(),"select 2;");assert!(!restored.is_running());
        restored.save_document(path.clone()).unwrap();assert!(!restored.has_draft());
        let mut reopened=ConsoleView::new(&manager,ConnId(1));reopened.open_document(path).unwrap();assert!(!reopened.has_draft());assert_eq!(reopened.text(),"select 2;");
    }
    #[test]
    fn saving_query_refuses_external_replacement_and_preserves_dirty_text(){
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("query.sql");let manager=DbManager::in_memory();
        let mut console=ConsoleView::new(&manager,ConnId(1));console.set_text("select 1;");console.save_document(path.clone()).unwrap();console.set_text("select 2;");
        std::fs::write(&path,"external").unwrap();assert!(console.save_document(path.clone()).is_err());assert!(console.has_draft());assert_eq!(std::fs::read_to_string(&path).unwrap(),"external");
        let mut restored=ConsoleView::new(&manager,ConnId(1));restored.restore_document(&console.document());assert!(restored.has_draft());assert!(restored.document_error.is_some());
    }
    #[test]
    fn consoles_on_same_connection_have_independent_edit_state() {
        let manager=DbManager::in_memory();
        let mut first=ConsoleView::new(&manager,ConnId(1)); let second=ConsoleView::new(&manager,ConnId(1));
        assert_ne!(first.editor_id,second.editor_id);
        first.set_text("delete from example; -- not executed");
        assert!(first.has_draft()); assert!(!first.is_running()); assert!(first.results.is_empty());
        assert_eq!(second.text(), "");
    }
}
