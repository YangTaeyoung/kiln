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
fn final_fix_published_ids(root: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(root.join("commands"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .map(|entry| {
            super::super::journal::read::<super::super::QueuedCommand>(&entry.path())
                .unwrap()
                .id
        })
        .collect()
}
fn final_fix_wait(p: &mut AgentPane, ready: impl Fn(&AgentPane) -> bool) {
    let ctx = egui::Context::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        p.tick(&ctx);
        if ready(p) {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "final fix condition timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn final_fix_failed_open_conversation_does_not_gate_healthy_conversation() {
    use super::super::journal;
    let a_root = tempfile::tempdir().unwrap();
    let b_root = tempfile::tempdir().unwrap();
    let mut a_journal = journal::Journal::open(a_root.path(), "A".into()).unwrap();
    a_journal.push(Event::Generation).unwrap();
    let (a_client, start) = Client::paused(a_root.path().into(), "A");
    let id = a_client.send(Command::Interrupt).unwrap();
    let mut a = pane();
    a.client = Some(a_client);
    let mut b_journal = journal::Journal::open(b_root.path(), "B".into()).unwrap();
    b_journal.push(Event::Generation).unwrap();
    b_journal.push(Event::Lifecycle(Lifecycle::Ready)).unwrap();
    let mut b = pane();
    b.replay_required = true;
    b.client = Some(Client::at_directory(Ok(b_root.path().into())));
    // A still owns an unpublished local command, but B has no writer or backlog.
    final_fix_wait(&mut b, |p| !p.replay_required);
    start.send(()).unwrap();
    final_fix_wait(&mut a, |_| {
        final_fix_published_ids(a_root.path()).contains(&id)
    });
    journal::atomic(
        &a_root.path().join("failure.json"),
        br#"{"reason":"fixture failure"}"#,
    )
    .unwrap();
    final_fix_wait(&mut a, |p| {
        p.transcript.commands.get(&id) == Some(&CommandPhase::Rejected)
    });
    b.require_replay();
    final_fix_wait(&mut b, |p| !p.replay_required);
    assert!(!b.settings_blocked());
    b.composer.text = "healthy B".into();
    b.submit();
    let submitted = b
        .pending
        .as_ref()
        .expect("B can explicitly submit while failed A stays open")
        .id
        .clone();
    final_fix_wait(&mut b, |_| {
        final_fix_published_ids(b_root.path()).contains(&submitted)
    });
    assert!(a.client.as_ref().unwrap().send(Command::Interrupt).is_err());
    assert_eq!(
        a.transcript.commands.get(&id),
        Some(&CommandPhase::Rejected)
    );
}
#[test]
fn final_fix_uncertain_settings_recover_only_after_absence_or_native_acceptance() {
    use super::super::journal;
    let commands = [
        Command::Configure {
            control: "model".into(),
            value: ControlValue::Text("new".into()),
        },
        Command::SetMode {
            mode: "plan".into(),
        },
        Command::NativeAction(NativeAction::Compact),
    ];
    for command in commands {
        for (point, accepted) in [
            (journal::PublishPoint::RenameAttempt, false),
            (journal::PublishPoint::AfterRename, false),
            (journal::PublishPoint::AfterRename, true),
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut journal = journal::Journal::open(root.path(), "fixture".into()).unwrap();
            journal.push(Event::Generation).unwrap();
            journal.push(Event::Lifecycle(Lifecycle::Ready)).unwrap();
            let (client, start) = Client::faulted_enqueue(root.path().into(), "fixture", point);
            let mut p = pane();
            p.client = Some(client);
            start.send(()).unwrap();
            final_fix_wait(&mut p, |p| p.transcript.sequence == 2);
            let id = p.send(command.clone()).unwrap();
            final_fix_wait(&mut p, |p| {
                p.transcript.commands.get(&id) == Some(&CommandPhase::Unconfirmed)
            });
            assert!(p.error.is_some(), "publication failure remains visible");
            let file = root.path().join("commands").join(format!("{id}.json"));
            if point == journal::PublishPoint::RenameAttempt {
                assert!(
                    !file.exists(),
                    "failure before rename leaves no executable command"
                );
                final_fix_wait(&mut p, |p| !p.local_intents.contains(&id));
                assert!(!p.settings_blocked());
                assert!(final_fix_published_ids(root.path()).is_empty());
                p.send(command.clone()).unwrap(); // only explicit retry queues work
                continue;
            }
            assert!(file.is_file());
            assert!(p.settings_blocked());
            let mut initialized = false;
            let mut h = Harness::builder().with_size([700., 200.]).build_ui_state(
                |ui, p: &mut AgentPane| {
                    if !initialized {
                        kiln_common::fonts::install(ui.ctx());
                        initialized = true;
                        return;
                    }
                    p.composer_action_ui(ui, false);
                },
                p,
            );
            h.run();
            h.get_by_label("중지").click();
            h.run();
            assert!(
                h.state().local_intents.contains(&id),
                "Stop cannot guess delivery"
            );
            final_fix_wait(h.state_mut(), |_| {
                final_fix_published_ids(root.path()).len() == 2
            });
            let interrupt = final_fix_published_ids(root.path())
                .into_iter()
                .find(|other| other != &id)
                .unwrap();
            let queued: super::super::QueuedCommand = journal::read(
                &root
                    .path()
                    .join("commands")
                    .join(format!("{interrupt}.json")),
            )
            .unwrap();
            assert!(matches!(queued.command, Command::Interrupt));
            journal
                .push(Event::CommandState {
                    id: interrupt.clone(),
                    state: CommandPhase::Accepted,
                })
                .unwrap();
            std::fs::remove_file(
                root.path()
                    .join("commands")
                    .join(format!("{interrupt}.json")),
            )
            .unwrap();
            if accepted {
                journal
                    .push(Event::Controls(ControlEvent::Outcome(ControlOutcome {
                        operation: "native".into(),
                        control: "model".into(),
                        requested: ControlValue::Text("new".into()),
                        effective: None,
                        state: ApplicationState::Pending,
                        settled: false,
                        message: None,
                    })))
                    .unwrap();
                journal
                    .push(Event::CommandState {
                        id: id.clone(),
                        state: CommandPhase::Accepted,
                    })
                    .unwrap();
            }
            std::fs::remove_file(file).unwrap();
            final_fix_wait(h.state_mut(), |p| !p.local_intents.contains(&id));
            assert!(h.state().error.is_some());
            if accepted {
                assert_eq!(
                    h.state().transcript.commands.get(&id),
                    Some(&CommandPhase::Accepted)
                );
                assert!(
                    h.state().settings_blocked(),
                    "native unsettled state survives local ACK and Stop"
                );
                h.run();
                assert!(h.query_by_label("중지").is_some());
                journal
                    .push(Event::Controls(ControlEvent::Settlement("native".into())))
                    .unwrap();
                final_fix_wait(h.state_mut(), |p| !p.settings_blocked());
            } else {
                assert_eq!(
                    h.state().transcript.commands.get(&id),
                    Some(&CommandPhase::Unconfirmed)
                );
                assert!(!h.state().settings_blocked());
            }
            h.run();
            assert!(h.query_by_label("중지").is_none());
            assert!(h.query_by_label("보내기").is_some());
            assert!(
                final_fix_published_ids(root.path()).is_empty(),
                "no automatic replay"
            );
            // A late authoritative event always wins even after local absence recovery.
            journal
                .push(Event::CommandState {
                    id: id.clone(),
                    state: CommandPhase::Accepted,
                })
                .unwrap();
            final_fix_wait(h.state_mut(), |p| {
                p.transcript.commands.get(&id) == Some(&CommandPhase::Accepted)
            });
            assert!(!h.state().settings_blocked());
            h.state_mut().send(command.clone()).unwrap(); // explicit user retry is available
        }
    }
}
#[test]
fn failed_enqueue_before_publication_recovers_live_submit_and_approval_controls() {
    use super::super::journal;
    for submit in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut journal = journal::Journal::open(root.path(), "fixture".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        journal
            .push(Event::Lifecycle(if submit {
                Lifecycle::Ready
            } else {
                Lifecycle::Waiting
            }))
            .unwrap();
        if !submit {
            journal
                .push(Event::Request(Request {
                    id: "approval".into(),
                    title: "Write fixture.txt".into(),
                    details: serde_json::json!({"file_path":"fixture.txt"}),
                    questions: vec![],
                    options: vec![],
                }))
                .unwrap();
        }
        std::fs::write(root.path().join("commands"), "blocked mailbox").unwrap();
        let (client, start) = Client::paused(root.path().into(), "fixture");
        let mut p = pane();
        p.transcript = Transcript::default();
        p.client = Some(client);
        start.send(()).unwrap();
        let ctx = egui::Context::default();
        let replay_until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while p.transcript.sequence < journal.transcript.sequence {
            p.tick(&ctx);
            assert!(std::time::Instant::now() < replay_until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        p.composer.text = "preserved 한글 task".into();
        let id = if submit {
            p.submit();
            p.pending.as_ref().unwrap().id.clone()
        } else {
            let id = p
                .send(Command::Respond {
                    request_id: "approval".into(),
                    decision: Decision::Allow,
                    answers: Default::default(),
                })
                .unwrap();
            p.responding.insert("approval".into(), id.clone());
            id
        };
        let ctx = egui::Context::default();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            p.tick(&ctx);
            if p.transcript.commands.get(&id) == Some(&CommandPhase::Rejected) {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "failed enqueue has no command outcome"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(p.error.is_some());
        if submit {
            assert!(p.pending_recoverable());
            assert_eq!(
                p.pending.as_ref().unwrap().draft.text,
                "preserved 한글 task"
            );
        } else {
            assert!(!p.responding.contains_key("approval"));
            assert!(p.transcript.requests.contains_key("approval"));
        }
        std::fs::remove_file(root.path().join("commands")).unwrap();
        let mut initialized = false;
        let mut h = Harness::builder().with_size([700., 600.]).build_ui_state(
            |ui, p: &mut AgentPane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                p.tick(ui.ctx());
                p.ui(ui);
            },
            p,
        );
        let label = if submit {
            "초안으로 복구"
        } else {
            "한 번 허용"
        };
        while h.query_by_label(label).is_none() {
            h.step();
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.get_by_label(label).click();
        h.run_steps(3);
        if submit {
            assert_eq!(h.state().composer.text, "preserved 한글 task");
            assert!(h.state().pending.is_none());
            assert!(!root.path().join("commands").exists());
        } else {
            while !root.path().join("commands").is_dir() {
                h.step();
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(h.state().responding.contains_key("approval"));
        }
    }
}
#[test]
fn ambiguous_enqueue_waits_for_publication_or_authoritative_outcome_before_ui_retry() {
    use super::super::journal;
    for submit in [true, false] {
        for accepted in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let mut journal = journal::Journal::open(root.path(), "fixture".into()).unwrap();
            journal.push(Event::Generation).unwrap();
            journal
                .push(Event::Lifecycle(if submit {
                    Lifecycle::Ready
                } else {
                    Lifecycle::Waiting
                }))
                .unwrap();
            if !submit {
                journal
                    .push(Event::Request(Request {
                        id: "approval".into(),
                        title: "Write fixture.txt".into(),
                        details: serde_json::json!({"file_path":"fixture.txt"}),
                        questions: vec![],
                        options: vec![],
                    }))
                    .unwrap();
            }
            let (client, start) = Client::faulted_enqueue(
                root.path().into(),
                "fixture",
                journal::PublishPoint::AfterRename,
            );
            let mut p = pane();
            p.transcript = Transcript::default();
            p.client = Some(client);
            start.send(()).unwrap();
            let ctx = egui::Context::default();
            let replay_until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while p.transcript.sequence < journal.transcript.sequence {
                p.tick(&ctx);
                assert!(std::time::Instant::now() < replay_until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            p.composer.text = "uncertain 한글 task".into();
            let id = if submit {
                p.submit();
                p.pending.as_ref().unwrap().id.clone()
            } else {
                let id = p
                    .send(Command::Respond {
                        request_id: "approval".into(),
                        decision: Decision::Allow,
                        answers: Default::default(),
                    })
                    .unwrap();
                p.responding.insert("approval".into(), id.clone());
                id
            };
            let ctx = egui::Context::default();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while p.transcript.commands.get(&id) != Some(&CommandPhase::Unconfirmed) {
                p.tick(&ctx);
                assert!(
                    std::time::Instant::now() < until,
                    "ambiguous enqueue has no explicit outcome"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let file = root.path().join("commands").join(format!("{id}.json"));
            assert!(file.is_file());
            for _ in 0..20 {
                p.tick(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if submit {
                assert!(!p.pending_recoverable());
            } else {
                assert!(p.responding.contains_key("approval"));
            }
            if accepted {
                journal
                    .push(Event::CommandState {
                        id: id.clone(),
                        state: CommandPhase::Accepted,
                    })
                    .unwrap();
                if !submit {
                    journal
                        .push(Event::RequestResolved("approval".into()))
                        .unwrap();
                }
            }
            std::fs::remove_file(file).unwrap();
            loop {
                p.tick(&ctx);
                let done = if submit {
                    if accepted {
                        p.pending.is_none()
                    } else {
                        p.pending_recoverable()
                    }
                } else {
                    !p.responding.contains_key("approval")
                };
                if done {
                    break;
                }
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(
                std::fs::read_dir(root.path().join("commands"))
                    .unwrap()
                    .count(),
                0,
                "no automatic resend"
            );
            if accepted {
                assert_eq!(
                    p.transcript.commands.get(&id),
                    Some(&CommandPhase::Accepted)
                );
                assert!(p.recovered.is_none());
            } else {
                let mut initialized = false;
                let mut h = Harness::builder().with_size([700., 600.]).build_ui_state(
                    |ui, p: &mut AgentPane| {
                        if !initialized {
                            kiln_common::fonts::install(ui.ctx());
                            initialized = true;
                            return;
                        }
                        p.tick(ui.ctx());
                        p.ui(ui);
                    },
                    p,
                );
                let label = if submit {
                    "초안으로 복구"
                } else {
                    "한 번 허용"
                };
                while h.query_by_label(label).is_none() {
                    h.step();
                    assert!(std::time::Instant::now() < until);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                h.get_by_label(label).click();
                h.run_steps(3);
                if submit {
                    assert_eq!(h.state().composer.text, "uncertain 한글 task");
                    assert_eq!(
                        std::fs::read_dir(root.path().join("commands"))
                            .unwrap()
                            .count(),
                        0
                    );
                } else {
                    while std::fs::read_dir(root.path().join("commands"))
                        .unwrap()
                        .count()
                        == 0
                    {
                        h.step();
                        assert!(std::time::Instant::now() < until);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    assert_eq!(
                        std::fs::read_dir(root.path().join("commands"))
                            .unwrap()
                            .count(),
                        1
                    );
                }
            }
        }
    }
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
    let request = p.transcript.requests["ask"].clone();
    p.cache.details.insert(
        request.id.clone(),
        (
            request.clone(),
            super::super::approval::project(&request, p.descriptor.tool),
        ),
    );
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

#[test]
fn review_i1_backlog_keeps_reused_approval_and_controls_on_applied_generation() {
    let mut p = pane();
    p.transcript.generation = "G1".into();
    p.transcript.sequence = 1;
    p.transcript.lifecycle = Lifecycle::Waiting;
    p.transcript.requests.insert(
        "7".into(),
        Request {
            id: "7".into(),
            title: "Approval".into(),
            questions: vec![],
            options: vec![],
            details: serde_json::Value::Null,
        },
    );
    let mut batch = (2..66)
        .map(|sequence| Envelope {
            sequence,
            generation: "G1".into(),
            event: Event::Session("old".into()),
        })
        .collect::<Vec<_>>();
    batch.push(Envelope {
        sequence: 66,
        generation: "G2".into(),
        event: Event::Generation,
    });
    batch.push(Envelope {
        sequence: 67,
        generation: "G2".into(),
        event: Event::Request(p.transcript.requests["7"].clone()),
    });
    let (client, rx) = Client::fixture_mailbox(batch, "G1");
    p.client = Some(client);
    p.tick(&egui::Context::default());
    assert_eq!(p.transcript.generation, "G1");
    assert!(!p.backlog.is_empty());
    let request = p.transcript.requests["7"].clone();
    p.cache.details.insert(
        "7".into(),
        (
            request.clone(),
            super::super::approval::project(&request, p.descriptor.tool),
        ),
    );
    let mut initialized = false;
    let mut h = Harness::builder().with_size([700., 700.]).build_ui_state(
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
    h.get_by_label("한 번 허용").click();
    h.run();
    assert_eq!(
        rx.recv().unwrap().generation,
        "G1",
        "visible G1 Allow cannot approve reused G2/request 7"
    );
    let p = h.state_mut();
    for command in [
        Command::Interrupt,
        Command::SetMode {
            mode: "mode".into(),
        },
        Command::Restart,
    ] {
        p.send(command).unwrap();
        assert_eq!(
            rx.recv().unwrap().generation,
            "G1",
            "a control must never target unseen G2"
        );
    }
}
#[test]
fn review_i3_committed_snapshot_uses_post_edit_composition_range() {
    for prior_preedit in [true, false] {
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
        if prior_preedit {
            h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "ㅎ".into(),
                active_range_chars: None,
            }));
            h.run();
        }
        h.input_mut().events.push(if prior_preedit {
            egui::Event::Ime(egui::ImeEvent::Commit("한".into()))
        } else {
            egui::Event::Text("abc".into())
        });
        h.input_mut()
            .events
            .push(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "ㄱ".into(),
                active_range_chars: None,
            }));
        h.run();
        let committed = if prior_preedit { "한" } else { "abc" };
        assert_eq!(h.state().composer.text, format!("{committed}ㄱ"));
        assert_eq!(
            h.state().draft().composer.text,
            committed,
            "preedit must never replace committed text in persisted state"
        );
        h.key_press(egui::Key::Tab);
        h.run();
        assert_eq!(h.state().composer.text, committed);
        h.get_by_label("보내기").click();
        h.run();
        assert!(matches!(h.state().sent.last(),Some(Command::Submit{text,..}) if text==committed));
    }
}
#[test]
fn review_i4_codex_permission_card_projects_actionable_details_without_transport_dump() {
    let mut p = pane();
    let request = Request {
        id: "7".into(),
        title: "item/commandExecution/requestApproval".into(),
        questions: vec![],
        options: vec![],
        details: serde_json::json!({"threadId":"private-thread","turnId":"private-turn","itemId":"private-item","command":"cargo test --lib","cwd":"/fixture/project","reason":"Run focused validation"}),
    };
    p.cache.details.insert(
        request.id.clone(),
        (
            request.clone(),
            super::super::approval::project(&request, p.descriptor.tool),
        ),
    );
    let mut initialized = false;
    let mut h = Harness::builder().with_size([700., 600.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.request_ui(ui, &request);
        },
        p,
    );
    h.run();
    h.get_by_label("명령 실행 승인");
    h.get_by_label("cargo test --lib");
    h.get_by_label("/fixture/project");
    h.get_by_label("Run focused validation");
    for forbidden in [
        "item/commandExecution/requestApproval",
        "private-thread",
        "private-turn",
        "private-item",
        "threadId",
    ] {
        assert_eq!(h.query_all_by_label_contains(forbidden).count(), 0);
    }
}

#[test]
fn review_i2_stopped_local_mailbox_recovers_only_after_complete_live_ready_replay() {
    let root = tempfile::tempdir().unwrap();
    let mut journal = super::super::journal::Journal::open(root.path(), "G1".into()).unwrap();
    journal.push(Event::Generation).unwrap();
    journal.push(Event::Lifecycle(Lifecycle::Ready)).unwrap();
    let worker_lock = super::super::journal::open(&root.path().join("host.lock"), false).unwrap();
    worker_lock.try_lock().unwrap();
    let (old, release) = Client::paused(root.path().into(), "G1");
    let id = old
        .send(Command::Submit {
            text: "exact  \n한글".into(),
            attachments: vec![],
            model: None,
        })
        .unwrap();
    let mut p = pane();
    p.restore_draft(SavedDraft {
        pending: Some(PendingDraft {
            id: id.clone(),
            draft: Draft {
                text: "exact  \n한글".into(),
                ..Default::default()
            },
        }),
        ..Default::default()
    });
    p.client = Some(Client::at_directory(Ok(root.path().into())));
    let ctx = egui::Context::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while p.transcript.sequence < 2 {
        p.tick(&ctx);
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(p.transcript.lifecycle, Lifecycle::Ready);
    assert!(
        !p.pending_recoverable(),
        "a locally pending command may still enqueue"
    );
    drop(old);
    release.send(()).unwrap();
    while !p.pending_recoverable() {
        p.tick(&ctx);
        assert!(
            std::time::Instant::now() < until,
            "missing restored command must become unknown/recoverable after full replay even while worker stays Ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(p.pending.as_ref().unwrap().draft.text, "exact  \n한글");
    assert!(p.sent.is_empty());
    assert_eq!(p.transcript.lifecycle, Lifecycle::Ready);
    assert!(super::super::journal::pending_ids(root.path()).is_empty());
    let mut initialized = false;
    let mut h = Harness::builder().with_size([700., 700.]).build_ui_state(
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
    h.get_by_label("초안으로 복구").click();
    h.run();
    assert!(h.state().pending.is_none());
    assert_eq!(h.state().composer.text, "exact  \n한글");
    assert_eq!(h.state().recovered.as_ref().unwrap().id, id);
    assert!(h.state().transcript.commands.is_empty());
    assert_eq!(h.state().transcript.lifecycle, Lifecycle::Ready);
    assert!(super::super::journal::pending_ids(root.path()).is_empty());
}

#[test]
fn review_i4_codex_elicitation_shows_questions_and_server_without_schema_or_ids() {
    let mut p = pane();
    let request = Request {
        id: "private-rpc-id".into(),
        title: "mcpServer/elicitation/request".into(),
        questions: vec![Question {
            id: "environment".into(),
            text: "Environment".into(),
            options: vec![],
            multi_select: false,
        }],
        options: vec![],
        details: serde_json::json!({"threadId":"private-thread","turnId":"private-turn","serverName":"Fixture MCP","message":"Pick deployment environment","requestedSchema":{"type":"object","properties":{"environment":{"type":"string","title":"Environment"}}}}),
    };
    p.cache.details.insert(
        request.id.clone(),
        (
            request.clone(),
            super::super::approval::project(&request, p.descriptor.tool),
        ),
    );
    let mut initialized = false;
    let mut h = Harness::builder().with_size([700., 600.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.request_ui(ui, &request);
        },
        p,
    );
    h.run();
    h.get_by_label("추가 정보 요청");
    h.get_by_label("Fixture MCP");
    h.get_by_label("Pick deployment environment");
    assert!(h.query_all_by_label_contains("Environment").count() > 0);
    for forbidden in [
        "mcpServer/elicitation/request",
        "private-rpc-id",
        "private-thread",
        "private-turn",
        "requestedSchema",
        "properties",
    ] {
        assert_eq!(h.query_all_by_label_contains(forbidden).count(), 0);
    }
}
#[test]
fn review_i4_file_and_permission_summaries_retain_paths_and_scope() {
    let mut request = Request {
        id: "7".into(),
        title: "item/fileChange/requestApproval".into(),
        questions: vec![],
        options: vec![],
        details: serde_json::json!({"threadId":"hidden","grantRoot":"/workspace","changes":[{"path":"src/main.rs"}],"reason":"Apply requested change"}),
    };
    let summary = super::super::approval::project(&request, kiln_accounts::Tool::Codex);
    assert_eq!(summary.kind, super::super::approval::Kind::Files);
    assert!(summary.fields.iter().any(|(_, v)| v == "/workspace"));
    assert!(summary.fields.iter().any(|(_, v)| v == "src/main.rs"));
    request.title = "item/permissions/requestApproval".into();
    request.details = serde_json::json!({"permissions":{"network":{"enabled":true},"fileSystem":{"read":["/read"],"write":["/write"]}},"scope":"turn","cwd":"/workspace","threadId":"hidden"});
    let summary = super::super::approval::project(&request, kiln_accounts::Tool::Codex);
    assert_eq!(summary.kind, super::super::approval::Kind::Permission);
    for required in [
        "/read",
        "/write",
        kiln_common::i18n::tr("현재 작업"),
        "/workspace",
    ] {
        assert!(summary.fields.iter().any(|(_, v)| v == required));
    }
    assert!(!format!("{:?}", summary.fields).contains("hidden"));
}

#[test]
fn review_i4_native_tool_envelopes_are_projected_but_domain_argument_ids_survive() {
    let mut p = pane();
    p.transcript.sequence = 1;
    p.transcript.entries = vec![
        Entry::Tool(ToolCall {
            id: "native-command-id".into(),
            name: "cargo test".into(),
            input: serde_json::json!({"id":"native-command-id","type":"commandExecution","status":"inProgress","command":"cargo test --lib","cwd":"/fixture/project","processId":"private-process"}),
            ..Default::default()
        }),
        Entry::Tool(ToolCall {
            id: "native-mcp-id".into(),
            name: "fixture / save_invoice".into(),
            input: serde_json::json!({"id":"native-mcp-id","type":"mcpToolCall","status":"inProgress","server":"fixture","tool":"save_invoice","arguments":{"id":"invoice-42","type":"invoice","status":"draft"}}),
            ..Default::default()
        }),
    ];
    let ctx = egui::Context::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        p.tick(&ctx);
        if p.cache.inputs.len() == 2 {
            break;
        }
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let command = &p.cache.inputs["native-command-id"];
    assert!(command.contains("cargo test --lib"));
    assert!(command.contains("/fixture/project"));
    for forbidden in [
        "native-command-id",
        "commandExecution",
        "inProgress",
        "private-process",
    ] {
        assert!(!command.contains(forbidden));
    }
    let mcp = &p.cache.inputs["native-mcp-id"];
    assert!(mcp.contains("invoice-42"));
    assert!(mcp.contains("invoice"));
    assert!(mcp.contains("draft"));
    for key in ["\"id\"", "\"type\"", "\"status\""] {
        assert!(mcp.contains(key));
    }
    assert!(!mcp.contains("native-mcp-id"));
    assert!(!mcp.contains("mcpToolCall"));
}

#[test]
fn review_i4_general_tool_arguments_are_not_treated_as_native_envelopes() {
    let tool = ToolCall {
        id: "domain-42".into(),
        name: "save_record".into(),
        input: serde_json::json!({"id":"domain-42","type":"commandExecution","status":"draft","custom":"preserved"}),
        ..Default::default()
    };
    let preview = super::super::approval::tool_input(&tool, kiln_accounts::Tool::Claude);
    for expected in ["domain-42", "commandExecution", "draft", "preserved"] {
        assert!(preview.contains(expected));
    }
}
#[test]
fn review_i4_changed_request_never_uses_stale_approval_summary() {
    let mut p = pane();
    let old = Request {
        id: "7".into(),
        title: "Edit".into(),
        questions: vec![],
        options: vec![],
        details: serde_json::json!({"file_path":"old.txt"}),
    };
    p.cache.details.insert(
        old.id.clone(),
        (
            old.clone(),
            super::super::approval::project(&old, p.descriptor.tool),
        ),
    );
    let mut new = old;
    new.details = serde_json::json!({"file_path":"new.txt"});
    let mut initialized = false;
    let mut h = Harness::builder().with_size([700., 500.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.request_ui(ui, &new);
        },
        p,
    );
    h.run();
    h.get_by_label("한 번 허용").click();
    h.run();
    assert!(h.state().sent.is_empty());
    assert_eq!(h.query_all_by_label_contains("old.txt").count(), 0);
}

#[cfg(unix)]
#[test]
fn review_i2_real_ready_host_keeps_running_while_lost_local_send_is_recovered() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    struct OwnedWorker(std::process::Child);
    impl Drop for OwnedWorker {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0.id() as i32, libc::SIGHUP);
            }
            let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while self.0.try_wait().unwrap().is_none() {
                if std::time::Instant::now() > until {
                    unsafe {
                        libc::kill(-(self.0.id() as i32), libc::SIGKILL);
                    }
                    let _ = self.0.wait();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("provider");
    let python = kiln_common::paths::find_executable("python3", &[]).unwrap();
    std::fs::write(&provider,format!("#!{}\n{}",python.display(),r#"
import json,sys,pathlib
for line in sys.stdin:
 v=json.loads(line)
 if v.get('type')=='control_request' and v['request'].get('subtype')=='initialize':
  print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':v['request_id'],'response':{'models':[]}}}),flush=True)
  print(json.dumps({'type':'system','subtype':'init','session_id':'fixture-live-session','model':'fixture'}),flush=True)
 elif v.get('type')=='user':
  with pathlib.Path('submitted.txt').open('a') as out:out.write('UNEXPECTED_SUBMIT\n')
"#)).unwrap();
    std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
    let descriptor = Descriptor {
        id: super::super::journal::unique(),
        tool: kiln_accounts::Tool::Claude,
        cwd: root.path().into(),
        account_label: "Fixture".into(),
    };
    let config = super::super::Config {
        descriptor: descriptor.clone(),
        program: provider,
        environment: vec![],
        launch_options: super::super::LaunchOptions::Inherited,
        initial_prompt: None,
    };
    let directory = root.path().join("agent-gui").join(&descriptor.id);
    super::super::journal::private_dir(&directory).unwrap();
    super::super::journal::atomic(
        &directory.join("config.json"),
        &serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "app::agent_gui::host::subprocess_tests::worker_entry",
            "--nocapture",
        ])
        .env("KILN_GUI_FIXTURE_ID", &descriptor.id)
        .env("KILN_CONFIG_DIR", root.path())
        .env("HOME", root.path())
        .env("KILN_ACCOUNTS_SANDBOX", root.path())
        .env("CLAUDE_CONFIG_DIR", root.path().join("claude"))
        .env("CODEX_HOME", root.path().join("codex"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for key in [
        "KILN_SOCKET",
        "KILN_SESSION",
        "KILN_INTEGRATION_DIR",
        "KILN_MACOS_RESPONSIBILITY_PID",
        "KILN_ORIGINAL_ZDOTDIR",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
    ] {
        command.env_remove(key);
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut worker = OwnedWorker(command.spawn().unwrap());
    let mut p = AgentPane::fixture(descriptor);
    p.client = Some(Client::at_directory(Ok(directory.clone())));
    let ctx = egui::Context::default();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while p.transcript.lifecycle != Lifecycle::Ready || p.transcript.provider_session_id.is_none() {
        p.tick(&ctx);
        assert!(std::time::Instant::now() < until);
        assert!(worker.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (old, release) = Client::paused(directory.clone(), &p.transcript.generation);
    let id = old
        .send(Command::Submit {
            text: "lost local 한글  \n".into(),
            attachments: vec![],
            model: None,
        })
        .unwrap();
    p.restore_draft(SavedDraft {
        pending: Some(PendingDraft {
            id: id.clone(),
            draft: Draft {
                text: "lost local 한글  \n".into(),
                ..Default::default()
            },
        }),
        ..Default::default()
    });
    assert!(!p.pending_recoverable());
    drop(old);
    release.send(()).unwrap();
    while !p.pending_recoverable() {
        p.tick(&ctx);
        assert!(std::time::Instant::now() < until);
        assert!(worker.0.try_wait().unwrap().is_none());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(p.transcript.lifecycle, Lifecycle::Ready);
    assert_eq!(
        p.pending.as_ref().unwrap().draft.text,
        "lost local 한글  \n"
    );
    assert!(!root.path().join("submitted.txt").exists());
    assert!(p.transcript.commands.is_empty());
    assert!(super::super::journal::pending_ids(&directory).is_empty());
    assert!(worker.0.try_wait().unwrap().is_none());
}

#[test]
fn review_round2_i4_claude_mcp_arguments_are_visible_without_an_adjacent_tool_card() {
    for tool_use_id in [Some("native-tool-id"), None] {
        let input = serde_json::json!({"text":"native GUI MCP check","recipient":"qa@example.invalid","body":"approval body","id":"domain-record-42","type":"invoice","status":"draft"});
        let mut native = serde_json::json!({"type":"control_request","request_id":"native-control-id","request":{"subtype":"can_use_tool","tool_name":"mcp__kiln_qa__echo","input":input}});
        if let Some(id) = tool_use_id {
            native["request"]["tool_use_id"] = id.into();
        }
        let mut adapter = super::super::providers::Adapter::new(
            kiln_accounts::Tool::Claude,
            "/fixture".into(),
            None,
        );
        let effects = adapter.ingest(native).unwrap();
        assert_eq!(
            effects.events.iter().any(|e| matches!(e, Event::Tool(_))),
            tool_use_id.is_some()
        );
        let request = effects
            .events
            .into_iter()
            .find_map(|event| {
                if let Event::Request(request) = event {
                    Some(request)
                } else {
                    None
                }
            })
            .unwrap();
        let mut p = pane();
        p.descriptor.tool = kiln_accounts::Tool::Claude;
        p.cache.details.insert(
            request.id.clone(),
            (
                request.clone(),
                super::super::approval::project(&request, p.descriptor.tool),
            ),
        );
        let mut initialized = false;
        let mut h = Harness::builder().with_size([700., 700.]).build_ui_state(
            |ui, p: &mut AgentPane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                p.request_ui(ui, &request);
            },
            p,
        );
        h.run();
        for argument in [
            "native GUI MCP check",
            "qa@example.invalid",
            "approval body",
            "domain-record-42",
            "invoice",
            "draft",
        ] {
            assert!(
                h.query_all_by_label_contains(argument).count() > 0,
                "approval itself must show {argument}, even without tool_use_id"
            );
        }
        for transport in ["native-control-id", "native-tool-id", "can_use_tool"] {
            assert_eq!(h.query_all_by_label_contains(transport).count(), 0);
        }
        h.get_by_label("한 번 허용").click();
        h.run();
        assert!(matches!(
            h.state().sent.last(),
            Some(Command::Respond {
                decision: Decision::Allow,
                ..
            })
        ));
    }
}


#[test]
fn final_alignment_cards_actions_and_readable_column_fit_native_themes() {
    for dark in [true, false] {
        for width in [320., 520., 1400.] {
            let mut p = pane();
            p.transcript.lifecycle = Lifecycle::Ready;
            p.transcript.selected_model = Some("fixture-model".into());
            p.transcript.models = vec![Model {
                id: "fixture-model".into(),
                label: "Fixture model".into(),
            }];
            p.transcript.modes = vec![Choice {
                id: "fixture-mode".into(),
                label: "Fixture mode".into(),
                description: String::new(),
            }];
            p.transcript.selected_mode = Some("fixture-mode".into());
            p.transcript.entries.push(Entry::Message(Message {
                id: "alignment-user".into(),
                role: Role::User,
                text: "Native layout probe".into(),
                attachments: vec![],
            }));
            p.transcript.entries.push(Entry::Tool(ToolCall {
                id: "alignment-tool".into(),
                name: "Bash".into(),
                status: ToolStatus::Running,
                output: "fixture command output".into(),
                diff: "--- a/fixture.txt\n+++ b/fixture.txt\n-old\n+new".into(),
                ..Default::default()
            }));
            let mut initialized = false;
            let mut h = Harness::builder().with_size([width, 760.]).build_ui_state(
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
            assert_eq!(h.get_by_label("모델").accesskit_node().value().as_deref(), Some("모델: 기본 모델"));
            assert_eq!(h.get_by_label("모드").accesskit_node().value().as_deref(), Some("모드: Fixture mode"));
            let (outer, clip) = h.state().composer_geometry.expect("actual composer geometry");
            assert!(clip.contains_rect(outer), "the complete composer frame must fit its actual painting clip: {outer:?} vs {clip:?}");
            let message = h.get_by_label("메시지").rect();
            let tool = h.get_by_label("명령 실행 · 실행 중").rect();
            let send = h.get_by_label("보내기").rect();
            let attach = h.get_by_label("이미지 첨부").rect();
            assert!(
                tool.width() >= message.width() - 12.,
                "tool card must fill the conversation column: {tool:?} vs {message:?}"
            );
            assert!(
                tool.width() <= 900.,
                "wide prose/tool column stays readable"
            );
            assert!(
                (send.right() - message.right()).abs() <= 4.,
                "send belongs at the composer's right edge: {send:?} vs {message:?}"
            );
            assert!(
                attach.left() < message.center().x,
                "attachment controls remain on the left"
            );
            assert!(
                h.get_by_label("나").rect().top() < if width > 1000. { 112. } else { 144. },
                "metadata and labeled settings navigation must stay compact: {:?}",
                h.get_by_label("나").rect()
            );
            for rect in [
                message,
                tool,
                send,
                attach,
                h.get_by_label("Enter로 보내기 · Shift+Enter로 줄바꿈")
                    .rect(),
            ] {
                assert!(h.ctx.content_rect().contains_rect(rect));
            }
            h.get_by_label("명령 실행 · 실행 중").click();
            h.run();
            assert!(
                h.query_all_by_label_contains("fixture command output")
                    .count()
                    > 0
            );
            assert!(h.query_all_by_label_contains("+new").count() > 0);
            let image = h.render().expect("native egui render");
            if let Some(dir) = std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                image
                    .save(dir.join(format!(
                        "alignment-{}-{width:.0}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
            let p = h.state_mut();
            p.transcript.lifecycle = Lifecycle::Running;
            h.run();
            let stop = h.get_by_label("중지").rect();
            assert!(
                (stop.right() - message.right()).abs() <= 4.,
                "Stop uses the same right anchor"
            );
            h.get_by_label("중지").click();
            h.run();
            assert!(matches!(h.state().sent.last(), Some(Command::Interrupt)));
        }
    }
}


fn visual_fix1_capture(h: &mut Harness<'_, impl Sized>, name: &str) -> image::RgbaImage {
    let image = h.render().expect("native egui render");
    if let Some(dir) = std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        image.save(dir.join(format!("{name}.png"))).unwrap();
    }
    image
}
#[test]
fn visual_fix1_persistent_segment_selection_is_visible_in_both_native_themes() {
    for dark in [true, false] {
        let mut initialized = false;
        let mut h = Harness::builder().with_size([320., 160.]).build_ui_state(
            |ui, value: &mut Presentation| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                Theme::current().apply(ui.ctx());
                value.ui(ui);
            },
            Presentation::Gui,
        );
        h.run();
        for (index, selected) in ["대화 GUI", "터미널"].into_iter().enumerate() {
            if index == 1 {
                h.get_by_label(selected).click();
                h.run();
            }
            let image = visual_fix1_capture(
                &mut h,
                &format!("segment-{}-{index}", if dark { "dark" } else { "light" }),
            );
            let sample = |label| {
                let rect = h.get_by_label(label).rect();
                let scale = image.width() as f32 / h.ctx.content_rect().width();
                *image.get_pixel(
                    ((rect.left() + 8.) * scale) as u32,
                    ((rect.top() + 8.) * scale) as u32,
                )
            };
            let gui = sample("대화 GUI");
            let terminal = sample("터미널");
            let difference: i32 = (0..3)
                .map(|c| (gui[c] as i32 - terminal[c] as i32).abs())
                .sum();
            assert!(
                difference > 24,
                "persistent selected material must differ away from text/focus/hover: {gui:?} vs {terminal:?}"
            );
            assert_eq!(
                h.state(),
                &if index == 0 {
                    Presentation::Gui
                } else {
                    Presentation::Terminal
                }
            );
        }
    }
}
#[test]
fn visual_fix1_tool_uses_one_flush_card_boundary_and_native_keyboard_collapse() {
    fn stroked_rects(shape: &egui::Shape, result: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.width > 0. => result.push(rect.rect),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    stroked_rects(shape, result);
                }
            }
            _ => {}
        }
    }
    for dark in [true, false] {
        let mut p = pane();
        let tool = ToolCall {
            id: "single-card".into(),
            name: "Bash".into(),
            status: ToolStatus::Completed,
            output: "keyboard expanded output".into(),
            ..Default::default()
        };
        p.transcript.entries.push(Entry::Tool(tool.clone()));
        let mut initialized = false;
        let mut h = Harness::builder().with_size([520., 420.]).build_ui_state(
            |ui, p: &mut AgentPane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                Theme::current().apply(ui.ctx());
                p.tool_ui(ui, &tool);
            },
            p,
        );
        h.run();
        let header = h.get_by_label("명령 실행 · 완료").rect();
        let mut boundaries = vec![];
        for shape in &h.output().shapes {
            stroked_rects(&shape.shape, &mut boundaries);
        }
        assert_eq!(
            boundaries.len(),
            1,
            "one card boundary, no nested outlined header: {boundaries:?}"
        );
        assert!(
            (header.left() - boundaries[0].left()).abs() <= 2.
                && (header.right() - boundaries[0].right()).abs() <= 2.,
            "header must be flush with the single boundary"
        );
        visual_fix1_capture(
            &mut h,
            &format!("tool-{}-collapsed", if dark { "dark" } else { "light" }),
        );
        h.key_press(egui::Key::Tab);
        h.run();
        assert!(h.get_by_label("명령 실행 · 완료").accesskit_node().is_focused());
        h.key_press(egui::Key::Enter);
        h.run();
        assert!(h.query_by_label("keyboard expanded output").is_some());
        visual_fix1_capture(
            &mut h,
            &format!("tool-{}-expanded", if dark { "dark" } else { "light" }),
        );
        h.key_press(egui::Key::Enter);
        h.run();
        assert!(h.query_by_label("keyboard expanded output").is_none());
    }
}


#[test]
fn visual_fix2_full_pane_approval_arguments_are_painted_before_decisions_after_long_history() {
    fn text_paint(h: &Harness<'_, AgentPane>, needle: &str) -> Vec<(egui::Rect, egui::Rect)> {
        fn collect(
            shape: &egui::Shape,
            clip: egui::Rect,
            needle: &str,
            out: &mut Vec<(egui::Rect, egui::Rect)>,
        ) {
            match shape {
                egui::Shape::Text(text) if text.galley.text().contains(needle) => {
                    out.push((text.visual_bounding_rect(), clip))
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, clip, needle, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = vec![];
        for shape in &h.output().shapes {
            collect(&shape.shape, shape.clip_rect, needle, &mut out);
        }
        out
    }
    for dark in [true, false] {
        for width in [1400., 520.] {
            for long in [false, true] {
                let mut p = pane();
                p.descriptor.tool = kiln_accounts::Tool::Claude;
                p.transcript.lifecycle = Lifecycle::Waiting;
                for index in 0..30 {
                    p.transcript.entries.push(Entry::Message(Message {
                        id: format!("earlier-{index}"), role: if index % 2 == 0 {Role::User} else {Role::Assistant},
                        text: "Earlier conversation context.\nAnother retained line.\nA third history line.".into(), attachments: vec![],
                    }));
                }
                p.transcript.entries.push(Entry::Tool(ToolCall {
                    id: "approval-tool".into(),
                    name: "mcp__fixture__echo".into(),
                    status: ToolStatus::Running,
                    ..Default::default()
                }));
                let needle = if long {
                    "bounded approval body"
                } else {
                    "native GUI MCP check"
                };
                let request = Request {
                    id: "permission".into(),
                    title: "mcp__fixture__echo".into(),
                    questions: vec![],
                    options: vec![],
                    details: serde_json::json!({"text": if long { "bounded approval body ".repeat(600) } else { "native GUI MCP check".into() }}),
                };
                p.cache.details.insert(
                    request.id.clone(),
                    (
                        request.clone(),
                        super::super::approval::project(&request, p.descriptor.tool),
                    ),
                );
                p.transcript.requests.insert(request.id.clone(), request);
                let mut initialized = false;
                let mut h = Harness::builder().with_size([width, 880.]).build_ui_state(
                    |ui, p: &mut AgentPane| {
                        if !initialized {
                            kiln_common::fonts::install(ui.ctx());
                            initialized = true;
                            return;
                        }
                        Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                        Theme::current().apply(ui.ctx());
                        ui.add_space(72.); // Native app toolbar and pane chrome above the complete AgentPane.
                        p.ui(ui);
                    },
                    p,
                );
                h.run();
                visual_fix1_capture(
                    &mut h,
                    &format!(
                        "approval-{}-{width:.0}-{}",
                        if dark { "dark" } else { "light" },
                        if long { "long" } else { "short" }
                    ),
                );
                let paints = text_paint(&h, needle);
                assert!(
                    !paints.is_empty(),
                    "actual argument must emit a paint shape, AX presence is insufficient"
                );
                if long {
                    assert!(
                        paints.iter().all(|(_, clip)| clip.height() <= 180. + 1.),
                        "long approval input retains a bounded scrolling paint viewport: {paints:?}"
                    );
                } else {
                    assert!(
                        paints
                            .iter()
                            .all(|(bounds, clip)| clip.contains_rect(*bounds)),
                        "short argument must be painted fully before decision controls: {paints:?}"
                    );
                }
                for label in ["한 번 허용", "거부"] {
                    let painted = text_paint(&h, label);
                    assert!(
                        !painted.is_empty()
                            && painted
                                .iter()
                                .all(|(bounds, clip)| clip.contains_rect(*bounds)),
                        "decision stays painted and reachable: {label} {painted:?}"
                    );
                }
                h.get_by_label("한 번 허용").click();
                h.run();
                assert!(
                    matches!(h.state().sent.last(), Some(Command::Respond { request_id, decision: Decision::Allow, .. }) if request_id == "permission")
                );
            }
        }
    }
}


#[test]
fn diagnostic_valid_hangul_preedit_commit_batches_preserve_text_and_never_autosend() {
    fn preedit(text: &str) -> egui::Event {
        let end = text.chars().count();
        egui::Event::Ime(egui::ImeEvent::Preedit { text: text.into(), active_range_chars: Some(end..end) })
    }
    // A valid two-syllable composition: each preedit replaces the current mark;
    // a committed syllable is followed by the next mark, not raw Jamo Text events.
    let stages = vec![
        vec![preedit("ㅎ")], vec![preedit("하")], vec![preedit("한")],
        vec![preedit(""), egui::Event::Ime(egui::ImeEvent::Commit("한".into())), preedit("ㄱ")],
        vec![preedit("그")], vec![preedit("글")],
    ];
    let groupings = [vec![vec![0],vec![1],vec![2],vec![3],vec![4],vec![5]], vec![vec![0,1,2,3,4,5]], vec![vec![0],vec![1],vec![2,3,4,5]]];
    for (group_index, groups) in groupings.iter().enumerate() {
        for commit_in_last_batch in [false, true] {
            let mut p = pane(); p.transcript.lifecycle = Lifecycle::Ready;
            let mut initialized = false;
            let mut h = Harness::builder().with_size([520.,760.]).build_ui_state(|ui, p: &mut AgentPane| {
                if !initialized { kiln_common::fonts::install(ui.ctx()); initialized = true; return; }
                p.ui(ui);
            }, p);
            h.run(); h.get_by_label("메시지").click(); h.run();
            let confirmation = || vec![preedit(""), egui::Event::Ime(egui::ImeEvent::Commit("글".into())), egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }];
            for (batch, indices) in groups.iter().enumerate() {
                for index in indices { h.input_mut().events.extend(stages[*index].clone()); }
                if commit_in_last_batch && batch + 1 == groups.len() { h.input_mut().events.extend(confirmation()); }
                h.run(); assert!(h.state().sent.is_empty(), "no automatic send in grouping {group_index}, batch {batch}");
            }
            if !commit_in_last_batch {
                assert_eq!(h.state().composer.text, "한글");
                assert!(h.state().composing);
                assert_eq!(h.state().draft().composer.text, "한", "persist only committed syllables");
                assert!(h.get_by_label("보내기").accesskit_node().is_disabled());
                h.input_mut().events.extend(confirmation()); h.run();
            }
            assert_eq!(h.state().composer.text, "한글\n", "grouping {group_index}, same-frame confirmation {commit_in_last_batch}");
            assert_eq!(h.state().draft().composer.text, "한글\n");
            assert!(!h.state().composing); assert!(h.state().sent.is_empty());
            h.get_by_label("보내기").click(); h.run();
            assert!(matches!(h.state().sent.last(), Some(Command::Submit { text, .. }) if text == "한글\n"));
            assert_eq!(h.state().sent.len(), 1);
        }
    }
}

#[test]
fn task6_native_setting_barrier_blocks_explicit_send() {
    let mut p = pane();
    p.transcript.lifecycle = Lifecycle::Ready;
    p.composer.text = "preserved draft".into();
    p.transcript.controls.outcomes.insert("setting-1".into(), ControlOutcome {
        operation: "setting-1".into(), control: "model".into(),
        requested: ControlValue::Text("new-model".into()), effective: None,
        state: ApplicationState::Unconfirmed, settled: false, message: None,
    });
    p.submit();
    assert!(p.sent.is_empty(), "unsettled native settings must block Send");
    assert_eq!(p.composer.text, "preserved draft");
}

#[test]
fn task6_localized_tool_examples_preserve_wire_data() {
    use kiln_common::i18n::{Language, with_language};
    let tools = [
        ("AskUserQuestion", "사용자에게 질문"),
        ("mcp__kiln_qa__echo", "Kiln QA · 메시지 반환"),
    ];
    with_language(Language::Korean, || {
        for (name, expected) in tools {
            let tool = ToolCall { name: name.into(), input: serde_json::json!({"text":"exact 한글"}), ..Default::default() };
            let original = tool.clone();
            assert_eq!(super::super::approval::tool_title(&tool, Tool::Claude), expected);
            assert_eq!(tool, original);
        }
    });
}
