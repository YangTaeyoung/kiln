//! Persistent form text. Conversion to protocol values belongs to explicit actions.
use super::*;

#[derive(Default)]
pub(super) struct EditingText {
    pub text: String,
    composing: bool,
    committed: Option<String>,
    ime_frame: Option<u64>,
}
impl EditingText {
    pub fn new(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
    pub fn ready(&self, ctx: &egui::Context) -> bool {
        !self.composing && self.ime_frame != Some(ctx.cumulative_frame_nr())
    }
    fn cancel(&mut self) {
        if let Some(text) = self.committed.take() {
            self.text = text;
        }
        self.composing = false;
    }
    pub fn show(&mut self, ui: &mut Ui, title: &str, multiline: bool, limit: usize) {
        let id = ui.id().with("editing-text");
        let frame = ui.ctx().cumulative_frame_nr();
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused && self.composing {
            self.cancel();
            self.ime_frame = Some(frame);
        }
        let mut disabled = false;
        if focused {
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                            self.composing = !text.is_empty();
                            self.ime_frame = Some(frame);
                        }
                        egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                            self.composing = false;
                            self.ime_frame = Some(frame);
                        }
                        #[allow(deprecated)]
                        egui::Event::Ime(egui::ImeEvent::Disabled) => {
                            disabled = true;
                            self.ime_frame = Some(frame);
                        }
                        _ => {}
                    }
                }
            });
        }
        let edit = if multiline {
            egui::TextEdit::multiline(&mut self.text)
        } else {
            egui::TextEdit::singleline(&mut self.text)
        };
        let output = edit
            .id(id)
            .desired_width(f32::INFINITY)
            .desired_rows(2)
            .char_limit(limit)
            .show(ui);
        output.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), title)
        });
        // Adding a label must retain the native multiline accessibility role.
        if multiline {
            ui.ctx().accesskit_node_builder(output.response.id, |node| {
                node.set_role(egui::accesskit::Role::MultilineTextInput);
            });
        }
        if self.composing
            && let Some(range) = output.cursor_range
        {
            let range = range.as_sorted_char_range();
            self.committed = Some(
                self.text
                    .chars()
                    .enumerate()
                    .filter_map(|(i, c)| (!(range.start.0..range.end.0).contains(&i)).then_some(c))
                    .collect(),
            );
        }
        if disabled || (output.response.lost_focus() && self.composing) {
            self.cancel();
            self.ime_frame = Some(frame);
        }
        if !self.composing {
            self.committed = None;
        }
    }
    /// A deliberate Apply/launch boundary; never rewrite the editing buffer.
    pub fn lines(&self) -> Vec<String> {
        self.text
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }
}
