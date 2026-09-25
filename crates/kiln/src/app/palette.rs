//! 명령 팔레트: 세션·명령·도구·스페이스·설정을 한 검색창에서 찾는다.

use kiln_common::icons::{self, Icon};
use kiln_common::widgets;
use egui::{Align2, Color32, CornerRadius, Frame, Key, Margin, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use kiln_common::{Theme, fonts};

#[derive(Default)]
pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Group {
    Sessions,
    Commands,
    Tools,
    Spaces,
    Settings,
}

impl Group {
    fn label(&self) -> &'static str {
        match self {
            Group::Sessions => "세션",
            Group::Commands => "명령",
            Group::Tools => "도구",
            Group::Spaces => "스페이스",
            Group::Settings => "설정",
        }
    }
}

pub struct Item<A> {
    pub group: Group,
    pub icon: Icon,
    pub label: String,
    pub hint: String,
    pub action: A,
}

/// 부분 수열 매칭 점수. 매칭되지 않으면 None.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev_match: Option<usize> = None;
    for qc in query.to_lowercase().chars() {
        if qc == ' ' {
            continue;
        }
        let mut found = None;
        while ti < t.len() {
            if t[ti] == qc {
                found = Some(ti);
                ti += 1;
                break;
            }
            ti += 1;
        }
        let i = found?;
        score += 10;
        if prev_match.is_some_and(|p| p + 1 == i) {
            score += 15;
        }
        if i == 0 || !t[i - 1].is_alphanumeric() {
            score += 20;
        }
        score -= i as i32 / 4;
        prev_match = Some(i);
    }
    Some(score)
}

impl Palette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn ui<A>(&mut self, ctx: &egui::Context, items: Vec<Item<A>>) -> Option<A> {
        if !self.open {
            return None;
        }
        let t = Theme::current();
        let mut scored: Vec<(i32, Item<A>)> = items.into_iter().filter_map(|it| fuzzy_score(&self.query, &it.label).map(|s| (s, it))).collect();
        if self.query.is_empty() {
            scored.sort_by_key(|(_, it)| it.group);
        } else {
            scored.sort_by_key(|x| std::cmp::Reverse(x.0));
        }
        let n = scored.len();
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.open = false;
            return None;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowDown)) && n > 0 {
            self.selected = (self.selected + 1) % n;
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp)) && n > 0 {
            self.selected = (self.selected + n - 1) % n;
        }
        self.selected = self.selected.min(n.saturating_sub(1));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let mut chosen: Option<usize> = if enter && n > 0 { Some(self.selected) } else { None };

        let screen = ctx.content_rect();
        // 뒤를 어둡게.
        let mut close_by_backdrop = false;
        egui::Area::new(egui::Id::new("palette-dim")).fixed_pos(screen.min).order(egui::Order::Foreground).show(ctx, |ui| {
            let r = ui.allocate_rect(screen, Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(if t.dark { 120 } else { 50 }));
            close_by_backdrop = r.clicked();
        });
        let width = 620.0f32.min(screen.width() - 40.0);
        egui::Area::new(egui::Id::new("palette"))
            .fixed_pos(pos2(screen.center().x - width / 2.0, screen.top() + screen.height() * 0.14))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border_strong))
                    .corner_radius(CornerRadius::same(14))
                    .shadow(t.shadow())
                    .inner_margin(Margin::same(0))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        // 검색 입력.
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            let (ir, _) = ui.allocate_exact_size(vec2(18.0, 52.0), Sense::hover());
                            icons::paint(ui.painter(), egui::Rect::from_center_size(ir.center(), vec2(17.0, 17.0)), Icon::Search, t.text_faint);
                            ui.add_space(6.0);
                            let te = ui.add(
                                egui::TextEdit::singleline(&mut self.query)
                                    .hint_text(RichText::new("무엇을 할까요? 세션, 명령, 테마…").color(t.text_faint))
                                    .font(fonts::regular(16.0))
                                    .frame(egui::Frame::NONE)
                                    .desired_width(width - 60.0),
                            );
                            if self.just_opened {
                                te.request_focus();
                                self.just_opened = false;
                            }
                            if te.changed() {
                                self.selected = 0;
                            }
                        });
                        let (sep, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                        ui.painter().rect_filled(sep, 0.0, t.border);
                        egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.add_space(6.0);
                            let mut last_group = None;
                            for (i, (_, it)) in scored.iter().enumerate() {
                                if self.query.is_empty() && last_group != Some(it.group) {
                                    last_group = Some(it.group);
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(18.0);
                                        ui.label(RichText::new(it.group.label()).font(fonts::semibold(11.0)).color(t.text_faint));
                                    });
                                }
                                let sel = i == self.selected;
                                let (row, resp) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
                                let r = row.shrink2(vec2(8.0, 1.0));
                                if sel {
                                    ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_selected);
                                    resp.scroll_to_me(None);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_hover);
                                }
                                let ib = egui::Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 12.0), vec2(24.0, 24.0));
                                ui.painter().rect_filled(ib, CornerRadius::same(6), if sel { t.accent_soft(40) } else { t.bg_hover });
                                icons::paint(ui.painter(), ib.shrink(6.0), it.icon, if sel { t.accent } else { t.text_dim });
                                ui.painter().with_clip_rect(r.shrink2(vec2(0.0, 0.0))).text(pos2(ib.right() + 12.0, r.center().y), Align2::LEFT_CENTER, &it.label, fonts::regular(13.5), t.text);
                                if !it.hint.is_empty() {
                                    let keys = widgets::split_keys(&it.hint);
                                    let mut kui = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(pos2(r.right() - 160.0, r.center().y - 9.0), pos2(r.right() - 10.0, r.center().y + 9.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
                                    let rev: Vec<&str> = keys.iter().rev().map(|s| s.as_str()).collect();
                                    widgets::keycaps(&mut kui, &rev);
                                }
                                if resp.clicked() {
                                    chosen = Some(i);
                                }
                                if resp.hovered() && ui.input(|i| i.pointer.delta().length() > 0.0) {
                                    self.selected = i;
                                }
                            }
                            if n == 0 {
                                ui.add_space(20.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new("일치하는 항목이 없습니다").color(t.text_faint));
                                });
                                ui.add_space(20.0);
                            }
                            ui.add_space(6.0);
                        });
                        let (sep, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                        ui.painter().rect_filled(sep, 0.0, t.border);
                        ui.horizontal(|ui| {
                            ui.set_height(32.0);
                            ui.add_space(16.0);
                            let hint = |ui: &mut egui::Ui, k: &[&str], s: &str| {
                                widgets::keycaps(ui, k);
                                ui.label(RichText::new(s).size(11.5).color(t.text_faint));
                                ui.add_space(10.0);
                            };
                            hint(ui, &["↑", "↓"], "이동");
                            hint(ui, &["↩"], "실행");
                            hint(ui, &["esc"], "닫기");
                        });
                    });
            });
        let _ = StrokeKind::Inside;
        if close_by_backdrop && chosen.is_none() {
            self.open = false;
            return None;
        }
        if let Some(i) = chosen {
            self.open = false;
            return scored.into_iter().nth(i).map(|(_, it)| it.action);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_prefers_word_starts_and_contiguous() {
        assert!(fuzzy_score("spl", "Split Right").unwrap() > fuzzy_score("spl", "Simple pull").unwrap());
        assert!(fuzzy_score("xyz", "Split").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
