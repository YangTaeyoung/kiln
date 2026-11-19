//! Native conversation presentation. The daemon owns the worker; this owns only a client.
use super::*;
use egui::{Frame, Margin, RichText, Stroke, Ui};
use egui::emath::GuiRounding;
use kiln_common::{
    Theme,
    task::Task,
    widgets::{self, ButtonKind},
};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
#[path = "settings_ui.rs"]
mod settings_ui;
#[path = "commands_ui.rs"]
mod commands_ui;
#[path = "actions_ui.rs"]
mod actions_ui;
#[path = "launch_ui.rs"]
mod launch_ui;
#[path = "editing_text.rs"]
mod editing_text;
#[path = "mentions_ui.rs"]
mod mentions_ui;
use settings_ui::{ControlsUi, Surface};
#[path = "tool_code_ui.rs"]
mod tool_code_ui;
#[path = "attachment_ui.rs"]
mod attachment_ui;
#[path = "activity_ui.rs"]
mod activity_ui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presentation {
    #[default]
    Gui,
    Terminal,
}
impl Presentation {
    /// One joined control, used inside each agent canvas rather than its frame header.
    pub fn switch_ui(self, ui: &mut Ui, enabled: [bool; 2]) -> Option<Self> {
        let t = Theme::current();
        let (track, _) = ui.allocate_exact_size(egui::vec2(112., 28.), egui::Sense::hover());
        ui.painter().rect_filled(track, 6, t.bg_input);
        let mut chosen = None;
        for (index, (value, label)) in [(Self::Gui, "GUI"), (Self::Terminal, "TUI")].into_iter().enumerate() {
            let rect = egui::Rect::from_min_max(
                track.min + egui::vec2(index as f32 * 56., 0.),
                track.min + egui::vec2((index + 1) as f32 * 56., 28.),
            );
            let response = ui.add_enabled_ui(enabled[index], |ui| {
                let response = ui.interact(rect, ui.id().with(("agent-presentation", index)), egui::Sense::click());
                response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), self == value, label));
                response
            }).inner;
            let selected = self == value;
            if selected || (enabled[index] && response.hovered()) {
                let radius = if index == 0 {
                    egui::CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }
                } else {
                    egui::CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }
                };
                ui.painter().rect_filled(rect.shrink(1.), radius, if selected {t.bg_selected} else {t.bg_hover});
            }
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, kiln_common::fonts::medium(12.),
                if selected {t.text} else if enabled[index] {t.text_dim} else {t.text_faint});
            widgets::focus_ring(ui, &response, 5);
            let response = response.on_disabled_hover_text(kiln_common::i18n::tr("응답과 승인 요청이 끝나고 대화 연결이 확인되면 전환할 수 있습니다."));
            if enabled[index] && response.clicked() && !selected {chosen = Some(value);}
        }
        ui.painter().line_segment([track.center_top(), track.center_bottom()], Stroke::new(1., t.border));
        ui.painter().rect_stroke(track, 6, Stroke::new(1., t.border_strong), egui::StrokeKind::Inside);
        chosen
    }
    pub fn ui(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        let (track, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(180.), 32.),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(track, 8, if t.dark { t.bg_input } else { t.bg_hover });
        for (index, (value, label)) in [
            (Self::Gui, kiln_common::i18n::tr("대화 GUI")),
            (Self::Terminal, kiln_common::i18n::tr("터미널")),
        ]
        .into_iter()
        .enumerate()
        {
            let width = (track.width() - 4.) / 2.;
            let rect = egui::Rect::from_min_size(
                track.min + egui::vec2(2. + index as f32 * width, 2.),
                egui::vec2(width, 28.),
            );
            let response = ui.interact(
                rect,
                ui.id().with(("presentation", index)),
                egui::Sense::click(),
            );
            let selected = *self == value;
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::RadioButton,
                    ui.is_enabled(),
                    selected,
                    label,
                )
            });
            if selected || response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    6,
                    if selected { t.bg_elevated } else { t.bg_hover },
                );
            }
            if selected {
                ui.painter().rect_stroke(rect, 6, Stroke::new(1., t.border_strong), egui::StrokeKind::Inside);
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                kiln_common::fonts::medium(12.5),
                if selected { t.text } else { t.text_dim },
            );
            widgets::focus_ring(ui, &response, 6);
            if response.clicked() {
                *self = value;
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedDraft {
    pub composer: Draft,
    pub pending: Option<PendingDraft>,
    pub recovered: Option<PendingDraft>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingDraft {
    pub id: String,
    pub draft: Draft,
}
#[derive(Clone)]
struct Cached {
    source: String,
    markdown: kiln_git::Markdown,
}
struct PreparedDisplay {
    messages: HashMap<String, Cached>,
    inputs: HashMap<String, String>,
    tools: HashMap<String, tool_code_ui::PreparedTool>,
    theme: Theme,
    details: HashMap<String, (Request, super::approval::Summary)>,
}
const WINDOW: usize = 100;
const MAX_ATTACHMENTS: usize = 8;
const DISPLAY_BYTES: usize = 64 * 1024;
fn bounded(s: &str) -> &str {
    let mut n = s.len().min(DISPLAY_BYTES);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

pub struct AgentPane {
    pub descriptor: Descriptor,
    controls_ui: ControlsUi,
    mentions: mentions_ui::MentionsState,
    local_intents: BTreeSet<String>,
    replay_required: bool,
    launch_request: Option<LaunchOptions>,
    terminal_request: bool,
    client: Option<Client>,
    pub transcript: Transcript,
    backlog: VecDeque<Envelope>,
    composer: Draft,
    dismissed_suggestion: Option<String>,
    pending: Option<PendingDraft>,
    recovered: Option<PendingDraft>,
    restored_pending: bool,
    cache: PreparedDisplay,
    parsing: Option<Task<(u64, usize, PreparedDisplay)>>,
    cached_revision: Option<(u64, usize)>,
    imports: Vec<Task<Result<Option<Attachment>, String>>>,
    image_ui: attachment_ui::ImageUi,
    validation: Option<Task<Result<(), String>>>,
    attachment_error: Option<String>,
    validate_on_tick: bool,
    error: Option<String>,
    answers: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    responding: BTreeMap<String, String>,
    composing: bool,
    committed_during_preedit: Option<String>,
    ime_frame: bool,
    ime_guard_frame: Option<u64>,
    transport_alive: bool,
    detached_shell: bool,
    legacy_return: Option<super::legacy_return::LegacyReturn>,
    legacy_cancel_to_terminal: bool,
    transport_known: bool,
    history_end: Option<usize>,
    activity_ui: activity_ui::ActivityUi,
    focus: bool,
    #[cfg(test)]
    pub(super) sent: Vec<Command>,
    #[cfg(test)]
    composer_geometry: Option<(egui::Rect, egui::Rect)>,
}
impl AgentPane {
    /// Stable across GUI/TUI handoffs and reconnects: use confirmed history,
    /// never the unsent composer or the most recent assistant/tool response.
    pub fn conversation_title(&self) -> String {
        let first = self.transcript.entries.iter().find_map(|entry| match entry {
            Entry::Message(message) if message.role == Role::User && !message.text.trim().is_empty() => Some(message.text.as_str()),
            _ => None,
        });
        if let Some(text) = first {
            let mut title = String::new();
            let mut chars = text.split_whitespace().flat_map(|word| word.chars().chain(std::iter::once(' ')));
            title.extend(chars.by_ref().take(64));
            let more = chars.any(|c| !c.is_whitespace());
            title.truncate(title.trim_end().len());
            if more {title.push('…');}
            return title;
        }
        let folder = self.descriptor.cwd.file_name().map(|name| name.to_string_lossy());
        match folder {
            Some(folder) if !folder.is_empty() => format!("{} · {folder}", kiln_common::i18n::tr("새 대화")),
            _ => kiln_common::i18n::tr("새 대화").into(),
        }
    }
    pub fn from_spec(spec: &SpawnSpec) -> Option<Self> {
        let descriptor = descriptor(spec)?;
        let client = Client::connect(descriptor.clone());
        let mut pane = Self::new(descriptor);
        pane.client = Some(client);
        Some(pane)
    }
    fn new(descriptor: Descriptor) -> Self {
        Self {
            descriptor,
            controls_ui: ControlsUi::default(),
            mentions: Default::default(),
            local_intents: BTreeSet::new(),
            replay_required: true,
            launch_request: None,
            terminal_request: false,
            client: None,
            transcript: Transcript::default(),
            backlog: VecDeque::new(),
            composer: Draft::default(),
            dismissed_suggestion: None,
            pending: None,
            recovered: None,
            restored_pending: false,
            cache: PreparedDisplay {
                messages: HashMap::new(),
                inputs: HashMap::new(),
                tools: HashMap::new(),
                theme: Theme::current(),
                details: HashMap::new(),
            },
            parsing: None,
            cached_revision: None,
            imports: vec![],
            image_ui: attachment_ui::ImageUi::default(),
            validation: None,
            attachment_error: None,
            validate_on_tick: false,
            error: None,
            answers: BTreeMap::new(),
            responding: BTreeMap::new(),
            composing: false,
            committed_during_preedit: None,
            ime_frame: false,
            ime_guard_frame: None,
            transport_alive: false,
            detached_shell: false,
            legacy_return: None,
            legacy_cancel_to_terminal: false,
            transport_known: false,
            history_end: None,
            activity_ui: Default::default(),
            focus: true,
            #[cfg(test)]
            sent: vec![],
            #[cfg(test)]
            composer_geometry: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture(descriptor: Descriptor) -> Self {
        let mut p = Self::new(descriptor);
        p.replay_required = false;
        p.transport_alive = true;
        p.transport_known = true;
        p
    }
    pub fn mark_shell_detached(&mut self) {
        self.detached_shell = true;
        self.set_transport_alive(false);
    }
    pub fn mark_shell_attached(&mut self) { self.detached_shell = false; }
    pub fn shell_detached(&self) -> bool { self.detached_shell }
    fn quiet_worker_error(&self, error: &str) -> bool {
        self.detached_shell && self.pending.is_none() && self.recovered.is_none()
            && error == "Agent worker is unavailable. Pending submissions are unconfirmed and will not be replayed."
    }
    /// Only a proven-dead shell host can use a local, read-only view toggle.
    pub fn preview_shell_presentation(&mut self, target: Presentation) {
        if self.detached_shell { self.transcript.presentation = target; self.transcript.return_requested = false; }
    }
    pub fn presentation(&self) -> Presentation { self.transcript.presentation }
    pub fn legacy_return_error(&self) -> Option<&str> { self.legacy_return.as_ref().and_then(|bridge|bridge.error()) }
    pub fn legacy_return_label(&self) -> Option<&'static str> { self.legacy_return.as_ref().map(|bridge|bridge.label()) }
    pub fn start_legacy_return(&mut self, ctx: &egui::Context, session: u64, pid: u32, socket: String) {
        if !matches!(self.descriptor.tool, Tool::Claude | Tool::Codex) || self.transcript.controls.status.contains_key("native_terminal_return") || self.legacy_return.is_some() { return; }
        if let Some(conversation) = self.transcript.provider_session_id.clone() {
            self.legacy_return = Some(super::legacy_return::LegacyReturn::start(ctx, super::legacy_return::Source::Gui {worker:self.descriptor.id.clone(),conversation},session,pid,socket));
        }
    }
    pub fn defer_terminal_after_native_exit(&mut self) { self.legacy_cancel_to_terminal = true; }
    pub fn automatic_terminal_return(&self) -> bool {
        self.transcript.controls.status.get("native_terminal_return").and_then(|status|status.fields.get("automatic")) == Some(&ControlValue::Bool(true))
    }
    fn prompt_suggestion(&self) -> Option<(&str, &str)> {
        if self.transcript.lifecycle != Lifecycle::Ready || self.pending.is_some()
            || !self.composer.text.is_empty() || !self.composer.attachments.is_empty()
            || self.composing || self.ime_frame { return None; }
        let status = self.transcript.controls.status.get("prompt_suggestion")?;
        let ControlValue::Text(id) = status.fields.get("id")? else {return None;};
        let ControlValue::Text(text) = status.fields.get("text")? else {return None;};
        if self.dismissed_suggestion.as_deref() == Some(id) || text.is_empty() {return None;}
        Some((id, text))
    }

    pub fn presentation_pending(&self) -> bool {
        !self.local_intents.is_empty() || self.legacy_return.as_ref().is_some_and(|bridge|bridge.cancel_requested())
    }
    pub fn selected_presentation(&self) -> Presentation {
        if self.transcript.return_requested {
            match self.presentation() { Presentation::Gui => Presentation::Terminal, Presentation::Terminal => Presentation::Gui }
        } else { self.presentation() }
    }
    pub fn can_switch_presentation(&self, target: Presentation) -> bool {
        if self.presentation_pending() || self.selected_presentation() == target
            || !self.transport_alive || self.replay_required { return false; }
        // Selecting the owning view cancels a deferred handoff, without touching its process.
        if self.presentation() == target { return true; }
        if target == Presentation::Terminal {
            self.live_input_allowed() && !self.settings_blocked() && self.pending.is_none()
                && self.transcript.provider_session_id.is_some()
                && !self.image_import_pending() && self.validation.is_none()
        } else { self.transcript.provider_session_id.is_some() }
    }
    pub fn request_presentation(&mut self, target: Presentation) -> bool {
        if !self.can_switch_presentation(target) { return false; }
        if target == Presentation::Terminal {
            if let Some(bridge) = &self.legacy_return {
                // Keep the old host's GUI takeover armed until native cancellation
                // is acknowledged. A committed native exit must still be adopted.
                bridge.request_cancel();
                self.legacy_cancel_to_terminal = true;
                return true;
            }
        }
        match self.send(Command::SetPresentation(target)) { Ok(_) => true, Err(error) => { self.error = Some(error); false } }
    }
    pub fn draft(&self) -> SavedDraft {
        let mut composer = self.composer.clone();
        if let Some(text) = &self.committed_during_preedit {
            composer.text = text.clone();
        }
        SavedDraft {
            composer,
            pending: self.pending.clone(),
            recovered: self.recovered.clone(),
        }
    }
    pub fn restore_draft(&mut self, mut saved: SavedDraft) {
        saved.composer.attachments.truncate(MAX_ATTACHMENTS);
        if let Some(pending) = &mut saved.pending {
            pending.draft.attachments.truncate(MAX_ATTACHMENTS);
        }
        if let Some(recovered) = &mut saved.recovered {
            recovered.draft.attachments.truncate(MAX_ATTACHMENTS);
        }
        self.replay_required = true;
        if let Some(client) = &self.client { client.invalidate_replay(); }
        self.composer = saved.composer;
        self.restored_pending = saved.pending.is_some();
        self.pending = saved.pending;
        self.recovered = saved.recovered;
        // Validation is scheduled by tick; restore never sends or reads files.
        self.attachment_error = None;
        self.validate_on_tick = true;
    }
    pub fn importing(&self) -> bool {
        self.image_import_pending()
    }
    pub fn has_draft(&self) -> bool {
        !self.composer.text.is_empty()
            || !self.composer.attachments.is_empty()
            || self.pending.is_some()
            || self.recovered.is_some()
            || self.image_import_pending()
    }
    pub fn set_transport_alive(&mut self, alive: bool) {
        if !alive && self.transport_alive { self.require_replay(); }
        self.transport_alive = alive;
        self.transport_known = true;
    }
    fn require_replay(&mut self) {
        self.replay_required = true;
        if let Some(client) = &self.client { client.invalidate_replay(); }
    }
    pub fn transport_unverified(&mut self) {
        if self.transport_alive { self.require_replay(); }
        self.transport_alive = false;
    }
    pub fn lifecycle(&self) -> Lifecycle {
        if !self.transport_known {
            Lifecycle::Starting
        } else if self.transport_alive {
            self.transcript.lifecycle.clone()
        } else {
            Lifecycle::Disconnected
        }
    }
    pub fn busy(&self) -> bool {
        self.transport_alive
            && (self.presentation() == Presentation::Terminal || matches!(
                self.transcript.lifecycle,
                Lifecycle::Running | Lifecycle::Waiting
            ))
    }
    pub fn status(&self) -> &'static str {
        kiln_common::i18n::tr(match self.lifecycle() {
            Lifecycle::Starting => "시작하는 중…",
            Lifecycle::Ready => "준비됨",
            Lifecycle::Running => "실행 중",
            Lifecycle::Waiting => "입력 필요",
            Lifecycle::Disconnected => "연결 끊김",
            Lifecycle::Failed => "실패",
            Lifecycle::Stopped => "중지됨",
        })
    }
    pub fn request_focus(&mut self) {
        self.focus = true;
    }
    fn send(&mut self, command: Command) -> Result<String, String> {
        let serial = matches!(command, Command::Configure { .. } | Command::SetMode { .. } | Command::NativeAction(_) | Command::SetPresentation(_));
        let result = self.send_transport(command);
        if serial && let Ok(id) = &result { self.local_intents.insert(id.clone()); }
        result
    }
    fn send_transport(&mut self, command: Command) -> Result<String, String> {
        if let Some(client) = &self.client {
            return client
                .send_for_generation(&self.transcript.generation, command)
                .map_err(|e| e.to_string());
        }
        #[cfg(test)]
        {
            self.sent.push(command);
            return Ok(format!("test-{}", self.sent.len()));
        }
        #[cfg(not(test))]
        {
            let _ = command;
            Err(kiln_common::i18n::tr("연결 끊김").into())
        }
    }
    fn apply(&mut self, envelope: &Envelope) {
        if matches!(envelope.event, Event::Generation) {
            self.local_intents.clear();
            self.answers.clear();
            self.controls_ui.grants.clear();
            self.responding.clear();
        }
        if let Event::Controls(ControlEvent::Options(options)) = &envelope.event {
            self.controls_ui.edits.retain(|id, edit| {
                options.iter().any(|option| {
                    option.id == *id && edit.compatible(option) && edit.dirty()
                })
            });
        }
        self.transcript.apply(envelope);
        self.reconcile();
    }
    fn reconcile(&mut self) {
        // Host journals native Pending before transport Accepted. Accepted releases
        // only local intent; native settled=false remains an independent barrier.
        self.local_intents.retain(|id| match self.transcript.commands.get(id) {
            Some(CommandPhase::Accepted | CommandPhase::Rejected) => false,
            Some(CommandPhase::Unconfirmed) => {
                // The enqueue error remains visible, but a completed writer plus
                // a consistent absent mailbox permits explicit user retry. Keep
                // the host phase intact so a later authoritative ACK still wins.
                !self.transcript.uncertain_deliveries.contains(id)
                    || !self.backlog.is_empty()
                    || !self.client.as_ref().is_some_and(|client| {
                        client.submission_is_unknown(
                            id,
                            &self.transcript.generation,
                            self.transcript.sequence,
                        )
                    })
            }
            _ => true,
        });
        if self.replay_required && self.backlog.is_empty()
            && self.client.as_ref().is_some_and(|c| c.replay_current(&self.transcript.generation, self.transcript.sequence)) {
            self.replay_required = false;
        }
        if self.transcript.controls.options_reported {
            self.composer.model = None; // Detailed native settings own the selection.
        }
        if let Some(p) = &self.pending {
            let confirmed = self.transcript.commands.get(&p.id) == Some(&CommandPhase::Accepted);
            if confirmed {
                self.pending = None;
                self.restored_pending = false;
            }
        }
        if let Some(recovered) = &self.recovered {
            let accepted = self.transcript.commands.get(&recovered.id)
                == Some(&CommandPhase::Accepted);
            if accepted {
                if self.composer == recovered.draft {
                    self.composer = Draft::default();
                }
                self.error = Some(
                    kiln_common::i18n::tr(
                        "원래 요청의 전송이 확인되었습니다. 대화 기록을 확인하세요.",
                    )
                    .into(),
                );
                self.recovered = None;
            }
        }
        self.controls_ui.grants.retain(|id,_|self.transcript.requests.contains_key(id));
        self.answers
            .retain(|id, _| self.transcript.requests.contains_key(id));
        self.responding.retain(|id, command| {
            self.transcript.requests.contains_key(id)
                && match self.transcript.commands.get(command) {
                    Some(CommandPhase::Rejected) => false,
                    Some(CommandPhase::Unconfirmed) => {
                        self.transcript.uncertain_deliveries.contains(command)
                            && !self.client.as_ref().is_some_and(|client| {
                                client.submission_is_unknown(
                                    command,
                                    &self.transcript.generation,
                                    self.transcript.sequence,
                                )
                            })
                    }
                    _ => true,
                }
        });
    }
    fn pending_recoverable(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            (match self.transcript.commands.get(&p.id) {
                Some(CommandPhase::Rejected) => true,
                Some(CommandPhase::Unconfirmed) => {
                    !self.transcript.uncertain_deliveries.contains(&p.id)
                        || self.client.as_ref().is_some_and(|client| {
                            client.submission_is_unknown(
                                &p.id,
                                &self.transcript.generation,
                                self.transcript.sequence,
                            )
                        })
                }
                _ => false,
            }) || (self.restored_pending
                && self.backlog.is_empty()
                && !self.transcript.commands.contains_key(&p.id)
                && self.client.as_ref().is_some_and(|client| {
                    client.submission_is_unknown(
                        &p.id,
                        &self.transcript.generation,
                        self.transcript.sequence,
                    )
                }))
                || (self.transport_known && !self.transport_alive)
                || matches!(
                    self.transcript.lifecycle,
                    Lifecycle::Failed | Lifecycle::Disconnected | Lifecycle::Stopped
                )
        })
    }
    fn live_input_allowed(&self) -> bool {
        self.transport_alive
            && self.presentation() == Presentation::Gui
            && matches!(self.transcript.lifecycle, Lifecycle::Ready | Lifecycle::Running | Lifecycle::Waiting)
    }
    fn submit(&mut self) {
        if self.composing
            || self.ime_frame
            || self.submit_settings_blocked()
            || !self.transport_alive
            || !self.live_input_allowed()
            || self.pending.is_some()
            || !self.backlog.is_empty()
            || self.validation.is_some()
            || self.attachment_error.is_some()
            || self.image_import_pending()
            || self
                .composer
                .model
                .as_ref()
                .is_some_and(|id| !self.transcript.models.iter().any(|m| &m.id == id))
            || (self.composer.text.trim().is_empty() && self.composer.attachments.is_empty())
        {
            return;
        }
        let draft = self.composer.clone();
        let command = if draft.skills.is_empty() {
            Command::Submit { text: draft.text.clone(), attachments: draft.attachments.clone(), model: draft.model.clone() }
        } else {
            Command::SubmitWithSkills { text: draft.text.clone(), attachments: draft.attachments.clone(), model: draft.model.clone(), skills: draft.skills.clone() }
        };
        match self.send(command) {
            Ok(id) => {
                self.pending = Some(PendingDraft { id, draft });
                self.restored_pending = false;
                self.recovered = None;
                self.composer.text.clear();
                self.composer.attachments.clear();
                self.composer.skills.clear();
                self.cached_revision = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn restart(&mut self) -> bool {
        if !self.transport_alive {
            return true;
        }
        match self.send(Command::Restart) {
            Ok(_) => {}
            Err(error) => self.error = Some(error),
        }
        false
    }
    /// Poll all panes, including hidden panes. Background jobs bound disk/parse work.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.validate_on_tick {
            self.validate_on_tick = false;
            self.validate_attachments(ctx);
        }
        let mut error = None;
        if let Some(client) = &mut self.client {
            if self.backlog.is_empty() {
                self.backlog.extend(client.drain());
            }
            error = client.take_error();
        }
        let since = std::time::Instant::now();
        for _ in 0..32 {
            let Some(event) = self.backlog.pop_front() else {
                break;
            };
            self.apply(&event);
            if since.elapsed() > std::time::Duration::from_millis(2) {
                break;
            }
        }
        if let Some(error) = error {
            self.error = Some(error);
        }
        self.reconcile();
        if self.presentation() == Presentation::Gui { self.legacy_return = None; }
        else if let Some(bridge) = &mut self.legacy_return {
            if let Some(error) = bridge.take_error() { self.error=Some(error); }
            if bridge.cancelled() {
                self.legacy_return = None;
                self.legacy_cancel_to_terminal = false;
                if let Err(error) = self.send(Command::SetPresentation(Presentation::Terminal)) { self.error=Some(error); }
            }
        }
        if self.legacy_cancel_to_terminal && self.presentation() == Presentation::Gui && self.can_switch_presentation(Presentation::Terminal) {
            self.legacy_cancel_to_terminal = false;
            self.request_presentation(Presentation::Terminal);
        }
        self.poll_image_picker(ctx);
        let mut i = 0;
        while i < self.imports.len() {
            if self.imports[i].is_pending() {
                i += 1;
                continue;
            }
            if let Some(result) = self.imports.remove(i).take() {
                match result {
                    Ok(Some(a)) => {
                        // A recovered draft can fill the composer while an import is running.
                        // Preserve that draft and never exceed the provider attachment bound.
                        if self.composer.attachments.len() >= MAX_ATTACHMENTS {
                            self.image_limit_notice();
                            continue;
                        }
                        self.composer.attachments.push(a);
                        self.validation = None;
                        self.attachment_error = None;
                        self.validate_attachments(ctx);
                    }
                    Ok(None) => {}
                    Err(e) => self.image_import_notice(e),
                }
            }
        }
        self.prune_image_previews();
        if let Some(task) = &mut self.validation
            && !task.is_pending()
        {
            self.attachment_error = task.take().and_then(Result::err);
            self.validation = None;
        }
        let end = self
            .history_end
            .unwrap_or(self.transcript.entries.len())
            .min(self.transcript.entries.len());
        if let Some(task) = &mut self.parsing
            && let Some((revision, end, cache)) = task.take()
        {
            self.cache = cache;
            self.cached_revision = Some((revision, end));
            self.parsing = None;
        }
        if self.parsing.is_none() && (self.cached_revision != Some((self.transcript.sequence, end)) || self.cache.theme != Theme::current()) {
            let entries = self.transcript.entries[end.saturating_sub(WINDOW)..end].to_vec();
            let previous = self.cache.messages.clone();
            let previous_tools = self.cache.tools.clone();
            let revision = self.transcript.sequence;
            let provider = self.descriptor.tool;
            let requests = self
                .transcript
                .requests
                .values()
                .take(8)
                .cloned()
                .collect::<Vec<_>>();
            self.parsing = Some(Task::spawn(ctx, move || {
                let mut display = PreparedDisplay {
                    messages: HashMap::new(),
                    inputs: HashMap::new(),
                    tools: HashMap::new(),
                    theme: Theme::current(),
                    details: HashMap::new(),
                };
                for entry in entries {
                    match entry {
                        Entry::Message(m) => {
                            let source = bounded(&m.text).to_owned();
                            let markdown = previous
                                .get(&m.id)
                                .filter(|cached| cached.source == source)
                                .map(|c| c.markdown.clone())
                                .unwrap_or_else(|| kiln_git::Markdown::parse(&source));
                            display.messages.insert(m.id, Cached { source, markdown });
                        }
                        Entry::Tool(t) => {
                            let prepared = tool_code_ui::PreparedTool::reuse_or_new(previous_tools.get(&t.id), &t, provider);
                            display.tools.insert(t.id.clone(), prepared);
                            let source = super::approval::tool_input(&t, provider);
                            display.inputs.insert(t.id, bounded(&source).to_owned());
                        }
                    }
                }
                for request in requests {
                    let summary = super::approval::project(&request, provider);
                    display
                        .details
                        .insert(request.id.clone(), (request, summary));
                }
                (revision, end, display)
            }));
        }
        if self.busy()
            || self.pending.is_some()
            || self.parsing.is_some()
            || !self.backlog.is_empty()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    fn validate_attachments(&mut self, ctx: &egui::Context) {
        let mut attachments = self.composer.attachments.clone();
        if let Some(p) = &self.pending {
            attachments.extend(p.draft.attachments.clone());
        }
        if attachments.is_empty() {
            self.validation = None;
            self.attachment_error = None;
            return;
        }
        let id = self.descriptor.id.clone();
        self.validation = Some(Task::spawn(ctx, move || {
            for attachment in attachments {
                super::attachment_bytes(&id, &attachment)
                    .map_err(|e| format!("{}: {e}", attachment.name))?;
            }
            Ok(())
        }));
    }
    fn import(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        if self.composer.attachments.len() + self.imports.len() >= MAX_ATTACHMENTS {
            self.image_limit_notice();
            return;
        }
        let descriptor = self.descriptor.clone();
        self.imports.push(Task::spawn(ctx, move || {
            import_attachment(&descriptor, &path).map(Some).map_err(|e| e.to_string())
        }));
    }
    /// Returns true only when a dead PTY needs an explicit daemon respawn.
    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        self.image_drop_ui(ui);
        let t = Theme::current();
        let mut restart = false;
        ui.spacing_mut().item_spacing.y = 10.;
        Frame::new()
            .inner_margin(Margin::symmetric(16, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&self.descriptor.account_label).color(t.text_dim));
                    ui.small(RichText::new(self.status()).color(if self.busy() { t.orange } else { t.text_dim }));
                    if let Some(raw_model) = self.transcript.selected_model.as_deref() {
                    let model = self.transcript.controls.models.iter().find(|m| m.id == raw_model).or_else(|| self.transcript.controls.models.iter().find(|m| m.id != "default" && m.resolved_model.as_deref() == Some(raw_model))).filter(|m| !m.display_name.is_empty()).map(settings_ui::metadata_title).unwrap_or_else(|| settings_ui::model_title(raw_model));
                    let metadata = self.transcript.selected_mode.as_ref().map(|mode| format!("{model} · {}", settings_ui::present_value("mode", &ControlValue::Text(mode.clone())))).unwrap_or(model);
                    ui.small(RichText::new(metadata).color(t.text_dim)).on_hover_text(kiln_common::i18n::tr("적용된 모델"));
                    }
                });
            });
        self.surface_navigation(ui);
        ui.separator();
        let available = ui.available_rect_before_wrap();
        let quick_count = self.transcript.controls.options.iter().filter(|o|o.scope==ControlScope::Live && (matches!(o.id.as_str(),"model"|"effort"|"effortLevel"|"thought_level"|"permissionMode"|"collaborationMode"|"mode"|"ultracode") || matches!(o.category.as_deref(),Some("model"|"thought_level"|"mode")))).count();
        let quick_rows = if available.width() < 440. {quick_count.max(usize::from(!self.transcript.models.is_empty()) + usize::from(!self.transcript.modes.is_empty())).max(1)} else {1};
        let estimated_h = 188. + quick_rows as f32 * 38.
            + if self.pending.is_some() || self.recovered.is_some() {62.}else{0.}
            + if !self.composer.attachments.is_empty() || !self.imports.is_empty(){62.}else{0.}
            + if !self.composer.skills.is_empty(){38.}else{0.};
        let measurement_id = ui.id().with(("composer-height", self.controls_ui.surface as u8, available.width().round() as i32));
        let composer_h = ui.data(|d| d.get_temp::<f32>(measurement_id)).unwrap_or(if self.controls_ui.surface == Surface::Conversation { estimated_h } else { 160. }) + 18.;
        let activity_h = self.activity_height(available.height());
        let composer_top = (available.bottom() - composer_h).max(available.top() + 60.);
        let transcript_rect = egui::Rect::from_min_max(
            available.min,
            egui::pos2(
                available.right(),
                (composer_top - activity_h - if activity_h > 0. {8.}else{0.}).max(available.top()+20.),
            ),
        );
        let mut body = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(transcript_rect)
                .id_salt("transcript"),
        );
        body.set_clip_rect(transcript_rect);
        let jump = self.activity_ui.jump;
        self.activity_ui.jump = false;
        if jump {self.history_end=None;}
        let scroll = egui::ScrollArea::vertical().id_salt(("messages",self.controls_ui.surface as u8));
        let scroll = if jump {scroll.animated(false)} else {scroll};
        let scroll = scroll.stick_to_bottom(self.controls_ui.surface==Surface::Conversation).auto_shrink([false,false]).show(&mut body,|ui|{
            let width = (ui.available_width() - 40.).clamp(0., 900.);
            let inset = (ui.available_width() - width) / 2.;
            ui.horizontal_top(|ui| {
                ui.add_space(inset);
                ui.vertical(|ui| {
                ui.set_width(width);
                if self.controls_ui.surface == Surface::Settings { self.settings_ui(ui); return; }
                if self.controls_ui.surface == Surface::Commands { self.commands_ui(ui); return; }
                let end=self.history_end.unwrap_or(self.transcript.entries.len()).min(self.transcript.entries.len());let start=end.saturating_sub(WINDOW);
                if start > 0 || self.history_end.is_some() {
                ui.horizontal_wrapped(|ui|{
                    if start>0 && ui.button(kiln_common::i18n::tr("이전 메시지")).clicked(){self.history_end=Some(start);}
                    if self.history_end.is_some() && ui.button(kiln_common::i18n::tr("최근 메시지")).clicked(){self.history_end=None;}
                });
                }
                if self.transcript.entries.is_empty(){ui.add_space(26.);ui.label(RichText::new(kiln_common::i18n::tr("새 대화를 시작하세요")).size(20.).strong());ui.label(kiln_common::i18n::tr("작업을 입력하면 선택한 계정의 에이전트가 이 폴더에서 실행합니다."));}
                for entry in &self.transcript.entries[start..end] {match entry {
                    Entry::Message(m)=>{
                        let user=m.role==Role::User;
                        if user {
                            ui.label(RichText::new(kiln_common::i18n::tr("나")).color(t.text_dim).strong());
                        } else { ui.horizontal(|ui| {
                                let icon = match self.descriptor.tool {
                                    Tool::Claude => kiln_common::icons::Icon::Claude,
                                    Tool::Codex => kiln_common::icons::Icon::Codex,
                                    Tool::Antigravity => kiln_common::icons::Icon::Antigravity,
                                };
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(18., 18.), egui::Sense::hover());
                                kiln_common::icons::paint(ui.painter(), rect, icon, super::super::ui::agent_ink(icon, &t));
                            ui.label(RichText::new(self.descriptor.tool.display_name()).color(t.text_dim).strong());
                        }); }
                        let frame=Frame::new().inner_margin(Margin::same(if user{12}else{2})).corner_radius(6).fill(if user{t.bg_elevated}else{egui::Color32::TRANSPARENT});
                        frame.show(ui,|ui|{
                            ui.set_width(ui.available_width());
                            if let Some(c)=self.cache.messages.get(&m.id) && c.source==bounded(&m.text){c.markdown.show(ui);} else {ui.add(egui::Label::new(bounded(&m.text)).wrap());}
                            for a in m.attachments.iter().take(MAX_ATTACHMENTS){ui.label(format!("▧ {}",a.name));}
                            if m.text.len()>DISPLAY_BYTES {ui.small(kiln_common::i18n::tr("긴 메시지는 표시가 제한됩니다. 전체 기록은 보존됩니다."));}
                        });ui.add_space(10.);
                    },
                    Entry::Tool(tool)=>self.tool_ui(ui, tool),
                }}
                let requests=self.transcript.requests.values().take(8).cloned().collect::<Vec<_>>();
                for request in requests {self.request_ui(ui,&request);}
                if let Some(error)=self.transcript.errors.last() && !self.quiet_worker_error(error) { let display = super::handoff_error(error).map(kiln_common::i18n::tr).unwrap_or_else(||bounded(error)); ui.add(egui::Label::new(RichText::new(display).color(t.red)).wrap());}
                if let Some(error)=&self.error && self.transcript.errors.last()!=Some(error) && !self.quiet_worker_error(error) {ui.add(egui::Label::new(RichText::new(bounded(error)).color(t.orange)).wrap());}
                if self.detached_shell {
                    ui.add(egui::Label::new(kiln_common::i18n::tr("에이전트가 종료되어 초안을 보존했습니다. TUI에서 원래 셸을 계속 사용할 수 있습니다.")).wrap());
                } else if matches!(self.lifecycle(),Lifecycle::Failed|Lifecycle::Disconnected|Lifecycle::Stopped) {
                    if ui.button(kiln_common::i18n::tr("같은 대화로 재시작")).clicked(){restart=self.restart();}
                }
                });
            });
            if jump { ui.scroll_to_cursor(Some(egui::Align::BOTTOM)); }
        });
        if self.controls_ui.surface==Surface::Conversation {
            self.activity_ui.away=self.history_end.is_some() || scroll.content_size.y - scroll.inner_rect.height() - scroll.state.offset.y > 36.;
            #[cfg(test)] {self.activity_ui.offset=scroll.state.offset.y;}
            if self.activity_ui.away {
                let jump_rect=egui::Rect::from_min_size(egui::pos2(transcript_rect.center().x-(transcript_rect.width()-20.).min(220.)/2.,transcript_rect.bottom()-36.),egui::vec2((transcript_rect.width()-20.).min(220.),32.));
                let mut overlay=ui.new_child(egui::UiBuilder::new().max_rect(jump_rect).id_salt("jump-latest"));
                if overlay.add_sized(jump_rect.size(),egui::Button::new(kiln_common::i18n::tr("↓ 최신 메시지로 이동")).corner_radius(16)).clicked(){self.activity_ui.jump=true;ui.ctx().request_repaint();}
            }
        }
        let column_width = (available.width() - 40.).clamp(0., 900.);
        let column_left = available.center().x - column_width / 2.;
        let composer_rect = egui::Rect::from_min_max(
            egui::pos2(column_left, composer_top + 8.),
            egui::pos2(column_left + column_width, available.bottom() - 4.),
        ).round_to_pixels(ui.ctx().pixels_per_point());
        if activity_h > 0. {
            let activity_rect=egui::Rect::from_min_max(egui::pos2(column_left,transcript_rect.bottom()+4.),egui::pos2(column_left+column_width,composer_top));
            let mut activity=ui.new_child(egui::UiBuilder::new().max_rect(activity_rect).id_salt("conversation-activity"));
            activity.set_clip_rect(activity_rect);self.activity_ui(&mut activity);
        }
        let mut composer = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(composer_rect)
                .id_salt("composer"),
        );
        composer.set_clip_rect(composer_rect);
        egui::ScrollArea::vertical()
            .id_salt("composer-scroll")
            .auto_shrink([false, false])
            .show(&mut composer, |ui| {
                let frame = self.composer_ui(ui);
                if (composer_h - frame.height() - 18.).abs() > 1. {
                    ui.data_mut(|d| d.insert_temp(measurement_id, frame.height()));
                    ui.ctx().request_repaint();
                }
                #[cfg(test)]
                { self.composer_geometry = Some((frame, ui.clip_rect())); }
                #[cfg(not(test))]
                let _ = frame;
            });
        ui.advance_cursor_after_rect(available);
        restart
    }
    fn tool_ui(&self, ui: &mut Ui, tool: &ToolCall) {
        let t = Theme::current();
        let status = kiln_common::i18n::tr(match tool.status {
            ToolStatus::Pending => "대기 중",
            ToolStatus::Running => "실행 중",
            ToolStatus::Completed => "완료",
            ToolStatus::Failed => "실패",
            ToolStatus::Cancelled => "중지됨",
        });
        let color = match tool.status {
            ToolStatus::Completed => t.green,
            ToolStatus::Failed => t.red,
            ToolStatus::Running | ToolStatus::Pending => t.orange,
            ToolStatus::Cancelled => t.text_dim,
        };
        let title = super::approval::tool_title(tool, self.descriptor.tool);
        Frame::new()
            .stroke(Stroke::new(1., t.border))
            .corner_radius(6)
            .inner_margin(Margin::ZERO)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // Keep native full-width collapse behavior, but share the card's
                // boundary instead of painting a second outlined control inside it.
                ui.visuals_mut().collapsing_header_frame = true;
                let open = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    ui.make_persistent_id(&tool.id),
                    false,
                )
                .is_open();
                let radius = egui::CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: if open { 0 } else { 6 },
                    se: if open { 0 } else { 6 },
                };
                let widgets = &mut ui.visuals_mut().widgets;
                for (visuals, fill) in [
                    (&mut widgets.inactive, t.bg_elevated),
                    (&mut widgets.hovered, t.bg_hover),
                    (&mut widgets.active, t.bg_selected),
                    (&mut widgets.noninteractive, t.bg_elevated),
                ] {
                    visuals.bg_stroke = Stroke::NONE;
                    visuals.corner_radius = radius;
                    visuals.weak_bg_fill = fill;
                    visuals.expansion = 0.;
                }
                let status_galley = egui::WidgetText::from(RichText::new(status).color(color))
                    .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button);
                let title_width = (ui.available_width() - status_galley.size().x - 48.).max(0.);
                let text = egui::WidgetText::from(title.clone()).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    title_width,
                    egui::TextStyle::Button,
                );
                let header = egui::CollapsingHeader::new(text)
                    .id_salt(&tool.id)
                    .show(ui, |ui| {
                        Frame::NONE.inner_margin(Margin::same(8)).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(bounded(&tool.name)).small().monospace().color(t.text_dim)).wrap());
                            if let Some(prepared) = self.cache.tools.get(&tool.id).filter(|p| p.theme == t) {
                                if !tool.output.is_empty() && ui.small_button(kiln_common::i18n::tr("출력 복사")).clicked() {
                                    ui.ctx().copy_text(tool.output.clone());
                                }
                                prepared.show(ui, &tool.id);
                            } else {
                                // Preparation is off-thread; do not synchronously parse
                                // source/JSON or draw a stale palette on this frame.
                                let height = tool_code_ui::viewport_height(ui.clip_rect().height());
                                egui::ScrollArea::both().id_salt((&tool.id, "pending-tool"))
                                    .min_scrolled_height(height).max_height(height)
                                    .show(ui, |ui| {
                                        if let Some(input) = self.cache.inputs.get(&tool.id) { ui.add(egui::Label::new(RichText::new(input).monospace()).selectable(true)); }
                                        if !tool.output.is_empty() { ui.add(egui::Label::new(RichText::new(bounded(&tool.output)).monospace()).selectable(true)); }
                                        if !tool.diff.is_empty() { ui.add(egui::Label::new(RichText::new(bounded(&tool.diff)).monospace()).selectable(true)); }
                                    });
                            }
                        });
                    });
                header.header_response.clone().on_hover_text(bounded(&tool.name));
                widgets::focus_ring(ui, &header.header_response, 6);
                let rect = header.header_response.rect;
                ui.painter().galley(
                    egui::pos2(
                        rect.right() - status_galley.size().x - 6.,
                        rect.center().y - status_galley.size().y / 2.,
                    ),
                    status_galley,
                    color,
                );
                header.header_response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::CollapsingHeader,
                        true,
                        format!("{title} · {status}"),
                    )
                });
            });
    }
    fn request_ui(&mut self, ui: &mut Ui, request: &Request) {
        let t = Theme::current();
        Frame::new()
            .fill(t.bg_panel)
            .stroke(Stroke::new(1., t.border_strong))
            .corner_radius(6)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let summary = self
                    .cache
                    .details
                    .get(&request.id)
                    .filter(|(cached, _)| cached == request)
                    .map(|(_, summary)| summary);
                let kind = summary
                    .map(|s| s.kind)
                    .unwrap_or_else(|| super::approval::kind(request, self.descriptor.tool));
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 18.), egui::Sense::hover());
                    kiln_common::icons::paint(ui.painter(), rect, kiln_common::icons::Icon::Warning, t.orange);
                    ui.label(RichText::new(kind.title()).color(t.orange).strong()).on_hover_text(&request.title);
                });
                let request_title=super::tool_presentation::request_title(request,self.descriptor.tool);
                if request_title!=kind.title() {ui.add(egui::Label::new(request_title).wrap()).on_hover_text(&request.title); }
                if let Some(summary) = summary {
                    egui::ScrollArea::vertical()
                        .id_salt((&request.id, "summary"))
                        // The enclosing history can offer almost no remaining
                        // height. Reserve the bounded summary viewport anyway;
                        // auto-shrink still fits shorter actual content.
                        .min_scrolled_height(180.)
                        .max_height(180.)
                        .show(ui, |ui| {
                            for (label, value) in &summary.fields {
                                ui.label(
                                    RichText::new(kiln_common::i18n::tr(label)).color(t.text_dim),
                                );
                                let presented;
                                let value = if *label == "작업" && value == &request.title { presented = super::tool_presentation::request_title(request, self.descriptor.tool); &presented } else { value };
                                let value = RichText::new(value);
                                let value = if *label == "도구 입력" {
                                    value.monospace()
                                } else {
                                    value
                                };
                                ui.add(egui::Label::new(value).wrap());
                            }
                        });
                }
                let answers = self.answers.entry(request.id.clone()).or_default();
                for question in request.questions.iter().take(16) {
                    ui.add(egui::Label::new(&question.text).wrap());
                    let values = answers.entry(question.id.clone()).or_default();
                    if question.options.is_empty() {
                        if values.is_empty() {
                            values.push(String::new());
                        }
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut values[0])
                                .hint_text(&question.text)
                                .char_limit(8000),
                        );
                        edit.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::TextEdit,
                                true,
                                &question.text,
                            )
                        });
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for option in question.options.iter().take(32) {
                                let selected = values.contains(&option.id);
                                if ui
                                    .selectable_label(selected, &option.label)
                                    .on_hover_text(&option.description)
                                    .clicked()
                                {
                                    if selected {
                                        values.retain(|v| v != &option.id);
                                    } else {
                                        if !question.multi_select {
                                            values.clear();
                                        }
                                        values.push(option.id.clone());
                                    }
                                }
                            }
                        });
                    }
                }
                let answers = answers.clone();
                let summary_ready = summary.is_some();
                let mut decision = self.permission_subset_ui(ui, request);
                ui.add_enabled_ui(
                    self.transport_alive
                        && summary_ready
                        && !self.responding.contains_key(&request.id),
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if request.options.is_empty() {
                                if !(self.descriptor.tool == Tool::Codex && request.title == "item/permissions/requestApproval") && widgets::button(
                                    ui,
                                    kiln_common::i18n::tr("한 번 허용"),
                                    ButtonKind::Primary,
                                )
                                .clicked()
                                {
                                    decision = Some(Decision::Allow);
                                }
                                if widgets::button(
                                    ui,
                                    kiln_common::i18n::tr("거부"),
                                    ButtonKind::Secondary,
                                )
                                .clicked()
                                {
                                    decision = Some(Decision::Deny);
                                }
                            } else {
                                for option in request.options.iter().take(32) {
                                    if ui
                                        .button(&option.label)
                                        .on_hover_text(&option.description)
                                        .clicked()
                                    {
                                        decision = Some(Decision::Option(option.id.clone()));
                                    }
                                }
                                if ui.button(kiln_common::i18n::tr("취소")).clicked() {
                                    decision = Some(Decision::Cancel);
                                }
                            }
                        })
                    },
                );
                if let Some(decision) = decision {
                    match self.send(Command::Respond {
                        request_id: request.id.clone(),
                        decision,
                        answers,
                    }) {
                        Ok(id) => {
                            self.responding.insert(request.id.clone(), id);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            });
    }
    fn composer_ui(&mut self, ui: &mut Ui) -> egui::Rect {
        let t = Theme::current();
        if self.recovered.is_some() {
            ui.add(
                egui::Label::new(
                    RichText::new(kiln_common::i18n::tr(
                        "이 요청은 이미 전송되었을 수 있습니다. 기록을 확인한 뒤 다시 보내세요.",
                    ))
                    .color(t.orange),
                )
                .wrap(),
            );
        }
        if let Some(p) = self.pending.clone() {
            let recoverable = self.pending_recoverable();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(kiln_common::i18n::tr(if recoverable {
                        "전송 결과 미확인 · 자동으로 다시 보내지 않습니다."
                    } else {
                        "전송 확인 대기"
                    }))
                    .color(t.orange),
                );
                if recoverable
                    && self.composer.text.is_empty()
                    && self.composer.attachments.is_empty()
                    && ui.button(kiln_common::i18n::tr("초안으로 복구")).clicked()
                {
                    self.composer = p.draft.clone();
                    self.recovered = Some(p.clone());
                    self.pending = None;
                    self.validate_attachments(ui.ctx());
                }
            });
        }
        let mut editor_focused = false;
        let frame = Frame::new()
            .fill(t.bg_input)
            .stroke(Stroke::new(1., t.border_strong))
            .corner_radius(12)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                let id = ui.id().with("message");
                let focused = ui.memory(|m| m.has_focus(id));
                self.image_paste_ui(ui, focused);
                self.ime_frame = self.ime_guard_frame == Some(ui.ctx().cumulative_frame_nr());
                if !focused && self.composing {
                    if let Some(text) = self.committed_during_preedit.take() {
                        self.composer.text = text;
                    }
                    self.composing = false;
                    self.ime_frame = true;
                }
                if focused {
                    ui.input(|input| {
                        for event in &input.events {
                            if let egui::Event::Ime(event) = event {
                                match event {
                                    egui::ImeEvent::Preedit { text, .. } => {
                                        self.composing = !text.is_empty();
                                        self.ime_frame = true;
                                    }
                                    egui::ImeEvent::Commit(_) => {
                                        self.composing = false;
                                        self.ime_frame = true;
                                    }
                                    #[allow(deprecated)]
                                    egui::ImeEvent::Disabled => {
                                        self.composing = false;
                                        self.ime_frame = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    });
                }
                if self.ime_frame {
                    self.ime_guard_frame = Some(ui.ctx().cumulative_frame_nr());
                }
                let suggestion = self.prompt_suggestion().map(|(id, text)|(id.to_owned(), text.to_owned()));
                let mut suggestion_handled = false;
                if focused && ui.input(|input| !input.modifiers.any()) && let Some((suggestion_id, text)) = &suggestion {
                    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
                        self.composer.text = text.clone();
                        self.dismissed_suggestion = Some(suggestion_id.clone());
                        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(text.chars().count()))));
                        state.store(ui.ctx(), id);
                        ui.ctx().memory_mut(|memory| { memory.move_focus(egui::FocusDirection::None); memory.request_focus(id); });
                        suggestion_handled = true;
                    } else if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                        self.dismissed_suggestion = Some(suggestion_id.clone());
                        suggestion_handled = true;
                    }
                }
                let completion_handled = suggestion_handled || self.mention_completion_input(ui, focused) || self.slash_completion_input(ui, focused);
                let enter = focused
                    && !completion_handled
                    && !self.composing
                    && !self.ime_frame
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Enter)
                            && !i.modifiers.shift
                            && !i.modifiers.command
                            && !i.modifiers.ctrl
                            && !i.modifiers.alt
                    });
                if enter {
                    ui.input_mut(|i| {
                        i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    });
                }
                let mut command_layouter = self.command_layouter();
                let output = egui::TextEdit::multiline(&mut self.composer.text)
                    .layouter(&mut command_layouter)
                    .id(id)
                    .event_filter(egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, escape: suggestion.is_some() && !suggestion_handled, ..Default::default() })
                    .hint_text(suggestion.as_ref().filter(|_|!suggestion_handled).map(|(_, text)|text.as_str()).unwrap_or_else(||kiln_common::i18n::tr("작업을 입력하세요…")))
                    .frame(Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(if self.controls_ui.surface == Surface::Conversation { 2 } else { 1 })
                    .char_limit(120_000)
                    .show(ui);
                if self.composing
                    && let Some(range) = output.cursor_range
                {
                    // TextEdit has processed the complete ordered event batch;
                    // exclude its actual composing range, preserving earlier commits.
                    let range = range.as_sorted_char_range();
                    self.committed_during_preedit = Some(
                        self.composer
                            .text
                            .chars()
                            .enumerate()
                            .filter_map(|(index, ch)| {
                                (!(range.start.0..range.end.0).contains(&index)).then_some(ch)
                            })
                            .collect(),
                    );
                }
                if output.response.response.changed() && !self.composer.text.is_empty()
                    && let Some((suggestion_id, _)) = &suggestion { self.dismissed_suggestion = Some(suggestion_id.clone()); }
                let edit = output.response.response.clone().on_hover_text(kiln_common::i18n::tr(if suggestion.is_some() && !suggestion_handled {
                    "Tab으로 추천 입력 · Esc로 숨기기"
                } else { "Enter로 보내기 · Shift+Enter로 줄바꿈" }));
                edit.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::TextEdit,
                        true,
                        kiln_common::i18n::tr("메시지"),
                    )
                });
                if self.focus {
                    edit.request_focus();
                    self.focus = false;
                }
                editor_focused = edit.has_focus();
                // Losing focus cancels stale composition state, but blocks the click frame.
                if edit.lost_focus() && self.composing {
                    if let Some(text) = self.committed_during_preedit.take() {
                        self.composer.text = text;
                    }
                    self.composing = false;
                    self.ime_frame = true;
                }
                if !self.composing {
                    self.committed_during_preedit = None;
                }
                if !self.composer.attachments.is_empty() || !self.imports.is_empty() {
                    egui::ScrollArea::vertical()
                        .id_salt("attachment-chips")
                        .max_height(60.)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let mut remove = None;
                                for (index, a) in self.composer.attachments.clone().iter().enumerate() {
                                    self.attachment_preview_ui(ui, a);
                                    let label = a.name.chars().take(28).collect::<String>();
                                    if ui
                                        .button(format!("▧ {label} ×"))
                                        .on_hover_text(format!(
                                            "{} · {}",
                                            kiln_common::i18n::tr("첨부 제거"),
                                            a.name
                                        ))
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                }
                                if let Some(index) = remove {
                                    self.composer.attachments.remove(index);
                                    self.attachment_error = None;
                                    self.validate_attachments(ui.ctx());
                                }
                                if !self.imports.is_empty() {
                                    ui.small(kiln_common::i18n::tr("이미지 가져오는 중…"));
                                }
                            })
                        });
                }
                self.image_notice_ui(ui);
                if let Some(error) = &self.attachment_error {
                    ui.add(egui::Label::new(RichText::new(error).color(t.red)).wrap());
                }
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    let controls_width = (ui.available_width() - if self.show_stop() { 74. } else { 38. }).max(0.);
                    let compact = ui.available_width() < 560.;
                    ui.allocate_ui_with_layout(egui::vec2(controls_width, 28.), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.;
                        self.composer_controls_ui(ui);
                        if self.controls_ui.surface == Surface::Conversation { self.composer_options_ui(ui, compact); }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.composer_action_ui(ui, enter));
                });
                if self.busy() && self.descriptor.tool == Tool::Codex {
                    ui.add(egui::Label::new(RichText::new(kiln_common::i18n::tr("모델·추론 강도·모드 선택은 다음 턴에 적용됩니다.")).small().color(t.text_dim)).wrap());
                }
                if suggestion.is_some() && !suggestion_handled && self.composer.text.is_empty() {
                    ui.small(kiln_common::i18n::tr("Tab으로 추천 입력 · Esc로 숨기기"));
                }
                self.slash_completion_ui(ui);
                self.mention_completion_ui(ui);

            });
        if editor_focused {
            ui.painter().rect_stroke(
                frame.response.rect,
                12,
                Stroke::new(1.5, widgets::tint(t.accent, 0.7)),
                egui::StrokeKind::Inside,
            );
        }
        frame.response.rect
    }
    fn composer_controls_ui(&mut self, ui: &mut Ui) {
        self.image_picker_ui(ui);
        let t = Theme::current();
        let widgets = &mut ui.visuals_mut().widgets;
        for visuals in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
        ] {
            visuals.corner_radius = egui::CornerRadius::same(6);
        }
        widgets.inactive.bg_fill = t.bg_elevated;
        widgets.inactive.weak_bg_fill = if t.dark { t.bg_elevated } else { t.bg_hover };
        widgets.inactive.bg_stroke = Stroke::NONE;
    }

    fn show_stop(&self) -> bool {
        self.busy() || !self.local_intents.is_empty()
            || self.transcript.controls.outcomes.values().any(|v| !v.settled)
    }
    fn composer_action_ui(&mut self, ui: &mut Ui, enter: bool) {
        if self.show_stop() {
            if composer_icon(ui, kiln_common::icons::Icon::Stop, kiln_common::i18n::tr("중지"), false)
            .clicked()
            {
                if let Err(error) = self.send(Command::Interrupt) {
                    self.error = Some(error);
                }
            }
        }
        {
            let enabled = !self.submit_settings_blocked() && self.live_input_allowed()
                && self.pending.is_none()
                && self.backlog.is_empty()
                && !self.image_import_pending()
                && self.validation.is_none()
                && self.attachment_error.is_none()
                && !self.composing
                && !self.ime_frame
                && self
                    .composer
                    .model
                    .as_ref()
                    .is_none_or(|id| self.transcript.models.iter().any(|m| &m.id == id));
            let clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    composer_icon(ui, kiln_common::icons::Icon::ArrowUp, kiln_common::i18n::tr("보내기"), true)
                })
                .inner
                .clicked();
            if clicked || enter {
                self.submit();
            }
        }
    }

}

fn composer_icon(ui: &mut Ui, icon: kiln_common::icons::Icon, label: &str, primary: bool) -> egui::Response {
    let t = Theme::current();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28., 28.), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    ui.painter().circle_filled(rect.center(), 14., if primary { t.accent } else { t.bg_hover });
    kiln_common::icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center(), egui::vec2(15.,15.)), icon, if primary { t.accent_fg } else { t.text });
    widgets::focus_ring(ui, &response, 14);
    response.on_hover_text(label)
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "settings_tests.rs"]
mod settings_tests;
