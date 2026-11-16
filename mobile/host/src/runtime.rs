use crate::jobs::{Job, Output, Runner};
use crate::{
    api::{self, Error, Result},
    metadata::{self, Workspace},
};
use kiln::app::agent_gui::*;

use kiln_proto::{ClientMsg, ServerMsg, SessionInfo};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub struct Reader {
    pub client: Client,
    pub transcript: Transcript,
    pub truncated: bool,
    pub attachments: BTreeMap<String, Attachment>,
}
impl Reader {
    fn refresh(&mut self) {
        for _ in 0..8 {
            let events = self.client.drain();
            if events.is_empty() {
                break;
            }
            for e in events {
                self.transcript.apply(&e);
            }
            // Retain a bounded safe view; the durable native journal remains authoritative.
            for entry in &mut self.transcript.entries {
                match entry {
                    Entry::Message(m) => {
                        if m.text.len() > api::FIELD_LIMIT {
                            m.text = api::clip(&m.text, api::FIELD_LIMIT);
                            self.truncated = true;
                        }
                        m.attachments.truncate(16);
                    }
                    Entry::Tool(t) => {
                        for field in [&mut t.output, &mut t.diff] {
                            if field.len() > api::FIELD_LIMIT {
                                *field = api::clip(field, api::FIELD_LIMIT);
                                self.truncated = true;
                            }
                        }
                        if serde_json::to_vec(&t.input).is_ok_and(|v| v.len() > api::FIELD_LIMIT) {
                            t.input = json!({"kiln_mobile_incomplete":true});
                            self.truncated = true;
                        }
                    }
                }
            }
            for r in self.transcript.requests.values_mut() {
                if !api::complete_request(r) {
                    r.title = api::clip(&r.title, 1024);
                    r.questions.clear();
                    r.options.clear();
                    r.details = json!({"kiln_mobile_incomplete":true});
                }
            }
            if self.transcript.entries.len() > 200 {
                let excess = self.transcript.entries.len() - 200;
                self.transcript.entries.drain(..excess);
                self.truncated = true;
            }
        }
    }
}
pub struct Runtime {
    pub workspaces: Vec<Workspace>,
    pub cli: PathBuf,
    pub root: PathBuf,
    readers: Mutex<BTreeMap<String, Reader>>,
    pub runner: Runner,
}
impl Runtime {
    pub fn new(workspaces: Vec<Workspace>, cli: PathBuf) -> Self {
        let config = kiln_common::paths::config_dir();
        let config = std::fs::canonicalize(&config).unwrap_or(config);
        Self {
            workspaces,
            cli,
            root: config.join("agent-gui"),
            readers: Mutex::new(BTreeMap::new()),
            runner: Runner::default(),
        }
    }
    pub fn descriptor(&self, id: &str) -> Result<(Descriptor, &Workspace)> {
        let d = metadata::read_descriptor(&self.root, id).map_err(|_| {
            Error::new(
                axum::http::StatusCode::NOT_FOUND,
                "conversation_unavailable",
            )
        })?;
        let w = metadata::allowed(&self.workspaces, &d.cwd).ok_or_else(|| {
            Error::new(
                axum::http::StatusCode::NOT_FOUND,
                "conversation_unavailable",
            )
        })?;
        Ok((d, w))
    }
    pub fn list(&self) -> Result<Value> {
        let iter = match std::fs::read_dir(&self.root) {
            Ok(i) => i,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(json!({"conversations":[],"truncated":false}));
            }
            Err(_) => return Err(Error::unavailable()),
        };
        metadata::directory(&self.root).map_err(|_| Error::unavailable())?;
        let mut out = vec![];
        let mut examined = 0;
        for e in iter.take(1025) {
            examined += 1;
            if out.len() >= 200 {
                break;
            }
            let Ok(e) = e else { continue };
            let Some(id) = e.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Ok((d, w)) = self.descriptor(&id) {
                let summary = Client::summary_hardened(&d);
                let row = match summary {
                    Ok(v) => {
                        json!({"descriptor":api::public_descriptor(&d,w),"title":v.title.map(|s|api::clip(&s,640)),"lifecycle":v.lifecycle,"summary":{"partial":v.partial,"unavailable":v.partial.then_some("History exceeds bounded summary coverage.")}})
                    }
                    Err(_) => {
                        json!({"descriptor":api::public_descriptor(&d,w),"title":null,"lifecycle":null,"summary":{"partial":true,"unavailable":"Conversation history is unavailable for safe summary."}})
                    }
                };
                out.push(row);
            }
        }
        Ok(json!({"conversations":out,"truncated":examined>200}))
    }
    pub fn with_reader<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Reader, &Descriptor, &Workspace) -> Result<T>,
    ) -> Result<T> {
        let (d, w) = self.descriptor(id)?;
        let mut readers = self.readers.lock().unwrap();
        if !readers.contains_key(id) {
            if readers.len() >= 16 {
                return Err(Error::new(
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    "reader_limit",
                ));
            }
            readers.insert(
                id.into(),
                Reader {
                    client: Client::connect_hardened(d.clone()).map_err(|_| {
                        Error::new(
                            axum::http::StatusCode::NOT_FOUND,
                            "conversation_unavailable",
                        )
                    })?,
                    transcript: Transcript::default(),
                    truncated: false,
                    attachments: BTreeMap::new(),
                },
            );
        }
        let r = readers.get_mut(id).unwrap();
        r.refresh();
        f(r, &d, w)
    }
    pub fn snapshot(&self, id: &str) -> Result<Value> {
        self.with_reader(id, |r, d, w| {
            let ready = r
                .client
                .replay_current(&r.transcript.generation, r.transcript.sequence);
            Ok(api::snapshot(
                api::public_descriptor(d, w),
                &r.transcript,
                ready,
                r.truncated,
                &mut r.attachments,
            ))
        })
    }
    pub fn launch(
        &self,
        wid: &str,
        provider: kiln_accounts::Tool,
        options: LaunchOptions,
    ) -> Result<Value> {
        let w = self
            .workspaces
            .iter()
            .find(|w| w.id == wid)
            .ok_or_else(|| Error::bad("workspace_unavailable"))?;
        metadata::allowed(&self.workspaces, &w.path)
            .ok_or_else(|| Error::bad("workspace_unavailable"))?;
        validate_launch(&options, &self.workspaces)?;
        let mut guards = vec![
            w.identity.clone(),
            metadata::PathIdentity::capture(&w.path)
                .map_err(|_| Error::bad("workspace_changed"))?,
        ];
        let additional = match &options {
            LaunchOptions::Codex(v) => v.workspace_roots.clone(),
            LaunchOptions::Antigravity(v) => v.additional_directories.clone(),
            _ => vec![],
        };
        for path in additional {
            guards.push(
                metadata::PathIdentity::capture(&path)
                    .map_err(|_| Error::bad("workspace_changed"))?,
            );
        }
        let mut spec = match self.runner.run(Job::Prepare {
            cwd: w.path.clone(),
            provider,
            options,
        })? {
            Output::Prepared(spec) => spec,
            _ => return Err(Error::unavailable()),
        };
        for guard in &guards {
            guard
                .validate()
                .map_err(|_| Error::bad("workspace_changed"))?;
        }
        spec.program = Some(self.cli.to_string_lossy().into_owned());
        let d = descriptor(&spec).ok_or_else(Error::unavailable)?;
        // Parent recheck after preparation; helper rechecks after bounded daemon Hello, directly before Create.
        for guard in &guards {
            guard
                .validate()
                .map_err(|_| Error::bad("workspace_changed"))?;
        }
        match self.runner.run(Job::Daemon {
            message: ClientMsg::Create { req: 1, spec },
            guards,
        })? {
            Output::Native(ServerMsg::Created { session, .. }) => {
                Ok(json!({"conversation_id":d.id,"terminal_id":session.to_string()}))
            }
            _ => Err(Error::unavailable()),
        }
    }

    pub fn command(
        &self,
        id: &str,
        generation: &str,
        command: api::MobileCommand,
    ) -> Result<Value> {
        if self
            .runner
            .stopping
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(Error::unavailable());
        }
        self.with_reader(id, |r, _d, _w| {
            if generation.is_empty() || generation != r.transcript.generation {
                return Err(Error::conflict("stale_generation"));
            }
            let ready = r
                .client
                .replay_current(&r.transcript.generation, r.transcript.sequence);
            if !matches!(command, api::MobileCommand::Interrupt) && !ready {
                return Err(Error::conflict("catching_up"));
            }
            let t = &r.transcript;
            let unsettled = t.controls.outcomes.values().any(|o| !o.settled);
            let convert_attachments = |ids: Vec<String>| -> Result<Vec<Attachment>> {
                if ids.len() > 16 {
                    return Err(Error::bad("attachment_limit"));
                }
                ids.into_iter()
                    .map(|id| {
                        r.attachments
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| Error::bad("attachment_unavailable"))
                    })
                    .collect()
            };
            let native = match command {
                api::MobileCommand::Submit {
                    text,
                    attachments,
                    model,
                } => {
                    validate_submit(t, &text, &model, unsettled)?;
                    Command::Submit {
                        text,
                        attachments: convert_attachments(attachments)?,
                        model,
                    }
                }
                api::MobileCommand::SubmitWithSkills {
                    text,
                    attachments,
                    model,
                    skills,
                } => {
                    validate_submit(t, &text, &model, unsettled)?;
                    if skills.len() > 128
                        || !skills.iter().all(|s| {
                            t.controls.skills.as_ref().is_some_and(|v| {
                                v.iter().any(|e| {
                                    e.name == s.name
                                        && e.path.as_deref() == Some(&s.path)
                                        && !e.terminal_only
                                        && e.enabled != Some(false)
                                })
                            })
                        })
                    {
                        return Err(Error::bad("skill_unavailable"));
                    }
                    Command::SubmitWithSkills {
                        text,
                        attachments: convert_attachments(attachments)?,
                        model,
                        skills,
                    }
                }
                api::MobileCommand::Respond {
                    request_id,
                    decision,
                    answers,
                } => {
                    let request = t
                        .requests
                        .get(&request_id)
                        .ok_or_else(|| Error::conflict("request_unavailable"))?;
                    let projected =
                        api::snapshot(Value::Null, t, ready, r.truncated, &mut BTreeMap::new());
                    if !api::complete_request(request)
                        || projected["coverage"]["approval_details_complete"] != true
                    {
                        return Err(Error::conflict("incomplete_approval"));
                    }
                    validate_response(request, &decision, &answers)?;
                    Command::Respond {
                        request_id,
                        decision,
                        answers,
                    }
                }
                api::MobileCommand::Configure { control, value } => {
                    if unsettled {
                        return Err(Error::conflict("configuration_pending"));
                    }
                    let option = t
                        .controls
                        .options
                        .iter()
                        .find(|o| o.id == control)
                        .ok_or_else(|| Error::bad("control_unavailable"))?;
                    validate_control(option, &value)?;
                    Command::Configure { control, value }
                }
                api::MobileCommand::SetMode { mode } => {
                    if unsettled || !t.modes.iter().any(|c| c.id == mode) {
                        return Err(Error::bad("mode_unavailable"));
                    }
                    Command::SetMode { mode }
                }
                api::MobileCommand::Discover(d) => Command::Discover(d),
                api::MobileCommand::Interrupt => Command::Interrupt,
                // Advanced actions remain visible in capabilities; mobile cannot turn arbitrary payloads into a working action.
                api::MobileCommand::NativeAction(_) | api::MobileCommand::Restart => {
                    return Err(Error::bad("unsupported_mobile_action"));
                }
            };
            let command_id = r
                .client
                .send_for_generation(generation, native)
                .map_err(|_| {
                    Error::new(
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        "delivery_unknown",
                    )
                })?;
            Ok(json!({"command_id":command_id,"delivery":"queued"}))
        })
    }
    pub fn upload(&self, id: &str, v: api::Upload) -> Result<Value> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(v.bytes_base64)
            .map_err(|_| Error::bad("invalid_image"))?;
        if bytes.len() > 20 * 1024 * 1024
            || bytes.is_empty()
            || v.filename.len() > 256
            || v.filename.is_empty()
            || v.filename.contains(['/', '\\', '\0'])
        {
            return Err(Error::bad("invalid_image"));
        }
        self.with_reader(id, |r, d, _| {
            if r.attachments.len() >= 128 {
                return Err(Error::bad("attachment_limit"));
            }
            let staging = tempfile::Builder::new()
                .prefix("kiln-mobile-upload-")
                .tempdir()
                .map_err(|_| Error::unavailable())?;
            let file = staging.path().join(&v.filename);
            std::fs::write(&file, &bytes).map_err(|_| Error::unavailable())?;
            let a = import_attachment(d, &file).map_err(|_| Error::bad("invalid_image"))?;
            if a.mime != v.mime {
                let _ = std::fs::remove_file(&a.path);
                return Err(Error::bad("invalid_image"));
            }
            let aid = format!(
                "upload-{}",
                a.path.file_name().unwrap_or_default().to_string_lossy()
            );
            let out =
                json!({"attachment":{"id":aid,"name":a.name,"mime":a.mime,"sha256":a.sha256}});
            r.attachments.insert(aid, a);
            Ok(out)
        })
    }
    pub fn sessions(&self) -> Result<Vec<SessionInfo>> {
        match self.runner.run(Job::Daemon {
            message: ClientMsg::ListSessions { req: 1 },
            guards: vec![],
        })? {
            Output::Native(ServerMsg::Sessions { sessions, .. }) => Ok(sessions
                .into_iter()
                .filter(|s| {
                    s.cwd.as_ref().is_some_and(|p| {
                        metadata::allowed(&self.workspaces, std::path::Path::new(p)).is_some()
                    })
                })
                .take(201)
                .collect()),
            _ => Err(Error::unavailable()),
        }
    }
    pub fn terminals(&self) -> Result<Value> {
        let sessions = self.sessions()?;
        Ok(
            json!({"terminals":sessions.iter().take(200).map(|s|json!({"id":s.id.to_string(),"title":api::clip(&s.title,256),"exited":s.exited})).collect::<Vec<_>>(),"truncated":sessions.len()>200}),
        )
    }
    pub fn terminal(&self, id: &str, input: Option<api::TerminalInput>) -> Result<Value> {
        let session = self
            .sessions()?
            .into_iter()
            .find(|s| s.id.to_string() == id)
            .ok_or_else(|| Error::bad("terminal_unavailable"))?;

        if let Some(v) = input {
            if session.exited.is_some() || v.text.len() > api::FIELD_LIMIT || v.text.contains('\0')
            {
                return Err(Error::bad("terminal_unavailable"));
            }
            let mut data = v.text.into_bytes();
            if v.enter {
                data.push(b'\r')
            }
            let cwd = session.cwd.as_ref().ok_or_else(Error::unavailable)?;
            self.runner.run(Job::Daemon {
                message: ClientMsg::Input {
                    session: session.id,
                    data,
                },
                guards: vec![
                    metadata::PathIdentity::capture(std::path::Path::new(cwd))
                        .map_err(|_| Error::bad("workspace_changed"))?,
                ],
            })?;
            return Ok(json!({"delivery":"queued"}));
        }
        let text = match self.runner.run(Job::Daemon {
            message: ClientMsg::ReadText {
                req: 1,
                session: session.id,
                history: 200,
            },
            guards: vec![],
        })? {
            Output::Native(ServerMsg::Text { text, .. }) => text,
            _ => return Err(Error::unavailable()),
        };
        Ok(
            json!({"id":id,"title":api::clip(&session.title,256),"exited":session.exited,"text":api::clip(&text,api::FIELD_LIMIT),"truncated":text.len()>api::FIELD_LIMIT}),
        )
    }
}
fn validate_launch(options: &LaunchOptions, workspaces: &[Workspace]) -> Result<()> {
    let dirs = match options {
        LaunchOptions::Codex(v) => &v.workspace_roots,
        LaunchOptions::Antigravity(v) => &v.additional_directories,
        _ => return Ok(()),
    };
    if dirs.len() > 64
        || dirs
            .iter()
            .any(|p| metadata::allowed(workspaces, p).is_none())
    {
        return Err(Error::bad("workspace_unavailable"));
    }
    Ok(())
}
fn validate_submit(
    t: &Transcript,
    text: &str,
    model: &Option<String>,
    unsettled: bool,
) -> Result<()> {
    if text.len() > api::FIELD_LIMIT || text.contains('\0') {
        return Err(Error::bad("invalid_text"));
    }
    if unsettled {
        return Err(Error::conflict("configuration_pending"));
    }
    if !matches!(t.lifecycle, Lifecycle::Ready) {
        return Err(Error::conflict("not_ready"));
    }
    if model
        .as_ref()
        .is_some_and(|m| !t.models.iter().any(|c| &c.id == m))
    {
        return Err(Error::bad("model_unavailable"));
    }
    Ok(())
}
fn validate_control(o: &ControlOption, v: &ControlValue) -> Result<()> {
    if !o.available || o.scope != ControlScope::Live {
        return Err(Error::bad("control_unavailable"));
    }
    if matches!(
        o.kind,
        ControlKind::Sandbox | ControlKind::GranularApproval | ControlKind::Unsupported(_)
    ) {
        return Err(Error::bad("unsupported_mobile_control"));
    }
    let valid = match (&o.kind, v) {
        (_, ControlValue::Default) => o.source.as_deref() != Some("acp"),
        (ControlKind::Select, ControlValue::Text(s)) => o.choices.iter().any(|c| &c.value == s),
        (ControlKind::Boolean, ControlValue::Bool(_)) => true,
        (ControlKind::Text, ControlValue::Text(s)) => s.len() <= 4096 && !s.contains('\0'),
        (ControlKind::Integer { min, max }, ControlValue::Integer(n)) => n >= min && n <= max,
        (ControlKind::Strings, ControlValue::Strings(v)) => {
            v.len() <= 128 && v.iter().all(|s| s.len() <= 4096 && !s.contains('\0'))
        }
        _ => false,
    };
    if !valid {
        return Err(Error::bad("invalid_control_value"));
    }
    Ok(())
}
fn validate_response(
    r: &Request,
    d: &Decision,
    answers: &BTreeMap<String, Vec<String>>,
) -> Result<()> {
    if answers.len() > r.questions.len()
        || answers.iter().any(|(key, v)| {
            !r.questions.iter().any(|q| {
                &q.id == key
                    && (!q.multi_select && v.len() <= 1 || q.multi_select)
                    && v.len() <= 128
                    && v.iter().all(|a| a.len() <= 4096)
            })
        })
    {
        return Err(Error::bad("invalid_answers"));
    }
    if r.details.get("permissions").is_some() {
        return Err(Error::bad("unsupported_mobile_decision"));
    }
    match d {
        Decision::Allow | Decision::Deny | Decision::Cancel => {}
        Decision::Option(s) if r.options.iter().any(|o| &o.id == s) => {} // Structured broad permission grants require a dedicated reviewed mobile form.
        _ => return Err(Error::bad("unsupported_mobile_decision")),
    }
    Ok(())
}

