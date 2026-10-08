//! 설정 화면: 왼쪽 분류, 오른쪽 카드형 항목. 바꾸면 바로 적용된다.

use kiln_common::icons::{self, Icon};
use super::state::Settings;
use kiln_common::widgets::{self, ButtonKind};
use super::Action;
use egui::{Align2, Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use kiln_common::{Theme, fonts};

#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    pub section: usize,
}

pub struct AboutInfo {
    pub daemon_pid: u32,
    pub daemon_build: String,
    pub connected: bool,
}

const SECTIONS: [(&str, Icon); 6] = [
    ("모양", Icon::Sparkle),
    ("터미널", Icon::Terminal),
    ("동작", Icon::Gear),
    ("계정", Icon::Person),
    ("단축키", Icon::Command),
    ("앱 정보", Icon::Bell),
];

impl SettingsUi {
    pub fn ui(&mut self, ctx: &egui::Context, s: &mut Settings, about: &AboutInfo, accounts: &kiln_accounts::AccountManager, keymap: &mut super::keymap::Keymap) -> Vec<Action> {
        let t = Theme::current();
        let mut acts = Vec::new();
        let screen = ctx.content_rect();
        let size = vec2(860.0f32.min(screen.width() - 60.0), 580.0f32.min(screen.height() - 80.0));
        let compact = size.y < 340.0 || size.x < 600.0;
        let nav_width = if compact { 0.0 } else {
            ctx.fonts_mut(|fonts| SECTIONS.iter().map(|(label, _)| {
                fonts.layout_no_wrap(kiln_common::i18n::tr(label).to_owned(), fonts::medium(13.5), t.text).size().x
            }).fold(100.0_f32, f32::max)) + 60.0
        };
        let mut close = false;
        let frame = Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(16)).shadow(t.shadow()).inner_margin(Margin::same(0));
        let modal = egui::Modal::new(egui::Id::new("settings")).frame(frame).backdrop_color(Color32::from_black_alpha(if t.dark { 130 } else { 60 })).show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // 왼쪽 분류.
                let (nav, _) = ui.allocate_exact_size(vec2(nav_width, size.y), Sense::hover());
                if !compact {
                ui.painter().rect_filled(nav, CornerRadius { nw: 16, sw: 16, ne: 0, se: 0 }, widgets::lerp_color(t.bg_panel, t.bg_elevated, 0.35));
                ui.painter().line_segment([nav.right_top(), nav.right_bottom()], Stroke::new(1.0, t.border));
                ui.painter().text(pos2(nav.left() + 22.0, nav.top() + 30.0), Align2::LEFT_CENTER, kiln_common::i18n::tr("설정"), fonts::semibold(17.0), t.text);
                for (i, (label, icon)) in SECTIONS.iter().enumerate() {
                    let r = egui::Rect::from_min_size(pos2(nav.left() + 12.0, nav.top() + 60.0 + i as f32 * 36.0), vec2(nav.width() - 24.0, 32.0));
                    let resp = ui.interact(r, ui.id().with(("nav", i)), Sense::click());
                    let sel = self.section == i;
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), sel, kiln_common::i18n::tr(label)));
                    widgets::focus_ring(ui, &resp, 8);
                    if sel {
                        ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_selected);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_hover);
                    }
                    icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(15.0, 15.0)), *icon, if sel { t.accent } else { t.text_dim });
                    ui.painter().text(pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, kiln_common::i18n::tr(label), fonts::medium(13.5), if sel { t.text } else { t.text_dim });
                    if resp.clicked() {
                        self.section = i;
                    }
                }
                ui.painter().text(pos2(nav.left() + 22.0, nav.bottom() - 22.0), Align2::LEFT_CENTER, format!("Kiln {}", env!("CARGO_PKG_VERSION")), fonts::regular(11.5), t.text_faint);

                }
                // 오른쪽 내용.
                let content = egui::Rect::from_min_max(pos2(nav.right(), nav.top()), pos2(nav.right() + size.x - nav_width, nav.bottom()));
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content.shrink2(if compact { vec2(16.0, 14.0) } else { vec2(24.0, 22.0) })).layout(egui::Layout::top_down(egui::Align::Min)));
                cui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x=8.0;
                    if compact {
                        ui.label(RichText::new(kiln_common::i18n::tr("설정")).font(fonts::semibold(17.0)));
                        egui::ComboBox::from_id_salt("settings-sections").selected_text(kiln_common::i18n::tr(SECTIONS[self.section].0)).width(130.0).show_ui(ui, |ui| {
                            for (index, (label, _)) in SECTIONS.iter().enumerate() { ui.selectable_value(&mut self.section, index, kiln_common::i18n::tr(label)); }
                        });
                    } else {
                        ui.label(RichText::new(kiln_common::i18n::tr(SECTIONS[self.section].0)).font(fonts::semibold(20.0)).color(t.text));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, 30.0, false, kiln_common::i18n::tr("닫기 (Esc)")).clicked() {
                            close = true;
                        }
                    });
                });
                cui.label(RichText::new(if self.section==4 {kiln_common::i18n::tr("변경 후 ‘단축키 저장’을 눌러 적용하세요")}else{kiln_common::i18n::tr("변경 사항은 자동으로 저장됩니다")}).size(12.0).color(t.text_dim));
                cui.add_space(if self.section==4 {8.0}else{16.0});
                if self.section == 4 {
                    let area = cui.available_rect_before_wrap();
                    let footer_top = (area.bottom() - keymap.footer_height()).max(area.top() + 32.0);
                    let body_rect = egui::Rect::from_min_max(area.min, pos2(area.right(), footer_top - 4.0));
                    let mut body = cui.new_child(egui::UiBuilder::new().max_rect(body_rect).layout(egui::Layout::top_down(egui::Align::Min)));
                    body.spacing_mut().scroll.floating = false;
                    body.spacing_mut().scroll.dormant_handle_opacity = 0.65;
                    egui::ScrollArea::vertical().id_salt("keybinding-edit-list").max_height(body_rect.height()).auto_shrink([false,false]).show(&mut body,|ui| {
                        ui.set_width((content.width()-64.0).max(200.0));
                        keymap.fields_ui(ui); ui.separator(); shortcuts(ui);
                    });
                    let mut footer = cui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(pos2(area.left(),footer_top),area.max)).layout(egui::Layout::top_down(egui::Align::Min)));
                    keymap.footer_ui(&mut footer);
                } else {
                egui::ScrollArea::vertical().id_salt(("settings-section", self.section)).auto_shrink([false, false]).show(&mut cui, |ui| {
                    ui.set_width((content.width() - 64.0).max(200.0));
                    match self.section {
                        0 => appearance(ui, s, &mut acts),
                        1 => terminal(ui, s),
                        2 => behavior(ui, s),
                        3 => accounts_page(ui, accounts, &mut acts, &mut close),
                        _ => about_page(ui, about, &mut acts),
                    }
                });
                }
            });
        });
        if close || modal.should_close() {
            self.open = false;
        }
        acts
    }
}

