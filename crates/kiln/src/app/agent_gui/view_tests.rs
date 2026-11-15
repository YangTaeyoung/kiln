use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
fn pane() -> AgentPane {
    AgentPane::fixture(Descriptor {
        id: "fixture".into(),
        tool: kiln_accounts::Tool::Codex,
        cwd: "/fixture".into(),
        account_label: "Pinned test account".into(),
    })
}
#[test]
fn gui_conversation_stream_approval_stop_and_attachment_at_520px() {
    assert_eq!(Presentation::default(), Presentation::Gui);
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "한글 task".into();
    p.composer.attachments.push(Attachment {
        path: "/private/image".into(),
        name: "image.png".into(),
        mime: "image/png".into(),
        sha256: "fixture".into(),
    });
    let mut initialized = false;
    let mut h = Harness::builder().with_size([520., 760.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.ui(ui);
        },
        p,
    );
    h.run();
    h.get_by_label("보내기").click();
    h.run();
    assert!(
        matches!(&h.state().sent[0],Command::Submit{text,attachments,..} if text=="한글 task" && attachments.len()==1)
    );
    let p = h.state_mut();
    p.apply(&Envelope {
        sequence: 1,
        generation: "g".into(),
        event: Event::Generation,
    });
    p.apply(&Envelope {
        sequence: 2,
        generation: "g".into(),
        event: Event::Message {
            message: Message {
                id: "reply".into(),
                role: Role::Assistant,
                text: "Streamed **reply**".into(),
                attachments: vec![],
            },
            append: false,
        },
    });
    p.apply(&Envelope {
        sequence: 3,
        generation: "g".into(),
        event: Event::Tool(ToolCall {
            id: "tool".into(),
            name: "Read".into(),
            status: ToolStatus::Running,
            ..Default::default()
        }),
    });
    p.apply(&Envelope {
        sequence: 4,
        generation: "g".into(),
        event: Event::Request(Request {
            id: "ask".into(),
            title: "Permission required".into(),
            questions: vec![],
            options: vec![],
            details: serde_json::Value::Null,
        }),
    });
    p.transcript.lifecycle = Lifecycle::Waiting;
    h.run();
    h.get_by_label("한 번 허용").click();
    h.run();
    assert!(h.state().sent.iter().any(|c|matches!(c,Command::Respond{request_id,decision:Decision::Allow,..} if request_id=="ask")));
    h.get_by_label("중지").click();
    h.run();
    assert!(matches!(h.state().sent.last(), Some(Command::Interrupt)));
}
#[test]
fn pending_draft_reconciles_without_resending_or_losing_unconfirmed_text() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "exact  \n한글".into();
    p.submit();
    let saved = p.draft();
    assert!(saved.pending.is_some());
    let mut restored = pane();
    restored.restore_draft(saved);
    assert!(restored.sent.is_empty());
    restored.transcript.lifecycle = Lifecycle::Failed;
    restored.reconcile();
    assert!(restored.pending_recoverable());
    assert_eq!(
        restored.draft().pending.unwrap().draft.text,
        "exact  \n한글"
    );
    let id = restored.pending.as_ref().unwrap().id.clone();
    restored
        .transcript
        .commands
        .insert(id, CommandPhase::Accepted);
    restored.reconcile();
    assert!(restored.pending.is_none());
    assert!(restored.composer.text.is_empty());
}
#[test]
fn ime_composition_across_frames_and_commit_never_submits() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    let mut initialized = false;
    let mut h = Harness::builder().with_size([520., 760.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.ui(ui);
        },
        p,
    );
    h.run();
    h.get_by_label("메시지").click();
    h.run();
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "ㅎ".into(),
        active_range_chars: None,
    }));
    h.run();
    assert!(h.state().composing);
    h.get_by_label("보내기").click();
    h.run();
    assert!(h.state().sent.is_empty());
    h.get_by_label("메시지").click();
    h.run();
    h.input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Commit("한글".into())));
    h.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert!(
        h.state().sent.is_empty(),
        "sent {:?}, draft {:?}, composing {}, ime {}",
        h.state().sent,
        h.state().composer,
        h.state().composing,
        h.state().ime_frame
    );
    assert!(h.state().composer.text.contains("한글"));
    h.get_by_label("보내기").click();
    h.run();
    assert_eq!(h.state().sent.len(), 1);
}
#[test]
fn dead_transport_disables_submit_even_when_journal_is_ready() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "keep".into();
    p.set_transport_alive(false);
    p.submit();
    assert!(p.sent.is_empty());
    assert_eq!(p.composer.text, "keep");
}

