use super::*;
impl Codex {
    pub(super) fn turn_settings_snapshot(&self, params: &Value) -> BTreeMap<String, ControlValue> {
        let mut values: BTreeMap<String, ControlValue> = [
            "model",
            "effort",
            "serviceTier",
            "summary",
            "approvalsReviewer",
        ]
        .into_iter()
        .filter_map(|key| {
            self.effective
                .get(key)
                .cloned()
                .map(|value| (key.into(), value))
        })
        .collect();
        if let Some(model) = &self.selected {
            values.insert("model".into(), ControlValue::Text(model.clone()));
        }
        for key in [
            "model",
            "effort",
            "serviceTier",
            "summary",
            "approvalsReviewer",
        ] {
            if let Some(value) = params.get(key).and_then(ControlValue::from_safe) {
                values.insert(key.into(), value);
            }
        }
        // Native collaboration settings take precedence over top-level turn fields.
        for (key, native) in [("model", "model"), ("effort", "reasoning_effort")] {
            if let Some(value) = params["collaborationMode"]["settings"]
                .get(native)
                .and_then(ControlValue::from_safe)
            {
                values.insert(key.into(), value);
            }
        }
        values
    }
    pub(super) fn adopt_turn(&mut self, turn: String) {
        if turn.is_empty() {
            return;
        }
        if self
            .active_turn_settings
            .as_ref()
            .is_none_or(|state| state.turn != turn)
        {
            let values = self
                .pending_turn_settings
                .take()
                .unwrap_or_else(|| self.turn_settings_snapshot(&json!({})));
            self.active_turn_settings = Some(ActiveTurnSettings {
                turn: turn.clone(),
                values,
            });
        }
        self.turn = turn;
    }

