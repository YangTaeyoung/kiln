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
    relogin: Option<(Tool, String)>,
    /// The Codex profile whose reset-credit redemption awaits confirmation.
    confirm_reset: Option<String>,
    /// The Codex profile whose usage history dialog is open.
    history: Option<String>,
    login_code: [String; 2],
    pub(crate) feedback_profile: [Option<String>; 2],
}

/// 도구별 카드를 그리고, 이번 프레임까지 쌓인 이벤트를 돌려준다.
pub fn accounts_settings_ui(ui: &mut Ui, mgr: &AccountManager) -> Vec<AccountsEvent> {
    mgr.set_repaint_context(ui.ctx());
    mgr.refresh_login_status();
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
    if tool == Tool::Codex { usage_history_dialog(ui, mgr); }
    let t = Theme::current();
    let profiles = mgr.profiles(tool);
    let active = mgr.active(tool);
    let refreshing = mgr.is_refreshing(tool);
    let index = if tool == Tool::Claude { 0 } else { 1 };
    let login_target = mgr.login_profile(tool).map(|p| p.id).or_else(|| mgr.ui_state().relogin.clone().filter(|(current, _)| *current == tool).map(|(_, id)| id));
    let feedback_target = mgr.ui_state().feedback_profile[index].clone().filter(|id| profiles.iter().any(|p| p.id == *id));
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
                        Tool::Codex => kiln_common::i18n::tr("저장한 계정의 사용량 확인"),
                    };
                    if widgets::icon_button(ui, Icon::Refresh, 26.0, false, tip).clicked() {
                        mgr.refresh_usage(tool);
                    }
                }
            });
        });
        ui.label(RichText::new(kiln_common::i18n::tr("자동 갱신 · 5분마다")).font(fonts::regular(11.0)).color(t.text_faint));
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
                if login_target.as_deref() == Some(&p.id) {
                    ui.add_space(8.0);
                    egui::Frame::new().fill(t.bg_hover).corner_radius(8).inner_margin(12).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 8.0;
                        login_form(ui, mgr, tool);
                    });
                }
                if feedback_target.as_deref() == Some(&p.id) { login_feedback(ui, busy.as_deref(), error.as_deref(), notice.as_deref()); }
            }
        }
        ui.add_space(6.0);

        // 동작 버튼과 상태 줄.
        let adding = mgr.ui_state().adding.as_ref().is_some_and(|(current, _)| *current == tool);
        let relogin = mgr.ui_state().relogin.as_ref().is_some_and(|(current, _)| *current == tool);
        if !adding && !relogin && mgr.login_status(tool).is_none() { ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(busy.is_none(), |ui| {
                if widgets::button_with(ui, Some(Icon::Save), kiln_common::i18n::tr("현재 로그인한 계정 저장"), ButtonKind::Secondary, true).clicked() {
                    mgr.ui_state().feedback_profile[index] = None;
                    mgr.run_async(tool, kiln_common::i18n::tr("저장 중…"), move |m| {
                        let p = m.save_current(tool, "")?;
                        Ok(kiln_common::trf!("‘{}’ 계정을 저장했습니다", p.label))
                    });
                }
                if widgets::button_with(ui, Some(Icon::Plus), kiln_common::i18n::tr("새 계정 추가"), ButtonKind::Primary, true).clicked() {
                    mgr.ui_state().feedback_profile[index] = None;
                    mgr.ui_state().adding = Some((tool, String::new()));
                    mgr.ui_state().adding_focus = true;
                }
            });
        }); }
        if login_target.is_none() { login_form(ui, mgr, tool); }
        if feedback_target.is_none() { login_feedback(ui, busy.as_deref(), error.as_deref(), notice.as_deref()); }
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

