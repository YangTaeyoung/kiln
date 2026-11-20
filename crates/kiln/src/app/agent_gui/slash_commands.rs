//! Provider commands implemented through Kiln's existing controls and native transports.
use super::*;
use kiln_common::i18n::tr;
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub(super) enum Slash {
    Catalog,
    Skills,
    Mcp,
    Status,
    Context,
    WorkingDirectory,
    Copy,
    Diff,
    Mention,
    Rename,
    Tasks,
    Policy(&'static str),
    Setting(&'static str),
    Compact,
    Review,
    Goal,
    Memory,
    ReloadPlugins,
    ReloadSkills,
    ReloadStyles,
    Terminal,
}

struct Builtin {
    name: &'static str,
    description: &'static str,
    action: Slash,
}

macro_rules! commands {
    ($($name:literal, $description:literal, $action:expr);* $(;)?) => {
        &[$(Builtin { name: $name, description: $description, action: $action }),*]
    };
}

const CODEX: &[Builtin] = commands![
    "mcp", "MCP 상태", Slash::Mcp;
    "model", "모델", Slash::Setting("model");
    "effort", "추론 강도", Slash::Setting("effort");
    "permissions", "권한 프로필", Slash::Policy("permissions");
    "approvals", "권한 프로필", Slash::Policy("permissions");
    "fast", "서비스 등급", Slash::Setting("serviceTier");
    "plan", "모드", Slash::Setting("collaborationMode");
    "status", "상태 및 연결", Slash::Status;
    "config", "대화 설정", Slash::Setting("");
    "skills", "스킬", Slash::Skills;
    "compact", "대화 압축", Slash::Compact;
    "review", "검토 시작", Slash::Review;
    "goal", "목표", Slash::Goal;
    "memories", "메모리 상태", Slash::Memory;
    "copy", "응답 복사", Slash::Copy;
    "diff", "변경 사항", Slash::Diff;
    "mention", "파일", Slash::Mention;
    "rename", "이름 바꾸기", Slash::Rename;
    "ps", "작업", Slash::Tasks;
    "pwd", "작업 폴더", Slash::WorkingDirectory;
    "cwd", "작업 폴더", Slash::WorkingDirectory;
];

const CLAUDE: &[Builtin] = commands![
    "mcp", "MCP 상태", Slash::Mcp;
    "help", "명령 및 스킬", Slash::Catalog;
    "skills", "스킬", Slash::Skills;
    "model", "모델", Slash::Setting("model");
    "effort", "추론 강도", Slash::Setting("effortLevel");
    "fast", "빠른 모드", Slash::Setting("fastMode");
    "plan", "모드", Slash::Setting("permissionMode");
    "permissions", "정책 요구사항", Slash::Policy("permissionMode");
    "allowed-tools", "정책 요구사항", Slash::Policy("permissionMode");
    "config", "대화 설정", Slash::Setting("");
    "settings", "대화 설정", Slash::Setting("");
    "status", "상태 및 연결", Slash::Status;
    "context", "컨텍스트", Slash::Context;
    "copy", "응답 복사", Slash::Copy;
    "diff", "변경 사항", Slash::Diff;
    "tasks", "작업", Slash::Tasks;
    "agents", "주 에이전트", Slash::Setting("agent");
    "output-style", "응답 스타일", Slash::Setting("outputStyle");
    "reload-plugins", "플러그인 다시 로드", Slash::ReloadPlugins;
    "reload-skills", "스킬 다시 로드", Slash::ReloadSkills;
    "reload-output-styles", "스타일 다시 로드", Slash::ReloadStyles;
];

const AGY: &[Builtin] = commands![
    "mcp", "MCP 상태", Slash::Mcp;
    "help", "명령 및 스킬", Slash::Catalog;
    "model", "모델", Slash::Setting("model");
    "effort", "추론 강도", Slash::Setting("thought_level");
    "context", "컨텍스트", Slash::Context;
    "copy", "응답 복사", Slash::Copy;
    "tasks", "작업", Slash::Tasks;
    "config", "대화 설정", Slash::Setting("");
    "settings", "대화 설정", Slash::Setting("");
    "skills", "스킬", Slash::Terminal;
];

#[derive(Clone, Copy)]
struct CatalogCommand {
    name: &'static str,
    description: &'static str,
    canonical: &'static str,
}

fn read_catalog(text: &'static str) -> Vec<CatalogCommand> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|row| {
            let (name, rest) = row
                .split_once('\t')
                .expect("a catalog row has a description");
            let (description, canonical) = rest.split_once('\t').unwrap_or((rest, name));
            CatalogCommand {
                name,
                description,
                canonical,
            }
        })
        .collect()
}

