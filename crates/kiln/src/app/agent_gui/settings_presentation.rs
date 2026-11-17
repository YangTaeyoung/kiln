//! Presentation-only names and accessible controls. Native values remain unchanged.
use super::*;

pub(crate) fn present_value(control: &str, value: &ControlValue) -> String {
    if let ControlValue::Sandbox(v) = value { return present_value("sandboxPolicy", &ControlValue::Text(v.policy_mode().into())); }
    if matches!(value,ControlValue::Default) { return tr("기본값").into(); }
    let ControlValue::Text(value) = value else { return value_label(Some(value)); };
    if let Some((title, _)) = choice_copy(control, value) { return title.into(); }
    if matches!(control, "model" | "advisorModel") {
        return model_title(value);
    }
    value.clone()
}
pub(crate) fn model_title(id: &str) -> String {
    if id == "default" {
        return tr("기본 모델").into();
    }
    for (id_value, name) in [("gpt-6.1-sol", "GPT-6.1 Sol"), ("gpt-6-astra", "GPT-6 Astra"), ("gpt-6.1-terra", "GPT-6.1 Terra"), ("gpt-6-sol", "GPT-6 Sol"), ("gpt-6-luna", "GPT-6 Luna")] {
        if id == id_value { return name.into(); }
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
    if metadata.id == "default" { return tr("기본 모델").into(); }
    if metadata.display_name.is_empty() || metadata.display_name == metadata.id { return model_title(&metadata.id); }
    if ["opus","sonnet","fable","haiku"].contains(&metadata.display_name.to_lowercase().as_str()) && let Some(resolved)=&metadata.resolved_model { return model_title(resolved); }
    metadata.display_name.clone()
}
pub(crate) fn choice_title(option: &ControlOption, choice: &ControlChoice) -> String {
    let id = semantic_id(option);
    if matches!(id, "model" | "advisorModel") {
        if choice.label.is_empty() || choice.label == choice.value {
            model_title(&choice.value)
        } else if choice.value == "default" {
            tr("기본 모델").into()
        } else {
            choice.label.clone()
        }
    } else {
        let title = present_value(id, &ControlValue::Text(choice.value.clone()));
        if title == choice.value && !choice.label.is_empty() {
            format!("{} · {}", tr("사용자 정의"), choice.label)
        } else {
            title
        }
    }
}
pub(crate) fn choice_row(
    ui: &Ui,
    option: &ControlOption,
    choice: &ControlChoice,
) -> egui::WidgetText {
    described_row(ui, &choice_title(option, choice), &choice_help(option, choice))
}
pub(crate) fn described_row(ui: &Ui, title: &str, description: &str) -> egui::WidgetText {
    let theme = Theme::current();
    let mut job = egui::text::LayoutJob::default();
    job.append(title, 0., egui::TextFormat { font_id: kiln_common::fonts::medium(13.), color: theme.text, ..Default::default() });
    job.append(&format!("\n{description}"), 0., egui::TextFormat { font_id: egui::FontId::proportional(12.), color: theme.text_dim, ..Default::default() });
    job.wrap.max_width = (ui.available_width() - 2. * ui.spacing().button_padding.x).min(ui.ctx().content_rect().width() - 64.).clamp(60., 360.);
    job.into()
}
pub(crate) fn described_choice(ui: &mut Ui, selected: bool, title: &str, description: &str) -> egui::Response {
    ui.set_max_width(360_f32.min(ui.ctx().content_rect().width() - 48.));
    let response = ui.add(egui::Button::selectable(selected, described_row(ui, title, description)).wrap());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), selected, title));
    response
}
pub(super) fn is_effort(option: &ControlOption) -> bool {
    matches!(
        option.id.as_str(),
        "effort" | "effortLevel" | "thought_level"
    ) || option.category.as_deref() == Some("thought_level")
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
