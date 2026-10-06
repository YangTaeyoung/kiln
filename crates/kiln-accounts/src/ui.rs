//! 설정 화면의 계정 섹션: 도구별 카드, 계정 목록, 사용량 막대, 저장·추가·자동 전환.

use crate::{AccountManager, Profile, Tool, Usage, now_unix};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use kiln_common::Theme;
use kiln_common::fonts;
use kiln_common::icons::{self, Icon};
use kiln_common::widgets::{self, ButtonKind};

/// 계정 섹션이 앱에 알리는 일.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountsEvent {
    /// Legacy integration event; browser sign-in is managed in the account form.
    RunLogin(Tool),
    /// 현재 자격증명이 이 계정으로 바뀌었다.
    Switched { tool: Tool, id: String },
    /// 현재 로그인 계정을 이 프로필로 저장했다.
    Saved { tool: Tool, id: String },
}

#[derive(Default)]
pub(crate) struct UiState {
    renaming: Option<(Tool, String, String)>,
    confirm_delete: Option<(Tool, String)>,
    adding: Option<(Tool, String)>,
    adding_focus: bool,
    login_code: [String; 2],
}

/// 도구별 카드를 그리고, 이번 프레임까지 쌓인 이벤트를 돌려준다.
pub fn accounts_settings_ui(ui: &mut Ui, mgr: &AccountManager) -> Vec<AccountsEvent> {
    mgr.set_repaint_context(ui.ctx());
    for tool in Tool::ALL {
        ui.push_id(("kiln-accounts", tool.key()), |ui| tool_card(ui, mgr, tool));
    }
    mgr.drain_events()
}

fn card(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    let t = Theme::current();
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin { left: 16, right: 14, top: 14, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            body(ui);
        });
    ui.add_space(14.0);
}

/// The same embedded brand mark used by task navigation and terminal headers.
fn brand_mark(ui: &mut Ui, tool: Tool) {
    let t = Theme::current();
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
    let (icon, color) = match tool {
        Tool::Claude => (Icon::Claude, t.orange),
        Tool::Codex => (Icon::Codex, t.text),
    };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(24.0, 24.0)), icon, color);
}

fn subtitle(tool: Tool) -> &'static str {
    match tool {
        Tool::Claude => kiln_common::i18n::tr("Anthropic 구독 계정"),
        Tool::Codex => kiln_common::i18n::tr("ChatGPT 구독 계정"),
    }
}

