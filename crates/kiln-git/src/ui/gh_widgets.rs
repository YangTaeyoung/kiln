//! GitHub 허브 공용 위젯: 오류 안내, 라벨 칩, 아바타, 다중 선택 팝업, 카운트 탭.

use egui::{Align2, Color32, CornerRadius, Id, Margin, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use kiln_common::fonts;
use kiln_common::widgets::{ButtonKind, button_with, lerp_color};

use super::widgets::*;
use crate::GitEvent;
use crate::cmd::GitError;

/// 오류 안내에서 사용자가 누른 동작.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorAction {
    None,
    Retry,
}

/// gh 설치/로그인/일반 오류를 빈 상태 화면으로 안내한다.
/// 로그인 안내의 버튼은 `GitEvent::RunInTerminal("gh auth login")` 을 `events` 에 넣는다.
pub(crate) fn gh_error_state(ui: &mut Ui, err: &GitError, subject: &str, events: &mut Vec<GitEvent>) -> ErrorAction {
    use kiln_common::icons::Icon as CI;
    let mut action = ErrorAction::None;
    match err {
        GitError::GhMissing => {
            let clicked = empty_panel(
                ui,
                Icon::Warning,
                "GitHub CLI를 찾을 수 없습니다",
                "GitHub 연결에는 gh가 필요합니다. 설치되어 있다면 실행 경로를 확인한 뒤 다시 시도하세요.",
                None,
                &[(None, "설치 안내 열기", ButtonKind::Primary), (Some(CI::Refresh), "다시 시도", ButtonKind::Secondary)],
            );
            match clicked {
                Some(0) => events.push(GitEvent::OpenUrl("https://cli.github.com".into())),
                Some(_) => action = ErrorAction::Retry,
                None => {}
            }
        }
        GitError::GhAuth(_) => {
            let clicked = empty_panel(
                ui,
                Icon::Lock,
                "GitHub에 로그인하세요",
                "로그인하면 이슈, Pull Request와 Actions를 이곳에서 확인할 수 있습니다. 터미널에서 로그인한 뒤 다시 시도하세요.",
                Some("gh auth login"),
                &[(Some(CI::Terminal), "터미널에서 로그인", ButtonKind::Primary), (None, "다시 시도", ButtonKind::Secondary)],
            );
            match clicked {
                Some(0) => events.push(GitEvent::RunInTerminal("gh auth login".into())),
                Some(_) => action = ErrorAction::Retry,
                None => {}
            }
        }
        other => {
            let title = format!("{subject} 정보를 불러오지 못했습니다");
            if empty_panel(ui, Icon::Warning, &title, &other.to_string(), None, &[(Some(CI::Refresh), "다시 시도", ButtonKind::Secondary)]).is_some() {
                action = ErrorAction::Retry;
            }
        }
    }
    action
}

/// 가운데 정렬 빈 상태: 아이콘 타일, 제목, 설명, 선택적 명령 표시, 버튼 줄. 눌린 버튼 번호를 돌려준다.
pub(crate) fn empty_panel(
    ui: &mut Ui,
    icon: Icon,
    title: &str,
    detail: &str,
    command: Option<&str>,
    buttons: &[(Option<kiln_common::icons::Icon>, &str, ButtonKind)],
) -> Option<usize> {
    let t = theme();
    let mut clicked = None;
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(ui.available_width().min(360.0));
        let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(12), t.bg_hover);
        paint_icon(ui.painter(), Rect::from_center_size(r.center(), vec2(20.0, 20.0)), icon, t.text_dim);
        ui.add_space(12.0);
        ui.label(RichText::new(title).font(fonts::semibold(14.0)).color(t.text));
        if !detail.is_empty() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(t.text_dim)).wrap());
        }
        if let Some(c) = command {
            ui.add_space(10.0);
            code_pill(ui, c);
        }
        if !buttons.is_empty() {
            ui.add_space(14.0);
            let gap = 8.0;
            let widths: Vec<f32> = buttons
                .iter()
                .map(|(i, l, _)| {
                    let g = ui.painter().layout_no_wrap(l.to_string(), fonts::medium(12.5), t.text);
                    g.size().x + 20.0 + if i.is_some() { 20.0 } else { 0.0 }
                })
                .collect();
            let total = widths.iter().sum::<f32>() + gap * (buttons.len() as f32 - 1.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0) - gap);
                for (n, (i, l, k)) in buttons.iter().enumerate() {
                    if button_with(ui, *i, l, *k, true).clicked() {
                        clicked = Some(n);
                    }
                }
            });
        }
    });
    ui.add_space(20.0);
    clicked
}