fn login_feedback(ui: &mut Ui, busy: Option<&str>, error: Option<&str>, notice: Option<&str>) {
    let t = Theme::current();
    if let Some(text) = busy {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.0).color(t.text_dim));
            ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(12.0)).color(t.text_dim)).wrap());
        });
    }
    if let Some(text) = error {
        ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(12.0)).color(t.red)).wrap());
    } else if busy.is_none() && let Some(text) = notice {
        ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(12.0)).color(t.green)).wrap());
    }
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
        if let Some(profile) = mgr.login_profile(tool) {
            info_banner(ui, &kiln_common::trf!("‘{}’ 다시 로그인 · {}", profile.label, profile.email.as_deref().unwrap_or("")));
        }
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
                ui.spacing_mut().item_spacing.x = 10.0;
                let width = (ui.available_width() - 86.0).clamp(64.0, 220.0);
                let field_id = ui.id().with("login-code");
                let field = widgets::input_frame(ui.memory(|memory| memory.has_focus(field_id)), false).inner_margin(egui::Margin::symmetric(9, 6))
                    .show(ui, |ui| ui.add(egui::TextEdit::singleline(&mut code).id(field_id).frame(egui::Frame::NONE).font(fonts::regular(12.0)).password(true).hint_text(kiln_common::i18n::tr("로그인 코드")).desired_width((width - 18.0).max(40.0)))).inner;
                field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), kiln_common::i18n::tr("로그인 코드")));
                if ui.add_enabled_ui(!code.trim().is_empty(), |ui| widgets::button(ui, kiln_common::i18n::tr("확인"), ButtonKind::Secondary)).inner.clicked() {
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
    let relogin = mgr.ui_state().relogin.clone().filter(|(current, _)| *current == tool);
    if let Some((_, id)) = relogin {
        if let Some(profile) = mgr.profiles(tool).into_iter().find(|p| p.id == id) {
            info_banner(ui, &kiln_common::trf!("‘{}’ 다시 로그인 · {}", profile.label, profile.email.as_deref().unwrap_or("")));
            ui.add(egui::Label::new(kiln_common::i18n::tr("기존 계정으로 로그인하세요. 프로필과 대화 기록은 유지됩니다")).wrap());
            ui.horizontal_wrapped(|ui| {
                if widgets::button(ui, kiln_common::i18n::tr("브라우저에서 로그인"), ButtonKind::Primary).clicked() {
                    mgr.ui_state().relogin = None;
                    mgr.start_relogin(tool, &id);
                }
                if widgets::button(ui, kiln_common::i18n::tr("취소"), ButtonKind::Secondary).clicked() { mgr.ui_state().relogin = None; }
            });
        } else { mgr.ui_state().relogin = None; }
        return;
    }
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
    let (rect, response) = ui.allocate_exact_size(vec2(236.0, 16.0), Sense::hover());
    let p = ui.painter();
    let label = p.layout_no_wrap(label.to_string(), fonts::medium(11.0), t.text_faint);
    let bar_offset = (label.size().x + 8.0).max(34.0);
    p.galley(pos2(rect.left(), rect.center().y - label.size().y / 2.0), label, t.text_faint);
    let bar = Rect::from_min_size(pos2(rect.left() + bar_offset, rect.center().y - 2.5), vec2((108.0 - bar_offset).max(12.0), 5.0));
    p.rect_filled(bar, CornerRadius::same(3), t.bg_hover);
    match w {
        Some((u, reset)) => {
            let f = u.clamp(0.0, 1.0);
            if f > 0.0 {
                let fill = Rect::from_min_size(bar.min, vec2((bar.width() * f).max(3.0), bar.height()));
                p.rect_filled(fill, CornerRadius::same(3), usage_color(u));
            }
            let percent = p.layout_no_wrap(format!("{:.0}%", u * 100.0), fonts::medium(11.0), t.text_dim);
            let reset_left = bar.right() + 8.0 + percent.size().x + 8.0;
            p.galley(pos2(bar.right() + 8.0, rect.center().y - percent.size().y / 2.0), percent, t.text_dim);
            if let Some(r) = reset {
                let text = fmt_reset(r, now);
                let mut job = egui::text::LayoutJob::simple(text.clone(), fonts::regular(10.0), t.text_faint, (rect.right() - reset_left).max(0.0));
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = p.layout_job(job);
                p.galley(pos2(rect.right() - galley.size().x, rect.center().y - galley.size().y / 2.0), galley, t.text_faint);
                response.on_hover_text(text);
            }
        }
        None => {
            p.text(pos2(bar.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, "—", fonts::medium(11.0), t.text_faint);
        }
    }
}

/// `amount` minor units of `currency` (USD when unknown) as money text, rounded to whole units.
fn money(amount: f64, currency: Option<&str>) -> String {
    let code = currency.unwrap_or("USD");
    let units = if matches!(code, "JPY" | "KRW") { amount } else { amount / 100.0 };
    let digits = grouped(units.round() as i64);
    match code {
        "USD" => format!("${digits}"),
        "EUR" => format!("€{digits}"),
        "GBP" => format!("£{digits}"),
        "JPY" => format!("¥{digits}"),
        "KRW" => format!("₩{digits}"),
        other => format!("{digits} {other}"),
    }
}

/// `n` with a comma between each group of three digits.
fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 { text.push('-'); }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { text.push(','); }
        text.push(digit);
    }
    text
}

/// `n` tokens, in thousands (K), millions (M) or billions (B) past 999.
fn tokens(n: u64) -> String {
    let short = |value: f64, unit: &str| {
        let text = format!("{value:.1}");
        format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
    };
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => short(n as f64 / 1e3, "K"),
        1_000_000..1_000_000_000 => short(n as f64 / 1e6, "M"),
        _ => short(n as f64 / 1e9, "B"),
    }
}

/// The credit figures of an account row: Claude extra usage, Codex credits and reset credits.
pub(crate) fn credit_caption(usage: &Usage, now: i64) -> Option<String> {
    if usage.is_unavailable() { return None; }
    let mut parts = Vec::new();
    if let Some(extra) = &usage.extra {
        let currency = extra.currency.as_deref();
        parts.push(match (extra.enabled, extra.used, extra.limit) {
            (false, _, _) => kiln_common::i18n::tr("추가 사용 꺼짐").to_string(),
            (true, Some(used), Some(limit)) => kiln_common::trf!("추가 사용 {}", format!("{} / {}", money(used, currency), money(limit, currency))),
            (true, Some(used), None) => kiln_common::trf!("추가 사용 {}", money(used, currency)),
            (true, None, _) => kiln_common::i18n::tr("추가 사용 켜짐").to_string(),
        });
    }
    if let Some(credits) = &usage.credits {
        if credits.unlimited {
            parts.push(kiln_common::i18n::tr("크레딧 무제한").to_string());
        } else if let Some(balance) = credits.balance.as_deref().filter(|_| credits.has_credits) {
            let shown = balance.trim().parse::<f64>().map(|amount| grouped(amount.round() as i64)).unwrap_or_else(|_| balance.to_string());
            parts.push(kiln_common::trf!("크레딧 {}", shown));
        }
    }
    if let Some(resets) = usage.resets.as_ref().filter(|resets| resets.available > 0) {
        parts.push(kiln_common::trf!("초기화권 {}개", resets.available));
        if let Some(at) = resets.next_expiry.filter(|at| *at > now) {
            let hours = (at - now) / 3600;
            parts.push(if hours < 48 { kiln_common::trf!("{}시간 후 만료", hours.max(1)) } else { kiln_common::trf!("{}일 후 만료", hours / 24) });
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The notice for a reset-credit redemption of the account labelled `label`.
pub fn reset_outcome_text(outcome: crate::ResetOutcome, label: &str) -> String {
    match outcome {
        crate::ResetOutcome::Reset => kiln_common::trf!("‘{}’ 사용량 한도를 초기화했습니다", label),
        crate::ResetOutcome::NothingToReset => kiln_common::i18n::tr("초기화할 사용량이 없어 초기화권을 쓰지 않았습니다").into(),
        crate::ResetOutcome::NoCredit => kiln_common::i18n::tr("남은 초기화권이 없습니다").into(),
        crate::ResetOutcome::AlreadyRedeemed => kiln_common::i18n::tr("이미 사용된 초기화권입니다").into(),
    }
}

/// Daily token bars for the last 14 buckets of `daily`, each labelled with its date and total.
fn daily_bars(ui: &mut Ui, daily: &[(String, u64)]) {
    let t = Theme::current();
    let recent = &daily[daily.len().saturating_sub(14)..];
    if recent.is_empty() {
        ui.label(RichText::new(kiln_common::i18n::tr("기록 없음")).color(t.text_faint));
        return;
    }
    let peak = recent.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    let height = 96.0;
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 18.0), Sense::hover());
    let slot = area.width() / recent.len() as f32;
    for (i, (date, n)) in recent.iter().enumerate() {
        let left = area.left() + slot * i as f32;
        let bar_height = (height * (*n as f32 / peak as f32)).max(1.0);
        let bar = egui::Rect::from_min_max(pos2(left + slot * 0.15, area.top() + height - bar_height), pos2(left + slot * 0.85, area.top() + height));
        let label = format!("{} · {}", date.get(5..).unwrap_or(date), tokens(*n));
        let response = ui.interact(egui::Rect::from_min_max(pos2(left, area.top()), pos2(left + slot, area.top() + height)), ui.id().with(("usage-day", i)), Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &label));
        ui.painter().rect_filled(bar, CornerRadius::same(2), if response.hovered() { t.accent } else { t.accent.gamma_multiply(0.7) });
        response.on_hover_text(label);
    }
    let date = |i: usize| recent[i].0.get(5..).unwrap_or(&recent[i].0).to_owned();
    ui.painter().text(area.left_bottom(), Align2::LEFT_BOTTOM, date(0), fonts::regular(10.5), t.text_faint);
    ui.painter().text(area.right_bottom(), Align2::RIGHT_BOTTOM, date(recent.len() - 1), fonts::regular(10.5), t.text_faint);
}