fn tool_card(ui: &mut Ui, mgr: &AccountManager, tool: Tool) {
    let t = Theme::current();
    let profiles = mgr.profiles(tool);
    let active = mgr.active(tool);
    let refreshing = mgr.is_refreshing(tool);
    let (busy, error, notice, live) = mgr.with_state(|s| {
        let i = if tool == Tool::Claude { 0 } else { 1 };
        (s.busy[i].clone(), s.error[i].clone(), s.notice[i].clone(), s.live_email[i].clone())
    });

    card(ui, |ui| {
        // 머리: 마크, 이름, 부제, 새로 고침.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            brand_mark(ui, tool);
            let title_width = (ui.available_width() - 26.0 - ui.spacing().item_spacing.x).max(0.0);
            ui.allocate_ui_with_layout(vec2(title_width, 32.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(title_width);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(egui::Label::new(RichText::new(tool.display_name()).font(fonts::semibold(15.0)).color(t.text)).truncate());
                let count = if profiles.is_empty() { String::new() } else { kiln_common::trf!(" · {}개", profiles.len()) };
                ui.add(egui::Label::new(RichText::new(format!("{}{count}", subtitle(tool))).font(fonts::regular(12.0)).color(t.text_faint)).truncate());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if refreshing {
                    ui.add(egui::Spinner::new().size(14.0).color(t.text_dim));
                } else {
                    let tip = match tool {
                        Tool::Claude => kiln_common::i18n::tr("저장한 계정의 사용량 확인"),
                        Tool::Codex => kiln_common::i18n::tr("사용량 새로 고침 (최근 Codex 세션 기록)"),
                    };
                    if widgets::icon_button(ui, Icon::Refresh, 26.0, false, tip).clicked() {
                        mgr.refresh_usage(tool);
                    }
                }
            });
        });
        ui.add_space(8.0);

        // 저장되지 않은 현재 로그인 안내.
        if let Some(Some(email)) = &live
            && !profiles.iter().any(|p| p.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(email)))
        {
            info_banner(ui, &kiln_common::trf!("현재 로그인 계정: {email} · 아직 Kiln에 저장하지 않았습니다"));
        }

        if profiles.is_empty() {
            empty_rows(ui, tool);
        } else {
            let now = now_unix();
            for (i, p) in profiles.iter().enumerate() {
                if i > 0 {
                    widgets::divider(ui);
                }
                account_row(ui, mgr, p, active.as_deref() == Some(p.id.as_str()), mgr.usage(tool, &p.id), now, busy.is_some());
            }
        }
        ui.add_space(6.0);

        // 동작 버튼과 상태 줄.
        let adding = mgr.ui_state().adding.as_ref().is_some_and(|(current, _)| *current == tool);
        if !adding && mgr.login_status(tool).is_none() { ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(busy.is_none(), |ui| {
                if widgets::button_with(ui, Some(Icon::Save), kiln_common::i18n::tr("현재 로그인한 계정 저장"), ButtonKind::Secondary, true).clicked() {
                    mgr.run_async(tool, kiln_common::i18n::tr("저장 중…"), move |m| {
                        let p = m.save_current(tool, "")?;
                        Ok(kiln_common::trf!("‘{}’ 계정을 저장했습니다", p.label))
                    });
                }
                if widgets::button_with(ui, Some(Icon::Plus), kiln_common::i18n::tr("새 계정 추가"), ButtonKind::Primary, true).clicked() {
                    mgr.ui_state().adding = Some((tool, String::new()));
                    mgr.ui_state().adding_focus = true;
                }
            });
        }); }
        login_form(ui, mgr, tool);
        if let Some(b) = &busy {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(12.0).color(t.text_dim));
                ui.label(RichText::new(b).font(fonts::regular(12.0)).color(t.text_dim));
            });
        }
        if let Some(e) = &error {
            ui.add(egui::Label::new(RichText::new(e).font(fonts::regular(12.0)).color(t.red)).wrap());
        } else if busy.is_none() && let Some(n) = &notice {
            ui.add(egui::Label::new(RichText::new(n).font(fonts::regular(12.0)).color(t.green)).wrap());
        }
        ui.add_space(6.0);
        widgets::divider(ui);

        let mut on = mgr.auto_rotate(tool);
        widgets::setting_row(ui, kiln_common::i18n::tr("자동 전환"), kiln_common::i18n::tr("사용량이 소진되면 다음 계정으로 전환하고 세션을 이어갑니다"), |ui| {
            if widgets::toggle(ui, &mut on).changed() {
                mgr.set_auto_rotate(tool, on);
            }
        });
    });
}

fn info_banner(ui: &mut Ui, text: &str) {
    let t = Theme::current();
    egui::Frame::new()
        .fill(widgets::tint(t.blue, if t.dark { 0.10 } else { 0.08 }))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Info, t.blue);
                ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(12.5)).color(t.text_dim)).wrap());
            });
        });
    ui.add_space(4.0);
}

fn empty_rows(ui: &mut Ui, _tool: Tool) {
    let t = Theme::current();
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(10), t.bg_hover);
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(18.0, 18.0)), Icon::Person, t.text_faint);
        ui.add_space(6.0);
        ui.label(RichText::new(kiln_common::i18n::tr("저장된 계정이 없습니다")).font(fonts::medium(13.0)).color(t.text_dim));
        let hint = kiln_common::i18n::tr("새 계정을 추가하면 브라우저에서 로그인할 수 있습니다");
        ui.label(RichText::new(hint).font(fonts::regular(12.0)).color(t.text_faint));
        ui.add_space(10.0);
    });
}