/// 복사하기 쉬운 고정폭 명령 표시.
pub(crate) fn code_pill(ui: &mut Ui, text: &str) {
    let t = theme();
    egui::Frame::new()
        .fill(t.bg_input)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(format!("$ {text}")).font(fonts::mono(12.5)).color(t.text)).selectable(true));
        });
}

/// `#rrggbb` 또는 `rrggbb` 를 색으로 바꾼다. 해석할 수 없으면 흐린 글자색.
pub(crate) fn hex_color(hex: &str) -> Color32 {
    let h = hex.trim_start_matches('#');
    Color32::from_hex(&format!("#{h}")).unwrap_or(theme().text_dim)
}

/// 라벨 칩 색: (바탕, 테두리, 글자).
fn chip_colors(c: Color32) -> (Color32, Color32, Color32) {
    let t = theme();
    if t.dark {
        // 어두운 라벨색은 글자를 더 밝게 섞어 대비를 맞춘다.
        let luma = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        let k = if luma < 90.0 { 0.62 } else { 0.45 };
        (alpha(c, if luma < 90.0 { 0.28 } else { 0.18 }), alpha(c, 0.6), lerp_color(c, Color32::WHITE, k))
    } else {
        (alpha(c, 0.16), alpha(c, 0.55), lerp_color(c, Color32::BLACK, 0.45))
    }
}

/// 라벨 칩(레이아웃에 자리를 차지한다).
pub(crate) fn label_chip(ui: &mut Ui, name: &str, color: Color32) -> Response {
    let g = ui.painter().layout_no_wrap(name.to_string(), fonts::medium(11.0), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 19.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let (bg, stroke, fg) = chip_colors(color);
        ui.painter().rect(rect, CornerRadius::same(10), bg, Stroke::new(1.0, stroke), StrokeKind::Inside);
        ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, fg);
    }
    resp
}

/// 라벨 칩을 `left_center` 에서 오른쪽으로 그린다. `max_right` 를 넘으면 그리지 않고 `None`.
pub(crate) fn paint_label_chip(p: &egui::Painter, left_center: Pos2, name: &str, color: Color32, max_right: f32) -> Option<f32> {
    let g = p.layout_no_wrap(name.to_string(), fonts::medium(10.5), Color32::WHITE);
    let w = g.size().x + 12.0;
    if left_center.x + w > max_right {
        return None;
    }
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 8.5), vec2(w, 17.0));
    let (bg, stroke, fg) = chip_colors(color);
    p.rect(rect, CornerRadius::same(9), bg, Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.galley_with_override_text_color(rect.center() - g.size() / 2.0, g, fg);
    Some(rect.right())
}

/// 이니셜 원형 아바타를 그린다.
pub(crate) fn paint_avatar(p: &egui::Painter, center: Pos2, r: f32, login: &str) {
    let t = theme();
    let c = kiln_common::widgets::hue_color(login);
    p.circle_filled(center, r, alpha(c, if t.dark { 0.30 } else { 0.24 }));
    let initial = login.chars().next().unwrap_or('?').to_uppercase().to_string();
    p.text(center, Align2::CENTER_CENTER, initial, fonts::semibold(r * 1.05), if t.dark { lerp_color(c, Color32::WHITE, 0.55) } else { lerp_color(c, Color32::BLACK, 0.5) });
}

/// 아바타 + 로그인 이름 한 줄.
pub(crate) fn person_row(ui: &mut Ui, login: &str) {
    let t = theme();
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        paint_avatar(ui.painter(), r.center(), 10.0, login);
        ui.label(RichText::new(login).font(fonts::medium(12.5)).color(t.text));
    });
}

