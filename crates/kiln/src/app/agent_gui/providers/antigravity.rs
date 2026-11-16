//! ACP v1. No filesystem/terminal capabilities are advertised: tools stay in agy.
use super::*;
pub(in crate::app::agent_gui) struct Antigravity {
    cwd: PathBuf,
    resume: Option<String>,
    session: String,
    ready: bool,
    busy: bool,
    loading: bool,
    image: bool,
    pending: BTreeMap<String, (Value, Value)>,
    tools: BTreeMap<String, ToolCall>,
    message: String,
    selected: Option<String>,
    model_config: Option<String>,
    mode_config: Option<String>,
    modes: Vec<Choice>,
    models: Vec<Model>,
    deferred: Option<ConfigOperation>,
    retired_configs: BTreeSet<String>,
    model_settled: bool,
    controls: Vec<ControlOption>,
    native: NativeFlight,
    config_ack: bool,
}
impl Antigravity {
    pub fn new(cwd: PathBuf, resume: Option<String>) -> Self {
        Self {
            cwd,
            session: resume.clone().unwrap_or_default(),
            resume,
            ready: false,
            busy: false,
            loading: false,
            image: false,
            pending: BTreeMap::new(),
            tools: BTreeMap::new(),
            message: String::new(),
            selected: None,
            model_config: None,
            mode_config: None,
            modes: vec![],
            models: vec![],
            deferred: None,
            retired_configs: BTreeSet::new(),
            model_settled: true,
            controls: vec![],
            native: NativeFlight::default(),
            config_ack: false,
        }
    }
    pub fn tick(&mut self) -> Effects {
        let mut e = self.native.tick();
        tick_config(&mut self.deferred, &mut e);
        e
    }
    pub(super) fn resuming(&self) -> bool {
        self.resume.is_some()
    }
    pub fn start(&mut self) -> Effects {
        let mut e = Effects::default();
        e.write(rpc(json!("initialize"),"initialize",json!({"protocolVersion":1,"clientInfo":{"name":"kiln","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})));
        e
    }
    fn permission(&self, options: &Value, decision: &Decision) -> Result<Value> {
        if matches!(decision, Decision::Cancel) {
            return Ok(json!({"outcome":{"outcome":"cancelled"}}));
        }
        let option = options
            .as_array()
            .into_iter()
            .flatten()
            .find(|o| match decision {
                Decision::Option(id) => s(o, "optionId") == *id,
                Decision::Allow => s(o, "kind") == "allow_once",
                Decision::Deny => s(o, "kind") == "reject_once",
                _ => false,
            })
            .context("the requested permission option is unavailable")?;
        Ok(json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}}))
    }
    pub fn command(&mut self, c: &Command, id: &str) -> Result<Effects> {
        let mut e = Effects::default();
        if !matches!(
            c,
            Command::Interrupt | Command::Respond { .. } | Command::Discover(_)
        ) {
            self.native.idle()?;
        }
        match c {
            Command::SubmitWithSkills { .. } => {
                anyhow::bail!("This provider uses native slash command drafts for skills")
            }
            Command::Configure { control, value } => {
                ensure!(
                    self.ready
                        && !self.busy
                        && self.deferred.is_none()
                        && self.retired_configs.is_empty(),
                    "Antigravity configuration is not ready"
                );
                self.controls
                    .iter()
                    .find(|o| o.id == *control)
                    .context("Unknown ACP option")?
                    .validate(value)?;
                let id = self.native.begin(control, value.clone(), &mut e)?;
                self.config_ack = false;
                e.write(rpc(
                    json!(id),
                    "session/set_config_option",
                    json!({"sessionId":self.session,"configId":control,"value":value.wire()}),
                ));
            }
            Command::Discover(_) => e.event(unavailable("discovery")),
            Command::NativeAction(_) => anyhow::bail!(
                "Use an advertised native command in the composer and Send explicitly"
            ),
            Command::Submit {
                text,
                attachments,
                model,
            } => {
                ensure!(
                    self.ready && !self.busy,
                    "Antigravity is not ready for a new turn"
                );
                ensure!(
                    attachments.is_empty() || self.image,
                    "this Antigravity session does not support images"
                );
                let target = if !self.model_settled {
                    Some(model.clone().or_else(||self.selected.clone()).context("The model configuration is unsettled; choose an explicit model before submitting")?)
                } else {
                    model.clone()
                };
                let mut prompt = vec![json!({"type":"text","text":text})];
                prompt.extend(image_blocks(id, attachments, Tool::Antigravity)?);
                let user = rpc(
                    json!("prompt"),
                    "session/prompt",
                    json!({"sessionId":self.session,"prompt":prompt}),
                );
                if !self.model_settled || (target.is_some() && target != self.selected) {
                    ensure!(
                        self.models.iter().any(|m| Some(&m.id) == target.as_ref()),
                        "model is not available in this Antigravity session"
                    );
                    let (method, params) = if let Some(config) = &self.model_config {
                        (
                            "session/set_config_option",
                            json!({"sessionId":self.session,"configId":config,"value":target}),
                        )
                    } else {
                        (
                            "session/set_model",
                            json!({"sessionId":self.session,"modelId":target}),
                        )
                    };
                    let id = format!("config-{}", journal::unique());
                    let wire = rpc(json!(id), method, params);
                    let operation = ConfigOperation {
                        id,
                        model: target,
                        mode: None,
                        prompt: Some(user),
                        wire,
                        sent: self.retired_configs.is_empty(),
                        started: std::time::Instant::now(),
                        timeout_reported: false,
                    };
                    if operation.sent {
                        e.write(operation.wire.clone());
                    }
                    self.deferred = Some(operation);
                    self.model_settled = false;
                    e.event(Event::Models {
                        choices: vec![],
                        selected: None,
                    });
                } else {
                    e.write(user);
                }
                self.message = format!("agy-{}", journal::unique());
                self.busy = true;
                e.state(Lifecycle::Running);
            }
            Command::Respond {
                request_id,
                decision,
                ..
            } => {
                let (id, options) = self
                    .pending
                    .get(request_id)
                    .context("this Antigravity request is no longer pending")?
                    .clone();
                let response = self.permission(&options, decision)?;
                self.pending.remove(request_id);
                e.write(result(id, response));
                e.event(Event::RequestResolved(request_id.clone()));
                e.state(Lifecycle::Running);
            }
            Command::SetMode { mode } => {
                ensure!(
                    self.ready && !self.busy && self.modes.iter().any(|m| m.id == *mode),
                    "mode is not available in this session"
                );
                let (method, params) = if let Some(config) = &self.mode_config {
                    (
                        "session/set_config_option",
                        json!({"sessionId":self.session,"configId":config,"value":mode}),
                    )
                } else {
                    (
                        "session/set_mode",
                        json!({"sessionId":self.session,"modeId":mode}),
                    )
                };
                let id = format!("config-{}", journal::unique());
                let wire = rpc(json!(id), method, params);
                let operation = ConfigOperation {
                    id,
                    model: None,
                    mode: Some(mode.clone()),
                    prompt: None,
                    wire,
                    sent: self.retired_configs.is_empty(),
                    started: std::time::Instant::now(),
                    timeout_reported: false,
                };
                if operation.sent {
                    e.write(operation.wire.clone());
                }
                self.deferred = Some(operation);
                self.model_settled = false;
                self.busy = true;
                e.event(Event::Modes {
                    choices: vec![],
                    selected: None,
                });
                e.event(Event::Models {
                    choices: vec![],
                    selected: None,
                });
                e.state(Lifecycle::Running);
            }
            Command::Interrupt => {
                self.native.cancel(&mut e);
                if cancel_config(&mut self.deferred, &mut self.retired_configs, &mut e) {
                    self.busy = false;
                    e.cancelled = true;
                    e.state(Lifecycle::Ready);
                }
                for (key, (id, _)) in std::mem::take(&mut self.pending) {
                    e.write(result(id, json!({"outcome":{"outcome":"cancelled"}})));
                    e.event(Event::RequestResolved(key));
                }
                e.write(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.session}}));
            }
            Command::Restart => anyhow::bail!("host restart is handled by the runtime"),
        }
        Ok(e)
    }
    fn settle_configuration(&mut self, e: &mut Effects) {
        if !self.config_ack {
            return;
        }
        let Some(op) = &self.native.operation else {
            return;
        };
        let actual = self
            .controls
            .iter()
            .find(|o| o.id == op.control)
            .and_then(|o| o.current.clone());
        if actual.as_ref() == Some(&op.value) {
            self.native
                .finish(ApplicationState::Applied, actual, None, e);
            self.config_ack = false;
        } else {
            e.event(op.outcome(ApplicationState::Unconfirmed, actual,
                Some("Native snapshot differs from the requested setting; Send remains blocked until confirmation.".into())));
        }
    }
    fn options(&mut self, r: &Value, e: &mut Effects) {
        if r.get("configOptions").is_some_and(|v| !v.is_array()) {
            e.event(unavailable("malformed_config_snapshot"));
            return;
        }
        let authoritative = self.deferred.is_none() && self.retired_configs.is_empty();
        if r.get("configOptions").is_none()
            && let Some(models) = r.get("models")
        {
            self.models = model_choices(&models["availableModels"], "modelId", "name");
            if authoritative
                && let Some(model) = models.get("currentModelId").and_then(Value::as_str)
            {
                self.selected = Some(model.into());
                self.model_settled = true;
            }
            e.event(Event::Models {
                choices: self.models.clone(),
                selected: if authoritative && self.model_settled {
                    self.selected.clone()
                } else {
                    None
                },
            });
        }
        if r.get("configOptions").is_none()
            && let Some(modes) = r.get("modes")
        {
            self.modes = modes["availableModes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| Choice {
                    id: s(m, "id"),
                    label: s(m, "name"),
                    description: s(m, "description"),
                })
                .collect();
            e.event(Event::Modes {
                choices: self.modes.clone(),
                selected: if authoritative {
                    modes
                        .get("currentModeId")
                        .and_then(Value::as_str)
                        .map(String::from)
                } else {
                    None
                },
            });
        }
        if r.get("configOptions").is_some() {
            self.controls = acp_options(&r["configOptions"]);
            let choices_partial = r["configOptions"]
                .as_array()
                .into_iter()
                .flatten()
                .zip(self.controls.iter())
                .any(|(raw, option)| {
                    let count: usize = raw["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|v| v["options"].as_array().map(Vec::len).unwrap_or(1))
                        .sum();
                    count > option.choices.len()
                });
            e.event(coverage(
                "config_options",
                &r["configOptions"],
                self.controls.len(),
                choices_partial,
            ));
            self.model_config = None;
            self.mode_config = None;
            self.models.clear();
            self.modes.clear();
            if authoritative {
                self.selected = None;
            }
            let mut selected_mode = None;
            for option in &self.controls {
                if !matches!(option.kind, ControlKind::Select) {
                    continue;
                }
                match option.category.as_deref() {
                    Some("model") => {
                        self.model_config = Some(option.id.clone());
                        self.models = option
                            .choices
                            .iter()
                            .map(|v| Model {
                                id: v.value.clone(),
                                label: v.label.clone(),
                            })
                            .collect();
                        if authoritative {
                            self.selected = option.current.as_ref().and_then(|v| {
                                if let ControlValue::Text(v) = v {
                                    Some(v.clone())
                                } else {
                                    None
                                }
                            });
                            self.model_settled = self.selected.is_some();
                        }
                    }
                    Some("mode") => {
                        self.mode_config = Some(option.id.clone());
                        self.modes = option
                            .choices
                            .iter()
                            .map(|v| Choice {
                                id: v.value.clone(),
                                label: v.label.clone(),
                                description: v.description.clone(),
                            })
                            .collect();
                        if authoritative {
                            selected_mode = option.current.as_ref().and_then(|v| {
                                if let ControlValue::Text(v) = v {
                                    Some(v.clone())
                                } else {
                                    None
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
            e.event(Event::Controls(ControlEvent::Options(
                self.controls.clone(),
            )));
            e.event(Event::Controls(ControlEvent::Quick {
                models: self.models.clone(),
                selected_model: if authoritative && self.model_settled {
                    self.selected.clone()
                } else {
                    None
                },
                modes: self.modes.clone(),
                selected_mode,
            }));
        }
    }

    fn tool(&mut self, u: &Value, e: &mut Effects) {
        let id = s(u, "toolCallId");
        if id.is_empty() {
            return;
        }
        let tool = self.tools.entry(id.clone()).or_insert_with(|| ToolCall {
            id,
            ..Default::default()
        });
        if let Some(title) = u.get("title").and_then(Value::as_str) {
            tool.name = title.into();
        }
        if u.get("kind").is_some() || u.get("name").is_some() || u.get("title").is_some() {
            let metadata = tool.presentation.get_or_insert_with(Default::default);
            for (key, slot) in [("kind", &mut metadata.kind), ("name", &mut metadata.native_name), ("title", &mut metadata.title)] {
                if let Some(value) = u.get(key).and_then(Value::as_str) {
                    *slot = Some(value.chars().take(2048).collect());
                }
            }
        }
        if let Some(input) = u.get("rawInput") {
            tool.input = input.clone();
        }
        if let Some(output) = u.get("rawOutput") {
            tool.output = text(output);
        }
        if let Some(content) = u.get("content").and_then(Value::as_array) {
            let mut outputs = Vec::new();
            let mut diffs = Vec::new();
            for item in content {
                match s(item, "type").as_str() {
                    "content" => {
                        if let Some(t) = item["content"].get("text").and_then(Value::as_str) {
                            outputs.push(t.to_owned());
                        }
                    }
                    "diff" => diffs.push(format!(
                        "{}\n- {}\n+ {}",
                        s(item, "path"),
                        s(item, "oldText"),
                        s(item, "newText")
                    )),
                    _ => {}
                }
            }
            if !outputs.is_empty() {
                tool.output = outputs.join("\n");
            }
            if !diffs.is_empty() {
                tool.diff = diffs.join("\n");
            }
        }
        if let Some(status) = u.get("status").and_then(Value::as_str) {
            tool.status = match status {
                "pending" => ToolStatus::Pending,
                "completed" => ToolStatus::Completed,
                "failed" => ToolStatus::Failed,
                _ => ToolStatus::Running,
            };
        }
        e.event(Event::Tool(tool.clone()));
    }
    pub fn ingest(&mut self, v: Value) -> Result<Effects> {
        let mut e = Effects::default();
        let method = s(&v, "method");
        let p = &v["params"];
        if !method.is_empty() && v.get("id").is_some() {
            if method == "session/request_permission" {
                ensure!(
                    s(p, "sessionId") == self.session,
                    "permission belongs to another session"
                );
                let id = v["id"].clone();
                let key = request_key(&id);
                self.pending.insert(key.clone(), (id, p["options"].clone()));
                self.tool(&p["toolCall"], &mut e);
                e.event(Event::Request(Request {
                    id: key,
                    title: s(&p["toolCall"], "title"),
                    questions: vec![],
                    options: p["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|o| Choice {
                            id: s(o, "optionId"),
                            label: s(o, "name"),
                            description: s(o, "kind"),
                        })
                        .collect(),
                    details: p["toolCall"].clone(),
                }));
                e.state(Lifecycle::Waiting);
            } else {
                e.write(unknown(v["id"].clone()));
            }
            return Ok(e);
        }
        if method.is_empty() && v.get("id").is_some() {
            let id = s(&v, "id");
            if id.starts_with("native-") {
                if self.native.operation.as_ref().is_none_or(|op| op.id != id) {
                    return Ok(e);
                }
                if v.get("error").is_some() {
                    self.config_ack = false;
                    self.native.finish(
                        ApplicationState::Rejected,
                        None,
                        Some("Native configuration was rejected.".into()),
                        &mut e,
                    );
                    return Ok(e);
                }
                self.config_ack = true;
                if !v["result"]["configOptions"].is_array() {
                    let op = self.native.operation.as_ref().unwrap();
                    e.event(op.outcome(ApplicationState::Unconfirmed, None,
                        Some("Native ACK returned no full snapshot; Send remains blocked until effective settings are confirmed.".into())));
                    return Ok(e);
                }
                self.options(&v["result"], &mut e);
                self.settle_configuration(&mut e);
                return Ok(e);
            }

            if (v.get("result").is_some() || v.get("error").is_some())
                && settle_retired_config(&id, &mut self.deferred, &mut self.retired_configs, &mut e)
            {
                return Ok(e);
            }
            if id.starts_with("config-")
                && self
                    .deferred
                    .as_ref()
                    .is_none_or(|operation| !operation.matches(&id))
            {
                return Ok(e);
            }
            if v.get("error").is_some() && self.deferred.as_ref().is_some_and(|op| op.matches(&id))
            {
                e.event(Event::Controls(ControlEvent::Settlement(id.clone())));
                self.deferred = None;
                self.busy = false;
                e.cancelled = true;
                e.event(Event::Error(
                    "Antigravity rejected the setting; no deferred task was sent.".into(),
                ));
                e.state(Lifecycle::Ready);
                return Ok(e);
            }
            if let Some(error) = v.get("error") {
                anyhow::bail!("Antigravity {id} failed: {}", text(error));
            }
            let r = &v["result"];
            if self
                .deferred
                .as_ref()
                .is_some_and(|operation| operation.matches(&id))
            {
                ensure!(
                    v.get("result").is_some(),
                    "invalid ACP configuration confirmation"
                );
                let operation = self.deferred.take().unwrap();
                e.event(Event::Controls(ControlEvent::Settlement(
                    operation.id.clone(),
                )));
                let config_ack = operation.wire["method"] == "session/set_config_option";
                if config_ack && !r["configOptions"].is_array() {
                    self.busy = false;
                    self.model_settled = false;
                    e.cancelled = true;
                    e.event(Event::Error("Native configuration ACK omitted the full settings snapshot; the task was not sent.".into()));
                    e.state(Lifecycle::Ready);
                    return Ok(e);
                }
                let target_model = operation.model.clone();
                let target_mode = operation.mode.clone();
                if let Some(model) = operation.model {
                    self.selected = Some(model);
                    self.model_settled = true;
                }
                if let Some(mode) = operation.mode {
                    e.event(Event::Modes {
                        choices: vec![],
                        selected: Some(mode),
                    });
                }
                self.options(r, &mut e);
                if config_ack
                    && (target_model
                        .as_ref()
                        .is_some_and(|model| self.selected.as_ref() != Some(model))
                        || target_mode.as_ref().is_some_and(|mode| {
                            !self.controls.iter().any(|o| {
                                o.category.as_deref() == Some("mode")
                                    && o.current == Some(ControlValue::Text(mode.clone()))
                            })
                        }))
                {
                    self.busy = false;
                    e.cancelled = true;
                    e.event(Event::Error("The returned native settings differ from the request; the task was not sent.".into()));
                    e.state(Lifecycle::Ready);
                    return Ok(e);
                }
                e.event(Event::Models {
                    choices: vec![],
                    selected: if self.model_settled {
                        self.selected.clone()
                    } else {
                        None
                    },
                });
                if let Some(user) = operation.prompt {
                    e.write(user);
                } else {
                    self.busy = false;
                    e.state(Lifecycle::Ready);
                }
                return Ok(e);
            }
            match id.as_str() {
                "initialize" => {
                    ensure!(
                        r["protocolVersion"].as_u64() == Some(1),
                        "unsupported Antigravity ACP version"
                    );
                    self.image = r["agentCapabilities"]["promptCapabilities"]["image"].as_bool()
                        == Some(true);
                    let (method, params) = if let Some(id) = &self.resume {
                        ensure!(
                            r["agentCapabilities"]["loadSession"].as_bool() == Some(true),
                            "Antigravity cannot resume this conversation; retained history is disconnected"
                        );
                        self.loading = true;
                        (
                            "session/load",
                            json!({"sessionId":id,"cwd":self.cwd,"mcpServers":[]}),
                        )
                    } else {
                        ("session/new", json!({"cwd":self.cwd,"mcpServers":[]}))
                    };
                    e.write(rpc(json!("session"), method, params));
                }
                "session" => {
                    if self.resume.is_none() {
                        self.session = s(r, "sessionId");
                    } else if let Some(id) = r.get("sessionId").and_then(Value::as_str) {
                        ensure!(
                            id == self.session,
                            "Antigravity resumed a different session"
                        );
                    }
                    ensure!(
                        !self.session.is_empty(),
                        "Antigravity did not return a native session ID"
                    );
                    self.loading = false;
                    self.ready = true;
                    e.event(Event::Session(self.session.clone()));
                    self.options(r, &mut e);
                    e.state(Lifecycle::Ready);
                }

                "prompt" => {
                    self.busy = false;
                    self.pending.clear();
                    e.accepted = true;
                    e.state(Lifecycle::Ready);
                }
                _ => {}
            }
            return Ok(e);
        }
        if method == "session/update" {
            ensure!(
                s(p, "sessionId") == self.session,
                "update belongs to another session"
            );
            // Loading replays native history; the durable journal already holds it. Do not duplicate it.
            let u = &p["update"];
            if self.loading
                && !matches!(
                    s(u, "sessionUpdate").as_str(),
                    "available_commands_update"
                        | "usage_update"
                        | "config_option_update"
                        | "current_mode_update"
                )
            {
                return Ok(e);
            }

            match s(u, "sessionUpdate").as_str() {
                "agent_message_chunk" => {
                    e.accepted = self.busy;
                    if let Some(t) = u["content"].get("text").and_then(Value::as_str) {
                        e.event(message(self.message.clone(), t.into(), true));
                    }
                }
                "tool_call" | "tool_call_update" => {
                    e.accepted = self.busy;
                    self.tool(u, &mut e);
                    self.message = format!("agy-{}", journal::unique());
                }
                "current_mode_update" => e.event(Event::Modes {
                    choices: vec![],
                    selected: if self.deferred.is_none() && self.retired_configs.is_empty() {
                        Some(s(u, "currentModeId"))
                    } else {
                        None
                    },
                }),
                "config_option_update" => {
                    self.options(u, &mut e);
                    if u["configOptions"].is_array() {
                        self.settle_configuration(&mut e);
                    }
                }
                "available_commands_update" => {
                    let entries = controls::native_entries(&u["availableCommands"]);
                    e.event(coverage(
                        "commands",
                        &u["availableCommands"],
                        entries.len(),
                        false,
                    ));
                    e.event(Event::Controls(ControlEvent::Catalog {
                        kind: CatalogKind::Commands,
                        entries,
                    }));
                }
                "usage_update" => e.event(status("context", u, &["used", "size"])),
                _ => {}
            }
        }
        Ok(e)
    }
}
