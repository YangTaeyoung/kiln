//! 커맨드 팔레트.

use egui::{Color32, Key, RichText};

#[derive(Default)]
pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
}

pub struct Item<A> {
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
        let theme = kiln_common::Theme::current();
        let mut scored: Vec<(i32, Item<A>)> = items.into_iter().filter_map(|it| fuzzy_score(&self.query, &it.label).map(|s| (s, it))).collect();
        scored.sort_by_key(|x| std::cmp::Reverse(x.0));
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
        let width = 560.0f32.min(screen.width() - 40.0);
        egui::Area::new(egui::Id::new("palette"))
            .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 80.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(theme.bg_elevated).inner_margin(8.0).corner_radius(8.0).show(ui, |ui| {
                    ui.set_width(width);
                    let te = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("명령 검색…").desired_width(f32::INFINITY).font(egui::TextStyle::Body));
                    if self.just_opened {
                        te.request_focus();
                        self.just_opened = false;
                    }
                    if te.changed() {
                        self.selected = 0;
                    }
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for (i, (_, it)) in scored.iter().enumerate() {
                            let sel = i == self.selected;
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
                            if sel {
                                ui.painter().rect_filled(rect, 4.0, theme.bg_selected);
                                resp.scroll_to_me(None);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 4.0, theme.bg_hover);
                            }
                            ui.painter().text(rect.left_center() + egui::vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &it.label, egui::FontId::proportional(13.0), theme.text);
                            ui.painter().text(rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, &it.hint, egui::FontId::proportional(11.5), theme.text_faint);
                            if resp.clicked() {
                                chosen = Some(i);
                            }
                        }
                        if n == 0 {
                            ui.label(RichText::new("일치하는 명령 없음").color(Color32::GRAY));
                        }
                    });
                });
            });
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
