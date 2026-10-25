//! Cursor-attached shell completion with local arguments and bounded CLI help discovery.
use std::path::Path;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub hint: String,
    pub kind: ChoiceKind,
    pub label: String,
    pub description: String,
    pub buffer: String,
    pub cursor: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKind {
    Command,
    Subcommand,
    Option,
    Value,
    Directory,
    File,
    Branch,
    Script,
}
mod discovery;
mod suggestions;
/// Simple words only. Shell expressions are deliberately left to native completion.
fn words(input: &str) -> Option<Vec<(usize, String)>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut active = false;
    let mut chars = input.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if !active && !ch.is_whitespace() {
            start = i;
            active = true;
        }
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            // Double quotes preserve a backslash before ordinary characters.
            // Only $, `, ", \\ and newline can be escaped in this context.
            if quote == Some('"')
                && chars
                    .peek()
                    .is_some_and(|(_, next)| !matches!(next, '$' | '`' | '"' | '\\' | '\n'))
            {
                word.push(ch);
                continue;
            }
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && matches!(ch, '$' | '`') {
                return None;
            }
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            continue;
        }
        if matches!(ch, ';' | '|' | '&' | '$' | '`' | '(' | ')' | '<' | '>') {
            return None;
        }
        // Leave home expansion to Zsh, while quoted/escaped literal ~ paths
        // remain traversable after accepting a directory suggestion.
        if ch == '~' && (word.is_empty() || word.ends_with('=')) {
            return None;
        }
        if ch.is_whitespace() {
            if active {
                result.push((start, std::mem::take(&mut word)));
                active = false;
            }
        } else {
            word.push(ch);
        }
    }
    if escaped {
        return None;
    }
    if active {
        result.push((start, word));
    } else {
        result.push((input.len(), String::new()));
    }
    Some(result)
}
fn quoted(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || "/_-.:@%".contains(c))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
pub fn candidates(
    buffer: &str,
    cursor: usize,
    cwd: &Path,
    native_commands: &[String],
) -> Vec<Choice> {
    suggestions::candidates(buffer, cursor, cwd, native_commands)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_quotes_unicode_and_no_shell_execution() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("한 글")).unwrap();
        std::fs::create_dir(d.path().join("O'Reilly")).unwrap();
        let rows = candidates("cd 한", 4, d.path(), &[]);
        assert_eq!(rows[0].buffer, "cd '한 글/'");
        assert!(!rows[0].buffer.contains('\n'));
        assert_eq!(
            candidates("cd 'O", 5, d.path(), &[])[0].buffer,
            "cd 'O'\\''Reilly/'"
        );
        assert!(candidates("cd $(evil)", 10, d.path(), &[]).is_empty());
        assert!(candidates("cd \"$HOME", 9, d.path(), &[]).is_empty());
        std::fs::create_dir(d.path().join("~literal")).unwrap();
        assert_eq!(
            candidates("cd '~", 5, d.path(), &[])[0].buffer,
            "cd '~literal/'"
        );
        assert!(candidates("git status", 3, d.path(), &[]).is_empty());
        assert_eq!(
            candidates("git st", 6, d.path(), &[])[0].buffer,
            "git status"
        );
    }

    #[test]
    fn command_context_offers_nested_subcommands_and_only_relevant_options() {
        let dir = tempfile::tempdir().unwrap();
        for (input, wanted) in [
            ("git pu", "push"),
            ("git commit --a", "--amend"),
            ("docker compose u", "up"),
            ("docker compose up --d", "--detach"),
            ("cargo test --n", "--no-default-features"),
            ("pnpm run ", "build"),
        ] {
            std::fs::write(
                dir.path().join("package.json"),
                r#"{"scripts":{"build":"echo fixture"}}"#,
            )
            .unwrap();
            let rows = candidates(input, input.chars().count(), dir.path(), &[]);
            assert!(
                rows.iter().any(|r| r.label == wanted),
                "{input:?}: {rows:?}"
            );
        }
        for input in [
            "git commit -m secret",
            "git commit --message=secret",
            "git switch $(touch bad)",
            "TOKEN=secret git ",
        ] {
            assert!(
                candidates(input, input.chars().count(), dir.path(), &[]).is_empty(),
                "{input:?}"
            );
        }
        let input = "git commit --amend --a";
        assert!(
            !candidates(input, input.chars().count(), dir.path(), &[])
                .iter()
                .any(|r| r.label == "--amend")
        );
    }

    #[test]
    fn quoted_inline_options_replace_the_complete_shell_word() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plain notes.txt"), "fixture").unwrap();
        for input in [
            "docker compose \"--file=pl",
            "docker compose '--file=pl",
            "docker compose --file=\"pl",
        ] {
            let rows = candidates(input, input.chars().count(), dir.path(), &[]);
            assert_eq!(
                rows[0].buffer, "docker compose --file='plain notes.txt'",
                "{input}"
            );
        }
    }

    #[test]
    fn literal_equals_in_positional_and_option_paths_remain_completable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a=b-dir")).unwrap();
        std::fs::write(dir.path().join("a=b.txt"), "fixture").unwrap();
        for (input, expected) in [
            ("cd a=b", "cd 'a=b-dir/'"),
            (
                "docker compose --file a=b.t",
                "docker compose --file 'a=b.txt'",
            ),
            (
                "docker compose --file=a=b.t",
                "docker compose --file='a=b.txt'",
            ),
        ] {
            let rows = candidates(input, input.chars().count(), dir.path(), &[]);
            assert!(
                rows.iter().any(|r| r.buffer == expected),
                "{input}: {rows:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn double_quoted_literal_backslashes_follow_shell_quoting() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("back\\slash")).unwrap();
        std::fs::create_dir(dir.path().join("backslash")).unwrap();
        let input = "cd \"back\\slash";
        let rows = candidates(input, input.chars().count(), dir.path(), &[]);
        assert_eq!(rows[0].buffer, "cd 'back\\slash/'");
    }

    #[test]
    fn quoted_literal_tilde_directories_continue_without_home_expansion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("~literal/child")).unwrap();
        let input = "cd '~literal/'";
        let rows = candidates(input, input.chars().count(), dir.path(), &[]);
        assert_eq!(rows[0].buffer, "cd '~literal/child/'");
        for input in ["cd ~literal/", "docker compose --file=~/"] {
            assert!(candidates(input, input.chars().count(), dir.path(), &[]).is_empty());
        }
    }

    #[test]
    fn local_project_arguments_are_read_without_running_scripts_or_commands() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git/refs/heads/feature")).unwrap();
        std::fs::write(
            dir.path().join(".git/refs/heads/feature/checkout"),
            "0123456789012345678901234567890123456789\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev:web":"touch must-not-exist","build":"echo fixture"}}"#,
        )
        .unwrap();
        for (input, expected) in [
            ("git switch fe", "git switch feature/checkout"),
            ("npm run dev", "npm run dev:web"),
        ] {
            let rows = candidates(input, input.chars().count(), dir.path(), &[]);
            assert!(
                rows.iter().any(|r| r.buffer == expected),
                "{input:?}: {rows:?}"
            );
        }
        assert!(!dir.path().join("must-not-exist").exists());
        std::fs::write(dir.path().join("plain notes.txt"), "fixture").unwrap();
        let input = "cat pl";
        assert_eq!(
            candidates(input, input.len(), dir.path(), &[])[0].buffer,
            "cat 'plain notes.txt'"
        );
    }
}

