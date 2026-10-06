use super::*;
use crate::completion::{self, Candidate, Context, Kind};
use egui::{Event, ImeEvent, Rect, Sense, Stroke, pos2, vec2};

// consume_key ignores extra Shift/Alt; completion must leave modified editor keys alone.
fn consume_exact(i: &mut egui::InputState, modifiers: Modifiers, key: Key) -> bool {
    let matches = i.events.iter().any(|e| matches!(e, Event::Key { key: k, pressed: true, modifiers: m, .. } if *k == key && m.matches_exact(modifiers)));
    matches && i.consume_key(modifiers, key)
}

pub(super) struct CompletionState {
    context: Context,
    items: Vec<Candidate>,
    selected: usize,
    manual: bool,
    rect: Rect,
}
impl ConsoleView {
    #[allow(deprecated)]
    pub(super) fn completion_keys(&mut self, ui: &mut Ui) {
        let popup_pointer = self.completion.as_ref().is_some_and(|s| {
            ui.input(|i| {
                i.pointer.interact_pos().is_some_and(|p| s.rect.contains(p))
                    && (i.pointer.any_down() || i.pointer.any_released())
            })
        });
        if popup_pointer {
            ui.memory_mut(|m| m.request_focus(self.editor_id));
        }
        let focused = ui.memory(|m| m.has_focus(self.editor_id));
        if !focused {
            self.completion = None;
            self.ime_composing = false;
            return;
        }
        for event in ui.input(|i| i.events.clone()) {
            if let Event::Ime(event) = event {
                match event {
                    ImeEvent::Preedit { text, .. } => self.ime_composing = !text.is_empty(),
                    ImeEvent::Commit(_) | ImeEvent::Disabled => self.ime_composing = false,
                    _ => {}
                }
            }
        }
        if self.ime_composing {
            self.completion = None;
            return;
        }
        self.completion_requested |= ui.input_mut(|i| {
            consume_exact(i, Modifiers::CTRL, Key::Space)
                || consume_exact(i, Modifiers::COMMAND, Key::Space)
                || consume_exact(i, Modifiers::ALT, Key::Escape)
        });
        if self.completion.is_none() {
            return;
        }
        if ui.input_mut(|i| consume_exact(i, Modifiers::NONE, Key::Escape)) {
            self.dismiss_completion();
            return;
        }
        let state = self.completion.as_mut().unwrap();
        if ui.input_mut(|i| consume_exact(i, Modifiers::NONE, Key::ArrowDown)) {
            state.selected = (state.selected + 1).min(state.items.len().saturating_sub(1));
        }
        if ui.input_mut(|i| consume_exact(i, Modifiers::NONE, Key::ArrowUp)) {
            state.selected = state.selected.saturating_sub(1);
        }
        if ui.input_mut(|i| consume_exact(i, Modifiers::NONE, Key::PageDown)) {
            state.selected = (state.selected + 8).min(state.items.len().saturating_sub(1));
        }
        if ui.input_mut(|i| consume_exact(i, Modifiers::NONE, Key::PageUp)) {
            state.selected = state.selected.saturating_sub(8);
        }
        // A text/IME event in this same batch may invalidate last frame's replacement.
        let editing = ui.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, Event::Text(_) | Event::Ime(_)))
        });
        if !editing
            && !state.items.is_empty()
            && ui.input_mut(|i| {
                consume_exact(i, Modifiers::NONE, Key::Enter)
                    || consume_exact(i, Modifiers::NONE, Key::Tab)
            })
        {
            self.accept_completion(ui.ctx());
        }
    }
    fn dismiss_completion(&mut self) {
        self.completion = None;
        self.completion_requested = false;
        self.completion_dismissed = Some((self.sql.clone(), self.cursor.map_or(0, |c| c.0)));
    }
    fn accept_completion(&mut self, ctx: &egui::Context) {
        let Some(state) = self.completion.take() else {
            return;
        };
        let Some(item) = state.items.get(state.selected) else {
            return;
        };
        let Some(mut edit) = egui::TextEdit::load_state(ctx, self.editor_id) else {
            return;
        };
        let Some(cursor) = edit.cursor.char_range() else {
            return;
        };
        if !cursor.is_empty() {
            return;
        }
        // Validate against the current text/caret; never apply a stale async result.
        let Some(current) = completion::context(
            &self.sql,
            self.driver,
            cursor.primary.index.0,
            &self.metadata.catalog,
        ) else {
            return;
        };
        if current.replace != state.context.replace
            || current.prefix != state.context.prefix
            || current.qualifier != state.context.qualifier
        {
            return;
        }
        let mut undo = edit.undoer();
        undo.add_undo(&(cursor, self.sql.clone()));
        let ci = self.sql[..current.replace.start].chars().count() + item.insert.chars().count()
            - item.back;
        self.sql.replace_range(current.replace, &item.insert);
        let new_cursor = CCursorRange::one(CCursor::new(ci));
        undo.add_undo(&(new_cursor, self.sql.clone()));
        edit.set_undoer(undo);
        edit.cursor.set_char_range(Some(new_cursor));
        edit.store(ctx, self.editor_id);
        self.cursor = Some((ci, ci));
        self.set_cursor = Some(ci);
        ctx.memory_mut(|m| m.request_focus(self.editor_id));
        self.completion_dismissed = Some((self.sql.clone(), ci));
        if item.kind == Kind::Schema {
            self.completion_requested = true;
        }
        ctx.request_repaint();
    }
    pub(super) fn completion_overlay(
        &mut self,
        ui: &mut Ui,
        out: &egui::text_edit::TextEditOutput,
        bounds: Rect,
        m: &DbManager,
    ) {
        if self.completion.as_ref().is_some_and(|s| {
            ui.input(|i| {
                i.pointer.interact_pos().is_some_and(|p| s.rect.contains(p))
                    && (i.pointer.any_down() || i.pointer.any_released())
            })
        }) {
            ui.memory_mut(|m| m.request_focus(self.editor_id));
        }
        if !out.response.has_focus() || self.ime_composing {
            self.completion = None;
            return;
        }
        let Some(cursor) = out.cursor_range.filter(|c| c.is_empty()) else {
            self.completion = None;
            return;
        };
        let caret = cursor.primary.index.0;
        let Some(context) =
            completion::context(&self.sql, self.driver, caret, &self.metadata.catalog)
        else {
            self.completion = None;
            self.completion_requested = false;
            return;
        };
        let needed = completion::needed(&context, &self.metadata.catalog);
        self.metadata.poll(m, self.conn, &needed);
        // Metadata may have just completed; resolve bindings against the fresh catalog.
        let context =
            completion::context(&self.sql, self.driver, caret, &self.metadata.catalog).unwrap();
        let requested = std::mem::take(&mut self.completion_requested);
        if requested {
            self.completion_dismissed = None;
        }
        let dismissed = self
            .completion_dismissed
            .as_ref()
            .is_some_and(|(text, ci)| text == &self.sql && *ci == caret);
        let trigger = context.prefix.chars().count() >= 2
            || (!context.qualifier.is_empty() && context.prefix.is_empty())
            || (out.response.changed() && context.tables_only && context.prefix.is_empty());
        let manual = requested || self.completion.as_ref().is_some_and(|s| s.manual);
        if !requested && (dismissed || (!manual && !trigger)) {
            self.completion = None;
            return;
        }
        let items = completion::candidates(&context, self.driver, &self.metadata.catalog);
        if items.is_empty() && !manual && !self.metadata.loading() {
            self.completion = None;
            return;
        }
        let selected = self
            .completion
            .as_ref()
            .and_then(|s| s.items.get(s.selected))
            .and_then(|item| items.iter().position(|c| c == item))
            .unwrap_or(0);
        let previous_rect = self.completion.as_ref().map_or(Rect::NOTHING, |s| s.rect);
        self.completion = Some(CompletionState {
            context,
            items,
            selected,
            manual,
            rect: previous_rect,
        });
        let anchor = out
            .galley
            .pos_from_cursor(cursor.primary)
            .translate(out.galley_pos.to_vec2());
        if !out.text_clip_rect.intersects(anchor) {
            self.completion = None;
            return;
        }
        let theme = Theme::current();
        let state = self.completion.as_ref().unwrap();
        let below_room = (bounds.bottom() - anchor.bottom() - 11.0).max(0.0);
        let above_room = (anchor.top() - bounds.top() - 11.0).max(0.0);
        let preferred = state.items.len().clamp(1, 8) as f32 * 30.0 + 38.0;
        let below = below_room >= preferred || below_room >= above_room;
        let room = if below { below_room } else { above_room };
        if room < 68.0 {
            self.completion = None;
            return;
        }
        let rows = state
            .items
            .len()
            .min(8)
            .min(((room - 38.0) / 30.0).floor().max(1.0) as usize);
        let width = (bounds.width() - 12.0).clamp(120.0, 460.0);
        let height = rows.max(1) as f32 * 30.0 + 38.0;
        let x = anchor.left().clamp(
            bounds.left() + 4.0,
            (bounds.right() - width - 8.0).max(bounds.left() + 4.0),
        );
        let y = if below {
            anchor.bottom() + 3.0
        } else {
            (anchor.top() - height - 3.0).max(bounds.top() + 4.0)
        };
        let top = state.selected.saturating_sub(rows.saturating_sub(1));
        let mut clicked = None;
        let mut refresh = false;
        let area = egui::Area::new(self.editor_id.with("sql-completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(x, y))
            .constrain_to(bounds)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(theme.bg_elevated)
                    .stroke(Stroke::new(1.0, theme.border_strong))
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                        if state.items.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(if self.metadata.loading() {
                                        kiln_common::i18n::tr("메타데이터를 불러오는 중…")
                                    } else {
                                        kiln_common::i18n::tr("일치하는 자동완성이 없습니다")
                                    })
                                    .color(theme.text_dim),
                                )
                                .wrap(),
                            );
                        }
                        for (index, item) in state.items.iter().enumerate().skip(top).take(rows) {
                            let (rect, response) =
                                ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    true,
                                    index == state.selected,
                                    format!(
                                        "{} · {} · {}",
                                        item.label,
                                        item.kind.label(),
                                        item.detail
                                    ),
                                )
                            });
                            if index == state.selected || response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    4.0,
                                    if index == state.selected {
                                        theme.bg_selected
                                    } else {
                                        theme.bg_hover
                                    },
                                );
                            }
                            let icon = match item.kind {
                                Kind::Column => Icon::Columns,
                                Kind::Table => Icon::Table,
                                Kind::Schema => Icon::Database,
                                Kind::Keyword | Kind::Function => Icon::Code,
                            };
                            kiln_common::icons::paint(
                                ui.painter(),
                                Rect::from_center_size(
                                    pos2(rect.left() + 14.0, rect.center().y),
                                    vec2(16.0, 16.0),
                                ),
                                icon,
                                theme.text_dim,
                            );
                            let name_width = ((width - 42.0) * 0.55).max(40.0);
                            for (text, font, color, left, max_width) in [
                                (
                                    &item.label,
                                    fonts::mono(12.5),
                                    theme.text,
                                    rect.left() + 30.0,
                                    name_width,
                                ),
                                (
                                    &item.detail,
                                    fonts::regular(11.0),
                                    theme.text_dim,
                                    rect.left() + 38.0 + name_width,
                                    width - name_width - 44.0,
                                ),
                            ] {
                                let mut job = egui::text::LayoutJob::simple_singleline(
                                    text.clone(),
                                    font,
                                    color,
                                );
                                job.wrap.max_width = max_width.max(1.0);
                                job.wrap.max_rows = 1;
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                ui.painter().galley(
                                    pos2(left, rect.center().y - galley.size().y / 2.0),
                                    galley,
                                    color,
                                );
                            }
                            response.clone().on_hover_text(format!(
                                "{}\n{} · {}",
                                item.label,
                                item.kind.label(),
                                item.detail
                            ));
                            if response.clicked() {
                                clicked = Some(index);
                            }
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            let status = if self.metadata.error.is_some() {
                                kiln_common::i18n::tr("메타데이터를 불러오지 못했습니다")
                            } else if self.metadata.loading() {
                                kiln_common::i18n::tr("메타데이터를 불러오는 중…")
                            } else {
                                kiln_common::i18n::tr("↑↓ 선택 · Tab/Enter 삽입 · Esc 닫기")
                            };
                            ui.add_sized(
                                vec2((width - 32.0).max(40.0), 22.0),
                                egui::Label::new(RichText::new(status).size(11.0).color(
                                    if self.metadata.error.is_some() {
                                        theme.red
                                    } else {
                                        theme.text_dim
                                    },
                                ))
                                .truncate(),
                            )
                            .on_hover_text(self.metadata.error.as_deref().unwrap_or(status));
                            refresh = crate::ui::icon_button(
                                ui,
                                Icon::Refresh,
                                kiln_common::i18n::tr("자동완성 메타데이터 새로고침"),
                            )
                            .clicked();
                        });
                    });
            });
        self.completion.as_mut().unwrap().rect = area.response.rect;
        if area.response.hovered() {
            let delta = ui.input_mut(|i| {
                let d = i.smooth_scroll_delta.y;
                i.smooth_scroll_delta.y = 0.0;
                d
            });
            if delta.abs() > 0.5 {
                let state = self.completion.as_mut().unwrap();
                if delta < 0.0 {
                    state.selected = (state.selected + 1).min(state.items.len().saturating_sub(1));
                } else {
                    state.selected = state.selected.saturating_sub(1);
                }
                ui.ctx().request_repaint();
            }
        }
        if ui.input(|i| {
            i.pointer
                .interact_pos()
                .is_some_and(|p| area.response.rect.contains(p))
                && (i.pointer.any_down() || i.pointer.any_released())
        }) {
            ui.memory_mut(|m| m.request_focus(self.editor_id));
        }
        if let Some(index) = clicked {
            self.completion.as_mut().unwrap().selected = index;
            self.accept_completion(ui.ctx());
        } else if refresh {
            self.metadata.reset();
            self.focus_pending = true;
            self.completion_requested = true;
            ui.memory_mut(|m| m.request_focus(self.editor_id));
        } else if ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|pos| !area.response.rect.contains(pos))
        }) {
            self.dismiss_completion();
        }
    }
}