/// The usage history dialog of the Codex profile chosen from its menu.
fn usage_history_dialog(ui: &mut Ui, mgr: &AccountManager) {
    let Some(id) = mgr.ui_state().history.clone() else { return };
    let Some(profile) = mgr.profiles(Tool::Codex).into_iter().find(|profile| profile.id == id) else {
        mgr.ui_state().history = None;
        return;
    };
    let t = Theme::current();
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new(("kiln-usage-history", &id))).show(ui.ctx(), |ui| {
        ui.set_width((ui.ctx().content_rect().width() - 48.0).clamp(240.0, 380.0));
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(RichText::new(kiln_common::trf!("‘{}’ 사용 내역", profile.label)).font(fonts::semibold(14.0)).color(t.text));
        ui.add_space(6.0);
        match mgr.usage_history(&id) {
            None | Some(crate::HistoryState::Loading) => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(kiln_common::i18n::tr("내역을 불러오는 중…"));
                });
            }
            Some(crate::HistoryState::Failed(error)) => {
                ui.label(RichText::new(kiln_common::i18n::tr("내역을 불러오지 못했습니다")).color(t.text_dim)).on_hover_text(error);
            }
            Some(crate::HistoryState::Ready(history)) => {
                let lines = [
                    history.lifetime_tokens.map(|n| kiln_common::trf!("누적 토큰 {}", tokens(n))),
                    history.peak_daily_tokens.map(|n| kiln_common::trf!("최고 일일 토큰 {}", tokens(n))),
                    history.current_streak_days.zip(history.longest_streak_days).map(|(now, best)| kiln_common::trf!("연속 사용 {}일 · 최장 {}일", now, best)),
                ];
                for line in lines.into_iter().flatten() { ui.label(RichText::new(line).color(t.text)); }
                ui.add_space(8.0);
                ui.label(RichText::new(kiln_common::i18n::tr("최근 14일")).font(fonts::medium(12.0)).color(t.text_dim));
                daily_bars(ui, &history.daily);
            }
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, kiln_common::i18n::tr("닫기"), ButtonKind::Secondary).clicked() { close = true; }
        });
    });
    if close || modal.should_close() { mgr.ui_state().history = None; }
}

/// 사용량 열(폭 236, 높이 36 고정).
fn usage_failure_label(usage: &Usage) -> &'static str {
    let reason = &usage.status;
    kiln_common::i18n::tr(if reason.contains("sign-in required") { "다시 로그인 필요" }
        else if reason.contains("sign-in expired") { "로그인 만료 · 이 계정을 쓰면 갱신" }
        else if reason.contains("token renewal") { "로그인 갱신 실패 · 자동 재시도" }
        else if reason.contains("HTTP 429") { "사용량 조회 제한 · 자동 재시도 대기" }
        else if reason.contains("HTTP 403") { "사용량 조회 권한 확인 필요" }
        else if reason.contains("HTTP 401") || reason.contains("authentication required") { "사용량 인증 확인 필요" }
        else { "사용량 확인 불가" })
}

/// How long ago usage was last read at `at`.
fn checked_ago(now: i64, at: i64) -> String {
    let minutes = now.saturating_sub(at).max(0) / 60;
    match minutes {
        0 => kiln_common::i18n::tr("방금 확인").to_string(),
        1..60 => kiln_common::trf!("{}분 전 확인", minutes),
        60..2880 => kiln_common::trf!("{}시간 전 확인", minutes / 60),
        _ => kiln_common::trf!("{}일 전 확인", minutes / 1440),
    }
}