fn login_form(ui: &mut Ui, mgr: &AccountManager, tool: Tool) {
    let t = Theme::current();
    let index = if tool == Tool::Claude { 0 } else { 1 };
    let status = mgr.login_status(tool);
    if let Some(status) = status {
        let cancelling = mgr.login_is_cancelling(tool);
        ui.add_enabled_ui(!cancelling, |ui| { ui.horizontal_wrapped(|ui| {
            if let Some(url) = status.browser_url {
                if widgets::button(ui, kiln_common::i18n::tr("로그인 페이지 다시 열기"), ButtonKind::Secondary).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
            if widgets::button(ui, kiln_common::i18n::tr("로그인 취소"), ButtonKind::Secondary).clicked() { mgr.cancel_login(tool); }
        }); });
        if status.accepts_code && !cancelling {
            ui.add(egui::Label::new(RichText::new(kiln_common::i18n::tr("브라우저에서 로그인 코드를 제공한 경우 여기에 붙여넣으세요")).font(fonts::regular(12.0)).color(t.text_dim)).wrap());
            let mut code = mgr.ui_state().login_code[index].clone();
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut code).password(true).hint_text(kiln_common::i18n::tr("로그인 코드")).desired_width((ui.available_width() - 82.0).clamp(80.0, 220.0)));
                field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), kiln_common::i18n::tr("로그인 코드")));
                if ui.add_enabled(!code.trim().is_empty(), egui::Button::new(kiln_common::i18n::tr("확인"))).clicked() {
                    if let Err(error) = mgr.submit_login_code(tool, &code) {
                        mgr.with_state(|s| s.error[index] = Some(error.to_string()));
                    } else { code.clear(); }
                }
            });
            mgr.ui_state().login_code[index] = code;
        }
        return;
    }
    // Clear ephemeral authorization codes once their job has ended.
    mgr.ui_state().login_code[index].clear();
    let adding = mgr.ui_state().adding.clone().filter(|(current, _)| *current == tool);
    if let Some((_, mut name)) = adding {
        ui.add_space(6.0);
        ui.label(RichText::new(kiln_common::i18n::tr("계정 이름")).font(fonts::medium(12.0)).color(t.text_dim));
        let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text(kiln_common::i18n::tr("예: 개인 계정, 회사 계정")).desired_width(ui.available_width()));
        field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), kiln_common::i18n::tr("계정 이름")));
        if mgr.ui_state().adding_focus {
            field.request_focus();
            mgr.ui_state().adding_focus = false;
        }
        let submit = ui.horizontal_wrapped(|ui| {
            let start = ui.add_enabled_ui(!name.trim().is_empty(), |ui| {
                widgets::button(ui, kiln_common::i18n::tr("브라우저에서 로그인"), ButtonKind::Primary).clicked()
            }).inner;
            let cancel = widgets::button(ui, kiln_common::i18n::tr("취소"), ButtonKind::Secondary).clicked();
            (start, cancel)
        }).inner;
        if submit.0 || field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
            mgr.ui_state().adding = None;
            mgr.start_login(tool, &name);
        } else if submit.1 {
            mgr.ui_state().adding = None;
        } else { mgr.ui_state().adding = Some((tool, name)); }
    }
}

/// 남은 시간을 "2시간 13분 후" 같은 한국어로 쓴다.
pub(crate) fn fmt_reset(reset: i64, now: i64) -> String {
    let d = reset - now;
    if d <= 0 {
        return kiln_common::i18n::tr("갱신 시각 지남").into();
    }
    let (days, hours, mins) = (d / 86400, (d % 86400) / 3600, (d % 3600) / 60);
    if days > 0 {
        if hours > 0 { kiln_common::trf!("{days}일 {hours}시간 후") } else { kiln_common::trf!("{days}일 후") }
    } else if hours > 0 {
        if mins > 0 { kiln_common::trf!("{hours}시간 {mins}분 후") } else { kiln_common::trf!("{hours}시간 후") }
    } else {
        kiln_common::trf!("{}분 후", mins.max(1))
    }
}

fn usage_color(u: f32) -> Color32 {
    let t = Theme::current();
    if u >= 0.9 {
        t.red
    } else if u >= 0.7 {
        t.yellow
    } else {
        t.green
    }
}

