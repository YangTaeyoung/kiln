use super::controls::*;
// Durable, provider-neutral conversation state. Only envelopes mutate the transcript.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
    #[serde(default)]
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Draft {
    #[serde(default)]
    pub skills: Vec<NativeSkillInput>,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub model: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
    Cancel,
    Option(String),
    Grant(PermissionGrant),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Submit {
        text: String,
        attachments: Vec<Attachment>,
        model: Option<String>,
    },
    SubmitWithSkills {
        text: String,
        attachments: Vec<Attachment>,
        model: Option<String>,
        skills: Vec<NativeSkillInput>,
    },
    Respond {
        request_id: String,
        decision: Decision,
        answers: BTreeMap<String, Vec<String>>,
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
    /// Exclusive host-owned handoff; never forwarded to a provider adapter.
    SetPresentation(super::Presentation),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedCommand {
    pub id: String,
    pub generation: String,
    pub command: Command,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Lifecycle {
    #[default]
    Starting,
    Ready,
    Running,
    Waiting,
    Disconnected,
    Failed,
    Stopped,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub role: Role,
    pub text: String,
    pub attachments: Vec<Attachment>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandPhase {
    Queued,
    Dispatched,
    Accepted,
    Rejected,
    Unconfirmed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<ToolPresentation>,
    pub id: String,
    pub name: String,
    pub input: Value,
    pub output: String,
    pub diff: String,
    pub status: ToolStatus,
}
/// Optional native display metadata; never mixed into execution arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolPresentation {
    pub kind: Option<String>,
    pub title: Option<String>,
    pub native_name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Entry {
    Message(Message),
    Tool(ToolCall),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub description: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub id: String,
    pub text: String,
    pub options: Vec<Choice>,
    pub multi_select: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: String,
    pub title: String,
    pub questions: Vec<Question>,
    pub options: Vec<Choice>,
    pub details: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Event {
    Generation,
    Presentation {
        current: super::Presentation,
        return_requested: bool,
    },
    Controls(ControlEvent),
    Lifecycle(Lifecycle),
    Session(String),
    Models {
        choices: Vec<Model>,
        selected: Option<String>,
    },
    Modes {
        choices: Vec<Choice>,
        selected: Option<String>,
    },
    Message {
        message: Message,
        append: bool,
    },
    Tool(ToolCall),
    Request(Request),
    RequestResolved(String),
    CommandState {
        id: String,
        state: CommandPhase,
    },
    Error(String),
    /// Client health overlay; sequence zero never advances or replaces journal history.
    RuntimeUnavailable {
        reason: String,
        rejected: Vec<String>,
    },
    EnqueueFailed {
        id: String,
        uncertain: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub sequence: u64,
    pub generation: String,
    pub event: Event,
}
#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub generation: String,
    pub controls: NativeControls,
    pub sequence: u64,
    pub lifecycle: Lifecycle,
    pub entries: Vec<Entry>,
    pub requests: BTreeMap<String, Request>,
    pub modes: Vec<Choice>,
    pub selected_mode: Option<String>,
    pub models: Vec<Model>,
    pub selected_model: Option<String>,
    pub provider_session_id: Option<String>,
    pub errors: Vec<String>,
    pub commands: BTreeMap<String, CommandPhase>,
    pub uncertain_deliveries: BTreeSet<String>,
    pub presentation: super::Presentation,
    pub return_requested: bool,
}
impl Transcript {
    fn invalidate_pending(&mut self) {
        self.lifecycle = Lifecycle::Failed;
        self.requests.clear();
        for state in self.commands.values_mut() {
            if matches!(state, CommandPhase::Queued | CommandPhase::Dispatched) {
                *state = CommandPhase::Unconfirmed;
            }
        }
        for entry in &mut self.entries {
            if let Entry::Tool(tool) = entry
                && matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
            {
                tool.status = ToolStatus::Failed;
            }
        }
    }
    pub fn apply(&mut self, envelope: &Envelope) {
        if envelope.sequence == 0
            && let Event::EnqueueFailed { id, uncertain } = &envelope.event
        {
            if !self.commands.contains_key(id) {
                self.commands.insert(
                    id.clone(),
                    if *uncertain {
                        CommandPhase::Unconfirmed
                    } else {
                        CommandPhase::Rejected
                    },
                );
                if *uncertain {
                    self.uncertain_deliveries.insert(id.clone());
                }
            }
            return;
        }
        if envelope.sequence == 0
            && let Event::RuntimeUnavailable { reason, rejected } = &envelope.event
        {
            self.invalidate_pending();
            for id in rejected {
                self.commands
                    .entry(id.clone())
                    .or_insert(CommandPhase::Rejected);
            }
            if !self.errors.contains(reason) {
                self.errors.push(reason.clone());
            }
            return;
        }
        if envelope.sequence <= self.sequence {
            return;
        }
        if matches!(envelope.event, Event::Generation) {
            self.uncertain_deliveries.clear();
            self.generation = envelope.generation.clone();
            self.requests.clear();
            for state in self.commands.values_mut() {
                if matches!(state, CommandPhase::Dispatched | CommandPhase::Queued) {
                    *state = CommandPhase::Unconfirmed;
                }
            }
            self.lifecycle = Lifecycle::Starting;
        } else if envelope.generation != self.generation {
            return;
        }
        self.sequence = envelope.sequence;
        match &envelope.event {
            Event::Presentation { current, return_requested } => {
                if *current == super::Presentation::Terminal && self.presentation != *current {
                    self.errors.retain(|error| super::handoff_error(error).is_none());
                }
                self.presentation = *current;
                self.return_requested = *return_requested;
            }
            Event::Generation => {
                self.controls = NativeControls::default();
            }
            Event::Controls(event) => {
                if let ControlEvent::Quick {
                    models,
                    selected_model,
                    modes,
                    selected_mode,
                } = event
                {
                    self.models = models.clone();
                    self.selected_model = selected_model.clone();
                    self.modes = modes.clone();
                    self.selected_mode = selected_mode.clone();
                }
                if let ControlEvent::ModelSnapshot { choices, selected } = event {
                    self.models = choices.clone();
                    self.selected_model = selected.clone();
                }
                if let ControlEvent::ModeSnapshot { choices, selected } = event {
                    self.modes = choices.clone();
                    self.selected_mode = selected.clone();
                }
                self.controls.apply(event);
            }
            Event::Lifecycle(state) => {
                self.lifecycle = state.clone();
                if matches!(
                    state,
                    Lifecycle::Disconnected
                        | Lifecycle::Failed
                        | Lifecycle::Stopped
                        | Lifecycle::Ready
                ) {
                    self.requests.clear();
                }
                if matches!(state, Lifecycle::Failed) {
                    self.invalidate_pending();
                }
            }
            Event::Session(id) => self.provider_session_id = Some(id.clone()),
            Event::Models { choices, selected } => {
                if !choices.is_empty() {
                    self.models = choices.clone();
                }
                self.selected_model = selected.clone();
            }
            Event::Modes { choices, selected } => {
                if !choices.is_empty() {
                    self.modes = choices.clone();
                }
                self.selected_mode = selected.clone();
            }
            Event::Message { message, append } => {
                if let Some(Entry::Message(current)) = self
                    .entries
                    .iter_mut()
                    .find(|item| matches!(item,Entry::Message(m) if m.id==message.id))
                {
                    if *append {
                        current.text.push_str(&message.text);
                    } else {
                        *current = message.clone();
                    }
                } else {
                    self.entries.push(Entry::Message(message.clone()));
                }
            }
            Event::Tool(tool) => {
                if let Some(Entry::Tool(current)) = self
                    .entries
                    .iter_mut()
                    .find(|item| matches!(item,Entry::Tool(t) if t.id==tool.id))
                {
                    *current = tool.clone();
                } else {
                    self.entries.push(Entry::Tool(tool.clone()));
                }
            }
            Event::Request(request) => {
                self.requests.insert(request.id.clone(), request.clone());
            }
            Event::RequestResolved(id) => {
                self.requests.remove(id);
            }
            Event::CommandState { id, state } => {
                self.uncertain_deliveries.remove(id);
                self.commands.insert(id.clone(), state.clone());
            }
            Event::Error(error) => self.errors.push(error.clone()),
            Event::RuntimeUnavailable { .. } | Event::EnqueueFailed { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enqueue_failure_overlay_never_replaces_authoritative_command_outcome() {
        let mut newer = Transcript::default();
        newer.apply(&e(1, "new", Event::Generation));
        newer.apply(&e(
            0,
            "old",
            Event::EnqueueFailed {
                id: "finished-old-writer".into(),
                uncertain: false,
            },
        ));
        assert_eq!(
            newer.commands.get("finished-old-writer"),
            Some(&CommandPhase::Rejected)
        );
        assert_eq!(newer.sequence, 1);
        for before in [true, false] {
            for uncertain in [true, false] {
                let mut transcript = Transcript::default();
                transcript.apply(&e(1, "g", Event::Generation));
                let accepted = e(
                    2,
                    "g",
                    Event::CommandState {
                        id: "command".into(),
                        state: CommandPhase::Accepted,
                    },
                );
                let local = e(
                    0,
                    "g",
                    Event::EnqueueFailed {
                        id: "command".into(),
                        uncertain,
                    },
                );
                if before {
                    transcript.apply(&accepted);
                    transcript.apply(&local);
                } else {
                    transcript.apply(&local);
                    transcript.apply(&accepted);
                }
                assert_eq!(
                    transcript.commands.get("command"),
                    Some(&CommandPhase::Accepted)
                );
                assert!(transcript.uncertain_deliveries.is_empty());
                assert_eq!(transcript.sequence, 2);
            }
        }
    }
    fn e(sequence: u64, generation: &str, event: Event) -> Envelope {
        Envelope {
            sequence,
            generation: generation.into(),
            event,
        }
    }
    #[test]
    fn envelopes_are_exact_once_and_stream_messages_upsert() {
        let mut t = Transcript::default();
        t.apply(&e(1, "a", Event::Generation));
        let m = |s: &str| Message {
            id: "text".into(),
            role: Role::Assistant,
            text: s.into(),
            attachments: vec![],
        };
        let delta = e(
            2,
            "a",
            Event::Message {
                message: m("안녕"),
                append: true,
            },
        );
        t.apply(&delta);
        t.apply(&delta);
        t.apply(&e(
            3,
            "a",
            Event::Message {
                message: m("안녕하세요"),
                append: false,
            },
        ));
        assert_eq!(t.entries, vec![Entry::Message(m("안녕하세요"))]);
    }
    #[test]
    fn generation_invalidates_requests_but_keeps_native_history() {
        let mut t = Transcript::default();
        t.apply(&e(1, "a", Event::Generation));
        t.apply(&e(2, "a", Event::Session("native-1".into())));
        let tool = ToolCall {
            id: "tool1".into(),
            name: "Read".into(),
            output: "contents".into(),
            ..Default::default()
        };
        t.apply(&e(3, "a", Event::Tool(tool.clone())));
        t.apply(&e(
            4,
            "a",
            Event::Request(Request {
                id: "approve".into(),
                title: "Read".into(),
                questions: vec![],
                options: vec![],
                details: Value::Null,
            }),
        ));
        t.apply(&e(5, "b", Event::Generation));
        t.apply(&e(6, "a", Event::Lifecycle(Lifecycle::Running)));
        assert!(t.requests.is_empty());
        assert_eq!(t.provider_session_id.as_deref(), Some("native-1"));
        assert_eq!(t.entries, vec![Entry::Tool(tool)]);
        assert_ne!(t.lifecycle, Lifecycle::Running);
    }
    #[test]
    fn successful_terminal_entry_clears_old_handoff_notice_but_failed_import_remains_visible() {
        let mut t = Transcript::default();
        t.apply(&e(1,"a",Event::Generation));
        t.apply(&e(2,"a",Event::Error("provider error".into())));
        t.apply(&e(3,"a",Event::Error("KILN_HANDOFF_HELPERS".into())));
        t.apply(&e(4,"a",Event::Presentation{current:super::super::Presentation::Terminal,return_requested:false}));
        assert_eq!(t.errors,["provider error"]);
        t.apply(&e(5,"a",Event::Error("KILN_HANDOFF_HISTORY".into())));
        t.apply(&e(6,"a",Event::Presentation{current:super::super::Presentation::Gui,return_requested:false}));
        assert_eq!(t.errors,["provider error","KILN_HANDOFF_HISTORY"]);
    }
    #[test]
    fn restoring_a_draft_cannot_submit_a_turn() {
        let draft = Draft {
            text: "do this later".into(),
            ..Default::default()
        };
        let restored: Draft = serde_json::from_slice(&serde_json::to_vec(&draft).unwrap()).unwrap();
        assert_eq!(restored, draft);
        assert!(Transcript::default().entries.is_empty());
    }
}