fn accounts_page(ui: &mut egui::Ui, mgr: &kiln_accounts::AccountManager, acts: &mut Vec<Action>, close: &mut bool) {
    use kiln_accounts::AccountsEvent;
    for e in kiln_accounts::accounts_settings_ui(ui, mgr) {
        match e {
            AccountsEvent::RunLogin(tool) => {
                acts.push(Action::RunLogin(tool));
                *close = true;
            }
            AccountsEvent::Switched { tool, .. } => {
                let name = mgr.active(tool).and_then(|id| mgr.profiles(tool).into_iter().find(|p| p.id == id)).map(|p| p.label).unwrap_or_default();
                acts.push(Action::Toast(kiln_common::trf!("{} 계정을 {name}(으)로 바꿨습니다. 새로 시작하는 세션부터 적용됩니다", tool.display_name())));
            }
            AccountsEvent::Saved { tool, .. } => acts.push(Action::Toast(kiln_common::trf!("{} 계정을 목록에 저장했습니다", tool.display_name()))),
        }
    }
}

fn appearance(ui: &mut egui::Ui, s: &mut Settings, acts: &mut Vec<Action>) {
    let t = Theme::current();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let label = ui.label(kiln_common::i18n::tr("언어"));
        let current = kiln_common::i18n::language();
        let mut selected = current;
        egui::ComboBox::from_id_salt("interface-language")
            .selected_text(current.native_name())
            .show_ui(ui, |ui| {
                for language in kiln_common::i18n::Language::ALL {
                    ui.selectable_value(&mut selected, language, language.native_name());
                }
            }).response.labelled_by(label.id);
        if selected != current { acts.push(Action::SetLanguage(selected)); }
    });
    ui.add_space(20.0);
    ui.label(RichText::new(kiln_common::i18n::tr("테마")).font(fonts::semibold(12.0)).color(t.text_faint));
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for th in Theme::ALL {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 74.0), Sense::click());
            let sel = s.theme == th.name;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), sel, kiln_common::i18n::tr(th.label)));
            widgets::focus_ring(ui, &resp, 10);
            // 미니 미리보기: 레일 + 카드 두 장.
            let preview = egui::Rect::from_min_size(r.min, vec2(r.width(), 46.0));
            ui.painter().rect_filled(preview, CornerRadius::same(10), widgets::canvas_color(&th));
            let rail = egui::Rect::from_min_size(preview.min + vec2(8.0, 10.0), vec2(22.0, preview.height() - 20.0));
            ui.painter().rect_filled(rail, CornerRadius::same(5), th.bg_elevated);
            ui.painter().rect_filled(egui::Rect::from_min_size(rail.min + vec2(5.0, 5.0), vec2(12.0, 12.0)), CornerRadius::same(3), th.accent);
            let c1 = egui::Rect::from_min_max(pos2(rail.right() + 6.0, rail.top()), pos2(preview.center().x + 18.0, rail.bottom()));
            let c2 = egui::Rect::from_min_max(pos2(c1.right() + 5.0, rail.top()), pos2(preview.right() - 8.0, rail.bottom()));
            for (c, lines) in [(c1, [th.green, th.text_dim, th.blue]), (c2, [th.purple, th.text_faint, th.yellow])] {
                ui.painter().rect_filled(c, CornerRadius::same(5), th.bg);
                ui.painter().rect_stroke(c, CornerRadius::same(5), Stroke::new(1.0, th.border), StrokeKind::Inside);
                for (li, col) in lines.iter().enumerate() {
                    let y = c.top() + 8.0 + li as f32 * 5.0;
                    ui.painter().line_segment([pos2(c.left() + 6.0, y), pos2(c.left() + 6.0 + (c.width() - 14.0) * (0.9 - li as f32 * 0.22), y)], Stroke::new(2.5, *col));
                }
            }
            let stroke = if sel { Stroke::new(2.0, t.accent) } else if resp.hovered() { Stroke::new(1.0, t.border_strong) } else { Stroke::new(1.0, t.border) };
            ui.painter().rect_stroke(preview, CornerRadius::same(10), stroke, StrokeKind::Outside);
            ui.painter().text(pos2(r.left() + 2.0, preview.bottom() + 15.0), Align2::LEFT_CENTER, kiln_common::i18n::tr(th.label), fonts::medium(13.0), if sel { t.text } else { t.text_dim });
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() && !sel {
                acts.push(Action::SetTheme(th.name.into()));
            }
        }
    });
    ui.add_space(18.0);
    widgets::group(ui, kiln_common::i18n::tr("화면"), |ui| {
        widgets::setting_row(ui, kiln_common::i18n::tr("UI 크기"), kiln_common::i18n::tr("창 전체의 글자와 여백 비율"), |ui| {
            let mut v = (s.ui_scale * 100.0).round() as i32;
            let before = v;
            let mut opts = [(90, kiln_common::i18n::tr("작게")), (100, kiln_common::i18n::tr("보통")), (115, kiln_common::i18n::tr("크게")), (130, kiln_common::i18n::tr("아주 크게"))].to_vec();
            if !opts.iter().any(|(x, _)| *x == v) {
                opts.push((v, kiln_common::i18n::tr("사용자 지정")));
            }
            widgets::segmented(ui, &mut v, &opts);
            if v != before {
                s.ui_scale = v as f32 / 100.0;
                ui.ctx().set_zoom_factor(s.ui_scale);
            }
        });
        widgets::divider(ui);
        widgets::setting_row(ui, kiln_common::i18n::tr("패널 간격"), kiln_common::i18n::tr("터미널·에디터 패널 사이 여백"), |ui| {
            ui.add(egui::Slider::new(&mut s.card_gap, 2.0..=16.0).step_by(1.0).suffix(" px").show_value(true));
        });
    });
}

