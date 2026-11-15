//! Claude stream-json control protocol, matching the official Agent SDK transport.
use super::*;
pub(in crate::app::agent_gui) struct Claude {
    resume: Option<String>,
    session: String,
    ready: bool,
    busy: bool,
    pending: BTreeMap<String, Value>,
    tools: BTreeMap<String, ToolCall>,
    blocks: BTreeMap<usize, String>,
    message: String,
    partial_input: BTreeMap<String, String>,
    deferred: Option<Value>,
    selected: Option<String>,
}
impl Claude {
    pub fn new(resume: Option<String>) -> Self {
        Self {
            session: resume.clone().unwrap_or_default(),
            resume,
            ready: false,
            busy: false,
            pending: BTreeMap::new(),
            tools: BTreeMap::new(),
            blocks: BTreeMap::new(),
            message: String::new(),
            partial_input: BTreeMap::new(),
            deferred: None,
            selected: None,
        }
    }
    pub fn arguments(&self) -> Vec<String> {
        let mut a = vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        if let Some(id) = &self.resume {
            a.extend(["--resume".into(), id.clone()]);
        }
        a
    }
    fn control(id: &str, request: Value) -> Value {
        json!({"type":"control_request","request_id":id,"request":request})
    }
    fn response(id: &str, response: Value) -> Value {
        json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}})
    }
    pub fn start(&mut self) -> Effects {
        Effects {
            writes: vec![Self::control("initialize", json!({"subtype":"initialize"}))],
            events: vec![],
            accepted: false,
            cancelled: false,
        }
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
                    "Claude is not ready for a new turn"
                );
                let mut content = vec![json!({"type":"text","text":text})];
                content.extend(image_blocks(id, attachments, Tool::Claude)?);
                let user = json!({"type":"user","message":{"role":"user","content":content},"session_id":self.session,"parent_tool_use_id":null});
                if model.is_some() && model != &self.selected {
                    self.deferred = Some(user);
                    self.selected = model.clone();
                    e.write(Self::control(
                        "set-model",
                        json!({"subtype":"set_model","model":model}),
                    ));
                } else {
                    e.write(user);
                }
                self.busy = true;
                e.state(Lifecycle::Running);
            }
            Command::Respond {
                request_id,
                decision,
                answers,
            } => {
                let r = self
                    .pending
                    .remove(request_id)
                    .context("this Claude request is no longer pending")?;
                let allow = matches!(decision, Decision::Allow);
                let mut input = r["input"].clone();
                if allow && !answers.is_empty() {
                    let map: serde_json::Map<String, Value> = answers
                        .iter()
                        .map(|(k, v)| (k.clone(), json!(v.join(", "))))
                        .collect();
                    input["answers"] = Value::Object(map);
                }
                e.write(Self::response(
                    request_id,
                    if allow {
                        json!({"behavior":"allow","updatedInput":input})
                    } else {
                        json!({"behavior":"deny","message":"User declined this request"})
                    },
                ));
                e.event(Event::RequestResolved(request_id.clone()));
                e.state(Lifecycle::Running);
            }
            Command::Interrupt => {
                if self.deferred.take().is_some() {
                    self.busy = false;
                    e.cancelled = true;
                    e.state(Lifecycle::Ready);
                }
                for (request, _) in std::mem::take(&mut self.pending) {
                    e.write(Self::response(
                        &request,
                        json!({"behavior":"deny","message":"User interrupted","interrupt":true}),
                    ));
                    e.event(Event::RequestResolved(request));
                }
                e.write(Self::control("interrupt", json!({"subtype":"interrupt"})));
            }
            Command::SetMode { .. } => anyhow::bail!("this provider does not expose session modes"),
            Command::Restart => anyhow::bail!("host restart is handled by the runtime"),
        }
        Ok(e)
    }
    fn tool(&mut self, id: String, name: String, input: Value, e: &mut Effects) {
        let tool = self.tools.entry(id.clone()).or_insert_with(|| ToolCall {
            id,
            name: name.clone(),
            ..Default::default()
        });
        if !name.is_empty() {
            tool.name = name;
        }
        if !input.is_null() {
            tool.input = input;
        }
        tool.status = ToolStatus::Running;
        if tool.input.get("old_string").is_some() {
            tool.diff = format!(
                "- {}\n+ {}",
                s(&tool.input, "old_string"),
                s(&tool.input, "new_string")
            );
        }
        e.event(Event::Tool(tool.clone()));
    }
    pub fn ingest(&mut self, v: Value) -> Result<Effects> {
        let mut e = Effects::default();
        match s(&v, "type").as_str() {
            "control_response" => {
                let r = &v["response"];
                let id = s(r, "request_id");
                if s(r, "subtype") == "error" {
                    anyhow::bail!("Claude control request {id} failed: {}", s(r, "error"));
                }
                if id == "initialize" {
                    self.ready = true;
                    e.event(Event::Models {
                        choices: model_choices(&r["response"]["models"], "value", "displayName"),
                        selected: self.selected.clone(),
                    });
                    e.state(Lifecycle::Ready);
                }
                if id == "set-model" {
                    if let Some(user) = self.deferred.take() {
                        e.write(user);
                    }
                }
            }
            "control_request" => {
                let id = s(&v, "request_id");
                let r = &v["request"];
                if s(r, "subtype") == "can_use_tool" {
                    self.pending.insert(id.clone(), r.clone());
                    let name = s(r, "tool_name");
                    let tool_id = s(r, "tool_use_id");
                    if !tool_id.is_empty() {
                        self.tool(tool_id, name.clone(), r["input"].clone(), &mut e);
                    }
                    e.event(Event::Request(Request {
                        id,
                        title: name,
                        questions: questions(&r["input"]["questions"]),
                        options: vec![],
                        details: r["input"].clone(),
                    }));
                    e.state(Lifecycle::Waiting);
                } else {
                    e.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Unsupported client control request"}}));
                }
            }
            "control_cancel_request" => {
                let id = s(&v, "request_id");
                self.pending.remove(&id);
                e.event(Event::RequestResolved(id));
            }
            "system" => {
                if let Some(id) = v
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.session = id.into();
                    e.event(Event::Session(id.into()));
                }
                if let Some(model) = v.get("model").and_then(Value::as_str) {
                    self.selected = Some(model.into());
                    e.event(Event::Models {
                        choices: vec![],
                        selected: self.selected.clone(),
                    });
                }
            }
            "stream_event" => {
                e.accepted = true;
                if let Some(id) = v
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    if self.session != id {
                        self.session = id.into();
                        e.event(Event::Session(id.into()));
                    }
                }
                let stream = &v["event"];
                let index = stream["index"].as_u64().unwrap_or(0) as usize;
                match s(stream, "type").as_str() {
                    "message_start" => {
                        self.message = s(&stream["message"], "id");
                        self.blocks.clear();
                    }
                    "content_block_start" => {
                        let block = &stream["content_block"];
                        if s(block, "type") == "tool_use" {
                            let id = s(block, "id");
                            self.blocks.insert(index, id.clone());
                            self.tool(id, s(block, "name"), block["input"].clone(), &mut e);
                        } else if s(block, "type") == "text" && !s(block, "text").is_empty() {
                            e.event(message(
                                format!("{}:{index}", self.message),
                                s(block, "text"),
                                false,
                            ));
                        }
                    }
                    "content_block_delta" => {
                        let delta = &stream["delta"];
                        if s(delta, "type") == "text_delta" {
                            e.event(message(
                                format!("{}:{index}", self.message),
                                s(delta, "text"),
                                true,
                            ));
                        }
                        if s(delta, "type") == "input_json_delta"
                            && let Some(id) = self.blocks.get(&index)
                        {
                            let part = s(delta, "partial_json");
                            let input = self.partial_input.entry(id.clone()).or_default();
                            ensure!(
                                input.len().saturating_add(part.len()) <= journal::MAX_RECORD,
                                "Claude streamed tool input exceeded the frame limit"
                            );
                            input.push_str(&part);
                        }
                    }
                    "content_block_stop" => {
                        if let Some(id) = self.blocks.get(&index).cloned()
                            && let Some(input) = self.partial_input.remove(&id)
                            && let Ok(input) = serde_json::from_str(&input)
                        {
                            self.tool(id, String::new(), input, &mut e);
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                e.accepted = true;
                let m = &v["message"];
                let id = s(m, "id");
                for (index, block) in m["content"].as_array().into_iter().flatten().enumerate() {
                    match s(block, "type").as_str() {
                        "text" => {
                            e.event(message(format!("{id}:{index}"), s(block, "text"), false))
                        }
                        "tool_use" => self.tool(
                            s(block, "id"),
                            s(block, "name"),
                            block["input"].clone(),
                            &mut e,
                        ),
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if s(block, "type") == "tool_result" {
                        let id = s(block, "tool_use_id");
                        let tool = self.tools.entry(id.clone()).or_insert_with(|| ToolCall {
                            id,
                            ..Default::default()
                        });
                        tool.output = if let Some(a) = block["content"].as_array() {
                            a.iter()
                                .map(|b| s(b, "text"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            text(&block["content"])
                        };
                        tool.status = if block["is_error"].as_bool() == Some(true) {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Completed
                        };
                        e.event(Event::Tool(tool.clone()));
                    }
                }
            }
            "result" => {
                e.accepted = true;
                if !s(&v, "session_id").is_empty() {
                    self.session = s(&v, "session_id");
                    e.event(Event::Session(self.session.clone()));
                }
                self.busy = false;
                if v["is_error"].as_bool() == Some(true) {
                    e.event(Event::Error(
                        v.get("errors").map(text).unwrap_or_else(|| s(&v, "result")),
                    ));
                }
                e.state(Lifecycle::Ready);
            }
            _ => {}
        }
        Ok(e)
    }
}