fn upstream_catalog(tool: Tool) -> &'static [CatalogCommand] {
    use std::sync::LazyLock;
    static CODEX: LazyLock<Vec<CatalogCommand>> =
        LazyLock::new(|| read_catalog(include_str!("slash_catalog/codex.tsv")));
    static CLAUDE: LazyLock<Vec<CatalogCommand>> =
        LazyLock::new(|| read_catalog(include_str!("slash_catalog/claude.tsv")));
    static AGY: LazyLock<Vec<CatalogCommand>> =
        LazyLock::new(|| read_catalog(include_str!("slash_catalog/antigravity.tsv")));
    match tool {
        Tool::Codex => &CODEX,
        Tool::Claude => &CLAUDE,
        Tool::Antigravity => &AGY,
    }
}

fn builtins(tool: Tool) -> &'static [Builtin] {
    match tool {
        Tool::Codex => CODEX,
        Tool::Claude => CLAUDE,
        Tool::Antigravity => AGY,
    }
}

fn resolve(tool: Tool, name: &str) -> Option<Slash> {
    let upstream = upstream_catalog(tool).iter().find(|item| item.name == name);
    let canonical = if tool == Tool::Codex && goal_alias(name) {
        "goal"
    } else {
        upstream.map_or(name, |item| item.canonical)
    };
    builtins(tool)
        .iter()
        .find(|command| command.name == canonical)
        .map(|command| command.action)
        .or_else(|| upstream.map(|_| Slash::Terminal))
}

fn goal_alias(name: &str) -> bool {
    name.strip_prefix('g')
        .and_then(|tail| tail.strip_suffix("al"))
        .is_some_and(|middle| !middle.is_empty() && middle.bytes().all(|c| c == b'o'))
}

fn catalog(tool: Tool) -> Vec<NativeEntry> {
    let direct = builtins(tool);
    let upstream = upstream_catalog(tool);
    let mut entries = direct
        .iter()
        .map(|command| {
            entry(
                command.name,
                tr(command.description),
                matches!(command.action, Slash::Terminal),
            )
        })
        .collect::<Vec<_>>();
    entries.extend(
        upstream
            .iter()
            .filter(|item| {
                item.canonical == item.name
                    && !direct.iter().any(|command| command.name == item.name)
            })
            .map(|item| entry(item.name, item.description, true)),
    );
    for item in upstream.iter().filter(|item| item.canonical != item.name) {
        if let Some(target) = entries
            .iter_mut()
            .find(|entry| entry.name == item.canonical)
        {
            target.aliases.push(item.name.into());
        } else {
            entries.push(entry(item.name, item.description, true));
        }
    }
    entries
}

fn entry(name: &str, description: &str, terminal_only: bool) -> NativeEntry {
    NativeEntry {
        name: name.into(),
        description: description.into(),
        terminal_only,
        builtin: Some(true),
        interface: Default::default(),
        enabled: None,
        scope: None,
        origin: None,
        model: None,
        aliases: vec![],
        argument_hint: None,
        path: None,
    }
}

/// GUI actions take a standalone slash command. Multiline prompts and paths
/// embedding slashes still go through the provider's normal message path.
pub(super) fn parse(text: &str) -> Option<(&str, &str)> {
    let text = text.trim();
    if text.contains(['\n', '\r']) {
        return None;
    }
    let token = text.strip_prefix('/')?;
    let (name, arguments) = token.split_once(char::is_whitespace).unwrap_or((token, ""));
    (!name.is_empty()).then_some((name, arguments.trim()))
}

