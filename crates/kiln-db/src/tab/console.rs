//! SQL 콘솔 탭: 강조 편집기, 문장 단위 실행, 결과 하위 탭, 취소, 기록, EXPLAIN, 자동 LIMIT.

use super::table::{ReadOnlyGrid, copy_result_selection};
use super::viewer::{ValueViewer, ViewerCell};
use crate::driver::{DbError, StmtOutcome};
use crate::export::ExportFormat;
use crate::manager::{HistoryEntry, Job};
use crate::sql::{apply_auto_limit, returns_rows, split_statements, statement_at};
use crate::ui::grid::{GridEvent, GridState, grid_ui};
use crate::ui::highlight::SqlHighlighter;
use crate::ui::{self, dim, thousands, toggle_button, tool_button};
use crate::{ConnId, ConsoleSession, DbManager, Driver, ResultSet};
use egui::text::{CCursor, CCursorRange};
use egui::{Key, Modifiers, RichText, Ui};
use kiln_common::Theme;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

pub(crate) struct ConsoleView {
    conn: ConnId,
    driver: Driver,
    sql: String,
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
}

impl ConsoleView {
    pub fn new(m: &DbManager, conn: ConnId) -> ConsoleView {
        ConsoleView {
            conn,
            driver: m.driver(conn).unwrap_or(Driver::Postgres),
            sql: String::new(),
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
        }
    }

    pub fn set_text(&mut self, sql: &str) {
        self.sql = sql.to_string();
        self.set_cursor = Some(sql.chars().count());
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
            grid: GridState::new(egui::Id::new(("db-console-grid", self.conn, self.run_seq))),
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
        self.messages.push(("취소를 요청했습니다".into(), false));
    }

