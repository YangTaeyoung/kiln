//! Staged SELECT result changes. The private backend plan is the only write authority.
use super::*;
use crate::ui::grid::{CellKind, CellView, GridSource, HeaderView};
use crate::{ChangeError, EditableOutcome, ResultEditCell, ResultEditPlan, Value};
use std::collections::BTreeMap;

pub(super) struct SaveResult {
    pub applied: Result<u64, ChangeError>,
    pub refreshed: Option<crate::DbResult<EditableOutcome>>,
}
#[derive(Default)]
pub(super) struct ResultChanges {
    pub(super) edits: BTreeMap<(usize, usize), Value>,
    display: BTreeMap<(usize, usize), String>,
    pub job: Option<Job<SaveResult>>,
    error: Option<String>,
    notice: Option<String>,
}
impl ResultChanges {
    pub fn pending(&self) -> usize {
        self.edits.len()
    }
    pub fn dirty(&self, grid: &GridState) -> bool {
        self.pending() > 0 || grid.editing.is_some() || self.job.is_some()
    }
    fn stage(
        &mut self,
        rs: &ResultSet,
        plan: &ResultEditPlan,
        row: usize,
        col: usize,
        value: Value,
    ) {
        if !plan.can_edit_column(col) || self.job.is_some() {
            return;
        }
        let Some(original) = rs.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        if &value == original {
            self.edits.remove(&(row, col));
            self.display.remove(&(row, col));
        } else {
            self.display
                .insert((row, col), value.display(crate::value::DISPLAY_MAX_CHARS));
            self.edits.insert((row, col), value);
        }
        self.error = None;
        self.notice = None;
    }
    fn stage_text(
        &mut self,
        rs: &ResultSet,
        plan: &ResultEditPlan,
        row: usize,
        col: usize,
        text: &str,
    ) -> bool {
        if !plan.can_edit_column(col) {
            return false;
        }
        match Value::parse_input(text, plan.columns()[col].class) {
            Ok(value) => {
                self.stage(rs, plan, row, col, value);
                true
            }
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }
    pub fn poll(&mut self, run: &mut StmtRun) {
        let Some(result) = self.job.as_mut().and_then(Job::poll) else {
            return;
        };
        self.job = None;
        match result.applied {
            Err(error) => self.error = Some(error.error.to_string()),
            Ok(count) => {
                // Committed snapshots must never be submitted twice, including refresh failure.
                run.editing = None;
                self.edits.clear();
                self.display.clear();
                self.notice = Some(kiln_common::trf!("{count}행 변경을 반영했습니다"));
                match result.refreshed {
                    Some(Ok(refreshed)) => {
                        run.outcome = Ok(refreshed.outcome);
                        run.editing = Some(refreshed.editing);
                    }
                    Some(Err(error)) => {
                        self.error = Some(kiln_common::trf!(
                            "변경은 저장되었지만 결과를 다시 가져오지 못했습니다: {error}"
                        ))
                    }
                    None => {}
                }
            }
        }
    }
}
struct ResultGrid<'a> {
    rs: &'a ResultSet,
    plan: Option<&'a ResultEditPlan>,
    changes: &'a ResultChanges,
    enabled: bool,
}
impl GridSource for ResultGrid<'_> {
    fn n_rows(&self) -> usize {
        self.rs.rows.len()
    }
    fn n_cols(&self) -> usize {
        self.rs.columns.len()
    }
    fn header(&self, c: usize) -> HeaderView<'_> {
        let column = &self.rs.columns[c];
        HeaderView {
            name: &column.name,
            type_name: &column.type_name,
            pk: self.plan.is_some_and(|p| p.key_columns().contains(&c)),
            fk: false,
            sort: None,
        }
    }
    fn cell(&self, r: usize, c: usize) -> CellView<'_> {
        let value = self
            .changes
            .edits
            .get(&(r, c))
            .unwrap_or(&self.rs.rows[r][c]);
        CellView {
            text: self
                .changes
                .display
                .get(&(r, c))
                .map(String::as_str)
                .unwrap_or(&self.rs.display[r][c]),
            kind: if value.is_null() {
                CellKind::Null
            } else {
                CellKind::Value
            },
            edited: self.changes.edits.contains_key(&(r, c)),
            numeric: self.rs.columns[c].class.is_numeric(),
        }
    }
    fn editable(&self) -> bool {
        self.enabled && self.plan.is_some()
    }
    fn editable_column(&self, c: usize) -> bool {
        self.enabled && self.plan.is_some_and(|p| p.can_edit_column(c))
    }
    fn row_operations(&self) -> bool {
        false
    }
    fn default_values(&self) -> bool {
        false
    }
    fn edit_text(&self, r: usize, c: usize) -> String {
        self.changes
            .edits
            .get(&(r, c))
            .unwrap_or(&self.rs.rows[r][c])
            .to_text()
            .unwrap_or_default()
    }
}
pub(super) fn grid(view: &mut ConsoleView, ui: &mut Ui, m: &DbManager) {
    let theme = Theme::current();
    let driver = view.driver;
    let Some(tab) = view.results.get_mut(view.active) else {
        return;
    };
    let Ok(outcome) = &tab.run.outcome else {
        return;
    };
    let rs = &outcome.result;
    let plan = tab.run.editing.as_ref().and_then(|p| p.as_ref().ok());
    let busy = tab.changes.job.is_some();
    let enabled = !busy && view.running.is_none();
    let viewer_dirty = view.viewer.has_draft();
    let mut apply = false;
    let mut revert = false;
    egui::Frame::new()
        .fill(theme.bg_panel)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(plan) = plan {
                    ui::glyph_label(ui, Icon::Table, theme.text_dim, 14.0);
                    ui.label(RichText::new(&plan.table().table).color(theme.text_dim));
                    let pending = tab.changes.pending();
                    let label = if pending > 0 {
                        kiln_common::trf!("변경 {pending}개 반영")
                    } else {
                        kiln_common::i18n::tr("변경 반영").into()
                    };
                    apply = tool_button_icon(
                        ui,
                        Some(Icon::Check),
                        &label,
                        enabled && !viewer_dirty && (pending > 0 || tab.grid.editing.is_some()),
                        true,
                    )
                    .clicked();
                    revert = ui::secondary_button(
                        ui,
                        None,
                        kiln_common::i18n::tr("되돌리기"),
                        !busy && (pending > 0 || tab.grid.editing.is_some() || viewer_dirty),
                    )
                    .clicked();
                    if busy {
                        ui::spinner(ui);
                        ui.label(kiln_common::i18n::tr("변경을 반영하는 중…"));
                    }
                } else {
                    ui::glyph_label(ui, Icon::Lock, theme.text_faint, 14.0);
                    ui.label(kiln_common::i18n::tr("읽기 전용"));
                    if let Some(Err(reason)) = &tab.run.editing {
                        ui.label(
                            RichText::new(reason.to_string())
                                .size(12.0)
                                .color(theme.text_dim),
                        );
                    }
                }
            });
            if let Some(error) = &tab.changes.error {
                ui::banner(ui, error, true);
            }
            if let Some(notice) = &tab.changes.notice {
                ui.colored_label(theme.green, notice);
            }
        });
    if revert {
        view.viewer.discard_draft();
        tab.grid.editing = None;
        tab.changes.edits.clear();
        tab.changes.display.clear();
        tab.changes.error = None;
    }
    if apply && let Some(plan) = plan {
        if let Some(edit) = tab.grid.editing.take()
            && !tab
                .changes
                .stage_text(rs, plan, edit.row, edit.col, &edit.text)
        {
            tab.grid.editing = Some(edit);
        }
        if tab.changes.error.is_none()
            && !tab.changes.edits.is_empty()
            && let Some(session) = view.session.lock().clone()
        {
            let plan = plan.clone();
            let cells = tab
                .changes
                .edits
                .iter()
                .map(|(&(row, column), value)| ResultEditCell {
                    row,
                    column,
                    value: value.clone(),
                })
                .collect::<Vec<_>>();
            tab.changes.job = Some(m.spawn(async move {
                let applied = session.submit_result_edits(&plan, &cells).await;
                let refreshed = if applied.is_ok() {
                    Some(session.refresh_result_edit(&plan, None).await)
                } else {
                    None
                };
                SaveResult { applied, refreshed }
            }));
        }
    }
    let mut viewer_change = None;
    if view.show_viewer {
        let cell = tab
            .grid
            .sel
            .cursor
            .filter(|(r, c)| *r < rs.rows.len() && *c < rs.columns.len());
        egui::Panel::right(view.editor_id.with("viewer"))
            .resizable(true)
            .default_size(300.0)
            .frame(egui::Frame::new().fill(theme.bg_panel).inner_margin(12))
            .show(ui, |ui| {
                let vc = cell.map(|(r, c)| ViewerCell {
                    row: r,
                    col: c,
                    name: &rs.columns[c].name,
                    type_name: &rs.columns[c].type_name,
                    class: plan.map_or(rs.columns[c].class, |p| p.columns()[c].class),
                    value: Some(tab.changes.edits.get(&(r, c)).unwrap_or(&rs.rows[r][c])),
                    editable: enabled && plan.is_some_and(|p| p.can_edit_column(c)),
                });
                viewer_change = view.viewer.ui(ui, vc).zip(cell);
            });
    }
    if let Some((text, (r, c))) = viewer_change
        && let Some(plan) = plan
    {
        if !tab.changes.stage_text(rs, plan, r, c, &text) { view.viewer.preserve_draft(); }
    }
    let source = ResultGrid {
        rs,
        plan,
        changes: &tab.changes,
        enabled: enabled && !apply,
    };
    let events = ui.add_enabled_ui(!view.viewer.has_draft(), |ui| grid_ui(ui, &mut tab.grid, &source)).inner;
    for event in events {
        match event {
            GridEvent::Commit { row, col, text } => {
                if let Some(plan) = plan {
                    if !tab.changes.stage_text(rs, plan, row, col, &text) {
                        tab.grid.editing = Some(crate::ui::grid::EditCell {
                            row,
                            col,
                            text,
                            focus_requested: false,
                        });
                    }
                }
            }
            GridEvent::SetNull => {
                if let Some(plan) = plan {
                    for (row, col) in tab.grid.sel.cells(rs.rows.len(), rs.columns.len()) {
                        if plan.columns()[col].nullable {
                            tab.changes.stage(rs, plan, row, col, Value::Null);
                        } else {
                            tab.changes.error = Some(
                                kiln_common::i18n::tr("NULL을 허용하지 않는 컬럼입니다").into(),
                            );
                        }
                    }
                }
            }
            GridEvent::RevertSelection => {
                for key in tab.grid.sel.cells(rs.rows.len(), rs.columns.len()) {
                    tab.changes.edits.remove(&key);
                    tab.changes.display.remove(&key);
                }
            }
            GridEvent::Copy(fmt, header) => {
                let cols = tab.grid.sel.cols(rs.columns.len());
                let columns = cols.iter().map(|&c| &rs.columns[c]).collect::<Vec<_>>();
                let values = tab.grid.sel.rows(rs.rows.len()).into_iter().map(|r| cols.iter().map(|&c|tab.changes.edits.get(&(r,c)).unwrap_or(&rs.rows[r][c])).collect::<Vec<_>>()).collect::<Vec<_>>();
                ui.ctx().copy_text(crate::export::format_rows(fmt, driver, None, &columns, &values, header));
            }
            GridEvent::ViewValue => view.show_viewer = true,
            _ => {}
        }
    }
}
