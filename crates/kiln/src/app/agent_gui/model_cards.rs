//! Compact model comparison. Provider diagnostics stay behind an explicit disclosure.
use super::*;

fn current_model_id<'a>(
    models: &'a [ModelMetadata],
    selected: Option<&ControlValue>,
) -> Option<&'a str> {
    let Some(ControlValue::Text(id)) = selected else {
        return None;
    };
    models
        .iter()
        .find(|m| m.id == *id)
        .or_else(|| {
            models
                .iter()
                .find(|m| m.id != "default" && m.resolved_model.as_ref() == Some(id))
        })
        .or_else(|| {
            models
                .iter()
                .find(|m| m.resolved_model.as_ref() == Some(id))
        })
        .map(|m| m.id.as_str())
}
fn badge(ui: &mut Ui, text: impl Into<String>, active: bool) {
    let theme = Theme::current();
    Frame::new()
        .fill(if active {
            theme.accent.gamma_multiply(0.16)
        } else {
            theme.bg_panel
        })
        .corner_radius(5)
        .inner_margin(Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).size(12.).color(if active {
                theme.accent
            } else {
                theme.text_dim
            }));
        });
}
pub(super) fn catalog(
    ui: &mut Ui,
    models: &[ModelMetadata],
    reported: bool,
    selected: Option<&ControlValue>,
    option: Option<&ControlOption>,
    enabled: bool,
    search: &str,
) -> Option<ControlValue> {
    if !reported {
        ui.label(tr("목록을 아직 받지 못했습니다."));
        return None;
    }
    help_text(
        ui,
        tr("현재 모델을 먼저 표시합니다. 다른 모델을 비교하고 바로 선택할 수 있습니다."),
    );
    let active_id = current_model_id(models, selected);
    let mut sorted = models
        .iter()
        .filter(|m| {
            search.is_empty()
                || format!(
                    "{} {} {} {}",
                    m.id,
                    metadata_title(m),
                    model_summary(&m.description),
                    m.description
                )
                .to_lowercase()
                .contains(search)
        })
        .collect::<Vec<_>>();
    sorted.sort_by_key(|m| Some(m.id.as_str()) != active_id);
    let mut request = None;
    for metadata in sorted {
        let active = Some(metadata.id.as_str()) == active_id;
        let theme = Theme::current();
        ui.push_id((&metadata.id, "model-card"), |ui| {
            Frame::new()
                .fill(theme.bg_panel)
                .stroke(Stroke::new(
                    if active { 1.5 } else { 1. },
                    if active { theme.accent } else { theme.border },
                ))
                .corner_radius(10)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 7.;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(metadata_title(metadata)).size(16.).strong());
                        if active {
                            badge(ui, tr("현재 사용 중"), true);
                        } else if metadata.capabilities.get("isDefault") == Some(&true) {
                            badge(ui, tr("기본 모델"), false);
                        }
                        if !active && let Some(option) = option {
                            let advertised = option.choices.iter().any(|c| c.value == metadata.id);
                            let response = ui.add_enabled(
                                enabled
                                    && advertised
                                    && option.available
                                    && option.scope == ControlScope::Live,
                                egui::Button::new(tr("이 모델 사용")),
                            );
                            if response.clicked() {
                                request = Some(ControlValue::Text(metadata.id.clone()));
                            }
                        }
                    });
                    ui.add(egui::Label::new(model_summary(&metadata.description)).wrap());
                    ui.horizontal_wrapped(|ui| {
                        if let Some(effort) = &metadata.default_effort {
                            badge(
                                ui,
                                format!(
                                    "{} · {}",
                                    tr("기본 추론 강도"),
                                    present_value("effort", &ControlValue::Text(effort.clone()))
                                ),
                                false,
                            );
                        }
                        for modality in &metadata.modalities {
                            badge(
                                ui,
                                tr(match modality.as_str() {
                                    "text" => "텍스트",
                                    "image" => "이미지",
                                    "audio" => "오디오",
                                    "video" => "동영상",
                                    _ => "추가 입력 형식",
                                }),
                                false,
                            );
                        }
                        if let Some(tier) = &metadata.default_tier {
                            badge(
                                ui,
                                present_value("serviceTier", &ControlValue::Text(tier.clone())),
                                false,
                            );
                        }
                    });
                    egui::CollapsingHeader::new(tr("상세 정보")).show(ui, |ui| {
                        ui.label(format!("{}: {}", tr("모델 식별자"), metadata.id));
                        if let Some(resolved) = &metadata.resolved_model
                            && resolved != &metadata.id
                        {
                            ui.label(format!("{}: {resolved}", tr("실제 사용 모델")));
                        }
                        if !metadata.efforts.is_empty() {
                            ui.strong(tr("지원 추론 강도"));
                            for choice in &metadata.efforts {
                                if let Some((title, help)) = choice_copy("effort", &choice.value) {
                                    help_text(ui, &format!("{title} · {help}"));
                                } else {
                                    help_text(
                                        ui,
                                        &format!("{} · {}", tr("제공자 정의 단계"), choice.label),
                                    );
                                }
                            }
                        }
                        if !metadata.tiers.is_empty() {
                            ui.strong(tr("서비스 등급"));
                            for choice in &metadata.tiers {
                                if let Some((title, help)) =
                                    choice_copy("serviceTier", &choice.value)
                                {
                                    help_text(ui, &format!("{title} · {help}"));
                                } else {
                                    help_text(
                                        ui,
                                        &format!("{} · {}", tr("제공자 정의 등급"), choice.label),
                                    );
                                }
                            }
                        }
                        for (key, enabled) in &metadata.capabilities {
                            if matches!(key.as_str(), "hidden" | "isDefault") {
                                continue;
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label(metadata_key(key));
                                badge(ui, tr(if *enabled { "지원" } else { "미지원" }), *enabled);
                            });
                        }
                        // No null/default placeholders or false flags in the comparison surface.
                        for (key, value) in &metadata.availability {
                            if matches!(value, ControlValue::Default) {
                                continue;
                            }
                            if key == "message" {
                                continue;
                            }
                            ui.add(
                                egui::Label::new(format!(
                                    "{}: {}",
                                    metadata_key(key),
                                    metadata_value(key, value)
                                ))
                                .wrap(),
                            );
                        }
                        if !metadata.description.is_empty() {
                            egui::CollapsingHeader::new(tr("제공자 설명 원문")).show(ui, |ui| {
                                ui.add(egui::Label::new(&metadata.description).wrap());
                                if let Some(ControlValue::Text(message)) =
                                    metadata.availability.get("message")
                                {
                                    ui.add(egui::Label::new(message).wrap());
                                }
                            });
                        }
                    });
                });
        });
        ui.add_space(6.);
    }
    request
}
fn metadata_key(key: &str) -> String {
    let known = match key {
        "accessPrograms" => "이용 프로그램",
        "modelSpecialty" => "모델 전문 분야",
        "multiAgentVersion" => "다중 에이전트 버전",
        "upgrade" => "권장 후속 모델",
        "supportsEffort" => "추론 강도 조절",
        "supportsFastMode" => "빠른 응답",
        "supportsAutoMode" => "자동 검토",
        "supportsThinkingOff" => "추론 끄기",
        "supportsAdaptiveThinking" => "자동 추론 조절",
        "availabilityNux" => "이용 안내",
        _ => return format!("{} · {key}", tr("추가 속성")),
    };
    tr(known).into()
}
fn metadata_value(key: &str, value: &ControlValue) -> String {
    let translate = |s: &str| match s {
        "standard" => tr("표준").to_owned(),
        "plus" => "Plus".into(),
        "pro" => "Pro".into(),
        "v1" => format!("{} 1", tr("버전")),
        "v2" => format!("{} 2", tr("버전")),
        _ => s.into(),
    };
    match value {
        ControlValue::Text(v) if key == "modelSpecialty" => format!("{} · {v}", tr("제공자 정의")),
        ControlValue::Text(v) => translate(v),
        ControlValue::Strings(v) => v
            .iter()
            .map(|v| translate(v))
            .collect::<Vec<_>>()
            .join(" · "),
        _ => value_label(Some(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn model_comparison_is_compact_localized_and_preserves_native_selection() {
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for dark in [true, false] {
                    for width in [320., 760.] {
                        let models = [
                            (
                                "gpt-6.1-sol",
                                "Latest workhorse model for coding and everyday work.",
                                "low",
                            ),
                            (
                                "gpt-6-astra",
                                "Frontier intelligence for the most demanding work.",
                                "medium",
                            ),
                            ("gpt-6.1-terra", "GPT-6.1 Terra Codex model.", "low"),
                        ]
                        .into_iter()
                        .map(|(id, description, effort)| ModelMetadata {
                            id: id.into(),
                            display_name: id.into(),
                            description: description.into(),
                            resolved_model: Some(id.into()),
                            default_effort: Some(effort.into()),
                            modalities: vec!["text".into(), "image".into()],
                            availability: BTreeMap::from([
                                (
                                    "accessPrograms".into(),
                                    ControlValue::Strings(vec!["standard".into()]),
                                ),
                                ("upgrade".into(), ControlValue::Default),
                            ]),
                            ..Default::default()
                        })
                        .collect::<Vec<_>>();
                        let option = ControlOption {
                            id: "model".into(),
                            label: "model".into(),
                            description: String::new(),
                            category: None,
                            kind: ControlKind::Select,
                            choices: models
                                .iter()
                                .map(|m| ControlChoice {
                                    value: m.id.clone(),
                                    label: m.id.clone(),
                                    description: m.description.clone(),
                                    group: None,
                                })
                                .collect(),
                            current: Some(ControlValue::Text("gpt-6-astra".into())),
                            scope: ControlScope::Live,
                            available: true,
                            reason: None,
                            source: None,
                            documentation: None,
                        };
                        let mut initialized = false;
                        let mut h = Harness::builder()
                            .with_size([width, if width < 400. { 720. } else { 560. }])
                            .build_ui_state(
                                move |ui, output: &mut Option<ControlValue>| {
                                    if !initialized {
                                        kiln_common::fonts::install(ui.ctx());
                                        initialized = true;
                                        return;
                                    }
                                    Theme::set_current(if dark {
                                        "kiln-dark"
                                    } else {
                                        "kiln-light"
                                    });
                                    Theme::current().apply(ui.ctx());
                                    ui.heading(tr("모델 비교"));
                                    if let Some(value) = catalog(
                                        ui,
                                        &models,
                                        true,
                                        option.current.as_ref(),
                                        Some(&option),
                                        true,
                                        "",
                                    ) {
                                        *output = Some(value);
                                    }
                                },
                                None,
                            );
                        h.run();
                        assert!(h.query_by_label(tr("현재 사용 중")).is_some());
                        assert_eq!(
                            h.query_all_by_label("GPT-6 Astra").count(),
                            1,
                            "no repeated native ID"
                        );
                        assert!(
                            h.query_by_label("GPT-6 Astra").unwrap().rect().top()
                                < h.get_by_label("GPT-6.1 Sol").rect().top(),
                            "selected model appears first"
                        );
                        assert!(
                            h.query_by_label(tr("고난도 작업을 위한 고성능 모델"))
                                .is_some()
                        );
                        assert!(h.query_by_label("accessPrograms: standard").is_none());
                        for button in h.query_all_by_label(tr("이 모델 사용")) {
                            assert!(button.rect().right() <= width);
                        }
                        if let Some(dir) = std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {
                            let dir = PathBuf::from(dir);
                            std::fs::create_dir_all(&dir).unwrap();
                            h.render()
                                .unwrap()
                                .save(dir.join(format!(
                                    "model-cards-{}-{dark}-{width}.png",
                                    language.code()
                                )))
                                .unwrap();
                        }
                        h.query_all_by_label(tr("이 모델 사용"))
                            .next()
                            .unwrap()
                            .click();
                        h.run();
                        assert_eq!(*h.state(), Some(ControlValue::Text("gpt-6.1-sol".into())));
                    }
                }
            });
        }
    }
}
