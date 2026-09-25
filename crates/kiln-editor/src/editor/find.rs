//! 편집기 찾기/바꾸기 상태와 떠 있는 찾기 막대.

use egui::{Align, Key, Layout, Rect, Stroke, Ui, UiBuilder, pos2, vec2};
use kiln_common::Theme;
use regex::Regex;

use super::{Editor, Reveal};
use crate::buffer::{EditKind, FindOptions, Pos, Selection, build_regex, expand_replacement};
use crate::ui_kit::{self, Icon};

/// 강조 표시할 최대 일치 수.
pub(crate) const MATCH_LIMIT: usize = 20_000;

#[derive(Default)]
pub(crate) struct FindState {
    pub open: bool,
    pub replace_open: bool,
    pub query: String,
    pub replacement: String,
    pub opts: FindOptions,
    pub matches: Vec<(Pos, Pos)>,
    pub current: Option<usize>,
    pub error: bool,
    pub focus_query: bool,
    pub select_query: bool,
    regex: Option<Regex>,
    key: Option<(u64, String, FindOptions)>,
    last_query: Option<(String, FindOptions)>,
}

impl FindState {
    pub fn invalidate(&mut self) {
        self.key = None;
    }
}

impl Editor {
    /// 찾기 막대를 연다. 한 줄 선택이 있으면 검색어로 채운다.
    pub fn open_find(&mut self, replace: bool) {
        let (a, b) = self.sel.range();
        if a != b && a.line == b.line {
            self.find.query = self.buf.text_range(a, b);
        }
        self.find.open = true;
        self.find.replace_open = replace;
        self.find.focus_query = true;
        self.find.select_query = true;
        self.find.invalidate();
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.find.matches.clear();
        self.find.current = None;
        self.find.invalidate();
    }

    /// 버전이나 검색어가 바뀌었으면 일치 목록을 다시 계산한다.
    pub(crate) fn update_find_matches(&mut self) {
        if !self.find.open {
            return;
        }
        let key = (self.buf.version(), self.find.query.clone(), self.find.opts);
        if self.find.key.as_ref() == Some(&key) {
            return;
        }
        let query_changed = self.find.last_query.as_ref() != Some(&(key.1.clone(), key.2));
        self.find.key = Some(key);
        self.find.last_query = Some((self.find.query.clone(), self.find.opts));
        self.find.matches.clear();
        self.find.current = None;
        self.find.error = false;
        self.find.regex = None;
        if self.find.query.is_empty() {
            return;
        }
        match build_regex(&self.find.query, self.find.opts) {
            Ok(re) => {
                self.find.matches = self.buf.find_all(&re, MATCH_LIMIT);
                self.find.regex = Some(re);
            }
            Err(_) => {
                self.find.error = true;
                return;
            }
        }
        let (a, _) = self.sel.range();
        let idx = self.find.matches.iter().position(|m| m.0 >= a).or((!self.find.matches.is_empty()).then_some(0));
        if query_changed {
            // 입력하는 동안 커서 이후 첫 일치로 이동한다.
            if let Some(i) = idx {
                self.select_match(i);
            }
        } else {
            let sel = self.sel.range();
            self.find.current = self.find.matches.iter().position(|m| *m == sel);
        }
    }

    fn select_match(&mut self, i: usize) {
        let (a, b) = self.find.matches[i];
        self.extra.clear();
        self.sel = Selection::new(a, b);
        self.find.current = Some(i);
        self.preferred_col = None;
        self.reveal = Some(Reveal::Center);
    }

    /// 다음(또는 이전) 일치로 이동한다. 끝에 닿으면 처음으로 돌아간다.
    pub fn find_next(&mut self, forward: bool) {
        if !self.find.open && !self.find.query.is_empty() {
            self.find.open = true;
        }
        self.update_find_matches();
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        let (a, b) = self.sel.range();
        let i = if forward {
            self.find.matches.iter().position(|m| m.0 >= b && (m.0, m.1) != (a, b)).unwrap_or(0)
        } else {
            self.find.matches.iter().rposition(|m| m.1 <= a && (m.0, m.1) != (a, b)).unwrap_or(n - 1)
        };
        self.select_match(i);
    }

