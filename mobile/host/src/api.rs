//! Public v1 DTO boundary. Never serialize Descriptor, Transcript, or Config wholesale.
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use kiln::app::agent_gui::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Debug)]
pub struct Error {
    pub status: StatusCode,
    pub code: &'static str,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
    pub fn bad(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code)
    }
    pub fn conflict(code: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code)
    }
    pub fn unavailable() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "native_unavailable")
    }
}
impl Error {
    pub fn body(&self) -> Value {
        json!({"error":{"code":self.code,"message":match self.code {
            "unauthorized"=>"Pair again with this computer.",
            "incomplete_approval"=>"Review complete approval details on the computer.",
            "stale_generation"=>"Refresh this conversation before acting.",
            "request_id_conflict"=>"Request ID already belongs to a different mutation.",
            _=>"The request could not be completed safely."
        }}})
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.status, Json(self.body())).into_response()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversation {
    pub request_id: String,
    pub workspace_id: String,
    pub provider: kiln_accounts::Tool,
    pub launch_options: LaunchOptions,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub request_id: String,
    pub generation: String,
    pub command: MobileCommand,
}
#[derive(Deserialize, Serialize, Clone)]
pub enum MobileCommand {
    Submit {
        text: String,
        attachments: Vec<String>,
        model: Option<String>,
    },
    SubmitWithSkills {
        text: String,
        attachments: Vec<String>,
        model: Option<String>,
        skills: Vec<NativeSkillInput>,
    },
    Respond {
        request_id: String,
        decision: Decision,
        answers: std::collections::BTreeMap<String, Vec<String>>,
    },
    Configure {
        control: String,
        value: ControlValue,
    },
    Discover(Discovery),
    NativeAction(NativeAction),
    SetMode {
        mode: String,
    },
    Interrupt,
    Restart,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub request_id: String,
    pub filename: String,
    pub mime: String,
    pub bytes_base64: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalInput {
    pub request_id: String,
    pub text: String,
    pub enter: bool,
}
pub const SNAPSHOT_LIMIT: usize = 2 * 1024 * 1024;
pub const FIELD_LIMIT: usize = 120_000;
pub const APPROVAL_LIMIT: usize = 64 * 1024;
pub fn clip(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1
    }
    s[..end].into()
}
pub fn public_descriptor(d: &Descriptor, w: &crate::metadata::Workspace) -> Value {
    json!({"id":d.id,"provider":d.tool,"workspace_id":w.id,"account_label":clip(&d.account_label,256)})
}
pub fn complete_request(r: &Request) -> bool {
    if r.details.get("kiln_mobile_incomplete") == Some(&Value::Bool(true)) {
        return false;
    }
    serde_json::to_vec(r).is_ok_and(|v| v.len() <= APPROVAL_LIMIT) && !r.title.is_empty()
}
pub fn request(r: &Request) -> Value {
    if complete_request(r) {
        let supported = r.details.get("permissions").is_none();
        json!({"id":r.id,"title":r.title,"questions":r.questions,"options":r.options,"details":r.details,"respondable":supported,"reason":if supported{Value::Null}else{json!("This request requires the computer's structured permission editor.")}})
    } else {
        json!({"id":clip(&r.id,256),"title":clip(&r.title,1024),"questions":[],"options":[],"details":null,"respondable":false,"reason":"Review complete approval details on the computer."})
    }
}
pub fn controls(c: &NativeControls) -> Value {
    let options = c
        .options
        .iter()
        .map(|o| {
            let supported = o.available
                && o.scope == ControlScope::Live
                && !matches!(
                    o.kind,
                    ControlKind::Sandbox
                        | ControlKind::GranularApproval
                        | ControlKind::Unsupported(_)
                );
            let mut v = serde_json::to_value(o).unwrap();
            v["mobile_supported"] = json!(supported);
            v["mobile_reason"] = if supported {
                Value::Null
            } else {
                json!(
                    "This setting requires review on the computer or is unavailable in this scope."
                )
            };
            v
        })
        .collect::<Vec<_>>();
    json!({"coverage":c.coverage,"options_reported":c.options_reported,"models_reported":c.models_reported,"capabilities":c.capabilities,"queue":c.queue,"options":options,"models":c.models,"commands":c.commands,"skills":c.skills,"agents":c.agents,"styles":c.styles,"outcomes":c.outcomes,"status":c.status})
}
pub fn snapshot(
    d: Value,
    t: &Transcript,
    replay_ready: bool,
    history_truncated: bool,
    attachments: &mut std::collections::BTreeMap<String, Attachment>,
) -> Value {
    let mut truncated = history_truncated || t.entries.len() > 200;
    let start = t.entries.len().saturating_sub(200);
    let mut entries = vec![];
    for e in &t.entries[start..] {
        let v = match e {
            Entry::Message(m) => {
                let a: Vec<_> = m
                    .attachments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let id = format!("{}-{i}", m.id);
                        if attachments.len() < 128 || attachments.contains_key(&id) {
                            attachments.insert(id.clone(), a.clone());
                        }
                        json!({"id":id,"name":clip(&a.name,256),"mime":a.mime,"sha256":a.sha256})
                    })
                    .collect();
                truncated |= m.text.len() > FIELD_LIMIT;
                json!({"Message":{"id":m.id,"role":m.role,"text":clip(&m.text,FIELD_LIMIT),"attachments":a}})
            }
            Entry::Tool(v) => {
                let input = serde_json::to_string(&v.input).unwrap_or_default();
                let input_truncated = input.len() > FIELD_LIMIT
                    || v.input.get("kiln_mobile_incomplete") == Some(&Value::Bool(true));
                truncated |=
                    input_truncated || v.output.len() > FIELD_LIMIT || v.diff.len() > FIELD_LIMIT;
                json!({"Tool":{"id":v.id,"name":clip(&v.name,1024),"input":if input_truncated{Value::Null}else{v.input.clone()},"input_truncated":input_truncated,"output":clip(&v.output,FIELD_LIMIT),"diff":clip(&v.diff,FIELD_LIMIT),"status":v.status,"presentation":v.presentation}})
            }
        };
        entries.push(v);
    }
    let requests: Vec<_> = t.requests.values().take(128).map(request).collect();
    truncated |= t.requests.len() > 128;
    let mut v = json!({"descriptor":d,"generation":t.generation,"sequence":t.sequence,"lifecycle":t.lifecycle,"entries":entries,"requests":requests,"models":t.models,"selected_model":t.selected_model.as_ref().map(|s|clip(s,4096)),"modes":t.modes,"selected_mode":t.selected_mode.as_ref().map(|s|clip(s,4096)),"controls":controls(&t.controls),"commands":t.commands,"uncertain_deliveries":t.uncertain_deliveries,"errors":if t.errors.is_empty(){vec![]}else{vec!["Native runtime reported an error; inspect the computer."]},"replay_ready":replay_ready,"coverage":{"truncated":truncated,"entries_retained":0,"entry_limit":200,"byte_limit":SNAPSHOT_LIMIT,"approval_details_complete":t.requests.len()<=128&&t.requests.values().all(complete_request)}});
    // Drop old entries first. A controls/request projection that is itself too big becomes visibly read-only.
    while serde_json::to_vec(&v).unwrap().len() > SNAPSHOT_LIMIT - 1024
        && !v["entries"].as_array().unwrap().is_empty()
    {
        v["entries"].as_array_mut().unwrap().remove(0);
        v["coverage"]["truncated"] = json!(true);
    }
    if serde_json::to_vec(&v).unwrap().len() > SNAPSHOT_LIMIT - 1024 {
        v["controls"] =
            json!({"unavailable":"Snapshot controls exceed mobile coverage; review on computer."});
        v["models"] = json!([]);
        v["modes"] = json!([]);
        v["commands"] = json!({});
        v["uncertain_deliveries"] = json!([]);
        v["requests"]=json!(t.requests.values().take(128).map(|r|json!({"id":clip(&r.id,256),"title":clip(&r.title,256),"questions":[],"options":[],"details":null,"respondable":false,"reason":"Snapshot exceeds complete mobile coverage; review on computer."})).collect::<Vec<_>>());
        v["coverage"]["truncated"] = json!(true);
        v["coverage"]["approval_details_complete"] = json!(false);
        v["replay_ready"] = json!(false);
    }
    v["coverage"]["entries_retained"] = json!(v["entries"].as_array().unwrap().len());
    v
}