    fn poll(&mut self, m: &DbManager) {
        if let Some(j) = &mut self.export_job
            && let Some(r) = j.poll()
        {
            self.export_job = None;
            self.messages.push(match r {
                Ok(n) => (format!("{}행을 내보냈습니다", thousands(n as i64)), false),
                Err(e) => (format!("내보내기 실패: {e}"), true),
            });
        }
        let Some(r) = &mut self.running else {
            return;
        };
        let new: Vec<StmtRun> = std::mem::take(&mut *r.done.lock());
        let finished = r.job.poll().is_some() || !r.job.is_running();
        for run in new {
            let ok = run.outcome.is_ok();
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
                    format!(
                        "{}행 조회 · {} ms{}",
                        thousands(o.result.len() as i64),
                        run.elapsed_ms,
                        if o.truncated { " (제한됨)" } else { "" }
                    ),
                    false,
                ),
                Ok(o) => (
                    format!("{}행 영향 · {} ms", o.affected, run.elapsed_ms),
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
                grid: GridState::new(egui::Id::new(("db-console-grid", self.conn, self.run_seq))),
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
                .push(("쿼리 대기를 중단했습니다".into(), true));
            *self.session.lock() = None;
        }
        if finished || stop_waiting {
            let n = self.results.len();
            let total = r.total;
            let ms = r.started.elapsed().as_millis();
            self.running = None;
            if total > 1 {
                self.messages.push((
                    format!("문 {total}개 중 {n}개 실행 · {ms} ms"),
                    false,
                ));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, m: &DbManager) {
        self.poll(m);
        let theme = Theme::current();
        if self.running.is_some() || self.export_job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        let (run_all, run_cur) = ui.input_mut(|i| {
            let all = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
            let cur = i.consume_key(Modifiers::COMMAND, Key::Enter);
            (all, cur)
        });
        if run_all {
            let s = self.all_statements();
            self.run(m, s, false);
        } else if run_cur {
            let s = self.current_statements();
            self.run(m, s, false);
        }
        egui::Frame::new().fill(theme.bg).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.toolbar(ui, m);
            if self.show_history {
                egui::Panel::right(egui::Id::new(("db-console-history", self.conn)))
                    .resizable(true)
                    .default_size(280.0)
                    .frame(egui::Frame::new().fill(theme.bg_panel).inner_margin(8))
                    .show(ui, |ui| self.history_ui(ui, m));
            }
            let editor_h = (ui.available_height() * 0.42).clamp(90.0, 600.0);
            egui::Panel::top(egui::Id::new(("db-console-editor", self.conn)))
                .resizable(true)
                .default_size(editor_h)
                .min_size(60.0)
                .frame(egui::Frame::new().fill(theme.bg))
                .show(ui, |ui| self.editor_ui(ui));
            self.results_ui(ui, m);
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        egui::Frame::new()
            .fill(theme.bg_panel)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let running = self.running.is_some();
                    if tool_button(ui, "▶ 실행", !running, true)
                        .on_hover_text("커서 위치 또는 선택 영역의 문 실행 (⌘↩)")
                        .clicked()
                    {
                        let s = self.current_statements();
                        self.run(m, s, false);
                    }
                    if tool_button(ui, "⏩ 모두 실행", !running, false)
                        .on_hover_text("모든 문 실행 (⌘⇧↩)")
                        .clicked()
                    {
                        let s = self.all_statements();
                        self.run(m, s, false);
                    }
                    if tool_button(ui, "■ 취소", running, false).clicked() {
                        self.cancel(m);
                    }
                    if tool_button(ui, "실행 계획", !running, false)
                        .on_hover_text("현재 문의 쿼리 실행 계획 표시")
                        .clicked()
                    {
                        let s = self.current_statements();
                        self.run(m, s, true);
                    }
                    ui.separator();
                    ui.checkbox(&mut self.auto_limit, RichText::new("행 제한").size(12.0))
                        .on_hover_text("LIMIT이 없는 SELECT 문에 LIMIT 추가");
                    ui.add_enabled(
                        self.auto_limit,
                        egui::DragValue::new(&mut self.limit)
                            .range(1..=1_000_000)
                            .speed(10.0),
                    );
                    if running && let Some(r) = &self.running {
                        ui::spinner(ui);
                        ui.label(dim(format!(
                            "{:.1}s · {}/{}",
                            r.started.elapsed().as_secs_f32(),
                            self.results.len(),
                            r.total
                        )));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if toggle_button(ui, "기록", self.show_history).clicked() {
                            self.show_history = !self.show_history;
                        }
                        if toggle_button(ui, "값", self.show_viewer).clicked() {
                            self.show_viewer = !self.show_viewer;
                        }
                        let name = m
                            .get(self.conn)
                            .map(|c| c.display_name())
                            .unwrap_or_default();
                        let status = m.status(self.conn);
                        ui.label(RichText::new(name).size(12.0).color(theme.text_dim));
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui::paint_dot(ui, rect.center(), ui::status_color(&status));
                    });
                });
            });
    }

    fn editor_ui(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let driver = self.driver;
        let te_id = egui::Id::new(("db-console-editor-te", self.conn));
        if let Some(ci) = self.set_cursor.take()
            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id)
        {
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), te_id);
            ui.memory_mut(|m| m.request_focus(te_id));
        }
        let hl = &mut self.hl;
        egui::ScrollArea::vertical()
            .id_salt(("db-console-editor-scroll", self.conn))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                    let job = hl.job(buf.as_str(), wrap, driver, 13.0);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let out = egui::TextEdit::multiline(&mut self.sql)
                    .id(te_id)
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .margin(egui::vec2(12.0, 8.0))
                    .desired_width(f32::INFINITY)
                    .desired_rows(8)
                    .lock_focus(true)
                    .hint_text(RichText::new("-- 여기에 SQL을 작성하세요. ⌘↩는 커서 위치의 문을, ⌘⇧↩는 전체를 실행합니다.").monospace().color(theme.text_faint))
                    .layouter(&mut layouter)
                    .show(ui);
                if let Some(cr) = out.cursor_range {
                    let r = cr.as_sorted_char_range();
                    self.cursor = Some((r.start.0, r.end.0));
                }
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
            .fill(theme.bg_panel)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let n = self.results.len();
                    for i in 0..n {
                        let t = &self.results[i];
                        let (label, color) = match &t.run.outcome {
                            Ok(o) if o.has_rows => (format!("결과 {} · {}", i + 1, thousands(o.result.len() as i64)), theme.text),
                            Ok(o) => (format!("#{} · {}행 영향", i + 1, o.affected), theme.text_dim),
                            Err(_) => (format!("#{} · 오류", i + 1), theme.red),
                        };
                        let sel = self.active == i;
                        let r = ui.add(
                            egui::Button::new(RichText::new(label).size(11.5).color(if sel { color } else { color.gamma_multiply(0.8) }))
                                .fill(if sel { theme.bg_elevated } else { egui::Color32::TRANSPARENT })
                                .corner_radius(4.0),
                        );
                        if r.clicked() {
                            self.active = i;
                        }
                        r.on_hover_text(crate::value::one_line(&t.run.sql, 300));
                    }
                    let out_sel = self.active == usize::MAX;
                    if ui
                        .add(
                            egui::Button::new(RichText::new("출력").size(11.5).color(if out_sel { theme.text } else { theme.text_dim }))
                                .fill(if out_sel { theme.bg_elevated } else { egui::Color32::TRANSPARENT })
                                .corner_radius(4.0),
                        )
                        .clicked()
                    {
                        self.active = usize::MAX;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let exportable = self
                            .results
                            .get(self.active)
                            .is_some_and(|t| matches!(&t.run.outcome, Ok(o) if o.has_rows));
                        ui.add_enabled_ui(exportable && self.export_job.is_none(), |ui| {
                            ui.menu_button(RichText::new("내보내기 ⏷").size(12.0), |ui| {
                                for f in [ExportFormat::Csv, ExportFormat::Json] {
                                    if ui.button(format!("모든 행을 {}로…", f.extension().to_uppercase())).clicked() {
                                        ui.close();
                                        self.export(m, f);
                                    }
                                }
                            });
                        });
                        if let Some(t) = self.results.get(self.active)
                            && let Ok(o) = &t.run.outcome
                        {
                            ui.label(dim(format!("{} ms", t.run.elapsed_ms)));
                            if o.truncated {
                                ui.label(RichText::new("제한됨").size(11.0).color(theme.yellow))
                                    .on_hover_text("행이 더 있습니다. 가져오려면 제한을 늘리거나 끄세요");
                            }
                        }
                    });
                });
            });
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
                egui::Frame::new().inner_margin(12).show(ui, |ui| {
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
                        ui.add_space(6.0);
                        ui.label(dim(format!("{line}줄, {col}열")));
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(crate::value::one_line(&sql, 500))
                            .monospace()
                            .size(12.0)
                            .color(theme.text_dim),
                    );
                });
            }
            Ok(o) if !o.has_rows => {
                let (aff, ms, sql) = (o.affected, tab.run.elapsed_ms, tab.run.sql.clone());
                egui::Frame::new().inner_margin(12).show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("✔ {aff}행 영향받음"))
                            .color(theme.green)
                            .size(13.0),
                    );
                    ui.label(dim(format!("{ms} ms")));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(crate::value::one_line(&sql, 500))
                            .monospace()
                            .size(12.0)
                            .color(theme.text_dim),
                    );
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
            egui::Panel::right(egui::Id::new(("db-console-viewer", self.conn)))
                .resizable(true)
                .default_size(300.0)
                .frame(egui::Frame::new().fill(theme.bg_panel).inner_margin(8))
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
            .id_salt(("db-console-output", self.conn))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(10).show(ui, |ui| {
                    if self.messages.is_empty() {
                        ui.label(dim("아직 출력이 없습니다"));
                    }
                    for (msg, err) in &self.messages {
                        ui.label(RichText::new(msg).monospace().size(12.0).color(if *err {
                            theme.red
                        } else {
                            theme.text_dim
                        }));
                    }
                });
            });
    }

    fn history_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        ui.label(RichText::new("기록").strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.history_filter)
                .hint_text("검색")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        let filter = self.history_filter.to_lowercase();
        let entries = m.history(self.conn);
        let mut pick: Option<(String, bool)> = None;
        egui::ScrollArea::vertical()
            .id_salt(("db-console-history-scroll", self.conn))
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
                    let (rect, resp) = ui::tree_row(ui, 38.0, false, &e.sql);
                    let p = ui.painter();
                    p.text(
                        rect.min + egui::vec2(6.0, 4.0),
                        egui::Align2::LEFT_TOP,
                        crate::value::one_line(&e.sql, 60),
                        egui::FontId::monospace(11.5),
                        if e.ok { theme.text } else { theme.red },
                    );
                    p.text(
                        rect.min + egui::vec2(6.0, 21.0),
                        egui::Align2::LEFT_TOP,
                        format!("{when} · {} ms", e.elapsed_ms),
                        egui::FontId::proportional(10.5),
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
            self.set_text(&sql);
            if run {
                self.run(m, vec![(0, sql)], false);
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
