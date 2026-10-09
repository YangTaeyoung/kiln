//! Persist review data, never session-bound write authority or runnable UPDATEs.
use super::*;
use crate::{ColumnInfo, Value};
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResultDraft {
    pub sql: String,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub source_columns: Vec<String>,
    pub key_columns: Vec<usize>,
    pub original_rows: Vec<Vec<Value>>,
    pub edits: Vec<(usize, usize, Value)>,
    pub pending_cell: Option<(usize, usize, String)>,
    pub viewer_cell: Option<(usize, usize, String)>,
    /// Abrupt recovery during submission cannot establish whether the DB committed.
    #[serde(default)]
    pub was_applying: bool,
}
impl ConsoleView {
    pub(super) fn result_drafts(&self) -> Vec<ResultDraft> {
        let mut drafts = self.recovered_results.clone();
        for (index, tab) in self.results.iter().enumerate() {
            let viewer = if index == self.active {
                self.viewer.draft()
            } else {
                None
            };
            if !tab.changes.dirty(&tab.grid) && viewer.is_none() {
                continue;
            }
            let Ok(outcome) = &tab.run.outcome else {
                continue;
            };
            let plan = tab.run.editing.as_ref().and_then(|p| p.as_ref().ok());
            drafts.push(ResultDraft {
                sql: tab.run.sql.clone(),
                schema: plan.and_then(|p| p.table().schema.clone()),
                table: plan.map(|p| p.table().table.clone()),
                columns: outcome.result.columns.clone(),
                source_columns: plan
                    .map(|p| p.columns().iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default(),
                key_columns: plan.map(|p| p.key_columns().to_vec()).unwrap_or_default(),
                original_rows: outcome.result.rows.clone(),
                edits: tab
                    .changes
                    .edits
                    .iter()
                    .map(|(&(r, c), v)| (r, c, v.clone()))
                    .collect(),
                pending_cell: tab
                    .grid
                    .editing
                    .as_ref()
                    .map(|e| (e.row, e.col, e.text.clone())),
                viewer_cell: viewer,
                was_applying: tab.changes.job.is_some(),
            });
        }
        drafts
    }
}
pub(super) fn ui(view: &mut ConsoleView, ui: &mut Ui) {
    if view.recovered_results.is_empty() {
        return;
    }
    let mut remove = None;
    egui::CollapsingHeader::new(kiln_common::i18n::tr("복원된 결과 변경 · 검토 전용")).id_salt(view.editor_id.with("result-recovery")).default_open(true).show(ui,|ui|{
        ui.label(kiln_common::i18n::tr("최신 결과를 다시 조회해 비교하세요. 복원된 변경은 자동 제출되지 않습니다."));
        egui::ScrollArea::vertical().id_salt(view.editor_id.with("result-recovery-scroll")).max_height(180.0).show(ui,|ui|{
            for (i,draft) in view.recovered_results.iter().enumerate(){ui.push_id(i,|ui|{
                ui.horizontal_wrapped(|ui|{
                    ui.strong(draft.table.as_deref().unwrap_or(kiln_common::i18n::tr("결과")));
                    if ui.button(kiln_common::i18n::tr("쿼리 복사")).clicked(){ui.ctx().copy_text(draft.sql.clone());}
                    if ui.button(kiln_common::i18n::tr("초안 복사")).clicked(){if let Ok(text)=serde_json::to_string_pretty(draft){ui.ctx().copy_text(text);}}
                    if ui.button(kiln_common::i18n::tr("초안 내보내기")).clicked(){
                        if let Some(path)=rfd::FileDialog::new().set_file_name("result-draft.json").add_filter("JSON",&["json"]).save_file(){
                            match serde_json::to_vec_pretty(draft).map_err(|e|e.to_string()).and_then(|data|kiln_common::safe_file::write(&path,&data).map_err(|e|e.to_string())){Ok(())=>{},Err(error)=>view.document_error=Some(error)}
                        }
                    }
                    if view.discard_recovered==Some(i){
                        if ui.button(kiln_common::i18n::tr("초안 삭제 확인")).clicked(){remove=Some(i);}
                        if ui.button(kiln_common::i18n::tr("취소")).clicked(){view.discard_recovered=None;}
                    }else if ui.button(kiln_common::i18n::tr("초안 삭제")).clicked(){view.discard_recovered=Some(i);}
                });
                if draft.was_applying {ui.colored_label(Theme::current().yellow,kiln_common::i18n::tr("제출 중 종료된 변경입니다. DB 반영 여부를 먼저 확인하세요."));}
                for (row,col,text) in draft.edits.iter().map(|(r,c,v)|(*r,*c,v.display(crate::value::DISPLAY_MAX_CHARS))).chain(draft.pending_cell.iter().cloned()).chain(draft.viewer_cell.iter().cloned()) {
                    let name=draft.columns.get(col).map(|c|c.name.as_str()).unwrap_or("?");
                    let old=draft.original_rows.get(row).and_then(|r|r.get(col)).map(|v|v.display(crate::value::DISPLAY_MAX_CHARS)).unwrap_or_default();
                    ui.label(kiln_common::trf!("{name} · {}행",row+1));
                    ui.add(egui::Label::new(egui::RichText::new(format!("{old} → {text}")).monospace()).truncate()).on_hover_text(format!("{old}\n→\n{text}"));
                }
                ui.separator();
            });}
        });
    });
    if let Some(i) = remove {
        view.recovered_results.remove(i);
        view.discard_recovered = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_roundtrip_preserves_raw_edits_and_pending_text_without_write_authority() {
        let manager = DbManager::in_memory();
        let mut view = ConsoleView::new(&manager, ConnId(1));
        view.push_result(
            "select id, value from example",
            ResultSet::new(
                vec![
                    ColumnInfo::new("id", "INTEGER"),
                    ColumnInfo::new("value", "TEXT"),
                ],
                vec![vec![Value::Int(7), Value::Text("before".into())]],
            ),
        );
        view.results[0]
            .changes
            .edits
            .insert((0, 1), Value::Text("after".into()));
        view.results[0].grid.editing = Some(crate::ui::grid::EditCell {
            row: 0,
            col: 1,
            text: "unfinished 🦊".into(),
            focus_requested: false,
        });
        let document = view.document();
        assert_eq!(document.result_drafts.len(), 1);
        let encoded = serde_json::to_string(&document).unwrap();
        assert!(!encoded.contains("generation"));
        assert!(!encoded.contains("session"));
        let document: ConsoleDocument = serde_json::from_str(&encoded).unwrap();
        let mut restored = ConsoleView::new(&manager, ConnId(1));
        restored.restore_document(&document);
        assert!(restored.results.is_empty());
        assert!(!restored.is_running());
        assert!(!restored.is_applying());
        assert!(restored.result_changes_dirty());
        assert_eq!(restored.document(), document);
        let legacy: ConsoleDocument =
            serde_json::from_str(r#"{"path":null,"text":"select 1","saved_text":null}"#).unwrap();
        assert!(legacy.result_drafts.is_empty());
    }
    #[test]
    fn hidden_console_polls_real_sqlite_apply_and_releases_close_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hidden.db"), b"").unwrap();
        let manager = DbManager::in_memory();
        let id = manager.add(
            crate::ConnConfig {
                driver: Driver::Sqlite,
                file: dir.path().join("hidden.db").to_string_lossy().into_owned(),
                ..Default::default()
            },
            None,
        );
        manager
            .block_on(manager.query(
                id,
                "CREATE TABLE sample (id INTEGER PRIMARY KEY, value TEXT)",
                None,
            ))
            .unwrap();
        manager
            .block_on(manager.query(id, "INSERT INTO sample VALUES (1,'before')", None))
            .unwrap();
        let mut view = ConsoleView::new(&manager, id);
        view.run(
            &manager,
            vec![(0, "SELECT id,value FROM sample".into())],
            false,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while view.is_running() {
            view.poll_background(&manager);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let plan = view.results[0]
            .run
            .editing
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone();
        let session = view.session.lock().clone().unwrap();
        view.results[0]
            .changes
            .edits
            .insert((0, 1), Value::Text("after".into()));
        view.results[0].changes.job = Some(manager.spawn(async move {
            let applied = session
                .submit_result_edits(
                    &plan,
                    &[crate::ResultEditCell {
                        row: 0,
                        column: 1,
                        value: Value::Text("after".into()),
                    }],
                )
                .await;
            let refreshed = if applied.is_ok() {
                Some(session.refresh_result_edit(&plan, None).await)
            } else {
                None
            };
            result_ui::SaveResult { applied, refreshed }
        }));
        assert!(view.is_applying());
        while view.is_applying() {
            view.poll_background(&manager);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!view.live_result_changes_dirty());
        assert_eq!(
            manager
                .block_on(manager.query(id, "SELECT value FROM sample", None))
                .unwrap()
                .result
                .rows[0][0],
            Value::Text("after".into())
        );
    }
}
