//! Bounded, asynchronous file-path references. No directory I/O runs on the UI thread.
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_RESULTS: usize = 32;
const MAX_VISITED: usize = 6000;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Query {
    start: usize,
    end: usize,
    needle: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    label: String,
    directory: bool,
    rank: u8,
}
struct SearchResult {
    query: Query,
    entries: Vec<Entry>,
    error: Option<String>,
}
#[derive(Default)]
pub(super) struct MentionsState {
    query: Option<Query>,
    running: Option<Task<SearchResult>>,
    loaded: Option<Query>,
    entries: Vec<Entry>,
    error: Option<String>,
    active: usize,
    dismissed: bool,
    scroll: bool,
}

fn byte_at_char(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}
fn query_at(text: &str, cursor: usize) -> Option<Query> {
    let cursor = byte_at_char(text, cursor);
    let prefix = &text[..cursor];
    let start = prefix.char_indices().rev().find_map(|(index, ch)| {
        (ch == '@'
            && (index == 0
                || text[..index].chars().next_back().is_some_and(|previous| {
                    previous.is_whitespace() || matches!(previous, '(' | '[')
                })))
        .then_some(index)
    })?;
    let token = &text[start + 1..cursor];
    let (needle, end) = if let Some(inner) = token.strip_prefix('"') {
        // A completed quoted token is a reference, not an open completion.
        let mut escaped = false;
        for ch in inner.chars() {
            if !escaped && ch == '"' {
                return None;
            }
            if !escaped && ch == '\\' {
                escaped = true;
            } else {
                escaped = false;
            }
        }
        let value: String = serde_json::from_str(&format!("\"{inner}\"")).ok()?;
        let mut escaped = false;
        let end = text[cursor..]
            .char_indices()
            .find_map(|(offset, ch)| {
                let closing = ch == '"' && !escaped;
                if ch == '\\' && !escaped {
                    escaped = true;
                } else {
                    escaped = false;
                }
                closing.then_some(cursor + offset + 1)
            })
            .unwrap_or(cursor);
        (value, end)
    } else {
        if token
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '<' | '>'))
        {
            return None;
        }
        let end = text[cursor..]
            .char_indices()
            .find_map(|(offset, ch)| ch.is_whitespace().then_some(cursor + offset))
            .unwrap_or(text.len());
        (token.to_owned(), end)
    };
    if needle.contains(['\n', '\r', '\0']) || needle.len() > 4096 {
        return None;
    }
    Some(Query { start, end, needle })
}

fn scan(cwd: &Path, query: Query) -> SearchResult {
    let needle = query.needle.replace('\\', "/");
    let expanded = if let Some(relative) = needle.strip_prefix("~/") {
        kiln_common::paths::home()
            .map(|home| home.join(relative))
            .unwrap_or_else(|| cwd.join(&needle))
    } else {
        PathBuf::from(&needle)
    };
    let explicit_parent = needle.contains('/');
    let (root, leaf) = if explicit_parent {
        let parent = if needle.ends_with('/') {
            expanded.as_path()
        } else {
            expanded.parent().unwrap_or(Path::new(""))
        };
        let root = if parent.is_absolute() {
            parent.to_path_buf()
        } else {
            cwd.join(parent)
        };
        let leaf = if needle.ends_with('/') {
            String::new()
        } else {
            expanded
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_lowercase()
        };
        (root, leaf)
    } else {
        (cwd.to_path_buf(), needle.to_lowercase())
    };
    let deadline = Instant::now() + Duration::from_millis(120);
    let mut queue = VecDeque::from([(root, 0usize)]);
    let mut entries = Vec::new();
    let mut visited = 0;
    let mut error = None;
    while let Some((directory, depth)) = queue.pop_front() {
        if visited >= MAX_VISITED || Instant::now() > deadline {
            break;
        }
        let listing = match std::fs::read_dir(&directory) {
            Ok(listing) => listing,
            Err(failure) => {
                if depth == 0 {
                    error = Some(failure.to_string());
                }
                continue;
            }
        };
        for item in listing {
            visited += 1;
            if visited > MAX_VISITED || Instant::now() > deadline {
                break;
            }
            let Ok(item) = item else {
                continue;
            };
            let Ok(kind) = item.file_type() else {
                continue;
            };
            if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
                continue;
            }
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.chars().any(char::is_control) {
                continue;
            }
            let directory = kind.is_dir();
            if directory
                && [
                    ".git",
                    "node_modules",
                    "target",
                    ".next",
                    ".venv",
                    "__pycache__",
                ]
                .contains(&name.as_str())
            {
                continue;
            }
            let path = item.path();
            let label = path
                .strip_prefix(cwd)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let lower = name.to_lowercase();
            let rank = if leaf.is_empty() || lower.starts_with(&leaf) {
                Some(0)
            } else if lower.contains(&leaf) || label.to_lowercase().contains(&leaf) {
                Some(1)
            } else {
                None
            };
            if let Some(rank) = rank {
                entries.push(Entry {
                    path: path.clone(),
                    label,
                    directory,
                    rank,
                });
            }
            if directory && !explicit_parent && depth < 3 {
                queue.push_back((path, depth + 1));
            }
        }
    }
    entries.sort_by(|left, right| {
        (left.rank, !left.directory, left.label.to_lowercase()).cmp(&(
            right.rank,
            !right.directory,
            right.label.to_lowercase(),
        ))
    });
    entries.truncate(MAX_RESULTS);
    SearchResult {
        query,
        entries,
        error,
    }
}