pub struct Record {
    pub fingerprint: Vec<u8>,
    pub started: Instant,
    pub receipt: Value,
    pub state: &'static str,
    pub result: Option<Value>,
    pub error: Option<&'static str>,
    pub reply: Option<(axum::http::StatusCode, Value)>,
    pub completion: tokio::sync::watch::Sender<bool>,
}
pub struct Mutations {
    pub records: Mutex<BTreeMap<String, Record>>,
    pub preparations: Arc<tokio::sync::Semaphore>,
}
impl Default for Mutations {
    fn default() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
            preparations: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}
impl Mutations {
    pub fn existing(&self, id: &str, fingerprint: &[u8]) -> Result<bool> {
        if !metadata::valid_id(id) {
            return Err(Error::bad("invalid_request_id"));
        }
        let records = self.records.lock().unwrap();
        if let Some(r) = records
            .get(id)
            .filter(|r| r.state == "pending" || r.started.elapsed() < Duration::from_secs(600))
        {
            if r.fingerprint != fingerprint {
                return Err(Error::conflict("request_id_conflict"));
            }
            return Ok(true);
        }
        Ok(false)
    }
    /// Reserve before starting any native work. Pending records are never evicted.
    pub fn register(&self, id: &str, fingerprint: Vec<u8>) -> Result<bool> {
        if !metadata::valid_id(id) {
            return Err(Error::bad("invalid_request_id"));
        }
        let mut records = self.records.lock().unwrap();
        records
            .retain(|_, v| v.state == "pending" || v.started.elapsed() < Duration::from_secs(600));
        if let Some(r) = records.get(id) {
            if r.fingerprint != fingerprint {
                return Err(Error::conflict("request_id_conflict"));
            }
            return Ok(false);
        }
        if records.len() >= 1024 {
            return Err(Error::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "request_cache_full",
            ));
        }
        let receipt = json!({"request_id":id,"operation_url":format!("/v1/operations/{id}"),"retention_seconds":600,"state":"pending"});
        let (completion, _) = tokio::sync::watch::channel(false);
        records.insert(
            id.into(),
            Record {
                fingerprint,
                started: Instant::now(),
                receipt,
                state: "pending",
                result: None,
                error: None,
                reply: None,
                completion,
            },
        );
        Ok(true)
    }
    pub fn receipt(&self, id: &str) -> Value {
        self.records.lock().unwrap()[id].receipt.clone()
    }
    pub async fn reply(&self, id: &str) -> Result<(axum::http::StatusCode, Value)> {
        let mut updates = self.records.lock().unwrap()[id].completion.subscribe();
        loop {
            if let Some(reply) = self.records.lock().unwrap()[id].reply.clone() {
                return Ok(reply);
            }
            updates.changed().await.map_err(|_| Error::unavailable())?;
        }
    }
    pub fn finish(&self, id: &str, result: Result<Value>, asynchronous: bool) {
        let mut records = self.records.lock().unwrap();
        let r = records.get_mut(id).unwrap();
        match result {
            Ok(v) => {
                r.state = "succeeded";
                r.result = Some(v.clone());
                if !asynchronous {
                    let mut receipt = v;
                    receipt["request_id"] = json!(id);
                    receipt["operation_url"] = json!(format!("/v1/operations/{id}"));
                    receipt["retention_seconds"] = json!(600);
                    r.reply = Some((axum::http::StatusCode::ACCEPTED, receipt));
                }
            }
            Err(e) => {
                r.state = if matches!(e.code, "delivery_unknown" | "launch_delivery_unknown") {
                    "unknown"
                } else {
                    "failed"
                };
                r.error = Some(e.code);
                if !asynchronous {
                    r.reply = Some((e.status, e.body()));
                }
            }
        }
        r.completion.send_replace(true);
    }
    pub fn operation(&self, id: &str) -> Result<Value> {
        let records = self.records.lock().unwrap();
        let r = records
            .get(id)
            .filter(|r| r.state == "pending" || r.started.elapsed() < Duration::from_secs(600))
            .ok_or_else(|| Error::new(axum::http::StatusCode::NOT_FOUND, "operation_expired"))?;
        Ok(
            json!({"request_id":id,"state":r.state,"result":r.result,"error":r.error.map(|code|json!({"code":code,"message":"Inspect the computer before making another mutation."})),"retention_seconds":600}),
        )
    }
}
