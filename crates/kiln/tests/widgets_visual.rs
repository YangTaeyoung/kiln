//! Focused visual evidence for common control state colors, independent of app sessions.
use egui_kittest::Harness;
use kiln_common::{Theme,fonts,widgets,icons::Icon};
#[test]
fn control_states_across_four_themes() {
    for theme in Theme::ALL {
        Theme::set_current(theme.name);
        let mut initialized=false;
        let mut h=Harness::builder().with_size([740.0,450.0]).build_ui(move |ui| {
            if !initialized {fonts::install(ui.ctx());theme.apply(ui.ctx());initialized=true;return;}
            ui.heading(theme.label);
            ui.label("기본 · 선택 · 비활성 · 키보드 포커스");
            ui.horizontal(|ui| {
                widgets::button(ui,"주요 동작",widgets::ButtonKind::Primary);
                widgets::button(ui,"삭제",widgets::ButtonKind::Danger);
                widgets::button(ui,"보조 동작",widgets::ButtonKind::Secondary);
                let _=ui.selectable_label(true,"선택한 항목");
            });
            ui.horizontal(|ui| {
                let mut off=false;let mut on=true;
                widgets::toggle(ui,&mut off);widgets::toggle(ui,&mut on);
                ui.checkbox(&mut on,"선택됨");
                let r=widgets::icon_button(ui,Icon::Folder,30.0,false,"파일 — 키보드 포커스");r.request_focus();
            });
            ui.add_enabled_ui(false,|ui| {ui.horizontal(|ui| {
                widgets::button(ui,"주요 동작",widgets::ButtonKind::Primary);
                widgets::button(ui,"삭제",widgets::ButtonKind::Danger);
                widgets::button(ui,"보조 동작",widgets::ButtonKind::Secondary);
                widgets::icon_button(ui,Icon::Folder,30.0,false,"비활성 파일");
            });});
            for (name,bg) in [("기본 배경",theme.bg),("선택 배경",theme.bg_selected)] {
                egui::Frame::new().fill(bg).inner_margin(12).show(ui,|ui| {
                    ui.label(name);
                    ui.horizontal_wrapped(|ui| {for (text,color) in [("성공",theme.green),("오류",theme.red),("경고",theme.yellow),("정보",theme.blue),("검토",theme.purple),("입력 필요",theme.orange),("링크",theme.accent)] {ui.colored_label(color,text);}});
                    ui.label(egui::RichText::new("본문 · 부가 정보 · 경로와 시간").color(theme.text_faint));
                });
            }
        });
        h.run_steps(4);
        h.render().unwrap().save(format!("/tmp/kiln-controls-{}.png",theme.name)).unwrap();
    }
}