/// 팝업 공통 프레임.
pub(crate) fn popup_frame() -> egui::Frame {
    let t = theme();
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(6))
        .shadow(t.shadow())
}

/// 다중 선택 팝업의 한 항목.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickOption {
    pub value: String,
    pub color: Option<Color32>,
    pub detail: String,
}

/// 다중 선택 팝업 입력 상태.
#[derive(Default)]
pub(crate) struct PickerState {
    pub filter: String,
    pub focus: bool,
}

/// 검색 가능한 다중 선택 팝업. `anchor` 아래에 열리고, 항목을 누르면 `selected` 에 넣거나 뺀다.
/// 선택이 바뀐 프레임에 `true`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn multi_pick_popup(
    ui: &Ui,
    popup_id: Id,
    anchor: Rect,
    st: &mut PickerState,
    hint: &str,
    options: &[PickOption],
    selected: &mut Vec<String>,
    loading: bool,
    error: Option<&str>,
) -> bool {
    let t = theme();
    let mut changed = false;
    let width = anchor.width().max(280.0);
    egui::Popup::new(popup_id, ui.ctx().clone(), anchor, ui.layer_id())
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(width)
        .frame(popup_frame())
        .show(|ui| {
            ui.set_width(width - 12.0);
            let focused = ui.memory(|m| m.focused()).is_some();
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.filter)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .frame(kiln_common::widgets::input_frame(focused, false)),
            );
            if st.focus {
                r.request_focus();
                st.focus = false;
            }
            ui.add_space(4.0);
            if loading && options.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    spinner(ui, 12.0);
                    ui.label(dim("불러오는 중…"));
                });
            }
            if let Some(e) = error {
                ui.add(egui::Label::new(RichText::new(e).color(t.red).size(12.0)).wrap());
            }
            let q = st.filter.trim().to_lowercase();
            egui::ScrollArea::vertical().max_height(300.0).min_scrolled_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut shown = 0;
                for o in options.iter().filter(|o| q.is_empty() || o.value.to_lowercase().contains(&q) || o.detail.to_lowercase().contains(&q)) {
                    shown += 1;
                    let sel = selected.contains(&o.value);
                    if pick_row(ui, o, sel).clicked() {
                        if sel {
                            selected.retain(|v| v != &o.value);
                        } else {
                            selected.push(o.value.clone());
                        }
                        changed = true;
                    }
                }
                if shown == 0 && !loading {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(faint("일치하는 항목이 없습니다"));
                    });
                    ui.add_space(6.0);
                }
            });
        });
    changed
}

fn pick_row(ui: &mut Ui, o: &PickOption, sel: bool) -> Response {
    let t = theme();
    let w = ui.available_width();
    let h = if o.detail.is_empty() { 30.0 } else { 40.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, sel, &o.value));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    kiln_common::widgets::paint_row(ui.painter(), rect, false, resp.hovered());
    let p = ui.painter();
    let box_r = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(14.0, 14.0));
    p.rect(
        box_r,
        CornerRadius::same(4),
        if sel { t.accent } else { t.bg_input },
        Stroke::new(1.0, if sel { t.accent } else { t.border_strong }),
        StrokeKind::Inside,
    );
    if sel {
        paint_icon(p, box_r.shrink(1.5), Icon::Check, t.accent_fg);
    }
    let mut x = rect.left() + 32.0;
    match o.color {
        Some(c) => {
            p.circle_filled(pos2(x + 5.0, rect.center().y - if o.detail.is_empty() { 0.0 } else { 7.0 }), 5.0, c);
            x += 16.0;
        }
        None => {
            paint_avatar(p, pos2(x + 9.0, rect.center().y), 9.0, &o.value);
            x += 24.0;
        }
    }
    let max_w = rect.right() - x - 8.0;
    if o.detail.is_empty() {
        let g = p.layout_job(super::panel::one_line_job(&[(&o.value, 13.0, t.text)], max_w));
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, t.text);
    } else {
        let g = p.layout_job(super::panel::one_line_job(&[(&o.value, 13.0, t.text)], max_w));
        p.galley(pos2(x, rect.top() + 5.0), g, t.text);
        let g = p.layout_job(super::panel::one_line_job(&[(&o.detail, 11.0, t.text_faint)], max_w));
        p.galley(pos2(x, rect.top() + 23.0), g, t.text_faint);
    }
    resp
}