#[test]
fn rejected_and_user_journal_reconcile_pending_identity() {
    for phase in [
        CommandPhase::Queued,
        CommandPhase::Dispatched,
        CommandPhase::Unconfirmed,
        CommandPhase::Rejected,
    ] {
        let mut p = pane();
        p.transcript.lifecycle = Lifecycle::Ready;
        p.composer.text = "recover me".into();
        p.submit();
        let id = p.pending.as_ref().unwrap().id.clone();
        p.transcript.commands.insert(id.clone(), phase.clone());
        p.reconcile();
        assert!(p.pending.is_some());
        assert_eq!(
            p.pending_recoverable(),
            matches!(phase, CommandPhase::Rejected | CommandPhase::Unconfirmed)
        );
        p.transcript.entries.push(Entry::Message(Message {
            id: format!("user-{id}"),
            role: Role::User,
            text: "recover me".into(),
            attachments: vec![],
        }));
        p.reconcile();
        assert!(p.pending.is_none());
        assert_eq!(p.sent.len(), 1);
    }
}
#[test]
fn old_saved_terminal_and_settings_remain_compatible() {
    let settings: crate::app::state::Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.agent_presentation, Presentation::Gui);
    let pane: crate::app::state::PaneP =
        serde_json::from_str(r#"{"id":1,"session":3,"cwd":"/tmp"}"#).unwrap();
    assert!(pane.agent_draft.is_none());
    assert!(pane.launch.is_none());
    assert!(AgentPane::from_spec(&SpawnSpec::default()).is_none());
}
#[test]
fn dark_and_light_controls_fit_narrow_pane_and_keyboard_focus() {
    for dark in [true, false] {
        let p = pane();
        let mut initialized = false;
        let mut h = Harness::builder().with_size([520., 600.]).build_ui_state(
            |ui, p: &mut AgentPane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                kiln_common::Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                kiln_common::Theme::current().apply(ui.ctx());
                p.ui(ui);
            },
            p,
        );
        h.run();
        for label in ["메시지", "이미지 첨부", "보내기"] {
            assert!(
                h.ctx
                    .content_rect()
                    .contains_rect(h.get_by_label(label).rect()),
                "{label} must fit"
            );
        }
        h.key_press(egui::Key::Tab);
        h.run();
        assert!(h.get_by_label("이미지 첨부").accesskit_node().is_focused());
    }
}

#[test]
fn recovered_payload_keeps_original_identity_until_late_acceptance() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "exact request".into();
    p.submit();
    let submitted = p.pending.take().unwrap();
    p.composer = submitted.draft.clone();
    p.recovered = Some(submitted.clone());
    let saved = p.draft();
    let mut restored = pane();
    restored.restore_draft(saved);
    assert_eq!(restored.recovered.as_ref().unwrap().id, submitted.id);
    assert!(restored.sent.is_empty());
    restored
        .transcript
        .commands
        .insert(submitted.id, CommandPhase::Accepted);
    restored.reconcile();
    assert!(restored.composer.text.is_empty());
    assert!(restored.recovered.is_none());
}
#[test]
fn restored_missing_attachment_is_visible_and_blocks_send_until_removed() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "keep text".into();
    p.composer.attachments.push(Attachment {
        path: "/outside/image".into(),
        name: "missing.png".into(),
        mime: "image/png".into(),
        sha256: "bad".into(),
    });
    p.validate_attachments(&egui::Context::default());
    let result = p.validation.take().unwrap().wait().unwrap();
    assert!(result.is_err());
    p.attachment_error = result.err();
    p.submit();
    assert!(p.sent.is_empty());
    assert_eq!(p.composer.attachments.len(), 1);
    p.composer.attachments.clear();
    p.attachment_error = None;
    p.submit();
    assert_eq!(p.sent.len(), 1);
}
#[test]
fn selected_composer_model_is_separate_from_confirmed_effective_model() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "task".into();
    p.composer.model = Some("requested".into());
    p.submit();
    assert!(p.sent.is_empty());
    p.transcript.models = vec![Model {
        id: "requested".into(),
        label: "Advertised model".into(),
    }];
    p.transcript.selected_model = Some("confirmed".into());
    p.submit();
    assert!(matches!(p.sent.last(),Some(Command::Submit{model:Some(id),..}) if id=="requested"));
    assert_eq!(p.transcript.selected_model.as_deref(), Some("confirmed"));
}

#[test]
fn unverified_restore_is_starting_and_never_enables_send() {
    let mut p = AgentPane::new(pane().descriptor);
    p.composer.text = "safe draft".into();
    p.transcript.lifecycle = Lifecycle::Ready;
    assert_eq!(p.lifecycle(), Lifecycle::Starting);
    p.submit();
    assert!(p.sent.is_empty());
    p.set_transport_alive(true);
    assert_eq!(p.lifecycle(), Lifecycle::Ready);
    p.transport_unverified();
    assert_eq!(p.lifecycle(), Lifecycle::Disconnected);
}
#[test]
fn new_generation_clears_question_answers_and_locally_responding_ids() {
    let mut p = pane();
    p.answers.insert(
        "reused".into(),
        BTreeMap::from([("question".into(), vec!["old answer".into()])]),
    );
    p.responding.insert("reused".into(), "old command".into());
    p.apply(&Envelope {
        sequence: 1,
        generation: "new".into(),
        event: Event::Generation,
    });
    assert!(p.answers.is_empty());
    assert!(p.responding.is_empty());
}

#[test]
fn eight_attachment_chips_and_advertised_choices_leave_send_reachable_at_520px() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    for index in 0..8 {
        p.composer.attachments.push(Attachment {
            path: format!("/private/{index}").into(),
            name: format!("long-image-name-number-{index}.png"),
            mime: "image/png".into(),
            sha256: "fixture".into(),
        });
    }
    p.transcript.models = vec![Model {
        id: "model".into(),
        label: "Advertised model".into(),
    }];
    p.transcript.modes = vec![Choice {
        id: "mode".into(),
        label: "Advertised mode".into(),
        description: String::new(),
    }];
    let mut initialized = false;
    let mut h = Harness::builder().with_size([520., 600.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.ui(ui);
        },
        p,
    );
    h.run();
    assert!(
        h.ctx
            .content_rect()
            .contains_rect(h.get_by_label("보내기").rect())
    );
}