fn terminal(ui: &mut egui::Ui, s: &mut Settings) {
    let t = Theme::current();
    // 미리보기.
    let fid = fonts::mono(s.font_size);
    let lh = ui.fonts_mut(|f| f.row_height(&fid)) * s.line_height;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (lh * 4.0 + 28.0).max(118.0)), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(12), t.bg);
    ui.painter().rect_stroke(r, CornerRadius::same(12), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let lines: [&[(&str, Color32)]; 4] = [
        &[("~/dev/kiln ", t.blue), ("main ", t.purple), ("❯ ", t.green), ("claude", t.text)],
        &[("● ", t.orange), (kiln_common::i18n::tr("작업을 계획하는 중… "), t.text), (kiln_common::i18n::tr("(Esc로 중단)"), t.text_faint)],
        &[("fn ", t.purple), ("main", t.blue), ("() { println!(", t.text), (kiln_common::i18n::tr("\"안녕, Kiln\""), t.green), ("); }", t.text)],
        &[("❯ ", t.green), ("cargo test ", t.text), ("-- --nocapture", t.text_dim)],
    ];
    let clip = ui.painter().with_clip_rect(r.shrink(2.0));
    for (i, segs) in lines.iter().enumerate() {
        let mut x = r.left() + 16.0;
        let y = r.top() + 14.0 + i as f32 * lh;
        for (txt, col) in segs.iter() {
            let g = ui.painter().layout_no_wrap(txt.to_string(), fid.clone(), *col);
            let w = g.size().x;
            clip.galley(pos2(x, y), g, *col);
            x += w;
        }
    }
    ui.add_space(16.0);
    widgets::group(ui, kiln_common::i18n::tr("글꼴"), |ui| {
        widgets::setting_row(ui, kiln_common::i18n::tr("글꼴 크기"), kiln_common::i18n::tr("⌘= / ⌘- 로도 바꿀 수 있습니다"), |ui| {
            ui.add(egui::Slider::new(&mut s.font_size, 9.0..=24.0).step_by(0.5).suffix(" pt"));
        });
        widgets::divider(ui);
        widgets::setting_row(ui, kiln_common::i18n::tr("줄 간격"), "", |ui| {
            let mut v = (s.line_height * 100.0).round() as i32;
            let before = v;
            widgets::segmented(ui, &mut v, &[(110, kiln_common::i18n::tr("좁게")), (120, kiln_common::i18n::tr("보통")), (140, kiln_common::i18n::tr("넓게"))]);
            if v != before {
                s.line_height = v as f32 / 100.0;
            }
        });
    });
    widgets::group(ui, kiln_common::i18n::tr("입력"), |ui| {
        widgets::setting_toggle(ui, kiln_common::i18n::tr("선택하면 복사"), kiln_common::i18n::tr("터미널에서 선택한 텍스트를 클립보드에 복사합니다"), &mut s.copy_on_select);
        widgets::divider(ui);
        widgets::setting_toggle(ui, kiln_common::i18n::tr("커서 깜박임"), kiln_common::i18n::tr("활성 터미널의 커서만 깜박입니다"), &mut s.cursor_blink);
        widgets::divider(ui);
        widgets::setting_toggle(ui, kiln_common::i18n::tr("Option 키를 Meta 로"), kiln_common::i18n::tr("Option+B 같은 조합을 셸 단축키로 보냅니다"), &mut s.option_as_meta);
    });
    widgets::group(ui, kiln_common::i18n::tr("셸"), |ui| {
        widgets::setting_row(ui, kiln_common::i18n::tr("기본 셸"), kiln_common::i18n::tr("비워 두면 로그인 셸($SHELL)을 씁니다"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut s.shell).hint_text("/bin/zsh").desired_width(200.0));
        });
    });
}