    /// 현재 선택이 일치 항목이면 바꾸고 다음 일치로 이동한다.
    pub fn replace_current(&mut self) {
        self.update_find_matches();
        let sel = self.sel.range();
        let Some(re) = self.find.regex.clone() else { return };
        if self.find.matches.contains(&sel) && !self.read_only {
            let matched = self.buf.text_range(sel.0, sel.1);
            let rep = expand_replacement(&re, &matched, &self.find.replacement, self.find.opts.regex);
            let before = self.cursor_snapshot();
            self.extra.clear();
            self.buf.begin(EditKind::Other, &before, self.now());
            let end = self.buf.replace(sel.0, sel.1, &rep);
            self.sel = Selection::caret(end);
            self.buf.end(&[self.sel]);
            self.sync_highlighter();
            self.update_find_matches();
        }
        self.find_next(true);
    }

    /// 모든 일치를 한 번의 되돌리기 단위로 바꾼다. 바꾼 개수를 돌려준다.
    pub fn replace_all(&mut self) -> usize {
        self.update_find_matches();
        let Some(re) = self.find.regex.clone() else { return 0 };
        if self.read_only || self.find.matches.is_empty() {
            return 0;
        }
        let matches = std::mem::take(&mut self.find.matches);
        let n = matches.len();
        let before = self.cursor_snapshot();
        self.buf.begin(EditKind::Other, &before, self.now());
        for &(a, b) in matches.iter().rev() {
            let matched = self.buf.text_range(a, b);
            let rep = expand_replacement(&re, &matched, &self.find.replacement, self.find.opts.regex);
            self.buf.replace(a, b, &rep);
            self.sync_line_edits();
        }
        let head = self.buf.clamp(self.sel.head);
        self.extra.clear();
        self.sel = Selection::caret(head);
        self.buf.end(&[self.sel]);
        self.sync_highlighter();
        n
    }

    pub(crate) fn find_query_id(&self) -> egui::Id {
        self.id().with("find-query")
    }

    pub(crate) fn find_replace_id(&self) -> egui::Id {
        self.id().with("find-replace")
    }

