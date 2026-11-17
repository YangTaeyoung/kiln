//! Creation-only typed forms. Never restart or submit to the current conversation.
use super::editing_text::EditingText;
use super::*;
use kiln_common::i18n::tr;
#[derive(Default)]
pub(super) struct LaunchForm {
    options: LaunchOptions,
    error: Option<String>,
    lists: BTreeMap<&'static str, EditingText>,
}
impl AgentPane {
    pub fn take_terminal_request(&mut self) -> bool {
        std::mem::take(&mut self.terminal_request)
    }
    pub fn take_launch_request(&mut self) -> Option<LaunchOptions> {
        self.launch_request.take()
    }
    pub(super) fn launch_ui(&mut self, ui: &mut Ui) {
        ui.add(
            egui::Label::new(tr(
                "현재 선택한 계정으로 새 대화를 엽니다. 이 대화와 초안은 유지됩니다.",
            ))
            .wrap(),
        );
        let form = &mut self.controls_ui.launch;
        let mut custom = !matches!(form.options, LaunchOptions::Inherited);
        if ui.checkbox(&mut custom, tr("새 대화 옵션 지정")).changed() {
            form.lists.clear();
            form.error = None;
            form.options = if custom {
                match self.descriptor.tool {
                    Tool::Claude => LaunchOptions::Claude(Default::default()),
                    Tool::Codex => LaunchOptions::Codex(Default::default()),
                    Tool::Antigravity => LaunchOptions::Antigravity(Default::default()),
                }
            } else {
                LaunchOptions::Inherited
            };
        }
        match &mut form.options {
            LaunchOptions::Inherited => {
                ui.label(tr("네이티브 기본값을 상속합니다."));
            }
            LaunchOptions::Claude(v) => {
                optional_u32(ui, "최대 턴 수", &mut v.max_turns, 1, 100000);
                optional_f64(ui, "최대 비용 (USD)", &mut v.max_budget_usd);
                optional_u32(
                    ui,
                    "자동 압축 컨텍스트 창",
                    &mut v.auto_compact_window,
                    100000,
                    1000000,
                );
                optional_text(ui, "시스템 지시", &mut v.system_prompt);
                optional_text(ui, "추가 시스템 지시", &mut v.append_system_prompt);
                let mut set = v.setting_sources.is_some();
                if ui.checkbox(&mut set, tr("설정 출처 지정")).changed() {
                    v.setting_sources = set.then(|| {
                        vec![
                            ClaudeSettingSource::User,
                            ClaudeSettingSource::Project,
                            ClaudeSettingSource::Local,
                        ]
                    });
                }
                launch_help(ui, "설정 출처 지정");
                if let Some(sources) = &mut v.setting_sources {
                    ui.horizontal_wrapped(|ui| {
                        for (name, source) in [
                            ("사용자", ClaudeSettingSource::User),
                            ("프로젝트", ClaudeSettingSource::Project),
                            ("로컬", ClaudeSettingSource::Local),
                        ] {
                            let mut checked = sources.contains(&source);
                            if ui.checkbox(&mut checked, tr(name)).changed() {
                                sources.retain(|s| s != &source);
                                if checked {
                                    sources.push(source);
                                }
                            }
                        }
                    });
                    launch_help(ui, "설정 출처별 설명");
                }
                let mut set = v.tools.is_some();
                if ui.checkbox(&mut set, tr("도구 목록 지정")).changed() {
                    v.tools = set.then(Vec::new);
                    form.lists.remove("도구 이름 · 한 줄에 하나");
                }
                if let Some(tools) = &mut v.tools {
                    strings(ui, "도구 이름 · 한 줄에 하나", tools, &mut form.lists);
                }
                strings(
                    ui,
                    "허용 도구 · 한 줄에 하나",
                    &v.allowed_tools,
                    &mut form.lists,
                );
                strings(
                    ui,
                    "제한 도구 · 한 줄에 하나",
                    &v.disallowed_tools,
                    &mut form.lists,
                );
            }
            LaunchOptions::Codex(v) => {
                let old = v.model.clone();
                let models = self.transcript.models.iter().map(|m| ControlChoice {
                    value: m.id.clone(), label: m.label.clone(), description: self.transcript.controls.models.iter().find(|meta| meta.id == m.id).map(|m|m.description.clone()).unwrap_or_default(), group: None,
                }).collect::<Vec<_>>();
                choices(ui, "모델", &mut v.model, &models);
                let meta = self
                    .transcript
                    .controls
                    .models
                    .iter()
                    .find(|m| Some(&m.id) == v.model.as_ref());
                if old != v.model {
                    v.effort = meta.and_then(|m| m.default_effort.clone());
                    v.service_tier = meta.and_then(|m| m.default_tier.clone());
                }
                if let Some(meta) = meta {
                    choices(ui, "추론 강도", &mut v.effort, &meta.efforts);
                    choices(ui, "서비스 등급", &mut v.service_tier, &meta.tiers);
                }
                if let Some(option) = self
                    .transcript
                    .controls
                    .options
                    .iter()
                    .find(|o| o.id == "permissions")
                {
                    choices(ui, "권한 프로필", &mut v.permissions, &option.choices);
                }
                enum_select(
                    ui,
                    "추론 요약",
                    &mut v.summary,
                    &[
                        (ReasoningSummary::Auto, "자동"),
                        (ReasoningSummary::Concise, "간결하게"),
                        (ReasoningSummary::Detailed, "자세하게"),
                        (ReasoningSummary::None, "없음"),
                    ],
                );
                enum_select(
                    ui,
                    "응답 상세도",
                    &mut v.verbosity,
                    &[
                        (Verbosity::Low, "낮음"),
                        (Verbosity::Medium, "중간"),
                        (Verbosity::High, "높음"),
                    ],
                );
                enum_select(
                    ui,
                    "웹 검색",
                    &mut v.web_search,
                    &[
                        (WebSearch::Disabled, "꺼짐"),
                        (WebSearch::Cached, "캐시 검색"),
                        (WebSearch::Live, "실시간 검색"),
                    ],
                );
                optional_u64(ui, "컨텍스트 창", &mut v.context_window);
                optional_u64(ui, "자동 압축 토큰 제한", &mut v.auto_compact_token_limit);
                optional_text(ui, "개발자 지시", &mut v.developer_instructions);
                optional_text(ui, "기본 지시", &mut v.base_instructions);
                enum_select(
                    ui,
                    "임시 대화",
                    &mut v.ephemeral,
                    &[(true, "켜짐"), (false, "꺼짐")],
                );
                paths(
                    ui,
                    "워크스페이스 절대 경로 · 한 줄에 하나",
                    &v.workspace_roots,
                    &mut form.lists,
                );
            }
            LaunchOptions::Antigravity(v) => {
                ui.checkbox(&mut v.sandbox, tr("샌드박스 요청"));
                launch_help(ui, "샌드박스 요청");
                optional_text(ui, "주 에이전트", &mut v.agent);
                paths(
                    ui,
                    "추가 절대 경로 · 한 줄에 하나",
                    &v.additional_directories,
                    &mut form.lists,
                );
            }
        }
        let mut options = form.options.clone();
        let valid = options.validate(self.descriptor.tool);
        if let Err(error) = &valid {
            ui.colored_label(Theme::current().orange, error.to_string());
        }
        if let Some(error) = &form.error {
            ui.colored_label(Theme::current().red, error);
        }
        if ui
            .add_enabled(
                valid.is_ok()
                    && !self.composing
                    && !self.ime_frame
                    && form.lists.values().all(|text| text.ready(ui.ctx())),
                egui::Button::new(tr("새 대화 열기")),
            )
            .clicked()
        {
            // Parse lists only after the explicit launch action, preserving text while editing.
            let list = |key| {
                form.lists
                    .get(key)
                    .map(EditingText::lines)
                    .unwrap_or_default()
            };
            match &mut options {
                LaunchOptions::Claude(v) => {
                    if v.tools.is_some() {
                        v.tools = Some(list("도구 이름 · 한 줄에 하나"));
                    }
                    v.allowed_tools = list("허용 도구 · 한 줄에 하나");
                    v.disallowed_tools = list("제한 도구 · 한 줄에 하나");
                }
                LaunchOptions::Codex(v) => {
                    v.workspace_roots = list("워크스페이스 절대 경로 · 한 줄에 하나")
                        .into_iter()
                        .map(PathBuf::from)
                        .collect()
                }
                LaunchOptions::Antigravity(v) => {
                    v.additional_directories = list("추가 절대 경로 · 한 줄에 하나")
                        .into_iter()
                        .map(PathBuf::from)
                        .collect()
                }
                LaunchOptions::Inherited => {}
            }
            match options.validate(self.descriptor.tool) {
                Ok(()) => {
                    form.error = None;
                    self.launch_request = Some(options);
                }
                Err(error) => form.error = Some(error.to_string()),
            }
        }
    }
}
fn optional_text(ui: &mut Ui, key: &str, value: &mut Option<String>) {
    ui.push_id(key, |ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, tr(key)).changed() {
            *value = enabled.then(String::new);
        }
        launch_help(ui, key);
        if let Some(text) = value {
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(2)
                    .char_limit(16000),
            )
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, tr(key)));
        }
    });
}
fn optional_u32(ui: &mut Ui, key: &str, value: &mut Option<u32>, min: u32, max: u32) {
    ui.push_id(key, |ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, tr(key)).changed() {
            *value = enabled.then_some(min);
        }
        launch_help(ui, key);
        if let Some(n) = value {
            ui.add(egui::DragValue::new(n).range(min..=max));
        }
    });
}
fn optional_u64(ui: &mut Ui, key: &str, value: &mut Option<u64>) {
    ui.push_id(key, |ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, tr(key)).changed() {
            *value = enabled.then_some(1);
        }
        launch_help(ui, key);
        if let Some(n) = value {
            ui.add(egui::DragValue::new(n).range(1..=u64::MAX));
        }
    });
}
fn optional_f64(ui: &mut Ui, key: &str, value: &mut Option<f64>) {
    ui.push_id(key, |ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, tr(key)).changed() {
            *value = enabled.then_some(1.);
        }
        launch_help(ui, key);
        if let Some(n) = value {
            ui.add(egui::DragValue::new(n).range(0.01..=1000000.).speed(0.1));
        }
    });
}
fn strings(
    ui: &mut Ui,
    key: &'static str,
    values: &[String],
    lists: &mut BTreeMap<&'static str, EditingText>,
) {
    let text = lists
        .entry(key)
        .or_insert_with(|| EditingText::new(values.join("\n")));
    ui.push_id(key, |ui| {
        ui.strong(tr(key));
        launch_help(ui, key);
        text.show(ui, tr(key), true, 16000);
    });
}
fn paths(
    ui: &mut Ui,
    key: &'static str,
    values: &[PathBuf],
    lists: &mut BTreeMap<&'static str, EditingText>,
) {
    let text = lists.entry(key).or_insert_with(|| {
        EditingText::new(
            values
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    });
    ui.push_id(key, |ui| {
        ui.strong(tr(key));
        launch_help(ui, key);
        text.show(ui, tr(key), true, 16000);
    });
}
fn choices(ui: &mut Ui, key: &str, value: &mut Option<String>, choices: &[ControlChoice]) {
    ui.push_id(key, |ui| {
        let id = match key { "모델" => "model", "추론 강도" => "effort", "서비스 등급" => "serviceTier", "권한 프로필" => "permissions", _ => key };
        ui.strong(tr(key));
        settings_ui::help_text(ui, settings_ui::control_help(id));
        let option = ControlOption { id:id.into(), label:id.into(), description:String::new(), category:None, kind:ControlKind::Select, choices:choices.to_vec(), current:value.clone().map(ControlValue::Text), scope:ControlScope::NewConversation, available:true, reason:None, source:None, documentation:None };
        let mut current = Some(option.current.clone().unwrap_or(ControlValue::Default));
        settings_ui::select_widget(ui, &option, &mut current, false);
        *value = current.and_then(|v| if let ControlValue::Text(v)=v {Some(v)}else{None});
        if ui.small_button(tr("기본값으로 되돌리기")).clicked() { *value = None; }
    });
}
fn enum_select<T: PartialEq + Clone>(
    ui: &mut Ui,
    key: &str,
    value: &mut Option<T>,
    choices: &[(T, &str)],
) {
    ui.push_id(key, |ui| {
        ui.strong(tr(key));
        launch_help(ui, key);
        let current = choices
            .iter()
            .find(|(v, _)| Some(v) == value.as_ref())
            .map(|(_, label)| tr(label))
            .unwrap_or(tr("네이티브 기본값"));
        egui::ComboBox::from_id_salt("select")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.set_max_width(360_f32.min(ui.ctx().content_rect().width() - 48.));
                if settings_ui::described_choice(ui, value.is_none(), tr("네이티브 기본값"), tr("설치된 CLI와 계정의 기본 설정을 따릅니다.")).clicked() { *value = None; }
                for (choice, label) in choices {
                    if settings_ui::described_choice(ui, value.as_ref() == Some(choice), tr(label), enum_help(key, label)).clicked() { *value = Some(choice.clone()); }
                }
            });
    });
}