    pub(super) fn publish_active_turn(&self, e: &mut Effects) {
        let mut fields = self.active_turn_settings.as_ref().map(|state| state.values.clone()).unwrap_or_default();
        if let Some(state) = &self.active_turn_settings {
            fields.insert("turnId".into(), ControlValue::Text(state.turn.clone()));
        }
        e.event(Event::Controls(ControlEvent::Status(NativeStatus {
            subject: "active_turn_settings".into(), fields,
            unavailable: self.active_turn_settings.is_none().then(|| "No active native turn".into()),
        })));
    }
    pub(super) fn action(&mut self, action: &CodexAction, e: &mut Effects) -> Result<()> {
        ensure!(self.ready, "Codex has no ready native thread");
        if let CodexAction::AcceptEffectiveSettings { operation } = action {
            let op = self
                .native
                .operation
                .as_ref()
                .context("No pending settings operation")?;
            ensure!(
                op.id == *operation && self.settings_ack && self.settings_notification,
                "Native settings have not reached an ACK and state boundary"
            );
            let current = self.effective.get(&op.control).cloned();
            self.native.finish(ApplicationState::Unconfirmed,current,Some("User accepted the current native state; the earlier requested value remains unconfirmed.".into()),e);
            self.settings_deadline = None;
            return Ok(());
        }
        self.native.idle()?;
        let live = matches!(
            action,
            CodexAction::Rename { .. }
                | CodexAction::UpdateActiveTurn { .. }
                | CodexAction::AddQueuedTask { .. }
                | CodexAction::UpdateQueuedTask { .. }
                | CodexAction::DeleteQueuedTask { .. }
        );
        ensure!(
            live || !self.busy,
            "This action requires an idle conversation"
        );
        let mut active_change = None;
        let (key, method, params) = match action {
            CodexAction::Rename { name } => {
                let name = name.trim();
                ensure!(!name.is_empty(), "A conversation name is required");
                (
                    "rename",
                    "thread/name/set",
                    json!({"threadId":self.thread,"name":name}),
                )
            }
            CodexAction::SetGoal {
                objective,
                status,
                token_budget,
            } => {
                if let Some(text) = objective {
                    valid_text(text)?;
                }
                ensure!(
                    token_budget.is_none_or(|v| v > 0 && v <= i64::MAX as u64),
                    "Invalid goal token budget"
                );
                ensure!(
                    objective.is_some() || status.is_some() || token_budget.is_some(),
                    "No goal change requested"
                );
                (
                    "goal",
                    "thread/goal/set",
                    json!({"threadId":self.thread,"objective":objective,"status":status,"tokenBudget":token_budget,"origin":"user"}),
                )
            }
            CodexAction::ClearGoal => (
                "goal-clear",
                "thread/goal/clear",
                json!({"threadId":self.thread,"origin":"user"}),
            ),
            CodexAction::AddQueuedTask { text } => {
                valid_text(text)?;
                (
                    "queue-add",
                    "thread/queue/add",
                    json!({"threadId":self.thread,"clientUserMessageId":journal::unique(),"input":[{"type":"text","text":text,"text_elements":[]}]}),
                )
            }
            CodexAction::UpdateQueuedTask { id, text } => {
                valid_text(text)?;
                ensure!(
                    self.queue.iter().any(|v| v.id == *id && v.editable),
                    "This queued task is stale or contains unsupported input"
                );
                (
                    "queue-update",
                    "thread/queue/update",
                    json!({"threadId":self.thread,"queuedSubmissionId":id,"input":[{"type":"text","text":text,"text_elements":[]}]}),
                )
            }
            CodexAction::DeleteQueuedTask { id } | CodexAction::StartQueuedTask { id } => {
                ensure!(
                    self.queue.iter().any(|v| v.id == *id),
                    "This queued task is stale"
                );
                (
                    if matches!(action, CodexAction::DeleteQueuedTask { .. }) {
                        "queue-delete"
                    } else {
                        "queue-start"
                    },
                    if matches!(action, CodexAction::DeleteQueuedTask { .. }) {
                        "thread/queue/delete"
                    } else {
                        "thread/queue/start"
                    },
                    json!({"threadId":self.thread,"queuedSubmissionId":id}),
                )
            }
            CodexAction::Review(target) => {
                match target {
                    ReviewTarget::BaseBranch { branch } => valid_text(branch)?,
                    ReviewTarget::Commit { sha } => ensure!(
                        (7..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()),
                        "Invalid commit hash"
                    ),
                    ReviewTarget::Custom { instructions } => valid_text(instructions)?,
                    _ => {}
                }
                (
                    "review",
                    "review/start",
                    json!({"threadId":self.thread,"target":target,"delivery":"inline"}),
                )
            }
            CodexAction::UpdateActiveTurn {
                turn_id,
                control,
                value,
            } => {
                ensure!(
                    self.busy
                        && !self.turn.is_empty()
                        && self.turn == *turn_id
                        && self.stopping.is_none(),
                    "The selected turn is no longer active"
                );
                ensure!(
                    [
                        "model",
                        "effort",
                        "serviceTier",
                        "summary",
                        "approvalsReviewer"
                    ]
                    .contains(&control.as_str()),
                    "This setting is not supported for an active turn"
                );
                let state = self
                    .active_turn_settings
                    .as_ref()
                    .filter(|state| state.turn == *turn_id)
                    .context("Settings for this active turn are unavailable")?;
                let mut option = self
                    .controls
                    .iter()
                    .find(|o| o.id == *control)
                    .context("Setting unavailable")?
                    .clone();
                if control == "effort" || control == "serviceTier" {
                    let selected = state.values.get("model");
                    let meta = self
                        .metadata
                        .iter()
                        .find(|m| {
                            selected == Some(&ControlValue::Text(m.id.clone()))
                                || m.resolved_model.as_ref().is_some_and(|model| {
                                    selected == Some(&ControlValue::Text(model.clone()))
                                })
                        })
                        .context("Active-turn model metadata is unavailable")?;
                    option.choices = if control == "effort" {
                        meta.efforts.clone()
                    } else {
                        meta.tiers.clone()
                    };
                    option.available = control == "serviceTier" || !option.choices.is_empty();
                }
                option.validate(value)?;
                ensure!(
                    !matches!(value, ControlValue::Default) || control == "serviceTier",
                    "Null leaves this active-turn value unchanged"
                );
                let mut params = json!({"threadId":self.thread,"turnId":turn_id});
                params[control] = if control == "model" {
                    self.metadata
                        .iter()
                        .find(|m| ControlValue::Text(m.id.clone()) == *value)
                        .and_then(|m| m.resolved_model.clone())
                        .map(Value::String)
                        .unwrap_or_else(|| value.wire())
                } else {
                    value.wire()
                };
                if control == "model" {
                    let meta = self
                        .metadata
                        .iter()
                        .find(|m| ControlValue::Text(m.id.clone()) == *value)
                        .context("Active-turn model metadata is unavailable")?;
                    params["effort"] = json!(
                        meta.default_effort
                            .as_ref()
                            .context("The model did not advertise a default effort")?
                    );
                    params["serviceTier"] = json!(meta.default_tier);
                }
                let values = [
                    "model",
                    "effort",
                    "serviceTier",
                    "summary",
                    "approvalsReviewer",
                ]
                .into_iter()
                .filter_map(|key| {
                    params
                        .get(key)
                        .and_then(ControlValue::from_safe)
                        .map(|value| (key.into(), value))
                })
                .collect();
                active_change = Some(ActiveTurnChange {
                    turn: turn_id.clone(),
                    control: control.clone(),
                    value: value.clone(),
                    values,
                });
                ("active-turn", "turn/settings/update", params)
            }
            CodexAction::SetMemoryEnabled(enabled) => (
                "memory",
                "thread/memoryMode/set",
                json!({"threadId":self.thread,"mode":if *enabled{"enabled"}else{"disabled"}}),
            ),
            CodexAction::ReloadMcp => ("mcp-reload", "config/mcpServer/reload", json!({})),
            CodexAction::AcceptEffectiveSettings { .. } => unreachable!(),
        };
        let id = self.native.begin(
            &format!("action:{key}"),
            active_change
                .as_ref()
                .map(|change| change.value.clone())
                .unwrap_or(ControlValue::Default),
            e,
        )?;
        self.pending_active_change = active_change;
        e.write(rpc(json!(id), method, params));
        if matches!(key, "review" | "queue-start") {
            self.busy = true;
            self.turn.clear();
            self.active_turn_settings = None;
            self.pending_turn_settings = Some(self.turn_settings_snapshot(&json!({})));
            e.state(Lifecycle::Running);
        }
        Ok(())
    }
    pub(super) fn action_result(&mut self, id: &str, v: &Value, e: &mut Effects) -> bool {
        let Some(op) = &self.native.operation else {
            return false;
        };
        if op.id != id || !op.control.starts_with("action:") {
            return false;
        }
        let key = op.control.clone();
        if v.get("error").is_some() {
            self.pending_active_change = None;
            self.native.finish(
                ApplicationState::Rejected,
                None,
                Some("The native action was rejected.".into()),
                e,
            );
            if ["action:review", "action:queue-start", "action:compact"].contains(&key.as_str()) {
                self.busy = false;
                self.compacting = false;
                e.state(Lifecycle::Ready);
            }
            return true;
        }
        let r = &v["result"];
        match key.as_str() {
            "action:goal" => e.event(status(
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
            "action:goal-clear" => e.event(status("goal", r, &["cleared"])),
            "action:queue-add" | "action:queue-update" | "action:queue-delete" => {
                self.discovery(Discovery::Queue, e)
            }
            "action:review" | "action:queue-start" => {
                self.adopt_turn(s(&r["turn"], "id"));
                if self.interrupt_pending && !self.turn.is_empty() {
                    self.interrupt_pending = false;
                    self.interrupt(e);
                }
                self.discovery(Discovery::Queue, e);
            }
            "action:mcp-reload" => self.discovery(Discovery::Mcp, e),
            "action:memory" => self.discovery(Discovery::Memory, e),
            _ => {}
        }
        let (state, message) = if key == "action:active-turn" {
            let change = self.pending_active_change.take();
            if r["status"] == "applied"
                && change.as_ref().is_some_and(|change| {
                    self.busy
                        && self.turn == change.turn
                        && self
                            .active_turn_settings
                            .as_ref()
                            .is_some_and(|state| state.turn == change.turn)
                })
            {
                let change = change.unwrap();
                let state = self.active_turn_settings.as_mut().unwrap();
                state.values.extend(change.values);
                let mut fields = state.values.clone();
                fields.insert("turnId".into(), ControlValue::Text(state.turn.clone()));
                fields.insert("changedControl".into(), ControlValue::Text(change.control));
                e.event(Event::Controls(ControlEvent::Status(NativeStatus {
                    subject: "active_turn_settings".into(),
                    fields,
                    unavailable: None,
                })));
                (
                    ApplicationState::Applied,
                    "Published to subsequent captures of the selected turn; already captured work is unchanged.",
                )
            } else {
                (
                    ApplicationState::Rejected,
                    "The native target turn is no longer available.",
                )
            }
        } else {
            (
                ApplicationState::Acknowledged,
                if matches!(
                    key.as_str(),
                    "action:compact" | "action:review" | "action:queue-start"
                ) {
                    "Native execution started; completion follows native turn/item events."
                } else {
                    "Native action acknowledged; refreshed state is reported separately."
                },
            )
        };
        self.native.finish(state, None, Some(message.into()), e);
        true
    }
    pub(super) fn queue_snapshot(&mut self, r: &Value, e: &mut Effects) {
        e.event(coverage(
            "queue",
            &r["data"],
            r["data"].as_array().map(Vec::len).unwrap_or(0).min(256),
            r["nextCursor"].is_string(),
        ));
        self.queue = r["data"]
            .as_array()
            .into_iter()
            .flatten()
            .take(256)
            .map(|q| {
                let blocks = q["input"].as_array();
                QueuedTask {
                    id: s(q, "id"),
                    text: blocks
                        .into_iter()
                        .flatten()
                        .filter(|b| b["type"] == "text")
                        .map(|b| s(b, "text"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    editable: blocks.is_some_and(|v| v.iter().all(|b| b["type"] == "text")),
                }
            })
            .collect();
        e.event(Event::Controls(ControlEvent::Queue(self.queue.clone())));
        if r["nextCursor"].is_string() {
            e.event(unavailable("queue_snapshot_partial"));
        }
    }
    pub(super) fn capabilities(&self, e: &mut Effects) {
        let methods: Vec<String> = serde_json::from_str(include_str!("codex_method_catalog.json"))
            .expect("static native inventory");
        let supported = [
            "model/list",
            "thread/settings/update",
            "turn/settings/update",
            "collaborationMode/list",
            "permissionProfile/list",
            "configRequirements/read",
            "config/read",
            "skills/list",
            "mcpServerStatus/list",
            "thread/read",
            "thread/name/set",
            "experimentalFeature/list",
            "thread/goal/get",
            "thread/goal/set",
            "thread/goal/clear",
            "thread/queue/list",
            "thread/queue/add",
            "thread/queue/update",
            "thread/queue/delete",
            "thread/queue/start",
            "review/start",
            "thread/compact/start",
            "thread/memory/read",
            "thread/memoryMode/set",
            "config/mcpServer/reload",
        ];
        e.event(Event::Controls(ControlEvent::Capabilities(methods.into_iter().map(|id|{let available=supported.contains(&id.as_str());let description=if id.starts_with("plugin/")||id.starts_with("marketplace/"){"Native plugin API is under development; production dispatch is disabled."}else if available{"Typed native transport is available; native policy and runtime errors remain authoritative."}else{"Native capability inventory only. Requires a dedicated host workflow or native terminal; no GUI action is dispatched."};NativeCapability{label:id.clone(),id,scope:if available{ControlScope::Live}else{ControlScope::Inherited},available,description:description.into(),documentation:"https://learn.chatgpt.com/docs/app-server".into()}}).collect())));
    }
}
fn valid_text(text: &str) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= 120_000 && !text.contains('\0'),
        "Invalid or oversized native action text"
    );
    Ok(())
}
