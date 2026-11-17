//! SDK 0.3.325 typed session flag layer; never calls update_settings (file writes).
use super::*;
impl Claude {
    pub(super) fn discover(&mut self, target: Discovery, e: &mut Effects) -> String {
        let request = match &target {
            Discovery::TaskOutput {task_id,..} => json!({"subtype":"get_task_output","task_id":task_id}),
            Discovery::Goal | Discovery::Queue | Discovery::Memory => {
                e.event(unavailable("native_discovery"));
                return String::new();
            }
            Discovery::Settings => json!({"subtype":"get_settings"}),
            Discovery::Mcp => json!({"subtype":"mcp_status"}),
            Discovery::Context => json!({"subtype":"get_context_usage","detail":"summary"}),
            Discovery::Policy => json!({"subtype":"list_permission_rules"}),
            Discovery::Status => json!({"subtype":"get_settings"}),
            Discovery::Skills => json!({"subtype":"reload_skills"}),
            Discovery::Commands => json!({"subtype":"initialize"}),
        };
        let id = self.native.read(target);
        e.write(Self::control(&id, request));
        id
    }
    pub(super) fn catalog(&mut self, r: &Value, e: &mut Effects) {
        for (key, kind) in [
            ("commands", CatalogKind::Commands),
            ("skills", CatalogKind::Skills),
            ("agents", CatalogKind::Agents),
            ("available_output_styles", CatalogKind::Styles),
        ] {
            if let Some(v) = r.get(key) {
                let mut entries = controls::native_entries(v);
                e.event(coverage(key, v, entries.len(), false));
                if kind == CatalogKind::Commands {
                    for entry in &mut entries {
                        entry.terminal_only = self.terminal_commands.contains(&entry.name);
                    }
                    self.command_catalog = entries.clone();
                }
                if kind == CatalogKind::Styles {
                    self.styles = entries.iter().map(|v| v.name.clone()).collect();
                }
                if kind == CatalogKind::Agents {
                    self.agents = entries.iter().map(|v| v.name.clone()).collect();
                }
                e.event(Event::Controls(ControlEvent::Catalog { kind, entries }));
            }
        }
        if let Some(models) = r.get("models").and_then(Value::as_array) {
            self.metadata = models
                .iter()
                .take(256)
                .map(|v| ModelMetadata {
                    id: s(v, "value"),
                    display_name: controls::bounded(v, "displayName"),
                    description: controls::bounded(v, "description"),
                    resolved_model: v["resolvedModel"].as_str().map(str::to_owned),
                    efforts: v["supportedEffortLevels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(choice)
                        .collect(),
                    capabilities: [
                        "supportsEffort",
                        "supportsAdaptiveThinking",
                        "supportsFastMode",
                        "supportsAutoMode",
                        "supportsThinkingOff",
                    ]
                    .into_iter()
                    .filter_map(|key| v[key].as_bool().map(|v| (key.into(), v)))
                    .collect(),
                    ..Default::default()
                })
                .collect();
            e.event(coverage("models", &r["models"], self.metadata.len(), false));
            e.event(Event::Controls(ControlEvent::Models(self.metadata.clone())));
            e.event(Event::Controls(ControlEvent::ModelSnapshot {
                choices: model_choices(&r["models"], "value", "displayName"),
                selected: self.selected.clone(),
            }));
        }
        e.event(status("cli", r, &["claude_code_version"]));
        e.event(status(
            "claude_availability",
            r,
            &[
                "fast_mode_state",
                "fast_mode_disabled_reason",
                "current_permission_mode",
            ],
        ));
        if let Some(mode) = r["current_permission_mode"].as_str() {
            self.permission = Some(mode.into());
        }
        self.refresh_options(e);
    }
    pub(super) fn refresh_options(&mut self, e: &mut Effects) {
        let catalog: Value = serde_json::from_str(include_str!("claude_settings_catalog.json"))
            .expect("validated static catalog");
        let mut options: Vec<ControlOption> = catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let id = s(row, "key");
                let note = s(row, "note");
                let scope = if note.contains("Managed policy") {
                    ControlScope::Managed
                } else if note.contains("terminal/interface") {
                    ControlScope::Terminal
                } else if ["autoCompactWindow", "systemPrompt"].contains(&id.as_str()) {
                    ControlScope::NewConversation
                } else {
                    ControlScope::Inherited
                };
                ControlOption {
                    id: id.clone(),
                    label: id.clone(),
                    description: note,
                    category: Some("Native settings inventory".into()),
                    kind: ControlKind::Unsupported(s(row, "type")),
                    choices: vec![],
                    current: None,
                    scope,
                    available: false,
                    reason: Some("Inherited native setting; live editing is not verified.".into()),
                    source: self.sources.get(&id).cloned(),
                    documentation: Some(format!(
                        "https://code.claude.com/docs/en/settings-reference.md#{}",
                        id.to_lowercase()
                    )),
                }
            })
            .collect();
        let model = self.metadata.iter().find(|m| {
            Some(&m.id) == self.selected.as_ref()
                || self
                    .selected
                    .as_ref()
                    .is_some_and(|selected| m.resolved_model.as_ref() == Some(selected))
        });
        let mut live = vec![
            option(
                "model",
                ControlKind::Select,
                self.metadata.iter().map(|m| ControlChoice {
                    value: m.id.clone(),
                    label: if m.display_name.is_empty() { m.id.clone() } else { m.display_name.clone() },
                    description: m.description.clone(),
                    group: None,
                }).collect(),
            ),
            option(
                "permissionMode",
                ControlKind::Select,
                ["default", "plan", "acceptEdits", "auto", "dontAsk"]
                    .into_iter()
                    .map(choice)
                    .collect(),
            ),
            option(
                "effortLevel",
                ControlKind::Select,
                model.map(|m| m.efforts.clone()).unwrap_or_default(),
            ),
            option("ultracode", ControlKind::Boolean, vec![]),
            option("fastMode", ControlKind::Boolean, vec![]),
            option("enableWorkflows", ControlKind::Boolean, vec![]),
            option(
                "workflowKeywordTriggerEnabled",
                ControlKind::Boolean,
                vec![],
            ),
            option(
                "workflowSizeGuideline",
                ControlKind::Select,
                ["small", "medium", "large", "unrestricted"]
                    .into_iter()
                    .map(choice)
                    .collect(),
            ),
            option(
                "outputStyle",
                ControlKind::Select,
                self.styles.iter().map(|v| choice(v)).collect(),
            ),
            option(
                "agent",
                ControlKind::Select,
                self.agents.iter().map(|v| choice(v)).collect(),
            ),
            option(
                "advisorModel",
                ControlKind::Select,
                self.metadata.iter().map(|m| ControlChoice {
                    value: m.id.clone(),
                    label: if m.display_name.is_empty() { m.id.clone() } else { m.display_name.clone() },
                    description: m.description.clone(),
                    group: None,
                }).collect(),
            ),
            option("autoCompactEnabled", ControlKind::Boolean, vec![]),
            option("language", ControlKind::Text, vec![]),
            option("fallbackModel", ControlKind::Strings, vec![]),
            option(
                "promptCacheTtl",
                ControlKind::Select,
                ["5m", "1h"].into_iter().map(choice).collect(),
            ),
            option(
                "subagentPromptCacheTtl",
                ControlKind::Select,
                ["5m", "1h"].into_iter().map(choice).collect(),
            ),
            option("promptSuggestionEnabled", ControlKind::Boolean, vec![]),
            option(
                "thinkingBudget",
                ControlKind::Integer {
                    min: 0,
                    max: 128000,
                },
                vec![],
            ),
            option(
                "thinkingDisplay",
                ControlKind::Select,
                ["summarized", "omitted", "highlights"]
                    .into_iter()
                    .map(choice)
                    .collect(),
            ),
        ];
        for entry in &self.command_catalog {
            if !entry.terminal_only && entry.builtin != Some(true) {
                live.push(option(
                    &format!("skill:{}", entry.name),
                    ControlKind::Select,
                    ["on", "name-only", "user-invocable-only", "off"]
                        .into_iter()
                        .map(choice)
                        .collect(),
                ));
            }
        }
        for skill in self.skill_overrides.keys() {
            let id = format!("skill:{skill}");
            if !live.iter().any(|o| o.id == id) {
                live.push(option(
                    &id,
                    ControlKind::Select,
                    ["on", "name-only", "user-invocable-only", "off"]
                        .into_iter()
                        .map(choice)
                        .collect(),
                ));
            }
        }
        for item in &mut live {
            item.current = self.safe_settings.get(&item.id).cloned();
            if item.id == "permissionMode" { item.current = self.permission.clone().map(ControlValue::Text); }
            if item.id == "model" && item.current.is_none() { item.current = self.selected.clone().map(ControlValue::Text); }
            item.source = self.sources.get(&item.id).cloned();
            if item.id == "effortLevel"
                && (item.choices.is_empty()
                    || model.is_some_and(|m| m.capabilities.get("supportsEffort") == Some(&false)))
            {
                item.available = false;
                item.reason = Some("Select a model that advertises effort levels.".into());
            }
            if item.id == "thinkingBudget"
                && model
                    .is_none_or(|m| m.capabilities.get("supportsAdaptiveThinking") != Some(&false))
            {
                item.available = false;
                item.reason = Some(
                    "A numerical thinking budget is not advertised as meaningful for this model."
                        .into(),
                );
            }
            if item.id == "fastMode"
                && model.is_some_and(|m| m.capabilities.get("supportsFastMode") == Some(&false))
            {
                item.available = false;
                item.reason = Some("The selected model does not support Fast mode.".into());
            }
            if item.id.starts_with("skill:") && !self.settings_known {
                item.available = false;
                item.reason =
                    Some("Refresh settings before merging skill exposure overrides.".into());
            }
            if item.id == "enableWorkflows" && self.workflows_disabled {
                item.available = false;
                item.reason = Some("Native policy disables workflows.".into());
            }
            if item.id == "permissionMode"
                && (self.auto_disabled
                    || model
                        .is_some_and(|m| m.capabilities.get("supportsAutoMode") == Some(&false)))
            {
                item.choices.retain(|v| v.value != "auto");
            }
            if let Some(index) = options.iter().position(|v| v.id == item.id) {
                options[index] = item.clone();
            } else {
                options.push(item.clone());
            }
        }
        self.controls = options;
        e.event(Event::Controls(ControlEvent::Options(
            self.controls.clone(),
        )));
        e.event(Event::Modes {
            choices: live
                .iter()
                .find(|o| o.id == "permissionMode")
                .unwrap()
                .choices
                .iter()
                .map(|v| Choice {
                    id: v.value.clone(),
                    label: v.label.clone(),
                    description: v.description.clone(),
                })
                .collect(),
            selected: self.permission.clone(),
        });
    }
    pub(super) fn configure(
        &mut self,
        control: &str,
        value: &ControlValue,
        e: &mut Effects,
    ) -> Result<()> {
        ensure!(
            self.ready && self.deferred.is_none() && self.retired_configs.is_empty(),
            "Claude is not ready for configuration"
        );
        self.native.idle()?;
        self.controls
            .iter()
            .find(|o| o.id == control)
            .context("Unknown native setting")?
            .validate(value)?;
        let request = match control {
            "permissionMode" => {
                ensure!(
                    !matches!(value, ControlValue::Default),
                    "Choose an explicit permission mode"
                );
                json!({"subtype":"set_permission_mode","mode":value.wire()})
            }
            "model" => json!({"subtype":"set_model","model":value.wire()}),
            "thinkingBudget" => {
                json!({"subtype":"set_max_thinking_tokens","max_thinking_tokens":value.wire()})
            }
            "thinkingDisplay" => {
                json!({"subtype":"set_max_thinking_tokens","thinking_display":value.wire()})
            }
            _ => {
                let mut patch = serde_json::Map::new();
                if let Some(skill) = control.strip_prefix("skill:") {
                    let mut overrides = self.skill_overrides.clone();
                    if matches!(value, ControlValue::Default) {
                        overrides.remove(skill);
                    } else {
                        overrides.insert(skill.into(), value.wire());
                    }
                    patch.insert("skillOverrides".into(), json!(overrides));
                } else {
                    patch.insert(control.into(), value.wire());
                }
                if control == "effortLevel" {
                    ensure!(
                        self.settings_known,
                        "Refresh native settings before changing effort"
                    );
                    patch.insert("ultracode".into(), json!(self.ultracode_requested));
                }
                json!({"subtype":"apply_flag_settings","settings":patch})
            }
        };
        self.invalidate_settings_reads();
        let id = self.native.begin(control, value.clone(), e)?;
        e.write(Self::control(&id, request));
        Ok(())
    }
    pub(super) fn invalidate_settings_reads(&mut self) {
        self.native.reads.retain(|_, target| {
            !matches!(
                target,
                Discovery::Settings | Discovery::Status | Discovery::Context
            )
        });
    }
    pub(super) fn settings(&mut self, r: &Value, e: &mut Effects) {
        self.settings_known = true;
        self.safe_settings
            .retain(|key, _| key == "permissionMode" || key == "autoCompactEnabled");
        self.skill_overrides = r["effective"]["skillOverrides"]
            .as_object()
            .map(|v| {
                v.iter()
                    .filter(|(_, v)| {
                        v.as_str().is_some_and(|v| {
                            ["on", "name-only", "user-invocable-only", "off"].contains(&v)
                        })
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (name, value) in &self.skill_overrides {
            if let Some(value) = ControlValue::from_safe(value) {
                self.safe_settings.insert(format!("skill:{name}"), value);
            }
        }
        self.workflows_disabled = r["effective"]["disableWorkflows"] == true;
        self.auto_disabled = r["effective"]["permissions"]["disableAutoMode"] == "disable";
        if let Some(v) = r["applied"]["ultracodeRequested"].as_bool() {
            self.ultracode_requested = v;
        }
        for key in [
            "language",
            "fallbackModel",
            "promptCacheTtl",
            "subagentPromptCacheTtl",
            "promptSuggestionEnabled",
            "workflowKeywordTriggerEnabled",
            "workflowSizeGuideline",
            "enableWorkflows",
            "outputStyle",
            "agent",
            "fastMode",
        ] {
            if let Some(v) = r["effective"].get(key).and_then(ControlValue::from_safe) {
                self.safe_settings.insert(key.into(), v);
            }
        }
        for (key, applied) in [
            ("model", "model"),
            ("effortLevel", "effort"),
            ("ultracode", "ultracode"),
            ("advisorModel", "advisor"),
        ] {
            if let Some(v) = r["applied"].get(applied).and_then(ControlValue::from_safe) {
                self.safe_settings.insert(key.into(), v);
            }
        }
        if self.deferred.is_none()
            && self.retired_configs.is_empty()
            && let Some(model) = r["applied"]["model"].as_str()
        {
            self.selected = Some(model.into());
            self.model_settled = true;
            e.event(Event::Models {
                choices: vec![],
                selected: self.selected.clone(),
            });
        }
        // Only source labels and declared key names survive; never source settings values.
        self.sources.clear();
        if let Some(sources) = r["sources"].as_object() {
            for (source, settings) in sources {
                if let Some(settings) = settings.as_object() {
                    for key in settings.keys() {
                        self.sources
                            .insert(key.clone(), source.chars().take(128).collect());
                    }
                }
            }
        }
        e.event(status(
            "claude_applied",
            &r["applied"],
            &[
                "effort",
                "effortAuto",
                "ultracode",
                "ultracodeRequested",
                "ultracodeAvailable",
                "ultrathinkAvailable",
                "advisor",
            ],
        ));
        e.event(status(
            "claude_policy",
            &r["effective"],
            &["disableWorkflows", "maxEffortLevel"],
        ));
        if let Some(settings) = r["effective"]["modelSettings"].as_object() {
            for (model, settings) in settings.iter().take(128) {
                e.event(status(
                    &format!(
                        "model_policy:{}",
                        model.chars().take(128).collect::<String>()
                    ),
                    settings,
                    &["maxEffortLevel", "effortLevel", "autoCompactWindow"],
                ));
            }
        }
        self.refresh_options(e);
    }
    pub(super) fn detailed_response(&mut self, id: &str, r: &Value, e: &mut Effects) -> bool {
        if id.starts_with("native-") {
            if self.native.operation.as_ref().is_none_or(|op| op.id != id) {
                return true;
            }
            if s(r, "subtype") == "error" {
                let reason = if r["error_code"] == "auto_mode_settings" {
                    self.auto_disabled = true;
                    self.refresh_options(e);
                    "Native policy disables Auto permission mode."
                } else {
                    "The native CLI rejected this operation."
                };
                self.native
                    .finish(ApplicationState::Rejected, None, Some(reason.into()), e);
                return true;
            }
            let control = self.native.operation.as_ref().unwrap().control.clone();
            if control == "permissionMode" {
                if let Some(mode) = r["response"]["mode"].as_str() {
                    self.permission = Some(mode.into());
                    self.safe_settings
                        .insert(control.clone(), ControlValue::Text(mode.into()));
                    self.refresh_options(e);
                    self.native.finish(
                        ApplicationState::Applied,
                        Some(ControlValue::Text(mode.into())),
                        None,
                        e,
                    );
                } else {
                    self.native.finish(
                        ApplicationState::Acknowledged,
                        None,
                        Some("Native ACK did not include effective mode.".into()),
                        e,
                    );
                }
                return true;
            }
            if control.starts_with("action:") {
                self.catalog(&r["response"], e);
                self.mcp(&r["response"], e);
                self.native
                    .finish(ApplicationState::Acknowledged, None, None, e);
                return true;
            }
            let target = if control == "autoCompactEnabled" {
                Discovery::Context
            } else {
                Discovery::Settings
            };
            let read = self.discover(target, e);
            self.native.operation.as_mut().unwrap().readback = Some(read);
            return true;
        }
        let Some(target) = self.native.reads.remove(id) else {
            return id.starts_with("discovery-");
        };
        let owns = self
            .native
            .operation
            .as_ref()
            .is_some_and(|op| op.readback.as_deref() == Some(id));
        if s(r, "subtype") == "error" {
            if let Discovery::TaskOutput {task_id,run_id}=&target {e.event(Event::TaskActivity(super::super::super::activity::Event::Output{task_id:task_id.clone(),run_id:run_id.clone(),text:String::new(),truncated:false,unavailable:true}));}
            e.event(unavailable(&format!("{target:?}")));
            if owns {
                self.native.finish(
                    ApplicationState::Unconfirmed,
                    None,
                    Some("Native ACK received; effective readback unavailable.".into()),
                    e,
                );
            }
            return true;
        }
        // Discovery does not own the legacy model-write settlement barrier.
        if !owns
            && (self.deferred.is_some() || !self.retired_configs.is_empty())
            && matches!(
                target,
                Discovery::Settings | Discovery::Status | Discovery::Context
            )
        {
            return true;
        }
        let payload = &r["response"];
        match target {
            Discovery::TaskOutput {task_id,run_id} => {
                let raw=payload["output"].as_str().unwrap_or_default();
                e.event(Event::TaskActivity(super::super::super::activity::Event::Output{task_id,run_id,text:raw.chars().take(8192).collect(),truncated:payload["truncated"]==true || raw.chars().count()>8192,unavailable:false}));
            }
            Discovery::Goal | Discovery::Queue | Discovery::Memory => {}
            Discovery::Settings => self.settings(payload, e),
            Discovery::Commands | Discovery::Skills => self.catalog(payload, e),
            Discovery::Mcp => self.mcp(payload, e),
            Discovery::Context => {
                e.event(status(
                    "context",
                    payload,
                    &[
                        "totalTokens",
                        "maxTokens",
                        "rawMaxTokens",
                        "isAutoCompactEnabled",
                        "model",
                    ],
                ));
                if let Some(v) = payload["isAutoCompactEnabled"].as_bool() {
                    self.safe_settings
                        .insert("autoCompactEnabled".into(), ControlValue::Bool(v));
                    self.refresh_options(e);
                }
            }
            Discovery::Policy => {
                // Rule text can contain secrets or command arguments; project only counts and native mode.
                let count = payload["state"]["rules"].as_array().map(Vec::len);
                let mut fields = BTreeMap::new();
                if let Some(v) = count {
                    fields.insert("rule_count".into(), ControlValue::Integer(v as u64));
                }
                for key in ["managedOnly", "withheld"] {
                    if let Some(value) = payload["state"].get(key).and_then(ControlValue::from_safe)
                    {
                        fields.insert(key.into(), value);
                    }
                }
                e.event(Event::Controls(ControlEvent::Status(NativeStatus {
                    subject: "permission_policy".into(),
                    fields,
                    unavailable: None,
                })));
            }
            Discovery::Status => self.settings(payload, e),
        }
        if owns {
            let op = self.native.operation.as_ref().unwrap();
            let effective = self.safe_settings.get(&op.control).cloned();
            let verified = [
                "model",
                "effortLevel",
                "ultracode",
                "advisorModel",
                "autoCompactEnabled",
            ]
            .contains(&op.control.as_str());
            let state = if verified && effective.is_some() {
                ApplicationState::Applied
            } else {
                ApplicationState::Acknowledged
            };
            let message = if verified && effective.as_ref() != Some(&op.value) {
                Some("Native effective value differs from the requested value (model or policy limits may apply).".into())
            } else if !verified {
                Some("Session source accepted; runtime effect is not reported by this CLI.".into())
            } else {
                None
            };
            self.native.finish(state, effective, message, e);
        }
        true
    }
    fn mcp(&self, r: &Value, e: &mut Effects) {
        if let Some(servers) = r["mcpServers"].as_array() {
            e.event(coverage(
                "mcp",
                &r["mcpServers"],
                servers.len().min(128),
                false,
            ));
            let mut fields = BTreeMap::new();
            fields.insert(
                "server_count".into(),
                ControlValue::Integer(servers.len() as u64),
            );
            for (i, server) in servers.iter().take(128).enumerate() {
                for key in ["name", "status", "scope"] {
                    if let Some(v) = server.get(key).and_then(ControlValue::from_safe) {
                        fields.insert(format!("{i}.{key}"), v);
                    }
                }
                if let Some(v) = server["tools"].as_array() {
                    fields.insert(
                        format!("{i}.tool_count"),
                        ControlValue::Integer(v.len() as u64),
                    );
                }
            }
            e.event(Event::Controls(ControlEvent::Status(NativeStatus {
                subject: "mcp".into(),
                fields,
                unavailable: None,
            })));
        }
    }
}
fn choice(v: &str) -> ControlChoice {
    ControlChoice {
        value: v.into(),
        label: v.into(),
        description: String::new(),
        group: None,
    }
}
fn option(id: &str, kind: ControlKind, choices: Vec<ControlChoice>) -> ControlOption {
    ControlOption {
        id: id.into(),
        label: id.into(),
        description: String::new(),
        category: Some("Session".into()),
        kind,
        choices,
        current: None,
        scope: ControlScope::Live,
        available: true,
        reason: None,
        source: None,
        documentation: Some(
            "https://code.claude.com/docs/en/agent-sdk/typescript#applyflagsettings".into(),
        ),
    }
}