fn launch_help(ui: &mut Ui, key: &str) {
    settings_ui::help_text(ui, tr(match key {
        "최대 턴 수" => "새 대화에서 AI가 수행할 턴 수의 상한입니다.",
        "최대 비용 (USD)" => "새 대화의 비용 상한을 미국 달러로 지정합니다.",
        "자동 압축 컨텍스트 창" | "컨텍스트 창" => "컨텍스트 창의 크기를 토큰 수로 지정합니다.",
        "자동 압축 토큰 제한" => "이 토큰 수에 도달하면 대화 내용을 압축하도록 설정합니다.",
        "시스템 지시" => "새 대화에 사용할 시스템 지시를 지정합니다.",
        "추가 시스템 지시" => "기본 시스템 지시 뒤에 추가할 지시를 입력합니다.",
        "개발자 지시" => "새 대화의 작업 방식을 정하는 개발자 지시를 입력합니다.",
        "기본 지시" => "새 대화에 사용할 기본 지시를 지정합니다.",
        "설정 출처 지정" => "새 대화에서 읽을 설정 파일의 범위를 직접 선택합니다.",
        "설정 출처별 설명" => "사용자: 개인 공통 설정 · 프로젝트: 공유 프로젝트 설정 · 로컬: 이 프로젝트의 개인 설정",
        "도구 이름 · 한 줄에 하나" => "새 대화에서 사용할 도구 이름을 한 줄에 하나씩 입력합니다.",
        "허용 도구 · 한 줄에 하나" => "별도 승인 없이 허용할 도구 규칙을 입력합니다.",
        "제한 도구 · 한 줄에 하나" => "AI가 사용할 수 없게 할 도구 규칙을 입력합니다.",
        "워크스페이스 절대 경로 · 한 줄에 하나" => "새 대화의 작업 폴더를 절대 경로로 입력합니다.",
        "추가 절대 경로 · 한 줄에 하나" => "새 대화에 추가로 포함할 작업 폴더를 절대 경로로 입력합니다.",
        "샌드박스 요청" => "Antigravity에 격리된 실행 환경을 요청합니다. 적용 여부는 CLI가 결정합니다.",
        "주 에이전트" => "새 대화를 맡을 설치된 에이전트의 이름을 입력합니다.",
        "추론 요약" => "추론 과정의 요약을 얼마나 자세히 표시할지 선택합니다.",
        "응답 상세도" => "최종 답변에 포함할 설명의 양을 선택합니다.",
        "웹 검색" => "웹 검색을 사용하지 않을지, 저장된 결과 또는 실시간 결과를 사용할지 선택합니다.",
        "임시 대화" => "이 대화를 기록에 저장할지 선택합니다.",
        _ => "체크하면 새 대화에 이 값을 지정합니다. 해제하면 기본 설정을 따릅니다.",
    }));
}
fn enum_help(key: &str, value: &str) -> &'static str {
    tr(match (key, value) {
        ("추론 요약", "자동") => "모델이 적절한 추론 요약 방식을 선택합니다.",
        ("추론 요약", "간결하게") => "추론 요약을 짧게 표시합니다.",
        ("추론 요약", "자세하게") => "추론 요약을 자세히 표시합니다.",
        ("추론 요약", _) => "추론 요약을 표시하지 않습니다. 추론 강도는 바뀌지 않습니다.",
        ("응답 상세도", "낮음") => "답변을 짧고 간결하게 작성합니다.",
        ("응답 상세도", "중간") => "답변에 적당한 양의 설명을 포함합니다.",
        ("응답 상세도", _) => "답변에 자세한 설명을 포함합니다.",
        ("웹 검색", "꺼짐") => "웹 검색을 사용하지 않습니다.",
        ("웹 검색", "캐시 검색") => "제공자가 저장한 웹 검색 결과를 사용합니다.",
        ("웹 검색", _) => "최신 웹 검색 결과를 요청합니다.",
        ("임시 대화", "켜짐") => "이 대화를 기록에 저장하지 않습니다.",
        ("임시 대화", _) => "이 대화를 기록에 저장합니다.",
        _ => "설치된 CLI와 계정의 기본 설정을 따릅니다.",
    })
}
