//! Codex app-server v2, verified against the installed generated JSON schemas.
use super::*;
pub(in crate::app::agent_gui) struct Codex {
    cwd: PathBuf,
    resume: Option<String>,
    thread: String,
    turn: String,
    ready: bool,
    busy: bool,
    interrupt_pending: bool,
    pending: BTreeMap<String, (Value, String, Value)>,
    tools: BTreeMap<String, ToolCall>,
    diffs: BTreeMap<String, ToolCall>,
    selected: Option<String>,
    pending_model: Option<String>,
}
impl Codex {
    pub fn new(cwd: PathBuf, resume: Option<String>) -> Self {
        Self {
            cwd,
            resume,
            thread: String::new(),
            turn: String::new(),
            ready: false,
            busy: false,
            interrupt_pending: false,
            pending: BTreeMap::new(),
            tools: BTreeMap::new(),
            diffs: BTreeMap::new(),
            selected: None,
            pending_model: None,
        }
    }
    pub fn start(&mut self) -> Effects {
        let mut e = Effects::default();
        e.write(rpc(json!("initialize"),"initialize",json!({"clientInfo":{"name":"kiln","title":"Kiln","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})));
        e
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
                    "Codex is not ready for a new turn"
                );
                let mut input = vec![json!({"type":"text","text":text,"text_elements":[]})];
                input.extend(image_blocks(id, attachments, Tool::Codex)?);
                e.write(rpc(
                    json!("turn"),
                    "turn/start",
                    json!({"threadId":self.thread,"input":input,"model":model}),
                ));
                self.busy = true;
                self.pending_model = model.clone();
                e.state(Lifecycle::Running);
            }
            Command::Respond {
                request_id,
                decision,
                answers,
            } => {
                let (id, method, p) = self
                    .pending
                    .get(request_id)
                    .context("this Codex request is no longer pending")?
                    .clone();
                let allow = matches!(decision, Decision::Allow);
                let cancelled = matches!(decision, Decision::Cancel);
                let response = match method.as_str() {
                    "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                        json!({"decision":if allow{"accept"}else if cancelled{"cancel"}else{"decline"}})
                    }
                    "execCommandApproval" | "applyPatchApproval" => {
                        json!({"decision":if allow{"approved"}else if cancelled{"abort"}else{"denied"}})
                    }
                    "item/permissions/requestApproval" => {
                        json!({"permissions":if allow{p["permissions"].clone()}else{json!({})},"scope":"turn"})
                    }
                    "item/tool/requestUserInput" => {
                        let map: serde_json::Map<String, Value> = answers
                            .iter()
                            .map(|(k, v)| (k.clone(), json!({"answers":v})))
                            .collect();
                        json!({"answers":map})
                    }
                    "mcpServer/elicitation/request" => {
                        let content = if allow {
                            form_content(&p["requestedSchema"], answers)?
                        } else {
                            Value::Null
                        };
                        json!({"action":if allow{"accept"}else if cancelled{"cancel"}else{"decline"},"content":content})
                    }
                    _ => anyhow::bail!("unsupported Codex request"),
                };
                self.pending.remove(request_id);
                e.write(result(id, response));
                e.event(Event::RequestResolved(request_id.clone()));
                e.state(Lifecycle::Running);
            }
            Command::Interrupt => {
                // Server clears live requests with serverRequest/resolved; reject late UI replies now.
                for (key, _) in std::mem::take(&mut self.pending) {
                    e.event(Event::RequestResolved(key));
                }
                if self.busy {
                    if self.turn.is_empty() {
                        self.interrupt_pending = true;
                    } else {
                        e.write(rpc(
                            json!("interrupt"),
                            "turn/interrupt",
                            json!({"threadId":self.thread,"turnId":self.turn}),
                        ));
                    }
                }
            }
            Command::SetMode { .. } => anyhow::bail!("this provider does not expose session modes"),
            Command::Restart => anyhow::bail!("host restart is handled by the runtime"),
        }
        Ok(e)
    }
    fn item(&mut self, item: &Value, complete: bool, e: &mut Effects) {
        let id = s(item, "id");
        let kind = s(item, "type");
        if id.is_empty() {
            return;
        }
        if kind == "agentMessage" {
            e.event(message(id, s(item, "text"), false));
            return;
        }
        if kind == "userMessage" || kind == "reasoning" {
            return;
        }
        let tool = self.tools.entry(id.clone()).or_insert_with(|| ToolCall {
            id,
            name: kind.clone(),
            ..Default::default()
        });
        tool.name = match kind.as_str() {
            "commandExecution" => s(item, "command"),
            "mcpToolCall" => format!("{} / {}", s(item, "server"), s(item, "tool")),
            _ => kind,
        };
        tool.input = item.clone();
        tool.status = match s(item, "status").as_str() {
            "failed" | "declined" => ToolStatus::Failed,
            "completed" => ToolStatus::Completed,
            _ if complete => ToolStatus::Completed,
            _ => ToolStatus::Running,
        };
        if let Some(output) = item.get("aggregatedOutput").filter(|v| !v.is_null()) {
            tool.output = text(output);
        }
        if let Some(output) = item.get("result").filter(|v| !v.is_null()) {
            tool.output = text(output);
        }
        if let Some(error) = item.get("error").filter(|v| !v.is_null()) {
            tool.output = text(error);
            tool.status = ToolStatus::Failed;
        }
        if let Some(changes) = item.get("changes").and_then(Value::as_array) {
            tool.diff = changes
                .iter()
                .map(|c| format!("{}\n{}", s(c, "path"), s(c, "diff")))
                .collect::<Vec<_>>()
                .join("\n");
        }
        e.event(Event::Tool(tool.clone()));
    }
    pub fn ingest(&mut self, v: Value) -> Result<Effects> {
        let mut e = Effects::default();
        let method = s(&v, "method");
        let p = &v["params"];
        if !method.is_empty() && v.get("id").is_some() {
            match method.as_str() {
                "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "item/tool/requestUserInput"
                | "mcpServer/elicitation/request"
                | "execCommandApproval"
                | "applyPatchApproval" => {
                    let id = v["id"].clone();
                    let key = request_key(&id);
                    self.pending
                        .insert(key.clone(), (id, method.clone(), p.clone()));
                    let questions = if method == "mcpServer/elicitation/request" {
                        form_questions(&p["requestedSchema"])
                    } else {
                        questions(&p["questions"])
                    };
                    e.event(Event::Request(Request {
                        id: key,
                        title: p
                            .get("message")
                            .or_else(|| p.get("reason"))
                            .and_then(Value::as_str)
                            .unwrap_or(&method)
                            .into(),
                        questions,
                        options: vec![],
                        details: p.clone(),
                    }));
                    e.state(Lifecycle::Waiting);
                }
                _ => e.write(unknown(v["id"].clone())),
            }
            return Ok(e);
        }
        if method.is_empty() && v.get("id").is_some() {
            let id = s(&v, "id");
            if let Some(error) = v.get("error") {
                anyhow::bail!("Codex {id} failed: {}", text(error));
            }
            let r = &v["result"];
            match id.as_str() {
                "initialize" => {
                    e.write(json!({"jsonrpc":"2.0","method":"initialized"}));
                    e.write(rpc(json!("models"), "model/list", json!({})));
                    let (method, params) = if let Some(id) = &self.resume {
                        (
                            "thread/resume",
                            json!({"threadId":id,"cwd":self.cwd,"excludeTurns":true}),
                        )
                    } else {
                        ("thread/start", json!({"cwd":self.cwd}))
                    };
                    e.write(rpc(json!("thread"), method, params));
                }
                "thread" => {
                    let id = s(&r["thread"], "id");
                    ensure!(!id.is_empty(), "Codex did not return a native thread ID");
                    if let Some(resume) = &self.resume {
                        ensure!(&id == resume, "Codex resumed a different conversation");
                    }
                    self.thread = id.clone();
                    self.ready = true;
                    e.event(Event::Session(id));
                    self.selected = r.get("model").and_then(Value::as_str).map(String::from);
                    e.event(Event::Models {
                        choices: vec![],
                        selected: self.selected.clone(),
                    });
                    e.state(Lifecycle::Ready);
                }
                "models" => e.event(Event::Models {
                    choices: model_choices(&r["data"], "id", "displayName"),
                    selected: self.selected.clone(),
                }),
                "turn" => {
                    if let Some(model) = self.pending_model.take() {
                        self.selected = Some(model);
                    }
                    e.event(Event::Models {
                        choices: vec![],
                        selected: self.selected.clone(),
                    });
                    self.turn = s(&r["turn"], "id");
                    e.accepted = true;
                    if std::mem::take(&mut self.interrupt_pending) {
                        e.write(rpc(
                            json!("interrupt"),
                            "turn/interrupt",
                            json!({"threadId":self.thread,"turnId":self.turn}),
                        ));
                    }
                }
                _ => {}
            }
            return Ok(e);
        }
        match method.as_str() {
            "thread/started" => {
                if self.thread.is_empty() {
                    self.thread = s(&p["thread"], "id");
                }
            }
            "turn/started" => {
                self.turn = s(&p["turn"], "id");
                e.accepted = true;
                e.state(Lifecycle::Running);
            }
            "item/agentMessage/delta" => e.event(message(s(p, "itemId"), s(p, "delta"), true)),
            "item/started" => self.item(&p["item"], false, &mut e),
            "item/completed" => self.item(&p["item"], true, &mut e),
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                let id = s(p, "itemId");
                let tool = self.tools.entry(id.clone()).or_insert_with(|| ToolCall {
                    id,
                    ..Default::default()
                });
                tool.output.push_str(&s(p, "delta"));
                e.event(Event::Tool(tool.clone()));
            }
            "turn/diff/updated" => {
                let turn = s(p, "turnId");
                let diff = ToolCall {
                    id: format!("diff-{}", s(p, "turnId")),
                    name: "Changes".into(),
                    diff: s(p, "diff"),
                    status: ToolStatus::Running,
                    ..Default::default()
                };
                self.diffs.insert(turn, diff.clone());
                e.event(Event::Tool(diff));
            }
            "serverRequest/resolved" => {
                let key = request_key(&p["requestId"]);
                self.pending.remove(&key);
                e.event(Event::RequestResolved(key));
            }
            "turn/completed" => {
                if let Some(mut diff) = self.diffs.remove(&s(&p["turn"], "id")) {
                    diff.status = match s(&p["turn"], "status").as_str() {
                        "interrupted" => ToolStatus::Cancelled,
                        "failed" => ToolStatus::Failed,
                        "completed" => ToolStatus::Completed,
                        _ => ToolStatus::Failed,
                    };
                    e.event(Event::Tool(diff));
                }
                self.busy = false;
                self.interrupt_pending = false;
                self.turn.clear();
                self.pending.clear();
                if let Some(error) = p["turn"].get("error").filter(|v| !v.is_null()) {
                    e.event(Event::Error(text(error)));
                }
                e.state(Lifecycle::Ready);
            }
            "error" => {
                e.event(Event::Error(text(&p["error"])));
                if p["willRetry"].as_bool() != Some(true) {
                    self.busy = false;
                    e.state(Lifecycle::Ready);
                }
            }
            "thread/closed" => {
                self.ready = false;
                e.state(Lifecycle::Disconnected);
            }
            _ => {}
        }
        Ok(e)
    }
}
fn form_questions(schema: &Value) -> Vec<Question> {
    schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, v)| {
            let options = if let Some(a) = v.get("enum").and_then(Value::as_array) {
                a.iter()
                    .map(|value| {
                        let id = value.as_str().unwrap_or_default().to_owned();
                        Choice {
                            id: id.clone(),
                            label: id,
                            description: String::new(),
                        }
                    })
                    .collect()
            } else {
                v["oneOf"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| Choice {
                        id: s(o, "const"),
                        label: s(o, "title"),
                        description: String::new(),
                    })
                    .collect()
            };
            Question {
                id: id.clone(),
                text: v.get("title").and_then(Value::as_str).unwrap_or(id).into(),
                options,
                multi_select: s(v, "type") == "array",
            }
        })
        .collect()
}
fn form_content(schema: &Value, answers: &BTreeMap<String, Vec<String>>) -> Result<Value> {
    let mut content = serde_json::Map::new();
    for (key, field) in schema["properties"].as_object().into_iter().flatten() {
        if let Some(answer) = answers.get(key) {
            let first = answer.first().map(String::as_str).unwrap_or_default();
            let value = match s(field, "type").as_str() {
                "boolean" => json!(
                    first
                        .parse::<bool>()
                        .context("answer must be true or false")?
                ),
                "number" | "integer" => {
                    let value =
                        serde_json::from_str::<Value>(first).context("answer must be a number")?;
                    ensure!(value.is_number(), "answer must be a number");
                    ensure!(
                        s(field, "type") != "integer" || value.is_i64() || value.is_u64(),
                        "answer must be an integer"
                    );
                    value
                }
                "array" => json!(answer),
                _ => json!(first),
            };
            content.insert(key.clone(), value);
        } else {
            ensure!(
                !schema["required"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(key))),
                "a required answer is missing"
            );
        }
    }
    Ok(Value::Object(content))
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    #[test]
    fn completed_turn_finalizes_only_its_synthetic_diff() {
        for (native, expected) in [
            ("completed", ToolStatus::Completed),
            ("interrupted", ToolStatus::Cancelled),
            ("failed", ToolStatus::Failed),
        ] {
            let mut adapter = Codex::new("/tmp".into(), None);
            adapter.ingest(json!({"method":"item/started","params":{"item":{"id":"async","type":"mcpToolCall","server":"fixture","tool":"background"}}})).unwrap();
            adapter.ingest(json!({"method":"turn/diff/updated","params":{"turnId":"other","diff":"other diff"}})).unwrap();
            let diff = "--- a/fixture\n+++ b/fixture\n@@ -1 +1 @@\n-old\n+new";
            adapter
                .ingest(
                    json!({"method":"turn/diff/updated","params":{"turnId":"turn","diff":diff}}),
                )
                .unwrap();
            let effects = adapter.ingest(json!({"method":"turn/completed","params":{"turn":{"id":"turn","status":native,"error":null}}})).unwrap();
            let finalized = effects
                .events
                .iter()
                .filter_map(|event| {
                    if let Event::Tool(tool) = event {
                        Some(tool)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].id, "diff-turn");
            assert_eq!(finalized[0].diff, diff);
            assert_eq!(finalized[0].status, expected);
            assert_eq!(adapter.tools["async"].status, ToolStatus::Running);
        }
    }
}