type Work = (
    kiln_proto::ShellCompletion,
    std::sync::mpsc::Sender<Vec<Choice>>,
);
fn enqueue(request: kiln_proto::ShellCompletion, tx: std::sync::mpsc::Sender<Vec<Choice>>) {
    static WORKER: std::sync::OnceLock<
        std::sync::Arc<(std::sync::Mutex<Option<Work>>, std::sync::Condvar)>,
    > = std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(None::<Work>),
            std::sync::Condvar::new(),
        ));
        let c = shared.clone();
        std::thread::spawn(move || {
            loop {
                let job = {
                    let (work, signal) = (&c.0, &c.1);
                    let mut slot = work.lock().unwrap();
                    while slot.is_none() {
                        slot = signal.wait(slot).unwrap();
                    }
                    slot.take().unwrap()
                };
                let (r, tx) = job;
                let _ = tx.send(candidates(
                    &r.buffer,
                    r.cursor,
                    Path::new(&r.cwd),
                    &r.commands,
                ));
            }
        });
        shared
    });
    *worker.0.lock().unwrap() = Some((request, tx));
    worker.1.notify_one();
}
#[derive(Default)]
pub struct Popup {
    request: Option<kiln_proto::ShellCompletion>,
    rows: Vec<Choice>,
    rx: Option<std::sync::mpsc::Receiver<Vec<Choice>>>,
    selected: usize,
    closed_revision: Option<u64>,
    navigated: bool,
    #[cfg(test)]
    last_rect: Option<egui::Rect>,
}
impl Popup {
    pub fn close(&mut self) {
        if let Some(request) = &self.request {
            self.closed_revision = Some(request.revision);
        }
        self.request = None;
        self.rows.clear();
        self.rx = None;
        self.navigated = false;
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        conn: &mut super::conn::Conn,
        session: kiln_proto::SessionId,
        allowed: bool,
        preview_enabled: bool,
        rect: egui::Rect,
        anchor: egui::Pos2,
        font_size: f32,
    ) {
        let snapshot = conn.shell_completions.get(&session).cloned();
        if !allowed || snapshot.is_none() {
            self.close();
            return;
        }
        let snapshot = snapshot.unwrap();
        if !preview_enabled && !snapshot.explicit {
            self.close();
            return;
        }
        if self.closed_revision == Some(snapshot.revision) {
            return;
        }
        if self
            .request
            .as_ref()
            .is_none_or(|r| r.revision != snapshot.revision)
        {
            let (tx, rx) = std::sync::mpsc::channel();
            enqueue(snapshot.clone(), tx);
            self.request = Some(snapshot);
            self.rx = Some(rx);
            self.rows.clear();
            self.selected = 0;
            self.navigated = false;
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(rows) => {
                    self.rows = rows;
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.close();
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let explicit = self.request.as_ref().is_some_and(|r| r.explicit);
        let engaged = explicit || self.navigated;
        let loading = explicit && self.rx.is_some();
        let ready = !self.rows.is_empty() && self.rx.is_none();
        let terminal_focus = ui.memory(|m| m.focused());
        let mut moved = false;
        let mut accept = false;
        let mut dismiss = false;
        ui.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() => {
                    if loading
                        && matches!(
                            key,
                            egui::Key::Enter
                                | egui::Key::Tab
                                | egui::Key::ArrowUp
                                | egui::Key::ArrowDown
                        )
                    {
                        return false;
                    }
                    match key {
                        egui::Key::Tab if ready => {
                            accept = true;
                            false
                        }
                        egui::Key::Enter if ready && engaged => {
                            accept = true;
                            false
                        }
                        egui::Key::ArrowDown if ready => {
                            if engaged {
                                self.selected = (self.selected + 1).min(self.rows.len() - 1);
                            }
                            self.navigated = true;
                            moved = true;
                            false
                        }
                        egui::Key::ArrowUp if ready && engaged => {
                            self.selected = self.selected.saturating_sub(1);
                            moved = true;
                            false
                        }
                        egui::Key::Escape if ready || explicit => {
                            dismiss = true;
                            false
                        }
                        _ => {
                            dismiss = true;
                            true
                        }
                    }
                }
                egui::Event::Text(_)
                | egui::Event::Paste(_)
                | egui::Event::Ime(_)
                | egui::Event::Key { pressed: true, .. } => {
                    dismiss = true;
                    true
                }
                _ => true,
            });
        });
        if dismiss {
            self.close();
            return;
        }
        let theme = kiln_common::Theme::current();
        if !explicit && self.rows.is_empty() {
            if self.rx.is_some() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(30));
            }
            return;
        }
        let frame = egui::Frame::new()
            .fill(theme.bg_elevated)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(6)
            .inner_margin(4.0);
        let margin = frame.total_margin().sum();
        let outer_width = (rect.width() - 8.0).clamp(1.0, 400.0);
        let row_height = ui
            .fonts_mut(|f| f.row_height(&egui::FontId::monospace(font_size)))
            .max(16.0)
            + 8.0;
        let requested_height = self.rows.len().clamp(1, 7) as f32 * row_height + 42.0 + margin.y;
        let outer_height = requested_height.min((rect.height() - 8.0).max(1.0));
        let width = (outer_width - margin.x).max(1.0);
        let content_height = (outer_height - margin.y).max(1.0);
        let pos = egui::pos2(
            anchor.x.clamp(
                rect.left() + 4.0,
                (rect.right() - outer_width - 4.0).max(rect.left() + 4.0),
            ),
            if anchor.y + outer_height + 2.0 <= rect.bottom() {
                anchor.y + 2.0
            } else {
                (anchor.y - outer_height - row_height + 6.0).max(rect.top() + 4.0)
            },
        );
        let popup_response = egui::Area::new(egui::Id::new(("shell-completion", session)))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain_to(rect)
            .show(ui.ctx(), |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width);
                    ui.set_min_height(content_height);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let list_height = (content_height - 42.0).max(1.0);
                    egui::ScrollArea::vertical()
                        .id_salt("completion-list")
                        .max_height(list_height)
                        .auto_shrink([false, false])
                        .min_scrolled_height(0.0)
                        .show(ui, |ui| {
                            ui.set_width(width);
                            if self.rx.is_some() {
                                ui.label(kiln_common::i18n::tr("추천 항목을 찾는 중…"));
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
                            } else if self.rows.is_empty() {
                                ui.label(kiln_common::i18n::tr("추천 항목이 없습니다. Esc로 닫거나 Tab으로 셸 자동완성을 사용하세요."));
                            }
                            for (i, row) in self.rows.iter().enumerate() {
                                let response = completion_row(ui, row, self.selected == i, width, row_height, font_size, &theme);
                                if moved && self.selected == i { response.scroll_to_me(Some(egui::Align::Center)); }
                                if response.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) { self.selected = i; }
                                if response.clicked() { self.selected = i; accept = true; }
                            }
                        });
                    ui.separator();
                    if let Some(row) = self.rows.get(self.selected) {
                        ui.add(egui::Label::new(egui::RichText::new(kiln_common::i18n::tr(&row.description))
                            .size(12.0).color(theme.text_dim)).truncate());
                    }
                    let keys = if engaged { "↑↓ 선택 · Tab/Enter 삽입 · Esc 닫기" }
                        else { "Tab 삽입 · ↓ 선택 · Enter 실행 · Esc 닫기" };
                    ui.add(egui::Label::new(egui::RichText::new(kiln_common::i18n::tr(keys))
                        .size(11.0).color(theme.text_faint)).truncate());
                });
            });
        #[cfg(test)]
        {
            self.last_rect = Some(popup_response.response.rect);
        }
        #[cfg(not(test))]
        let _ = popup_response;
        if accept {
            if let Some(id) = terminal_focus {
                ui.memory_mut(|m| m.request_focus(id));
            }
            if let (Some(request), Some(row)) = (&self.request, self.rows.get(self.selected)) {
                conn.send(kiln_proto::ClientMsg::ApplyShellCompletion {
                    session,
                    revision: request.revision,
                    buffer: row.buffer.clone(),
                    cursor: row.cursor,
                });
            }
            self.close();
        }
    }
}

