//! Discoverable native catalogs. Selection edits a draft; only Send executes it.
use super::*;
use kiln_common::i18n::tr;

#[derive(Clone, Default)]
struct CompletionState {
    token: String,
    active: usize,
    dismissed: bool,
    scroll_to_active: bool,
    navigated: bool,
}
#[derive(Clone)]
struct CompletionEntry {
    native: NativeEntry,
    skill: bool,
}

impl AgentPane {
    pub(super) fn commands_ui(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.;
        let skills_only = matches!(
            self.controls_ui.slash_panel,
            Some(slash_commands::SlashPanel::Skills)
        );
        if let Some(panel) = self.controls_ui.slash_panel.clone() {
            if ui.button(tr("목록")).clicked() {
                self.controls_ui.slash_panel = None;
            }
            if !skills_only {
                self.slash_panel_ui(ui, panel);
                self.command_errors_ui(ui);
                return;
            }
        }
        ui.heading(tr(if skills_only {
            "스킬"
        } else {
            "명령 및 스킬"
        }));
        ui.add(egui::Label::new(tr("초안에 삽입한 뒤 보내기를 눌러 실행하세요.")).wrap());
        self.command_search_ui(ui);
        if self.descriptor.tool != Tool::Antigravity {
            ui.horizontal_wrapped(|ui| {
                if !skills_only
                    && ui
                        .add_enabled(
                            !self.settings_blocked(),
                            egui::Button::new(tr("명령 새로 고침")),
                        )
                        .clicked()
                {
                    self.discover(Discovery::Commands);
                }
                if ui
                    .add_enabled(
                        self.editing_allowed(),
                        egui::Button::new(tr("스킬 새로 고침")),
                    )
                    .clicked()
                {
                    self.discover(Discovery::Skills);
                }
            });
        }
        let controls = self.transcript.controls.clone();
        let commands: Vec<_> = self
            .slash_catalog()
            .into_iter()
            .map(std::borrow::Cow::into_owned)
            .collect();
        let commands = Some(commands.iter().collect::<Vec<_>>());
        let skills = controls
            .skills
            .as_ref()
            .map(|entries| entries.iter().collect::<Vec<_>>());
        // The catalog, including expanded descriptions, has its own scrolling boundary.
        // Changing the search starts its results at the top without moving the composer.
        egui::ScrollArea::vertical()
            .id_salt(("command-catalog", &self.controls_ui.command_search))
            .max_height(
                (ui.clip_rect().bottom() - ui.next_widget_position().y - 24.).clamp(80., 420.),
            )
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (title, catalog, skill) in [("명령", &commands, false), ("스킬", &skills, true)]
                {
                    if skills_only && !skill {
                        continue;
                    }
                    ui.add_space(6.);
                    ui.strong(tr(title));
                    match catalog {
                        None => {
                            ui.label(tr("목록을 아직 받지 못했습니다."));
                        }
                        Some(entries) => {
                            let query = self
                                .controls_ui
                                .command_search
                                .trim()
                                .trim_start_matches(['/', '$'])
                                .to_lowercase();
                            let visible = entries
                                .iter()
                                .copied()
                                .filter(|entry| self.entry_visible(entry, skill))
                                .collect::<Vec<_>>();
                            let filtered = visible
                                .iter()
                                .copied()
                                .filter(|entry| {
                                    format!(
                                        "{} {} {} {}",
                                        entry.name,
                                        entry
                                            .interface
                                            .get("displayName")
                                            .map(String::as_str)
                                            .unwrap_or(""),
                                        entry.description,
                                        entry.aliases.join(" ")
                                    )
                                    .to_lowercase()
                                    .contains(&query)
                                })
                                .collect::<Vec<_>>();
                            if filtered.is_empty() {
                                ui.label(tr(if visible.is_empty() {
                                    "제공된 항목이 없습니다."
                                } else {
                                    "검색 결과가 없습니다."
                                }));
                            }
                            for entry in filtered {
                                ui.push_id((title, &entry.name), |ui| {
                                    self.command_row(ui, entry, skill)
                                });
                            }
                        }
                    }
                    self.coverage_ui(ui, Some(if skill { "skills" } else { "commands" }));
                }
                for (title, entries) in [
                    ("에이전트", &controls.agents),
                    ("응답 스타일", &controls.styles),
                ] {
                    if !skills_only && let Some(entries) = entries {
                        egui::CollapsingHeader::new(tr(title)).show(ui, |ui| {
                            for entry in entries.iter().filter(|entry| !entry.terminal_only) {
                                ui.strong(&entry.name);
                                ui.add(egui::Label::new(&entry.description).wrap());
                            }
                            if entries.is_empty() {
                                ui.label(tr("제공된 항목이 없습니다."));
                            }
                        });
                    }
                }
            });
        self.command_errors_ui(ui);
        for status in controls.status.values().filter(|s| s.unavailable.is_some()) {
            ui.add(egui::Label::new(status.unavailable.as_deref().unwrap()).wrap());
        }
    }

    fn command_errors_ui(&self, ui: &mut Ui) {
        if let Some(error) = &self.error {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
        if let Some(error) = self.transcript.errors.last() {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
    }

    fn command_search_ui(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        Frame::new()
            .fill(theme.bg_input)
            .stroke(Stroke::new(1., theme.border_strong))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(18., 18.), egui::Sense::hover());
                    kiln_common::icons::paint(
                        ui.painter(),
                        rect,
                        kiln_common::icons::Icon::Search,
                        theme.text_dim,
                    );
                    let width = (ui.available_width() - 28.).max(40.);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.controls_ui.command_search)
                            .desired_width(width)
                            .frame(Frame::NONE)
                            .hint_text(tr("명령 및 스킬 검색")),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            true,
                            tr("명령 및 스킬 검색"),
                        )
                    });
                    if !self.controls_ui.command_search.is_empty()
                        && widgets::icon_button(
                            ui,
                            kiln_common::icons::Icon::Close,
                            22.,
                            false,
                            tr("검색 초기화"),
                        )
                        .clicked()
                    {
                        self.controls_ui.command_search.clear();
                        response.request_focus();
                    }
                });
            });
    }

    fn entry_visible(&self, entry: &NativeEntry, skill: bool) -> bool {
        !skill
            || (!entry.terminal_only
                && (self.descriptor.tool != Tool::Codex || entry.path.is_some()))
    }

    fn command_row(&mut self, ui: &mut Ui, entry: &NativeEntry, skill: bool) {
        let theme = Theme::current();
        ui.spacing_mut().item_spacing.y = 2.;
        let id = ui.id().with("expanded");
        let mut expanded = ui
            .ctx()
            .data(|data| data.get_temp::<bool>(id).unwrap_or(false));
        let prefix = if skill && self.descriptor.tool == Tool::Codex {
            "$"
        } else {
            "/"
        };
        let name = format!("{prefix}{}", entry.name.trim_start_matches(['/', '$']));
        let enabled = !self.composing && !self.ime_frame && entry.enabled != Some(false);
        // Reserve one bounded row before placing children. Intrinsic button widths
        // must never widen the ScrollArea's content and then the following row.
        let width = ui.available_width();
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, 28.), egui::Sense::hover());
        let insert_label = tr("초안에 삽입");
        let insert_width = ui
            .fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        insert_label.into(),
                        egui::TextStyle::Button.resolve(ui.style()),
                        theme.text,
                    )
                    .size()
                    .x
                    + 32.
            })
            .min((width * 0.44).max(48.));
        let insert_rect = egui::Rect::from_min_size(
            egui::pos2(row.right() - insert_width, row.center().y - 12.),
            egui::vec2(insert_width, 24.),
        );
        let expand_rect = egui::Rect::from_min_size(row.min, egui::vec2(22., row.height()));
        let expand_response = ui.interact(
            expand_rect,
            ui.id().with("expand-command"),
            egui::Sense::click(),
        );
        expand_response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, expanded, tr("세부 정보"))
        });
        kiln_common::icons::paint(
            ui.painter(),
            expand_rect.shrink2(egui::vec2(3., 6.)),
            if expanded {
                kiln_common::icons::Icon::ChevronDown
            } else {
                kiln_common::icons::Icon::ChevronRight
            },
            theme.text_dim,
        );
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(expand_rect.right() + 4., row.top()),
            egui::pos2(
                (insert_rect.left() - 8.).max(expand_rect.right() + 4.),
                row.bottom(),
            ),
        );
        let name_response = ui.interact(
            name_rect,
            ui.id().with("command-name"),
            egui::Sense::click(),
        );
        name_response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
        let mut name_job = egui::text::LayoutJob::simple(
            name.clone(),
            kiln_common::fonts::semibold(14.),
            theme.text,
            name_rect.width(),
        );
        name_job.wrap.max_rows = 1;
        name_job.halign = egui::Align::Min;
        let name_galley = ui.fonts_mut(|fonts| fonts.layout_job(name_job));
        let name_position = egui::pos2(
            name_rect.left(),
            name_rect.center().y - name_galley.size().y * 0.5,
        );
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(name_rect))
            .galley(name_position, name_galley, theme.text);
        if expand_response.clicked() || name_response.clicked() {
            expanded = !expanded;
        }
        let insert_response = ui
            .add_enabled_ui(enabled, |ui| {
                ui.put(
                    insert_rect,
                    egui::Button::new(insert_label).small().truncate(),
                )
            })
            .inner;
        if insert_response.clicked() {
            self.insert_entry(entry, skill, false);
        }
        if entry.terminal_only {
            ui.small(tr("CLI 명령"));
        }
        if !entry.description.trim().is_empty() {
            summary(ui, &entry.description, 1);
        }
        if expanded {
            Frame::new()
                .fill(theme.bg_input)
                .corner_radius(4)
                .inner_margin(8)
                .show(ui, |ui| {
                    if let Some(display_name) = entry.interface.get("displayName") {
                        ui.strong(display_name);
                    }
                    ui.add(egui::Label::new(&entry.description).wrap());
                    if let Some(hint) = entry
                        .argument_hint
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                    {
                        ui.add(egui::Label::new(format!("{}: {hint}", tr("입력 안내"))).wrap());
                    }
                    for value in [&entry.scope, &entry.origin, &entry.path, &entry.model]
                        .into_iter()
                        .flatten()
                    {
                        ui.add(
                            egui::Label::new(RichText::new(value).small().color(theme.text_dim))
                                .wrap(),
                        );
                    }
                    if !entry.aliases.is_empty() {
                        ui.small(entry.aliases.join(" · "));
                    }
                    if entry.builtin == Some(true) {
                        ui.small(tr("기본 제공"));
                    }
                    if entry.enabled == Some(false) {
                        ui.small(tr("비활성화됨"));
                    }
                });
        }
        ui.ctx().data_mut(|data| data.insert_temp(id, expanded));
        ui.add(egui::Separator::default().spacing(4.));
    }

    pub(super) fn insert_entry(&mut self, entry: &NativeEntry, skill: bool, complete: bool) {
        if self.composing
            || self.ime_frame
            || !self.entry_visible(entry, skill)
            || entry.enabled == Some(false)
        {
            return;
        }
        let prefix = if skill && self.descriptor.tool == Tool::Codex {
            "$"
        } else {
            "/"
        };
        let insertion = format!("{prefix}{} ", entry.name.trim_start_matches(['/', '$']));
        if complete {
            if let Some(index) = completion_start(&self.composer.text) {
                self.composer.text.truncate(index);
            }
        } else if !self.composer.text.is_empty()
            && !self.composer.text.ends_with(char::is_whitespace)
        {
            self.composer.text.push('\n');
        }
        self.composer.text.push_str(&insertion);
        if skill
            && self.descriptor.tool == Tool::Codex
            && let Some(path) = &entry.path
        {
            let input = NativeSkillInput {
                name: entry.name.clone(),
                path: path.clone(),
            };
            if !self.composer.skills.contains(&input) {
                self.composer.skills.push(input);
            }
        }
        self.focus = true;
    }

    fn completion_id(&self) -> egui::Id {
        egui::Id::new(("agent-command-completion", &self.descriptor.id))
    }

    fn completion_entries(&self) -> Vec<CompletionEntry> {
        let Some(start) = completion_start(&self.composer.text) else {
            return vec![];
        };
        let token = &self.composer.text[start..];
        let query = token[1..].to_lowercase();
        let mut seen = BTreeSet::new();
        let mut result = vec![];
        let commands = self.slash_catalog();
        let skills = self
            .transcript
            .controls
            .skills
            .as_deref()
            .unwrap_or_default();
        for (catalog, skill) in [
            (
                commands
                    .iter()
                    .map(|entry| entry.as_ref())
                    .collect::<Vec<_>>(),
                false,
            ),
            (skills.iter().collect(), true),
        ] {
            let prefix = if skill && self.descriptor.tool == Tool::Codex {
                '$'
            } else {
                '/'
            };
            if !token.starts_with(prefix) {
                continue;
            }
            for entry in catalog {
                if self.entry_visible(entry, skill)
                    && entry.enabled != Some(false)
                    && (entry
                        .name
                        .trim_start_matches(['/', '$'])
                        .to_lowercase()
                        .starts_with(&query)
                        || entry.aliases.iter().any(|alias| {
                            alias
                                .trim_start_matches(['/', '$'])
                                .to_lowercase()
                                .starts_with(&query)
                        }))
                    && seen.insert(entry.name.clone())
                {
                    result.push(CompletionEntry {
                        native: entry.clone(),
                        skill,
                    });
                }
            }
        }
        result
    }

    fn completion_state(&self, ui: &Ui, count: usize) -> CompletionState {
        let token = completion_start(&self.composer.text)
            .map(|start| self.composer.text[start..].to_owned())
            .unwrap_or_default();
        let mut state = ui.ctx().data(|data| {
            data.get_temp::<CompletionState>(self.completion_id())
                .unwrap_or_default()
        });
        if state.token != token {
            state = CompletionState {
                token,
                ..Default::default()
            };
        }
        state.active = state.active.min(count.saturating_sub(1));
        state
    }

    fn cursor_to_draft_end(&self, ui: &Ui) {
        let id = ui.id().with("message");
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(self.composer.text.chars().count()),
            )));
        state.store(ui.ctx(), id);
    }

    /// Must run after the IME event guard and before TextEdit or the send shortcut.
    pub(super) fn slash_completion_input(&mut self, ui: &mut Ui, focused: bool) -> bool {
        if self.focus {
            self.cursor_to_draft_end(ui);
        }
        if !focused || self.composing || self.ime_frame {
            return false;
        }
        let entries = self.completion_entries();
        let mut state = self.completion_state(ui, entries.len());
        if entries.is_empty() || state.dismissed {
            ui.ctx()
                .data_mut(|data| data.insert_temp(self.completion_id(), state));
            return false;
        }
        let exact_command = !state.navigated
            && slash_commands::parse(&self.composer.text).is_some_and(|(name, arguments)| {
                arguments.is_empty()
                    && entries.iter().any(|entry| {
                        !entry.skill
                            && (entry.native.name.trim_start_matches('/') == name
                                || entry
                                    .native
                                    .aliases
                                    .iter()
                                    .any(|alias| alias.trim_start_matches('/') == name))
                    })
            });
        let mut handled = false;
        // consume_key deliberately tolerates Shift/Alt; completion accepts plain keys only.
        let plain = ui.input(|i| {
            !i.modifiers.shift && !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt
        });
        if plain {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                state.dismissed = true;
                handled = true;
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                state.active = (state.active + 1) % entries.len();
                state.scroll_to_active = true;
                state.navigated = true;
                handled = true;
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                state.active = (state.active + entries.len() - 1) % entries.len();
                state.scroll_to_active = true;
                state.navigated = true;
                handled = true;
            } else if ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                    || (!exact_command && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            }) {
                let entry = &entries[state.active];
                self.insert_entry(&entry.native, entry.skill, true);
                self.cursor_to_draft_end(ui);
                state.dismissed = true;
                handled = true;
            }
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(self.completion_id(), state));
        handled
    }

    pub(super) fn slash_completion_ui(&mut self, ui: &mut Ui) {
        if !self.composing && !self.ime_frame {
            let entries = self.completion_entries();
            let mut state = self.completion_state(ui, entries.len());
            if !entries.is_empty() && !state.dismissed {
                // egui resolves Tab/Esc focus navigation at the next begin_pass, before
                // our input handler runs. Reserve them while the list is open.
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        ui.id().with("message"),
                        egui::EventFilter {
                            tab: true,
                            escape: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                        },
                    )
                });
                let theme = Theme::current();
                let bounds = ui.ctx().content_rect().shrink(8.);
                let list_height =
                    (entries.len().min(5) as f32 * 44. - 2.).min((bounds.height() * 0.42).max(84.));
                let anchor = ui.min_rect();
                let width = ui.available_width().min(456.).min(bounds.width()).max(120.);
                let position = egui::pos2(
                    anchor.left().min(bounds.right() - width).max(bounds.left()),
                    (anchor.top() - list_height - 56.).max(bounds.top()),
                );
                // Floating above the editor preserves conversation space in short/narrow panes.
                egui::Area::new(self.completion_id().with("popup"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(position)
                    .constrain_to(bounds)
                    .movable(false)
                    .fade_in(false)
                    .show(ui.ctx(), |ui| {
                        ui.set_width(width);
                        Frame::new()
                            .fill(theme.bg_panel)
                            .stroke(Stroke::new(1., theme.border))
                            .corner_radius(6)
                            .inner_margin(4)
                            .show(ui, |ui| {
                                ui.small(tr("명령 및 스킬"));
                                egui::ScrollArea::vertical()
                                    .id_salt("slash-completion-list")
                                    .max_height(list_height)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 2.;
                                        for (index, entry) in entries.iter().enumerate() {
                                            let prefix = if entry.skill
                                                && self.descriptor.tool == Tool::Codex
                                            {
                                                "$"
                                            } else {
                                                "/"
                                            };
                                            let name = format!(
                                                "{prefix}{}",
                                                entry.native.name.trim_start_matches(['/', '$'])
                                            );
                                            let response = completion_row_ui(
                                                ui,
                                                &name,
                                                &entry.native.description,
                                                state.active == index,
                                            );
                                            if state.scroll_to_active && state.active == index {
                                                response.scroll_to_me(Some(egui::Align::Center));
                                            }
                                            if response.clicked() {
                                                self.insert_entry(&entry.native, entry.skill, true);
                                                state.dismissed = true;
                                            }
                                        }
                                    });
                                ui.small(tr("↑↓ 선택 · Tab/Enter 삽입 · Esc 닫기"));
                            });
                    });
            }
            state.scroll_to_active = false;
            ui.ctx()
                .data_mut(|data| data.insert_temp(self.completion_id(), state));
        }
        if !self.composer.skills.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let mut remove = None;
                for (index, skill) in self.composer.skills.iter().enumerate() {
                    if ui
                        .button(format!("{} ×", skill.name))
                        .on_hover_text(tr("네이티브 스킬 입력 제거"))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                }
                if let Some(index) = remove {
                    self.composer.skills.remove(index);
                }
            });
        }
    }

    pub(super) fn command_layouter(
        &self,
    ) -> impl FnMut(&Ui, &dyn egui::TextBuffer, f32) -> std::sync::Arc<egui::Galley> + 'static {
        let mut names = vec![];
        let commands = self.slash_catalog();
        let skills = self
            .transcript
            .controls
            .skills
            .as_deref()
            .unwrap_or_default();
        for (catalog, skill) in [
            (
                commands
                    .iter()
                    .map(|entry| entry.as_ref())
                    .collect::<Vec<_>>(),
                false,
            ),
            (skills.iter().collect(), true),
        ] {
            for entry in catalog
                .into_iter()
                .filter(|entry| self.entry_visible(entry, skill) && entry.enabled != Some(false))
            {
                let prefix = if skill && self.descriptor.tool == Tool::Codex {
                    "$"
                } else {
                    "/"
                };
                names.push(
                    format!("{prefix}{}", entry.name.trim_start_matches(['/', '$'])).to_lowercase(),
                );
            }
        }
        move |ui, text, width| {
            let theme = Theme::current();
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = width;
            for part in text.as_str().split_inclusive(char::is_whitespace) {
                let token = part.trim_end_matches(char::is_whitespace);
                let known = !token.is_empty()
                    && names
                        .iter()
                        .any(|name| name.starts_with(&token.to_lowercase()));
                job.append(
                    token,
                    0.,
                    egui::TextFormat {
                        font_id: egui::TextStyle::Body.resolve(ui.style()),
                        color: if known {
                            theme.accent
                        } else {
                            ui.visuals().text_color()
                        },
                        background: if known {
                            widgets::tint(theme.accent, 0.10)
                        } else {
                            egui::Color32::TRANSPARENT
                        },
                        ..Default::default()
                    },
                );
                job.append(
                    &part[token.len()..],
                    0.,
                    egui::TextFormat {
                        font_id: egui::TextStyle::Body.resolve(ui.style()),
                        color: ui.visuals().text_color(),
                        ..Default::default()
                    },
                );
            }
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        }
    }
}