/// 폭을 채우는 탭 막대. 각 탭은 (값, 아이콘, 이름, 개수). 선택이 바뀌면 `true`.
pub(crate) fn count_tabs<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, tabs: &[(T, Icon, &str, Option<u64>)]) -> bool {
    let t = theme();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(8), t.bg_input, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let n = tabs.len().max(1) as f32;
    let w = (rect.width() - 4.0) / n;
    let mut changed = false;
    for (i, (v, icon, label, count)) in tabs.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + w * i as f32, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, ui.id().with(("count_tab", *label)), Sense::click());
        let sel = *value == *v;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, sel, *label));
        let p = ui.painter();
        if sel {
            p.rect(r, CornerRadius::same(6), t.bg_selected, Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
        } else if resp.hovered() {
            p.rect_filled(r, CornerRadius::same(6), t.bg_hover);
        }
        let fg = if sel { t.text } else { t.text_dim };
        let g = p.layout_no_wrap(label.to_string(), fonts::medium(12.5), fg);
        let cg = count.map(|c| p.layout_no_wrap(compact_count(c), fonts::medium(10.5), if sel { t.accent } else { t.text_dim }));
        let cw = cg.as_ref().map(|g| (g.size().x + 10.0).max(18.0) + 5.0).unwrap_or(0.0);
        let total = 14.0 + 6.0 + g.size().x + cw;
        let show_icon = total + 8.0 < r.width();
        let mut x = r.center().x - if show_icon { total } else { total - 20.0 } / 2.0;
        if show_icon {
            paint_icon(p, Rect::from_center_size(pos2(x + 7.0, r.center().y), vec2(14.0, 14.0)), *icon, if sel { t.accent } else { t.text_faint });
            x += 20.0;
        }
        let gw = g.size().x;
        p.galley(pos2(x, r.center().y - g.size().y / 2.0), g, fg);
        x += gw + 5.0;
        if let Some(cg) = cg {
            let pr = Rect::from_min_size(pos2(x, r.center().y - 8.0), vec2((cg.size().x + 10.0).max(18.0), 16.0));
            p.rect_filled(pr, CornerRadius::same(8), if sel { alpha(t.accent, 0.18) } else { t.bg_hover });
            p.galley(pr.center() - cg.size() / 2.0, cg, fg);
        }
        if resp.clicked() && !sel {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// 큰 숫자를 짧게(`1.2k`).
pub(crate) fn compact_count(n: u64) -> String {
    if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// 드롭다운처럼 보이는 필터 버튼("브랜치: 전체 ▾").
pub(crate) fn filter_button(ui: &mut Ui, label: &str, value: &str, active: bool) -> Response {
    let t = theme();
    let lg = ui.painter().layout_no_wrap(format!("{label} "), fonts::regular(12.0), t.text_faint);
    let vg = ui.painter().layout_no_wrap(value.to_string(), fonts::medium(12.0), t.text);
    let size = vec2(lg.size().x + vg.size().x + 34.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, format!("{label} {value}")));
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { t.bg_hover } else { t.bg_elevated };
        let stroke = if active { alpha(t.accent, 0.6) } else { t.border };
        ui.painter().rect(rect, CornerRadius::same(7), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let y = rect.center().y;
        let lw = lg.size().x;
        ui.painter().galley(pos2(rect.left() + 10.0, y - lg.size().y / 2.0), lg, t.text_faint);
        ui.painter().galley_with_override_text_color(pos2(rect.left() + 10.0 + lw, y - vg.size().y / 2.0), vg, if active { t.accent } else { t.text });
        paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.right() - 12.0, y), vec2(11.0, 11.0)), Icon::ChevronDown, t.text_faint);
    }
    resp
}