fn usage_block(ui: &mut Ui, usage: Option<&Usage>, now: i64, fetched_at: Option<i64>, refreshing: bool) {
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
                let l = ui.add(egui::Label::new(RichText::new(text).font(fonts::regular(11.5)).color(color)).truncate()).on_hover_text(text);
                resp | l
            })
            .inner
        };
        if refreshing {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(kiln_common::i18n::tr("확인 중…"));
            });
            return;
        }
        match usage {
            Some(u) if !u.is_unavailable() && fetched_at.is_some_and(|at| now.saturating_sub(at) < 300) && (u.five_hour.is_some() || u.seven_day.is_some()) => {
                usage_line(ui, kiln_common::i18n::tr("5시간"), u.five_hour, now);
                usage_line(ui, kiln_common::i18n::tr("7일"), u.seven_day, now);
                if let Some(at) = fetched_at {
                    ui.label(RichText::new(checked_ago(now, at)).font(fonts::regular(10.5)).color(t.text_faint));
                }
            }
            Some(u) if u.is_unavailable() => {
                let reason = u.status.trim_start_matches("unavailable:").trim().to_string();
                let label = match fetched_at {
                    Some(at) => format!("{} · {}", usage_failure_label(u), checked_ago(now, at)),
                    None => usage_failure_label(u).to_string(),
                };
                note(ui, Icon::Warning, &label, t.text_faint).on_hover_text(reason);
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
    let login_required = mgr.login_required(tool, &p.id);
    let compact = ui.available_width() < 600.0;
    let identity_width = (ui.available_width() - 166.0 - if compact || login_required { 0.0 } else { 252.0 }).max(64.0);
    let exhausted = !mgr.is_refreshing(tool) && mgr.usage_fetched_at(tool, &p.id).is_some_and(|at| now.saturating_sub(at) < 300) && usage.as_ref().is_some_and(|u| !u.is_unavailable() && u.exhausted_at(now));
    let renaming = mgr.ui_state().renaming.as_ref().filter(|(rt, id, _)| *rt == tool && *id == p.id).map(|r| r.2.clone());
    let confirming = mgr.ui_state().confirm_delete.as_ref().is_some_and(|(ct, id)| *ct == tool && *id == p.id);
    let confirming_reset = tool == Tool::Codex && mgr.ui_state().confirm_reset.as_deref() == Some(p.id.as_str());
    let caption = usage.as_ref().and_then(|usage| credit_caption(usage, now));
    let resets_available = usage.as_ref().filter(|usage| !usage.is_unavailable()).and_then(|usage| usage.resets.as_ref()).map_or(0, |resets| resets.available);

    ui.horizontal(|ui| {
        ui.set_min_height(56.0);
        ui.spacing_mut().item_spacing.x = 10.0;
        let (ar, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        widgets::avatar(ui, ar, &p.label, is_active);

        ui.allocate_ui_with_layout(vec2(identity_width, if caption.is_some() && !compact { 50.0 } else { 36.0 }), Layout::top_down(Align::Min), |ui| {
            ui.set_width(identity_width);
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
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add(egui::Label::new(RichText::new(&p.label).font(fonts::semibold(13.5)).color(t.text)).truncate());
                    if is_active {
                        widgets::pill(ui, kiln_common::i18n::tr("사용 중"), t.green);
                    }
                    if exhausted {
                        widgets::pill(ui, kiln_common::i18n::tr("소진"), t.red);
                    }
                });
            }
            let email = p.email.as_deref().unwrap_or(kiln_common::i18n::tr("이메일 알 수 없음"));
            ui.add(egui::Label::new(RichText::new(email).font(fonts::regular(12.0)).color(t.text_faint)).truncate()).on_hover_text(email);
            if let Some(caption) = caption.as_ref().filter(|_| !compact) {
                ui.add(egui::Label::new(RichText::new(caption).font(fonts::regular(11.0)).color(t.text_dim)).truncate()).on_hover_text(caption);
            }
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if confirming {
                if widgets::button_with(ui, None, kiln_common::i18n::tr("삭제"), ButtonKind::Danger, true).clicked() {
                    mgr.ui_state().feedback_profile[if tool == Tool::Claude { 0 } else { 1 }] = Some(p.id.clone());
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
            if confirming_reset {
                if ui.add_enabled_ui(!busy, |ui| widgets::button_with(ui, None, kiln_common::i18n::tr("사용"), ButtonKind::Primary, true)).inner.clicked() {
                    mgr.ui_state().feedback_profile[1] = Some(p.id.clone());
                    mgr.ui_state().confirm_reset = None;
                    let id = p.id.clone();
                    let label = p.label.clone();
                    mgr.run_async(tool, kiln_common::i18n::tr("초기화권 사용 중…"), move |m| {
                        let outcome = m.redeem_reset_credit(&id)?;
                        Ok(reset_outcome_text(outcome, &label))
                    });
                }
                if widgets::button_with(ui, None, kiln_common::i18n::tr("취소"), ButtonKind::Ghost, true).clicked() {
                    mgr.ui_state().confirm_reset = None;
                }
                ui.label(RichText::new(kiln_common::i18n::tr("초기화권 1개를 사용할까요?")).font(fonts::medium(12.5)).color(t.text_dim));
                return;
            }
            let more = widgets::icon_button(ui, Icon::Menu, 26.0, false, kiln_common::i18n::tr("계정 메뉴"));
            egui::Popup::menu(&more).gap(4.0).show(|ui| {
                ui.set_min_width(150.0);
                if menu_item(ui, Icon::Pencil, kiln_common::i18n::tr("이름 변경"), t.text) {
                    mgr.ui_state().renaming = Some((tool, p.id.clone(), p.label.clone()));
                    ui.close();
                }
                if ui.add_enabled_ui(!busy, |ui| menu_item(ui, Icon::Refresh, kiln_common::i18n::tr("다시 로그인"), t.text)).inner {
                    mgr.ui_state().feedback_profile[if tool == Tool::Claude { 0 } else { 1 }] = Some(p.id.clone());
                    mgr.with_state(|s| { s.error[if tool == Tool::Claude { 0 } else { 1 }] = None; s.notice[if tool == Tool::Claude { 0 } else { 1 }] = None; });
                    mgr.ui_state().adding = None;
                    mgr.ui_state().relogin = Some((tool, p.id.clone()));
                    ui.close();
                }
                if tool == Tool::Codex {
                    if ui.add_enabled_ui(resets_available > 0 && !busy, |ui| menu_item(ui, Icon::Undo, kiln_common::i18n::tr("초기화권 사용"), t.text)).inner {
                        mgr.ui_state().confirm_reset = Some(p.id.clone());
                        ui.close();
                    }
                    if menu_item(ui, Icon::History, kiln_common::i18n::tr("사용 내역"), t.text) {
                        mgr.ui_state().history = Some(p.id.clone());
                        mgr.load_usage_history(&p.id);
                        ui.close();
                    }
                }
                let can_delete = !is_active && !busy;
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
                            mgr.ui_state().feedback_profile[if tool == Tool::Claude { 0 } else { 1 }] = Some(p.id.clone());
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
            if !login_required && !compact { usage_block(ui, usage.as_ref(), now, mgr.usage_fetched_at(p.tool, &p.id), mgr.is_refreshing(p.tool)); }
        });
    });
    if compact && !login_required { usage_block(ui, usage.as_ref(), now, mgr.usage_fetched_at(p.tool, &p.id), mgr.is_refreshing(p.tool)); }
    if let Some(caption) = caption.as_ref().filter(|_| compact) {
        ui.add(egui::Label::new(RichText::new(caption).font(fonts::regular(11.0)).color(t.text_dim)).wrap()).on_hover_text(caption);
    }
    if login_required {
        ui.add_space(8.0);
        ui.indent(("login-recovery", &p.id), |ui| { ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            widgets::pill(ui, kiln_common::i18n::tr("로그인 필요"), t.orange);
            if ui.add_enabled_ui(!busy, |ui| widgets::button_with(ui, Some(Icon::Refresh), kiln_common::i18n::tr("다시 로그인"), ButtonKind::Secondary, true)).inner.clicked() {
                mgr.ui_state().feedback_profile[if tool == Tool::Claude { 0 } else { 1 }] = Some(p.id.clone());
                mgr.with_state(|s| { s.error[if tool == Tool::Claude { 0 } else { 1 }] = None; s.notice[if tool == Tool::Claude { 0 } else { 1 }] = None; });
                mgr.ui_state().adding = None;
                mgr.ui_state().relogin = Some((tool, p.id.clone()));
            }
        }); });
    }
    ui.add_space(8.0);
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
    use crate::CredentialStore;

    fn test_fonts(ui: &mut Ui) -> bool {
        let id = egui::Id::new("accounts-login-test-fonts");
        if ui.ctx().data(|data| data.get_temp::<bool>(id)).unwrap_or(false) { return true; }
        ui.ctx().data_mut(|data| data.insert_temp(id, true));
        ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
        false
    }

    #[test]
    fn usage_card_hides_cached_numbers_during_refresh_failure_and_expiry() {
        use egui_kittest::kittest::Queryable;
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            for (refreshing, failed, age) in [(true, false, 0), (false, true, 0), (false, false, 301), (false, false, 0)] {
                let usage = Usage { five_hour: Some((0.73, None)), seven_day: None, status: if failed { "unavailable: HTTP 429" } else { "allowed" }.into(), ..Default::default() };
                let mut harness = egui_kittest::Harness::builder().with_size(vec2(400.0, 150.0)).build_ui(move |ui| {
                    if test_fonts(ui) { usage_block(ui, Some(&usage), 1000, Some(1000-age), refreshing); }
                });
                for _ in 0..4 { harness.step(); }
                assert_eq!(harness.query_all_by_label("확인 중…").count(), usize::from(refreshing));
                assert_eq!(harness.query_all_by_label("방금 확인").count(), usize::from(!refreshing && !failed && age == 0));
            }
        });
    }

    #[test]
    fn credit_captions_name_extra_usage_credits_and_reset_credits() {
        use crate::{Credits, ExtraUsage, ResetCredits};
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let now = 1_000_000;
            let usage = |extra: Option<ExtraUsage>, credits: Option<Credits>, resets: Option<ResetCredits>| Usage { five_hour: Some((0.1, None)), status: "allowed".into(), extra, credits, resets, ..Default::default() };
            let spend = |used: Option<f64>, limit: Option<f64>, currency: &str| Some(ExtraUsage { enabled: true, used, limit, currency: Some(currency.into()) });
            assert_eq!(credit_caption(&usage(spend(Some(1230.0), Some(5000.0), "USD"), None, None), now).as_deref(), Some("추가 사용 $12 / $50"));
            assert_eq!(credit_caption(&usage(spend(Some(500.0), None, "JPY"), None, None), now).as_deref(), Some("추가 사용 ¥500"));
            assert_eq!(credit_caption(&usage(spend(Some(1999.0), Some(10000.0), "CAD"), None, None), now).as_deref(), Some("추가 사용 20 CAD / 100 CAD"));
            assert_eq!(credit_caption(&usage(spend(Some(123_456.0), Some(1_000_000.0), "USD"), None, None), now).as_deref(), Some("추가 사용 $1,235 / $10,000"));
            assert_eq!(credit_caption(&usage(spend(None, None, "USD"), None, None), now).as_deref(), Some("추가 사용 켜짐"));
            assert_eq!(credit_caption(&usage(Some(ExtraUsage::default()), None, None), now).as_deref(), Some("추가 사용 꺼짐"));
            let balance = Some(Credits { has_credits: true, unlimited: false, balance: Some("120".into()) });
            assert_eq!(credit_caption(&usage(None, balance, Some(ResetCredits { available: 2, next_expiry: Some(now + 5 * 86_400 + 60) })), now).as_deref(), Some("크레딧 120 · 초기화권 2개 · 5일 후 만료"));
            let fractional = Some(Credits { has_credits: true, unlimited: false, balance: Some("28655.4911590000".into()) });
            assert_eq!(credit_caption(&usage(None, fractional, None), now).as_deref(), Some("크레딧 28,655"));
            let unlimited = Some(Credits { has_credits: true, unlimited: true, balance: None });
            assert_eq!(credit_caption(&usage(None, unlimited, Some(ResetCredits { available: 1, next_expiry: Some(now + 5 * 3600) })), now).as_deref(), Some("크레딧 무제한 · 초기화권 1개 · 5시간 후 만료"));
            assert_eq!(credit_caption(&usage(None, None, Some(ResetCredits { available: 1, next_expiry: None })), now).as_deref(), Some("초기화권 1개"));
            assert_eq!(credit_caption(&usage(None, Some(Credits::default()), Some(ResetCredits::default())), now), None);
            assert_eq!(credit_caption(&Usage::unavailable("offline"), now), None);
        });
    }

    #[test]
    fn token_counts_are_shortened() {
        assert_eq!([0, 999, 1000, 123_456, 12_345_678, 2_500_000_000].map(tokens), ["0", "999", "1K", "123.5K", "12.3M", "2.5B"]);
    }

    fn codex_with_reset_credit(root: &std::path::Path, available: u32) -> (crate::Env, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (mut env, _) = crate::Env::sandbox(root, false);
        env.codex_probe = Some(std::sync::Arc::new(move |_| Ok(Usage { five_hour: Some((1.0, Some(4_200_000_000))), status: "allowed".into(),
            resets: Some(crate::ResetCredits { available, next_expiry: None }), ..Default::default() })));
        let redeemed = std::sync::Arc::new(AtomicUsize::new(0));
        let count = redeemed.clone();
        env.codex_reset = std::sync::Arc::new(move |_, _| { count.fetch_add(1, Ordering::SeqCst); Ok(crate::ResetOutcome::Reset) });
        std::fs::create_dir_all(&env.codex_home).unwrap();
        std::fs::write(env.codex_home.join("auth.json"), crate::codex::tests::fake_auth("work@example.test", "fixture")).unwrap();
        (env, redeemed)
    }

    #[test]
    fn a_reset_credit_is_used_only_after_the_row_confirms() {
        use egui_kittest::kittest::Queryable;
        use std::sync::atomic::Ordering;
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let root = tempfile::tempdir().unwrap();
            let (env, redeemed) = codex_with_reset_credit(root.path(), 1);
            let manager = AccountManager::with_env(env);
            manager.save_current(Tool::Codex, "Work").unwrap();
            manager.refresh_usage_blocking(Tool::Codex);
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(760.0, 520.0))
                .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager.clone());
            for _ in 0..3 { harness.step(); }
            harness.get_by_label("계정 메뉴").click();
            for _ in 0..3 { harness.step(); }
            harness.get_by_label("초기화권 사용").click();
            for _ in 0..3 { harness.step(); }
            assert!(harness.query_by_label("초기화권 1개를 사용할까요?").is_some());
            assert_eq!(redeemed.load(Ordering::SeqCst), 0, "opening the confirmation redeems nothing");
            harness.get_by_label("사용").click();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while harness.query_by_label("‘Work’ 사용량 한도를 초기화했습니다").is_none() {
                assert!(std::time::Instant::now() < deadline, "no outcome notice");
                harness.step();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(redeemed.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn the_reset_item_is_unavailable_without_a_credit() {
        use egui_kittest::kittest::{NodeT, Queryable};
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let root = tempfile::tempdir().unwrap();
            let (env, _) = codex_with_reset_credit(root.path(), 0);
            let manager = AccountManager::with_env(env);
            manager.save_current(Tool::Codex, "Work").unwrap();
            manager.refresh_usage_blocking(Tool::Codex);
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(760.0, 520.0))
                .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager);
            for _ in 0..3 { harness.step(); }
            harness.get_by_label("계정 메뉴").click();
            for _ in 0..3 { harness.step(); }
            assert!(harness.get_by_label("초기화권 사용").accesskit_node().is_disabled());
        });
    }

    #[test]
    fn usage_history_dialog_shows_the_summary_and_daily_tokens() {
        use egui_kittest::kittest::Queryable;
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let root = tempfile::tempdir().unwrap();
            let (mut env, _) = codex_with_reset_credit(root.path(), 1);
            env.codex_history = std::sync::Arc::new(|_, _| Ok(crate::UsageHistory {
                lifetime_tokens: Some(123_456), peak_daily_tokens: Some(5000), current_streak_days: Some(3), longest_streak_days: Some(9),
                daily: (1..=20).map(|day| (format!("2026-10-{day:02}"), day * 100)).collect(),
            }));
            let manager = AccountManager::with_env(env);
            manager.save_current(Tool::Codex, "Work").unwrap();
            manager.refresh_usage_blocking(Tool::Codex);
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(760.0, 620.0)).wgpu()
                .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager);
            for _ in 0..3 { harness.step(); }
            harness.get_by_label("계정 메뉴").click();
            for _ in 0..3 { harness.step(); }
            harness.get_by_label("사용 내역").click();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while harness.query_by_label("누적 토큰 123.5K").is_none() {
                assert!(std::time::Instant::now() < deadline, "history not shown");
                harness.step();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            for label in ["‘Work’ 사용 내역", "최고 일일 토큰 5K", "연속 사용 3일 · 최장 9일", "10-07 · 700", "10-20 · 2K"] {
                assert!(harness.query_by_label(label).is_some(), "{label}");
            }
            assert!(harness.query_by_label("10-06 · 600").is_none(), "only the last 14 days are drawn");
            // The dialog is centered again once the history replaces the spinner.
            for _ in 0..3 { harness.step(); }
            harness.render().unwrap().save(std::env::temp_dir().join("kiln024-usage-history.png")).unwrap();
            harness.get_by_label("닫기").click();
            for _ in 0..3 { harness.step(); }
            assert!(harness.query_by_label("‘Work’ 사용 내역").is_none());
        });
    }

    #[test]
    fn credit_captions_fit_four_languages_at_narrow_scaled_width() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                let root = tempfile::tempdir().unwrap();
                let (mut env, _) = codex_with_reset_credit(root.path(), 2);
                env.codex_probe = Some(std::sync::Arc::new(|_| Ok(Usage { five_hour: Some((0.25, Some(4_200_000_000))), status: "allowed".into(),
                    credits: Some(crate::Credits { has_credits: true, unlimited: false, balance: Some("28655.4911590000".into()) }),
                    resets: Some(crate::ResetCredits { available: 2, next_expiry: Some(crate::now_unix() + 3 * 86_400 + 600) }), ..Default::default() })));
                let manager = AccountManager::with_env(env);
                manager.save_current(Tool::Codex, "Work").unwrap();
                manager.refresh_usage_blocking(Tool::Codex);
                let caption = credit_caption(&manager.usage(Tool::Codex, &manager.profiles(Tool::Codex)[0].id).unwrap(), crate::now_unix()).unwrap();
                let mut harness = egui_kittest::Harness::builder().with_size(vec2(420.0, 760.0)).wgpu()
                    .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager);
                harness.ctx.set_zoom_factor(1.3);
                for _ in 0..4 { harness.step(); }
                let node = harness.get_by_label(&caption);
                assert!(harness.ctx.content_rect().contains_rect(node.rect()), "{language:?}: caption clipped");
                harness.render().unwrap().save(std::env::temp_dir().join(format!("kiln024-credit-caption-{language:?}.png"))).unwrap();
            });
        }
    }

    #[test]
    fn a_failed_usage_read_names_the_reason_and_when_usage_was_last_read() {
        use egui_kittest::kittest::Queryable;
        kiln_common::i18n::with_language(kiln_common::i18n::Language::Korean, || {
            let now = 1_000_000;
            for (reason, fetched, shown) in [
                ("sign-in expired", Some(now - 6 * 3600), "로그인 만료 · 이 계정을 쓰면 갱신 · 6시간 전 확인"),
                ("sign-in required", Some(now - 3 * 86_400), "다시 로그인 필요 · 3일 전 확인"),
                ("token renewal failed (HTTP 503)", Some(now - 300), "로그인 갱신 실패 · 자동 재시도 · 5분 전 확인"),
                ("usage authorization rejected (HTTP 401)", None, "사용량 인증 확인 필요"),
            ] {
                let usage = Usage { five_hour: Some((0.5, None)), seven_day: None, status: format!("unavailable: {reason}"), ..Default::default() };
                let mut harness = egui_kittest::Harness::builder().with_size(vec2(400.0, 150.0)).build_ui(move |ui| {
                    if test_fonts(ui) { usage_block(ui, Some(&usage), now, fetched, false); }
                });
                for _ in 0..4 { harness.step(); }
                assert_eq!(harness.query_all_by_label(shown).count(), 1, "{reason}");
            }
        });
    }

    #[test]
    fn reauthentication_controls_stay_between_target_account_and_next_row() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for target in [0, 1] {
                    for waiting in [false, true] {
                        let root = tempfile::tempdir().unwrap();
                        let (env, store) = crate::Env::sandbox(root.path(), true);
                        let manager = AccountManager::with_env(env.clone());
                        let mut profiles = vec![];
                        for (label, email) in [("Personal", "personal@example.test"), ("Work", "work@example.test")] {
                            store.set(crate::claude::CLAUDE_SERVICE, "tester", r#"{"claudeAiOauth":{"accessToken":"fixture"}}"#).unwrap();
                            crate::claude::write_oauth_account(&env.home.join(".claude.json"), Some(&serde_json::json!({"emailAddress":email}))).unwrap();
                            profiles.push(manager.save_current(Tool::Claude, label).unwrap());
                        }
                        if waiting {
                            manager.login_fixture_for_profile(Tool::Claude, crate::LoginStatus { browser_url: Some("https://claude.ai/oauth/authorize?state=fixture".into()), accepts_code: true }, Some(profiles[target].id.clone()));
                        } else { manager.ui_state().relogin = Some((Tool::Claude, profiles[target].id.clone())); }
                        let mut h = egui_kittest::Harness::builder().with_size(vec2(420.0, 1050.0)).wgpu()
                            .build_ui_state(|ui, mgr: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, mgr, Tool::Claude); } }, manager);
                        h.ctx.set_zoom_factor(1.3);
                        for _ in 0..4 { h.step(); }
                        let labels: &[&str] = if waiting { &["로그인 페이지 다시 열기", "로그인 코드", "로그인 취소", "확인"] } else { &["브라우저에서 로그인", "취소"] };
                        let account_rect = h.get_by_label(&profiles[target].label).rect();
                        for label in labels {
                            let rect = h.get_by_label(kiln_common::i18n::tr(label)).rect();
                            assert!(h.ctx.content_rect().contains_rect(rect), "{language:?}: {label} clipped");
                            assert!(rect.top() > account_rect.bottom());
                            if target == 0 { assert!(rect.bottom() < h.get_by_label("Work").rect().top(), "{label} rendered under the wrong account"); }
                        }
                        if waiting { h.render().unwrap().save(std::env::temp_dir().join(format!("kiln022-reauth-{language:?}-{target}.png"))).unwrap(); }
                    }
                }
            });
        }
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
    fn automatic_usage_card_fits_four_languages_at_narrow_scaled_width() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                let root = tempfile::tempdir().unwrap();
                let (mut env, _) = crate::Env::sandbox(root.path(), false);
                env.codex_probe = Some(std::sync::Arc::new(|_| Ok(Usage { five_hour: Some((0.25, Some(4_200_000_000))), seven_day:Some((0.7,Some(4_200_000_000))), status:"allowed".into(), ..Default::default() })));
                std::fs::create_dir_all(&env.codex_home).unwrap();
                std::fs::write(env.codex_home.join("auth.json"), crate::codex::tests::fake_auth("work@example.test", "fixture")).unwrap();
                let manager = AccountManager::with_env(env);
                manager.save_current(Tool::Codex, "Work").unwrap();
                manager.refresh_usage_blocking(Tool::Codex);
                let mut harness = egui_kittest::Harness::builder().with_size(vec2(420.0, 760.0)).wgpu()
                    .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager);
                harness.ctx.set_zoom_factor(1.3);
                for _ in 0..4 { harness.step(); }
                for label in ["자동 갱신 · 5분마다", "방금 확인"] {
                    assert!(harness.ctx.content_rect().contains_rect(harness.get_by_label(kiln_common::i18n::tr(label)).rect()), "{language:?}: {label} clipped");
                }
                harness.render().unwrap().save(std::env::temp_dir().join(format!("kiln020-usage-card-{language:?}.png"))).unwrap();
            });
        }
    }

    #[test]
    fn missing_login_row_opens_reauthentication_and_cancel_keeps_account() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                let root = tempfile::tempdir().unwrap();
                let (env, _) = crate::Env::sandbox(root.path(), false);
                std::fs::create_dir_all(&env.codex_home).unwrap();
                std::fs::write(env.codex_home.join("auth.json"), crate::codex::tests::fake_auth("work@example.test", "old")).unwrap();
                let manager = AccountManager::with_env(env);
                let profile = manager.save_current(Tool::Codex, "Work").unwrap();
                let variables = manager.launch_environment(Tool::Codex).unwrap();
                let home = variables.iter().find(|(key,_)|key=="CODEX_HOME").unwrap().1.clone();
                std::fs::remove_file(std::path::Path::new(&home).join("auth.json")).unwrap();
                manager.refresh_login_status();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !manager.login_required(Tool::Codex, &profile.id) {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let mut harness = egui_kittest::Harness::builder().with_size(vec2(420.0, 760.0)).wgpu()
                    .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { tool_card(ui, manager, Tool::Codex); } }, manager.clone());
                harness.ctx.set_zoom_factor(1.3);
                for _ in 0..4 { harness.step(); }
                for label in ["로그인 필요", "다시 로그인"] {
                    assert!(harness.ctx.content_rect().contains_rect(harness.get_by_label(kiln_common::i18n::tr(label)).rect()), "{language:?}: {label} clipped in full card");
                }
                harness.render().unwrap().save(std::env::temp_dir().join(format!("kiln019-reauth-row-{language:?}.png"))).unwrap();
                harness.get_by_label(kiln_common::i18n::tr("다시 로그인")).click();
                for _ in 0..4 { harness.step(); }
                let button = harness.get_by_label(kiln_common::i18n::tr("브라우저에서 로그인"));
                assert!(harness.ctx.content_rect().contains_rect(button.rect()));
                harness.render().unwrap().save(std::env::temp_dir().join(format!("kiln019-reauth-form-{language:?}.png"))).unwrap();
                assert_eq!(manager.ui_state().relogin, Some((Tool::Codex, profile.id.clone())));
                harness.get_by_label(kiln_common::i18n::tr("취소")).click();
                for _ in 0..4 { harness.step(); }
                assert!(manager.ui_state().relogin.is_none());
                assert_eq!(manager.profiles(Tool::Codex), vec![profile]);
            });
        }
    }

    #[test]
    fn reauthentication_form_keeps_profile_and_fits_all_languages() {
        use egui_kittest::kittest::Queryable;
        for language in kiln_common::i18n::Language::ALL {
            kiln_common::i18n::with_language(language, || {
                for width in [420.0, 720.0] {
                    let root = tempfile::tempdir().unwrap();
                    let (env, _) = crate::Env::sandbox(root.path(), false);
                    std::fs::create_dir_all(&env.codex_home).unwrap();
                    std::fs::write(env.codex_home.join("auth.json"), crate::codex::tests::fake_auth("work@example.test", "old")).unwrap();
                    let manager = AccountManager::with_env(env);
                    let profile = manager.save_current(Tool::Codex, "Work").unwrap();
                    manager.ui_state().relogin = Some((Tool::Codex, profile.id.clone()));
                    let mut harness = egui_kittest::Harness::builder().with_size(vec2(width, 400.0))
                        .build_ui_state(|ui, manager: &mut AccountManager| { if test_fonts(ui) { login_form(ui, manager, Tool::Codex); } }, manager.clone());
                    harness.ctx.set_zoom_factor(1.3);
                    for _ in 0..4 { harness.step(); }
                    for label in ["브라우저에서 로그인", "취소"] {
                        assert!(harness.ctx.content_rect().contains_rect(harness.get_by_label(kiln_common::i18n::tr(label)).rect()), "{language:?}: {label} at {width}");
                    }
                    assert_eq!(harness.query_all_by_label(kiln_common::i18n::tr("계정 이름")).count(), 0);
                    harness.get_by_label(kiln_common::i18n::tr("취소")).click();
                    for _ in 0..4 { harness.step(); }
                    assert!(manager.ui_state().relogin.is_none());
                    assert_eq!(manager.profiles(Tool::Codex), vec![profile]);
                    assert!(manager.login_status(Tool::Codex).is_none());
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
                                Tool::Codex => kiln_common::i18n::tr("저장한 계정의 사용량 확인"),
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
            // This headless test inspects shapes and cache identities without a renderer.
            // Clear unapplied deltas before assertions so failures can unwind normally.
            output.textures_delta.clear();
            for tool in Tool::ALL {
                let name = match tool { Tool::Claude => "claude", Tool::Codex => "codex" };
                let pixels = (24.0 * ctx.pixels_per_point()).ceil() as u32;
                let texture = ctx.data(|data| data.get_temp::<egui::TextureHandle>(egui::Id::new(("bundled-mark", name, pixels))))
                    .expect("account header must use the shared embedded SVG, not a hand-drawn substitute");
                let meshes: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh.calc_bounds()),
                    _ => None,
                }).collect();
                assert_eq!(meshes.len(), 1, "missing account logo for {name}");
                assert_eq!(meshes[0].size(), vec2(24.0, 24.0));
            }
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
                        image.save(std::env::temp_dir().join(format!("kiln-account-login-{language:?}-{theme}-{tool:?}-420.png"))).unwrap();
                    }
                }
            });
        }
        kiln_common::Theme::set_current("kiln-dark");
    }
}