fn behavior(ui: &mut egui::Ui, s: &mut Settings) {
    widgets::group(ui, kiln_common::i18n::tr("안전"), |ui| {
        widgets::setting_toggle(ui, kiln_common::i18n::tr("실행 중인 프로세스를 종료하기 전 확인"), kiln_common::i18n::tr("패널·탭·워크스페이스를 닫을 때 실행 중인 프로세스가 있으면 확인합니다"), &mut s.confirm_close_running);
    });
    #[cfg(target_os = "macos")]
    crate::local_network::settings(ui);
    widgets::group(ui, kiln_common::i18n::tr("알림"), |ui| {
        widgets::setting_toggle(ui, kiln_common::i18n::tr("방해 금지"), kiln_common::i18n::tr("에이전트 팝업과 시스템 알림을 끕니다. 알림 센터에는 계속 보관됩니다."), &mut s.do_not_disturb);
        widgets::divider(ui);
        widgets::setting_toggle(ui, kiln_common::i18n::tr("앱 내 알림 팝업"), kiln_common::i18n::tr("새 에이전트 알림을 화면 모서리에 표시합니다"), &mut s.notification_toasts);
        widgets::divider(ui);
        widgets::setting_toggle(ui, kiln_common::i18n::tr("시스템 알림"), kiln_common::i18n::tr("창이 비활성일 때 에이전트 알림을 OS 알림으로도 보냅니다"), &mut s.os_notifications);
    });
}