/// "5시간 ▓▓░ 43% 2시간 후" 한 줄.
fn usage_line(ui: &mut Ui, label: &str, w: Option<(f32, Option<i64>)>, now: i64) {
    let t = Theme::current();
    let w = Usage::effective(w, now);
    let (rect, _) = ui.allocate_exact_size(vec2(236.0, 16.0), Sense::hover());
    let p = ui.painter();
    p.text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, label, fonts::medium(11.0), t.text_faint);
    let bar = Rect::from_min_size(pos2(rect.left() + 34.0, rect.center().y - 2.5), vec2(96.0, 5.0));
    p.rect_filled(bar, CornerRadius::same(3), t.bg_hover);
    match w {
        Some((u, reset)) => {
            let f = u.clamp(0.0, 1.0);
            if f > 0.0 {
                let fill = Rect::from_min_size(bar.min, vec2((bar.width() * f).max(3.0), bar.height()));
                p.rect_filled(fill, CornerRadius::same(3), usage_color(u));
            }
            p.text(pos2(bar.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, format!("{:.0}%", u * 100.0), fonts::medium(11.0), t.text_dim);
            if let Some(r) = reset {
                p.text(pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, fmt_reset(r, now), fonts::regular(11.0), t.text_faint);
            }
        }
        None => {
            p.text(pos2(bar.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, "—", fonts::medium(11.0), t.text_faint);
        }
    }
}

/// 사용량 열(폭 236, 높이 36 고정).
fn usage_block(ui: &mut Ui, usage: Option<&Usage>, now: i64) {
    let t = Theme::current();
    ui.allocate_ui_with_layout(vec2(236.0, 36.0), Layout::top_down(Align::Min), |ui| {
        ui.set_min_size(vec2(236.0, 36.0));
        ui.spacing_mut().item_spacing.y = 2.0;
        let note = |ui: &mut Ui, icon: Icon, text: &str, color: Color32| {
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (r, resp) = ui.allocate_exact_size(vec2(13.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, color);
                let l = ui.label(RichText::new(text).font(fonts::regular(11.5)).color(color));
                resp | l
            })
            .inner
        };
        match usage {
            Some(u) if u.five_hour.is_some() || u.seven_day.is_some() => {
                usage_line(ui, kiln_common::i18n::tr("5시간"), u.five_hour, now);
                usage_line(ui, kiln_common::i18n::tr("7일"), u.seven_day, now);
                if u.is_unavailable() {
                    ui.label(RichText::new(kiln_common::i18n::tr("갱신 실패 · 마지막으로 확인한 사용량")).font(fonts::regular(10.5)).color(t.text_faint)).on_hover_text(&u.status);
                }
            }
            Some(u) if u.is_unavailable() => {
                let reason = u.status.trim_start_matches("unavailable:").trim().to_string();
                note(ui, Icon::Warning, kiln_common::i18n::tr("사용량 확인 불가"), t.text_faint).on_hover_text(reason);
            }
            _ => {
                note(ui, Icon::Info, kiln_common::i18n::tr("사용량 정보 없음"), t.text_faint);
            }
        }
    });
}

