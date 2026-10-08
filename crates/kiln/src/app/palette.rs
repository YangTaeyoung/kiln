//! 명령 팔레트: 세션·명령·도구·스페이스·설정을 한 검색창에서 찾는다.

use kiln_common::icons::{self, Icon};
use kiln_common::widgets;
use egui::{Align2, Color32, CornerRadius, Frame, Key, Margin, RichText, Sense, Stroke, pos2, vec2};
use kiln_common::{Theme, fonts};

#[derive(Default)]
pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
    recent_only: bool,
    project_filter: String,
    frozen: Vec<String>,
    recent_current: Option<String>,
    selected_key: Option<String>,
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
            Group::Sessions => kiln_common::i18n::tr("열린 작업 · 실행 중 우선"),
            Group::Commands => kiln_common::i18n::tr("명령"),
            Group::Tools => kiln_common::i18n::tr("도구"),
            Group::Spaces => kiln_common::i18n::tr("워크스페이스"),
            Group::Settings => kiln_common::i18n::tr("설정"),
        }
    }
}

pub struct Item<A> {
    pub key: String,
    pub project: String,
    pub group: Group,
    pub icon: Icon,
    pub label: String,
    pub hint: String,
    pub detail: String,
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
        self.recent_only = false;
        self.open = true;
        self.query.clear();
        self.project_filter.clear();
        self.frozen.clear();
        self.recent_current = None;
        self.selected_key = None;
        self.selected = 0;
        self.just_opened = true;
    }

    pub fn open_recent(&mut self, current: Option<String>) { self.open(); self.recent_only = true; self.recent_current = current; }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn ui<A>(&mut self, ctx: &egui::Context, items: Vec<Item<A>>) -> Option<A> {
        if !self.open { return None; }
        let mut items: Vec<_> = items.into_iter().filter(|item| !self.recent_only || item.group == Group::Sessions).collect();
        if self.just_opened {
            self.frozen=items.iter().map(|i|i.key.clone()).collect();
            if self.recent_only {
                self.selected=items.iter().position(|i|Some(&i.key)!=self.recent_current.as_ref()).unwrap_or(0);
            }
        }
        items.retain(|i|self.frozen.contains(&i.key));
        items.sort_by_key(|i|self.frozen.iter().position(|k|k==&i.key).unwrap_or(usize::MAX));
        let mut projects:Vec<_>=items.iter().filter(|i|!i.project.is_empty()).map(|i|i.project.clone()).collect(); projects.sort(); projects.dedup();
        let t = Theme::current();
        let screen = ctx.content_rect();
        let width = 640.0f32.min(screen.width() - 48.0);
        let mut chosen = None;
        let mut scored = Vec::new();
        let frame = Frame::new().fill(t.bg_elevated).stroke(Stroke::new(1.0, t.border_strong))
            .corner_radius(CornerRadius::same(12)).shadow(t.shadow()).inner_margin(Margin::same(12));
        let modal = egui::Modal::new(egui::Id::new("palette")).frame(frame)
            .backdrop_color(Color32::from_black_alpha(if t.dark { 130 } else { 60 }))
            .show(ctx, |ui| {
                ui.set_width(width);
                let opened = self.just_opened;
                let mut changed = false;
                ui.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(vec2(22.0, 38.0), Sense::hover());
                    icons::paint(ui.painter(), egui::Rect::from_center_size(ir.center(), vec2(17.0, 17.0)), Icon::Search, t.accent);
                    let te = ui.add(egui::TextEdit::singleline(&mut self.query)
                        .hint_text(if self.recent_only { kiln_common::i18n::tr("열린 작업 검색 · 파일, 터미널, Git, DB…") } else { kiln_common::i18n::tr("작업, 명령, 도구 검색…") }).font(fonts::regular(16.0))
                        .frame(egui::Frame::NONE).desired_width(width - 50.0));
                    if opened { te.request_focus(); self.just_opened = false; }
                    changed = te.changed();
                    if changed { self.selected = 0; self.selected_key = None; }
                });
                if self.recent_only {
                    ui.horizontal(|ui|{
                        ui.label(kiln_common::i18n::tr("워크스페이스"));
                        let menu_width=(ui.available_width()-16.0).min(420.0);
                        egui::ComboBox::from_id_salt("recent-project-filter").width(menu_width).truncate().selected_text(if self.project_filter.is_empty(){kiln_common::i18n::tr("전체")}else{&self.project_filter}).show_ui(ui,|ui|{
                            if ui.selectable_value(&mut self.project_filter,String::new(),kiln_common::i18n::tr("전체")).changed(){changed=true;}
                            ui.set_min_width(menu_width); ui.set_max_width(menu_width);
                            for project in &projects {
                                let response=ui.add_sized([menu_width,28.0],egui::Button::selectable(self.project_filter==*project,project).truncate()).on_hover_text(project);
                                if response.clicked(){self.project_filter=project.clone();changed=true;ui.close();}
                            }
                        });
                    });
                }
                widgets::divider(ui);
                // Filter after editing, so typing and Enter in one frame use the visible query.
                scored = items.into_iter().filter(|i|self.project_filter.is_empty() || i.project==self.project_filter).filter_map(|it| fuzzy_score(&self.query, &format!("{} {} {}", it.label, it.hint, it.detail)).map(|s| (s, it))).collect();
                if self.query.trim().is_empty() { scored.sort_by_key(|(_, it)| it.group); }
                else { scored.sort_by_key(|x| std::cmp::Reverse(x.0)); }
                let n = scored.len();
                if changed { self.selected_key = None; }
                if let Some(key)=&self.selected_key {
                    if let Some(index)=scored.iter().position(|(_,item)|&item.key==key){self.selected=index;}
                }
                let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
                let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
                self.selected = self.selected.min(n.saturating_sub(1));
                if down && n > 0 { self.selected = (self.selected + 1) % n; }
                if up && n > 0 { self.selected = (self.selected + n - 1) % n; }
                self.selected_key=scored.get(self.selected).map(|(_,item)|item.key.clone());
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) && n > 0 { chosen = Some(self.selected); }
                let follow_selection = opened || changed || down || up;
                egui::ScrollArea::vertical().id_salt("palette-results")
                    .max_height((screen.height() - 180.0).clamp(100.0, 420.0)).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add_space(8.0);
                    let mut last_group = None;
                    for (i, (_, it)) in scored.iter().enumerate() {
                        if self.query.trim().is_empty() && last_group != Some(it.group) {
                            last_group = Some(it.group);
                            ui.add_space(8.0);
                            ui.label(RichText::new(it.group.label()).font(fonts::semibold(11.5)).color(t.text_dim));
                            ui.add_space(4.0);
                        }
                        let sel = i == self.selected;
                        let (row, response) = ui.allocate_exact_size(vec2(ui.available_width(), if it.detail.is_empty(){38.0}else{54.0}), Sense::click());
                        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, format!("{}\n{}",it.label,it.detail)));
                        if sel || response.hovered() { ui.painter().rect_filled(row, CornerRadius::same(6), if sel { t.bg_selected } else { t.bg_hover }); }
                        if sel && follow_selection { response.scroll_to_me(None); }
                        let icon = egui::Rect::from_center_size(pos2(row.left() + 20.0, row.center().y), vec2(15.0, 15.0));
                        icons::paint(ui.painter(), icon, it.icon, if sel { t.accent } else { t.text_dim });
                        let hint_width = if it.hint.is_empty() { 12.0 } else { 120.0 };
                        let text_rect = egui::Rect::from_min_max(pos2(row.left() + 42.0, row.top()), pos2(row.right() - hint_width, row.bottom()));
                        let mut job = egui::text::LayoutJob::simple(it.label.clone(), fonts::regular(13.5), t.text, text_rect.width());
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let galley = ui.painter().layout_job(job);
                        let title_y=if it.detail.is_empty(){row.center().y-galley.size().y*0.5}else{row.top()+7.0};
                        ui.painter().galley(pos2(text_rect.left(), title_y), galley, t.text);
                        if !it.detail.is_empty() {
                            let mut detail=egui::text::LayoutJob::simple(it.detail.clone(),fonts::regular(11.5),t.text_dim,text_rect.width());
                            detail.wrap.max_rows=1; detail.wrap.break_anywhere=true;
                            let galley=ui.painter().layout_job(detail);
                            ui.painter().galley(pos2(text_rect.left(),row.top()+29.0),galley,t.text_dim);
                        }
                        if !it.hint.is_empty() {
                            ui.painter().text(pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, &it.hint, fonts::medium(11.5), t.text_dim);
                        }
                        let response = response.on_hover_text(format!("{}\n{}",it.label,it.detail));
                        if response.clicked() { chosen = Some(i); }
                        if response.hovered() && ui.input(|i| i.pointer.delta().length() > 0.0) { self.selected = i; }
                    }
                    if n == 0 {
                        ui.add_space(28.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(kiln_common::i18n::tr("검색 결과가 없습니다")).font(fonts::semibold(14.0)));
                            ui.label(RichText::new(if self.recent_only {kiln_common::i18n::tr("다른 검색어를 입력하거나 워크스페이스 필터를 전체로 바꾸세요.")} else {kiln_common::i18n::tr("다른 검색어를 입력하거나 검색어를 지워 전체 명령을 확인하세요.")}).color(t.text_dim));
                            if (!self.query.is_empty() || !self.project_filter.is_empty()) && ui.button(kiln_common::i18n::tr("검색 초기화")).clicked(){self.query.clear();self.project_filter.clear();self.selected=0;}
                        });
                        ui.add_space(28.0);
                    }
                    ui.add_space(8.0);
                });
                widgets::divider(ui);
                ui.horizontal(|ui| {
                    ui.set_height(28.0);
                    ui.label(RichText::new(if self.recent_only {kiln_common::i18n::tr("↑ ↓ 선택     ↩ 이동     Esc 닫기")}else{kiln_common::i18n::tr("↑ ↓ 선택     ↩ 실행     Esc 닫기")}).size(11.5).color(t.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(kiln_common::trf!("{}개 결과", scored.len())).size(11.5).color(t.text_dim));
                    });
                });
            });
        if modal.should_close() { self.open = false; return None; }
        if let Some(i) = chosen {
            self.open = false;
            return scored.into_iter().nth(i).map(|(_, item)| item.action);
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

#[cfg(test)]
mod responsive_tests {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    #[test]
    fn running_first_recent_selection_skips_current_and_freezes_live_activity_order() {
        let mut palette=Palette::default();palette.open_recent(Some("current".into()));
        let mut initialized=false;
        let mut reordered=false;
        let mut chosen=None;
        let mut h=Harness::builder().with_size([800.0,600.0]).build_ui_state(|ui,p:&mut Palette| {
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            let keys=if reordered{["idle","current","running"]}else{["running","current","idle"]};
            let items=keys.into_iter().map(|key|Item{key:key.into(),project:"fixture".into(),group:Group::Sessions,icon:Icon::Terminal,label:key.into(),hint:String::new(),detail:String::new(),action:key}).collect();
            if let Some(action)=p.ui(ui.ctx(),items){chosen=Some(action);}
            reordered=true;
        },palette);
        h.run_steps(4);
        assert_eq!(h.state().selected,0);
        assert_eq!(h.state().frozen,["running","current","idle"]);
        h.key_press(egui::Key::Enter);h.run_steps(2);drop(h);
        assert_eq!(chosen,Some("running"));
    }
    #[test]
    fn recent_project_menu_stays_inside_minimum_viewport_and_resets_empty_filter() {
        let project="매우 긴 프로젝트 이름 · /workspace/".repeat(8);
        let mut palette=Palette::default();palette.open_recent(None);
        let mut initialized=false;
        let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).build_ui_state(|ui,p:&mut Palette| {
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            p.ui(ui.ctx(),vec![Item{key:"fixture".into(),project:project.clone(),group:Group::Sessions,icon:Icon::Terminal,label:"터미널 1.1".into(),hint:"터미널".into(),detail:"작업 1".into(),action:()}]);
        },palette);
        h.run_steps(4);h.get_by_value("전체").click();h.run_steps(3);
        let row=h.get_by_label(&project).rect();assert!(h.ctx.content_rect().contains_rect(row),"{row:?}");
        h.render().unwrap().save("/tmp/kiln-project-filter-minimum.png").unwrap();
        h.get_by_label(&project).click();h.run_steps(2);
        h.state_mut().query="does-not-match".into();h.run_steps(3);
        h.get_by_label("검색 초기화").click();h.run_steps(3);
        assert!(h.state().query.is_empty() && h.state().project_filter.is_empty());
    }
}
