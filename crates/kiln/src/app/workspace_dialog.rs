//! Named workspaces share a directory, never terminal/pane identities.
use super::*;
use egui::{Frame, Margin, RichText, Stroke};
use kiln_common::icons::Icon;
use kiln_common::{
    fonts,
    i18n::tr,
    widgets::{self, ButtonKind},
};

pub(super) struct WorkspaceDialog {
    folder: String,
    name: String,
    focus_name: bool,
}

fn folder_path(folder: &str) -> PathBuf {
    let folder = folder.trim();
    let path = if folder == "~" || folder.starts_with("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(folder.strip_prefix("~/").unwrap_or(""))
    } else {
        PathBuf::from(folder)
    };
    normalize_path(path.canonicalize().unwrap_or(path))
}

pub(super) fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control)
}

impl KilnApp {
    pub(super) fn open_workspace_dialog(&mut self, root: PathBuf) {
        let root = normalize_path(root.canonicalize().unwrap_or(root));
        let base = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| tr("워크스페이스").into());
        let mut name = base.clone();
        let mut suffix = 2;
        while self
            .workspaces
            .iter()
            .any(|w| w.name == name && w.tools.canonical_root() == root)
        {
            name = format!("{base} {suffix}");
            suffix += 1;
        }
        self.workspace_dialog = Some(WorkspaceDialog {
            folder: root.to_string_lossy().into_owned(),
            name,
            focus_name: true,
        });
        self.focus_terminal = false;
    }

    pub(super) fn ui_workspace_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.workspace_dialog.take() else {
            return;
        };
        let root = folder_path(&draft.folder);
        let existing: Vec<_> = self
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, w)| w.tools.canonical_root() == root)
            .map(|(i, w)| (i, w.name.clone()))
            .collect();
        let duplicate=existing.iter().any(|(_,name)|name==draft.name.trim());
        let theme = self.theme;
        let mut choose = None;
        let mut create = false;
        let mut cancel = false;
        let width = (ctx.content_rect().width() - 48.).clamp(240., 380.);
        let frame = Frame::new()
            .fill(theme.bg_panel)
            .stroke(Stroke::new(1., theme.border_strong))
            .corner_radius(12)
            .inner_margin(Margin::same(16));
        let response = egui::Modal::new(egui::Id::new("new-workspace"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(RichText::new(tr("새 워크스페이스")).font(fonts::semibold(17.)));
                ui.add_space(8.);
                egui::ScrollArea::vertical()
                    .id_salt("new-workspace-fields")
                    .max_height((ctx.content_rect().height() - 156.).max(80.))
                    .show(ui, |ui| {
                        let label = ui.label(tr("워크스페이스 이름"));
                        let input = ui
                            .add(
                                egui::TextEdit::singleline(&mut draft.name)
                                    .desired_width(f32::INFINITY),
                            )
                            .labelled_by(label.id);
                        if draft.focus_name {
                            input.request_focus();
                            draft.focus_name = false;
                        }
                        create = input.has_focus()
                            && ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            });
                        ui.add_space(6.);
                        let label = ui.label(tr("폴더"));
                        ui.horizontal(|ui| {
                            let field_width = (ui.available_width() - 32.).max(80.);
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.folder)
                                    .desired_width(field_width),
                            )
                            .labelled_by(label.id);
                            if widgets::icon_button(ui, Icon::Folder, 26., false, tr("폴더 선택"))
                                .clicked()
                                && let Some(path) =
                                    rfd::FileDialog::new().set_directory(&root).pick_folder()
                            {
                                draft.folder = path.to_string_lossy().into_owned();
                            }
                        });
                        if !existing.is_empty() {
                            ui.add_space(10.);
                            ui.label(
                                RichText::new(tr("이 폴더의 워크스페이스"))
                                    .small()
                                    .color(theme.text_dim),
                            );
                            for (i, name) in &existing {
                                if ui.add_sized([ui.available_width(),30.], egui::Button::new(name).truncate()).on_hover_text(name).clicked()
                                {
                                    choose = Some(*i);
                                }
                            }
                        }
                        if duplicate {
                            ui.colored_label(theme.red,tr("이 폴더에 같은 이름의 워크스페이스가 있습니다."));
                        } else if !valid_name(&draft.name) {
                            ui.colored_label(
                                theme.red,
                                tr("워크스페이스 이름을 1~100자로 입력하세요."),
                            );
                        } else if !folder_path(&draft.folder).is_dir() {
                            ui.colored_label(theme.red, tr("기존 폴더를 선택하세요."));
                        }
                    });
                ui.add_space(10.);
                ui.horizontal(|ui| {
                    cancel = widgets::button(ui, tr("취소"), ButtonKind::Secondary).clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let valid = valid_name(&draft.name) && folder_path(&draft.folder).is_dir() && !existing.iter().any(|(_,name)|name==draft.name.trim());
                        ui.add_enabled_ui(valid, |ui| {
                            create |=
                                widgets::button(ui, tr("만들기"), ButtonKind::Primary).clicked();
                        });
                        create &= valid;
                    });
                });
            });
        if let Some(index) = choose {
            self.actions.push(Action::SelectWorkspace(index));
            self.focus_terminal = true;
        } else if create {
            self.actions.push(Action::CreateWorkspace {
                root: folder_path(&draft.folder),
                name: draft.name.trim().into(),
            });
        } else if cancel || response.should_close() {
            self.focus_terminal = true;
        } else {
            self.workspace_dialog = Some(draft);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_names_have_no_control_characters_or_empty_values() {
        assert!(valid_name(" API review "));
        for value in ["", "  ", "name\nline", "bad\0", &"가".repeat(101)] {
            assert!(!valid_name(value));
        }
    }
}
