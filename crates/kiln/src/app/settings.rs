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
    ("정보", Icon::Bell),
];

impl SettingsUi {
    pub fn ui(&mut self, ctx: &egui::Context, s: &mut Settings, about: &AboutInfo, accounts: &kiln_accounts::AccountManager) -> Vec<Action> {
        let t = Theme::current();
        let mut acts = Vec::new();
        let screen = ctx.content_rect();
        let size = vec2(860.0f32.min(screen.width() - 60.0), 580.0f32.min(screen.height() - 80.0));
        let mut close = false;
        let frame = Frame::new().fill(t.bg_panel).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(CornerRadius::same(16)).shadow(t.shadow()).inner_margin(Margin::same(0));
        let modal = egui::Modal::new(egui::Id::new("settings")).frame(frame).backdrop_color(Color32::from_black_alpha(if t.dark { 130 } else { 60 })).show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // 왼쪽 분류.
                let (nav, _) = ui.allocate_exact_size(vec2(200.0, size.y), Sense::hover());
                ui.painter().rect_filled(nav, CornerRadius { nw: 16, sw: 16, ne: 0, se: 0 }, widgets::lerp_color(t.bg_panel, t.bg_elevated, 0.35));
                ui.painter().line_segment([nav.right_top(), nav.right_bottom()], Stroke::new(1.0, t.border));
                ui.painter().text(pos2(nav.left() + 22.0, nav.top() + 30.0), Align2::LEFT_CENTER, "설정", fonts::semibold(17.0), t.text);
                for (i, (label, icon)) in SECTIONS.iter().enumerate() {
                    let r = egui::Rect::from_min_size(pos2(nav.left() + 12.0, nav.top() + 60.0 + i as f32 * 36.0), vec2(nav.width() - 24.0, 32.0));
                    let resp = ui.interact(r, ui.id().with(("nav", i)), Sense::click());
                    let sel = self.section == i;
                    if sel {
                        ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_selected);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(8), t.bg_hover);
                    }
                    icons::paint(ui.painter(), egui::Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(15.0, 15.0)), *icon, if sel { t.accent } else { t.text_dim });
                    ui.painter().text(pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, *label, fonts::medium(13.5), if sel { t.text } else { t.text_dim });
                    if resp.clicked() {
                        self.section = i;
                    }
                }
                ui.painter().text(pos2(nav.left() + 22.0, nav.bottom() - 22.0), Align2::LEFT_CENTER, format!("Kiln {}", env!("CARGO_PKG_VERSION")), fonts::regular(11.5), t.text_faint);

                // 오른쪽 내용.
                let content = egui::Rect::from_min_max(pos2(nav.right(), nav.top()), pos2(nav.right() + size.x - 200.0, nav.bottom()));
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content.shrink2(vec2(28.0, 22.0))).layout(egui::Layout::top_down(egui::Align::Min)));
                cui.horizontal(|ui| {
                    ui.label(RichText::new(SECTIONS[self.section].0).font(fonts::semibold(20.0)).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, 30.0, false, "닫기 (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
                cui.add_space(14.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut cui, |ui| {
                    ui.set_width(content.width() - 60.0);
                    match self.section {
                        0 => appearance(ui, s, &mut acts),
                        1 => terminal(ui, s),
                        2 => behavior(ui, s),
                        3 => accounts_page(ui, accounts, &mut acts, &mut close),
                        4 => shortcuts(ui),
                        _ => about_page(ui, about, &mut acts),
                    }
                });
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
                acts.push(Action::Toast(format!("{} 계정을 {name}(으)로 바꿨습니다. 새로 시작하는 세션부터 적용됩니다", tool.display_name())));
            }
            AccountsEvent::Saved { tool, .. } => acts.push(Action::Toast(format!("현재 {} 로그인을 계정 목록에 저장했습니다", tool.display_name()))),
        }
    }
}

