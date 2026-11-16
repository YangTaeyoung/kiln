//! Explicit new-conversation options. Never re-applied by a resumed host.
use super::*;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum LaunchOptions {
    #[default]
    Inherited,
    Claude(ClaudeLaunch),
    Codex(CodexLaunch),
    Antigravity(AntigravityLaunch),
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ClaudeLaunch {
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    pub auto_compact_window: Option<u32>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub setting_sources: Option<Vec<ClaudeSettingSource>>,
    pub tools: Option<Vec<String>>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeSettingSource {
    User,
    Project,
    Local,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CodexLaunch {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub permissions: Option<String>,
    pub summary: Option<ReasoningSummary>,
    pub verbosity: Option<Verbosity>,
    pub web_search: Option<WebSearch>,
    pub context_window: Option<u64>,
    pub auto_compact_token_limit: Option<u64>,
    pub developer_instructions: Option<String>,
    pub base_instructions: Option<String>,
    pub ephemeral: Option<bool>,
    pub workspace_roots: Vec<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
    None,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WebSearch {
    Disabled,
    Cached,
    Live,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AntigravityLaunch {
    pub sandbox: bool,
    pub agent: Option<String>,
    pub additional_directories: Vec<PathBuf>,
}
impl LaunchOptions {
    pub(crate) fn validate(&self, tool: Tool) -> Result<()> {
        ensure!(
            matches!(
                (self, tool),
                (Self::Inherited, _)
                    | (Self::Claude(_), Tool::Claude)
                    | (Self::Codex(_), Tool::Codex)
                    | (Self::Antigravity(_), Tool::Antigravity)
            ),
            "Launch options do not match the provider"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= 64_000,
            "Launch options exceed the size limit"
        );
        match self {
            Self::Claude(v) => {
                ensure!(
                    v.max_turns.is_none_or(|v| v > 0)
                        && v.max_budget_usd.is_none_or(|v| v.is_finite() && v > 0.0),
                    "Invalid session budget"
                );
                ensure!(
                    v.auto_compact_window
                        .is_none_or(|v| (100_000..=1_000_000).contains(&v)),
                    "Invalid compaction window"
                );
            }
            Self::Codex(v) => {
                ensure!(
                    (v.effort.is_none() && v.service_tier.is_none()) || v.model.is_some(),
                    "Choose an explicit model for startup effort or service tier"
                );
                ensure!(
                    v.workspace_roots.iter().all(|v| v.is_absolute()),
                    "Workspace roots must be absolute"
                );
                ensure!(
                    v.context_window.is_none_or(|v| v > 0)
                        && v.auto_compact_token_limit.is_none_or(|v| v > 0),
                    "Invalid context budget"
                );
            }
            Self::Antigravity(v) => ensure!(
                v.additional_directories.iter().all(|v| v.is_absolute()),
                "Additional directories must be absolute"
            ),
            Self::Inherited => {}
        }
        Ok(())
    }
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut args = vec![];
        match self {
            Self::Claude(v) => {
                for (flag, value) in [
                    ("--max-turns", v.max_turns.map(|v| v.to_string())),
                    ("--max-budget-usd", v.max_budget_usd.map(|v| v.to_string())),
                    (
                        "--autocompact",
                        v.auto_compact_window.map(|v| v.to_string()),
                    ),
                    ("--system-prompt", v.system_prompt.clone()),
                    ("--append-system-prompt", v.append_system_prompt.clone()),
                    (
                        "--setting-sources",
                        v.setting_sources.as_ref().map(|v| {
                            v.iter()
                                .map(|v| match v {
                                    ClaudeSettingSource::User => "user",
                                    ClaudeSettingSource::Project => "project",
                                    ClaudeSettingSource::Local => "local",
                                })
                                .collect::<Vec<_>>()
                                .join(",")
                        }),
                    ),
                    ("--tools", v.tools.as_ref().map(|v| v.join(","))),
                ] {
                    if let Some(value) = value {
                        args.extend([flag.into(), value]);
                    }
                }
                for (flag, values) in [
                    ("--allowedTools", &v.allowed_tools),
                    ("--disallowedTools", &v.disallowed_tools),
                ] {
                    if !values.is_empty() {
                        args.extend([flag.into(), values.join(",")]);
                    }
                }
            }
            Self::Antigravity(v) => {
                if v.sandbox {
                    args.push("--sandbox".into());
                }
                if let Some(agent) = &v.agent {
                    args.extend(["--agent".into(), agent.clone()]);
                }
                for dir in &v.additional_directories {
                    args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                }
            }
            _ => {}
        }
        args
    }
    pub(crate) fn codex_params(&self) -> serde_json::Value {
        let mut p = serde_json::json!({});
        if let Self::Codex(v) = self {
            for (key, value) in [
                ("model", &v.model),
                ("serviceTier", &v.service_tier),
                ("permissions", &v.permissions),
                ("baseInstructions", &v.base_instructions),
                ("developerInstructions", &v.developer_instructions),
            ] {
                if let Some(value) = value {
                    p[key] = serde_json::json!(value);
                }
            }
            if let Some(ephemeral) = v.ephemeral {
                p["ephemeral"] = serde_json::json!(ephemeral);
            }
            if !v.workspace_roots.is_empty() {
                p["runtimeWorkspaceRoots"] = serde_json::json!(v.workspace_roots);
            }
            let mut config = serde_json::json!({});
            for (key, value) in [
                ("model_reasoning_effort", serde_json::json!(v.effort)),
                ("model_reasoning_summary", serde_json::json!(v.summary)),
                ("model_verbosity", serde_json::json!(v.verbosity)),
                ("web_search", serde_json::json!(v.web_search)),
                ("model_context_window", serde_json::json!(v.context_window)),
                (
                    "model_auto_compact_token_limit",
                    serde_json::json!(v.auto_compact_token_limit),
                ),
            ] {
                if !value.is_null() {
                    config[key] = value;
                }
            }
            if config.as_object().is_some_and(|v| !v.is_empty()) {
                p["config"] = config;
            }
        }
        p
    }
    pub(crate) fn metadata(&self) -> NativeStatus {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            "scope".into(),
            ControlValue::Text(
                "New conversation launch arguments; native effective state is reported separately"
                    .into(),
            ),
        );
        fields.insert(
            "custom_options".into(),
            ControlValue::Bool(!matches!(self, Self::Inherited)),
        );
        match self {
            Self::Claude(v) => {
                if let Some(n) = v.max_turns {
                    fields.insert("max_turns".into(), ControlValue::Integer(n as u64));
                }
                if let Some(n) = v.auto_compact_window {
                    fields.insert(
                        "auto_compact_window".into(),
                        ControlValue::Integer(n as u64),
                    );
                }
                fields.insert(
                    "system_instructions_supplied".into(),
                    ControlValue::Bool(
                        v.system_prompt.is_some() || v.append_system_prompt.is_some(),
                    ),
                );
            }
            Self::Codex(v) => {
                fields.insert(
                    "system_instructions_supplied".into(),
                    ControlValue::Bool(
                        v.base_instructions.is_some() || v.developer_instructions.is_some(),
                    ),
                );
            }
            Self::Antigravity(v) => {
                fields.insert("sandbox_requested".into(), ControlValue::Bool(v.sandbox));
            }
            _ => {}
        }
        NativeStatus {
            subject: "launch".into(),
            fields,
            unavailable: None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_options_are_typed_and_do_not_leak_instructions() {
        let v = LaunchOptions::Claude(ClaudeLaunch {
            system_prompt: Some("secret-canary".into()),
            max_turns: Some(3),
            ..Default::default()
        });
        assert!(v.validate(Tool::Codex).is_err());
        assert!(v.arguments().contains(&"secret-canary".into()));
        assert!(
            !serde_json::to_string(&v.metadata())
                .unwrap()
                .contains("secret-canary")
        );
    }
}
