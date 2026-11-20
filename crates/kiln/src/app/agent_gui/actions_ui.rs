//! Explicit typed actions, kept separate from insert-only commands.
use super::*;
use kiln_common::i18n::tr;
#[derive(Default)]
pub(super) struct ActionForm {
    objective: String,
    budget: Option<u64>,
    status: Option<GoalStatus>,
    queue_text: String,
    queue_edits: BTreeMap<String, String>,
    review_kind: u8,
    review_text: String,
    turn_control: String,
    turn_value: Option<ControlValue>,
}
pub(super) fn capability_label(id: &str) -> String {
    tr(match id {
        "thread/goal/get" => "목표 상태",
        "thread/goal/set" => "목표 설정",
        "thread/goal/clear" => "목표 지우기",
        "thread/queue/list" => "대기 작업",
        "thread/queue/add" => "대기 작업 추가",
        "thread/queue/update" => "대기 작업 편집",
        "thread/queue/delete" => "대기 작업 삭제",
        "thread/queue/start" => "대기 작업 시작",
        "review/start" => "검토 시작",
        "thread/compact/start" => "대화 압축",
        "turn/settings/update" => "현재 턴 설정",
        "thread/memory/read" => "메모리 상태",
        "thread/memoryMode/set" => "메모리 모드",
        "config/mcpServer/reload" => "MCP 다시 로드",
        "model/list" => "모델 목록",
        "thread/settings/update" => "대화 설정",
        "collaborationMode/list" => "협업 모드",
        "permissionProfile/list" => "권한 프로필",
        "configRequirements/read" => "정책 요구사항",
        "config/read" => "설정 출처",
        "skills/list" => "스킬 목록",
        "mcpServerStatus/list" => "MCP 상태",
        "thread/read" => "대화 상태",
        "experimentalFeature/list" => "실험 기능",
        _ => "네이티브 전용 기능",
    })
    .into()
}
impl AgentPane {
    pub(super) fn action_available(&self, id: &str) -> bool {
        self.transcript
            .controls
            .capabilities
            .iter()
            .any(|c| c.id == id && c.available)
    }
    pub(super) fn native_action(&mut self, action: NativeAction) {
        if self.composing || self.ime_frame || self.settings_blocked() || !self.transport_alive {
            return;
        }
        if let Err(e) = self.send(Command::NativeAction(action)) {
            self.error = Some(e);
        }
    }
    pub(super) fn actions_ui(&mut self, ui: &mut Ui) {
        let ready = self.editing_allowed();
        let queue_editable = self.transport_alive
            && matches!(
                self.transcript.lifecycle,
                Lifecycle::Ready | Lifecycle::Running | Lifecycle::Waiting
            )
            && !self.settings_blocked()
            && self.pending.is_none()
            && !self.composing
            && !self.ime_frame;
        ui.small(tr("명시적으로 실행하며 현재 대화에 적용됩니다."));
        if self.descriptor.tool == Tool::Claude {
            ui.add_enabled_ui(ready, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (title, action) in [
                        ("스킬 다시 로드", NativeAction::ReloadSkills),
                        ("플러그인 다시 로드", NativeAction::ReloadPlugins),
                        ("스타일 다시 로드", NativeAction::ReloadStyles),
                    ] {
                        if ui.button(tr(title)).clicked() {
                            self.native_action(action);
                        }
                    }
                });
            });
            ui.small(tr(
                "플러그인 다시 로드는 네이티브 지원과 정책 확인이 필요합니다.",
            ));
            return;
        }
        if self.descriptor.tool != Tool::Codex {
            ui.label(tr("명령 목록에서 제공된 작업을 삽입하세요."));
            return;
        }
        if self.transcript.controls.capabilities.is_empty() {
            ui.label(tr("네이티브 기능을 아직 받지 못했습니다."));
            return;
        }
        if self.action_available("thread/compact/start")
            && ui
                .add_enabled(ready, egui::Button::new(tr("대화 압축")))
                .clicked()
        {
            self.native_action(NativeAction::Compact);
        }
        if self.action_available("thread/goal/set") {
            egui::CollapsingHeader::new(tr("목표")).show(ui, |ui| {
                if ui.button(tr("목표 상태 새로 고침")).clicked() {
                    self.discover(Discovery::Goal);
                }
                named_text(ui, &mut self.controls_ui.actions.objective, "목표 내용");
                let form = &mut self.controls_ui.actions;
                let mut budget = form.budget.is_some();
                if ui.checkbox(&mut budget, tr("토큰 예산 지정")).changed() {
                    form.budget = budget.then_some(1000);
                }
                if let Some(budget) = &mut form.budget {
                    ui.add(egui::DragValue::new(budget).range(1..=i64::MAX as u64));
                }
                egui::ComboBox::from_id_salt("goal-status")
                    .selected_text(goal_label(form.status.as_ref()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut form.status, None, tr("상태 유지"));
                        for status in [
                            GoalStatus::Active,
                            GoalStatus::Paused,
                            GoalStatus::Complete,
                            GoalStatus::Blocked,
                            GoalStatus::UsageLimited,
                            GoalStatus::BudgetLimited,
                        ] {
                            let name = goal_label(Some(&status));
                            ui.selectable_value(&mut form.status, Some(status), name);
                        }
                    });
                let valid = !form.objective.trim().is_empty()
                    || form.status.is_some()
                    || form.budget.is_some();
                let action = CodexAction::SetGoal {
                    objective: (!form.objective.trim().is_empty()).then(|| form.objective.clone()),
                    status: form.status.clone(),
                    token_budget: form.budget,
                };
                if ui
                    .add_enabled(ready && valid, egui::Button::new(tr("목표 설정")))
                    .clicked()
                {
                    self.native_action(NativeAction::Codex(action));
                }
                if ui
                    .add_enabled(ready, egui::Button::new(tr("목표 지우기")))
                    .clicked()
                {
                    self.native_action(NativeAction::Codex(CodexAction::ClearGoal));
                }
            });
        }
        if self.action_available("thread/queue/add") {
            egui::CollapsingHeader::new(tr("대기 작업")).show(ui, |ui| {
                if ui.button(tr("대기 작업 새로 고침")).clicked() {
                    self.discover(Discovery::Queue);
                }
                named_text(
                    ui,
                    &mut self.controls_ui.actions.queue_text,
                    "대기 작업 내용",
                );
                if ui
                    .add_enabled(
                        queue_editable && !self.controls_ui.actions.queue_text.trim().is_empty(),
                        egui::Button::new(tr("대기 작업 추가")),
                    )
                    .clicked()
                {
                    self.native_action(NativeAction::Codex(CodexAction::AddQueuedTask {
                        text: self.controls_ui.actions.queue_text.clone(),
                    }));
                }
                match self.transcript.controls.queue.clone() {
                    None => {
                        ui.label(tr("목록을 아직 받지 못했습니다."));
                    }
                    Some(queue) => {
                        self.controls_ui
                            .actions
                            .queue_edits
                            .retain(|id, _| queue.iter().any(|task| task.id == *id));
                        for task in queue {
                            ui.push_id(&task.id, |ui| {
                                let text = self
                                    .controls_ui
                                    .actions
                                    .queue_edits
                                    .entry(task.id.clone())
                                    .or_insert(task.text);
                                ui.add_enabled(
                                    task.editable,
                                    egui::TextEdit::multiline(text)
                                        .char_limit(120000)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(2),
                                );
                                let text = text.clone();
                                ui.horizontal_wrapped(|ui| {
                                    for (title, action, enabled) in [
                                        (
                                            "대기 작업 편집",
                                            CodexAction::UpdateQueuedTask {
                                                id: task.id.clone(),
                                                text: text.clone(),
                                            },
                                            task.editable && !text.trim().is_empty(),
                                        ),
                                        (
                                            "대기 작업 삭제",
                                            CodexAction::DeleteQueuedTask {
                                                id: task.id.clone(),
                                            },
                                            task.editable,
                                        ),
                                        (
                                            "대기 작업 시작",
                                            CodexAction::StartQueuedTask {
                                                id: task.id.clone(),
                                            },
                                            true,
                                        ),
                                    ] {
                                        if ui
                                            .add_enabled(
                                                enabled
                                                    && if matches!(
                                                        action,
                                                        CodexAction::StartQueuedTask { .. }
                                                    ) {
                                                        ready
                                                    } else {
                                                        queue_editable
                                                    },
                                                egui::Button::new(tr(title)),
                                            )
                                            .clicked()
                                        {
                                            self.native_action(NativeAction::Codex(action));
                                        }
                                    }
                                });
                            });
                        }
                    }
                }
            });
        }
        if self.action_available("review/start") {
            egui::CollapsingHeader::new(tr("검토")).show(ui, |ui| {
                let form = &mut self.controls_ui.actions;
                egui::ComboBox::from_id_salt("review-kind")
                    .selected_text(tr([
                        "커밋되지 않은 변경",
                        "기준 브랜치",
                        "커밋",
                        "검토 지시",
                    ][form.review_kind as usize]))
                    .show_ui(ui, |ui| {
                        for (i, title) in ["커밋되지 않은 변경", "기준 브랜치", "커밋", "검토 지시"]
                            .iter()
                            .enumerate()
                        {
                            ui.selectable_value(&mut form.review_kind, i as u8, tr(title));
                        }
                    });
                if form.review_kind != 0 {
                    named_text(ui, &mut form.review_text, "검토 대상");
                }
                let valid = form.review_kind == 0 || !form.review_text.trim().is_empty();
                let target = match form.review_kind {
                    1 => ReviewTarget::BaseBranch {
                        branch: form.review_text.clone(),
                    },
                    2 => ReviewTarget::Commit {
                        sha: form.review_text.clone(),
                    },
                    3 => ReviewTarget::Custom {
                        instructions: form.review_text.clone(),
                    },
                    _ => ReviewTarget::UncommittedChanges,
                };
                if ui
                    .add_enabled(ready && valid, egui::Button::new(tr("검토 시작")))
                    .clicked()
                {
                    self.native_action(NativeAction::Codex(CodexAction::Review(target)));
                }
            });
        }
        if self.action_available("turn/settings/update") {
            egui::CollapsingHeader::new(tr("현재 턴 설정")).show(ui,|ui|{
                ui.add(egui::Label::new(tr("현재 실행 중인 턴의 이후 작업에 적용됩니다. 이미 시작한 작업은 변경하지 않습니다.")).wrap());
                let turn=self.transcript.controls.status.get("active_turn_settings");
                let turn_id=turn.and_then(|s|s.fields.get("turnId")).and_then(|v|if let ControlValue::Text(v)=v{Some(v.clone())}else{None});
                if turn_id.is_none() {ui.label(tr("네이티브 턴을 아직 확인하지 못했습니다."));}
                let active_model=turn.and_then(|s|s.fields.get("model")).and_then(|v|if let ControlValue::Text(v)=v{Some(v.as_str())}else{None});
                let metadata=self.transcript.controls.models.iter().find(|m|Some(m.id.as_str())==active_model || m.resolved_model.as_deref()==active_model);

                let options=self.transcript.controls.options.iter().filter(|o|matches!(o.id.as_str(),"model"|"effort"|"serviceTier"|"summary"|"approvalsReviewer")).cloned().map(|mut o| {
                    if o.id=="model" { for choice in &mut o.choices { if let Some(meta)=self.transcript.controls.models.iter().find(|m|m.id==choice.value) { choice.description=meta.description.clone(); } } }
                    if let Some(meta)=metadata {if o.id=="effort" {o.choices=meta.efforts.clone();}if o.id=="serviceTier" {o.choices=meta.tiers.clone();}}
                    o
                }).collect::<Vec<_>>();
                let form=&mut self.controls_ui.actions;
                let control_combo = egui::ComboBox::from_id_salt("turn-control").selected_text(settings_ui::label(&form.turn_control)).show_ui(ui,|ui|{for o in &options{if settings_ui::described_choice(ui,form.turn_control==o.id,settings_ui::label(&o.id),settings_ui::control_help(&o.id)).clicked(){form.turn_control=o.id.clone();form.turn_value=None;}}});
                control_combo.response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::ComboBox,true,tr("현재 턴 설정 항목")));
                if let Some(o)=options.iter().find(|o|o.id==form.turn_control) {
                    if form.turn_value.as_ref().is_some_and(|value| !o.choices.iter().any(|c| value==&ControlValue::Text(c.value.clone()))) {form.turn_value=None;}
                    let value_combo = egui::ComboBox::from_id_salt("turn-value").selected_text(form.turn_value.as_ref().map(|v| settings_ui::present_value(&o.id,v)).unwrap_or_else(|| tr("선택하세요").into())).show_ui(ui,|ui|{for c in &o.choices{if settings_ui::described_choice(ui,form.turn_value==Some(ControlValue::Text(c.value.clone())),&settings_ui::choice_title(o,c),&settings_ui::choice_help(o,c)).clicked(){form.turn_value=Some(ControlValue::Text(c.value.clone()));}}});
                    value_combo.response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::ComboBox,true,tr("현재 턴 설정 값")));
                }
                let action=form.turn_value.clone().map(|value|CodexAction::UpdateActiveTurn{turn_id:turn_id.clone().unwrap_or_default(),control:form.turn_control.clone(),value});
                let valid=action.is_some() && turn_id.is_some() && self.busy() && !self.settings_blocked();
                if ui.add_enabled(valid,egui::Button::new(tr("현재 턴에 적용"))).clicked(){self.native_action(NativeAction::Codex(action.unwrap()));}
            });
        }
        ui.horizontal_wrapped(|ui| {
            if self.action_available("config/mcpServer/reload")
                && ui
                    .add_enabled(ready, egui::Button::new(tr("MCP 다시 로드")))
                    .clicked()
            {
                self.native_action(NativeAction::Codex(CodexAction::ReloadMcp));
            }
            if self.action_available("thread/memory/read")
                && ui.button(tr("메모리 상태 새로 고침")).clicked()
            {
                self.discover(Discovery::Memory);
            }
            if self.action_available("thread/memoryMode/set") {
                for (name, value) in [("메모리 켜기", true), ("메모리 끄기", false)] {
                    if ui.add_enabled(ready, egui::Button::new(tr(name))).clicked() {
                        self.native_action(NativeAction::Codex(CodexAction::SetMemoryEnabled(
                            value,
                        )));
                    }
                }
            }
        });
    }
}
fn goal_label(status: Option<&GoalStatus>) -> &'static str {
    tr(match status {
        None => "상태 유지",
        Some(GoalStatus::Active) => "활성",
        Some(GoalStatus::Paused) => "일시 중지",
        Some(GoalStatus::Complete) => "완료",
        Some(GoalStatus::Blocked) => "차단됨",
        Some(GoalStatus::UsageLimited) => "사용량 제한",
        Some(GoalStatus::BudgetLimited) => "예산 제한",
    })
}