    /// 찾기 막대를 `area` 오른쪽 위에 그린다. 막대 영역을 돌려준다.
    pub(crate) fn find_bar_ui(&mut self, ui: &mut Ui, area: Rect) -> Rect {
        let t = Theme::current();
        let width = (area.width() - 28.0).clamp(240.0, 470.0);
        let row_h = 28.0;
        let height = if self.find.replace_open { row_h * 2.0 + 14.0 } else { row_h + 10.0 };
        let rect = Rect::from_min_size(pos2(area.right() - width - 18.0, area.top() + 6.0), vec2(width, height));
        let qid = self.find_query_id();
        let rid = self.find_replace_id();
        let mut close = false;
        let mut next: Option<bool> = None;
        let mut do_replace = false;
        let mut do_replace_all = false;

        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
            egui::Frame::new()
                .fill(t.bg_elevated)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(6)
                .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: egui::Color32::from_black_alpha(90) })
                .inner_margin(egui::Margin { left: 4, right: 6, top: 5, bottom: 5 })
                .show(ui, |ui| {
                    ui.set_width(width - 10.0);
                    ui.spacing_mut().item_spacing = vec2(3.0, 4.0);
                    let fixed = 22.0 * 3.0 + 3.0 * 5.0 + 64.0;
                    let field_w = (width - 10.0 - 20.0 - fixed).max(80.0);
                    ui.horizontal(|ui| {
                        ui.set_height(row_h);
                        let chevron = if self.find.replace_open { Icon::ChevronDown } else { Icon::ChevronRight };
                        let (r, resp) = ui.allocate_exact_size(vec2(16.0, row_h), egui::Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r.shrink2(vec2(0.0, 3.0)), 3.0, t.bg_hover);
                        }
                        ui_kit::paint_icon(ui.painter(), r.shrink(1.0), chevron, t.text_dim);
                        if resp.on_hover_text("바꾸기 전환").clicked() {
                            self.find.replace_open = !self.find.replace_open;
                        }
                        let focused = ui.memory(|m| m.has_focus(qid));
                        ui_kit::field_frame(focused || self.find.error).show(ui, |ui| {
                            ui.set_width(field_w - 8.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 1.0;
                                let te = ui_kit::bare_text_edit(&mut self.find.query, qid, "찾기", self.find.error)
                                    .desired_width(field_w - 8.0 - 3.0 * 23.0 - 6.0);
                                let out = te.show(ui);
                                if self.find.focus_query {
                                    out.response.request_focus();
                                    self.find.focus_query = false;
                                }
                                if self.find.select_query {
                                    let mut st = out.state.clone();
                                    let n = self.find.query.chars().count();
                                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(0),
                                        egui::text::CCursor::new(n),
                                    )));
                                    st.store(ui.ctx(), qid);
                                    self.find.select_query = false;
                                }
                                let o = &mut self.find.opts;
                                if ui_kit::option_chip(ui, "Aa", "대/소문자 구분", o.case_sensitive).clicked() {
                                    o.case_sensitive = !o.case_sensitive;
                                }
                                if ui_kit::option_chip(ui, "ab", "단어 단위로", o.whole_word).clicked() {
                                    o.whole_word = !o.whole_word;
                                }
                                if ui_kit::option_chip(ui, ".*", "정규식 사용", o.regex).clicked() {
                                    o.regex = !o.regex;
                                }
                            });
                        });
                        let n = self.find.matches.len();
                        let label = if self.find.query.is_empty() {
                            String::new()
                        } else if self.find.error {
                            "잘못됨".to_owned()
                        } else if n == 0 {
                            "결과 없음".to_owned()
                        } else {
                            let more = if n >= MATCH_LIMIT { "+" } else { "" };
                            match self.find.current {
                                Some(i) => format!("{}/{n}{more}", i + 1),
                                None => format!("?/{n}{more}"),
                            }
                        };
                        let (lr, _) = ui.allocate_exact_size(vec2(64.0, row_h), egui::Sense::hover());
                        let color = if n == 0 && !self.find.query.is_empty() { t.red.gamma_multiply(0.9) } else { t.text_dim };
                        ui.painter().text(
                            lr.left_center() + vec2(4.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(11.5),
                            color,
                        );
                        let has = n > 0;
                        if ui_kit::icon_toggle(ui, Icon::ArrowUp, "이전 일치 항목 (Shift+Enter)", false, has).clicked() {
                            next = Some(false);
                        }
                        if ui_kit::icon_toggle(ui, Icon::ArrowDown, "다음 일치 항목 (Enter)", false, has).clicked() {
                            next = Some(true);
                        }
                        if ui_kit::icon_button(ui, Icon::Close, "닫기 (Escape)").clicked() {
                            close = true;
                        }
                    });
                    if self.find.replace_open {
                        ui.horizontal(|ui| {
                            ui.set_height(row_h);
                            ui.add_space(16.0 + 3.0);
                            let focused = ui.memory(|m| m.has_focus(rid));
                            ui_kit::field_frame(focused).show(ui, |ui| {
                                ui.set_width(field_w - 8.0);
                                ui.add(
                                    ui_kit::bare_text_edit(&mut self.find.replacement, rid, "바꾸기", false)
                                        .desired_width(field_w - 8.0),
                                );
                            });
                            let can = !self.find.matches.is_empty() && !self.read_only;
                            if ui_kit::icon_toggle(ui, Icon::ReplaceOne, "바꾸기 (Enter)", false, can).clicked() {
                                do_replace = true;
                            }
                            if ui_kit::icon_toggle(ui, Icon::ReplaceAll, "모두 바꾸기 (Cmd/Ctrl+Alt+Enter)", false, can).clicked() {
                                do_replace_all = true;
                            }
                        });
                    }
                });
        });

        let (q_focus, r_focus) = ui.memory(|m| (m.has_focus(qid), m.has_focus(rid)));
        if q_focus || r_focus {
            let (enter, shift, cmd_alt) = ui.input(|i| {
                (i.key_pressed(Key::Enter), i.modifiers.shift, i.modifiers.command && i.modifiers.alt)
            });
            if enter {
                if r_focus && cmd_alt {
                    do_replace_all = true;
                } else if r_focus {
                    do_replace = true;
                } else {
                    next = Some(!shift);
                }
            }
        }
        if do_replace_all {
            self.replace_all();
        } else if do_replace {
            self.replace_current();
        }
        if let Some(fwd) = next {
            self.find_next(fwd);
        }
        if close {
            self.close_find();
            ui.memory_mut(|m| m.request_focus(self.id()));
        }
        rect
    }

    pub(crate) fn goto_id(&self) -> egui::Id {
        self.id().with("goto")
    }

    /// 줄 이동 입력칸을 `area` 위쪽 가운데에 그린다.
    pub(crate) fn goto_ui(&mut self, ui: &mut Ui, area: Rect) -> Option<Rect> {
        let t = Theme::current();
        let mut text = self.goto.take()?;
        let width = (area.width() - 40.0).clamp(200.0, 360.0);
        let rect = Rect::from_min_size(pos2(area.center().x - width / 2.0, area.top() + 6.0), vec2(width, 62.0));
        let id = self.goto_id();
        let n = self.buf.line_count();
        let mut commit = false;
        let mut cancel = false;
        ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
            egui::Frame::new()
                .fill(t.bg_elevated)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(6)
                .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: egui::Color32::from_black_alpha(90) })
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    let focused = ui.memory(|m| m.has_focus(id));
                    let parsed = parse_goto(&text);
                    let bad = !text.is_empty() && parsed.is_none();
                    ui_kit::field_frame(focused).show(ui, |ui| {
                        ui.set_width(width - 16.0 - 8.0);
                        let out = ui.add(ui_kit::bare_text_edit(&mut text, id, "줄[:열]", bad).desired_width(width - 30.0));
                        if !focused && !out.lost_focus() {
                            out.request_focus();
                        }
                    });
                    let head = self.sel.head;
                    let hint = match parse_goto(&text) {
                        Some((l, c)) => format!("{l}줄{}로 이동", c.map(|c| format!(", {c}열")).unwrap_or_default()),
                        None => format!("현재 줄: {}. 1에서 {n} 사이의 줄 번호를 입력하세요.", head.line + 1),
                    };
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(hint).size(11.5).color(t.text_dim));
                    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                    if focused && enter {
                        commit = true;
                    }
                    if esc {
                        cancel = true;
                    }
                });
        });
        if commit {
            if let Some((l, c)) = parse_goto(&text) {
                self.goto(l, c.unwrap_or(1));
            }
            ui.memory_mut(|m| m.request_focus(self.id()));
        } else if cancel {
            ui.memory_mut(|m| m.request_focus(self.id()));
        } else {
            self.goto = Some(text);
        }
        Some(rect)
    }
}