#[derive(Clone)]
pub(super) enum SlashPanel {
    Mcp { verbose: bool },
    Skills,
    Status,
    Actions,
    WorkingDirectory,
    Diff,
    Rename,
    Tasks,
    Terminal(String),
    Output { title: &'static str, text: String },
}

pub(in crate::app) struct NativeSlashDraft {
    generation: String,
    session: String,
    text: String,
}

impl AgentPane {
    pub(super) fn slash_catalog(&self) -> Vec<Cow<'_, NativeEntry>> {
        let tool = self.descriptor.tool;
        let mut entries = self
            .transcript
            .controls
            .commands
            .iter()
            .flatten()
            .map(|entry| {
                if entry.terminal_only
                    && resolve(tool, entry.name.trim_start_matches('/'))
                        .is_some_and(|action| !matches!(action, Slash::Terminal))
                {
                    let mut entry = entry.clone();
                    entry.terminal_only = false;
                    Cow::Owned(entry)
                } else {
                    Cow::Borrowed(entry)
                }
            })
            .collect::<Vec<_>>();
        for entry in catalog(tool) {
            if let Some(existing) = entries
                .iter_mut()
                .find(|existing| existing.name.trim_start_matches('/') == entry.name)
            {
                if entry
                    .aliases
                    .iter()
                    .any(|alias| !existing.aliases.contains(alias))
                {
                    let existing = existing.to_mut();
                    for alias in entry.aliases {
                        if !existing.aliases.contains(&alias) {
                            existing.aliases.push(alias);
                        }
                    }
                }
            } else {
                entries.push(Cow::Owned(entry));
            }
        }
        entries
    }

