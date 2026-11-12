use super::*;
use crate::connection::{AwsAuth, Connection, Preset};
use kiln_common::i18n::tr;

pub(super) struct Draft {
    pub tool: Tool,
    id: Option<String>,
    label: String,
    config: Connection,
    headers: Vec<(String, String)>,
    env: Vec<(String, String)>,
}

impl Draft {
    pub(super) fn new(tool: Tool) -> Self {
        Self {
            tool,
            id: None,
            label: String::new(),
            config: Connection {
                preset: Preset::Gateway,
                ..Default::default()
            },
            headers: Vec::new(),
            env: Vec::new(),
        }
    }
    pub(super) fn edit(mgr: &AccountManager, p: &Profile) -> anyhow::Result<Self> {
        let mut config = mgr.edit_connection(p.tool, &p.id)?;
        let headers = std::mem::take(&mut config.headers).into_iter().collect();
        let env = std::mem::take(&mut config.extra_env).into_iter().collect();
        Ok(Self {
            tool: p.tool,
            id: Some(p.id.clone()),
            label: p.label.clone(),
            config,
            headers,
            env,
        })
    }
}

pub(super) fn preset_label(preset: Preset) -> &'static str {
    match preset {
        Preset::ApiKey => tr("API 키"),
        Preset::Gateway => tr("게이트웨이"),
        Preset::Bedrock => "Amazon Bedrock",
        Preset::Custom => tr("사용자 지정"),
    }
}

fn field(ui: &mut Ui, label: &str, value: &mut String, secret: bool, hint: &str) {
    let label = ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(ui.available_width())
            .password(secret)
            .hint_text(hint),
    )
    .labelled_by(label.id);
}

fn pairs(ui: &mut Ui, title: &str, rows: &mut Vec<(String, String)>, hint: &str) {
    ui.collapsing(title, |ui| {
        let mut remove = None;
        for (i, (name, value)) in rows.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                field(ui, tr("이름"), name, false, hint);
                field(ui, tr("값"), value, true, "");
                if ui.small_button(tr("삭제")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            rows.remove(i);
        }
        if ui.small_button(tr("추가")).clicked() {
            rows.push(Default::default());
        }
    });
}

