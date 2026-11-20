//! Native session settings: a reading column, with explicit value/scope/status.
use super::editing_text::EditingText;
use super::*;
use kiln_common::i18n::tr;
#[path = "settings_presentation.rs"]
mod presentation;
use presentation::*;
#[path = "settings_copy.rs"]
mod copy;
use copy::*;
#[path = "model_cards.rs"]
mod model_cards;
pub(super) use presentation::{model_title, present_value, metadata_title, choice_row, choice_title, described_choice};
pub(super) use copy::{choice_copy, choice_help, control_help, help_text, option_title};
#[derive(Default, Clone, Copy, PartialEq)]
pub(super) enum Surface {
    #[default]
    Conversation,
    Settings,
    Commands,
}
#[derive(Default)]
pub(super) struct ControlsUi {
    pub surface: Surface,
    pub search: String,
    pub command_search: String,
    pub slash_panel: Option<slash_commands::SlashPanel>,
    pub slash_diff: Option<kiln_git::DiffView>,
    pub slash_name: EditingText,
    pub composer_options_open: bool,
    pub edits: BTreeMap<String, ControlEditor>,
    pub actions: actions_ui::ActionForm,
    pub launch: launch_ui::LaunchForm,
    pub grants: BTreeMap<String, (Request, PermissionGrant)>,
}
pub(super) fn label(id: &str) -> &str {
    tr(match id {
        "model" => "모델",
        "effort" | "effortLevel" | "thought_level" => "추론 강도",
        "permissionMode" | "collaborationMode" | "mode" => "모드",
        "ultracode" => "Ultracode",
        "serviceTier" => "서비스 등급",
        "fastMode" => "빠른 모드",
        "thinkingBudget" => "생각 토큰 예산",
        "thinkingDisplay" => "생각 과정 표시",
        "enableWorkflows" => "동적 워크플로",
        "workflowSizeGuideline" => "워크플로 크기",
        "workflowKeywordTriggerEnabled" => "키워드로 워크플로 시작",
        "outputStyle" => "응답 스타일",
        "agent" => "주 에이전트",
        "advisorModel" => "자문 모델",
        "autoCompactEnabled" => "자동 압축",
        "language" => "응답 언어",
        "fallbackModel" => "대체 모델",
        "promptCacheTtl" => "프롬프트 캐시 유지 시간",
        "subagentPromptCacheTtl" => "하위 에이전트 캐시 유지 시간",
        "promptSuggestionEnabled" => "프롬프트 제안",
        "permissions" => "권한 프로필",
        "approvalPolicy" => "승인 정책",
        "approvalsReviewer" => "승인 검토자",
        "summary" => "추론 요약",
        "sandboxPolicy" => "샌드박스 정책",
        "granularApproval" => "세부 승인 정책",
        "mcp" => "MCP 상태",
        "context" => "컨텍스트",
        "permission_policy" | "requirements" => "정책 요구사항",
        "active_turn_settings" => "현재 턴 설정",
        "native_features" => "네이티브 기능",
        "launch" => "시작 옵션",
        "supportsEffort" => "추론 강도 지원",
        "supportsFastMode" => "빠른 모드 지원",
        "supportsAdaptiveThinking" => "자동 생각 지원",
        "hidden" => "숨겨진 모델",
        "isDefault" => "기본 모델",

        _ => id,
    })
}
pub(super) fn scope_label(scope: &ControlScope) -> &'static str {
    tr(match scope {
        ControlScope::Live => "현재 대화",
        ControlScope::NewConversation => "새 대화",
        ControlScope::Inherited => "상속됨",
        ControlScope::Managed => "관리됨",
        ControlScope::Terminal => "터미널 전용",
    })
}
pub(super) fn value_label(value: Option<&ControlValue>) -> String {
    match value {
        None => tr("확인 대기").into(),
        Some(ControlValue::Default) => tr("네이티브 기본값").into(),
        Some(ControlValue::Bool(v)) => tr(if *v { "켜짐" } else { "꺼짐" }).into(),
        Some(ControlValue::Text(v)) => v.clone(),
        Some(ControlValue::Integer(v)) => v.to_string(),
        Some(ControlValue::Strings(v)) => v.join(", "),
        Some(ControlValue::Sandbox(v)) => v.policy_mode().into(),
        Some(ControlValue::GranularApproval(_)) => tr("세부 승인 정책").into(),
    }
}
impl AgentPane {
    pub(super) fn settings_blocked(&self) -> bool {
        self.replay_required
            || !self.local_intents.is_empty()
            || !self.backlog.is_empty()
            || self
                .transcript
                .controls
                .outcomes
                .values()
                .any(|v| !v.settled)
    }
    pub(super) fn submit_settings_blocked(&self) -> bool {
        if self.descriptor.tool != kiln_accounts::Tool::Codex
            || !matches!(self.transcript.lifecycle, Lifecycle::Running | Lifecycle::Waiting)
            || !self.transcript.controls.status.get("active_turn_settings")
                .and_then(|status| status.fields.get("turnId"))
                .is_some_and(|id| matches!(id, ControlValue::Text(id) if !id.is_empty()))
        {
            return self.settings_blocked();
        }
        // A confirmed active turn accepts turn/steer independently of thread
        // default setting readback. Keep all local and non-setting barriers.
        self.replay_required
            || !self.local_intents.is_empty()
            || !self.backlog.is_empty()
            || self.transcript.controls.outcomes.values().any(|outcome| {
                !outcome.settled && outcome.control.starts_with("action:")
            })
    }
    pub(super) fn editing_allowed(&self) -> bool {
        self.transport_alive
            && self.transcript.lifecycle == Lifecycle::Ready
            && !self.settings_blocked()
            && self.pending.is_none()
            && !self.composing
            && !self.ime_frame
    }
    fn live_settings_allowed(&self) -> bool {
        self.live_input_allowed() && !self.settings_blocked()
            && !self.composing && !self.ime_frame
    }
    pub(super) fn configure(&mut self, option: &ControlOption, value: ControlValue) {
        if !self.live_settings_allowed() {
            return;
        }
        if let Err(error) = option.validate(&value) {
            self.error = Some(error.to_string());
            return;
        }
        if let Err(error) = self.send(Command::Configure {
            control: option.id.clone(),
            value,
        }) {
            self.error = Some(error);
        }
    }
    pub(super) fn surface_navigation(&mut self, ui: &mut Ui) {
        Frame::NONE.inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.;
                let t = Theme::current();
                for (surface, title) in [(Surface::Conversation, "대화"), (Surface::Settings, "대화 설정"), (Surface::Commands, "명령 및 스킬")] {
                    let selected = self.controls_ui.surface == surface;
                    let response = ui.add(egui::Button::new(RichText::new(tr(title)).color(if selected {t.text} else {t.text_dim})).frame(false).fill(egui::Color32::TRANSPARENT));
                    if selected { ui.painter().line_segment([response.rect.left_bottom() + egui::vec2(0., 4.), response.rect.right_bottom() + egui::vec2(0., 4.)], Stroke::new(2., t.accent)); }
                    if response.clicked() { self.controls_ui.surface = surface; self.controls_ui.slash_panel = None; self.controls_ui.composer_options_open = false; }
                }
            });
        });
    }
    pub(super) fn composer_options_ui(&mut self, ui: &mut Ui, compact: bool) {
        if !self.transcript.controls.options_reported {
            self.legacy_composer_options(ui, compact);
            return;
        }
        let options = self.transcript.controls.options.iter().filter(|o| editable_option(o) && quick_option(o)).cloned().collect::<Vec<_>>();
        ui.spacing_mut().icon_width = 10.;
        ui.spacing_mut().icon_spacing = 5.;
        let visuals = &mut ui.visuals_mut().widgets;
        for v in [&mut visuals.inactive, &mut visuals.hovered, &mut visuals.active] { v.corner_radius = egui::CornerRadius::same(6); }
        visuals.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        visuals.inactive.bg_stroke = Stroke::NONE;
        let model = options.iter().find(|o| o.id == "model" || o.category.as_deref() == Some("model"));
        if let Some(model) = model {
            let width = if compact { (ui.available_width() - 38.).clamp(70., 150.) } else { 158_f32.min(ui.available_width()) };
            ui.allocate_ui(egui::vec2(width, 28.), |ui| self.control_widget_inner(ui, model, true, false));
        }
        if compact {
            let response = widgets::icon_button(ui, kiln_common::icons::Icon::Gear, 26., false, tr("대화 옵션"));
            if options.iter().any(|o| o.id == "ultracode" && o.current == Some(ControlValue::Bool(true))) {
                kiln_common::icons::paint(ui.painter(), egui::Rect::from_center_size(response.rect.center(), egui::vec2(14.,14.)), kiln_common::icons::Icon::Gear, Theme::current().purple);
            }
            let mut open = self.controls_ui.composer_options_open;
            if response.clicked() { open = !open; }
            egui::Popup::menu(&response).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_width(240_f32.min(ui.ctx().content_rect().width() - 50.));
                ui.strong(tr("대화 옵션"));
                for option in options.iter().filter(|o| Some(o.id.as_str()) != model.map(|m| m.id.as_str())) {
                    ui.add_space(6.);
                    self.control_widget_inner(ui, option, false, false);
                }
            });
            self.controls_ui.composer_options_open = open;
        } else {
            for option in options.iter().filter(|o| matches!(o.id.as_str(), "mode" | "permissionMode" | "collaborationMode") || o.category.as_deref() == Some("mode")) {
                ui.allocate_ui(egui::vec2(108., 28.), |ui| self.control_widget_inner(ui, option, true, false));
            }
            let thoughts = options.iter().filter(|o| is_effort(o) || o.id == "ultracode").collect::<Vec<_>>();
            if !thoughts.is_empty() {
                let strength = thoughts.iter().find(|o| is_effort(o)).and_then(|o| o.current.as_ref().map(|v| present_value(semantic_id(o), v))).unwrap_or_else(|| tr("사고 설정").into());
                let ultra = thoughts.iter().any(|o| o.id == "ultracode" && o.current == Some(ControlValue::Bool(true)));
                let text = if ultra { format!("    {strength}") } else { strength };
                let response = ui.button(RichText::new(text).color(if ultra { Theme::current().purple } else { Theme::current().text_dim }));
                let mut open = self.controls_ui.composer_options_open;
                if response.clicked() { open = !open; }
                egui::Popup::menu(&response).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                    ui.set_width(240.);
                    ui.strong(tr("사고 설정"));
                    for option in &thoughts {
                        ui.add_space(8.);
                        self.control_widget_inner(ui, option, false, false);
                    }
                });
                self.controls_ui.composer_options_open = open;
                if ultra {
                    let rect = egui::Rect::from_center_size(egui::pos2(response.rect.left()+10., response.rect.center().y), egui::vec2(11.,11.));
                    kiln_common::icons::paint(ui.painter(), rect, kiln_common::icons::Icon::Sparkle, Theme::current().purple);
                }
                response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tr("사고 설정")));
                response.on_hover_text(tr("추론 강도"));
            }
        }
    }
    fn legacy_composer_options(&mut self, ui: &mut Ui, compact: bool) {
        if !self.transcript.models.is_empty() {
            let option = ControlOption { id:"model".into(), label:"model".into(), description:String::new(), category:None, kind:ControlKind::Select,
                choices:self.transcript.models.iter().map(|m|ControlChoice{value:m.id.clone(),label:m.label.clone(),description:String::new(),group:None}).collect(),
                current:self.composer.model.clone().map(ControlValue::Text),scope:ControlScope::Live,available:true,reason:None,source:Some("legacy-model".into()),documentation:None };
            let mut selected=option.current.clone();
            let width=if compact {(ui.available_width()-38.).clamp(70.,150.)}else{158.};
            ui.allocate_ui(egui::vec2(width,28.), |ui| {ui.add_enabled_ui(self.live_settings_allowed(), |ui|select_widget(ui,&option,&mut selected,true));});
            if selected != option.current { self.composer.model=selected.and_then(|v|if let ControlValue::Text(s)=v{Some(s)}else{None}); }
        }
        if !self.transcript.modes.is_empty() {
            if compact {
                let response=widgets::icon_button(ui,kiln_common::icons::Icon::Gear,26.,false,tr("대화 옵션"));
                let mut open = self.controls_ui.composer_options_open;
                if response.clicked() { open = !open; }
                egui::Popup::menu(&response).open_bool(&mut open).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui|self.legacy_mode_widget(ui));
                self.controls_ui.composer_options_open = open;
            } else {ui.allocate_ui(egui::vec2(108.,28.),|ui|self.legacy_mode_widget(ui));}
        }
    }
    fn legacy_mode_widget(&mut self, ui:&mut Ui) {
        let option=ControlOption{id:"mode".into(),label:"mode".into(),description:String::new(),category:None,kind:ControlKind::Select,
            choices:self.transcript.modes.iter().map(|m|ControlChoice{value:m.id.clone(),label:m.label.clone(),description:m.description.clone(),group:None}).collect(),
            current:self.transcript.selected_mode.clone().map(ControlValue::Text),scope:ControlScope::Live,available:true,reason:None,source:None,documentation:None};
        let mut selected=option.current.clone();
        ui.add_enabled_ui(self.live_settings_allowed(),|ui|select_widget(ui,&option,&mut selected,true));
        if selected != option.current && let Some(ControlValue::Text(mode))=selected && let Err(error)=self.send(Command::SetMode{mode}) {self.error=Some(error);}
    }
    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.;
        ui.heading(tr("대화 설정"));
        ui.add(egui::Label::new(tr("현재 대화에 사용할 모델과 작업 방식을 선택하세요.")).wrap());
        search_field(ui, &mut self.controls_ui.search, tr("설정 검색"));
        ui.horizontal(|ui| {
            ui.small(tr("현재 대화"));
            let refresh=widgets::icon_button(ui,kiln_common::icons::Icon::Refresh,24.,false,tr("설정 새로 고침"));
            if refresh.clicked() && !self.settings_blocked() { self.discover(Discovery::Settings); }
        });
        self.outcomes_ui(ui);
        let controls = self.transcript.controls.clone();
        if !controls.options_reported {
            ui.label(tr("네이티브 설정을 아직 받지 못했습니다."));
        } else if controls.options.is_empty() {
            ui.label(tr("제공된 설정이 없습니다."));
        }
        let search = self.controls_ui.search.to_lowercase();
        let filtered = controls
            .options
            .iter()
            .filter(|o| {
                editable_option(o)
                    && (search.is_empty()
                        || format!("{} {} {} {} {}", o.id, option_title(o), control_help(semantic_id(o)), o.label, o.description)
                            .to_lowercase()
                            .contains(&search))
            })
            .cloned()
            .collect::<Vec<_>>();
        ui.add_space(6.);
        ui.strong(tr("주요 설정"));
        let core = filtered.iter().filter(|o| o.scope == ControlScope::Live && quick_option(o)).collect::<Vec<_>>();
        if !core.is_empty() {
            Frame::new().fill(Theme::current().bg_panel).stroke(Stroke::new(1.,Theme::current().border)).corner_radius(10).inner_margin(6).show(ui, |ui| {
                for (index, option) in core.iter().enumerate() {
                    if index > 0 { ui.separator(); }
                    self.option_row(ui, option);
                }
            });
        }
        let advanced = filtered
            .iter()
            .filter(|o| o.scope == ControlScope::Live && !quick_option(o))
            .collect::<Vec<_>>();
        let mut groups:BTreeMap<String,Vec<&ControlOption>>=BTreeMap::new();
        for option in advanced {
            let group=match option.id.as_str() {
                "language"|"outputStyle"|"agent"|"advisorModel"|"promptSuggestionEnabled"|"summary"|"thinkingDisplay" => tr("응답"),
                "fastMode"|"enableWorkflows"|"workflowSizeGuideline"|"workflowKeywordTriggerEnabled" => tr("작업 방식"),
                "autoCompactEnabled"|"promptCacheTtl"|"subagentPromptCacheTtl"|"fallbackModel"|"thinkingBudget"|"serviceTier" => tr("컨텍스트와 비용"),
                "permissions"|"approvalPolicy"|"approvalsReviewer"|"sandboxPolicy"|"granularApproval" => tr("권한과 실행"),
                id if id.starts_with("skill:") => tr("스킬"),
                _ => tr("추가 설정"),
            };
            groups.entry(group.into()).or_default().push(option);
        }
        for (group,options) in groups {
            egui::CollapsingHeader::new(&group).open((!search.is_empty()).then_some(true)).show(ui,|ui| {
                Frame::new().fill(Theme::current().bg_panel).stroke(Stroke::new(1.,Theme::current().border)).corner_radius(10).inner_margin(6).show(ui,|ui| {
                    for (index,option) in options.iter().enumerate() { if index>0 {ui.separator();} self.option_row(ui,option); }
                });
            });
        }
        if filtered.is_empty() && !search.is_empty() && !controls.models.iter().any(|m| format!("{} {} {}", m.id, metadata_title(m), model_summary(&m.description)).to_lowercase().contains(&search)) {ui.label(tr("검색 결과가 없습니다."));}
        egui::CollapsingHeader::new(tr("변경 기록")).id_salt("settings-history").show(ui, |ui| self.outcome_history_ui(ui));
        egui::CollapsingHeader::new(tr("모델 비교")).id_salt("model-catalog").open((!search.is_empty()).then_some(true)).show(ui, |ui| {
            let model = controls.options.iter().find(|o| o.id == "model" || o.category.as_deref() == Some("model"));
            let selected = model.and_then(|o| o.current.as_ref());
            let enabled = self.live_settings_allowed();
            if let Some(value) = model_cards::catalog(ui, &controls.models, controls.models_reported, selected, model, enabled, &search)
                && let Some(option) = model { self.configure(option, value); }
        });
        egui::CollapsingHeader::new(tr("상태 및 연결")).show(ui, |ui| self.status_ui(ui));
        egui::CollapsingHeader::new(tr("네이티브 작업")).show(ui, |ui| self.actions_ui(ui));
        egui::CollapsingHeader::new(tr("새 대화 설정")).show(ui, |ui| self.launch_ui(ui));
        if let Some(error) = &self.error {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
        if let Some(error) = self.transcript.errors.last() {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
    }
    fn core_option_row(&mut self, ui: &mut Ui, option: &ControlOption) {
        ui.push_id((&option.id, "row"), |ui| {
            Frame::new()
                .fill(egui::Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(8, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 4.;
                    let heading = |ui: &mut Ui| {
                        ui.strong(option_title(option));
                    };
                    if ui.available_width() >= 360. {
                        ui.horizontal(|ui| {
                            ui.allocate_ui(egui::vec2(140., 28.), heading);
                            let width = (ui.available_width() - 34.).max(0.);
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 28.),
                                egui::Layout::left_to_right(egui::Align::Center)
                                    .with_main_wrap(true),
                                |ui| self.control_widget_inner(ui, option, false, false),
                            );
                            self.core_details(ui, option);
                        });
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            heading(ui);
                            self.core_details(ui, option);
                        });
                        ui.horizontal_wrapped(|ui| {
                            self.control_widget_inner(ui, option, false, false)
                        });
                    }
                    help_text(ui, control_help(semantic_id(option)));
                    if let Some(choice) = option.choices.iter().find(|c| option.current == Some(ControlValue::Text(c.value.clone()))) {
                        help_text(ui, &choice_help(option, choice));
                    }
                    if !option.available {
                        ui.small(tr("이 대화에서 변경할 수 없음"));
                    }
                    if let Some(reason) = &option.reason {
                        ui.add(
                            egui::Label::new(RichText::new(native_notice(reason)).small()).wrap(),
                        );
                    }
                });
            ui.add_space(4.);
        });
    }
    fn core_details(&mut self, ui: &mut Ui, option: &ControlOption) {
        let response=widgets::icon_button(ui,kiln_common::icons::Icon::Info,22.,false,tr("세부 정보"));
        egui::Popup::menu(&response).show(|ui| {
            ui.set_max_width(280.);
            if !option.description.is_empty() {
                ui.strong(tr("제공자 설명"));
                ui.add(egui::Label::new(&option.description).wrap());
            }
            if option.choices.iter().any(|c| !c.description.is_empty()) {
                egui::CollapsingHeader::new(tr("제공자 설명 원문")).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(240.).show(ui, |ui| {
                        for choice in &option.choices {
                            if !choice.description.is_empty() {
                                ui.strong(choice_title(option, choice));
                                ui.add(egui::Label::new(&choice.description).wrap());
                            }
                        }
                    });
                });
            }
            if let Some(category) = &option.category {
                ui.small(category);
            }
            if let Some(source) = &option.source {
                ui.small(format!("{}: {source}", tr("출처")));
            }
            if let Some(doc) = &option.documentation {
                ui.hyperlink_to(tr("공식 문서"), doc);
            }
            if self.reset_control_ui(ui, option) && option.current != Some(ControlValue::Default) {
                self.configure(option, ControlValue::Default);
            }
        });
    }
    fn option_row(&mut self, ui: &mut Ui, option: &ControlOption) {
        self.core_option_row(ui, option);
    }
    pub(super) fn control_widget(&mut self, ui: &mut Ui, option: &ControlOption, quick: bool) {
        self.control_widget_inner(ui, option, quick, !quick);
    }
    fn control_widget_inner(
        &mut self,
        ui: &mut Ui,
        option: &ControlOption,
        quick: bool,
        reset: bool,
    ) {
        let mut presented = option.clone();
        if matches!(option.id.as_str(), "model" | "advisorModel") {
            for choice in &mut presented.choices {
                if let Some(metadata) = self.transcript.controls.models.iter().find(|m| m.id == choice.value) {
                    if choice.description.is_empty() { choice.description = metadata.description.clone(); }
                    if let Some(resolved) = &metadata.resolved_model {
                        if ["opus", "fable", "sonnet", "haiku"].contains(&choice.label.to_lowercase().as_str()) {
                            choice.label = model_title(resolved);
                        }
                        if metadata.id != "default" && option.current == Some(ControlValue::Text(resolved.clone())) {
                            // Mark a resolved selection against its advertised alias, without
                            // changing the provider catalog or sending an implicit config.
                            choice.value = resolved.clone();
                        }
                    }
                }
            }
        }
        let option = &presented;
        let mut selected = option.current.clone();
        let enabled =
            self.live_settings_allowed() && option.available && option.scope == ControlScope::Live;
        ui.push_id((&option.id, quick), |ui| {
            ui.add_enabled_ui(enabled, |ui| match &option.kind {
                ControlKind::Select => select_widget(ui, option, &mut selected, quick),
                ControlKind::Boolean if option.id == "ultracode" => ultracode_switch(ui, option, &mut selected),
                ControlKind::Boolean => {
                    // Unknown is shown explicitly before a deliberate on/off selection.
                    let mut o = option.clone();
                    o.choices = vec![
                        ControlChoice {
                            value: "true".into(),
                            label: tr("켜짐").into(),
                            description: String::new(),
                            group: None,
                        },
                        ControlChoice {
                            value: "false".into(),
                            label: tr("꺼짐").into(),
                            description: String::new(),
                            group: None,
                        },
                    ];
                    let mut v = option.current.as_ref().and_then(|v| {
                        if let ControlValue::Bool(v) = v {
                            Some(ControlValue::Text(v.to_string()))
                        } else {
                            None
                        }
                    });
                    select_widget(ui, &o, &mut v, quick);
                    selected = v.and_then(|v| {
                        if let ControlValue::Text(v) = v {
                            Some(ControlValue::Bool(v == "true"))
                        } else {
                            None
                        }
                    });
                }
                ControlKind::Unsupported(_) => {
                    ui.label(tr("지원되지 않는 설정 유형"));
                }
                _ => {
                    let edit = self
                        .controls_ui
                        .edits
                        .entry(option.id.clone())
                        .or_insert_with(|| ControlEditor::new(option));
                    if !edit.compatible(option) {
                        *edit = ControlEditor::new(option);
                    }
                    let scalar=matches!(edit.value,ControlValue::Text(_)|ControlValue::Integer(_));
                    let mut apply=false;
                    if scalar {
                        ui.horizontal(|ui| {
                            let width=(ui.available_width()-64.).clamp(40.,280.);
                            ui.allocate_ui(egui::vec2(width,28.),|ui|edit_value(ui,option,edit));
                            apply=ui.add_enabled(edit.text.ready(ui.ctx()) && edit.dirty(),egui::Button::new(tr("적용"))).clicked();
                        });
                    } else {
                        edit_value(ui,option,edit);
                        apply=ui.add_enabled(edit.text.ready(ui.ctx()) && edit.dirty(),egui::Button::new(tr("적용"))).clicked();
                    }
                    if apply { selected=Some(edit.value()); }
                }
            });
        });
        if reset && self.reset_control_ui(ui, option) {
            selected = Some(ControlValue::Default);
        }
        if selected != option.current
            && let Some(value) = selected
        {
            self.configure(option, value);
        }
    }
    fn reset_control_ui(&mut self, ui: &mut Ui, option: &ControlOption) -> bool {
        let supported = (self.descriptor.tool == Tool::Claude && option.id != "permissionMode")
            || (self.descriptor.tool == Tool::Codex
                && matches!(option.id.as_str(), "effort" | "serviceTier"));
        if supported
            && self.editing_allowed()
            && option.available
            && option.scope == ControlScope::Live
        {
            if ui
                .add_enabled(
                    self.controls_ui
                        .edits
                        .get(&option.id)
                        .is_none_or(|edit| edit.text.ready(ui.ctx())),
                    egui::Button::new(tr("네이티브 기본값으로 재설정")),
                )
                .clicked()
            {
                self.controls_ui.edits.remove(&option.id);
                return true;
            }
        }
        false
    }
    pub(super) fn discover(&mut self, target: Discovery) {
        if let Err(e) = self.send(Command::Discover(target)) {
            self.error = Some(e);
        }
    }
    fn outcomes_ui(&mut self, ui: &mut Ui) {
        let outcomes = self
            .transcript
            .controls
            .outcomes
            .values()
            .cloned()
            .collect::<Vec<_>>();
        if !self.local_intents.is_empty() || self.replay_required {
            ui.colored_label(Theme::current().orange, tr("설정 확인 대기"));
        }
        for outcome in outcomes.iter().rev().filter(|v| !v.settled || !matches!(v.state, ApplicationState::Applied | ApplicationState::Cancelled)).take(3) {
            ui.horizontal_wrapped(|ui| {
                ui.strong(label(&outcome.control));
                ui.label(tr(match outcome.state {
                    ApplicationState::Pending => "적용 대기",
                    ApplicationState::Applied => "적용됨",
                    ApplicationState::Acknowledged => "요청 수락 · 효과 미확인",
                    ApplicationState::Rejected => "거부됨",
                    ApplicationState::Cancelled => "취소됨",
                    ApplicationState::Unconfirmed => "확인되지 않음",
                }));
            });
            ui.add(
                egui::Label::new(format!(
                    "{}: {} · {}: {}",
                    tr("요청값"),
                    present_value(&outcome.control, &outcome.requested),
                    tr("네이티브 값"),
                    outcome.effective.as_ref().map(|v| present_value(&outcome.control, v)).unwrap_or_else(|| tr("확인 대기").into())
                ))
                .wrap(),
            );
            if let Some(message) = &outcome.message {
                ui.add(egui::Label::new(message).wrap());
            }
            if !outcome.settled
                && outcome.effective.is_some()
                && self.descriptor.tool == Tool::Codex
                && ui.button(tr("현재 네이티브 값 수락")).clicked()
            {
                if let Err(e) = self.send(Command::NativeAction(NativeAction::Codex(
                    CodexAction::AcceptEffectiveSettings {
                        operation: outcome.operation.clone(),
                    },
                ))) {
                    self.error = Some(e);
                }
            }
        }
    }
    fn outcome_history_ui(&self, ui: &mut Ui) {
        egui::ScrollArea::vertical().id_salt("outcome-history-list").max_height(180.).show(ui, |ui| {
            for outcome in self.transcript.controls.outcomes.values().rev().take(20) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(label(&outcome.control));
                    ui.label(present_value(&outcome.control, outcome.effective.as_ref().unwrap_or(&outcome.requested)));
                    ui.small(tr(match outcome.state { ApplicationState::Applied => "적용됨", ApplicationState::Pending => "적용 대기", ApplicationState::Rejected => "거부됨", ApplicationState::Cancelled => "취소됨", _ => "확인되지 않음" }));
                });
            }
        });
    }
    pub(super) fn coverage_ui(&self, ui: &mut Ui, subject: Option<&str>) {
        for coverage in self
            .transcript
            .controls
            .coverage
            .values()
            .filter(|v| v.partial && subject.is_none_or(|s| v.subject.contains(s)))
        {
            ui.add(
                egui::Label::new(format!(
                    "{} · {}: {} / {}",
                    tr("일부 목록만 표시됨"),
                    coverage.subject,
                    coverage.retained,
                    coverage.received
                ))
                .wrap(),
            );
            if let Some(reason) = &coverage.reason {
                ui.add(egui::Label::new(reason).wrap());
            }
        }
    }
    pub(super) fn status_ui(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for (label, target) in [
                ("MCP 새로 고침", Discovery::Mcp),
                ("컨텍스트 새로 고침", Discovery::Context),
                ("정책 새로 고침", Discovery::Policy),
                ("상태 새로 고침", Discovery::Status),
            ] {
                if ui.button(tr(label)).clicked() {
                    self.discover(target);
                }
            }
        });
        if self.transcript.controls.status.contains_key("mcp") {
            self.mcp_status_ui(ui, false);
        }
        let visible: Vec<_> = self.transcript.controls.status.values().filter(|status| !matches!(status.subject.as_str(), "native_terminal_return" | "native_cli_frontend" | "prompt_suggestion" | "mcp")).collect();
        if visible.is_empty() {
            ui.label(tr("네이티브 상태를 아직 받지 못했습니다."));
        }
        for status in visible {
            let extension = status.subject.starts_with("profile_extension:");
            ui.strong(if extension {
                tr("상속된 확장")
            } else {
                label(&status.subject)
            });
            if extension {
                ui.small(tr("준비·보존 상태는 네이티브 로드 확인이 아닙니다."));
            }
            if let Some(error) = &status.unavailable {
                ui.add(egui::Label::new(error).wrap());
            }
            if extension {
                let text = |key: &str| {
                    status
                        .fields
                        .get(key)
                        .and_then(|v| {
                            if let ControlValue::Text(s) = v {
                                Some(s.as_str())
                            } else {
                                None
                            }
                        })
                        .unwrap_or("")
                };
                ui.add(egui::Label::new(text("name")).wrap());
                ui.label(extension_state(text("state")));
                ui.add(egui::Label::new(extension_reason(text("reason"))).wrap());
                egui::CollapsingHeader::new(tr("진단 정보"))
                    .id_salt(&status.subject)
                    .show(ui, |ui| {
                        for (key, value) in &status.fields {
                            ui.add(
                                egui::Label::new(format!("{key}: {}", value_label(Some(value))))
                                    .wrap(),
                            );
                        }
                    });
            } else {
                for (key, value) in &status.fields {
                    ui.add(
                        egui::Label::new(format!("{}: {}", label(key), value_label(Some(value))))
                            .wrap(),
                    );
                }
            }
            ui.separator();
        }
    }
}
pub(super) fn select_widget(
    ui: &mut Ui,
    option: &ControlOption,
    selected: &mut Option<ControlValue>,
    quick: bool,
) {
    let value = selected.as_ref().and_then(|v| {
        if let ControlValue::Text(v) = v {
            Some(v.as_str())
        } else {
            None
        }
    });
    let display = option
        .choices
        .iter()
        .find(|c| Some(c.value.as_str()) == value)
        .map(|c| choice_title(option, c))
        .unwrap_or_else(|| {
            if option.source.as_deref() == Some("legacy-model") && selected.is_none() {
                tr("기본 모델").into()
            } else {
                selected.as_ref().map(|v| present_value(semantic_id(option),v)).unwrap_or_else(|| tr("선택하세요").into())
            }
        });
    let title = option_title(option);
    let text = if quick && option.id != "model" {
        if matches!(option.id.as_str(), "mode" | "permissionMode" | "collaborationMode") { display.clone() } else { format!("{title}: {display}") }
    } else {
        display
    };
    let combo = egui::ComboBox::from_id_salt(("choice", &option.id))
        .height((ui.ctx().content_rect().height() * 0.6).min(400.))
        .width(if quick {
            ui.available_width().min(320.)
        } else {
            ui.available_width().min(280.)
        })
        .selected_text(text)
        .truncate()
        .show_ui(ui, |ui| {
            ui.set_max_width(360_f32.min(ui.ctx().content_rect().width() - 48.));
            if option.source.as_deref() == Some("legacy-model")
                && ui
                    .selectable_label(selected.is_none(), tr("기본 모델"))
                    .clicked()
            {
                *selected = None;
                ui.close();
            }
            let mut group = None;
            for choice in &option.choices {
                if choice.group != group {
                    group = choice.group.clone();
                    if let Some(group) = &group {
                        ui.strong(format!("{} · {group}", tr("분류")));
                    }
                }
                let is_selected = selected.as_ref() == Some(&ControlValue::Text(choice.value.clone()));
                let response = ui.add(egui::Button::selectable(is_selected, choice_row(ui, option, choice)).wrap());
                response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), is_selected, choice_title(option, choice)));
                if response.on_hover_text(choice_help(option, choice)).clicked()
                {
                    *selected = Some(ControlValue::Text(choice.value.clone()));
                    ui.close();
                }
            }
        });
    combo.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), &title)
    });
    combo.response.on_hover_text(format!(
        "{title}\n{}\n{}",
        control_help(semantic_id(option)),
        option.current.as_ref().map(|v| present_value(semantic_id(option),v)).unwrap_or_else(|| tr("확인 대기").into())
    ));
}
pub(super) struct ControlEditor {
    kind: ControlKind,
    value: ControlValue,
    text: EditingText,
    initial_value: ControlValue,
    initial_text: String,
}
impl ControlEditor {
    pub fn compatible(&self, option: &ControlOption) -> bool {
        self.kind == option.kind
    }
    pub fn dirty(&self) -> bool {
        self.value != self.initial_value || self.text.text != self.initial_text
    }
    fn new(option: &ControlOption) -> Self {
        let seed = match option.kind {
            ControlKind::Integer { min, .. } => ControlValue::Integer(min),
            ControlKind::Strings => ControlValue::Strings(vec![]),
            ControlKind::Sandbox => ControlValue::Sandbox(NativeSandbox::ReadOnly {
                network_access: false,
            }),
            ControlKind::GranularApproval => ControlValue::GranularApproval(GranularApproval {
                sandbox_approval: false,
                rules: false,
                mcp_elicitations: false,
                request_permissions: false,
                skill_approval: false,
            }),
            _ => ControlValue::Text(String::new()),
        };
        let value = option
            .current
            .as_ref()
            .filter(|current| std::mem::discriminant(*current) == std::mem::discriminant(&seed))
            .cloned()
            .unwrap_or(seed);
        let text = match &value {
            ControlValue::Text(v) => v.clone(),
            ControlValue::Strings(v) => v.join("\n"),
            ControlValue::Sandbox(NativeSandbox::WorkspaceWrite { writable_roots, .. }) => {
                writable_roots
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => String::new(),
        };
        Self {
            kind: option.kind.clone(),
            initial_value: value.clone(),
            initial_text: text.clone(),
            value,
            text: EditingText::new(text),
        }
    }
    fn value(&self) -> ControlValue {
        let mut value = self.value.clone();
        match &mut value {
            ControlValue::Text(v) => *v = self.text.text.clone(),
            ControlValue::Strings(v) => *v = self.text.lines(),
            ControlValue::Sandbox(NativeSandbox::WorkspaceWrite { writable_roots, .. }) => {
                *writable_roots = self.text.lines().into_iter().map(PathBuf::from).collect()
            }
            _ => {}
        }
        value
    }
}
fn edit_value(ui: &mut Ui, option: &ControlOption, edit: &mut ControlEditor) {
    match &mut edit.value {
        ControlValue::Text(_) => edit.text.show(ui, label(&option.id), false, 4096),
        ControlValue::Integer(v) => {
            let range = if let ControlKind::Integer { min, max } = option.kind {
                min..=max
            } else {
                0..=u64::MAX
            };
            ui.add(egui::DragValue::new(v).range(range));
        }
        ControlValue::Strings(_) => edit.text.show(ui, label(&option.id), true, 32000),
        ControlValue::Sandbox(v) => sandbox_ui(ui, v, &mut edit.text),
        ControlValue::GranularApproval(v) => {
            for (key, value) in [
                ("샌드박스 승인", &mut v.sandbox_approval),
                ("규칙 승인", &mut v.rules),
                ("MCP 정보 요청", &mut v.mcp_elicitations),
                ("권한 요청", &mut v.request_permissions),
                ("스킬 승인", &mut v.skill_approval),
            ] {
                ui.checkbox(value, tr(key));
                help_text(ui, tr(match key {
                    "샌드박스 승인" => "샌드박스 제한을 넘는 실행의 승인 요청을 표시합니다.",
                    "규칙 승인" => "실행 규칙이 요구하는 승인 요청을 표시합니다.",
                    "MCP 정보 요청" => "MCP 서버가 추가 정보를 요청할 수 있게 합니다.",
                    "권한 요청" => "추가 권한을 요청하는 도구의 승인 요청을 표시합니다.",
                    _ => "스킬 스크립트 실행에 대한 승인 요청을 표시합니다.",
                }));
            }
        }
        _ => {
            ui.label(value_label(Some(&edit.value)));
        }
    }
}
fn sandbox_ui(ui: &mut Ui, value: &mut NativeSandbox, text: &mut EditingText) {
    let mut kind = value.policy_mode().to_owned();
    egui::ComboBox::from_id_salt("sandbox-kind")
        .selected_text(present_value("sandboxPolicy", &ControlValue::Text(kind.clone())))
        .show_ui(ui, |ui| {
            for id in ["read-only", "workspace-write", "external-sandbox"] {
                let (title, help) = choice_copy("sandboxPolicy", id).unwrap();
                if described_choice(ui, kind == id, title, help).clicked() { kind = id.into(); }
            }
        });
    if kind != value.policy_mode() {
        *text = EditingText::default();
        *value = match kind.as_str() {
            "workspace-write" => NativeSandbox::WorkspaceWrite {
                network_access: false,
                writable_roots: vec![],
                exclude_slash_tmp: false,
                exclude_tmpdir_env_var: false,
            },
            "external-sandbox" => NativeSandbox::ExternalSandbox {
                network_access: ExternalNetworkAccess::Restricted,
            },
            _ => NativeSandbox::ReadOnly {
                network_access: false,
            },
        };
    }
    match value {
        NativeSandbox::ReadOnly { network_access } => {
            ui.checkbox(network_access, tr("네트워크 접근"));
            help_text(ui, tr("켜면 이 샌드박스에서 네트워크 접근을 허용합니다."));
        }
        NativeSandbox::WorkspaceWrite {
            network_access,
            writable_roots: _,
            exclude_slash_tmp,
            exclude_tmpdir_env_var,
        } => {
            ui.checkbox(network_access, tr("네트워크 접근"));
            help_text(ui, tr("켜면 이 샌드박스에서 네트워크 접근을 허용합니다."));
            ui.checkbox(exclude_slash_tmp, tr("공용 임시 폴더 제외"));
            help_text(ui, tr("공용 임시 폴더를 기본 쓰기 허용 경로에서 제외합니다."));
            ui.checkbox(exclude_tmpdir_env_var, tr("환경 임시 폴더 제외"));
            help_text(ui, tr("환경 변수로 지정된 임시 폴더를 기본 쓰기 허용 경로에서 제외합니다."));
            let title = tr("쓰기 허용 절대 경로 · 한 줄에 하나");
            ui.label(title);
            text.show(ui, title, true, 32000);
        }
        NativeSandbox::ExternalSandbox { network_access } => {
            let mut enabled = *network_access == ExternalNetworkAccess::Enabled;
            if ui.checkbox(&mut enabled, tr("네트워크 접근")).changed() {
                *network_access = if enabled {
                    ExternalNetworkAccess::Enabled
                } else {
                    ExternalNetworkAccess::Restricted
                };
            }
        }
        NativeSandbox::DangerFullAccess => {
            help_text(ui, choice_copy("sandboxPolicy", "danger-full-access").unwrap().1);
        }
    }
}

pub(super) fn extension_state(state: &str) -> &'static str {
    tr(match state {
        "prepared" => "확장 준비됨",
        "retained" => "기존 확장 보존됨",
        "profile_override" => "프로필의 사용자 설정 유지",
        "profile_deleted" | "source_removed" => "상속 항목 제거됨",
        "failed" => "확장 준비 실패",
        "unsupported" => "자동 상속 미지원",
        "native_account" => "네이티브 계정에서 관리",
        "native" => "네이티브 위치 사용",
        "absent" => "확장 없음",
        _ => "확장 상태 확인 필요",
    })
}
pub(super) fn extension_reason(reason: &str) -> &'static str {
    tr(match reason {
        "PRIVATE_VERSION_RETAINED_FOR_READERS" => {
            "기존 실행에서 사용하는 이전 버전을 보존했습니다."
        }
        "STABLE_PLUGIN_PROFILE_INTENT" | "DEPENDENCY_PROFILE_OVERRIDE" => {
            "프로필에서 직접 변경한 선택을 유지합니다."
        }
        "ALREADY_NATIVE_NO_DUPLICATION" => "원래 네이티브 위치를 사용하며 중복 복사하지 않습니다.",
        "NATIVE_ACCOUNT_SYNC"
        | "NATIVE_ACCOUNT_OR_CONFIGURED_PLUGIN"
        | "NATIVE_ACCOUNT_OR_INLINE_PLUGIN" => "네이티브 계정 또는 직접 구성한 확장입니다.",
        "EXPLICIT_MARKETPLACE_DECLARATION_REQUIRES_NATIVE_SETUP"
        | "PLUGIN_REQUIRES_CONFIGURATION"
        | "PLUGIN_BINARY_COMPONENT_REQUIRES_NATIVE_SETUP" => {
            "네이티브 환경에서 추가 구성이 필요합니다."
        }
        "SOURCE_UNAVAILABLE" => "확장 원본에 접근할 수 없습니다.",
        "PLUGIN_UNIT_PREPARATION_FAILED" => "확장을 준비하지 못했습니다. 진단 정보를 확인하세요.",
        "" => "추가 진단 정보가 없습니다.",
        _ => "확장 준비에 대한 추가 진단 정보를 확인하세요.",
    })
}