    pub(super) fn submit_slash(&mut self) -> bool {
        let text = self.composer.text.trim().to_owned();
        let Some((name, arguments)) = parse(&text) else {
            return false;
        };
        let tool = self.descriptor.tool;
        let advertised = self
            .transcript
            .controls
            .commands
            .iter()
            .flatten()
            .find(|entry| {
                entry.name.trim_start_matches('/') == name
                    || entry
                        .aliases
                        .iter()
                        .any(|alias| alias.trim_start_matches('/') == name)
            });
        let canonical = advertised.map_or(name, |entry| entry.name.trim_start_matches('/'));
        let Some(action) = resolve(tool, canonical).or_else(|| {
            advertised
                .filter(|entry| entry.terminal_only)
                .map(|_| Slash::Terminal)
        }) else {
            return false;
        };
        if advertised.is_some_and(|entry| entry.enabled == Some(false)) {
            self.error = Some(tr("비활성화됨").into());
            return true;
        }
        if tool != Tool::Codex
            && advertised.is_some_and(|entry| !entry.terminal_only)
            && (matches!(action, Slash::Terminal)
                || (tool == Tool::Antigravity
                    && matches!(action, Slash::Skills | Slash::Setting(_))
                    && !arguments.is_empty()))
        {
            return false;
        }
        if !self.composer.attachments.is_empty() || !self.composer.skills.is_empty() {
            self.error = Some(tr("이 명령은 단독으로 보내세요.").into());
            return true;
        }
        if self.settings_blocked() {
            return true;
        }

        let mut panel = None;
        let mut surface = None;
        let command = match action {
            Slash::Mcp if matches!(arguments, "" | "list" | "verbose") => {
                panel = Some(SlashPanel::Mcp {
                    verbose: arguments == "verbose",
                });
                Some(Command::Discover(Discovery::Mcp))
            }
            Slash::Catalog | Slash::Skills if arguments.is_empty() => {
                self.controls_ui.command_search.clear();
                surface = Some(Surface::Commands);
                if matches!(action, Slash::Skills) {
                    panel = Some(SlashPanel::Skills);
                    Some(Command::Discover(Discovery::Skills))
                } else {
                    None
                }
            }
            Slash::Status | Slash::Context if arguments.is_empty() => {
                panel = Some(SlashPanel::Status);
                (tool != Tool::Antigravity).then_some(Command::Discover(Discovery::Context))
            }
            Slash::WorkingDirectory if arguments.is_empty() => {
                panel = Some(SlashPanel::WorkingDirectory);
                None
            }
            Slash::Diff if arguments.is_empty() => {
                self.controls_ui.slash_diff =
                    Some(kiln_git::DiffView::for_worktree(&self.descriptor.cwd));
                panel = Some(SlashPanel::Diff);
                None
            }
            Slash::Mention => {
                self.composer.text = format!("@{arguments}");
                self.controls_ui.surface = Surface::Conversation;
                self.controls_ui.slash_panel = None;
                self.focus = true;
                return true;
            }
            Slash::Rename if arguments.is_empty() => {
                self.controls_ui.slash_name =
                    editing_text::EditingText::new(self.conversation_title());
                panel = Some(SlashPanel::Rename);
                None
            }
            Slash::Rename if arguments.len() <= 256 && !arguments.chars().any(char::is_control) => {
                Some(Command::NativeAction(NativeAction::Codex(
                    CodexAction::Rename {
                        name: arguments.into(),
                    },
                )))
            }
            Slash::Tasks if arguments.is_empty() => {
                panel = Some(SlashPanel::Tasks);
                None
            }
            Slash::Copy
                if arguments.is_empty() || arguments.parse::<usize>().is_ok_and(|n| n > 0) =>
            {
                let index = arguments.parse::<usize>().unwrap_or(1) - 1;
                let response = self
                    .transcript
                    .entries
                    .iter()
                    .rev()
                    .filter_map(|entry| match entry {
                        Entry::Message(message)
                            if message.role == Role::Assistant && !message.text.is_empty() =>
                        {
                            Some(&message.text)
                        }
                        _ => None,
                    })
                    .nth(index);
                let Some(response) = response else {
                    self.error = Some(tr("복사할 응답이 없습니다.").into());
                    return true;
                };
                panel = Some(SlashPanel::Output {
                    title: "응답 복사",
                    text: response.clone(),
                });
                None
            }
            Slash::Policy(control) | Slash::Setting(control) => {
                if let Some(value) = slash_value(action, canonical, arguments) {
                    if !self.editing_allowed() {
                        self.error = Some(tr("현재 대화에서 실행할 수 없습니다.").into());
                        return true;
                    }
                    let option = self.transcript.controls.options.iter().find(|option| {
                        option.id == control
                            || (tool == Tool::Antigravity
                                && option.category.as_deref() == Some(control))
                    });
                    if let Some(option) = option.filter(|option| option.validate(&value).is_ok()) {
                        Some(Command::Configure {
                            control: option.id.clone(),
                            value,
                        })
                    } else {
                        panel = Some(SlashPanel::Terminal(text.clone()));
                        None
                    }
                } else if arguments.is_empty() {
                    self.controls_ui.search = control.into();
                    surface = Some(Surface::Settings);
                    matches!(action, Slash::Policy(_))
                        .then_some(Command::Discover(Discovery::Policy))
                } else {
                    panel = Some(SlashPanel::Terminal(text.clone()));
                    None
                }
            }
            Slash::Memory if arguments.is_empty() => {
                panel = Some(SlashPanel::Actions);
                Some(Command::Discover(Discovery::Memory))
            }
            Slash::Review if arguments.is_empty() => {
                panel = Some(SlashPanel::Actions);
                None
            }
            Slash::Goal if matches!(arguments, "" | "edit") => {
                panel = Some(SlashPanel::Actions);
                Some(Command::Discover(Discovery::Goal))
            }
            Slash::Mcp if arguments == "reload" && tool == Tool::Codex => {
                let Some(command) = self.slash_action(NativeAction::Codex(CodexAction::ReloadMcp))
                else {
                    return true;
                };
                panel = Some(SlashPanel::Mcp { verbose: false });
                Some(command)
            }
            Slash::Goal => {
                let goal = match arguments {
                    "clear" => CodexAction::ClearGoal,
                    "pause" => goal_status(GoalStatus::Paused),
                    "resume" => goal_status(GoalStatus::Active),
                    objective => CodexAction::SetGoal {
                        objective: Some(objective.into()),
                        status: None,
                        token_budget: None,
                    },
                };
                self.slash_action(NativeAction::Codex(goal))
            }
            Slash::Review => self.slash_action(NativeAction::Codex(CodexAction::Review(
                ReviewTarget::Custom {
                    instructions: arguments.into(),
                },
            ))),
            Slash::Compact | Slash::ReloadPlugins | Slash::ReloadSkills | Slash::ReloadStyles
                if arguments.is_empty() =>
            {
                let native = match action {
                    Slash::Compact => NativeAction::Compact,
                    Slash::ReloadPlugins => NativeAction::ReloadPlugins,
                    Slash::ReloadSkills => NativeAction::ReloadSkills,
                    Slash::ReloadStyles => NativeAction::ReloadStyles,
                    _ => unreachable!(),
                };
                self.slash_action(native)
            }
            _ => {
                panel = Some(SlashPanel::Terminal(text));
                None
            }
        };
        if let Some(command) = command {
            if let Err(error) = self.send(command) {
                self.error = Some(error);
                return true;
            }
        } else if panel.is_none() && surface.is_none() {
            return true;
        }
        self.error = None;
        if let Some(SlashPanel::Terminal(command)) = &panel {
            self.open_native_slash(command);
        }
        self.controls_ui.surface = surface.unwrap_or(if panel.is_some() {
            Surface::Commands
        } else {
            self.controls_ui.surface
        });
        self.controls_ui.slash_panel = panel;
        self.controls_ui.composer_options_open = false;
        if !matches!(self.controls_ui.slash_panel, Some(SlashPanel::Terminal(_))) {
            self.composer.text.clear();
        }
        self.focus = true;
        true
    }

