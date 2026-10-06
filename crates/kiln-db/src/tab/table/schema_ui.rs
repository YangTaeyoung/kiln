//! Focused, explicitly previewed schema changes. Drafts never execute automatically.
use super::*;
use crate::schema::{ColumnSpec, IndexColumn, IndexSpec, SchemaAction, SchemaPlan, SchemaResult};
use kiln_common::i18n::tr;

// Reserve the action slot without centering short identifiers in the remaining width.
pub(super) fn left_label(ui: &mut Ui, size: egui::Vec2, text: RichText) -> egui::Response {
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_size(size);
        ui.add(egui::Label::new(text).truncate())
    }).inner
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct SchemaFormDraft {
    action: SchemaAction,
    was_applying: bool,
}

pub(super) struct SchemaEditor {
    action: SchemaAction,
    plan: Option<SchemaPlan>,
    prepare: Option<Job<DbResult<SchemaPlan>>>,
    apply: Option<Job<DbResult<SchemaResult>>>,
    error: Option<String>,
    confirm: String,
    reviewed: bool,
}
impl SchemaEditor {
    fn new(action: SchemaAction) -> Self {
        Self {
            action,
            plan: None,
            prepare: None,
            apply: None,
            error: None,
            confirm: String::new(),
            reviewed: false,
        }
    }
    fn busy(&self) -> bool {
        self.prepare.is_some() || self.apply.is_some()
    }
    fn invalidate(&mut self) {
        self.plan = None;
        self.reviewed = false;
        self.error = None;
    }
}

impl TableView {
    pub fn table_ref(&self) -> Option<&TableRef> {
        (!self.dropped).then_some(&self.t)
    }
    pub(super) fn schema_form_draft(&self) -> Option<SchemaFormDraft> {
        self.schema_editor.as_ref().map(|e| SchemaFormDraft {
            action: e.action.clone(),
            was_applying: e.apply.is_some(),
        })
    }
    pub(super) fn restore_schema_form(&mut self, draft: Option<&SchemaFormDraft>) {
        if let Some(draft) = draft {
            let mut editor = SchemaEditor::new(draft.action.clone());
            editor.error = Some(
                tr(if draft.was_applying {
                    "이전 구조 변경의 실행 결과를 확인하세요. 복원된 폼은 자동 실행되지 않습니다."
                } else {
                    "구조 변경 입력을 복원했습니다. SQL을 다시 미리 보고 적용하세요."
                })
                .into(),
            );
            self.schema_editor = Some(editor);
        }
    }
    pub(super) fn watch_schema_epoch(&mut self, m: &DbManager) {
        let epoch = m.connection_epoch(self.conn);
        if self.observed_epoch != epoch {
            self.observed_epoch = epoch;
            self.load_job = None;
            self.count_job = None;
            self.details_job = None;
            self.kind_job = None;
            self.ddl_job = None;
            self.recovery_check = None;
            self.ddl = None;
            if let Some(editor) = &mut self.schema_editor {
                editor.plan = None;
                editor.reviewed = false;
            }
            if self.pending_changes() > 0 {
                self.schema_conflict = true;
                self.recovery_review = true;
                self.status=Some((tr("연결 또는 구조가 변경되었습니다. 초안을 내보내거나 편집을 취소한 뒤 다시 불러오세요.").into(),true));
            } else {
                self.details = None;
                self.data = None;
                self.load_details(m);
                self.reload(m, true);
            }
        }
        if self.schema_conflict && self.pending_changes() == 0 {
            self.schema_conflict = false;
            self.recovery_review = false;
            self.details = None;
            self.data = None;
            self.load_details(m);
            self.reload(m, true);
        }
        if !self.dropped
            && !self.schema_conflict
            && self.sub == TableSection::Ddl
            && self.ddl.is_none()
            && self.ddl_job.is_none()
        {
            self.show_section(TableSection::Ddl, m);
        }
    }

