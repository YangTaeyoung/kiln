//! 셀 값 뷰어 사이드 패널: 긴 텍스트, JSON(정렬 출력), 바이트(16진 덤프).

use crate::ui::{self, faint};
use crate::value::hex_dump;
use crate::{TypeClass, Value};
use egui::{RichText, Ui};
use kiln_common::icons::Icon;
use kiln_common::widgets::{self, ButtonKind};
use kiln_common::{Theme, fonts};

#[derive(Default)]
pub(crate) struct ValueViewer {
    key: Option<(usize, usize, u64)>,
    text: String,
    dirty: bool,
}

/// 뷰어에 보여줄 셀 정보.
pub(crate) struct ViewerCell<'a> {
    pub row: usize,
    pub col: usize,
    pub name: &'a str,
    pub type_name: &'a str,
    pub class: TypeClass,
    /// `None` 은 DEFAULT(새 행).
    pub value: Option<&'a Value>,
    pub editable: bool,
}

/// 값 식별용 지문: 주소, 길이, 앞부분 바이트.
fn fingerprint(v: Option<&Value>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match v {
        None => 0u8.hash(&mut h),
        Some(v) => {
            (v as *const Value as usize).hash(&mut h);
            match v {
                Value::Bytes(b) => {
                    b.len().hash(&mut h);
                    b[..b.len().min(64)].hash(&mut h);
                }
                Value::Text(s)
                | Value::Json(s)
                | Value::Other(s)
                | Value::Array(s)
                | Value::Decimal(s) => {
                    s.len().hash(&mut h);
                    s.as_bytes()[..s.len().min(64)].hash(&mut h);
                }
                other => format!("{other:?}").hash(&mut h),
            }
        }
    }
    h.finish()
}

fn render_text(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(Value::Null) => String::new(),
        Some(Value::Json(s)) => serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|j| serde_json::to_string_pretty(&j).ok())
            .unwrap_or_else(|| s.clone()),
        Some(Value::Bytes(b)) => hex_dump(b, 64 * 1024),
        Some(other) => other.to_text().unwrap_or_default(),
    }
}

impl ValueViewer {
    /// 뷰어를 그린다. 사용자가 적용을 누르면 새 텍스트를 돌려준다.
    pub fn ui(&mut self, ui: &mut Ui, cell: Option<ViewerCell<'_>>) -> Option<String> {
        let theme = Theme::current();
        let Some(cell) = cell else {
            widgets::empty_state(ui, Icon::Table, kiln_common::i18n::tr("값을 보려면 셀을 선택하세요"), None);
            return None;
        };
        let fp = fingerprint(cell.value);
        let key = (cell.row, cell.col, fp);
        if self.key != Some(key) {
            self.key = Some(key);
            self.text = render_text(cell.value);
            self.dirty = false;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new(cell.name).font(fonts::semibold(13.5)).color(theme.text));
            if !cell.type_name.is_empty() {
                widgets::pill(ui, &cell.type_name.to_ascii_lowercase(), theme.purple);
            }
        });
        let meta = match cell.value {
            None => "DEFAULT".to_string(),
            Some(Value::Null) => "NULL".to_string(),
            Some(Value::Bytes(b)) => kiln_common::trf!("{}바이트", b.len()),
            Some(v) => {
                let t = v.to_text().unwrap_or_default();
                kiln_common::trf!("{}자", t.chars().count())
            }
        };
        ui.label(faint(kiln_common::trf!("{}행 · {meta}", cell.row + 1)));
        ui.add_space(8.0);
        let is_bytes = matches!(cell.value, Some(Value::Bytes(_)));
        let can_edit = cell.editable && !is_bytes;
        let mut apply = None;
        if can_edit {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if ui
                    .add_enabled_ui(self.dirty, |ui| widgets::button_with(ui, None, kiln_common::i18n::tr("셀에 반영"), ButtonKind::Primary, true))
                    .inner
                    .on_hover_text(kiln_common::i18n::tr("DB에 제출하기 전까지 임시 변경으로 유지됩니다"))
                    .clicked()
                {
                    let text = if cell.class == TypeClass::Json {
                        serde_json::from_str::<serde_json::Value>(&self.text)
                            .map(|j| j.to_string())
                            .unwrap_or_else(|_| self.text.clone())
                    } else {
                        self.text.clone()
                    };
                    apply = Some(text);
                    self.dirty = false;
                }
                if ui::secondary_button(ui, None, kiln_common::i18n::tr("초기화"), self.dirty).clicked()
                {
                    self.text = render_text(cell.value);
                    self.dirty = false;
                }
            });
            ui.add_space(8.0);
        }
        let editing = ui.memory(|m| m.has_focus(egui::Id::new("value-viewer-text")));
        widgets::input_frame(editing, false)
            .inner_margin(egui::Margin::same(2))
            .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::both()
            .id_salt("value-viewer")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if matches!(cell.value, Some(Value::Null) | None) && !can_edit {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(if cell.value.is_none() { "<default>" } else { "<null>" })
                                .font(fonts::mono(12.5))
                                .italics()
                                .color(theme.text_faint),
                        );
                    });
                    return;
                }
                let font = egui::TextStyle::Monospace;
                if can_edit {
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut self.text)
                            .id(egui::Id::new("value-viewer-text"))
                            .font(font)
                            .code_editor()
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)))
                            .desired_width(f32::INFINITY)
                            .desired_rows(12),
                    );
                    if r.changed() {
                        self.dirty = true;
                    }
                } else {
                    let mut s = self.text.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut s)
                            .font(font)
                            .code_editor()
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)))
                            .desired_width(f32::INFINITY),
                    );
                }
            });
            });
        apply
    }
}
