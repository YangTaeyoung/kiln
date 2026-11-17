//! Presentation-only names and accessible controls. Native values remain unchanged.
use super::*;

pub(crate) fn present_value(control: &str, value: &ControlValue) -> String {
    if matches!(value,ControlValue::Default) { return tr("기본값").into(); }
    let ControlValue::Text(value) = value else { return value_label(Some(value)); };
    if matches!(control, "permissionMode" | "collaborationMode" | "mode") {
        return tr(match value.as_str() {
            "auto" => "자동",
            "default" => "기본",
            "plan" => "계획",
            "acceptEdits" => "편집 자동 승인",
            "dontAsk" => "승인 요청 안 함",
            "bypassPermissions" => "권한 확인 생략",
            "ask" => "질문",
            "code" => "코딩",
            _ => value,
        })
        .into();
    }
    if matches!(control, "effort" | "effortLevel" | "thought_level") {
        return tr(match value.as_str() {
            "none" => "없음",
            "minimal" => "최소",
            "low" => "낮음",
            "medium" => "보통",
            "high" => "높음",
            "xhigh" => "매우 높음",
            "max" => "최대",
            "ultra" => "최상",
            _ => value,
        })
        .into();
    }
    if matches!(control, "model" | "advisorModel") {
        return model_title(value);
    }
    value.clone()
}
pub(crate) fn model_title(id: &str) -> String {
    if id == "default" {
        return tr("기본 모델").into();
    }
    let clean = id.trim_start_matches("claude-");
    for (family, title) in [
        ("opus", "Opus"),
        ("fable", "Fable"),
        ("sonnet", "Sonnet"),
        ("haiku", "Haiku"),
    ] {
        if clean == family {
            return title.into();
        }
        if let Some(version) = clean.strip_prefix(&format!("{family}-")) {
            let (version, context) = version
                .strip_suffix("[1m]")
                .map(|v| (v, " · 1M"))
                .unwrap_or((version, ""));
            if version
                .split('-')
                .all(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
            {
                return format!("{title} {}{context}", version.replace('-', "."));
            }
        }
    }
    id.into()
}
pub(crate) fn metadata_title(metadata: &ModelMetadata) -> String {
    if metadata.display_name.is_empty() { return model_title(&metadata.id); }
    if ["opus","sonnet","fable","haiku"].contains(&metadata.display_name.to_lowercase().as_str()) && let Some(resolved)=&metadata.resolved_model { return model_title(resolved); }
    metadata.display_name.clone()
}
pub(super) fn choice_title(option: &ControlOption, choice: &ControlChoice) -> String {
    if matches!(option.id.as_str(), "model" | "advisorModel") {
        if choice.label.is_empty() || choice.label == choice.value {
            model_title(&choice.value)
        } else if choice.value == "default" {
            tr("기본 모델").into()
        } else {
            choice.label.clone()
        }
    } else {
        let title = present_value(&option.id, &ControlValue::Text(choice.value.clone()));
        if title == choice.value && !choice.label.is_empty() {
            choice.label.clone()
        } else {
            title
        }
    }
}
pub(super) fn choice_row(
    ui: &Ui,
    option: &ControlOption,
    choice: &ControlChoice,
) -> egui::WidgetText {
    let theme = Theme::current();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &choice_title(option, choice),
        0.,
        egui::TextFormat {
            font_id: kiln_common::fonts::medium(13.),
            color: theme.text,
            ..Default::default()
        },
    );
    if !choice.description.is_empty() {
        job.append(
            &format!("\n{}", choice.description),
            0.,
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.5),
                color: theme.text_dim,
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = ui.available_width().clamp(120., 360.);
    job.wrap.max_rows = 3;
    job.into()
}
pub(super) fn is_effort(option: &ControlOption) -> bool {
    matches!(
        option.id.as_str(),
        "effort" | "effortLevel" | "thought_level"
    ) || option.category.as_deref() == Some("thought_level")
}
#[derive(Clone, PartialEq)]
struct SliderDraft {
    native: Option<ControlValue>,
    choices: Vec<String>,
    index: usize,
}
pub(super) fn effort_slider(
    ui: &mut Ui,
    option: &ControlOption,
    selected: &mut Option<ControlValue>,
    quick: bool,
) {
    let id = ui.id().with(("effort-draft", &option.id));
    let choices = option
        .choices
        .iter()
        .map(|c| c.value.clone())
        .collect::<Vec<_>>();
    let current = option
        .choices
        .iter()
        .position(|c| option.current == Some(ControlValue::Text(c.value.clone())));
    let initial = SliderDraft {
        native: option.current.clone(),
        choices: choices.clone(),
        index: current.unwrap_or(0),
    };
    let mut draft = ui
        .data_mut(|d| d.get_temp::<SliderDraft>(id))
        .filter(|s| s.native == initial.native && s.choices == choices)
        .unwrap_or(initial);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.;
        ui.horizontal(|ui| {
            let display = if current.is_some() {
                present_value(
                    &option.id,
                    &ControlValue::Text(choices[draft.index].clone()),
                )
            } else {
                value_label(option.current.as_ref())
            };
            ui.label(
                RichText::new(format!("{} · {display}", label(&option.id)))
                    .small()
                    .strong(),
            );
        });
        if choices.len() < 2 {
            ui.small(tr("추론 강도를 제공하는 모델을 선택하세요."));
            return;
        }
        let previous_width = ui.spacing().slider_width;
        let previous_fill = ui.visuals().selection.bg_fill;
        ui.visuals_mut().selection.bg_fill = if Theme::current().dark { egui::Color32::from_rgb(168,143,255) } else { egui::Color32::from_rgb(108,66,210) };
        ui.spacing_mut().slider_width = ui.available_width().min(if quick { 190. } else { 280. });
        let slider = ui.add(
            egui::Slider::new(&mut draft.index, 0..=choices.len() - 1)
                .integer()
                .show_value(false)
                .trailing_fill(true),
        );
        slider.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Slider, ui.is_enabled(), label(&option.id))
        });
        // A pointer drag previews locally; only release commits, keyboard/click commits once.
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            *selected = Some(ControlValue::Text(choices[draft.index].clone()));
        }
        slider.on_hover_text(
            option
                .choices
                .iter()
                .map(|c| present_value(&option.id, &ControlValue::Text(c.value.clone())))
                .collect::<Vec<_>>()
                .join(" · "),
        );
        ui.spacing_mut().slider_width = previous_width;
        ui.visuals_mut().selection.bg_fill = previous_fill;
    });
    ui.data_mut(|d| d.insert_temp(id, draft));
}
pub(super) fn ultracode_switch(
    ui: &mut Ui,
    _option: &ControlOption,
    selected: &mut Option<ControlValue>,
) {
    let t = Theme::current();
    let known = matches!(selected, Some(ControlValue::Bool(_)));
    let on = matches!(selected, Some(ControlValue::Bool(true)));
    let purple = if t.dark {
        egui::Color32::from_rgb(168, 143, 255)
    } else {
        egui::Color32::from_rgb(108, 66, 210)
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "Ultracode · {}",
                tr(if !known {
                    "확인 대기"
                } else if on {
                    "켜짐"
                } else {
                    "꺼짐"
                })
            ))
            .color(if on { purple } else { t.text_dim })
            .strong(),
        );
        let (rect, response) = ui.allocate_exact_size(egui::vec2(34., 20.), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, "Ultracode")
        });
        let progress = ui.ctx().animate_bool_with_time(response.id, on, 0.15);
        if on {
            for spread in [2., 4.] {
                ui.painter().rect_stroke(
                    rect.expand(spread),
                    12,
                    Stroke::new(1., purple.gamma_multiply(0.10)),
                    egui::StrokeKind::Outside,
                );
            }
        }
        ui.painter()
            .rect_filled(rect, 10, if on { purple } else { t.bg_hover });
        ui.painter().rect_stroke(
            rect,
            10,
            Stroke::new(1., if on { purple } else { t.border_strong }),
            egui::StrokeKind::Inside,
        );
        ui.painter().circle_filled(
            egui::pos2(rect.left() + 10. + 14. * progress, rect.center().y),
            7.,
            if on { egui::Color32::WHITE } else { t.text_dim },
        );
        widgets::focus_ring(ui, &response, 10);
        if response.clicked() {
            *selected = Some(ControlValue::Bool(!on));
        }
        response.on_hover_text(format!(
            "Ultracode · {}",
            tr(if !known {
                "확인 대기"
            } else if on {
                "켜짐"
            } else {
                "꺼짐"
            })
        ));
    });
}
pub(super) fn search_field(ui: &mut Ui, value: &mut String, hint: &str) {
    let t = Theme::current();
    Frame::new()
        .fill(t.bg_input)
        .stroke(Stroke::new(1., t.border))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 16.), egui::Sense::hover());
                kiln_common::icons::paint(
                    ui.painter(),
                    rect,
                    kiln_common::icons::Icon::Search,
                    t.text_dim,
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .frame(Frame::NONE)
                        .hint_text(hint)
                        .desired_width((ui.available_width() - 28.).max(30.)),
                )
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, hint));
                if !value.is_empty()
                    && ui
                        .small_button("×")
                        .on_hover_text(tr("검색 초기화"))
                        .clicked()
                {
                    value.clear();
                }
            });
        });
}