fn appearance(ui: &mut egui::Ui, s: &mut Settings, acts: &mut Vec<Action>) {
    let t = Theme::current();
    ui.label(RichText::new("테마").font(fonts::semibold(12.0)).color(t.text_faint));
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for th in Theme::ALL {
            let (r, resp) = ui.allocate_exact_size(vec2(138.0, 104.0), Sense::click());
            let sel = s.theme == th.name;
            // 미니 미리보기: 레일 + 카드 두 장.
            let preview = egui::Rect::from_min_size(r.min, vec2(r.width(), 76.0));
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
                    let y = c.top() + 12.0 + li as f32 * 9.0;
                    ui.painter().line_segment([pos2(c.left() + 6.0, y), pos2(c.left() + 6.0 + (c.width() - 14.0) * (0.9 - li as f32 * 0.22), y)], Stroke::new(2.5, *col));
                }
            }
            let stroke = if sel { Stroke::new(2.0, t.accent) } else if resp.hovered() { Stroke::new(1.0, t.border_strong) } else { Stroke::new(1.0, t.border) };
            ui.painter().rect_stroke(preview, CornerRadius::same(10), stroke, StrokeKind::Outside);
            ui.painter().text(pos2(r.left() + 2.0, preview.bottom() + 15.0), Align2::LEFT_CENTER, th.label, fonts::medium(13.0), if sel { t.text } else { t.text_dim });
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() && !sel {
                acts.push(Action::SetTheme(th.name.into()));
            }
        }
    });
    ui.add_space(18.0);
    widgets::group(ui, "화면", |ui| {
        widgets::setting_row(ui, "UI 크기", "창 전체의 글자와 여백 비율", |ui| {
            let mut v = (s.ui_scale * 100.0).round() as i32;
            let before = v;
            let mut opts = [(90, "작게"), (100, "보통"), (115, "크게"), (130, "아주 크게")].to_vec();
            if !opts.iter().any(|(x, _)| *x == v) {
                opts.push((v, "사용자"));
            }
            widgets::segmented(ui, &mut v, &opts);
            if v != before {
                s.ui_scale = v as f32 / 100.0;
                ui.ctx().set_zoom_factor(s.ui_scale);
            }
        });
        widgets::divider(ui);
        widgets::setting_row(ui, "카드 간격", "터미널·에디터 카드 사이 여백", |ui| {
            ui.add(egui::Slider::new(&mut s.card_gap, 2.0..=16.0).step_by(1.0).suffix(" px").show_value(true));
        });
    });
}