fn shortcuts(ui: &mut egui::Ui) {
    let rows: [(&str, &[(&str, &str)]); 4] = [
        (kiln_common::i18n::tr("패널과 탭"), &[(kiln_common::i18n::tr("패널 이동"), "⌥⌘←"), (kiln_common::i18n::tr("탭 번호로 전환"), "⌥⌘1")]),
        (kiln_common::i18n::tr("탐색"), &[(kiln_common::i18n::tr("찾기"), "⌘F"), (kiln_common::i18n::tr("프로젝트 전환"), "⌘1")]),
        (kiln_common::i18n::tr("도구"), &[(kiln_common::i18n::tr("파일"), "⇧⌘E"), (kiln_common::i18n::tr("검색"), "⇧⌘F"), ("Git", "⇧⌘G"), ("GitHub", "⇧⌘R"), (kiln_common::i18n::tr("데이터베이스"), "⇧⌘B"), (kiln_common::i18n::tr("문제"), "⇧⌘M"), (kiln_common::i18n::tr("Git 로그"), "⇧⌘L")]),
        (kiln_common::i18n::tr("화면"), &[(kiln_common::i18n::tr("글꼴 크게/작게"), "⌘="), (kiln_common::i18n::tr("설정"), "⌘,")]),
    ];
    for (title, items) in rows {
        widgets::group(ui, title, |ui| {
            for (i, (label, keys)) in items.iter().enumerate() {
                if i > 0 {
                    widgets::divider(ui);
                }
                widgets::setting_row(ui, label, "", |ui| {
                    let ks = widgets::split_keys(keys);
                    let rev: Vec<&str> = ks.iter().rev().map(|s| s.as_str()).collect();
                    widgets::keycaps(ui, &rev);
                });
            }
        });
    }
}