fn editable_option(option: &ControlOption) -> bool {
    option.scope == ControlScope::Live && option.available && match &option.kind { ControlKind::Unsupported(_) => false, ControlKind::Select => !option.choices.is_empty(), _ => true }
}
fn quick_option(option: &ControlOption) -> bool {
    matches!(
        option.id.as_str(),
        "model"
            | "effort"
            | "effortLevel"
            | "thought_level"
            | "permissionMode"
            | "collaborationMode"
            | "mode"
            | "ultracode"
    ) || matches!(
        option.category.as_deref(),
        Some("model" | "thought_level" | "mode")
    )
}

fn native_notice(message: &str) -> &str {
    tr(match message {
        "Inherited native setting; live editing is not verified." => {
            "이 대화에서 직접 변경하는 기능은 확인되지 않았습니다."
        }
        "Native capability has not been advertised." => "네이티브 기능을 아직 받지 못했습니다.",
        "Select a model that advertises effort levels." => {
            "추론 강도를 제공하는 모델을 선택하세요."
        }
        "A numerical thinking budget is not advertised as meaningful for this model." => {
            "이 모델은 숫자로 지정하는 생각 예산을 제공하지 않습니다."
        }
        "The selected model does not support Fast mode." => {
            "선택한 모델은 빠른 모드를 지원하지 않습니다."
        }
        "Refresh settings before merging skill exposure overrides." => {
            "스킬 노출을 변경하기 전에 설정을 새로 고치세요."
        }
        "Native policy disables workflows." => "네이티브 정책에서 워크플로를 비활성화했습니다.",
        _ => message,
    })
}