fn terminal(ui: &mut egui::Ui, s: &mut Settings) {
    let t = Theme::current();
    // 미리보기.
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 118.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(12), t.bg);
    ui.painter().rect_stroke(r, CornerRadius::same(12), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let fid = fonts::mono(s.font_size);
    let lh = ui.fonts_mut(|f| f.row_height(&fid)) * s.line_height;
    let lines: [&[(&str, Color32)]; 4] = [
        &[("~/dev/kiln ", t.blue), ("main ", t.purple), ("❯ ", t.green), ("claude", t.text)],
        &[("● ", t.orange), ("작업을 계획하는 중… ", t.text), ("(esc 로 중단)", t.text_faint)],
        &[("fn ", t.purple), ("main", t.blue), ("() { println!(", t.text), ("\"안녕, Kiln\"", t.green), ("); }", t.text)],
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
    widgets::group(ui, "글꼴", |ui| {
        widgets::setting_row(ui, "글꼴 크기", "⌘= / ⌘- 로도 바꿀 수 있습니다", |ui| {
            ui.add(egui::Slider::new(&mut s.font_size, 9.0..=24.0).step_by(0.5).suffix(" pt"));
        });
        widgets::divider(ui);
        widgets::setting_row(ui, "줄 간격", "", |ui| {
            let mut v = (s.line_height * 100.0).round() as i32;
            let before = v;
            widgets::segmented(ui, &mut v, &[(115, "좁게"), (125, "보통"), (140, "넓게")]);
            if v != before {
                s.line_height = v as f32 / 100.0;
            }
        });
    });
    widgets::group(ui, "입력", |ui| {
        widgets::setting_row(ui, "Option 키를 Meta 로", "Option+B 같은 조합을 셸 단축키로 보냅니다", |ui| {
            widgets::toggle(ui, &mut s.option_as_meta);
        });
    });
    widgets::group(ui, "셸", |ui| {
        widgets::setting_row(ui, "기본 셸", "비워 두면 로그인 셸($SHELL)을 씁니다", |ui| {
            ui.add(egui::TextEdit::singleline(&mut s.shell).hint_text("/bin/zsh").desired_width(200.0));
        });
    });
}

fn behavior(ui: &mut egui::Ui, s: &mut Settings) {
    widgets::group(ui, "안전", |ui| {
        widgets::setting_row(ui, "실행 중인 카드를 닫을 때 확인", "claude, codex 같은 프로세스가 돌고 있으면 한 번 더 묻습니다", |ui| {
            widgets::toggle(ui, &mut s.confirm_close_running);
        });
    });
    widgets::group(ui, "알림", |ui| {
        widgets::setting_row(ui, "시스템 알림", "창이 비활성일 때 에이전트 알림을 OS 알림으로도 보냅니다", |ui| {
            widgets::toggle(ui, &mut s.os_notifications);
        });
    });
}

fn shortcuts(ui: &mut egui::Ui) {
    let rows: [(&str, &[(&str, &str)]); 4] = [
        ("카드와 페이지", &[("새 페이지", "⌘T"), ("오른쪽으로 나누기", "⌘D"), ("아래로 나누기", "⇧⌘D"), ("카드 닫기", "⌘W"), ("카드 크게 보기", "⇧⌘↩"), ("카드 이동", "⌥⌘←"), ("페이지 전환", "⌥⌘1")]),
        ("탐색", &[("명령 팔레트", "⌘K"), ("파일 빠르게 열기", "⌘P"), ("찾기", "⌘F"), ("스페이스 전환", "⌘1"), ("최근 알림", "⇧⌘U")]),
        ("도구 시트", &[("파일", "⇧⌘E"), ("검색", "⇧⌘F"), ("Git", "⇧⌘G"), ("GitHub", "⇧⌘R"), ("데이터베이스", "⇧⌘B"), ("문제", "⇧⌘M"), ("Git 로그", "⇧⌘L")]),
        ("화면", &[("스페이스 레일", "⌘B"), ("글꼴 크게/작게", "⌘="), ("설정", "⌘,")]),
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
    widgets::group(ui, "세션 데몬", |ui| {
        widgets::setting_row(ui, "상태", if about.connected { "터미널 세션은 앱과 분리된 데몬에서 돕니다. 앱을 꺼도 유지됩니다." } else { "연결되지 않음" }, |ui| {
            let (c, label) = if about.connected { (t.green, format!("실행 중 · pid {}", about.daemon_pid)) } else { (t.red, "끊김".into()) };
            ui.label(RichText::new(label).color(t.text_dim));
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, c);
        });
        widgets::divider(ui);
        widgets::setting_row(ui, "데몬 빌드", &about.daemon_build, |ui| {
            if widgets::button(ui, "이 버전으로 교체", ButtonKind::Secondary).on_hover_text("실행 중인 세션을 유지한 채 데몬만 바꿉니다").clicked() {
                acts.push(Action::UpgradeDaemon);
            }
        });
    });
    widgets::group(ui, "앱", |ui| {
        widgets::setting_row(ui, "버전", &format!("Kiln {} · build {}", env!("CARGO_PKG_VERSION"), kiln_daemon::build_id()), |_| {});
        widgets::divider(ui);
        widgets::setting_row(ui, "설정 폴더", &kiln_common::paths::config_dir().display().to_string(), |ui| {
            if widgets::button(ui, "열기", ButtonKind::Secondary).clicked() {
                let _ = open::that_detached(kiln_common::paths::config_dir());
            }
        });
    });
}