fn about_page(ui: &mut egui::Ui, about: &AboutInfo, acts: &mut Vec<Action>) {
    let t = Theme::current();
    widgets::group(ui, kiln_common::i18n::tr("세션 데몬"), |ui| {
        widgets::setting_row(ui, kiln_common::i18n::tr("상태"), if about.connected { kiln_common::i18n::tr("앱을 닫아도 터미널 세션은 계속 실행됩니다.") } else { kiln_common::i18n::tr("연결되지 않음") }, |ui| {
            let (c, label) = if about.connected { (t.green, kiln_common::trf!("실행 중 · pid {}", about.daemon_pid)) } else { (t.red, kiln_common::i18n::tr("끊김").into()) };
            ui.label(RichText::new(label).color(t.text_dim));
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, c);
        });
        widgets::divider(ui);
        widgets::setting_row(ui, kiln_common::i18n::tr("데몬 빌드"), &about.daemon_build, |ui| {
            if widgets::button(ui, kiln_common::i18n::tr("이 버전으로 교체"), ButtonKind::Secondary).on_hover_text(kiln_common::i18n::tr("실행 중인 세션을 유지한 채 데몬만 바꿉니다")).clicked() {
                acts.push(Action::UpgradeDaemon);
            }
        });
    });
    widgets::group(ui, kiln_common::i18n::tr("앱"), |ui| {
        widgets::setting_row(ui, kiln_common::i18n::tr("버전"), &kiln_common::trf!("Kiln {} · 빌드 {}", env!("CARGO_PKG_VERSION"), kiln_daemon::build_id()), |_| {});
        #[cfg(target_os = "macos")]
        { widgets::divider(ui); super::updater::settings(ui); }
        widgets::divider(ui);
        widgets::setting_row(ui, kiln_common::i18n::tr("설정 폴더"), &kiln_common::paths::config_dir().display().to_string(), |ui| {
            if widgets::button(ui, kiln_common::i18n::tr("열기"), ButtonKind::Secondary).clicked() {
                let _ = open::that_detached(kiln_common::paths::config_dir());
            }
        });
    });
}

#[cfg(test)]
mod keybinding_layout_tests {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    #[test]
    fn keybinding_save_and_error_remain_visible_at_minimum_scaled_size() {
        let dir=tempfile::tempdir().unwrap();
        let accounts=kiln_accounts::AccountManager::with_env(kiln_accounts::Env::sandbox(dir.path(),false).0);
        let mut initialized=false;
        let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).build_ui_state(|ui,state:&mut (SettingsUi,Settings,super::super::keymap::Keymap)|{
            if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
            state.0.ui(ui.ctx(),&mut state.1,&AboutInfo{daemon_pid:0,daemon_build:String::new(),connected:false},&accounts,&mut state.2);
        },(SettingsUi{open:true,section:4},Settings::default(),super::super::keymap::Keymap::default()));
        h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("단축키 저장").rect()));
        let save=h.get_by_label("단축키 저장").rect();
        let fourth=h.get_by_label("새 워크스페이스").rect();
        assert!(fourth.bottom()<save.top(),"four bindings must fit above the fixed footer");
        h.get_by_label("관리").click();h.run_steps(2);
        assert!(h.query_by_label("기본값 복원").is_some());
        h.key_press(egui::Key::Escape);h.run_steps(2);
        h.render().unwrap().save("/tmp/kiln-keybindings-fixed-small.png").unwrap();
        h.state_mut().2.error=Some("중복된 단축키입니다. 다른 키를 입력하세요.".into());
        h.run_steps(3);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("단축키 저장").rect()));
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("중복된 단축키입니다. 다른 키를 입력하세요.").rect()));
        h.event(egui::Event::PointerMoved(egui::pos2(250.0,150.0)));
        h.event(egui::Event::MouseWheel {unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.0,-900.0),phase:egui::TouchPhase::Move,modifiers:egui::Modifiers::NONE});
        h.run_steps(5);
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("단축키 저장").rect()));
        assert!(h.ctx.content_rect().contains_rect(h.get_by_label("중복된 단축키입니다. 다른 키를 입력하세요.").rect()));
        h.render().unwrap().save("/tmp/kiln-keybindings-error-small.png").unwrap();
        let corrupt=dir.path().join("keybindings.json");
        std::fs::write(&corrupt,b"broken").unwrap();
        h.state_mut().2=super::super::keymap::Keymap::load_path(&corrupt);
        h.run_steps(3);
        for label in ["원본 백업 후 기본값 복구","다시 읽기"] {
            assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{label}");
        }
        h.render().unwrap().save("/tmp/kiln-keybindings-recovery-small.png").unwrap();
    }
}

