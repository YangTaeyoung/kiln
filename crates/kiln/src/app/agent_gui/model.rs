//! Durable, provider-neutral conversation state. Only envelopes mutate the transcript.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

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
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Submit {
        text: String,
        attachments: Vec<Attachment>,
        model: Option<String>,
    },
    Respond {
        request_id: String,
        decision: Decision,
        answers: BTreeMap<String, Vec<String>>,
    },
    SetMode {
        mode: String,
    },
    Interrupt,
    Restart,
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
    pub id: String,
    pub name: String,
    pub input: Value,
    pub output: String,
    pub diff: String,
    pub status: ToolStatus,
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
}
impl Transcript {
    pub fn apply(&mut self, envelope: &Envelope) {
        if envelope.sequence <= self.sequence {
            return;
        }
        if matches!(envelope.event, Event::Generation) {
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
            Event::Generation => {}
            Event::Lifecycle(state) => {
                self.lifecycle = state.clone();
                if matches!(
                    state,
                    Lifecycle::Disconnected | Lifecycle::Stopped | Lifecycle::Ready
                ) {
                    self.requests.clear();
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
                self.commands.insert(id.clone(), state.clone());
            }
            Event::Error(error) => self.errors.push(error.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
