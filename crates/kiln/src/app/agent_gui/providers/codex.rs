//! Codex app-server v2, verified against the installed generated JSON schemas.
use super::*;
#[path = "codex_actions.rs"]
mod actions;
#[path = "codex_controls.rs"]
mod detailed;
use std::time::{Duration, Instant};
struct ActiveTurnSettings {
    turn: String,
    values: BTreeMap<String, ControlValue>,
}
struct ActiveTurnChange {
    turn: String,
    control: String,
    value: ControlValue,
    values: BTreeMap<String, ControlValue>,
}
struct StopTarget {
    item: String,
    process: String,
}
struct StopOperation {
    turn: String,
    interrupt: String,
    pending: BTreeMap<String, StopTarget>,
    requested: BTreeSet<String>,
    unconfirmed: BTreeSet<String>,
    turn_done: bool,
    failed: bool,
    deadline: Instant,
}
fn native_error_message(error: &Value) -> String {
    error
        .as_str()
        .or_else(|| error.get("message").and_then(Value::as_str))
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("Codex reported an error.")
        .into()
}
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
    tool_turns: BTreeMap<String, String>,
    stopping: Option<StopOperation>,
    selected: Option<String>,
    pending_turn_settings: Option<BTreeMap<String, ControlValue>>,
    active_turn_settings: Option<ActiveTurnSettings>,
    pending_active_change: Option<ActiveTurnChange>,
    pub(super) launch_options: LaunchOptions,
    native: NativeFlight,
    controls: Vec<ControlOption>,
    metadata: Vec<ModelMetadata>,
    model_choices: Vec<Model>,
    model_pages: usize,
    collaboration: BTreeMap<String, Option<String>>,
    profiles: Vec<ControlChoice>,
    profile_pages: usize,
    allowed_sandboxes: Option<Vec<String>>,
    allowed_granular: Option<Vec<GranularApproval>>,
    policy_loaded: bool,
    effective: BTreeMap<String, ControlValue>,
    allowed_policies: Vec<String>,
    allowed_reviewers: Vec<String>,
    start_after_models: bool,
    compacting: bool,
    queue: Vec<QueuedTask>,
    skills: Vec<NativeEntry>,
    skill_enabled: BTreeMap<String, bool>,
    explicit_effort: bool,
    expected_settings: BTreeMap<String, ControlValue>,
    settings_ack: bool,
    settings_notification: bool,
    settings_deadline: Option<Instant>,
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
            tool_turns: BTreeMap::new(),
            stopping: None,
            selected: None,
            pending_turn_settings: None,
            active_turn_settings: None,
            pending_active_change: None,
            launch_options: LaunchOptions::Inherited,
            native: NativeFlight::default(),
            controls: vec![],
            metadata: vec![],
            model_choices: vec![],
            model_pages: 0,
            collaboration: BTreeMap::new(),
            profiles: vec![],
            profile_pages: 0,
            allowed_sandboxes: None,
            allowed_granular: None,
            policy_loaded: false,
            effective: BTreeMap::new(),
            allowed_policies: vec![],
            allowed_reviewers: vec![],
            start_after_models: false,
            compacting: false,
            queue: vec![],
            skills: vec![],
            skill_enabled: BTreeMap::new(),
            explicit_effort: false,
            expected_settings: BTreeMap::new(),
            settings_ack: false,
            settings_notification: false,
            settings_deadline: None,
        }
    }
    pub(super) fn resuming(&self) -> bool {
        self.resume.is_some()
    }
    pub fn start(&mut self) -> Effects {
        let mut e = Effects::default();
        e.write(rpc(json!("initialize"),"initialize",json!({"clientInfo":{"name":"kiln","title":"Kiln","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}})));
        e
    }
    pub fn tick(&mut self) -> Effects {
        self.tick_at(Instant::now())
    }
    fn tick_at(&mut self, now: Instant) -> Effects {
        let mut effects = self.native.tick_at(now);
        if let Some(stop) = &mut self.stopping
            && !stop.failed
            && now >= stop.deadline
        {
            stop.failed = true;
            effects.event(Event::Error("Codex stopping is unconfirmed; a command may still be running. Waiting for native completion; no new task was sent.".into()));
            effects.state(Lifecycle::Failed);
        }
        effects
    }
    fn stop_state(&mut self, effects: &mut Effects) {
        let Some(stop) = &self.stopping else {
            return;
        };
        if stop.turn_done && stop.pending.is_empty() && stop.unconfirmed.is_empty() {
            self.stopping = None;
            self.busy = false;
            effects.state(Lifecycle::Ready);
        } else {
            effects.state(if stop.failed {
                Lifecycle::Failed
            } else {
                Lifecycle::Running
            });
        }
    }
    fn queue_terminations(&mut self, effects: &mut Effects) {
        let Some(stop) = &mut self.stopping else {
            return;
        };
        for (id, tool) in &self.tools {
            if self.tool_turns.get(id) != Some(&stop.turn)
                || tool.status != ToolStatus::Running
                || tool.input["type"] != "commandExecution"
                || stop.requested.contains(id)
            {
                continue;
            }
            let Some(process) = tool.input["processId"]
                .as_str()
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let request = format!("stop-terminal-{}", journal::unique());
            effects.write(rpc(
                json!(request),
                "thread/backgroundTerminals/terminate",
                json!({"threadId":self.thread,"processId":process}),
            ));
            stop.requested.insert(id.clone());
            stop.pending.insert(
                request,
                StopTarget {
                    item: id.clone(),
                    process: process.into(),
                },
            );
        }
    }
    fn interrupt(&mut self, effects: &mut Effects) {
        if self.stopping.is_some() {
            return;
        }
        let interrupt = format!("stop-interrupt-{}", journal::unique());
        self.stopping = Some(StopOperation {
            turn: self.turn.clone(),
            interrupt: interrupt.clone(),
            pending: Default::default(),
            requested: Default::default(),
            unconfirmed: Default::default(),
            turn_done: false,
            failed: false,
            deadline: Instant::now() + Duration::from_secs(10),
        });
        effects.write(rpc(
            json!(interrupt),
            "turn/interrupt",
            json!({"threadId":self.thread,"turnId":self.turn}),
        ));
        self.queue_terminations(effects);
    }
    fn stop_response(&mut self, id: &str, value: &Value, effects: &mut Effects) -> bool {
        if !id.starts_with("stop-terminal-") && !id.starts_with("stop-interrupt-") {
            return false;
        }
        let Some(stop) = &mut self.stopping else {
            return true;
        };
        if id == stop.interrupt {
            if let Some(error) = value.get("error") {
                stop.failed = true;
                effects.event(Event::Error(format!(
                    "Codex interruption is unconfirmed: {}",
                    native_error_message(error)
                )));
                self.stop_state(effects);
            }
            return true;
        }
        let Some(target) = stop.pending.remove(id) else {
            return true;
        };
        let completed = self.tools.get(&target.item).is_some_and(|tool| {
            matches!(
                tool.status,
                ToolStatus::Completed | ToolStatus::Failed | ToolStatus::Cancelled
            )
        });
        if !completed {
            if let Some(tool) = self.tools.get_mut(&target.item)
                && tool.status == ToolStatus::Running
                && self.tool_turns.get(&target.item) == Some(&stop.turn)
                && tool.input["processId"] == target.process
                && value["result"]["terminated"] == true
            {
                tool.status = ToolStatus::Cancelled;
                stop.unconfirmed.remove(&target.item);
                effects.event(Event::Tool(tool.clone()));
            } else {
                stop.failed = true;
                stop.unconfirmed.insert(target.item);
                effects.event(Event::Error(format!(
                    "Codex could not confirm stopping a command; it may still be running: {}",
                    value
                        .get("error")
                        .map(native_error_message)
                        .unwrap_or_else(|| "termination was not confirmed".into())
                )));
            }
        }
        self.stop_state(effects);
        true
    }
    fn native_item(&mut self, params: &Value, complete: bool, effects: &mut Effects) {
        if complete
            && self.compacting
            && params["threadId"] == self.thread
            && params["item"]["type"] == "contextCompaction"
        {
            self.compacting = false;
            self.busy = false;
            effects.state(Lifecycle::Ready);
        }
        let id = s(&params["item"], "id");
        let same_thread = params["threadId"] == self.thread;
        let turn = s(params, "turnId");
        if !complete && same_thread && !turn.is_empty() {
            self.tool_turns.entry(id.clone()).or_insert(turn.clone());
        }
        self.item(&params["item"], complete, effects);
        if complete
            && same_thread
            && let Some(stop) = &mut self.stopping
            && turn == stop.turn
            && stop.requested.contains(&id)
        {
            stop.pending.retain(|_, target| target.item != id);
            stop.unconfirmed.remove(&id);
            self.stop_state(effects);
        } else if !complete && same_thread {
            self.queue_terminations(effects);
        }
    }
    pub fn command(&mut self, c: &Command, id: &str) -> Result<Effects> {
        let mut e = Effects::default();
        if !matches!(
            c,
            Command::Interrupt
                | Command::Respond { .. }
                | Command::Discover(_)
                | Command::NativeAction(NativeAction::Codex(
                    CodexAction::AcceptEffectiveSettings { .. }
                ))
        ) {
            self.native.idle()?;
        }
        match c {
            Command::Configure { control, value } => self.configure(control, value, &mut e)?,
            Command::Discover(target) => {
                ensure!(
                    self.native.reads.len() < 32,
                    "Too many native discovery requests"
                );
                ensure!(self.ready, "Codex is not ready");
                self.discovery(target.clone(), &mut e);
            }
            Command::NativeAction(NativeAction::Codex(action)) => self.action(action, &mut e)?,
            Command::NativeAction(action) => {
                ensure!(self.ready && !self.busy, "Codex is busy");
                match action {
                    NativeAction::ReloadSkills => self.discovery(Discovery::Skills, &mut e),
                    NativeAction::Compact => {
                        self.compacting = true;
                        self.busy = true;
                        self.turn.clear();
                        self.active_turn_settings = None;
                        self.pending_turn_settings = None;
                        e.state(Lifecycle::Running);
                        let id =
                            self.native
                                .begin("action:compact", ControlValue::Default, &mut e)?;
                        e.write(rpc(
                            json!(id),
                            "thread/compact/start",
                            json!({"threadId":self.thread}),
                        ));
                    }
                    _ => anyhow::bail!("This native action is not available for Codex"),
                }
            }
            Command::Submit {
                text,
                attachments,
                model,
            }
            | Command::SubmitWithSkills {
                text,
                attachments,
                model,
                ..
            } => {
                ensure!(
                    self.ready && !self.busy,
                    "Codex is not ready for a new turn"
                );
                if model.is_none() {
                    if let Some(meta) = self.metadata.iter().find(|m| {
                        Some(&m.id) == self.selected.as_ref()
                            || self
                                .selected
                                .as_ref()
                                .is_some_and(|selected| m.resolved_model.as_ref() == Some(selected))
                    }) {
                        if let Some(ControlValue::Text(effort)) = self.effective.get("effort") {
                            ensure!(
                                meta.efforts.iter().any(|e| e.value == *effort),
                                "The native effort is not advertised for this model; explicitly choose a supported effort before Send"
                            );
                        }
                    }
                }
                let mut input = vec![json!({"type":"text","text":text,"text_elements":[]})];
                input.extend(image_blocks(id, attachments, Tool::Codex)?);
                if let Command::SubmitWithSkills { skills, .. } = c {
                    ensure!(skills.len() <= 32, "Too many skills");
                    for skill in skills {
                        ensure!(
                            self.skills
                                .iter()
                                .any(|s| s.name == skill.name
                                    && s.path.as_deref() == Some(&skill.path))
                                && self.skill_enabled.get(&skill.path) == Some(&true),
                            "Skill is no longer available"
                        );
                        input.push(json!({"type":"skill","name":skill.name,"path":skill.path}));
                    }
                }
                let mut params = json!({"threadId":self.thread,"input":input});
                if let Some(model) = model {
                    if !self.metadata.is_empty() {
                        let patch = self.patch("model", &ControlValue::Text(model.clone()))?;
                        for key in ["collaborationMode", "model", "effort", "serviceTier"] {
                            if let Some(value) = patch.get(key) {
                                params[key] = value.clone();
                            }
                        }
                    } else {
                        params["model"] = json!(model);
                    }
                }
                self.pending_turn_settings = Some(self.turn_settings_snapshot(&params));
                e.write(rpc(json!("turn"), "turn/start", params));
                self.busy = true;
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
                        let proposed = match decision {
                            Decision::Option(key) => {
                                let index = key
                                    .strip_prefix("native-decision-")
                                    .and_then(|v| v.parse::<usize>().ok())
                                    .context("Unknown native approval choice")?;
                                p["availableDecisions"]
                                    .as_array()
                                    .and_then(|v| v.get(index))
                                    .cloned()
                                    .context("Native approval choice is no longer available")?
                            }
                            Decision::Allow => json!("accept"),
                            Decision::Cancel => json!("cancel"),
                            Decision::Deny => json!("decline"),
                            _ => anyhow::bail!("Invalid approval response"),
                        };
                        if let Some(available) = p["availableDecisions"].as_array() {
                            ensure!(
                                available.contains(&proposed),
                                "The native server did not offer this decision"
                            );
                        }
                        json!({"decision":proposed})
                    }
                    "execCommandApproval" | "applyPatchApproval" => {
                        json!({"decision":if allow{"approved"}else if cancelled{"abort"}else{"denied"}})
                    }
                    "item/permissions/requestApproval" => {
                        detailed::permission_grant(&p["permissions"], decision)?
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
                self.native.cancel(&mut e);
                // Server clears live requests with serverRequest/resolved; reject late UI replies now.
                for (key, _) in std::mem::take(&mut self.pending) {
                    e.event(Event::RequestResolved(key));
                }
                if self.busy && self.stopping.is_none() {
                    if self.turn.is_empty() {
                        self.interrupt_pending = true;
                    } else {
                        self.interrupt(&mut e);
                    }
                }
            }
            Command::SetMode { mode } => self.configure(
                "collaborationMode",
                &ControlValue::Text(mode.clone()),
                &mut e,
            )?,
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
            tool.output = native_error_message(error);
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
            if p.get("threadId")
                .and_then(Value::as_str)
                .is_some_and(|thread| !self.thread.is_empty() && thread != self.thread)
            {
                e.write(unknown(v["id"].clone()));
                return Ok(e);
            }

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
                        options: p["availableDecisions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(i, d)| Choice {
                                id: format!("native-decision-{i}"),
                                label: match d.as_str() {
                                    Some("accept") => "Allow once",
                                    Some("acceptForSession") => "Allow for session",
                                    Some("decline") => "Decline",
                                    Some("cancel") => "Cancel turn",
                                    _ if d.get("acceptWithExecpolicyAmendment").is_some() => {
                                        "Allow and save command policy"
                                    }
                                    _ if d.get("applyNetworkPolicyAmendment").is_some() => {
                                        "Apply network policy"
                                    }
                                    _ => "Native decision",
                                }
                                .into(),
                                description: String::new(),
                            })
                            .collect(),
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
            if id == "models" && self.start_after_models && v.get("error").is_some() {
                anyhow::bail!(
                    "The native model catalog is required to validate explicit new-conversation options"
                );
            }
            if self.detailed_response(&id, &v, &mut e) {
                return Ok(e);
            }
            if self.stop_response(&id, &v, &mut e) {
                return Ok(e);
            }
            if let Some(error) = v.get("error") {
                anyhow::bail!("Codex request failed: {}", native_error_message(error));
            }
            let r = &v["result"];
            match id.as_str() {
                "initialize" => {
                    e.write(json!({"jsonrpc":"2.0","method":"initialized"}));
                    e.write(rpc(
                        json!("models"),
                        "model/list",
                        json!({"limit":100,"includeHidden":true}),
                    ));
                    let (method, params) = if let Some(id) = &self.resume {
                        (
                            "thread/resume",
                            json!({"threadId":id,"cwd":self.cwd,"excludeTurns":true}),
                        )
                    } else {
                        ("thread/start", {
                            let mut p = self.launch_options.codex_params();
                            p["cwd"] = json!(self.cwd);
                            p
                        })
                    };
                    if self.resume.is_none()
                        && matches!(&self.launch_options,LaunchOptions::Codex(v) if v.model.is_some())
                    {
                        self.start_after_models = true;
                    } else {
                        e.write(rpc(json!("thread"), method, params));
                    }
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
                    self.capabilities(&mut e);
                    self.settings(r, &mut e);
                    e.write(rpc(
                        json!("collaboration"),
                        "collaborationMode/list",
                        json!({}),
                    ));

                    self.discovery(Discovery::Policy, &mut e);
                    self.discovery(Discovery::Skills, &mut e);
                    e.state(Lifecycle::Ready);
                }
                "models" => self.models(r, &mut e)?,
                "turn" => {
                    e.event(Event::Models {
                        choices: vec![],
                        selected: self.selected.clone(),
                    });
                    self.adopt_turn(s(&r["turn"], "id"));
                    self.publish_active_turn(&mut e);
                    e.accepted = true;
                    if std::mem::take(&mut self.interrupt_pending) {
                        self.interrupt(&mut e);
                    }
                }
                _ => {}
            }
            return Ok(e);
        }
        if self.notification(&method, p, &mut e) {
            return Ok(e);
        }
        match method.as_str() {
            "thread/started" => {
                if self.thread.is_empty() {
                    self.thread = s(&p["thread"], "id");
                }
            }
            "turn/started" => {
                if p.get("threadId").is_some() && p["threadId"] != self.thread {
                    return Ok(e);
                }
                self.adopt_turn(s(&p["turn"], "id"));
                self.publish_active_turn(&mut e);
                e.accepted = true;
                if self.stopping.is_some() {
                    self.stop_state(&mut e);
                } else {
                    e.state(Lifecycle::Running);
                }
            }
            "item/agentMessage/delta" => e.event(message(s(p, "itemId"), s(p, "delta"), true)),
            "item/started" => self.native_item(p, false, &mut e),
            "item/completed" => self.native_item(p, true, &mut e),
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
                if p.get("threadId").is_some() && p["threadId"] != self.thread {
                    return Ok(e);
                }
                let turn = s(&p["turn"], "id");
                if !self.turn.is_empty() && turn != self.turn {
                    return Ok(e);
                }
                if let Some(mut diff) = self.diffs.remove(&s(&p["turn"], "id")) {
                    diff.status = match s(&p["turn"], "status").as_str() {
                        "interrupted" => ToolStatus::Cancelled,
                        "failed" => ToolStatus::Failed,
                        "completed" => ToolStatus::Completed,
                        _ => ToolStatus::Failed,
                    };
                    e.event(Event::Tool(diff));
                }
                self.interrupt_pending = false;
                self.turn.clear();
                self.active_turn_settings = None;
                self.publish_active_turn(&mut e);
                self.pending_turn_settings = None;
                self.pending.clear();
                if let Some(error) = p["turn"].get("error").filter(|v| !v.is_null()) {
                    e.event(Event::Error(native_error_message(error)));
                }
                if let Some(stop) = &mut self.stopping
                    && stop.turn == turn
                {
                    stop.turn_done = true;
                    self.stop_state(&mut e);
                } else {
                    self.busy = false;
                    e.state(Lifecycle::Ready);
                }
            }
            "error" => {
                e.event(Event::Error(native_error_message(&p["error"])));
                if p["willRetry"].as_bool() != Some(true) {
                    if let Some(stop) = &mut self.stopping {
                        stop.failed = true;
                        self.stop_state(&mut e);
                    } else {
                        self.busy = false;
                        e.state(Lifecycle::Ready);
                    }
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

#[cfg(test)]
mod stop_tests {
    use super::*;
    fn running() -> Codex {
        let mut adapter = Codex::new("/tmp".into(), None);
        adapter
            .ingest(json!({"id":"thread","result":{"thread":{"id":"thread"}}}))
            .unwrap();
        adapter
            .command(
                &Command::Submit {
                    text: "task".into(),
                    attachments: vec![],
                    model: None,
                },
                "fixture",
            )
            .unwrap();
        adapter
            .ingest(json!({"id":"turn","result":{"turn":{"id":"current"}}}))
            .unwrap();
        adapter
    }
    fn item(adapter: &mut Codex, id: &str, thread: &str, turn: &str, kind: &str, process: &str) {
        adapter.ingest(json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,"item":{"id":id,"type":kind,"command":"fixture command","processId":process,"status":"inProgress"}}})).unwrap();
    }
    fn termination(effects: &Effects) -> Value {
        let requests = effects
            .writes
            .iter()
            .filter(|v| v["method"] == "thread/backgroundTerminals/terminate")
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        requests[0].clone()
    }
    #[test]
    fn stop_terminates_only_current_native_command_and_waits_for_matching_receipt() {
        let mut adapter = running();
        item(
            &mut adapter,
            "prior",
            "thread",
            "prior",
            "commandExecution",
            "10",
        );
        item(
            &mut adapter,
            "active",
            "thread",
            "current",
            "commandExecution",
            "20",
        );
        item(
            &mut adapter,
            "foreign",
            "other-thread",
            "current",
            "commandExecution",
            "30",
        );
        item(
            &mut adapter,
            "mcp",
            "thread",
            "current",
            "mcpToolCall",
            "40",
        );
        let stop = adapter.command(&Command::Interrupt, "fixture").unwrap();
        let request = termination(&stop);
        assert_eq!(
            request["params"],
            json!({"threadId":"thread","processId":"20"})
        );
        assert!(
            !stop
                .events
                .iter()
                .any(|e| matches!(e,Event::Tool(t) if t.status==ToolStatus::Cancelled))
        );
        let turn = adapter.ingest(json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"current","status":"interrupted"}}})).unwrap();
        assert!(!turn.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
        adapter.command(&Command::Interrupt, "fixture").unwrap();
        assert!(
            adapter
                .command(
                    &Command::Submit {
                        text: "too soon".into(),
                        attachments: vec![],
                        model: None
                    },
                    "fixture"
                )
                .is_err()
        );
        let settled = adapter
            .ingest(json!({"id":request["id"],"result":{"terminated":true}}))
            .unwrap();
        assert!(settled.events.iter().any(
            |e| matches!(e,Event::Tool(t) if t.id=="active"&&t.status==ToolStatus::Cancelled)
        ));
        assert!(settled.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
        for id in ["prior", "foreign", "mcp"] {
            assert_eq!(adapter.tools[id].status, ToolStatus::Running);
        }
        adapter
            .command(
                &Command::Submit {
                    text: "next turn".into(),
                    attachments: vec![],
                    model: None,
                },
                "fixture",
            )
            .unwrap();
        let next = adapter
            .ingest(json!({"id":"turn","result":{"turn":{"id":"next"}}}))
            .unwrap();
        assert!(!next.writes.iter().any(|v| v["method"] == "turn/interrupt"));
        let late = adapter
            .ingest(json!({"id":request["id"],"error":{"code":-32601,"message":"late"}}))
            .unwrap();
        assert!(late.events.is_empty());
        assert!(adapter.busy);
        assert_eq!(adapter.turn, "next");
    }
    #[test]
    fn unsupported_or_unconfirmed_termination_keeps_native_status_and_blocks_new_turns() {
        for response in [
            json!({"result":{"terminated":false}}),
            json!({"error":{"code":-32601,"message":"unsupported"}}),
        ] {
            let mut adapter = running();
            item(
                &mut adapter,
                "active",
                "thread",
                "current",
                "commandExecution",
                "20",
            );
            let stop = adapter.command(&Command::Interrupt, "fixture").unwrap();
            let request = termination(&stop);
            adapter.ingest(json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"current","status":"interrupted"}}})).unwrap();
            let mut response = response;
            response["id"] = request["id"].clone();
            let failed = adapter.ingest(response).unwrap();
            assert!(failed.events.iter().any(|e| matches!(e, Event::Error(_))));
            assert!(failed.events.contains(&Event::Lifecycle(Lifecycle::Failed)));
            let error = adapter.ingest(json!({"method":"error","params":{"error":{"message":"turn failed"},"willRetry":false}})).unwrap();
            assert!(!error.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
            assert_eq!(adapter.tools["active"].status, ToolStatus::Running);
            assert!(
                adapter
                    .command(
                        &Command::Submit {
                            text: "blocked".into(),
                            attachments: vec![],
                            model: None
                        },
                        "fixture"
                    )
                    .is_err()
            );
            let done = adapter.ingest(json!({"method":"item/completed","params":{"threadId":"thread","turnId":"current","item":{"id":"active","type":"commandExecution","processId":"20","status":"completed"}}})).unwrap();
            assert!(done.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
        }
    }
    #[test]
    fn missing_termination_receipt_times_out_without_completing_or_replaying_a_command() {
        let mut adapter = running();
        item(
            &mut adapter,
            "active",
            "thread",
            "current",
            "commandExecution",
            "20",
        );
        let stop = adapter.command(&Command::Interrupt, "fixture").unwrap();
        let request = termination(&stop);
        adapter.ingest(json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"current","status":"interrupted"}}})).unwrap();
        assert!(
            adapter
                .tick_at(Instant::now() + Duration::from_secs(1))
                .events
                .is_empty()
        );
        let timed_out = adapter.tick_at(Instant::now() + Duration::from_secs(11));
        assert!(
            timed_out
                .events
                .contains(&Event::Lifecycle(Lifecycle::Failed))
        );
        assert!(
            timed_out
                .events
                .iter()
                .any(|e| matches!(e, Event::Error(_)))
        );
        assert!(timed_out.writes.is_empty());
        assert_eq!(adapter.tools["active"].status, ToolStatus::Running);
        assert!(
            adapter
                .tick_at(Instant::now() + Duration::from_secs(12))
                .events
                .is_empty()
        );
        let late = adapter
            .ingest(json!({"id":request["id"],"result":{"terminated":true}}))
            .unwrap();
        assert!(late.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
        assert!(late.writes.is_empty());
    }
    #[test]
    fn native_command_started_during_stop_is_terminated_before_turn_readiness() {
        let mut adapter = running();
        let stop = adapter.command(&Command::Interrupt, "fixture").unwrap();
        assert_eq!(stop.writes.len(), 1);
        let started = adapter.ingest(json!({"method":"item/started","params":{"threadId":"thread","turnId":"current","item":{"id":"late-start","type":"commandExecution","processId":"50","status":"inProgress"}}})).unwrap();
        let request = termination(&started);
        assert_eq!(
            request["params"],
            json!({"threadId":"thread","processId":"50"})
        );
        let receipt = adapter
            .ingest(json!({"id":request["id"],"result":{"terminated":true}}))
            .unwrap();
        assert!(!receipt.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
        let done = adapter.ingest(json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"current","status":"interrupted"}}})).unwrap();
        assert!(done.events.contains(&Event::Lifecycle(Lifecycle::Ready)));
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    #[test]
    fn native_errors_project_readable_messages_without_protocol_objects() {
        let error = json!({"message":"Sign in again to continue.","codexErrorInfo":{"type":"authentication"},"additionalDetails":{"internal":"fixture"},"misalignment":null});
        for message in [
            json!({"method":"turn/completed","params":{"turn":{"id":"turn","status":"failed","error":error}}}),
            json!({"method":"error","params":{"error":error,"willRetry":false}}),
        ] {
            let mut adapter = Codex::new("/tmp".into(), None);
            let effects = adapter.ingest(message).unwrap();
            let errors = effects
                .events
                .iter()
                .filter_map(|event| {
                    if let Event::Error(message) = event {
                        Some(message.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(errors, vec!["Sign in again to continue."]);
        }
        let mut adapter = Codex::new("/tmp".into(), None);
        let rpc = adapter
            .ingest(json!({"id":"transport-only-id","error":error}))
            .err()
            .unwrap()
            .to_string();
        assert!(rpc.contains("Sign in again to continue."));
        for hidden in [
            "transport-only-id",
            "codexErrorInfo",
            "additionalDetails",
            "misalignment",
        ] {
            assert!(!rpc.contains(hidden));
        }
        let unknown = adapter.ingest(json!({"method":"error","params":{"error":{"internal_only":"fixture"},"willRetry":true}})).unwrap();
        assert!(unknown.events.iter().any(|event|matches!(event,Event::Error(message) if !message.contains("internal_only")&&!message.contains('{'))));
    }
}