impl AgentPane {
    fn mention_cursor(&self, ui: &Ui) -> Option<usize> {
        if let Some(range) = egui::TextEdit::load_state(ui.ctx(), ui.id().with("message"))
            .and_then(|state| state.cursor.char_range())
        {
            return range.is_empty().then_some(range.primary.index.0);
        }
        Some(self.composer.text.chars().count())
    }
    fn refresh_mentions(&mut self, ui: &Ui) {
        let query = self
            .mention_cursor(ui)
            .and_then(|cursor| query_at(&self.composer.text, cursor));
        if self.mentions.query != query {
            self.mentions.query = query;
            self.mentions.entries.clear();
            self.mentions.loaded = None;
            self.mentions.error = None;
            self.mentions.active = 0;
            self.mentions.dismissed = false;
        }
        if let Some(result) = self.mentions.running.as_mut().and_then(Task::take) {
            self.mentions.running = None;
            if self.mentions.query.as_ref() == Some(&result.query) {
                self.mentions.loaded = Some(result.query.clone());
                self.mentions.entries = result.entries;
                self.mentions.error = result.error;
                self.mentions.active = 0;
            }
        }
        // At most one task exists per pane; rapid edits coalesce into the latest query.
        if self.mentions.running.is_none()
            && self.mentions.loaded != self.mentions.query
            && !self.mentions.dismissed
            && let Some(query) = self.mentions.query.clone()
        {
            let cwd = self.descriptor.cwd.clone();
            self.mentions.running = Some(Task::spawn(ui.ctx(), move || scan(&cwd, query)));
        }
    }
    fn insert_mention(&mut self, ui: &Ui, entry: &Entry, browse: bool) {
        if self.composing || self.ime_frame {
            return;
        }
        let Some(query) = self
            .mention_cursor(ui)
            .and_then(|cursor| query_at(&self.composer.text, cursor))
        else {
            return;
        };
        if self.mentions.query.as_ref() != Some(&query) {
            return;
        }
        let Some(path) = entry.path.to_str() else {
            return;
        };
        let mut path = path.replace('\\', "/");
        if entry.directory && !path.ends_with('/') {
            path.push('/');
        }
        let quoted = serde_json::to_string(&path).unwrap_or_default();
        let sigil = if self.descriptor.tool == Tool::Codex && !(browse && entry.directory) {
            ""
        } else {
            "@"
        };
        let insertion = format!(
            "{sigil}{quoted}{}",
            if browse && entry.directory { "" } else { " " }
        );
        self.composer
            .text
            .replace_range(query.start..query.end, &insertion);
        let cursor = self.composer.text[..query.start].chars().count() + insertion.chars().count()
            - usize::from(browse && entry.directory);
        let id = ui.id().with("message");
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(cursor),
            )));
        state.store(ui.ctx(), id);
        self.mentions.dismissed = !(browse && entry.directory);
        ui.ctx().memory_mut(|memory| memory.request_focus(id));
        self.focus = false;
    }
    /// Call after the IME guard and before TextEdit/Enter-to-send handling.
    pub(super) fn mention_completion_input(&mut self, ui: &mut Ui, focused: bool) -> bool {
        if !focused || self.composing || self.ime_frame {
            return false;
        }
        self.refresh_mentions(ui);
        if self.mentions.query.is_none() || self.mentions.dismissed {
            return false;
        }
        let plain = ui.input(|input| {
            !input.modifiers.shift
                && !input.modifiers.command
                && !input.modifiers.ctrl
                && !input.modifiers.alt
        });
        if !plain {
            return false;
        }
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.mentions.dismissed = true;
            return true;
        }
        let count = self.mentions.entries.len();
        for (key, down) in [(egui::Key::ArrowDown, true), (egui::Key::ArrowUp, false)] {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                if count > 0 {
                    self.mentions.active = if down {
                        (self.mentions.active + 1) % count
                    } else {
                        (self.mentions.active + count - 1) % count
                    };
                }
                self.mentions.scroll = true;
                return true;
            }
        }
        let tab = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        let enter =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        if tab || enter {
            if let Some(entry) = self.mentions.entries.get(self.mentions.active).cloned() {
                self.insert_mention(ui, &entry, tab);
            }
            return true; // Pending/empty results must never turn selection into Send.
        }
        false
    }
    pub(super) fn mention_completion_ui(&mut self, ui: &mut Ui) {
        if self.composing
            || self.ime_frame
            || !ui.memory(|memory| memory.has_focus(ui.id().with("message")))
        {
            return;
        }
        self.refresh_mentions(ui);
        if self.mentions.query.is_none() || self.mentions.dismissed {
            return;
        }
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                ui.id().with("message"),
                egui::EventFilter {
                    tab: true,
                    escape: true,
                    vertical_arrows: true,
                    horizontal_arrows: false,
                },
            )
        });
        let theme = Theme::current();
        let bounds = ui.ctx().content_rect().shrink(8.);
        let width = ui.available_width().min(bounds.width()).min(456.).max(120.);
        let height = (self.mentions.entries.len().max(1) as f32 * 46. + 48.)
            .min(230.)
            .min(bounds.height() * 0.5);
        let anchor = ui.min_rect();
        let position = egui::pos2(
            anchor.left().min(bounds.right() - width).max(bounds.left()),
            (anchor.top() - height - 12.).max(bounds.top()),
        );
        let entries = self.mentions.entries.clone();
        let mut chosen = None;
        egui::Area::new(egui::Id::new(("agent-path-mentions", &self.descriptor.id)))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .constrain_to(bounds)
            .movable(false)
            .fade_in(false)
            .show(ui.ctx(), |popup| {
                popup.set_width(width);
                popup.with_layout(egui::Layout::top_down(egui::Align::Min), |popup| {
                    Frame::new()
                        .fill(theme.bg_panel)
                        .stroke(Stroke::new(1., theme.border))
                        .corner_radius(6)
                        .inner_margin(6)
                        .show(popup, |popup| {
                            popup.strong(kiln_common::i18n::tr("파일 및 폴더"));
                            if self.mentions.running.is_some() {
                                popup.small(kiln_common::i18n::tr("경로를 찾는 중…"));
                            } else if self.mentions.error.is_some() {
                                popup.small(kiln_common::i18n::tr("폴더를 읽을 수 없습니다."));
                            } else if entries.is_empty() {
                                popup.small(kiln_common::i18n::tr("검색 결과가 없습니다."));
                            }
                            egui::ScrollArea::vertical()
                                .max_height((height - 48.).max(36.))
                                .auto_shrink([false, true])
                                .show(popup, |popup| {
                                    for (index, entry) in entries.iter().enumerate() {
                                        let (rect, response) = popup.allocate_exact_size(
                                            egui::vec2(popup.available_width(), 44.),
                                            egui::Sense::click(),
                                        );
                                        if index == self.mentions.active || response.hovered() {
                                            popup.painter().rect_filled(rect, 4, theme.bg_hover);
                                        }
                                        let icon = egui::Rect::from_min_size(
                                            rect.min + egui::vec2(5., 7.),
                                            egui::vec2(16., 16.),
                                        );
                                        kiln_common::icons::paint(
                                            popup.painter(),
                                            icon,
                                            if entry.directory {
                                                kiln_common::icons::Icon::Folder
                                            } else {
                                                kiln_common::icons::Icon::File
                                            },
                                            theme.text_dim,
                                        );
                                        let text_rect = egui::Rect::from_min_max(
                                            rect.min + egui::vec2(28., 2.),
                                            rect.max - egui::vec2(4., 2.),
                                        );
                                        let painter = popup.painter().with_clip_rect(text_rect);
                                        painter.text(
                                            text_rect.min,
                                            egui::Align2::LEFT_TOP,
                                            &entry.label,
                                            kiln_common::fonts::medium(12.5),
                                            theme.text,
                                        );
                                        painter.text(
                                            text_rect.min + egui::vec2(0., 21.),
                                            egui::Align2::LEFT_TOP,
                                            kiln_common::i18n::tr(if entry.directory {
                                                "폴더"
                                            } else {
                                                "파일"
                                            }),
                                            kiln_common::fonts::regular(11.),
                                            theme.text_dim,
                                        );
                                        response.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Button,
                                                true,
                                                &entry.label,
                                            )
                                        });
                                        if index == self.mentions.active && self.mentions.scroll {
                                            response.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if response
                                            .on_hover_text(entry.path.to_string_lossy())
                                            .clicked()
                                        {
                                            chosen = Some(entry.clone());
                                        }
                                    }
                                });
                        })
                });
            });
        self.mentions.scroll = false;
        if let Some(entry) = chosen {
            self.insert_mention(ui, &entry, false);
        }
    }
}

#[cfg(test)]
#[path = "mentions_tests.rs"]
mod tests;