#[cfg(test)]
mod typography_tests {
    use super::*;
    #[test]
    fn largest_terminal_preview_keeps_its_final_line_inside_the_scroll_view() {
        let ctx=egui::Context::default();fonts::install(&ctx);Theme::current().apply(&ctx);
        let mut settings=Settings{font_size:24.0,line_height:1.4,..Default::default()};
        let input=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,vec2(720.0/1.3,440.0/1.3))),..Default::default()};
        let mut output=ctx.run_ui(input,|ui| {
            egui::ScrollArea::vertical().show(ui,|ui|terminal(ui,&mut settings));
        });
        let mut checked=false;
        for clipped in &output.shapes {
            if let egui::Shape::Text(text)=&clipped.shape {
                if text.galley.job.text=="cargo test " {
                    assert!(clipped.clip_rect.contains_rect(text.galley.rect.translate(text.pos.to_vec2())),"last preview line must not be cut off by its own preview or parent scroll viewport");
                    checked=true;
                }
            }
        }
        output.textures_delta.clear();
        assert!(checked,"last preview line must be painted");
    }
}

#[cfg(test)]
mod language_layout_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn language_sidebar_labels_fit_their_navigation_rows() {
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for size in [[1100.0, 760.0], [720.0, 440.0]] {
                    let dir = tempfile::tempdir().unwrap();
                    let accounts = kiln_accounts::AccountManager::with_env(kiln_accounts::Env::sandbox(dir.path(), false).0);
                    let mut initialized = false;
                    let mut h = Harness::builder().with_size(size).build_ui_state(|ui, state: &mut (SettingsUi, Settings, super::super::keymap::Keymap)| {
                        if !initialized {
                            ui.ctx().set_fonts(fonts::definitions_for_language(false, language));
                            Theme::current().apply(ui.ctx()); initialized = true; return;
                        }
                        state.0.ui(ui.ctx(), &mut state.1, &AboutInfo { daemon_pid: 0, daemon_build: String::new(), connected: false }, &accounts, &mut state.2);
                    }, (SettingsUi { open: true, section: 0 }, Settings::default(), super::super::keymap::Keymap::default()));
                    h.run_steps(3);
                    for (source, _) in SECTIONS {
                        let label = kiln_common::i18n::tr(source);
                        let row = h.get_all_by_label(label).map(|node| node.rect()).min_by(|a, b| a.left().total_cmp(&b.left())).unwrap();
                        let painted = h.output().shapes.iter().filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label && text.pos.x < row.right() => Some(text.galley.rect.translate(text.pos.to_vec2())),
                            _ => None,
                        }).collect::<Vec<_>>();
                        assert!(!painted.is_empty(), "missing {label} in {language:?}");
                        for bounds in painted {
                            assert!(row.contains_rect(bounds), "{language:?} / {label} overflows navigation row: {bounds:?} outside {row:?}");
                        }
                    }
                }
            });
        }
    }
}
