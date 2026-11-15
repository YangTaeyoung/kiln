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
    deferred: Option<(String, Option<String>, Option<Value>)>,
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
        }
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
        match c {
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
                let mut prompt = vec![json!({"type":"text","text":text})];
                prompt.extend(image_blocks(id, attachments, Tool::Antigravity)?);
                let user = rpc(
                    json!("prompt"),
                    "session/prompt",
                    json!({"sessionId":self.session,"prompt":prompt}),
                );
                if model.is_some() && model != &self.selected {
                    ensure!(
                        self.models.iter().any(|m| Some(&m.id) == model.as_ref()),
                        "model is not available in this Antigravity session"
                    );
                    let (method, params) = if let Some(config) = &self.model_config {
                        (
                            "session/set_config_option",
                            json!({"sessionId":self.session,"configId":config,"value":model}),
                        )
                    } else {
                        (
                            "session/set_model",
                            json!({"sessionId":self.session,"modelId":model}),
                        )
                    };
                    let operation = format!("config-{}", journal::unique());
                    self.deferred = Some((operation.clone(), model.clone(), Some(user)));
                    e.write(rpc(json!(operation), method, params));
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
                let operation = format!("config-{}", journal::unique());
                self.deferred = Some((operation.clone(), None, None));
                self.busy = true;
                e.write(rpc(json!(operation), method, params));
                e.state(Lifecycle::Running);
            }
            Command::Interrupt => {
                if self.deferred.take().is_some() {
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
    fn options(&mut self, r: &Value, e: &mut Effects) {
        if let Some(models) = r.get("models") {
            self.models = model_choices(&models["availableModels"], "modelId", "name");
            self.selected = models
                .get("currentModelId")
                .and_then(Value::as_str)
                .map(String::from);
            e.event(Event::Models {
                choices: self.models.clone(),
                selected: self.selected.clone(),
            });
        }
        if let Some(modes) = r.get("modes") {
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
                selected: modes
                    .get("currentModeId")
                    .and_then(Value::as_str)
                    .map(String::from),
            });
        }
        for option in r["configOptions"].as_array().into_iter().flatten() {
            let mut values = Vec::new();
            for group in option["options"].as_array().into_iter().flatten() {
                if let Some(nested) = group.get("options").and_then(Value::as_array) {
                    values.extend(nested.iter());
                } else {
                    values.push(group);
                }
            }
            match s(option, "category").as_str() {
                "model" => {
                    self.model_config = Some(s(option, "id"));
                    self.models = values
                        .iter()
                        .map(|v| Model {
                            id: s(v, "value"),
                            label: s(v, "name"),
                        })
                        .collect();
                    self.selected = Some(s(option, "currentValue"));
                    e.event(Event::Models {
                        choices: self.models.clone(),
                        selected: self.selected.clone(),
                    });
                }
                "mode" => {
                    self.mode_config = Some(s(option, "id"));
                    self.modes = values
                        .iter()
                        .map(|v| Choice {
                            id: s(v, "value"),
                            label: s(v, "name"),
                            description: s(v, "description"),
                        })
                        .collect();
                    e.event(Event::Modes {
                        choices: self.modes.clone(),
                        selected: Some(s(option, "currentValue")),
                    });
                }
                _ => {}
            }
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
            if id.starts_with("config-")
                && self
                    .deferred
                    .as_ref()
                    .is_none_or(|(pending, _, _)| pending != &id)
            {
                return Ok(e);
            }
            if let Some(error) = v.get("error") {
                anyhow::bail!("Antigravity {id} failed: {}", text(error));
            }
            let r = &v["result"];
            if self
                .deferred
                .as_ref()
                .is_some_and(|(pending, _, _)| pending == &id)
            {
                let (_, model, user) = self.deferred.take().unwrap();
                if let Some(model) = model {
                    self.selected = Some(model);
                }
                self.options(r, &mut e);
                e.event(Event::Models {
                    choices: vec![],
                    selected: self.selected.clone(),
                });
                if let Some(user) = user {
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
            if self.loading {
                return Ok(e);
            }
            let u = &p["update"];

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
                    selected: Some(s(u, "currentModeId")),
                }),
                "config_option_update" => self.options(u, &mut e),
                _ => {}
            }
        }
        Ok(e)
    }
}
