//! 패널·탭이 함께 쓰는 UI 부품.

pub(crate) mod grid;
pub(crate) mod highlight;

use crate::ConnStatus;
use egui::{Color32, FontId, Pos2, Response, RichText, Sense, Ui, vec2};
use kiln_common::Theme;

/// 연결 상태 점 색.
pub(crate) fn status_color(st: &ConnStatus) -> Color32 {
    let t = Theme::current();
    match st {
        ConnStatus::Disconnected => t.text_faint,
        ConnStatus::Connecting => t.yellow,
        ConnStatus::Connected => t.green,
        ConnStatus::Failed(_) => t.red,
    }
}

pub(crate) fn paint_dot(ui: &Ui, center: Pos2, color: Color32) {
    ui.painter().circle_filled(center, 3.5, color);
}

/// 작은 아이콘 버튼(프레임 없음).
pub(crate) fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    let t = Theme::current();
    ui.add(
        egui::Button::new(RichText::new(icon).size(13.0).color(t.text_dim))
            .frame(false)
            .min_size(vec2(22.0, 20.0)),
    )
    .on_hover_text(tip)
}

/// 툴바 버튼. `primary` 면 강조색.
pub(crate) fn tool_button(ui: &mut Ui, label: &str, enabled: bool, primary: bool) -> Response {
    let t = Theme::current();
    let text = RichText::new(label).size(12.0);
    let btn = if primary && enabled {
        egui::Button::new(text.color(Color32::WHITE)).fill(t.accent.gamma_multiply(0.85))
    } else {
        egui::Button::new(text).fill(t.bg_elevated)
    };
    ui.add_enabled(enabled, btn.corner_radius(4.0).min_size(vec2(0.0, 22.0)))
}

/// 켜고 끄는 툴바 버튼.
pub(crate) fn toggle_button(ui: &mut Ui, label: &str, on: bool) -> Response {
    let t = Theme::current();
    let text = RichText::new(label)
        .size(12.0)
        .color(if on { t.accent } else { t.text });
    ui.add(
        egui::Button::new(text)
            .fill(if on {
                t.accent.gamma_multiply(0.18)
            } else {
                t.bg_elevated
            })
            .stroke(egui::Stroke::new(
                1.0,
                if on {
                    t.accent.gamma_multiply(0.6)
                } else {
                    Color32::TRANSPARENT
                },
            ))
            .corner_radius(4.0)
            .min_size(vec2(0.0, 22.0)),
    )
}

/// 회전하는 로딩 표시.
pub(crate) fn spinner(ui: &mut Ui) {
    ui.add(egui::Spinner::new().size(12.0));
}

/// 흐린 보조 텍스트.
pub(crate) fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into())
        .color(Theme::current().text_dim)
        .size(11.5)
}

/// 알림 줄(오류/정보).
pub(crate) fn banner(ui: &mut Ui, text: &str, error: bool) {
    let t = Theme::current();
    let color = if error { t.red } else { t.blue };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(if error { "⚠" } else { "ℹ" }).color(color));
                ui.label(RichText::new(text).color(t.text).size(12.0));
            });
        });
}

/// 입력한 이름이 일치해야 확인되는 위험 작업 대화상자.
pub(crate) struct TypedConfirm {
    pub title: String,
    pub message: String,
    pub expected: String,
    pub input: String,
    pub action_label: String,
}

impl TypedConfirm {
    /// 모달을 그린다. `Some(true)` 확인, `Some(false)` 취소.
    pub fn show(&mut self, ctx: &egui::Context, id: egui::Id) -> Option<bool> {
        let t = Theme::current();
        let mut result = None;
        let resp = egui::Modal::new(id).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(&self.title).size(15.0).strong());
            ui.add_space(6.0);
            ui.label(&self.message);
            ui.add_space(6.0);
            ui.label(dim(format!("Type \"{}\" to confirm:", self.expected)));
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .desired_width(f32::INFINITY)
                    .hint_text(self.expected.as_str()),
            );
            r.request_focus();
            ui.add_space(8.0);
            let ok = self.input == self.expected;
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let danger =
                        egui::Button::new(RichText::new(&self.action_label).color(if ok {
                            Color32::WHITE
                        } else {
                            t.text_faint
                        }))
                        .fill(if ok {
                            t.red.gamma_multiply(0.85)
                        } else {
                            t.bg_elevated
                        });
                    if ui.add_enabled(ok, danger).clicked()
                        || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        result = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        result = Some(false);
                    }
                });
            });
        });
        if resp.should_close() && result.is_none() {
            result = Some(false);
        }
        result
    }
}

/// 트리 한 줄을 할당한다(클릭·더블클릭·우클릭 감지).
pub(crate) fn tree_row(
    ui: &mut Ui,
    height: f32,
    selected: bool,
    label: &str,
) -> (egui::Rect, Response) {
    let t = Theme::current();
    let w = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if selected {
        ui.painter().rect_filled(rect, 3.0, t.bg_selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, t.bg_hover);
    }
    (rect, resp)
}

/// 위젯 위에서 기본 버튼 더블클릭이 끝났는지.
pub(crate) fn double_clicked(ui: &Ui, resp: &Response) -> bool {
    resp.hovered()
        && ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
        })
}

/// 펼침 화살표.
pub(crate) fn chevron(ui: &Ui, pos: Pos2, open: bool, visible: bool) {
    if !visible {
        return;
    }
    let t = Theme::current();
    ui.painter().text(
        pos,
        egui::Align2::CENTER_CENTER,
        if open { "⏷" } else { "⏵" },
        FontId::proportional(9.0),
        t.text_faint,
    );
}

/// 숫자에 천 단위 구분자를 넣는다.
pub(crate) fn thousands(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}