/// 한 줄 선택 목록 팝업(단일 선택). 고른 값을 돌려준다(`None` 항목은 빈 문자열).
pub(crate) fn single_pick_popup(ui: &Ui, popup_id: Id, anchor: Rect, options: &[(Option<String>, String)], current: &Option<String>) -> Option<Option<String>> {
    let t = theme();
    let mut picked = None;
    let width = anchor.width().max(220.0);
    egui::Popup::new(popup_id, ui.ctx().clone(), anchor, ui.layer_id())
        .open_memory(None)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(width)
        .frame(popup_frame())
        .show(|ui| {
            ui.set_width(width - 12.0);
            egui::ScrollArea::vertical().max_height(300.0).min_scrolled_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (v, label) in options {
                    let sel = v == current;
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, label));
                    kiln_common::widgets::paint_row(ui.painter(), rect, false, resp.hovered());
                    if sel {
                        paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(12.0, 12.0)), Icon::Check, t.accent);
                    }
                    let g = ui.painter().layout_job(super::panel::one_line_job(&[(label, 12.5, if sel { t.text } else { t.text_dim })], w - 36.0));
                    ui.painter().galley(pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, t.text);
                    if resp.clicked() {
                        picked = Some(v.clone());
                    }
                }
            });
        });
    if picked.is_some() {
        egui::Popup::close_id(ui.ctx(), popup_id);
    }
    picked
}

/// 섹션 제목 + 오른쪽 편집 아이콘(사이드바용). 편집 버튼 응답을 돌려준다.
pub(crate) fn sidebar_heading(ui: &mut Ui, title: &str, editable: bool) -> Option<Response> {
    let t = theme();
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(fonts::semibold(12.0)).color(t.text_dim));
        if editable {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                out = Some(icon_button(ui, Icon::Plus, &format!("{title} 편집")));
            });
        }
    });
    out
}

/// 상태 색으로 채운 알약(아이콘+이름). 이슈 상태 표시용.
pub(crate) fn state_pill(ui: &mut Ui, icon: Icon, label: &str, color: Color32) -> Response {
    let t = theme();
    let g = ui.painter().layout_no_wrap(label.to_string(), fonts::semibold(12.5), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 38.0, 26.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let fill = if t.dark { alpha(color, 0.22) } else { alpha(color, 0.14) };
        ui.painter().rect(rect, CornerRadius::same(13), fill, Stroke::new(1.0, alpha(color, 0.5)), StrokeKind::Inside);
        paint_icon(ui.painter(), Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), vec2(14.0, 14.0)), icon, color);
        let fg = if t.dark { lerp_color(color, Color32::WHITE, 0.25) } else { lerp_color(color, Color32::BLACK, 0.25) };
        ui.painter().galley_with_override_text_color(pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}

/// 이슈 상태 아이콘과 색.
pub(crate) fn issue_state_style(s: crate::github::IssueState) -> (Icon, Color32) {
    let t = theme();
    match s {
        crate::github::IssueState::Open => (Icon::Issue, t.green),
        crate::github::IssueState::Completed => (Icon::IssueClosed, t.purple),
        crate::github::IssueState::NotPlanned => (Icon::Skip, t.text_dim),
    }
}

/// 실행 상태 아이콘과 색.
pub(crate) fn run_status_style(s: crate::github::RunStatus) -> (Icon, Color32) {
    use crate::github::RunStatus;
    let t = theme();
    match s {
        RunStatus::Success => (Icon::CheckCircle, t.green),
        RunStatus::Failure => (Icon::XCircle, t.red),
        RunStatus::InProgress => (Icon::PendingCircle, t.yellow),
        RunStatus::Queued => (Icon::PendingCircle, t.text_faint),
        RunStatus::Cancelled => (Icon::Skip, t.text_dim),
        RunStatus::Skipped => (Icon::Skip, t.text_faint),
    }
}

/// 라벨 목록을 선택 팝업 항목으로 바꾼다.
pub(crate) fn label_options(labels: &[crate::github::Label]) -> Vec<PickOption> {
    labels.iter().map(|l| PickOption { value: l.name.clone(), color: Some(hex_color(&l.color)), detail: l.description.clone() }).collect()
}

/// 사용자 목록을 선택 팝업 항목으로 바꾼다.
pub(crate) fn user_options(users: &[String]) -> Vec<PickOption> {
    users.iter().map(|u| PickOption { value: u.clone(), color: None, detail: String::new() }).collect()
}
