//! Safe native control contracts. No provider settings/configuration objects cross this boundary.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ControlValue {
    Default,
    Text(String),
    Bool(bool),
    Integer(u64),
    Strings(Vec<String>),
    Sandbox(NativeSandbox),
    GranularApproval(GranularApproval),
}
impl ControlValue {
    pub(crate) fn wire(&self) -> Value {
        match self {
            Self::Default => Value::Null,
            Self::Text(v) => v.clone().into(),
            Self::Bool(v) => (*v).into(),
            Self::Integer(v) => (*v).into(),
            Self::Strings(v) => serde_json::json!(v),
            Self::Sandbox(v) => serde_json::json!(v),
            Self::GranularApproval(v) => serde_json::json!({"granular":v}),
        }
    }
    pub(crate) fn from_safe(v: &Value) -> Option<Self> {
        match v {
            Value::Null => Some(Self::Default),
            Value::Bool(v) => Some(Self::Bool(*v)),
            Value::String(v) if v.len() <= 4096 => Some(Self::Text(v.clone())),
            Value::Number(v) => v.as_u64().map(Self::Integer),
            Value::Array(v)
                if v.len() <= 128
                    && v.iter()
                        .all(|v| v.as_str().is_some_and(|s| s.len() <= 4096)) =>
            {
                Some(Self::Strings(
                    v.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                ))
            }
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ControlKind {
    Select,
    Boolean,
    Text,
    Integer { min: u64, max: u64 },
    Strings,
    Sandbox,
    GranularApproval,
    Unsupported(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ControlScope {
    Live,
    NewConversation,
    Inherited,
    Managed,
    Terminal,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlChoice {
    pub value: String,
    pub label: String,
    pub description: String,
    pub group: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub category: Option<String>,
    pub kind: ControlKind,
    pub choices: Vec<ControlChoice>,
    pub current: Option<ControlValue>,
    pub scope: ControlScope,
    pub available: bool,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub documentation: Option<String>,
}
impl ControlOption {
    pub(crate) fn validate(&self, value: &ControlValue) -> Result<()> {
        ensure!(
            self.available && self.scope == ControlScope::Live,
            "This control is unavailable in the current session"
        );
        let valid = match (&self.kind, value) {
            (ControlKind::Sandbox, ControlValue::Sandbox(v)) => v.valid(),
            (ControlKind::GranularApproval, ControlValue::GranularApproval(_)) => true,
            (_, ControlValue::Default) => self.source.as_deref() != Some("acp"),
            (ControlKind::Select, ControlValue::Text(v)) => {
                self.choices.iter().any(|c| c.value == *v)
            }
            (ControlKind::Boolean, ControlValue::Bool(_)) => true,
            (ControlKind::Text, ControlValue::Text(v)) => v.len() <= 4096 && !v.contains('\0'),
            (ControlKind::Integer { min, max }, ControlValue::Integer(v)) => v >= min && v <= max,
            (ControlKind::Strings, ControlValue::Strings(v)) => {
                v.len() <= 128 && v.iter().all(|s| s.len() <= 4096 && !s.contains('\0'))
            }
            _ => false,
        };
        ensure!(
            valid,
            "Value is not advertised or supported for this control"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelMetadata {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub description: String,
    pub resolved_model: Option<String>,
    pub efforts: Vec<ControlChoice>,
    pub default_effort: Option<String>,
    pub tiers: Vec<ControlChoice>,
    pub default_tier: Option<String>,
    pub modalities: Vec<String>,
    #[serde(default)]
    pub availability: BTreeMap<String, ControlValue>,
    pub capabilities: BTreeMap<String, bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CatalogKind {
    Commands,
    Skills,
    Agents,
    Styles,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NativeEntry {
    #[serde(default)]
    pub interface: BTreeMap<String, String>,
    #[serde(default)]
    pub builtin: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub path: Option<String>,
    pub terminal_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Discovery {
    Settings,
    Commands,
    Skills,
    Mcp,
    Context,
    Policy,
    Status,
    Goal,
    Queue,
    Memory,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum NativeAction {
    ReloadSkills,
    ReloadPlugins,
    ReloadStyles,
    Compact,
    Codex(CodexAction),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ApplicationState {
    Pending,
    Applied,
    Acknowledged,
    Rejected,
    Cancelled,
    Unconfirmed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlOutcome {
    /// False retains the serialization barrier even after Stop or timeout.
    #[serde(default)]
    pub settled: bool,
    pub operation: String,
    pub control: String,
    pub requested: ControlValue,
    pub effective: Option<ControlValue>,
    pub state: ApplicationState,
    pub message: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct NativeStatus {
    pub subject: String,
    pub fields: BTreeMap<String, ControlValue>,
    pub unavailable: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ControlEvent {
    /// A complete ordered replacement, including an empty vector.
    Options(Vec<ControlOption>),
    Coverage(CatalogCoverage),
    Capabilities(Vec<NativeCapability>),
    Queue(Vec<QueuedTask>),
    Models(Vec<ModelMetadata>),
    ModelSnapshot {
        choices: Vec<Model>,
        selected: Option<String>,
    },
    ModeSnapshot {
        choices: Vec<Choice>,
        selected: Option<String>,
    },
    Catalog {
        kind: CatalogKind,
        entries: Vec<NativeEntry>,
    },
    Outcome(ControlOutcome),
    Settlement(String),
    Status(NativeStatus),
    /// Replacement for legacy quick selectors; unlike old Models/Modes, empty clears.
    Quick {
        models: Vec<Model>,
        selected_model: Option<String>,
        modes: Vec<Choice>,
        selected_mode: Option<String>,
    },
}
#[derive(Clone, Debug, Default)]
pub struct NativeControls {
    pub coverage: BTreeMap<String, CatalogCoverage>,
    pub options_reported: bool,
    pub models_reported: bool,
    pub capabilities: Vec<NativeCapability>,
    pub queue: Option<Vec<QueuedTask>>,
    pub options: Vec<ControlOption>,
    pub models: Vec<ModelMetadata>,
    pub commands: Option<Vec<NativeEntry>>,
    pub skills: Option<Vec<NativeEntry>>,
    pub agents: Option<Vec<NativeEntry>>,
    pub styles: Option<Vec<NativeEntry>>,
    pub outcomes: BTreeMap<String, ControlOutcome>,
    pub status: BTreeMap<String, NativeStatus>,
}
impl NativeControls {
    pub(crate) fn apply(&mut self, event: &ControlEvent) {
        match event {
            ControlEvent::Settlement(id) => {
                if let Some(outcome) = self.outcomes.get_mut(id) {
                    outcome.settled = true;
                }
            }
            ControlEvent::Coverage(v) => {
                self.coverage.insert(v.subject.clone(), v.clone());
            }
            ControlEvent::Capabilities(v) => self.capabilities = v.clone(),
            ControlEvent::Queue(v) => self.queue = Some(v.clone()),
            ControlEvent::Options(v) => {
                self.options = v.clone();
                self.options_reported = true;
            }
            ControlEvent::Models(v) => {
                self.models = v.clone();
                self.models_reported = true;
            }
            ControlEvent::Catalog { kind, entries } => {
                *match kind {
                    CatalogKind::Commands => &mut self.commands,
                    CatalogKind::Skills => &mut self.skills,
                    CatalogKind::Agents => &mut self.agents,
                    CatalogKind::Styles => &mut self.styles,
                } = Some(entries.clone())
            }
            ControlEvent::Outcome(v) => {
                self.outcomes.insert(v.operation.clone(), v.clone());
            }
            ControlEvent::Status(v) => {
                self.status.insert(v.subject.clone(), v.clone());
            }
            ControlEvent::Quick { .. }
            | ControlEvent::ModelSnapshot { .. }
            | ControlEvent::ModeSnapshot { .. } => {}
        }
    }
}
pub(crate) fn bounded(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(2048)
        .collect()
}
pub(crate) fn native_entries(v: &Value) -> Vec<NativeEntry> {
    v.as_array()
        .into_iter()
        .flatten()
        .take(512)
        .map(|v| NativeEntry {
            interface: [
                "displayName",
                "shortDescription",
                "defaultPrompt",
                "brandColor",
                "iconSmall",
                "iconLarge",
                "iconSmallUrl",
                "iconLargeUrl",
            ]
            .into_iter()
            .filter_map(|key| {
                v["interface"][key]
                    .as_str()
                    .map(|value| (key.into(), value.chars().take(2048).collect()))
            })
            .collect(),
            builtin: v["builtin"].as_bool(),
            enabled: v["enabled"].as_bool(),
            scope: v["scope"].as_str().map(str::to_owned),
            origin: v["pluginId"].as_str().map(str::to_owned),
            model: v["model"].as_str().map(str::to_owned),
            aliases: v["aliases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(32)
                .map(str::to_owned)
                .collect(),
            name: v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| bounded(v, "name")),
            description: bounded(v, "description"),
            argument_hint: v
                .get("argumentHint")
                .or_else(|| v.pointer("/input/hint"))
                .and_then(Value::as_str)
                .map(|v| v.chars().take(512).collect()),
            path: None,
            terminal_only: false,
        })
        .filter(|v| !v.name.is_empty())
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_empty_catalog_clears_but_old_selection_preserves() {
        let mut t = Transcript::default();
        for (sequence, event) in [
            Event::Generation,
            Event::Controls(ControlEvent::Catalog {
                kind: CatalogKind::Commands,
                entries: vec![NativeEntry {
                    interface: BTreeMap::new(),
                    builtin: None,
                    enabled: None,
                    scope: None,
                    origin: None,
                    model: None,
                    aliases: vec![],
                    name: "foo".into(),
                    description: String::new(),
                    argument_hint: None,
                    path: None,
                    terminal_only: false,
                }],
            }),
            Event::Controls(ControlEvent::Catalog {
                kind: CatalogKind::Commands,
                entries: vec![],
            }),
        ]
        .into_iter()
        .enumerate()
        {
            t.apply(&Envelope {
                sequence: sequence as u64 + 1,
                generation: "g".into(),
                event,
            });
        }
        assert_eq!(t.controls.commands, Some(vec![]));
    }
    #[test]
    fn projection_rejects_secret_objects() {
        assert!(ControlValue::from_safe(&serde_json::json!({"env":{"SECRET":"canary"}})).is_none());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NativeSkillInput {
    pub name: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PermissionScope {
    Turn,
    Session,
}
/// Indices reference the matching live request, never caller-supplied permission objects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PermissionGrant {
    pub entry_indices: Vec<usize>,
    pub read_indices: Vec<usize>,
    pub write_indices: Vec<usize>,
    pub network: bool,
    pub scope: PermissionScope,
    pub strict_review: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CodexAction {
    SetGoal {
        objective: Option<String>,
        status: Option<GoalStatus>,
        token_budget: Option<u64>,
    },
    ClearGoal,
    AddQueuedTask {
        text: String,
    },
    UpdateQueuedTask {
        id: String,
        text: String,
    },
    DeleteQueuedTask {
        id: String,
    },
    StartQueuedTask {
        id: String,
    },
    Review(ReviewTarget),
    UpdateActiveTurn {
        turn_id: String,
        control: String,
        value: ControlValue,
    },
    AcceptEffectiveSettings {
        operation: String,
    },
    SetMemoryEnabled(bool),
    ReloadMcp,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ReviewTarget {
    UncommittedChanges,
    BaseBranch { branch: String },
    Commit { sha: String },
    Custom { instructions: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueuedTask {
    pub id: String,
    pub text: String,
    pub editable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NativeCapability {
    pub id: String,
    pub label: String,
    pub scope: ControlScope,
    pub available: bool,
    pub description: String,
    pub documentation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeSandbox {
    ReadOnly {
        #[serde(rename = "networkAccess")]
        network_access: bool,
    },
    WorkspaceWrite {
        #[serde(rename = "networkAccess")]
        network_access: bool,
        #[serde(rename = "writableRoots")]
        writable_roots: Vec<PathBuf>,
        #[serde(rename = "excludeSlashTmp")]
        exclude_slash_tmp: bool,
        #[serde(rename = "excludeTmpdirEnvVar")]
        exclude_tmpdir_env_var: bool,
    },
    DangerFullAccess,
    ExternalSandbox {
        #[serde(rename = "networkAccess")]
        network_access: ExternalNetworkAccess,
    },
}
impl NativeSandbox {
    pub(crate) fn valid(&self) -> bool {
        match self {
            Self::WorkspaceWrite { writable_roots, .. } => {
                writable_roots.len() <= 128 && writable_roots.iter().all(|p| p.is_absolute())
            }
            _ => true,
        }
    }
    pub(crate) fn policy_mode(&self) -> &str {
        match self {
            Self::ReadOnly { .. } => "read-only",
            Self::WorkspaceWrite { .. } => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
            Self::ExternalSandbox { .. } => "external-sandbox",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExternalNetworkAccess {
    Restricted,
    Enabled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GranularApproval {
    pub sandbox_approval: bool,
    pub rules: bool,
    pub mcp_elicitations: bool,
    #[serde(default)]
    pub request_permissions: bool,
    #[serde(default)]
    pub skill_approval: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CatalogCoverage {
    pub subject: String,
    pub received: usize,
    pub retained: usize,
    pub partial: bool,
    pub reason: Option<String>,
}
