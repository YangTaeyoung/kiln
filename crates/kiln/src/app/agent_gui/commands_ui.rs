//! Discoverable native catalogs. Selection edits a draft; only Send executes it.
use super::*;
use kiln_common::i18n::tr;
impl AgentPane {
    pub(super) fn commands_ui(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.;
        ui.heading(tr("명령 및 스킬"));
        ui.add(egui::Label::new(tr("초안에 삽입한 뒤 보내기를 눌러 실행하세요.")).wrap());
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.controls_ui.command_search)
                .desired_width(f32::INFINITY)
                .hint_text(tr("명령 및 스킬 검색")),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, tr("명령 및 스킬 검색"))
        });
        ui.horizontal_wrapped(|ui| {
            if ui
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
        let controls = self.transcript.controls.clone();
        for (title, catalog, skill) in [
            ("명령", &controls.commands, false),
            ("스킬", &controls.skills, true),
        ] {
            ui.add_space(8.);
            ui.strong(tr(title));
            match catalog {
                None => {
                    ui.label(tr("목록을 아직 받지 못했습니다."));
                }
                Some(entries) if entries.is_empty() => {
                    ui.label(tr("제공된 항목이 없습니다."));
                }
                Some(entries) => {
                    let query = self.controls_ui.command_search.to_lowercase();
                    let filtered = entries
                        .iter()
                        .filter(|entry| {
                            format!(
                                "{} {} {}",
                                entry.name,
                                entry.description,
                                entry.aliases.join(" ")
                            )
                            .to_lowercase()
                            .contains(&query)
                        })
                        .collect::<Vec<_>>();
                    if filtered.is_empty() {
                        ui.label(tr("검색 결과가 없습니다."));
                    }
                    for entry in filtered {
                        ui.push_id((title, &entry.name), |ui| {
                            self.command_card(ui, entry, skill)
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
            if let Some(entries) = entries {
                egui::CollapsingHeader::new(tr(title)).show(ui, |ui| {
                    for entry in entries {
                        ui.strong(&entry.name);
                        ui.add(egui::Label::new(&entry.description).wrap());
                    }
                    if entries.is_empty() {
                        ui.label(tr("제공된 항목이 없습니다."));
                    }
                });
            }
        }
        if let Some(error) = &self.error {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
        if let Some(error) = self.transcript.errors.last() {
            ui.add(egui::Label::new(RichText::new(error).color(Theme::current().red)).wrap());
        }
        for status in controls.status.values().filter(|s| s.unavailable.is_some()) {
            ui.add(egui::Label::new(status.unavailable.as_deref().unwrap()).wrap());
        }
    }
    fn command_card(&mut self, ui: &mut Ui, entry: &NativeEntry, skill: bool) {
        let theme = Theme::current();
        Frame::new()
            .fill(theme.bg_panel)
            .stroke(Stroke::new(1., theme.border))
            .corner_radius(8)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.;
                ui.horizontal_wrapped(|ui| {
                    ui.strong(entry.interface.get("displayName").unwrap_or(&entry.name));
                    let enabled = !self.composing
                        && !self.ime_frame
                        && !entry.terminal_only
                        && entry.enabled != Some(false)
                        && (!skill || self.descriptor.tool != Tool::Codex || entry.path.is_some());
                    if ui
                        .add_enabled(enabled, egui::Button::new(tr("초안에 삽입")))
                        .clicked()
                    {
                        self.insert_entry(entry, skill, false);
                    }
                    ui.menu_button(tr("세부 정보"), |ui| {
                        ui.set_max_width(280.);
                        egui::ScrollArea::vertical()
                            .max_height(300.)
                            .show(ui, |ui| {
                                ui.strong(&entry.name);
                                ui.add(egui::Label::new(&entry.description).wrap());
                                if let Some(hint) = entry
                                    .argument_hint
                                    .as_deref()
                                    .filter(|s| !s.trim().is_empty())
                                {
                                    ui.add(
                                        egui::Label::new(format!("{}: {hint}", tr("입력 안내")))
                                            .wrap(),
                                    );
                                }
                                if let Some(scope) = &entry.scope {
                                    ui.small(scope);
                                }
                                if let Some(origin) = &entry.origin {
                                    ui.small(origin);
                                }
                            });
                    });
                    if let Some(scope) = &entry.scope {
                        ui.small(scope);
                    }
                    if entry.builtin == Some(true) {
                        ui.small(tr("기본 제공"));
                    }
                    if entry.enabled == Some(false) {
                        ui.small(tr("비활성화됨"));
                    }
                    if entry.terminal_only {
                        ui.small(tr("터미널 전용"));
                    }
                });
                if !entry.description.trim().is_empty() {
                    summary(ui, &entry.description, 2);
                }
                if let Some(hint) = entry
                    .argument_hint
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                {
                    summary(ui, &format!("{}: {hint}", tr("입력 안내")), 1);
                }
            });
        ui.add_space(4.);
    }
    pub(super) fn insert_entry(&mut self, entry: &NativeEntry, skill: bool, complete: bool) {
        if self.composing || self.ime_frame || entry.terminal_only || entry.enabled == Some(false) {
            return;
        }
        let prefix = if skill && self.descriptor.tool == Tool::Codex {
            "$"
        } else {
            "/"
        };
        let insertion = format!("{prefix}{} ", entry.name.trim_start_matches(['/', '$']));
        if complete {
            if let Some(index) = slash_start(&self.composer.text) {
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
    pub(super) fn slash_completion_ui(&mut self, ui: &mut Ui) {
        if !self.composing
            && !self.ime_frame
            && let Some(start) = slash_start(&self.composer.text)
        {
            let query = self.composer.text[start + 1..].to_lowercase();
            let entries = self
                .transcript
                .controls
                .commands
                .as_ref()
                .into_iter()
                .flatten()
                .filter(|e| {
                    !e.terminal_only
                        && e.enabled != Some(false)
                        && e.name
                            .trim_start_matches('/')
                            .to_lowercase()
                            .starts_with(&query)
                })
                .take(4)
                .cloned()
                .collect::<Vec<_>>();
            if !entries.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for entry in &entries {
                        if ui
                            .button(format!(
                                "{} /{}",
                                tr("삽입"),
                                entry.name.trim_start_matches('/')
                            ))
                            .on_hover_text(&entry.description)
                            .clicked()
                        {
                            self.insert_entry(entry, false, true);
                        }
                    }
                });
            }
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
}
fn slash_start(text: &str) -> Option<usize> {
    let start = text
        .rfind(char::is_whitespace)
        .map(|i| i + text[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    text[start..].starts_with('/').then_some(start)
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