fn account_row(ui: &mut Ui, mgr: &AccountManager, p: &Profile, is_active: bool, usage: Option<Usage>, now: i64, busy: bool) {
    let t = Theme::current();
    let tool = p.tool;
    let exhausted = usage.as_ref().is_some_and(|u| u.exhausted_at(now));
    let renaming = mgr.ui_state().renaming.as_ref().filter(|(rt, id, _)| *rt == tool && *id == p.id).map(|r| r.2.clone());
    let confirming = mgr.ui_state().confirm_delete.as_ref().is_some_and(|(ct, id)| *ct == tool && *id == p.id);

    ui.horizontal(|ui| {
        ui.set_min_height(56.0);
        ui.spacing_mut().item_spacing.x = 10.0;
        let (ar, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        widgets::avatar(ui, ar, &p.label, is_active);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(2.0);
            if let Some(mut buf) = renaming.clone() {
                let id = ui.id().with(("rename", &p.id));
                let focused = ui.memory(|m| m.has_focus(id));
                let resp = widgets::input_frame(focused, false)
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .show(ui, |ui| ui.add(egui::TextEdit::singleline(&mut buf).id(id).frame(egui::Frame::NONE).font(fonts::medium(13.0)).desired_width(160.0)))
                    .inner;
                if !focused && !resp.lost_focus() {
                    resp.request_focus();
                }
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let mut st = mgr.ui_state();
                if esc {
                    st.renaming = None;
                } else if resp.lost_focus() || enter {
                    st.renaming = None;
                    drop(st);
                    if let Err(e) = mgr.rename(tool, &p.id, &buf) {
                        mgr.with_state(|s| s.error[if tool == Tool::Claude { 0 } else { 1 }] = Some(format!("{e:#}")));
                    }
                } else if let Some(r) = st.renaming.as_mut() {
                    r.2 = buf;
                }
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(&p.label).font(fonts::semibold(13.5)).color(t.text));
                    if is_active {
                        widgets::pill(ui, kiln_common::i18n::tr("사용 중"), t.green);
                    }
                    if exhausted {
                        widgets::pill(ui, kiln_common::i18n::tr("소진"), t.red);
                    }
                });
            }
            let email = p.email.as_deref().unwrap_or(kiln_common::i18n::tr("이메일 알 수 없음"));
            ui.label(RichText::new(email).font(fonts::regular(12.0)).color(t.text_faint));
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if confirming {
                if widgets::button_with(ui, None, kiln_common::i18n::tr("삭제"), ButtonKind::Danger, true).clicked() {
                    mgr.ui_state().confirm_delete = None;
                    let id = p.id.clone();
                    let label = p.label.clone();
                    mgr.run_async(tool, kiln_common::i18n::tr("삭제 중…"), move |m| {
                        m.remove(tool, &id)?;
                        Ok(kiln_common::trf!("‘{label}’ 계정을 삭제했습니다"))
                    });
                }
                if widgets::button_with(ui, None, kiln_common::i18n::tr("취소"), ButtonKind::Ghost, true).clicked() {
                    mgr.ui_state().confirm_delete = None;
                }
                ui.label(RichText::new(kiln_common::i18n::tr("삭제할까요?")).font(fonts::medium(12.5)).color(t.text_dim));
                return;
            }
            let more = widgets::icon_button(ui, Icon::Menu, 26.0, false, kiln_common::i18n::tr("계정 메뉴"));
            egui::Popup::menu(&more).gap(4.0).show(|ui| {
                ui.set_min_width(150.0);
                if menu_item(ui, Icon::Pencil, kiln_common::i18n::tr("이름 변경"), t.text) {
                    mgr.ui_state().renaming = Some((tool, p.id.clone(), p.label.clone()));
                    ui.close();
                }
                let can_delete = !is_active;
                let resp = ui.add_enabled_ui(can_delete, |ui| menu_item(ui, Icon::Trash, kiln_common::i18n::tr("삭제"), t.red)).inner;
                if resp {
                    mgr.ui_state().confirm_delete = Some((tool, p.id.clone()));
                    ui.close();
                }
            });
            // 전환 버튼 칸은 폭을 고정해 사용량 열이 행마다 같은 자리에 오게 한다.
            ui.allocate_ui_with_layout(vec2(58.0, 30.0), Layout::right_to_left(Align::Center), |ui| {
                ui.set_min_size(vec2(58.0, 30.0));
                if !is_active || mgr.pending_login(tool, &p.id) {
                    ui.add_enabled_ui(!busy, |ui| {
                        let label = if mgr.pending_login(tool, &p.id) { kiln_common::i18n::tr("적용") } else { kiln_common::i18n::tr("전환") };
                        if widgets::button_with(ui, None, label, ButtonKind::Secondary, true).clicked() {
                            let id = p.id.clone();
                            let name = p.label.clone();
                            mgr.run_async(tool, kiln_common::i18n::tr("전환 중…"), move |m| {
                                m.switch_to(tool, &id)?;
                                Ok(kiln_common::trf!("‘{name}’ 계정으로 전환했습니다. 새로 여는 세션부터 적용됩니다"))
                            });
                        }
                    });
                }
            });
            ui.add_space(8.0);
            usage_block(ui, usage.as_ref(), now);
        });
    });
}