    fn slash_action(&mut self, action: NativeAction) -> Option<Command> {
        if self.editing_allowed() {
            Some(Command::NativeAction(action))
        } else {
            self.error = Some(tr("현재 대화에서 실행할 수 없습니다.").into());
            None
        }
    }

    pub(super) fn mcp_status_ui(&self, ui: &mut Ui, verbose: bool) {
        ui.heading(tr("MCP 상태"));
        let Some(status) = self.transcript.controls.status.get("mcp") else {
            ui.label(tr("목록을 아직 받지 못했습니다."));
            return;
        };
        if let Some(message) = &status.unavailable {
            ui.add(egui::Label::new(message).wrap());
        }
        for (index, fields) in mcp_rows(&status.fields) {
            let Some(ControlValue::Text(name)) = fields.get("name") else {
                continue;
            };
            ui.push_id(("mcp-server", index), |ui| {
                ui.add(egui::Label::new(RichText::new(name.as_str()).strong()).wrap());
                ui.horizontal_wrapped(|ui| {
                    for key in ["status", "state", "authStatus", "scope", "transport"] {
                        if let Some(value) = fields.get(key) {
                            ui.small(settings_ui::value_label(Some(value)));
                        }
                    }
                    if let Some(count) = fields.get("tool_count") {
                        ui.small(format!(
                            "{} · {}",
                            tr("도구"),
                            settings_ui::value_label(Some(count))
                        ));
                    }
                });
                if let Some(ControlValue::Strings(tools)) = fields.get("tool_names")
                    && !tools.is_empty()
                {
                    egui::CollapsingHeader::new(tr("도구"))
                        .default_open(verbose)
                        .show(ui, |ui| {
                            for name in tools {
                                ui.add(egui::Label::new(name).wrap());
                            }
                        });
                }
            });
        }
        if status.fields.keys().all(|key| !key.ends_with(".name")) && status.unavailable.is_none() {
            ui.label(tr("제공된 항목이 없습니다."));
        }
        self.coverage_ui(ui, Some("mcp"));
    }