fn completion_start(text: &str) -> Option<usize> {
    let start = text
        .rfind(char::is_whitespace)
        .map(|i| i + text[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    text[start..].starts_with(['/', '$']).then_some(start)
}
/// Paint two leading-edge text lines inside one full-width selectable target.
fn completion_row_ui(ui: &mut Ui, name: &str, description: &str, selected: bool) -> egui::Response {
    let theme = Theme::current();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    let fill = if selected {
        widgets::tint(theme.accent, 0.16)
    } else if response.hovered() {
        theme.bg_hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 4, fill);
    let text_rect = rect.shrink2(egui::vec2(8., 0.));
    let painter = ui
        .painter()
        .with_clip_rect(ui.clip_rect().intersect(text_rect));
    for (text, top, font, color) in [
        (name, 4., kiln_common::fonts::semibold(13.), theme.text),
        (
            description.lines().next().unwrap_or_default(),
            23.,
            egui::FontId::proportional(11.5),
            theme.text_dim,
        ),
    ] {
        let mut job = egui::text::LayoutJob::simple(text.into(), font, color, text_rect.width());
        job.wrap.max_rows = 1;
        job.halign = egui::Align::Min;
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        painter.galley(
            egui::pos2(text_rect.left(), rect.top() + top),
            galley,
            color,
        );
    }
    response
}

fn summary(ui: &mut Ui, text: &str, rows: usize) {
    let mut job = egui::text::LayoutJob::simple(
        text.into(),
        egui::TextStyle::Body.resolve(ui.style()),
        Theme::current().text_dim,
        ui.available_width(),
    );
    job.wrap.max_rows = rows;
    ui.add(egui::Label::new(job).wrap());
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