/// 아이콘 + 글자 메뉴 항목. 눌리면 true.
fn menu_item(ui: &mut Ui, icon: Icon, label: &str, color: Color32) -> bool {
    let t = Theme::current();
    let enabled = ui.is_enabled();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(150.0), 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if resp.hovered() && enabled {
        ui.painter().rect_filled(r, CornerRadius::same(6), t.bg_hover);
    }
    let fg = if enabled { color } else { t.text_faint };
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(14.0, 14.0)), icon, fg);
    ui.painter().text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, label, fonts::medium(13.0), fg);
    resp.clicked() && enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fonts(ui: &mut Ui) -> bool {
        let id = egui::Id::new("accounts-login-test-fonts");
        if ui.ctx().data(|data| data.get_temp::<bool>(id)).unwrap_or(false) { return true; }
        ui.ctx().data_mut(|data| data.insert_temp(id, true));
        ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
        false
    }

    #[test]
    fn browser_login_waits_in_settings_reopens_url_cancels_and_blocks_duplicate_job() {
        use egui_kittest::kittest::Queryable;
        let root = tempfile::tempdir().unwrap();
        let (env, _) = crate::Env::sandbox(root.path(), false);
        let manager = AccountManager::with_env(env);
        let url = "https://auth.openai.com/oauth/authorize?client_id=fixture&redirect_uri=fixture&state=fixture";
        let pending = manager.login_fixture(Tool::Codex, crate::LoginStatus { browser_url: Some(url.into()), accepts_code: false });
        let mut harness = egui_kittest::Harness::builder().with_size(vec2(720.0, 950.0)).build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { accounts_settings_ui(ui, manager); } }, manager.clone());
        for _ in 0..4 { harness.step(); }
        assert_eq!(harness.query_all_by_label("로그인 페이지 다시 열기").count(), 1);
        assert_eq!(harness.query_all_by_label("로그인 취소").count(), 1);
        harness.get_by_label("로그인 페이지 다시 열기").click();
        let mut opened = false;
        for _ in 0..4 {
            harness.step();
            opened |= harness.output().platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::OpenUrl(open) if open.url == url));
        }
        assert!(opened, "the browser can be reopened without a terminal");
        // It resolves the executable but must not launch another process while
        // this tool already has a login job. No real authentication is started.
        manager.start_login(Tool::Codex, "Duplicate");
        assert_eq!(manager.login_status(Tool::Codex).unwrap().browser_url.as_deref(), Some(url));
        assert!(manager.profiles(Tool::Codex).is_empty());
        harness.get_by_label("로그인 취소").click();
        for _ in 0..4 { harness.step(); }
        assert!(pending.cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!manager.drain_events().iter().any(|event| matches!(event, AccountsEvent::RunLogin(_))));
    }

    #[test]
    fn claude_login_exposes_code_fallback_only_when_cli_requests_it() {
        use egui_kittest::kittest::Queryable;
        let root = tempfile::tempdir().unwrap();
        let (env, _) = crate::Env::sandbox(root.path(), false);
        let manager = AccountManager::with_env(env);
        manager.login_fixture(Tool::Claude, crate::LoginStatus { browser_url: None, accepts_code: true });
        let mut harness = egui_kittest::Harness::builder().with_size(vec2(720.0, 950.0)).build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { accounts_settings_ui(ui, manager); } }, manager);
        for _ in 0..4 { harness.step(); }
        assert_eq!(harness.query_all_by_label("로그인 코드").count(), 1);
        assert_eq!(harness.query_all_by_label("로그인 페이지 다시 열기").count(), 0);
    }

    #[test]
    fn browser_login_forms_fit_four_languages_and_narrow_scaled_settings() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for width in [420.0, 720.0] {
                    for scale in [1.0, 1.3] {
                        for waiting in [false, true] {
                            let root = tempfile::tempdir().unwrap();
                            let (env, _) = crate::Env::sandbox(root.path(), false);
                            let manager = AccountManager::with_env(env);
                            if waiting {
                                manager.login_fixture(Tool::Claude, crate::LoginStatus {
                                    browser_url: Some("https://claude.ai/oauth/authorize?client_id=x&redirect_uri=x&state=x".into()),
                                    accepts_code: true,
                                });
                            } else { manager.ui_state().adding = Some((Tool::Claude, "Work".into())); }
                            let mut harness = egui_kittest::Harness::builder().with_size(vec2(width, 600.0))
                                .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { login_form(ui, manager, Tool::Claude); } }, manager);
                            harness.ctx.set_fonts(kiln_common::fonts::definitions(false));
                            harness.ctx.set_zoom_factor(scale);
                            for _ in 0..4 { harness.step(); }
                            let labels: &[&str] = if waiting { &["로그인 페이지 다시 열기", "로그인 취소", "로그인 코드", "확인"] } else { &["브라우저에서 로그인", "취소"] };
                            for label in labels {
                                let translated = kiln_common::i18n::tr(label);
                                let node = harness.get_by_label(translated);
                                assert!(harness.ctx.content_rect().contains_rect(node.rect()), "{language:?}: {label} overflow at {width}px / {scale}");
                            }
                        }
                    }
                }
            });
        }
    }

    #[test]
    fn account_headers_reserve_refresh_space_in_narrow_scaled_settings() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for width in [420.0, 720.0] {
                    for scale in [1.0, 1.3] {
                        for tool in Tool::ALL {
                            let root = tempfile::tempdir().unwrap();
                            let (env, _) = crate::Env::sandbox(root.path(), false);
                            let manager = AccountManager::with_env(env);
                            let mut harness = egui_kittest::Harness::builder().with_size(vec2(width, 600.0))
                                .build_ui_state(move |ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, tool); } }, manager);
                            harness.ctx.set_zoom_factor(scale);
                            for _ in 0..4 { harness.step(); }
                            let subtitle_rect = harness.get_by_label(subtitle(tool)).rect();
                            let refresh_label = match tool {
                                Tool::Claude => kiln_common::i18n::tr("저장한 계정의 사용량 확인"),
                                Tool::Codex => kiln_common::i18n::tr("사용량 새로 고침 (최근 Codex 세션 기록)"),
                            };
                            let refresh_rect = harness.get_by_label(refresh_label).rect();
                            assert!(harness.ctx.content_rect().contains_rect(refresh_rect), "{language:?}/{tool:?}: refresh overflow at {width}px / {scale}");
                            assert!(subtitle_rect.right() < refresh_rect.left(), "{language:?}/{tool:?}: subtitle overlaps refresh at {width}px / {scale}");
                        }
                    }
                }
            });
        }
    }

    #[test]
    fn account_headers_use_shared_agent_marks_at_every_pixel_density() {
        for density in [1.0_f32, 1.25, 1.5, 2.0, 3.0] {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().native_pixels_per_point = Some(density);
            let mut output = ctx.run_ui(input, |ui| {
                for tool in Tool::ALL { brand_mark(ui, tool); }
            });
            for tool in Tool::ALL {
                let name = match tool { Tool::Claude => "claude", Tool::Codex => "codex" };
                let pixels = (24.0 * ctx.pixels_per_point()).ceil() as u32;
                let texture = ctx.data(|data| data.get_temp::<egui::TextureHandle>(egui::Id::new(("agent-mark", name, pixels))))
                    .expect("account header must use the shared embedded SVG, not a hand-drawn substitute");
                let meshes: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh.calc_bounds()),
                    _ => None,
                }).collect();
                assert_eq!(meshes.len(), 1, "missing account logo for {name}");
                assert_eq!(meshes[0].size(), vec2(24.0, 24.0));
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn reset_formatting() {
        assert_eq!(fmt_reset(100, 200), "갱신 시각 지남");
        assert_eq!(fmt_reset(30, 0), "1분 후");
        assert_eq!(fmt_reset(45 * 60, 0), "45분 후");
        assert_eq!(fmt_reset(2 * 3600 + 13 * 60, 0), "2시간 13분 후");
        assert_eq!(fmt_reset(3 * 3600, 0), "3시간 후");
        assert_eq!(fmt_reset(2 * 86400 + 5 * 3600, 0), "2일 5시간 후");
    }

    #[test]
    fn browser_login_waiting_and_code_captures() {
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for theme in ["kiln-dark", "kiln-light"] {
                    kiln_common::Theme::set_current(theme);
                    for tool in Tool::ALL {
                        let root = tempfile::tempdir().unwrap();
                        let (env, _) = crate::Env::sandbox(root.path(), false);
                        let manager = AccountManager::with_env(env);
                        manager.login_fixture(tool, crate::LoginStatus {
                            browser_url: Some(match tool {
                                Tool::Claude => "https://claude.ai/oauth/authorize?client_id=fixture&redirect_uri=fixture&state=fixture",
                                Tool::Codex => "https://auth.openai.com/oauth/authorize?client_id=fixture&redirect_uri=fixture&state=fixture",
                            }.into()),
                            accepts_code: tool == Tool::Claude,
                        });
                        let mut harness = egui_kittest::Harness::builder().with_size(vec2(420.0, 580.0)).wgpu()
                            .build_ui_state(move |ui, manager: &mut AccountManager| {
                                kiln_common::Theme::current().apply(ui.ctx());
                                if test_fonts(ui) { tool_card(ui, manager, tool); }
                            }, manager);
                        harness.ctx.set_zoom_factor(1.3);
                        for _ in 0..4 { harness.step(); }
                        let image = harness.render().expect("login waiting state must render");
                        image.save(format!("/tmp/kiln-account-login-{language:?}-{theme}-{tool:?}-420.png")).unwrap();
                    }
                }
            });
        }
        kiln_common::Theme::set_current("kiln-dark");
    }
}
