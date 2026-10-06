mod common;
use egui::{Event,Key,Modifiers};
use egui_kittest::{Harness,kittest::Queryable};
use kiln_editor::{Editor,Pos,Selection};

#[test]
fn local_completion_language_override_ime_and_narrow_bounds(){
    let mut h=Harness::builder().with_size(egui::vec2(420.0,300.0)).wgpu().build_ui_state(|ui,ed:&mut Editor|{
        if common::fonts_ready(ui.ctx()){ed.ui(ui);}
    },Editor::from_text("script.py","result_document_word\nret"));
    common::apply_theme(&h.ctx);h.run_steps(5);
    h.state_mut().goto(2,4);let id=h.state().id();h.ctx.memory_mut(|m|m.request_focus(id));h.run_steps(2);
    h.key_press_modifiers(Modifiers::CTRL,Key::Space);h.run_steps(3);
    let item=h.get_by_label("return · 언어 키워드");assert!(h.ctx.content_rect().contains_rect(item.rect()));
    h.render().unwrap().save("/tmp/kiln-editor-completion-narrow.png").unwrap();
    h.key_press(Key::Enter);h.run_steps(3);assert_eq!(h.state().text(),"result_document_word\nreturn");
    h.key_press_modifiers(Modifiers::COMMAND,Key::Z);h.run_steps(3);assert_eq!(h.state().text(),"result_document_word\nret");
    h.key_press_modifiers(Modifiers::ALT,Key::Escape);h.run_steps(3);assert!(h.query_by_label("return · 언어 키워드").is_some());
    h.event(Event::Ime(egui::ImeEvent::Preedit{text:"한".into(),active_range_chars:None}));h.run_steps(2);assert!(h.query_by_label("return · 언어 키워드").is_none());
    h.event(Event::Ime(egui::ImeEvent::Commit("한".into())));h.run_steps(3);assert!(h.state().text().ends_with("ret한"));
    h.state_mut().set_language_override(Some("JavaScript".into()));h.run_steps(3);assert_eq!(h.state().status().language,"JavaScript");
    h.state_mut().set_language_override(None);h.run_steps(3);assert_eq!(h.state().status().language,"Python");
    h.ctx.set_zoom_factor(1.3);h.run_steps(3);h.render().unwrap().save("/tmp/kiln-editor-language-130.png").unwrap();
}
#[test]
fn short_completion_popup_keeps_keyboard_selection_visible_before_accepting(){
    let text="alpha0 alpha1 alpha2 alpha3 alpha4 alpha5 alpha6 alpha7 alpha8 alpha9\nal";
    let mut h=Harness::builder().with_size(egui::vec2(420.0,180.0)).wgpu().build_ui_state(|ui,ed:&mut Editor|{
        if common::fonts_ready(ui.ctx()){ed.ui(ui);}
    },Editor::from_text("words.txt",text));
    common::apply_theme(&h.ctx);h.run_steps(5);
    h.state_mut().goto(2,3);let id=h.state().id();h.ctx.memory_mut(|m|m.request_focus(id));h.run_steps(2);
    h.key_press_modifiers(Modifiers::CTRL,Key::Space);h.run_steps(3);
    for selected in 0..10 {
        if selected>0 {h.key_press(Key::ArrowDown);h.run_steps(2);}
        let label=format!("alpha{selected} · 문서의 단어");
        let candidate=h.get_by_label(&label);
        assert!(h.ctx.content_rect().contains_rect(candidate.rect()),"candidate {selected} outside viewport");
    }
    h.render().unwrap().save("/tmp/kiln-editor-completion-short.png").unwrap();
    h.key_press(Key::Enter);h.run_steps(3);assert!(h.state().text().ends_with("\nalpha9"));
    h.key_press_modifiers(Modifiers::COMMAND,Key::Z);h.run_steps(3);assert_eq!(h.state().text(),text);
    h.key_press_modifiers(Modifiers::CTRL,Key::Space);h.run_steps(3);
    let at=h.get_by_label("alpha0 · 문서의 단어").rect().center();
    h.event(Event::PointerMoved(at));
    h.event(Event::MouseWheel{phase:egui::TouchPhase::Move,unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.0,-90.0),modifiers:Modifiers::NONE});
    h.run_steps(5);
    h.event(Event::PointerGone);h.run_steps(2);
    assert!(h.query_by_label("alpha0 · 문서의 단어").is_none(),"wheel must not snap back to the old selection");
    let visible:Vec<_>=(0..10).filter(|i|h.query_by_label(&format!("alpha{i} · 문서의 단어")).is_some()).collect();
    assert!(!visible.is_empty());
    h.key_press(Key::Enter);h.run_steps(3);
    assert!(visible.iter().any(|i|h.state().text().ends_with(&format!("\nalpha{i}"))),"wheel selection must accept a visible candidate");
}
#[test]
fn reload_and_edited_shebang_share_detection_and_override(){
    let dir=tempfile::tempdir().unwrap();let file=dir.path().join("script");std::fs::write(&file,"#!/usr/bin/env python3\nprint(1)").unwrap();
    let mut e=Editor::open(&file).unwrap();assert_eq!(e.status().language,"Python");
    std::fs::write(&file,"#!/usr/bin/env ruby\nputs 1").unwrap();e.reload_from_disk().unwrap();assert_eq!(e.status().language,"Ruby");
    e.set_language_override(Some("Plain Text".into()));assert_eq!(e.status().language,"Plain Text");
    e.set_language_override(None);assert_eq!(e.status().language,"Ruby");
    e.set_selection(Selection::caret(Pos::new(0,0)));
}
