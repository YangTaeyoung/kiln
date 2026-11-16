//! Generated app-server 0.167.2 v2 contracts. Empty settings ACK is not effective state.
use super::*;
impl Codex {
    pub(super) fn discovery(&mut self, target: Discovery, e: &mut Effects) {
        if self.native.reads.len() >= 32 {
            e.event(unavailable("native_discovery_capacity"));
            return;
        }
        if target == Discovery::Policy {
            self.profiles.clear();
            self.profile_pages = 0;
            e.write(rpc(
                json!("profiles"),
                "permissionProfile/list",
                json!({"cwd":self.cwd,"limit":100}),
            ));
        }

        let (method, params) = match target {
            Discovery::Goal => ("thread/goal/get", json!({"threadId":self.thread})),
            Discovery::Queue => (
                "thread/queue/list",
                json!({"threadId":self.thread,"limit":100}),
            ),
            Discovery::Memory => ("thread/memory/read", json!({"threadId":self.thread})),
            Discovery::Skills => ("skills/list", json!({"cwds":[self.cwd],"forceReload":true})),
            Discovery::Mcp => (
                "mcpServerStatus/list",
                json!({"threadId":self.thread,"limit":100}),
            ),
            Discovery::Policy => ("configRequirements/read", json!({})),
            Discovery::Settings => ("config/read", json!({"cwd":self.cwd,"includeLayers":false})),
            Discovery::Commands => {
                e.event(unavailable("slash_commands"));
                return;
            }
            Discovery::Context => (
                "thread/read",
                json!({"threadId":self.thread,"includeTurns":false}),
            ),
            Discovery::Status => ("experimentalFeature/list", json!({"limit":100})),
        };
        let id = self.native.read(target);
        e.write(rpc(json!(id), method, params));
    }
    pub(super) fn models(&mut self, r: &Value, e: &mut Effects) -> Result<()> {
        if !r["data"].is_array() {
            ensure!(
                !self.start_after_models,
                "Native model catalog was malformed"
            );
            e.event(unavailable("malformed_model_catalog"));
            return Ok(());
        }
        if self.model_pages == 0 {
            self.metadata.clear();
            self.model_choices.clear();
        }
        for v in r["data"].as_array().into_iter().flatten().take(256) {
            let id = s(v, "id");
            if id.is_empty() {
                continue;
            }
            self.model_choices.push(Model {
                id: id.clone(),
                label: controls::bounded(v, "displayName"),
            });
            self.metadata.push(ModelMetadata {
                id,
                description: controls::bounded(v, "description"),
                resolved_model: v["model"].as_str().map(str::to_owned),
                efforts: v["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| ControlChoice {
                        value: s(v, "reasoningEffort"),
                        label: s(v, "reasoningEffort"),
                        description: controls::bounded(v, "description"),
                        group: None,
                    })
                    .collect(),
                default_effort: v["defaultReasoningEffort"].as_str().map(str::to_owned),
                tiers: v["serviceTiers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| ControlChoice {
                        value: s(v, "id"),
                        label: s(v, "name"),
                        description: controls::bounded(v, "description"),
                        group: None,
                    })
                    .collect(),
                default_tier: v["defaultServiceTier"].as_str().map(str::to_owned),
                availability: {
                    let mut fields = BTreeMap::new();
                    for key in ["upgrade", "modelSpecialty", "multiAgentVersion"] {
                        if let Some(value) = v.get(key).and_then(ControlValue::from_safe) {
                            fields.insert(key.into(), value);
                        }
                    }
                    if let Some(value) = v["availabilityNux"]
                        .get("message")
                        .and_then(ControlValue::from_safe)
                    {
                        fields.insert("message".into(), value);
                    }
                    if let Some(value) = v["availableAccessPrograms"]
                        .get("cyber")
                        .and_then(ControlValue::from_safe)
                    {
                        fields.insert("accessPrograms".into(), value);
                    }
                    for key in [
                        "model",
                        "modelLink",
                        "retirementAt",
                        "upgradeCopy",
                        "migrationMarkdown",
                    ] {
                        if let Some(value) =
                            v["upgradeInfo"].get(key).and_then(ControlValue::from_safe)
                        {
                            fields.insert(format!("upgrade.{key}"), value);
                        }
                    }
                    fields
                },
                modalities: v["inputModalities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                capabilities: ["hidden", "isDefault"]
                    .into_iter()
                    .filter_map(|k| v[k].as_bool().map(|v| (k.into(), v)))
                    .collect(),
            });
        }
        e.event(coverage(
            "model_page",
            &r["data"],
            r["data"].as_array().map(Vec::len).unwrap_or(0).min(256),
            false,
        ));
        self.model_pages += 1;
        if let Some(cursor) = r["nextCursor"].as_str().filter(|s| !s.is_empty()) {
            if self.model_pages < 16 {
                e.write(rpc(
                    json!("models"),
                    "model/list",
                    json!({"cursor":cursor,"limit":100,"includeHidden":true}),
                ));
                return Ok(());
            }
            e.event(unavailable("model_catalog_pagination_limit"));
        }
        self.model_pages = 0;
        e.event(Event::Controls(ControlEvent::ModelSnapshot {
            choices: self.model_choices.clone(),
            selected: self.selected.clone(),
        }));
        e.event(Event::Controls(ControlEvent::Models(self.metadata.clone())));
        self.refresh_options(e);
        if self.start_after_models {
            let LaunchOptions::Codex(options) = &self.launch_options else {
                unreachable!()
            };
            let selected = options.model.as_ref().context("Explicit model missing")?;
            let meta = self
                .metadata
                .iter()
                .find(|m| m.id == *selected || m.resolved_model.as_ref() == Some(selected))
                .context("Startup model is not advertised")?;
            ensure!(
                options
                    .effort
                    .as_ref()
                    .is_none_or(|v| meta.efforts.iter().any(|e| &e.value == v)),
                "Startup effort is not advertised for this model"
            );
            ensure!(
                options
                    .service_tier
                    .as_ref()
                    .is_none_or(|v| meta.tiers.iter().any(|e| &e.value == v)),
                "Startup service tier is not advertised for this model"
            );
            let mut params = self.launch_options.codex_params();
            params["cwd"] = json!(self.cwd);
            params["model"] = json!(meta.resolved_model.as_ref().unwrap_or(selected));
            self.start_after_models = false;
            e.write(rpc(json!("thread"), "thread/start", params));
        }
        Ok(())
    }
    pub(super) fn refresh_options(&mut self, e: &mut Effects) {
        let model = self.metadata.iter().find(|m| {
            Some(&m.id) == self.selected.as_ref()
                || self
                    .selected
                    .as_ref()
                    .is_some_and(|selected| m.resolved_model.as_ref() == Some(selected))
        });
        let mut options = vec![
            option(
                "model",
                self.model_choices
                    .iter()
                    .map(|m| ControlChoice {
                        value: m.id.clone(),
                        label: m.label.clone(),
                        description: String::new(),
                        group: None,
                    })
                    .collect(),
            ),
            option(
                "effort",
                model.map(|m| m.efforts.clone()).unwrap_or_default(),
            ),
            option(
                "serviceTier",
                model.map(|m| m.tiers.clone()).unwrap_or_default(),
            ),
            option(
                "collaborationMode",
                self.collaboration
                    .iter()
                    .map(|(id, _)| choice(id))
                    .collect(),
            ),
            option("permissions", self.profiles.clone()),
            option(
                "approvalPolicy",
                self.allowed_policies.iter().map(|v| choice(v)).collect(),
            ),
            option(
                "approvalsReviewer",
                self.allowed_reviewers.iter().map(|v| choice(v)).collect(),
            ),
            option(
                "summary",
                ["auto", "concise", "detailed", "none"]
                    .into_iter()
                    .map(choice)
                    .collect(),
            ),
        ];
        for (id, kind) in [
            ("sandboxPolicy", ControlKind::Sandbox),
            ("granularApproval", ControlKind::GranularApproval),
        ] {
            let mut control = option(id, vec![]);
            control.kind = kind;
            control.available = self.policy_loaded;
            options.push(control);
        }
        for o in &mut options {
            o.current = self.effective.get(&o.id).cloned();
            if o.id == "model" {
                o.current = self.selected.clone().map(ControlValue::Text);
            }
            if matches!(o.kind, ControlKind::Select)
                && o.choices.is_empty()
                && o.id != "serviceTier"
            {
                o.available = false;
                o.reason = Some("Native capability has not been advertised.".into());
            }
        }
        self.controls = options;
        e.event(Event::Controls(ControlEvent::Options(
            self.controls.clone(),
        )));
        e.event(Event::Controls(ControlEvent::ModeSnapshot {
            choices: self
                .collaboration
                .keys()
                .map(|id| Choice {
                    id: id.clone(),
                    label: id.clone(),
                    description: String::new(),
                })
                .collect(),
            selected: self.effective.get("collaborationMode").and_then(|v| {
                if let ControlValue::Text(v) = v {
                    Some(v.clone())
                } else {
                    None
                }
            }),
        }));
    }
    fn value_text(&self, key: &str) -> Option<String> {
        self.effective.get(key).and_then(|v| {
            if let ControlValue::Text(v) = v {
                Some(v.clone())
            } else {
                None
            }
        })
    }
    pub(super) fn patch(&self, control: &str, value: &ControlValue) -> Result<Value> {
        self.controls
            .iter()
            .find(|o| o.id == control)
            .context("Unknown native thread setting")?
            .validate(value)?;
        ensure!(
            !matches!(value, ControlValue::Default)
                || control == "serviceTier"
                || control == "effort",
            "This native field does not define reset semantics"
        );
        let mut patch = json!({"threadId":self.thread});
        if let ControlValue::Sandbox(policy) = value {
            ensure!(
                control == "sandboxPolicy"
                    && self
                        .allowed_sandboxes
                        .as_ref()
                        .is_none_or(|v| v.iter().any(|v| v == policy.policy_mode())),
                "Native requirements disallow this sandbox policy"
            );
        }
        if let ControlValue::GranularApproval(policy) = value {
            ensure!(
                control == "granularApproval"
                    && self
                        .allowed_granular
                        .as_ref()
                        .is_none_or(|v| v.contains(policy)),
                "Native requirements disallow this granular approval policy"
            );
            patch["approvalPolicy"] = value.wire();
            return Ok(patch);
        }

        if ["model", "effort", "collaborationMode"].contains(&control) {
            let selected = if control == "model" {
                value.wire().as_str().map(str::to_owned)
            } else {
                self.selected.clone()
            }
            .context("The native model has not been reported")?;
            let meta = self
                .metadata
                .iter()
                .find(|m| m.id == selected || m.resolved_model.as_ref() == Some(&selected));
            let model = meta
                .and_then(|m| m.resolved_model.clone())
                .unwrap_or(selected);
            let mode = if control == "collaborationMode" {
                value.wire().as_str().map(str::to_owned)
            } else {
                self.value_text("collaborationMode")
            };
            let mut effort = if control == "effort" {
                match value {
                    ControlValue::Default => meta.and_then(|m| m.default_effort.clone()),
                    ControlValue::Text(v) => Some(v.clone()),
                    _ => None,
                }
            } else {
                self.value_text("effort")
            };
            if control == "collaborationMode" && !self.explicit_effort {
                effort = mode
                    .as_ref()
                    .and_then(|m| self.collaboration.get(m).cloned())
                    .flatten()
                    .or(effort);
            }
            if let Some(meta) = meta {
                if effort
                    .as_ref()
                    .is_some_and(|v| !meta.efforts.iter().any(|e| &e.value == v))
                {
                    effort = meta.default_effort.clone();
                }
                if control == "model"
                    && self
                        .value_text("serviceTier")
                        .as_ref()
                        .is_some_and(|v| !meta.tiers.iter().any(|t| &t.value == v))
                {
                    patch["serviceTier"] = json!(meta.default_tier);
                }
            }
            if let Some(mode) = mode {
                patch["collaborationMode"] = json!({"mode":mode,"settings":{"model":model,"reasoning_effort":effort,"developer_instructions":null}});
            } else {
                patch["model"] = json!(model);
                patch["effort"] = json!(effort);
            }
        } else {
            patch[control] = value.wire();
        }
        Ok(patch)
    }
    pub(super) fn configure(
        &mut self,
        control: &str,
        value: &ControlValue,
        e: &mut Effects,
    ) -> Result<()> {
        ensure!(
            self.ready && !self.busy,
            "Codex is not ready for thread settings"
        );
        self.native.idle()?;
        let patch = self.patch(control, value)?;
        self.expected_settings.clear();
        for key in [
            "model",
            "effort",
            "serviceTier",
            "summary",
            "permissions",
            "approvalsReviewer",
        ] {
            if let Some(value) = patch.get(key).and_then(ControlValue::from_safe) {
                self.expected_settings.insert(key.into(), value);
            }
        }
        if let Some(mode) = patch["collaborationMode"]["mode"].as_str() {
            self.expected_settings
                .insert("collaborationMode".into(), ControlValue::Text(mode.into()));
            for (native, key) in [("model", "model"), ("reasoning_effort", "effort")] {
                if let Some(value) = patch["collaborationMode"]["settings"]
                    .get(native)
                    .and_then(ControlValue::from_safe)
                {
                    self.expected_settings.insert(key.into(), value);
                }
            }
        }

        let id = self.native.begin(control, value.clone(), e)?;
        self.settings_ack = false;
        self.settings_notification = false;
        self.settings_deadline = Some(Instant::now() + Duration::from_secs(15));
        e.write(rpc(json!(id), "thread/settings/update", patch));
        Ok(())
    }
    pub(super) fn settings(&mut self, r: &Value, e: &mut Effects) {
        for key in [
            "model",
            "effort",
            "serviceTier",
            "summary",
            "approvalPolicy",
            "approvalsReviewer",
            "cwd",
        ] {
            if let Some(v) = r.get(key).and_then(ControlValue::from_safe) {
                self.effective.insert(key.into(), v);
            }
        }
        if let Ok(policy) = serde_json::from_value::<NativeSandbox>(r["sandboxPolicy"].clone()) {
            self.effective
                .insert("sandboxPolicy".into(), ControlValue::Sandbox(policy));
        }
        if let Ok(policy) =
            serde_json::from_value::<GranularApproval>(r["approvalPolicy"]["granular"].clone())
        {
            self.effective.insert(
                "granularApproval".into(),
                ControlValue::GranularApproval(policy),
            );
            e.event(status(
                "granular_approval",
                &r["approvalPolicy"]["granular"],
                &[
                    "sandbox_approval",
                    "rules",
                    "mcp_elicitations",
                    "request_permissions",
                    "skill_approval",
                ],
            ));
        }
        if let Some(mode) = r["collaborationMode"]["mode"].as_str() {
            self.effective
                .insert("collaborationMode".into(), ControlValue::Text(mode.into()));
        }
        if let Some(id) = r["activePermissionProfile"]["id"].as_str() {
            self.effective
                .insert("permissions".into(), ControlValue::Text(id.into()));
        }
        if let Some(model) = r["model"].as_str() {
            self.selected = Some(model.into());
            e.event(Event::Models {
                choices: vec![],
                selected: self.selected.clone(),
            });
        }
        e.event(status(
            "thread_policy",
            r,
            &[
                "approvalPolicy",
                "approvalsReviewer",
                "cwd",
                "modelProvider",
            ],
        ));
        e.event(status(
            "sandbox",
            &r["sandboxPolicy"],
            &[
                "type",
                "networkAccess",
                "writableRoots",
                "excludeSlashTmp",
                "excludeTmpdirEnvVar",
            ],
        ));
        self.refresh_options(e);
    }
    pub(super) fn settle_settings(&mut self, e: &mut Effects) {
        if !(self.settings_ack && self.settings_notification) {
            return;
        }
        let Some(op) = self.native.operation.as_ref() else {
            return;
        };
        let actual = self.effective.get(&op.control).cloned();
        let mut expected = op.value.clone();
        if op.control == "model" {
            if let ControlValue::Text(id) = &op.value {
                if let Some(model) = self
                    .metadata
                    .iter()
                    .find(|m| m.id == *id)
                    .and_then(|m| m.resolved_model.clone())
                {
                    expected = ControlValue::Text(model);
                }
            }
        }
        if op.control == "effort" && matches!(op.value, ControlValue::Default) {
            if let Some(default) = self
                .metadata
                .iter()
                .find(|m| {
                    Some(&m.id) == self.selected.as_ref()
                        || self
                            .selected
                            .as_ref()
                            .is_some_and(|selected| m.resolved_model.as_ref() == Some(selected))
                })
                .and_then(|m| m.default_effort.clone())
            {
                expected = ControlValue::Text(default);
            }
        }
        let dependencies_match = self.expected_settings.iter().all(|(key, value)| {
            self.effective.get(key) == Some(value)
                || (key == "serviceTier"
                    && *value == ControlValue::Default
                    && self.effective.get(key) == Some(&ControlValue::Text("default".into())))
        });
        let compatible = dependencies_match
            && (actual.as_ref() == Some(&expected)
                || (op.control == "serviceTier"
                    && matches!(op.value, ControlValue::Default)
                    && (actual == Some(ControlValue::Default)
                        || actual == Some(ControlValue::Text("default".into())))));
        if !compatible {
            e.event(op.outcome(ApplicationState::Unconfirmed,actual,Some("The effective native state differs from this request. Confirm the current native state or wait for matching readback before Send.".into())));
            return;
        }
        if op.control == "effort" && !op.cancelled {
            self.explicit_effort = !matches!(op.value, ControlValue::Default);
        }
        self.native
            .finish(ApplicationState::Applied, actual, None, e);
        self.settings_deadline = None;
    }
    pub(super) fn detailed_response(&mut self, id: &str, v: &Value, e: &mut Effects) -> bool {
        if self.action_result(id, v, e) {
            return true;
        }
        if id.starts_with("native-") {
            if self.native.operation.as_ref().is_none_or(|op| op.id != id) {
                return true;
            }
            if v.get("error").is_some() {
                self.native.finish(
                    ApplicationState::Rejected,
                    None,
                    Some(
                        "Native operation was rejected; existing settings remain authoritative."
                            .into(),
                    ),
                    e,
                );
                self.settings_deadline = None;
                return true;
            }
            let op = self.native.operation.as_ref().unwrap();
            if op.control == "action:compact" {
                self.native.finish(
                    ApplicationState::Acknowledged,
                    None,
                    Some("Compaction was requested; waiting for native item completion.".into()),
                    e,
                );
                return true;
            }
            self.settings_ack = true;
            self.settle_settings(e);
            return true;
        }
        if id == "models" && v.get("error").is_some() {
            e.event(unavailable("models"));
            return true;
        }
        if id == "collaboration" {
            if v.get("error").is_none() {
                self.collaboration = v["result"]["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| {
                        v["mode"].as_str().map(|mode| {
                            (
                                mode.into(),
                                v["reasoning_effort"].as_str().map(str::to_owned),
                            )
                        })
                    })
                    .collect();
                self.refresh_options(e);
            } else {
                e.event(unavailable("collaboration"));
            }
            return true;
        }
        if id == "profiles" {
            if v.get("error").is_none() {
                for p in v["result"]["data"].as_array().into_iter().flatten() {
                    if p["allowed"] == true {
                        self.profiles.push(ControlChoice {
                            value: s(p, "id"),
                            label: s(p, "id"),
                            description: controls::bounded(p, "description"),
                            group: None,
                        });
                    }
                }
                if let Some(cursor) = v["result"]["nextCursor"].as_str() {
                    if self.profile_pages < 16 {
                        self.profile_pages += 1;
                        e.write(rpc(
                            json!("profiles"),
                            "permissionProfile/list",
                            json!({"cwd":self.cwd,"cursor":cursor,"limit":100}),
                        ));
                    } else {
                        e.event(unavailable("profiles_pagination_limit"));
                    }
                }
                self.refresh_options(e);
            } else {
                e.event(unavailable("permission_profiles"));
            }
            return true;
        }
        let Some(target) = self.native.reads.remove(id) else {
            return id.starts_with("discovery-");
        };
        if v.get("error").is_some() {
            e.event(unavailable(&format!("{target:?}")));
            return true;
        }
        let r = &v["result"];
        if matches!(
            target,
            Discovery::Skills | Discovery::Mcp | Discovery::Status | Discovery::Queue
        ) && !r["data"].is_array()
        {
            e.event(unavailable("malformed_native_catalog"));
            return true;
        }
        match target {
            Discovery::Goal => e.event(status(
                "goal",
                &r["goal"],
                &[
                    "objective",
                    "status",
                    "tokenBudget",
                    "tokensUsed",
                    "timeUsedSeconds",
                ],
            )),
            Discovery::Queue => self.queue_snapshot(r, e),
            Discovery::Memory => e.event(status(
                "memory",
                &json!({"summary_available":r["summary"].as_str().is_some_and(|v|!v.is_empty())}),
                &["summary_available"],
            )),
            Discovery::Skills => {
                let mut skills = vec![];
                let mut errors = 0;
                for group in r["data"].as_array().into_iter().flatten() {
                    errors += group["errors"].as_array().map(Vec::len).unwrap_or(0);
                    for skill in group["skills"].as_array().into_iter().flatten().take(512) {
                        let mut entries = controls::native_entries(&json!([skill]));
                        if let Some(mut entry) = entries.pop() {
                            entry.path = skill["path"].as_str().map(str::to_owned);
                            self.skill_enabled.insert(
                                entry.path.clone().unwrap_or_default(),
                                skill["enabled"] == true,
                            );
                            skills.push(entry);
                        }
                    }
                }
                let total: usize = r["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|g| g["skills"].as_array().map(Vec::len).unwrap_or(0))
                    .sum();
                e.event(Event::Controls(ControlEvent::Coverage(CatalogCoverage {
                    subject: "skills".into(),
                    received: total,
                    retained: skills.len(),
                    partial: total > skills.len(),
                    reason: (total > skills.len()).then(|| {
                        "Skill catalog exceeded the per-workspace 512-entry projection limit."
                            .into()
                    }),
                })));
                e.event(coverage(
                    "skill_metadata",
                    &r["data"],
                    r["data"].as_array().map(Vec::len).unwrap_or(0),
                    false,
                ));
                self.skills = skills.clone();
                e.event(Event::Controls(ControlEvent::Catalog {
                    kind: CatalogKind::Skills,
                    entries: skills,
                }));
                e.event(status(
                    "skill_discovery",
                    &json!({"error_count":errors}),
                    &["error_count"],
                ));
            }
            Discovery::Policy => {
                let r = &r["requirements"];
                self.policy_loaded = true;
                self.allowed_sandboxes = r["allowedSandboxModes"].as_array().map(|v| {
                    v.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                });
                self.allowed_granular = r["allowedApprovalPolicies"].as_array().map(|v| {
                    v.iter()
                        .filter_map(|v| {
                            serde_json::from_value::<GranularApproval>(v["granular"].clone()).ok()
                        })
                        .collect()
                });
                self.allowed_policies = allowed(
                    r,
                    "allowedApprovalPolicies",
                    &["untrusted", "on-request", "never"],
                );
                self.allowed_reviewers =
                    allowed(r, "allowedApprovalsReviewers", &["user", "auto_review"]);
                e.event(status(
                    "requirements",
                    r,
                    &[
                        "allowedApprovalPolicies",
                        "allowedApprovalsReviewers",
                        "allowedSandboxModes",
                        "allowedWebSearchModes",
                        "allowManagedHooksOnly",
                        "allowBrowserAndComputerUse",
                    ],
                ));
                self.refresh_options(e);
            }
            Discovery::Settings => e.event(status(
                "inherited_config",
                &r["config"],
                &[
                    "model",
                    "model_provider",
                    "model_reasoning_effort",
                    "model_reasoning_summary",
                    "model_verbosity",
                    "service_tier",
                    "web_search",
                    "model_context_window",
                    "model_auto_compact_token_limit",
                    "model_post_turn_compact_threshold_percent",
                ],
            )),
            Discovery::Context => {
                if r["thread"]["id"] == self.thread {
                    e.event(status(
                        "thread_context",
                        &r["thread"],
                        &["model", "reasoningEffort"],
                    ));
                }
            }
            Discovery::Mcp => {
                e.event(coverage(
                    "mcp",
                    &r["data"],
                    r["data"].as_array().map(Vec::len).unwrap_or(0).min(256),
                    r["nextCursor"].is_string(),
                ));
                let mut fields = BTreeMap::new();
                for (i, mcp) in r["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(256)
                    .enumerate()
                {
                    for key in ["name", "authStatus"] {
                        if let Some(v) = mcp.get(key).and_then(ControlValue::from_safe) {
                            fields.insert(format!("{i}.{key}"), v);
                        }
                    }
                    if let Some(v) = mcp["runtimeStatus"].as_str() {
                        fields.insert(format!("{i}.state"), ControlValue::Text(v.into()));
                    }
                    fields.insert(
                        format!("{i}.tool_count"),
                        ControlValue::Integer(
                            mcp["tools"]
                                .as_object()
                                .map(|v| v.len() as u64)
                                .unwrap_or_default(),
                        ),
                    );
                }
                e.event(Event::Controls(ControlEvent::Status(NativeStatus {
                    subject: "mcp".into(),
                    fields,
                    unavailable: r["nextCursor"].as_str().map(|_| {
                        "More servers are available; this bounded snapshot is partial.".into()
                    }),
                })));
            }
            Discovery::Status => {
                e.event(coverage(
                    "features",
                    &r["data"],
                    r["data"].as_array().map(Vec::len).unwrap_or(0).min(512),
                    r["nextCursor"].is_string(),
                ));
                let mut fields = BTreeMap::new();
                for feature in r["data"].as_array().into_iter().flatten().take(512) {
                    let name = controls::bounded(feature, "name");
                    for key in ["stage", "enabled", "defaultEnabled"] {
                        if let Some(v) = feature.get(key).and_then(ControlValue::from_safe) {
                            fields.insert(format!("{name}.{key}"), v);
                        }
                    }
                }
                e.event(Event::Controls(ControlEvent::Status(NativeStatus {
                    subject: "native_features".into(),
                    fields,
                    unavailable: None,
                })));
            }
            Discovery::Commands => {}
        }
        true
    }
    pub(super) fn notification(&mut self, method: &str, p: &Value, e: &mut Effects) -> bool {
        match method {
            "thread/settings/updated" => {
                if p["threadId"] != self.thread {
                    return true;
                }
                self.settings(&p["threadSettings"], e);
                if self
                    .native
                    .operation
                    .as_ref()
                    .is_some_and(|op| !op.control.starts_with("action:"))
                {
                    self.settings_notification = true;
                    self.settle_settings(e);
                }
                true
            }
            "thread/tokenUsage/updated" => {
                if p["threadId"] != self.thread {
                    return true;
                }
                for (name, raw) in [
                    ("context_last", &p["tokenUsage"]["last"]),
                    ("usage_total", &p["tokenUsage"]["total"]),
                ] {
                    e.event(status(
                        name,
                        raw,
                        &[
                            "totalTokens",
                            "inputTokens",
                            "cachedInputTokens",
                            "outputTokens",
                            "reasoningOutputTokens",
                        ],
                    ));
                }
                e.event(status(
                    "context_window",
                    &p["tokenUsage"],
                    &["modelContextWindow"],
                ));
                true
            }
            "thread/goal/updated" => {
                if p["threadId"] == self.thread {
                    e.event(status(
                        "goal",
                        &p["goal"],
                        &[
                            "objective",
                            "status",
                            "tokenBudget",
                            "tokensUsed",
                            "timeUsedSeconds",
                        ],
                    ));
                }
                true
            }
            "thread/goal/cleared" => {
                if p["threadId"] == self.thread {
                    e.event(status("goal", &json!({"cleared":true}), &["cleared"]));
                }
                true
            }
            "thread/queue/changed" => {
                if p["threadId"] == self.thread {
                    self.discovery(Discovery::Queue, e);
                }
                true
            }
            "skills/changed" => {
                self.discovery(Discovery::Skills, e);
                true
            }
            _ => false,
        }
    }
}
fn allowed(r: &Value, key: &str, defaults: &[&str]) -> Vec<String> {
    match r.get(key).and_then(Value::as_array) {
        Some(v) => v
            .iter()
            .filter_map(Value::as_str)
            .filter(|v| defaults.contains(v))
            .map(str::to_owned)
            .collect(),
        None => defaults.iter().map(|v| (*v).into()).collect(),
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
fn option(id: &str, choices: Vec<ControlChoice>) -> ControlOption {
    ControlOption {
        id: id.into(),
        label: id.into(),
        description: String::new(),
        category: Some("Next turns".into()),
        kind: ControlKind::Select,
        choices,
        current: None,
        scope: ControlScope::Live,
        available: true,
        reason: None,
        source: Some("native thread settings".into()),
        documentation: Some("https://learn.chatgpt.com/docs/app-server".into()),
    }
}
pub(super) fn permission_grant(request: &Value, decision: &Decision) -> Result<Value> {
    let Decision::Grant(grant) = decision else {
        ensure!(
            !matches!(decision, Decision::Allow),
            "Select the exact permission subset and scope before granting"
        );
        return Ok(json!({"permissions":{},"scope":"turn"}));
    };
    let mut permissions = json!({});
    let mut fs = json!({});
    for (key, indices) in [
        ("entries", &grant.entry_indices),
        ("read", &grant.read_indices),
        ("write", &grant.write_indices),
    ] {
        if !indices.is_empty() {
            let source = request["fileSystem"][key]
                .as_array()
                .context("Requested permission list is unavailable")?;
            let mut selected = vec![];
            for index in indices {
                selected.push(
                    source
                        .get(*index)
                        .context("Permission index is not in the request")?
                        .clone(),
                );
            }
            fs[key] = json!(selected);
        }
    }
    if !fs.as_object().unwrap().is_empty() {
        // Keep native deny entries; dropping them could broaden a selected glob/path grant.
        if let Some(entries) = request["fileSystem"]["entries"].as_array() {
            for entry in entries.iter().filter(|e| e["access"] == "deny") {
                if !fs["entries"].is_array() {
                    fs["entries"] = json!([]);
                }
                let dest = fs["entries"].as_array_mut().unwrap();
                if !dest.contains(entry) {
                    dest.push(entry.clone());
                }
            }
        }
        if let Some(depth) = request["fileSystem"].get("globScanMaxDepth") {
            fs["globScanMaxDepth"] = depth.clone();
        }
        permissions["fileSystem"] = fs;
    }
    if grant.network {
        ensure!(
            request["network"]["enabled"] == true,
            "Network permission was not requested"
        );
        permissions["network"] = json!({"enabled":true});
    }
    Ok(
        json!({"permissions":permissions,"scope":grant.scope,"strictAutoReview":grant.strict_review}),
    )
}