pub(super) fn form(ui: &mut Ui, mgr: &AccountManager, tool: Tool, busy: bool) {
    let draft = {
        let mut state = mgr.ui_state();
        if state
            .connection
            .as_ref()
            .is_none_or(|draft| draft.tool != tool)
        {
            return;
        }
        state.connection.take()
    };
    let Some(mut draft) = draft else {
        return;
    };
    let mut save = false;
    let mut cancel = false;
    let theme = Theme::current();
    egui::Frame::new()
        .fill(theme.bg_hover)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(tr(if draft.id.is_some() {
                "API 연결 편집"
            } else {
                "API 연결 추가"
            }));
            field(ui, tr("이름"), &mut draft.label, false, "");
            let c = &mut draft.config;
            let previous_preset = c.preset;
            let auth = ui.label(tr("인증 방식"));
            egui::ComboBox::from_id_salt("connection-preset")
                .selected_text(preset_label(c.preset))
                .show_ui(ui, |ui| {
                    for preset in [
                        Preset::Gateway,
                        Preset::ApiKey,
                        Preset::Bedrock,
                        Preset::Custom,
                    ] {
                        ui.selectable_value(&mut c.preset, preset, preset_label(preset));
                    }
                })
                .response
                .labelled_by(auth.id);
            if previous_preset != c.preset {
                c.key.clear();
                c.aws_access_id.clear();
                c.aws_session_token.clear();
                c.base_url.clear();
            }
            if c.preset == Preset::Gateway {
                let hint = if tool == Tool::Codex {
                    "https://gateway.example/v1"
                } else {
                    "https://gateway.example"
                };
                field(ui, "Base URL", &mut c.base_url, false, hint);
                if tool == Tool::Claude {
                    ui.checkbox(&mut c.x_api_key, tr("X-Api-Key 헤더 사용"));
                }
            }
            if c.preset == Preset::Bedrock {
                field(ui, tr("AWS 리전"), &mut c.region, false, "us-east-1");
                let previous_auth = c.aws_auth;
                let mode_label = |auth| match auth {
                    AwsAuth::Profile => tr("AWS 프로필"),
                    AwsAuth::Bearer => tr("Bedrock API 키"),
                    AwsAuth::AccessKey => tr("AWS 액세스 키"),
                };
                egui::ComboBox::from_id_salt("connection-aws")
                    .selected_text(mode_label(c.aws_auth))
                    .show_ui(ui, |ui| {
                        for auth in [AwsAuth::Profile, AwsAuth::Bearer, AwsAuth::AccessKey] {
                            ui.selectable_value(&mut c.aws_auth, auth, mode_label(auth));
                        }
                    });
                if c.aws_auth != previous_auth {
                    c.key.clear();
                    c.aws_access_id.clear();
                    c.aws_session_token.clear();
                }
                if tool == Tool::Claude {
                    field(ui, "Base URL", &mut c.base_url, false, tr("선택 사항"));
                }
                if c.aws_auth == AwsAuth::Profile {
                    field(ui, tr("AWS 프로필"), &mut c.aws_profile, false, "default");
                } else if c.aws_auth == AwsAuth::AccessKey {
                    field(ui, "AWS Access Key ID", &mut c.aws_access_id, true, "");
                    field(
                        ui,
                        "AWS Session Token",
                        &mut c.aws_session_token,
                        true,
                        tr("선택 사항"),
                    );
                }
            }
            if matches!(c.preset, Preset::Gateway | Preset::ApiKey)
                || c.preset == Preset::Bedrock && c.aws_auth != AwsAuth::Profile
            {
                let label = if c.preset == Preset::Bedrock && c.aws_auth == AwsAuth::AccessKey {
                    "AWS Secret Access Key"
                } else {
                    tr("API 키")
                };
                field(ui, label, &mut c.key, true, "");
            }
            field(ui, tr("모델"), &mut c.model, false, tr("선택 사항"));
            if tool == Tool::Claude || matches!(c.preset, Preset::ApiKey | Preset::Gateway) {
                pairs(ui, tr("추가 헤더"), &mut draft.headers, "X-Team");
            }
            pairs(ui, tr("추가 환경 변수"), &mut draft.env, "NAME");
            ui.horizontal_wrapped(|ui| {
                save = widgets::button_with(
                    ui,
                    Some(Icon::Save),
                    tr("저장"),
                    ButtonKind::Primary,
                    !busy && !draft.label.trim().is_empty(),
                )
                .clicked();
                cancel =
                    widgets::button_with(ui, None, tr("취소"), ButtonKind::Ghost, true).clicked();
            });
        });
    if save {
        let Draft {
            id,
            label,
            mut config,
            headers,
            env,
            ..
        } = draft;
        if tool == Tool::Claude || matches!(config.preset, Preset::ApiKey | Preset::Gateway) {
            config.headers = headers
                .into_iter()
                .filter(|(name, value)| !name.is_empty() || !value.is_empty())
                .collect();
        }
        config.extra_env = env
            .into_iter()
            .filter(|(name, value)| !name.is_empty() || !value.is_empty())
            .collect();
        let notice = tr("API 연결을 저장했습니다").to_owned();
        mgr.run_async(tool, tr("저장 중…"), move |mgr| {
            match mgr.save_connection(tool, id.as_deref(), &label, config.clone()) {
                Ok(_) => Ok(notice),
                Err(error) => {
                    let mut draft = Draft {
                        tool,
                        id,
                        label,
                        config,
                        headers: Vec::new(),
                        env: Vec::new(),
                    };
                    draft.headers = std::mem::take(&mut draft.config.headers)
                        .into_iter()
                        .collect();
                    draft.env = std::mem::take(&mut draft.config.extra_env)
                        .into_iter()
                        .collect();
                    let mut state = mgr.ui_state();
                    if state.connection.is_none() {
                        state.connection = Some(draft);
                    }
                    Err(error)
                }
            }
        });
    } else if !cancel {
        mgr.ui_state().connection = Some(draft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn gateway_form_fits_narrow_scaled_settings_in_four_languages() {
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for tool in [Tool::Claude, Tool::Codex] {
                    let root = tempfile::tempdir().unwrap();
                    let (env, _) = crate::Env::sandbox(root.path(), false);
                    let mgr = AccountManager::with_env(env);
                    mgr.ui_state().connection = Some(Draft::new(tool));
                    let mut h = egui_kittest::Harness::builder()
                        .with_size(vec2(420.0, 760.0))
                        .build_ui_state(
                            move |ui, mgr: &mut AccountManager| {
                                if super::super::tests::test_fonts(ui) {
                                    form(ui, mgr, tool, false);
                                }
                            },
                            mgr,
                        );
                    h.ctx.set_fonts(kiln_common::fonts::definitions(false));
                    h.ctx.set_zoom_factor(1.3);
                    for _ in 0..4 {
                        h.step();
                    }
                    for label in ["API 연결 추가", "Base URL", "API 키", "취소"] {
                        let node = h.get_by_label(tr(label));
                        assert!(
                            h.ctx.content_rect().contains_rect(node.rect()),
                            "{language:?}: {label} overflow"
                        );
                    }
                }
            });
        }
    }
}
