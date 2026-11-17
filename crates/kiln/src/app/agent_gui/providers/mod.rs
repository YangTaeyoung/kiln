//! Each adapter owns native IDs and live requests; unknown requests fail closed.
mod native;
use native::*;
mod antigravity;
mod claude;
mod codex;
use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub(super) struct Effects {
    pub writes: Vec<Value>,
    pub events: Vec<Event>,
    pub accepted: bool,
    pub cancelled: bool,
}
impl Effects {
    pub fn merge(&mut self, other: Self) {
        self.writes.extend(other.writes);
        for event in other.events {
            self.event(event);
        }
        self.accepted |= other.accepted;
        self.cancelled |= other.cancelled;
    }
    fn write(&mut self, v: Value) {
        self.writes.push(v);
    }
    fn event(&mut self, e: Event) {
        if let Event::Message {
            message,
            append: true,
        } = &e
            && let Some(Event::Message {
                message: previous,
                append: true,
            }) = self.events.last_mut()
            && previous.id == message.id
            && previous.role == message.role
        {
            previous.text.push_str(&message.text);
            return;
        }
        self.events.push(e);
    }
    fn state(&mut self, s: Lifecycle) {
        self.event(Event::Lifecycle(s));
    }
}
// A sent configuration operation may still take effect after Stop. Its reply is a
// settlement barrier, never proof of rollback. New controls wait behind that barrier.
struct ConfigOperation {
    id: String,
    model: Option<String>,
    mode: Option<String>,
    prompt: Option<Value>,
    wire: Value,
    sent: bool,
    started: std::time::Instant,
    timeout_reported: bool,
}
impl ConfigOperation {
    fn matches(&self, id: &str) -> bool {
        self.sent && self.id == id
    }
}
fn tick_config(pending: &mut Option<ConfigOperation>, effects: &mut Effects) {
    if let Some(op) = pending {
        if !op.timeout_reported && op.started.elapsed() >= std::time::Duration::from_secs(15) {
            op.timeout_reported = true;
            op.prompt = None;
            effects.cancelled = true;
            effects.event(Event::Controls(ControlEvent::Outcome(ControlOutcome{settled:false,operation:op.id.clone(),control:if op.mode.is_some(){"mode"}else{"model"}.into(),requested:op.model.as_ref().or(op.mode.as_ref()).cloned().map(ControlValue::Text).unwrap_or(ControlValue::Default),effective:None,state:ApplicationState::Unconfirmed,message:Some("Native configuration timed out. The deferred task was withdrawn; waiting for settlement before another Send.".into())})));
            effects.state(Lifecycle::Ready);
        }
    }
}
fn cancel_config(
    pending: &mut Option<ConfigOperation>,
    retired: &mut BTreeSet<String>,
    effects: &mut Effects,
) -> bool {
    if let Some(operation) = pending.take() {
        if operation.sent {
            retired.insert(operation.id);
        } else {
            effects.event(Event::Controls(ControlEvent::Settlement(operation.id)));
        }
        true
    } else {
        false
    }
}
fn settle_retired_config(
    id: &str,
    pending: &mut Option<ConfigOperation>,
    retired: &mut BTreeSet<String>,
    effects: &mut Effects,
) -> bool {
    if !retired.remove(id) {
        return false;
    }
    effects.event(Event::Controls(ControlEvent::Settlement(id.into())));
    if retired.is_empty()
        && let Some(operation) = pending
        && !operation.sent
    {
        operation.sent = true;
        operation.started = std::time::Instant::now();
        effects.write(operation.wire.clone());
    }
    true
}
pub(super) enum Adapter {
    Claude(claude::Claude),
    Codex(codex::Codex),
    Antigravity(antigravity::Antigravity),
}
impl Adapter {
    pub fn tick(&mut self) -> Effects {
        match self {
            Self::Codex(adapter) => adapter.tick(),
            Self::Claude(adapter) => adapter.tick(),
            Self::Antigravity(adapter) => adapter.tick(),
        }
    }
    pub fn new(tool: Tool, cwd: PathBuf, resume: Option<String>) -> Self {
        match tool {
            Tool::Claude => Self::Claude(claude::Claude::new(resume)),
            Tool::Codex => Self::Codex(codex::Codex::new(cwd, resume)),
            Tool::Antigravity => Self::Antigravity(antigravity::Antigravity::new(cwd, resume)),
        }
    }
    pub(super) fn fresh_claude_session(&mut self, id: String) {
        if let Self::Claude(claude) = self { claude.fresh_session(id); }
    }
    pub fn launch_options(&mut self, options: LaunchOptions) {
        if let Self::Codex(a) = self {
            a.launch_options = options;
        }
    }
    pub fn is_resume(&self) -> bool {
        match self {
            Self::Claude(a) => a.resuming(),
            Self::Codex(a) => a.resuming(),
            Self::Antigravity(a) => a.resuming(),
        }
    }
    pub fn arguments(&self) -> Vec<String> {
        match self {
            Self::Claude(a) => a.arguments(),
            Self::Codex(_) => vec!["--no-daemon".into(), "app-server".into()],
            Self::Antigravity(_) => vec!["acp".into()],
        }
    }
    pub fn start(&mut self) -> Effects {
        match self {
            Self::Claude(a) => a.start(),
            Self::Codex(a) => a.start(),
            Self::Antigravity(a) => a.start(),
        }
    }
    pub fn ingest(&mut self, v: Value) -> Result<Effects> {
        match self {
            Self::Claude(a) => a.ingest(v),
            Self::Codex(a) => a.ingest(v),
            Self::Antigravity(a) => a.ingest(v),
        }
    }
    pub fn command(&mut self, command: &Command, id: &str) -> Result<Effects> {
        match self {
            Self::Claude(a) => a.command(command, id),
            Self::Codex(a) => a.command(command, id),
            Self::Antigravity(a) => a.command(command, id),
        }
    }
}
fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into()
}
fn text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        s.into()
    } else {
        serde_json::to_string_pretty(v).unwrap_or_default()
    }
}
fn request_key(v: &Value) -> String {
    v.to_string()
}
fn rpc(id: Value, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
fn result(id: Value, value: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":value})
}
fn unknown(id: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Client method is not implemented"}})
}
fn model_choices(value: &Value, id_key: &str, label_key: &str) -> Vec<Model> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let id = s(v, id_key);
            (!id.is_empty()).then(|| Model {
                id: id.clone(),
                label: v
                    .get(label_key)
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .into(),
            })
        })
        .collect()
}
fn questions(value: &Value) -> Vec<Question> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|q| Question {
            id: q
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| q.get("question").and_then(Value::as_str))
                .unwrap_or_default()
                .into(),
            text: q
                .get("question")
                .or_else(|| q.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            multi_select: q
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            options: q
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|o| {
                    let label = s(o, "label");
                    Choice {
                        id: label.clone(),
                        label,
                        description: s(o, "description"),
                    }
                })
                .collect(),
        })
        .collect()
}
fn message(id: String, text: String, append: bool) -> Event {
    Event::Message {
        message: Message {
            id,
            role: Role::Assistant,
            text,
            attachments: vec![],
        },
        append,
    }
}
fn image_blocks(id: &str, attachments: &[Attachment], format: Tool) -> Result<Vec<Value>> {
    use base64::Engine;
    ensure!(attachments.len() <= 8, "too many image attachments");
    attachments.iter().map(|a|{
        let bytes=attachment_bytes(id,a)?;let data=base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(match format{Tool::Claude=>json!({"type":"image","source":{"type":"base64","media_type":a.mime,"data":data}}),Tool::Codex=>json!({"type":"localImage","path":a.path}),Tool::Antigravity=>json!({"type":"image","mimeType":a.mime,"data":data})})
    }).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_requires_live_permission_and_preserves_tool_identity() {
        let mut a = Adapter::new(Tool::Claude, "/tmp".into(), None);
        let init = a.start();
        assert_eq!(init.writes[0]["request"]["subtype"], "initialize");
        a.ingest(json!({"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"models":[{"value":"sonnet","displayName":"Sonnet"}]}}})).unwrap();
        let request=a.ingest(json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Write","tool_use_id":"tool1","input":{"file_path":"a","content":"ok"}}})).unwrap();
        assert!(
            request
                .events
                .iter()
                .any(|e| matches!(e, Event::Request(_)))
        );
        assert!(request.writes.is_empty());
        let answer = Command::Respond {
            request_id: "r1".into(),
            decision: Decision::Allow,
            answers: BTreeMap::new(),
        };
        let response = a.command(&answer, "fixture").unwrap();
        assert_eq!(
            response.writes[0]["response"]["response"]["updatedInput"]["content"],
            "ok"
        );
        assert!(a.command(&answer, "fixture").is_err());
        let out=a.ingest(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool1","content":"written"}]}})).unwrap();
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e,Event::Tool(t)if t.id=="tool1"&&t.output=="written"))
        );
    }
    #[test]
    fn codex_preserves_numeric_request_ids_and_typed_permissions() {
        let mut a = Adapter::new(Tool::Codex, "/tmp".into(), None);
        let out=a.ingest(json!({"id":7,"method":"item/permissions/requestApproval","params":{"permissions":{"network":{"enabled":true}}}})).unwrap();
        let id = out
            .events
            .iter()
            .find_map(|e| {
                if let Event::Request(r) = e {
                    Some(r.id.clone())
                } else {
                    None
                }
            })
            .unwrap();
        let out = a
            .command(
                &Command::Respond {
                    request_id: id,
                    decision: Decision::Grant(PermissionGrant {
                        entry_indices: vec![],
                        read_indices: vec![],
                        write_indices: vec![],
                        network: true,
                        scope: PermissionScope::Turn,
                        strict_review: false,
                    }),
                    answers: BTreeMap::new(),
                },
                "fixture",
            )
            .unwrap();
        assert_eq!(out.writes[0]["id"], 7);
        assert_eq!(
            out.writes[0]["result"]["permissions"]["network"]["enabled"],
            true
        );
        let unknown = a
            .ingest(json!({"id":"future","method":"future/approval","params":{}}))
            .unwrap();
        assert_eq!(unknown.writes[0]["error"]["code"], -32601);
    }
    #[test]
    fn acp_rejects_unsupported_resume_and_cancels_with_native_session_id() {
        let mut a = Adapter::new(Tool::Antigravity, "/tmp".into(), Some("existing".into()));
        a.start();
        assert!(a.ingest(json!({"jsonrpc":"2.0","id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":false}}})).is_err());
        let mut a = Adapter::new(Tool::Antigravity, "/tmp".into(), None);
        a.start();
        a.ingest(json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{}}}))
            .unwrap();
        a.ingest(json!({"id":"session","result":{"sessionId":"s1"}}))
            .unwrap();
        let out = a.command(&Command::Interrupt, "fixture").unwrap();
        assert_eq!(out.writes.last().unwrap()["params"]["sessionId"], "s1");
        assert_eq!(out.writes.last().unwrap()["method"], "session/cancel");
    }
}
#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn codex_interrupt_before_turn_ack_is_sent_after_ack() {
        let mut adapter = Adapter::new(Tool::Codex, "/tmp".into(), None);
        adapter
            .ingest(json!({"id":"thread","result":{"thread":{"id":"native"}}}))
            .unwrap();
        adapter
            .command(
                &Command::Submit {
                    text: "work".into(),
                    attachments: vec![],
                    model: None,
                },
                "fixture",
            )
            .unwrap();
        let early = adapter
            .command(&Command::Interrupt, "fixture")
            .expect("interrupt must queue while waiting for turn ID");
        assert!(early.writes.is_empty());
        let ack = adapter
            .ingest(json!({"id":"turn","result":{"turn":{"id":"turn1"}}}))
            .unwrap();
        assert!(
            ack.writes
                .iter()
                .any(|v| v["method"] == "turn/interrupt" && v["params"]["turnId"] == "turn1")
        );
    }
    #[test]
    fn unknown_notifications_do_not_complete_running_turns() {
        let mut a = Adapter::new(Tool::Claude, "/tmp".into(), None);
        let e = a
            .ingest(json!({"type":"future_result","result":"done"}))
            .unwrap();
        assert!(e.events.is_empty());
    }
}
#[cfg(test)]
mod cancellation_regressions {
    use super::*;
    #[test]
    fn stopping_during_model_selection_never_sends_the_deferred_prompt() {
        let mut claude = Adapter::new(Tool::Claude, "/tmp".into(), None);
        claude.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{}}})).unwrap();
        claude
            .command(
                &Command::Submit {
                    text: "must not run".into(),
                    attachments: vec![],
                    model: Some("model".into()),
                },
                "fixture",
            )
            .unwrap();
        claude.command(&Command::Interrupt, "fixture").unwrap();
        let response=claude.ingest(json!({"type":"control_response","response":{"request_id":"set-model","subtype":"success","response":{}}})).unwrap();
        assert!(!response.writes.iter().any(|v| v["type"] == "user"));
        let mut acp = Adapter::new(Tool::Antigravity, "/tmp".into(), None);
        acp.ingest(
            json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{}}}),
        )
        .unwrap();
        acp.ingest(json!({"id":"session","result":{"sessionId":"native","models":{"currentModelId":"old","availableModels":[{"modelId":"new","name":"New"}]}}})).unwrap();
        acp.command(
            &Command::Submit {
                text: "must not run".into(),
                attachments: vec![],
                model: Some("new".into()),
            },
            "fixture",
        )
        .unwrap();
        acp.command(&Command::Interrupt, "fixture").unwrap();
        let response = acp.ingest(json!({"id":"model","result":{}})).unwrap();
        assert!(
            !response
                .writes
                .iter()
                .any(|v| v["method"] == "session/prompt")
        );
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    #[test]
    fn bounded_read_batch_coalesces_contiguous_text_without_dropping_tool_boundaries() {
        let mut combined = Effects::default();
        for part in ["안녕", "하세요"] {
            let mut e = Effects::default();
            e.event(message("message".into(), part.into(), true));
            combined.merge(e);
        }
        assert_eq!(combined.events.len(), 1);
        assert!(
            matches!(&combined.events[0],Event::Message{message,..}if message.text=="안녕하세요")
        );
        let mut boundary = Effects::default();
        boundary.event(Event::Tool(ToolCall {
            id: "tool".into(),
            ..Default::default()
        }));
        combined.merge(boundary);
        let mut after = Effects::default();
        after.event(message("message".into(), "after".into(), true));
        combined.merge(after);
        assert_eq!(combined.events.len(), 3);
    }
}
#[cfg(test)]
mod input_bounds_tests {
    use super::*;
    #[test]
    fn streamed_tool_json_is_bounded_across_multiple_frames() {
        let mut a = Adapter::new(Tool::Claude, "/tmp".into(), None);
        a.ingest(json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool","name":"Write","input":{}}}})).unwrap();
        let frame = json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"x".repeat(1024*1024)}}});
        a.ingest(frame.clone()).unwrap();
        a.ingest(frame.clone()).unwrap();
        assert!(a.ingest(frame).is_err());
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    fn initialized(tool: Tool) -> Adapter {
        let mut a = Adapter::new(tool, "/tmp".into(), None);
        if tool == Tool::Claude {
            a.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{}}})).unwrap();
            a.ingest(json!({"type":"system","model":"old"})).unwrap();
        } else {
            a.ingest(
                json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{}}}),
            )
            .unwrap();
            a.ingest(json!({"id":"session","result":{"sessionId":"native","models":{"currentModelId":"old","availableModels":[{"modelId":"old","name":"Old"},{"modelId":"A","name":"A"},{"modelId":"B","name":"B"}]}}})).unwrap();
        }
        a
    }
    fn submit(a: &mut Adapter, model: &str) -> Effects {
        a.command(
            &Command::Submit {
                text: format!("prompt {model}"),
                attachments: vec![],
                model: Some(model.into()),
            },
            "fixture",
        )
        .unwrap()
    }
    fn control_id(tool: Tool, e: &Effects) -> Value {
        if tool == Tool::Claude {
            e.writes[0]["request_id"].clone()
        } else {
            e.writes[0]["id"].clone()
        }
    }
    fn ack(tool: Tool, id: Value, model: &str) -> Value {
        if tool == Tool::Claude {
            json!({"type":"control_response","response":{"request_id":id,"subtype":"success","response":{}}})
        } else {
            json!({"id":id,"result":{"configOptions":[{"id":"model","category":"model","type":"select","currentValue":model,"options":[{"value":"old","name":"Old"},{"value":"A","name":"A"},{"value":"B","name":"B"}]}]}})
        }
    }
    fn prompt(tool: Tool, v: &Value) -> bool {
        if tool == Tool::Claude {
            v["type"] == "user"
        } else {
            v["method"] == "session/prompt"
        }
    }
    #[test]
    fn cancelled_model_ack_cannot_dispatch_replacement_and_only_matching_success_confirms_model() {
        for tool in [Tool::Claude, Tool::Antigravity] {
            for b_model in ["A", "B"] {
                let mut a = initialized(tool);
                let a_id = control_id(tool, &submit(&mut a, "A"));
                a.command(&Command::Interrupt, "fixture").unwrap();
                let b = submit(&mut a, b_model);
                assert!(!b.writes.iter().any(|v| prompt(tool, v)));
                assert!(
                    b.writes.is_empty(),
                    "replacement control must wait for the outstanding operation"
                );
                let old = a.ingest(ack(tool, a_id.clone(), "A")).unwrap();
                assert!(!old.writes.iter().any(|v| prompt(tool, v)));
                let b_id = control_id(tool, &old);
                assert_ne!(a_id, b_id);
                assert!(!old.events.iter().any(|e| matches!(e, Event::Models { .. })));
                let success = a.ingest(ack(tool, b_id, b_model)).unwrap();
                assert_eq!(success.writes.iter().filter(|v| prompt(tool, v)).count(), 1);
                assert!(
                    success.events.iter().any(
                        |e| matches!(e,Event::Models{selected:Some(model),..}if model==b_model)
                    )
                );
                let duplicate = a.ingest(ack(tool, a_id, "A")).unwrap();
                assert!(duplicate.writes.is_empty());
            }
        }
    }
    #[test]
    fn retired_model_errors_are_ignored_and_live_model_errors_never_dispatch() {
        for tool in [Tool::Claude, Tool::Antigravity] {
            let mut a = initialized(tool);
            let old = control_id(tool, &submit(&mut a, "A"));
            a.command(&Command::Interrupt, "fixture").unwrap();
            let queued = submit(&mut a, "B");
            assert!(queued.writes.is_empty());
            let error = |id: Value| {
                if tool == Tool::Claude {
                    json!({"type":"control_response","response":{"request_id":id,"subtype":"error","error":"failed"}})
                } else {
                    json!({"id":id,"error":{"code":-1,"message":"failed"}})
                }
            };
            let settled = a.ingest(error(old)).unwrap();
            assert!(!settled.writes.iter().any(|v| prompt(tool, v)));
            let current = control_id(tool, &settled);
            let failure = a.ingest(error(current)).unwrap();
            assert!(failure.cancelled);
            assert!(!failure.writes.iter().any(|v| prompt(tool, v)));
        }
    }
    #[test]
    fn codex_turn_ack_does_not_fabricate_effective_model_readback() {
        let mut a = Adapter::new(Tool::Codex, "/tmp".into(), None);
        a.ingest(json!({"id":"thread","result":{"thread":{"id":"native"},"model":"old"}}))
            .unwrap();
        let queued = submit(&mut a, "new");
        assert!(
            !queued
                .events
                .iter()
                .any(|e| matches!(e, Event::Models { .. }))
        );
        let accepted = a
            .ingest(json!({"id":"turn","result":{"turn":{"id":"turn1"}}}))
            .unwrap();
        assert!(
            accepted
                .events
                .iter()
                .any(|e| matches!(e,Event::Models{selected:Some(model),..}if model=="old"))
        );
    }
    #[test]
    fn numeric_elicitation_rejects_wrong_json_types_without_consuming_the_request() {
        for (kind, invalid, valid) in [
            ("number", vec!["true", "{}", "[]", "\"4\""], "1.5"),
            ("integer", vec!["true", "{}", "1.5", "\"4\""], "4"),
        ] {
            let mut a = Adapter::new(Tool::Codex, "/tmp".into(), None);
            let e=a.ingest(json!({"id":7,"method":"mcpServer/elicitation/request","params":{"mode":"form","requestedSchema":{"type":"object","properties":{"value":{"type":kind}},"required":["value"]}}})).unwrap();
            let request_id = e
                .events
                .iter()
                .find_map(|e| {
                    if let Event::Request(r) = e {
                        Some(r.id.clone())
                    } else {
                        None
                    }
                })
                .unwrap();
            let answer = |text: &str| Command::Respond {
                request_id: request_id.clone(),
                decision: Decision::Allow,
                answers: BTreeMap::from([("value".into(), vec![text.into()])]),
            };
            for invalid in invalid {
                assert!(
                    a.command(&answer(invalid), "fixture").is_err(),
                    "{kind} accepted {invalid}"
                );
            }
            let accepted = a.command(&answer(valid), "fixture").unwrap();
            assert!(accepted.writes[0]["result"]["content"]["value"].is_number());
        }
    }
    fn failed_config(tool: Tool, id: Value, cancelled: bool) -> Value {
        if tool == Tool::Claude {
            json!({"type":"control_response","response":{"request_id":id,"subtype":"error","error":if cancelled{"cancelled"}else{"failed"}}})
        } else {
            json!({"id":id,"error":{"code":if cancelled{-32800}else{-1},"message":"configuration did not complete"}})
        }
    }
    #[test]
    fn original_model_and_default_reconfirm_after_cancelled_native_change_in_both_response_orders()
    {
        for tool in [Tool::Claude, Tool::Antigravity] {
            for before_b in [false, true] {
                for use_default in [false, true] {
                    for outcome in [0, 1, 2] {
                        let mut a = initialized(tool);
                        let old_id = control_id(tool, &submit(&mut a, "A"));
                        a.command(&Command::Interrupt, "fixture").unwrap();
                        let old_response = if outcome == 0 {
                            ack(tool, old_id.clone(), "A")
                        } else {
                            failed_config(tool, old_id.clone(), outcome == 2)
                        };
                        if before_b {
                            let old = a.ingest(old_response.clone()).unwrap();
                            assert!(!old.writes.iter().any(|v| prompt(tool, v)));
                        }
                        let mut b = a
                            .command(
                                &Command::Submit {
                                    text: "must use original O".into(),
                                    attachments: vec![],
                                    model: if use_default {
                                        None
                                    } else {
                                        Some("old".into())
                                    },
                                },
                                "fixture",
                            )
                            .unwrap();
                        assert!(
                            !b.writes.iter().any(|v| prompt(tool, v)),
                            "{tool:?}: formerly confirmed O cannot justify dispatch"
                        );
                        if !before_b {
                            assert!(
                                b.writes.is_empty(),
                                "configuration must wait for the outstanding native change"
                            );
                            b = a.ingest(old_response).unwrap();
                        }
                        assert!(!b.writes.iter().any(|v| prompt(tool, v)));
                        let b_id = control_id(tool, &b);
                        assert_ne!(old_id, b_id);
                        let target = if tool == Tool::Claude {
                            &b.writes[0]["request"]["model"]
                        } else {
                            b.writes[0]["params"]
                                .get("modelId")
                                .unwrap_or(&b.writes[0]["params"]["value"])
                        };
                        assert_eq!(target, "old");
                        let stale = a.ingest(ack(tool, old_id, "A")).unwrap();
                        assert!(stale.writes.is_empty());
                        let confirmed = a.ingest(ack(tool, b_id, "old")).unwrap();
                        assert_eq!(
                            confirmed.writes.iter().filter(|v| prompt(tool, v)).count(),
                            1
                        );
                        assert!(confirmed.events.iter().any(
                            |e| matches!(e,Event::Models{selected:Some(model),..}if model=="old")
                        ));
                    }
                }
            }
        }
    }
    #[test]
    fn unsettled_none_without_known_model_is_rejected_until_explicit_model_is_confirmed() {
        for tool in [Tool::Claude, Tool::Antigravity] {
            let mut a = Adapter::new(tool, "/tmp".into(), None);
            if tool == Tool::Claude {
                a.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{}}})).unwrap();
            } else {
                a.ingest(json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{}}})).unwrap();
                a.ingest(json!({"id":"session","result":{"sessionId":"native","models":{"availableModels":[{"modelId":"A","name":"A"},{"modelId":"B","name":"B"}]}}})).unwrap();
            }
            let old = control_id(tool, &submit(&mut a, "A"));
            a.command(&Command::Interrupt, "fixture").unwrap();
            assert!(
                a.command(
                    &Command::Submit {
                        text: "unknown default must not run".into(),
                        attachments: vec![],
                        model: None
                    },
                    "fixture"
                )
                .is_err()
            );
            a.ingest(failed_config(tool, old, true)).unwrap();
            let next = submit(&mut a, "B");
            assert!(!next.writes.iter().any(|v| prompt(tool, v)));
            let confirmed = a.ingest(ack(tool, control_id(tool, &next), "B")).unwrap();
            assert!(confirmed.writes.iter().any(|v| prompt(tool, v)));
        }
    }
    #[test]
    fn cancelled_acp_mode_stays_unknown_until_fresh_mode_evidence_and_cannot_bypass_model_barrier()
    {
        let mut a = initialized(Tool::Antigravity);
        a.ingest(json!({"id":"session","result":{"sessionId":"native","modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Ask"},{"id":"code","name":"Code"}]}}})).unwrap();
        let change = a
            .command(
                &Command::SetMode {
                    mode: "code".into(),
                },
                "fixture",
            )
            .unwrap();
        let mode_id = control_id(Tool::Antigravity, &change);
        assert!(
            change
                .events
                .iter()
                .any(|e| matches!(e, Event::Modes { selected: None, .. }))
        );
        a.command(&Command::Interrupt, "fixture").unwrap();
        let b = submit(&mut a, "old");
        assert!(b.writes.is_empty());
        let settled=a.ingest(json!({"id":mode_id,"result":{"configOptions":[{"id":"mode","category":"mode","type":"select","currentValue":"code","options":[{"value":"ask","name":"Ask"},{"value":"code","name":"Code"}]}]}})).unwrap();
        assert!(!settled.events.iter().any(|e| matches!(
            e,
            Event::Modes {
                selected: Some(_),
                ..
            }
        )));
        let model_id = control_id(Tool::Antigravity, &settled);
        let confirmed = a.ingest(ack(Tool::Antigravity, model_id, "old")).unwrap();
        assert!(
            confirmed
                .writes
                .iter()
                .any(|v| prompt(Tool::Antigravity, v))
        );
        assert!(
            !confirmed.events.iter().any(|e| matches!(
                e,
                Event::Modes {
                    selected: Some(_),
                    ..
                }
            )),
            "model confirmation alone cannot confirm a permission mode"
        );
    }
    #[test]
    fn cancelling_a_queued_replacement_does_not_create_an_unsent_settlement_barrier() {
        for tool in [Tool::Claude, Tool::Antigravity] {
            let mut a = initialized(tool);
            let sent = submit(&mut a, "A");
            let old_id = control_id(tool, &sent);
            a.command(&Command::Interrupt, "fixture").unwrap();
            assert!(submit(&mut a, "old").writes.is_empty());
            a.command(&Command::Interrupt, "fixture").unwrap();
            assert!(submit(&mut a, "B").writes.is_empty());
            let released = a.ingest(ack(tool, old_id, "A")).unwrap();
            assert_eq!(released.writes.len(), 1);
            assert!(!released.writes.iter().any(|v| prompt(tool, v)));
            let confirmed = a
                .ingest(ack(tool, control_id(tool, &released), "B"))
                .unwrap();
            assert_eq!(
                confirmed.writes.iter().filter(|v| prompt(tool, v)).count(),
                1
            );
        }
    }
    #[test]
    fn uncorrelated_model_metadata_cannot_replace_the_last_confirmed_default_during_settlement() {
        for tool in [Tool::Claude, Tool::Antigravity] {
            let mut a = initialized(tool);
            let old_id = control_id(tool, &submit(&mut a, "A"));
            a.command(&Command::Interrupt, "fixture").unwrap();
            let metadata = if tool == Tool::Claude {
                json!({"type":"system","model":"A"})
            } else {
                json!({"method":"session/update","params":{"sessionId":"native","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"model","category":"model","type":"select","currentValue":"A","options":[{"value":"old","name":"Old"},{"value":"A","name":"A"}]}]}}})
            };
            let update = a.ingest(metadata).unwrap();
            assert!(!update.events.iter().any(|e| matches!(
                e,
                Event::Models {
                    selected: Some(_),
                    ..
                }
            )));
            a.command(
                &Command::Submit {
                    text: "default must reconfirm last known O".into(),
                    attachments: vec![],
                    model: None,
                },
                "fixture",
            )
            .unwrap();
            let released = a.ingest(ack(tool, old_id, "A")).unwrap();
            let wire = &released.writes[0];
            let model = if tool == Tool::Claude {
                &wire["request"]["model"]
            } else {
                wire["params"]
                    .get("modelId")
                    .unwrap_or(&wire["params"]["value"])
            };
            assert_eq!(model, "old");
        }
    }
    #[test]
    fn claude_startup_model_metadata_cannot_undo_a_later_matching_confirmation() {
        let mut a = initialized(Tool::Claude);
        let selected = submit(&mut a, "A");
        a.ingest(ack(Tool::Claude, control_id(Tool::Claude, &selected), "A"))
            .unwrap();
        a.ingest(json!({"type":"result","is_error":false})).unwrap();
        a.ingest(json!({"type":"system","subtype":"init","model":"old"}))
            .unwrap();
        let restore = submit(&mut a, "old");
        assert!(
            !restore.writes.iter().any(|v| prompt(Tool::Claude, v)),
            "stale startup metadata cannot justify the old model"
        );
        assert_eq!(restore.writes[0]["request"]["model"], "old");
    }
}

#[cfg(test)]
mod controls_tests;

#[cfg(test)]
mod image_wire_tests {
    use super::*;
    /// A child test process supplies isolated storage without changing the runner's environment.
    #[test]
    fn all_three_provider_commands_deliver_decodable_image_bytes() {
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "app::agent_gui::providers::image_wire_tests::image_wire_fixture_child", "--nocapture"])
            .env("KILN_IMAGE_WIRE_FIXTURE", "1")
            .env("KILN_CONFIG_DIR", root.path())
            .status().unwrap();
        assert!(status.success());
    }
    #[test]
    fn image_wire_fixture_child() {
        if std::env::var("KILN_IMAGE_WIRE_FIXTURE").as_deref() != Ok("1") { return; }
        use base64::Engine;
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([34, 67, 90, 255])));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        for tool in [Tool::Claude, Tool::Codex, Tool::Antigravity] {
            let descriptor = Descriptor { id: journal::unique(), tool, cwd: std::env::temp_dir(), account_label: "Image wire fixture".into() };
            let source = kiln_common::paths::config_dir().join("fixture-source.png");
            std::fs::write(&source, png.get_ref()).unwrap();
            let attachment = super::super::import_attachment(&descriptor, &source).unwrap();
            let mut adapter = Adapter::new(tool, descriptor.cwd.clone(), None);
            adapter.start();
            match tool {
                Tool::Claude => {
                    adapter.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{}}})).unwrap();
                    adapter.ingest(json!({"type":"system","subtype":"init","session_id":"fixture","model":"fixture-model"})).unwrap();
                }
                Tool::Codex => {
                    adapter.ingest(json!({"id":"initialize","result":{}})).unwrap();
                    adapter.ingest(json!({"id":"thread","result":{"thread":{"id":"fixture"},"model":"fixture-model"}})).unwrap();
                }
                Tool::Antigravity => {
                    adapter.ingest(json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"promptCapabilities":{"image":true}}}})).unwrap();
                    adapter.ingest(json!({"id":"session","result":{"sessionId":"fixture","models":{"currentModelId":"fixture-model","availableModels":[{"modelId":"fixture-model","name":"Fixture"}]}}})).unwrap();
                }
            }
            let mut effects = adapter.command(&Command::Submit { text: "inspect the image".into(), attachments: vec![attachment.clone()], model: None }, &descriptor.id).unwrap();
            // Claude may require a model acknowledgement before dispatching a prompt.
            if tool == Tool::Claude && !effects.writes.iter().any(|v| v["type"] == "user") {
                let request = effects.writes.iter().find(|v| v["request"]["subtype"] == "set_model").unwrap();
                effects = adapter.ingest(json!({"type":"control_response","response":{"request_id":request["request_id"],"subtype":"success","response":{}}})).unwrap();
            }
            let bytes = match tool {
                Tool::Claude => {
                    let wire = effects.writes.iter().find(|v| v["type"] == "user").unwrap();
                    let block = &wire["message"]["content"][1];
                    assert_eq!(block["source"]["media_type"], "image/png");
                    base64::engine::general_purpose::STANDARD.decode(block["source"]["data"].as_str().unwrap()).unwrap()
                }
                Tool::Codex => {
                    let wire = effects.writes.iter().find(|v| v["method"] == "turn/start").unwrap();
                    let block = &wire["params"]["input"][1];
                    assert_eq!(block["type"], "localImage");
                    let path = PathBuf::from(block["path"].as_str().unwrap());
                    assert_eq!(path, attachment.path);
                    assert!(path.is_absolute());
                    std::fs::read(path).unwrap()
                }
                Tool::Antigravity => {
                    let wire = effects.writes.iter().find(|v| v["method"] == "session/prompt").unwrap();
                    let block = &wire["params"]["prompt"][1];
                    assert_eq!(block["mimeType"], "image/png");
                    base64::engine::general_purpose::STANDARD.decode(block["data"].as_str().unwrap()).unwrap()
                }
            };
            assert_eq!(bytes, *png.get_ref());
            assert_eq!(super::super::decode_attachment_image(&bytes).unwrap().into_rgba8(), image.to_rgba8());
        }
    }
}