fn named_text(ui: &mut Ui, value: &mut String, key: &str) {
    ui.label(tr(key));
    ui.add(
        egui::TextEdit::multiline(value)
            .desired_width(f32::INFINITY)
            .desired_rows(2)
            .char_limit(120000),
    )
    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, tr(key)));
}

impl AgentPane {
    pub(super) fn permission_subset_ui(
        &mut self,
        ui: &mut Ui,
        request: &Request,
    ) -> Option<Decision> {
        if self.descriptor.tool != Tool::Codex
            || request.title != "item/permissions/requestApproval"
        {
            return None;
        }
        let entry = self
            .controls_ui
            .grants
            .entry(request.id.clone())
            .or_insert_with(|| {
                (
                    request.clone(),
                    PermissionGrant {
                        entry_indices: vec![],
                        read_indices: vec![],
                        write_indices: vec![],
                        network: false,
                        scope: PermissionScope::Turn,
                        strict_review: true,
                    },
                )
            });
        if entry.0 != *request {
            *entry = (
                request.clone(),
                PermissionGrant {
                    entry_indices: vec![],
                    read_indices: vec![],
                    write_indices: vec![],
                    network: false,
                    scope: PermissionScope::Turn,
                    strict_review: true,
                },
            );
        }
        let grant = &mut entry.1;
        ui.small(tr("요청한 권한 중 허용할 항목과 범위를 선택하세요."));
        let permissions = &request.details["permissions"];
        let mut decision = None;
        ui.add_enabled_ui(
            self.transport_alive && !self.responding.contains_key(&request.id),
            |ui| {
                for (key, label, selected) in [
                    ("entries", "파일 접근", &mut grant.entry_indices),
                    ("read", "읽기 경로", &mut grant.read_indices),
                    ("write", "쓰기 경로", &mut grant.write_indices),
                ] {
                    for (index, value) in permissions["fileSystem"][key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .take(128)
                    {
                        if value["access"] == "deny" {
                            continue;
                        }
                        let path = value
                            .as_str()
                            .or_else(|| value["path"].as_str())
                            .or_else(|| value["path"]["path"].as_str())
                            .or_else(|| value["path"]["pattern"].as_str());
                        let Some(path) = path else {
                            ui.label(tr("알 수 없는 권한 항목은 허용하지 않습니다."));
                            continue;
                        };
                        let mut checked = selected.contains(&index);
                        if ui
                            .checkbox(&mut checked, format!("{}: {}", tr(label), path))
                            .changed()
                        {
                            selected.retain(|v| *v != index);
                            if checked {
                                selected.push(index);
                            }
                        }
                    }
                }
                if permissions["network"]["enabled"] == true {
                    ui.checkbox(&mut grant.network, tr("네트워크 접근"));
                }
                egui::ComboBox::from_id_salt((&request.id, "permission-scope"))
                    .selected_text(tr(if grant.scope == PermissionScope::Turn {
                        "현재 작업"
                    } else {
                        "현재 세션"
                    }))
                    .show_ui(ui, |ui| {
                        if settings_ui::described_choice(ui, grant.scope == PermissionScope::Turn, tr("현재 작업"), tr("현재 작업이 끝날 때까지만 이 권한을 허용합니다.")).clicked() { grant.scope = PermissionScope::Turn; }
                        if settings_ui::described_choice(ui, grant.scope == PermissionScope::Session, tr("현재 세션"), tr("이 세션의 후속 작업에도 이 권한을 허용합니다.")).clicked() { grant.scope = PermissionScope::Session; }
                    });
                ui.checkbox(&mut grant.strict_review, tr("엄격한 자동 검토"));
                let nonempty = grant.network
                    || !grant.entry_indices.is_empty()
                    || !grant.read_indices.is_empty()
                    || !grant.write_indices.is_empty();
                if ui
                    .add_enabled(nonempty, egui::Button::new(tr("선택한 권한 허용")))
                    .clicked()
                {
                    decision = Some(Decision::Grant(grant.clone()));
                }
            },
        );
        decision
    }
}