    pub fn schema_has_draft(&self) -> bool {
        self.schema_editor.is_some()
    }
    pub fn show_section(&mut self, section: TableSection, m: &DbManager) {
        self.sub = section;
        if section == TableSection::Ddl && self.ddl.is_none() && self.ddl_job.is_none() {
            let (id, t, m2) = (self.conn, self.t.clone(), m.clone());
            self.ddl_job = Some(m.spawn(async move { m2.table_ddl(id, &t).await }));
        }
    }
    pub fn request_schema_action(&mut self, action: SchemaAction) {
        if self.schema_editor.is_some() || self.dropped {
            return;
        }
        self.schema_editor = Some(SchemaEditor::new(action));
    }
    pub(super) fn poll_schema(&mut self, m: &DbManager) {
        let Some(editor) = &mut self.schema_editor else {
            return;
        };
        if let Some(job) = &mut editor.prepare
            && let Some(result) = job.poll()
        {
            editor.prepare = None;
            match result {
                Ok(plan) => editor.plan = Some(plan),
                Err(e) => editor.error = Some(e.to_string()),
            }
        }
        let result = editor.apply.as_mut().and_then(Job::poll);
        if let Some(result) = result {
            editor.apply = None;
            match result {
                Err(e) => {
                    editor.error = Some(e.to_string());
                    editor.plan = None;
                    editor.reviewed = false;
                }
                Ok(result) => {
                    self.schema_editor = None;
                    self.observed_epoch = m.connection_epoch(self.conn);
                    self.load_job = None;
                    self.count_job = None;
                    self.details_job = None;
                    self.kind_job = None;
                    self.ddl_job = None;
                    self.ddl = None;
                    self.details = None;
                    self.data = None;
                    self.total = None;
                    self.grid.editing = None;
                    if let Some(table) = result.table {
                        self.t = table;
                        self.page = 0;
                        self.filter.clear();
                        self.order.clear();
                        self.applied_filter.clear();
                        self.applied_order.clear();
                        self.status = Some((tr("구조 변경을 DB에 적용했습니다.").into(), false));
                        self.load_details(m);
                        self.reload(m, true);
                        if self.sub == TableSection::Ddl {
                            self.show_section(TableSection::Ddl, m);
                        }
                    } else {
                        self.dropped = true;
                    }
                }
            }
        }
    }
    fn schema_allowed(&self) -> bool {
        !self.is_view
            && self.pending_changes() == 0
            && self.submit_job.is_none()
            && self.kind_job.is_none()
    }
    fn schema_notice(&self, ui: &mut Ui) {
        if self.pending_changes() > 0 {
            ui.label(
                RichText::new(tr(
                    "데이터의 편집 내용을 적용하거나 취소한 뒤 구조를 변경하세요.",
                ))
                .color(Theme::current().yellow),
            );
        }
    }
    pub(super) fn structure_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        let allowed = self.schema_allowed();
        let mut action = None;
        egui::Frame::new().inner_margin(12).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(tr("컬럼"));
                if ui
                    .add_enabled(allowed, egui::Button::new(tr("컬럼 추가…")))
                    .clicked()
                {
                    action = Some(SchemaAction::AddColumn(ColumnSpec {
                        name: String::new(),
                        data_type: "TEXT".into(),
                        nullable: true,
                        default: None,
                    }));
                }
                if ui.button(tr("새로고침")).clicked() {
                    self.load_details(m);
                }
            });
            self.schema_notice(ui);
            let Some(det) = &self.details else {
                if self.details_job.is_some() {
                    ui.spinner();
                } else {
                    ui.label(tr(
                        "구조를 불러오지 못했습니다. 새로고침을 눌러 다시 시도하세요.",
                    ));
                }
                return;
            };
            egui::ScrollArea::vertical()
                .id_salt(self.grid.id.with("schema-columns"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for c in &det.columns {
                        ui.push_id(&c.name, |ui| {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                left_label(
                                    ui,
                                    egui::vec2((ui.available_width() - 80.0).max(60.0), 22.0),
                                    RichText::new(&c.name).font(fonts::mono(13.0)),
                                )
                                .on_hover_text(&c.name);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_enabled_ui(allowed, |ui| {
                                            ui.menu_button(tr("수정"), |ui| {
                                                if ui.button(tr("타입 · 기본값 수정…")).clicked()
                                                {
                                                    action = Some(SchemaAction::AlterColumn {
                                                        column: c.name.clone(),
                                                        spec: ColumnSpec {
                                                            name: c.name.clone(),
                                                            data_type: c.data_type.clone(),
                                                            nullable: c.nullable,
                                                            default: c.default.clone(),
                                                        },
                                                    });
                                                    ui.close();
                                                }
                                                if ui.button(tr("이름 변경…")).clicked() {
                                                    action = Some(SchemaAction::RenameColumn {
                                                        column: c.name.clone(),
                                                        name: c.name.clone(),
                                                    });
                                                    ui.close();
                                                }
                                                if ui
                                                    .button(
                                                        RichText::new(tr("컬럼 삭제…"))
                                                            .color(theme.red),
                                                    )
                                                    .clicked()
                                                {
                                                    action = Some(SchemaAction::DropColumn {
                                                        column: c.name.clone(),
                                                    });
                                                    ui.close();
                                                }
                                            });
                                        });
                                    },
                                );
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&c.data_type)
                                        .font(fonts::mono(12.0))
                                        .color(theme.purple),
                                );
                                if c.is_pk() {
                                    widgets::pill(ui, tr("기본 키"), theme.yellow);
                                }
                                if !c.nullable {
                                    ui.label(RichText::new("NOT NULL").color(theme.text_dim));
                                }
                                if c.auto_increment {
                                    ui.label(tr("자동"));
                                }
                            });
                            if let Some(default) = &c.default {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("DEFAULT {default}"))
                                            .font(fonts::mono(12.0))
                                            .color(theme.text_dim),
                                    )
                                    .wrap(),
                                );
                            }
                            if let Some(fk) = det.fk_target(&c.name) {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("→ {fk}"))
                                            .font(fonts::mono(12.0))
                                            .color(theme.blue),
                                    )
                                    .wrap(),
                                );
                            }
                            ui.add_space(8.0);
                            ui.separator();
                        });
                    }
                    if !det.foreign_keys.is_empty() {
                        ui.add_space(12.0);
                        ui.strong(tr("외래 키"));
                        for fk in &det.foreign_keys {
                            ui.label(format!(
                                "{}: {} → {} ({})",
                                fk.name,
                                fk.columns.join(", "),
                                fk.ref_table,
                                fk.ref_columns.join(", ")
                            ));
                            ui.label(
                                RichText::new(format!(
                                    "ON UPDATE {} · ON DELETE {}",
                                    fk.on_update, fk.on_delete
                                ))
                                .small()
                                .color(theme.text_dim),
                            );
                        }
                    }
                });
        });
        if let Some(action) = action {
            self.request_schema_action(action);
        }
    }
    pub(super) fn indexes_ui(&mut self, ui: &mut Ui, m: &DbManager) {
        let theme = Theme::current();
        let allowed = self.schema_allowed();
        let mut action = None;
        egui::Frame::new().inner_margin(12).show(ui,|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.strong(tr("인덱스"));
                if ui.add_enabled(allowed,egui::Button::new(tr("인덱스 추가…"))).clicked(){action=Some(SchemaAction::AddIndex(IndexSpec{name:String::new(),columns:Vec::new(),unique:false}));}
                if ui.button(tr("새로고침")).clicked(){self.load_details(m);}
            });
            self.schema_notice(ui);
            let Some(det)=&self.details else {if self.details_job.is_some(){ui.spinner();}else{ui.label(tr("구조를 불러오지 못했습니다. 새로고침을 눌러 다시 시도하세요."));}return;};
            egui::ScrollArea::vertical().id_salt(self.grid.id.with("schema-indexes")).auto_shrink([false,false]).show(ui,|ui|{
                if det.indexes.is_empty(){ui.add_space(12.0);ui.label(tr("이 테이블에는 인덱스가 없습니다."));}
                for ix in &det.indexes {ui.push_id(&ix.name,|ui|{
                    ui.add_space(10.0);
                    ui.horizontal(|ui|{
                        left_label(ui,egui::vec2((ui.available_width()-80.0).max(60.0),22.0),RichText::new(&ix.name).font(fonts::mono(13.0))).on_hover_text(&ix.name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                            if ui.add_enabled(allowed&&!ix.primary&&!ix.constraint,egui::Button::new(tr("삭제…"))).on_hover_text(if ix.primary||ix.constraint{tr("제약 조건이 소유한 인덱스는 여기서 삭제할 수 없습니다.")}else{tr("SQL을 확인한 뒤 인덱스를 삭제합니다.")}).clicked(){action=Some(SchemaAction::DropIndex{name:ix.name.clone()});}
                        });
                    });
                    ui.horizontal_wrapped(|ui|{
                        if ix.primary{widgets::pill(ui,tr("기본 키"),theme.yellow);}else if ix.unique{widgets::pill(ui,tr("고유"),theme.blue);}
                        if ix.constraint{ui.label(tr("제약 조건"));}
                        if !ix.valid{ui.label(RichText::new(tr("유효하지 않음")).color(theme.red));}
                        if !ix.method.is_empty(){ui.label(RichText::new(&ix.method).color(theme.text_dim));}
                    });
                    ui.label(RichText::new(ix.columns.join(", ")).font(fonts::mono(12.0)));
                    if !ix.included_columns.is_empty(){ui.label(format!("INCLUDE ({})",ix.included_columns.join(", ")));}
                    if let Some(predicate)=&ix.predicate {ui.label(RichText::new(format!("WHERE {predicate}")).font(fonts::mono(12.0)));}
                    ui.collapsing(tr("인덱스 DDL"),|ui|{if ix.definition.is_empty(){ui.label(tr("이 인덱스에 표시할 DDL이 없습니다."));}else{ui.add(egui::Label::new(RichText::new(&ix.definition).font(fonts::mono(12.0))).wrap().selectable(true));}});
                    ui.add_space(8.0);ui.separator();
                });}
            });
        });
        if let Some(action) = action {
            self.request_schema_action(action);
        }
    }
    pub(super) fn schema_dialog(&mut self, ui: &mut Ui, m: &DbManager) {
        let allowed = self.schema_allowed();
        let Some(mut editor) = self.schema_editor.take() else {
            return;
        };
        if editor
            .plan
            .as_ref()
            .is_some_and(|p| p.epoch != m.connection_epoch(self.conn))
        {
            editor.invalidate();
            editor.error = Some(tr("연결이 변경되었습니다. SQL을 다시 미리 보세요.").into());
        }
        if editor.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(60));
        }
        let mut close = false;
        let viewport = ui.ctx().content_rect();
        let modal = egui::Modal::new(self.grid.id.with("schema-edit")).show(ui.ctx(), |ui| {
            ui.style_mut().spacing.scroll.floating = false;
            ui.set_width((viewport.width() - 48.0).clamp(180.0, 600.0));
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.strong(action_title(&editor.action));
            let connection = m
                .get(self.conn)
                .map(|c| c.display_name())
                .unwrap_or_else(|| self.driver.label().to_string());
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{connection} · {}", self.t.sql_name(self.driver)))
                        .font(fonts::mono(12.0))
                        .color(Theme::current().text_dim),
                )
                .truncate(),
            )
            .on_hover_text(format!("{connection} · {}", self.t.sql_name(self.driver)));
            let busy = editor.busy();
            egui::ScrollArea::vertical()
                .id_salt("schema-form-body")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .max_height((viewport.height() - 210.0).max(65.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if !allowed {
                        ui.label(
                            RichText::new(tr(
                                "데이터의 편집 내용을 적용하거나 취소한 뒤 구조를 변경하세요.",
                            ))
                            .color(Theme::current().yellow),
                        );
                    }
                    let mut changed = false;
                    ui.add_enabled_ui(!busy, |ui| {
                        changed = action_fields(ui, &mut editor.action, self.details.as_ref());
                    });
                    if changed {
                        editor.invalidate();
                    }
                    if matches!(editor.action, SchemaAction::DropTable) {
                        ui.label(tr(
                            "이 테이블과 모든 데이터가 영구 삭제됩니다. 테이블 이름을 입력하세요.",
                        ));
                        let confirmation_label = ui.label(tr("테이블 이름"));
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.confirm)
                                .desired_width(f32::INFINITY)
                                .hint_text(&self.t.table),
                        )
                        .labelled_by(confirmation_label.id);
                    }
                    if let Some(plan) = &editor.plan {
                        if let Some(warning) = &plan.warning {
                            ui.label(RichText::new(warning).color(Theme::current().yellow));
                        }
                        ui.separator();
                        ui.strong(tr("실행할 SQL"));
                        ui.add(
                            egui::Label::new(
                                RichText::new(plan.sql.join(";\n")).font(fonts::mono(12.0)),
                            )
                            .wrap()
                            .selectable(true),
                        );
                        ui.checkbox(&mut editor.reviewed, tr("SQL과 변경 대상을 확인했습니다."));
                    }
                });
            if let Some(error) = &editor.error {
                egui::ScrollArea::vertical()
                    .id_salt("schema-error")
                    .max_height(48.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(error).color(Theme::current().red));
                    });
            }
            if editor.apply.is_some() {
                ui.label(tr("DB에 적용 중… 탭을 닫아도 실행은 취소되지 않습니다."));
            } else if editor.prepare.is_some() {
                ui.label(tr("SQL과 현재 구조를 확인 중…"));
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("취소")))
                    .clicked()
                {
                    close = true;
                }
                if ui
                    .add_enabled(allowed && !busy, egui::Button::new(tr("SQL 미리보기")))
                    .clicked()
                {
                    editor.invalidate();
                    let (id, t, action, m2) =
                        (self.conn, self.t.clone(), editor.action.clone(), m.clone());
                    editor.prepare = Some(
                        m.spawn(async move { m2.prepare_schema_change(id, &t, action).await }),
                    );
                }
                let confirmed = !matches!(editor.action, SchemaAction::DropTable)
                    || editor.confirm == self.t.table;
                if ui
                    .add_enabled(
                        allowed && !busy && editor.plan.is_some() && editor.reviewed && confirmed,
                        egui::Button::new(tr("DB에 적용")),
                    )
                    .clicked()
                {
                    let (id, plan, m2) = (self.conn, editor.plan.take().unwrap(), m.clone());
                    editor.apply =
                        Some(m.spawn(async move { m2.apply_schema_change(id, plan).await }));
                }
            });
        });
        if modal.should_close() && !editor.busy() {
            close = true;
        }
        if !close {
            self.schema_editor = Some(editor);
        }
    }
}
fn action_title(action: &SchemaAction) -> &str {
    match action {
        SchemaAction::RenameTable { .. } => tr("테이블 이름 변경"),
        SchemaAction::DropTable => tr("테이블 삭제"),
        SchemaAction::AddColumn(_) => tr("컬럼 추가"),
        SchemaAction::AlterColumn { .. } => tr("컬럼 수정"),
        SchemaAction::RenameColumn { .. } => tr("컬럼 이름 변경"),
        SchemaAction::DropColumn { .. } => tr("컬럼 삭제"),
        SchemaAction::AddIndex(_) => tr("인덱스 추가"),
        SchemaAction::DropIndex { .. } => tr("인덱스 삭제"),
    }
}
fn field(ui: &mut Ui, label: &str, value: &mut String) -> bool {
    let label = ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
        .labelled_by(label.id)
        .changed()
}
fn action_fields(ui: &mut Ui, action: &mut SchemaAction, details: Option<&TableDetails>) -> bool {
    let mut changed = false;
    let altering = matches!(action, SchemaAction::AlterColumn { .. });
    match action {
        SchemaAction::RenameTable { name } => {
            changed |= field(ui, tr("새 이름"), name);
        }
        SchemaAction::RenameColumn { column, name } => {
            ui.label(tr("컬럼 이름"));
            ui.add(egui::Label::new(RichText::new(column.as_str()).font(fonts::mono(13.0))).wrap());
            changed |= field(ui, tr("새 이름"), name);
        }
        SchemaAction::AddColumn(spec) | SchemaAction::AlterColumn { spec, .. } => {
            if altering {
                ui.label(tr("컬럼 이름"));
                ui.label(RichText::new(&spec.name).font(fonts::mono(13.0)));
            } else {
                changed |= field(ui, tr("컬럼 이름"), &mut spec.name);
            }
            changed |= field(ui, tr("데이터 타입"), &mut spec.data_type);
            changed |= ui.checkbox(&mut spec.nullable, tr("NULL 허용")).changed();
            let mut has_default = spec.default.is_some();
            if ui
                .checkbox(&mut has_default, tr("기본값 지정 (SQL 표현식)"))
                .changed()
            {
                spec.default = has_default.then(String::new);
                changed = true;
            }
            if let Some(default) = &mut spec.default {
                changed |= field(ui, tr("기본값"), default);
                ui.label(
                    RichText::new(tr(
                        "문자열은 'text', 현재 시각은 CURRENT_TIMESTAMP처럼 SQL로 입력하세요.",
                    ))
                    .small()
                    .color(Theme::current().text_dim),
                );
            }
        }
        SchemaAction::AddIndex(spec) => {
            changed |= field(ui, tr("인덱스 이름"), &mut spec.name);
            changed |= ui.checkbox(&mut spec.unique, tr("고유 인덱스")).changed();
            ui.label(tr("컬럼을 선택한 순서대로 인덱스가 생성됩니다."));
            if let Some(details) = details {
                egui::ComboBox::from_id_salt("add-index-column")
                    .selected_text(tr("컬럼 추가"))
                    .width(ui.available_width().min(280.0))
                    .show_ui(ui, |ui| {
                        for c in &details.columns {
                            if !spec.columns.iter().any(|x| x.name == c.name)
                                && ui.selectable_label(false, &c.name).clicked()
                            {
                                spec.columns.push(IndexColumn {
                                    name: c.name.clone(),
                                    descending: false,
                                });
                                changed = true;
                            }
                        }
                    });
            }
            let mut movement = None;
            let mut remove = None;
            let len = spec.columns.len();
            for (i, c) in spec.columns.iter_mut().enumerate() {
                ui.push_id(i, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}. {}", i + 1, c.name))
                                    .font(fonts::mono(12.0)),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&c.name);
                        changed |= ui.checkbox(&mut c.descending, tr("내림차순")).changed();
                        if ui
                            .add_enabled(i > 0, egui::Button::new(tr("위로")))
                            .clicked()
                        {
                            movement = Some((i, i - 1));
                        }
                        if ui
                            .add_enabled(i + 1 < len, egui::Button::new(tr("아래로")))
                            .clicked()
                        {
                            movement = Some((i, i + 1));
                        }
                        if ui.button(tr("제거")).clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some((a, b)) = movement {
                spec.columns.swap(a, b);
                changed = true;
            }
            if let Some(i) = remove {
                spec.columns.remove(i);
                changed = true;
            }
        }
        SchemaAction::DropColumn { column } => {
            ui.label(RichText::new(column.as_str()).font(fonts::mono(13.0)));
            ui.label(tr("이 컬럼의 데이터가 영구 삭제됩니다."));
        }
        SchemaAction::DropIndex { name } => {
            ui.label(RichText::new(name.as_str()).font(fonts::mono(13.0)));
            ui.label(tr("인덱스를 삭제합니다. 테이블의 행은 유지됩니다."));
        }
        SchemaAction::DropTable => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_a_schema_draft_invalidates_review_and_error() {
        let mut editor = SchemaEditor::new(SchemaAction::RenameTable {
            name: "renamed".into(),
        });
        editor.reviewed = true;
        editor.error = Some("previous failure".into());
        editor.invalidate();
        assert!(!editor.reviewed);
        assert!(editor.error.is_none());
        assert!(editor.plan.is_none());
    }
    #[test]
    fn opening_a_destructive_form_does_not_execute_it() {
        let m = DbManager::in_memory();
        let mut table = TableView::new(&m, ConnId(1), None, "items".into());
        table.request_schema_action(SchemaAction::DropTable);
        assert!(table.schema_has_draft());
        assert!(!table.dropped);
        assert_eq!(table.table_name(), "items");
    }
    #[test]
    fn uncommitted_cell_text_blocks_schema_change_and_survives_form_open() {
        let m = DbManager::in_memory();
        let mut table = TableView::new(&m, ConnId(1), None, "items".into());
        table.kind_job = None;
        table.data = Some(TableData::new(ResultSet::new(
            vec![ColumnInfo::new("id", "INTEGER")],
            vec![vec![Value::Int(1)]],
        )));
        table.grid.editing = Some(crate::ui::grid::EditCell {
            row: 0,
            col: 0,
            text: "unfinished".into(),
            focus_requested: false,
        });
        let draft = table.recovery_draft().unwrap();
        assert!(!table.schema_allowed());
        table.request_schema_action(SchemaAction::DropTable);
        assert!(!table.schema_allowed());
        let mut after = table.recovery_draft().unwrap();
        assert!(after.schema_form.is_some());
        after.schema_form = None;
        assert_eq!(after, draft);
        assert!(table.schema_editor.as_ref().unwrap().prepare.is_none());
        assert!(table.schema_editor.as_ref().unwrap().apply.is_none());
    }
    #[test]
    fn schema_only_draft_roundtrip_restores_inputs_without_sql_or_confirmation() {
        let m = DbManager::in_memory();
        let mut table = TableView::new(&m, ConnId(1), None, "items".into());
        table.request_schema_action(SchemaAction::DropTable);
        let editor = table.schema_editor.as_mut().unwrap();
        editor.confirm = "items".into();
        editor.reviewed = true;
        let draft = table.recovery_draft().unwrap();
        assert!(draft.rows.is_empty());
        let draft: TableDraft =
            serde_json::from_slice(&serde_json::to_vec(&draft).unwrap()).unwrap();
        let mut restored = TableView::new(&m, ConnId(1), None, "items".into());
        restored.restore_draft(&draft);
        let editor = restored.schema_editor.as_ref().unwrap();
        assert!(matches!(editor.action, SchemaAction::DropTable));
        assert!(editor.confirm.is_empty());
        assert!(!editor.reviewed);
        assert!(editor.plan.is_none());
        assert!(!editor.busy());
        assert!(restored.load_job.is_some());
        assert!(restored.recovery_draft().is_some());
    }
    #[test]
    fn connection_epoch_change_preserves_row_draft_and_blocks_submit() {
        let m = DbManager::in_memory();
        let mut table = TableView::new(&m, ConnId(1), None, "items".into());
        table.data = Some(TableData::new(ResultSet::new(
            vec![ColumnInfo::new("id", "INTEGER")],
            vec![vec![Value::Int(1)]],
        )));
        table.grid.editing = Some(crate::ui::grid::EditCell {
            row: 0,
            col: 0,
            text: "2".into(),
            focus_requested: false,
        });
        let pending = table.pending_cell();
        m.schema_changed(ConnId(1));
        table.watch_schema_epoch(&m);
        assert!(table.schema_conflict);
        assert_eq!(table.pending_cell(), pending);
        table.submit(&m);
        assert!(table.submit_job.is_none());
        assert_eq!(table.pending_cell(), pending);
        let draft = table.recovery_draft().unwrap();
        assert!(draft.schema_conflict);
        table.grid.editing = None;
        table.watch_schema_epoch(&m);
        assert!(!table.schema_conflict);
        assert!(table.load_job.is_some());
    }
    #[test]
    fn schema_forms_keep_actions_inside_minimum_viewport_in_all_locales() {
        use egui_kittest::{Harness, kittest::Queryable};
        use kiln_common::i18n::{Language, with_language};
        struct RestoreTheme(String);
        impl Drop for RestoreTheme {
            fn drop(&mut self) {
                Theme::set_current(&self.0);
            }
        }
        let _restore = RestoreTheme(Theme::current().name.to_string());
        for theme in ["kiln-dark", "kiln-light"] {
            Theme::set_current(theme);
            for language in Language::ALL {
                with_language(language, || {
                    for width in [420.0, 720.0] {
                        let m = DbManager::in_memory();
                        let mut table = TableView::new(
                            &m,
                            ConnId(1),
                            None,
                            "customer_delivery_addresses_with_extended_metadata".into(),
                        );
                        table.kind_job = None;
                        table.load_job = None;
                        table.details_job = None;
                        table.count_job = None;
                        table.request_schema_action(SchemaAction::AddColumn(ColumnSpec {
                            name: "delivery_instructions".into(),
                            data_type: "VARCHAR(255)".into(),
                            nullable: false,
                            default: Some("'Leave the package at the reception desk'".into()),
                        }));
                        table.schema_editor.as_mut().unwrap().error = Some(
                            tr("구조를 불러오지 못했습니다. 새로고침을 눌러 다시 시도하세요.")
                                .into(),
                        );
                        let mut initialized = false;
                        let mut harness = Harness::builder()
                            .with_size([width / 1.3, 440.0 / 1.3])
                            .with_pixels_per_point(1.3)
                            .wgpu()
                            .build_ui_state(
                                |ui, table: &mut TableView| {
                                    if !initialized {
                                        fonts::install(ui.ctx());
                                        Theme::current().apply(ui.ctx());
                                        initialized = true;
                                        return;
                                    }
                                    table.schema_dialog(ui, &m);
                                },
                                table,
                            );
                        harness.run_steps(5);
                        for key in ["취소", "SQL 미리보기", "DB에 적용"] {
                            let r = harness.get_by_label(tr(key)).rect();
                            assert!(
                                harness.ctx.content_rect().contains_rect(r),
                                "{theme} {} {width}: {key} {r:?}",
                                language.code()
                            );
                        }
                        harness
                            .render()
                            .unwrap()
                            .save(format!(
                                "/tmp/kiln-schema-form-{}-{theme}-{}.png",
                                language.code(),
                                width as u32
                            ))
                            .unwrap();
                    }
                });
            }
        }
    }
    #[test]
    fn narrow_schema_inspectors_show_actions_and_long_metadata() {
        use egui_kittest::{Harness, kittest::Queryable};
        let previous = Theme::current().name;
        for theme in ["kiln-dark", "kiln-light"] {
            Theme::set_current(theme);
            for section in [TableSection::Structure, TableSection::Indexes] {
                let m = DbManager::in_memory();
                let mut table =
                    TableView::new(&m, ConnId(1), None, "customer_delivery_addresses".into());
                table.kind_job = None;
                table.load_job = None;
                table.details_job = None;
                table.count_job = None;
                table.sub = section;
                table.details=Some(TableDetails{columns:vec![crate::ColumnDef{name:"delivery_instructions_long_column_name".into(),data_type:"VARCHAR(255)".into(),cast_type:"varchar".into(),nullable:false,default:Some("'Leave the package at the reception desk'".into()),pk_ordinal:0,auto_increment:false,class:TypeClass::Text}],indexes:vec![crate::IndexInfo{name:"idx_customer_delivery_instructions_long_index_name".into(),columns:vec!["customer_id ASC".into(),"created_at DESC".into()],unique:true,primary:false,definition:"CREATE UNIQUE INDEX idx_customer_delivery_instructions ON customer_delivery_addresses (customer_id, created_at DESC) WHERE deleted_at IS NULL".into(),method:"btree".into(),predicate:Some("deleted_at IS NULL".into()),included_columns:vec!["delivery_instructions".into()],constraint:false,valid:true}],foreign_keys:vec![]});
                let mut initialized = false;
                let mut harness = Harness::builder()
                    .with_size([420.0 / 1.3, 600.0 / 1.3])
                    .with_pixels_per_point(1.3)
                    .wgpu()
                    .build_ui_state(
                        |ui, table: &mut TableView| {
                            if !initialized {
                                fonts::install(ui.ctx());
                                Theme::current().apply(ui.ctx());
                                initialized = true;
                                return;
                            }
                            table.ui(ui, &m);
                        },
                        table,
                    );
                harness.run_steps(5);
                let action = if section == TableSection::Structure {
                    "컬럼 추가…"
                } else {
                    "인덱스 추가…"
                };
                assert!(
                    harness
                        .ctx
                        .content_rect()
                        .contains_rect(harness.get_by_label(tr(action)).rect())
                );
                harness
                    .render()
                    .unwrap()
                    .save(format!(
                        "/tmp/kiln-schema-inspector-{section:?}-{theme}-420.png"
                    ))
                    .unwrap();
            }
        }
        Theme::set_current(previous);
    }
}