/// `"12"` 또는 `"12:5"` 형태를 (줄, 열)로 해석한다.
pub(crate) fn parse_goto(s: &str) -> Option<(usize, Option<usize>)> {
    let s = s.trim().trim_start_matches(':');
    let mut it = s.splitn(2, [':', ',']);
    let line = it.next()?.trim().parse::<usize>().ok()?;
    let col = match it.next() {
        Some(c) if !c.trim().is_empty() => Some(c.trim().parse::<usize>().ok()?),
        _ => None,
    };
    Some((line.max(1), col))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        Editor::from_text("t.txt", text)
    }

    #[test]
    fn find_next_wraps_and_prev_goes_back() {
        let mut e = ed("foo bar foo\nfoo");
        e.find.open = true;
        e.find.query = "foo".into();
        e.update_find_matches();
        assert_eq!(e.find.matches.len(), 3);
        assert_eq!(e.sel.range(), (Pos::new(0, 0), Pos::new(0, 3)));
        e.find_next(true);
        assert_eq!(e.sel.range().0, Pos::new(0, 8));
        e.find_next(true);
        assert_eq!(e.sel.range().0, Pos::new(1, 0));
        e.find_next(true);
        assert_eq!(e.sel.range().0, Pos::new(0, 0));
        e.find_next(false);
        assert_eq!(e.sel.range().0, Pos::new(1, 0));
    }

    #[test]
    fn replace_current_then_all_with_regex_groups() {
        let mut e = ed("a=1, b=2, c=3");
        e.find.open = true;
        e.find.query = r"(\w)=(\d)".into();
        e.find.opts.regex = true;
        e.find.replacement = "$2:$1".into();
        e.update_find_matches();
        e.replace_current();
        assert_eq!(e.text(), "1:a, b=2, c=3");
        assert_eq!(e.replace_all(), 2);
        assert_eq!(e.text(), "1:a, 2:b, 3:c");
        e.undo();
        assert_eq!(e.text(), "1:a, b=2, c=3");
    }

    #[test]
    fn replace_all_literal_is_single_undo_step() {
        let mut e = ed("x.x.x");
        e.find.open = true;
        e.find.query = ".".into();
        e.find.replacement = "$1".into();
        assert_eq!(e.replace_all(), 2);
        assert_eq!(e.text(), "x$1x$1x");
        e.undo();
        assert_eq!(e.text(), "x.x.x");
    }

    #[test]
    fn goto_input_parsing() {
        assert_eq!(parse_goto("12"), Some((12, None)));
        assert_eq!(parse_goto("12:5"), Some((12, Some(5))));
        assert_eq!(parse_goto(":7"), Some((7, None)));
        assert_eq!(parse_goto("x"), None);
        assert_eq!(parse_goto("3:y"), None);
    }
}