    pub(super) fn slash_panel_ui(&mut self, ui: &mut Ui, panel: SlashPanel) {
        match panel {
            SlashPanel::Mcp { verbose } => {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("MCP 새로 고침")).clicked() {
                        self.discover(Discovery::Mcp);
                    }
                    self.native_slash_button(ui, "/mcp");
                });
                self.mcp_status_ui(ui, verbose);
            }
            SlashPanel::Skills => {}
            SlashPanel::Status => {
                if self.descriptor.tool == Tool::Antigravity {
                    if let Some(context) = self.transcript.controls.status.get("context") {
                        ui.heading(tr("컨텍스트"));
                        for (key, value) in &context.fields {
                            ui.label(format!("{key} · {}", settings_ui::value_label(Some(value))));
                        }
                    } else {
                        ui.label(tr("네이티브 상태를 아직 받지 못했습니다."));
                    }
                } else {
                    self.status_ui(ui);
                }
            }
            SlashPanel::Actions => self.actions_ui(ui),
            SlashPanel::WorkingDirectory => {
                ui.label(tr("작업 폴더"));
                ui.add(
                    egui::Label::new(
                        RichText::new(self.descriptor.cwd.display().to_string()).monospace(),
                    )
                    .wrap(),
                );
            }
            SlashPanel::Diff => {
                if let Some(diff) = &mut self.controls_ui.slash_diff {
                    diff.ui(ui);
                }
            }
            SlashPanel::Rename => {
                ui.heading(tr("이름 바꾸기"));
                self.controls_ui.slash_name.show(ui, tr("이름"), false, 256);
                let name = self.controls_ui.slash_name.text.trim();
                if ui
                    .add_enabled(
                        !name.is_empty()
                            && name.len() <= 256
                            && !name.chars().any(char::is_control)
                            && self.controls_ui.slash_name.ready(ui.ctx())
                            && !self.settings_blocked(),
                        egui::Button::new(tr("적용")),
                    )
                    .clicked()
                {
                    let name = name.to_owned();
                    self.native_action(NativeAction::Codex(CodexAction::Rename { name }));
                }
            }
            SlashPanel::Tasks => {
                ui.heading(tr("작업"));
                self.activity_ui(ui);
            }
            SlashPanel::Output { title, text } => {
                ui.heading(tr(title));
                if ui.button(tr("복사")).clicked() {
                    ui.ctx().copy_text(text.clone());
                }
                ui.add(egui::Label::new(text).wrap().selectable(true));
            }
            SlashPanel::Terminal(command) => {
                ui.heading(tr("CLI 명령"));
                ui.add(egui::Label::new(egui::RichText::new(&command).monospace()).wrap());
                self.native_slash_button(ui, &command);
            }
        }
    }

    fn native_slash_button(&mut self, ui: &mut Ui, command: &str) {
        if ui
            .add_enabled(
                self.can_switch_presentation(Presentation::Terminal),
                egui::Button::new(tr("CLI 명령 열기")),
            )
            .clicked()
        {
            self.open_native_slash(command);
        }
    }

    fn open_native_slash(&mut self, text: &str) -> bool {
        if text.len() > 16 * 1024 || text.chars().any(char::is_control) {
            self.error = Some(tr("이 명령은 단독으로 보내세요.").into());
            return false;
        }
        let Some(session) = self.transcript.provider_session_id.clone() else {
            return false;
        };
        if !self.request_presentation(Presentation::Terminal) {
            return false;
        }
        self.native_slash = Some(NativeSlashDraft {
            generation: self.transcript.generation.clone(),
            session,
            text: text.into(),
        });
        true
    }

    pub(in crate::app) fn native_slash_text(&self) -> Option<&str> {
        let draft = self.native_slash.as_ref()?;
        (draft.generation == self.transcript.generation
            && self.transcript.provider_session_id.as_deref() == Some(draft.session.as_str()))
        .then_some(draft.text.as_str())
    }

    pub(in crate::app) fn native_slash_can_insert(&self) -> bool {
        self.native_slash_text().is_some()
            && self.presentation() == Presentation::Terminal
            && self.live_cli_frontend()
            && !self.presentation_pending()
            && !self.transcript.return_requested
            && self.transport_alive
    }

    fn live_cli_frontend(&self) -> bool {
        self.transcript
            .controls
            .status
            .get("native_cli_frontend")
            .and_then(|status| status.fields.get("running"))
            == Some(&ControlValue::Bool(true))
    }

    pub(in crate::app) fn take_native_slash_input(&mut self) -> Option<String> {
        if !self.native_slash_can_insert() {
            return None;
        }
        let draft = self.native_slash.take()?;
        if self.composer.text.trim() == draft.text {
            self.composer.text.clear();
        }
        Some(draft.text)
    }
}

fn slash_value(action: Slash, name: &str, arguments: &str) -> Option<ControlValue> {
    match (action, name, arguments) {
        (Slash::Setting(_), "plan", "") => Some(ControlValue::Text("plan".into())),
        (Slash::Setting("fastMode"), _, "on") => Some(ControlValue::Bool(true)),
        (Slash::Setting("fastMode"), _, "off") => Some(ControlValue::Bool(false)),
        (Slash::Setting("fastMode"), _, _) => None,
        (Slash::Setting(control) | Slash::Policy(control), _, value)
            if !control.is_empty() && !value.is_empty() =>
        {
            Some(ControlValue::Text(value.into()))
        }
        _ => None,
    }
}

fn mcp_rows(
    fields: &BTreeMap<String, ControlValue>,
) -> BTreeMap<usize, BTreeMap<&str, &ControlValue>> {
    let mut rows = BTreeMap::<usize, BTreeMap<&str, &ControlValue>>::new();
    for (field, value) in fields {
        if let Some((index, key)) = field.split_once('.')
            && let Ok(index) = index.parse()
        {
            rows.entry(index).or_default().insert(key, value);
        }
    }
    rows
}

fn goal_status(status: GoalStatus) -> CodexAction {
    CodexAction::SetGoal {
        objective: None,
        status: Some(status),
        token_budget: None,
    }
}