fn completion_row(
    ui: &mut egui::Ui,
    row: &Choice,
    selected: bool,
    width: f32,
    height: f32,
    font_size: f32,
    theme: &kiln_common::Theme,
) -> egui::Response {
    use kiln_common::icons::{self, Icon};
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            &row.label,
        )
    });
    if ui.is_rect_visible(rect) {
        if selected {
            ui.painter()
                .rect_filled(rect, 3, ui.visuals().selection.bg_fill);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 3, theme.bg_hover);
        }
        let icon = match row.kind {
            ChoiceKind::Directory => Icon::Folder,
            ChoiceKind::File => Icon::File,
            ChoiceKind::Branch => Icon::Branch,
            ChoiceKind::Script => Icon::Play,
            ChoiceKind::Option => Icon::Filter,
            ChoiceKind::Value => Icon::Tag,
            _ => Icon::Terminal,
        };
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 12.0, rect.center().y),
                egui::vec2(14.0, 14.0),
            ),
            icon,
            if selected {
                ui.visuals().selection.stroke.color
            } else {
                theme.text_dim
            },
        );
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::monospace(font_size);
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            theme.text
        };
        job.append(
            &row.label,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
        if !row.hint.is_empty() {
            job.append(
                &format!(" {}", row.hint),
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: if selected {
                        color.gamma_multiply(0.75)
                    } else {
                        theme.text_faint
                    },
                    ..Default::default()
                },
            );
        }
        job.wrap = egui::text::TextWrapping {
            max_width: (width - 32.0).max(1.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(
            egui::pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    response
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use egui_kittest::Harness;
    #[test]
    fn preview_and_empty_results_preserve_native_keys_and_themes_fit() {
        for theme_name in ["kiln-dark", "kiln-light"] {
            kiln_common::Theme::set_current(theme_name);
            for (explicit, with_rows, loading, consumed) in [
                (false, true, false, true),
                (true, false, false, false),
                (true, true, false, true),
                (true, false, true, true),
            ] {
                let request = kiln_proto::ShellCompletion {
                    explicit,
                    revision: 1,
                    buffer: "git st".into(),
                    cursor: 6,
                    cwd: "/".into(),
                    request_file: String::new(),
                    commands: vec![],
                };
                let mut conn = super::super::conn::Conn::offline(egui::Context::default());
                conn.shell_completions.insert(1, request.clone());
                let rows = if with_rows {
                    candidates("git st", 6, Path::new("/"), &[])
                } else {
                    vec![]
                };
                let (_keep_alive, rx) = std::sync::mpsc::channel();
                let popup = Popup {
                    rx: loading.then_some(rx),
                    request: Some(request),
                    rows,
                    ..Default::default()
                };
                let theme = kiln_common::Theme::by_name(theme_name);
                let mut fonts = false;
                let mut h = Harness::builder().with_size([360.0, 240.0]).build_ui_state(
                    move |ui, state: &mut (Popup, super::super::conn::Conn, bool)| {
                        if !fonts {
                            kiln_common::fonts::install(ui.ctx());
                            fonts = true;
                            return;
                        }
                        theme.apply(ui.ctx());
                        let rect = ui.available_rect_before_wrap();
                        state.0.ui(
                            ui,
                            &mut state.1,
                            1,
                            true,
                            true,
                            rect,
                            rect.left_top() + egui::vec2(12.0, 24.0),
                            13.5,
                        );
                        state.2 = ui.input(|i| {
                            i.events.iter().any(|e| {
                                matches!(
                                    e,
                                    egui::Event::Key {
                                        key: egui::Key::Tab,
                                        pressed: true,
                                        ..
                                    }
                                )
                            })
                        });
                    },
                    (popup, conn, false),
                );
                h.run_steps(3);
                if explicit && with_rows {
                    h.render()
                        .unwrap()
                        .save(
                            std::env::temp_dir()
                                .join(format!("kiln-shell-completion-{theme_name}.png")),
                        )
                        .unwrap();
                }
                h.event(egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
                h.step();
                assert_eq!(
                    h.state().2,
                    !consumed,
                    "explicit={explicit}, rows={with_rows}"
                );
            }
        }
    }
}

#[cfg(test)]
mod short_popup_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    #[test]
    fn short_pane_scrolls_whole_popup_and_keyboard_selection_stays_visible() {
        for theme_name in ["kiln-dark", "kiln-light"] {
            kiln_common::Theme::set_current(theme_name);
            for height in [100.0, 136.0] {
                let r = kiln_proto::ShellCompletion {
                    explicit: true,
                    revision: 10,
                    buffer: "git ".into(),
                    cursor: 4,
                    cwd: "/".into(),
                    request_file: String::new(),
                    commands: vec![],
                };
                let rows = (0..12)
                    .map(|i| Choice {
                        label: format!("fixture-option-{i:02}"),
                        hint: String::new(),
                        kind: ChoiceKind::Option,
                        description: "작업 트리 변경 사항 보기".into(),
                        buffer: format!("git fixture-{i}"),
                        cursor: 13,
                    })
                    .collect();
                let popup = Popup {
                    request: Some(r.clone()),
                    rows,
                    ..Default::default()
                };
                let mut conn = super::super::conn::Conn::offline(egui::Context::default());
                conn.shell_completions.insert(1, r);
                let theme = kiln_common::Theme::by_name(theme_name);
                let mut fonts = false;
                let mut h = Harness::builder()
                    .with_size([320.0, height])
                    .build_ui_state(
                        move |ui, s: &mut (Popup, super::super::conn::Conn, egui::Rect)| {
                            if !fonts {
                                kiln_common::fonts::install(ui.ctx());
                                fonts = true;
                                return;
                            }
                            theme.apply(ui.ctx());
                            s.2 = ui.available_rect_before_wrap();
                            s.0.ui(
                                ui,
                                &mut s.1,
                                1,
                                true,
                                true,
                                s.2,
                                s.2.left_top() + egui::vec2(8.0, 18.0),
                                13.5,
                            );
                        },
                        (popup, conn, egui::Rect::NOTHING),
                    );
                h.run_steps(4);
                let rect = h.state().0.last_rect.unwrap();
                assert!(
                    h.state().2.expand(0.5).contains_rect(rect),
                    "popup {rect:?} exceeds {:?}",
                    h.state().2
                );
                assert!(
                    rect.contains_rect(h.get_by_label("작업 트리 변경 사항 보기").rect()),
                    "the selected command description must remain visible while the list scrolls"
                );
                for _ in 0..7 {
                    h.event(egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                    h.step();
                }
                h.run_steps(3);
                assert_eq!(h.state().0.selected, 7);
                assert!(
                    h.state()
                        .0
                        .last_rect
                        .unwrap()
                        .contains_rect(h.get_by_label("fixture-option-07").rect()),
                    "keyboard-selected row must be visible before accepting"
                );
                h.render()
                    .unwrap()
                    .save(std::env::temp_dir().join(format!(
                        "kiln-shell-completion-short-{theme_name}-{}.png",
                        height as u32
                    )))
                    .unwrap();
            }
        }
    }
}

#[cfg(test)]
mod dismissal_tests {
    use super::*;
    use egui_kittest::Harness;
    #[test]
    fn escape_then_blur_does_not_reopen_the_same_request() {
        let request = kiln_proto::ShellCompletion {
            explicit: true,
            revision: 77,
            buffer: "git st".into(),
            cursor: 6,
            cwd: "/".into(),
            request_file: String::new(),
            commands: vec![],
        };
        let popup = Popup {
            request: Some(request.clone()),
            rows: candidates("git st", 6, Path::new("/"), &[]),
            ..Default::default()
        };
        let mut conn = super::super::conn::Conn::offline(egui::Context::default());
        conn.shell_completions.insert(1, request);
        let mut fonts = false;
        let mut h = Harness::builder().with_size([360.0, 240.0]).build_ui_state(
            move |ui, s: &mut (Popup, super::super::conn::Conn, bool)| {
                if !fonts {
                    kiln_common::fonts::install(ui.ctx());
                    fonts = true;
                    return;
                }
                kiln_common::Theme::current().apply(ui.ctx());
                let rect = ui.available_rect_before_wrap();
                s.0.ui(ui, &mut s.1, 1, s.2, true, rect, rect.left_top(), 13.5);
            },
            (popup, conn, true),
        );
        h.run_steps(3);
        h.event(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        assert_eq!(h.state().0.closed_revision, Some(77));
        h.state_mut().2 = false;
        h.run_steps(3);
        h.state_mut().2 = true;
        h.run_steps(3);
        assert!(h.state().0.request.is_none());
        assert_eq!(h.state().0.closed_revision, Some(77));
        h.state_mut()
            .1
            .shell_completions
            .get_mut(&1)
            .unwrap()
            .revision = 78;
        h.step();
        assert_eq!(
            h.state().0.request.as_ref().map(|r| r.revision),
            Some(78),
            "a fresh explicit request must still open"
        );
    }
}
