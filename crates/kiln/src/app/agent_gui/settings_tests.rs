use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
fn fixture(tool: Tool) -> AgentPane {
    let mut p = AgentPane::fixture(Descriptor {
        id: "controls-fixture".into(),
        tool,
        cwd: "/fixture".into(),
        account_label: "Fixture account".into(),
    });
    p.transcript.lifecycle = Lifecycle::Ready;
    p.transcript.generation = "fixture".into();
    p.composer.text = "keep 한글 draft".into();
    p
}
fn select(id: &str, current: &str, choices: &[&str]) -> ControlOption {
    ControlOption {
        id: id.into(),
        label: id.into(),
        description: "Native fixture choice".into(),
        category: None,
        kind: ControlKind::Select,
        choices: choices
            .iter()
            .map(|s| ControlChoice {
                value: (*s).into(),
                label: (*s).into(),
                description: String::new(),
                group: None,
            })
            .collect(),
        current: Some(ControlValue::Text(current.into())),
        scope: ControlScope::Live,
        available: true,
        reason: None,
        source: Some("acp".into()),
        documentation: None,
    }
}
fn entry(name: &str) -> NativeEntry {
    NativeEntry {
        name: name.into(),
        description: "Native description".into(),
        argument_hint: Some("target".into()),
        path: None,
        terminal_only: false,
        interface: BTreeMap::new(),
        builtin: Some(false),
        enabled: Some(true),
        scope: Some("project".into()),
        origin: Some("fixture".into()),
        model: None,
        aliases: vec![],
    }
}
fn harness(p: AgentPane, width: f32, dark: bool) -> Harness<'static, AgentPane> {
    let mut initialized = false;
    Harness::builder().with_size([width, 820.]).build_ui_state(
        move |ui, p| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
            Theme::current().apply(ui.ctx());
            p.ui(ui);
        },
        p,
    )
}
#[test]
fn task6_actual_quick_controls_serialize_intent_and_native_settlement_all_sizes() {
    for width in [320., 900.] {
        for dark in [false, true] {
            let mut p = fixture(Tool::Antigravity);
            p.transcript.controls.options_reported = true;
            p.transcript.controls.options = vec![
                select("model", "old", &["old", "new"]),
                select("thought_level", "low", &["low", "high"]),
                select("mode", "ask", &["ask", "code"]),
            ];
            let mut h = harness(p, width, dark);
            h.run();
            h.get_by_label("모델").click();
            h.run();
            h.get_by_label("new").click();
            h.run();
            assert_eq!(
                h.state().sent,
                vec![Command::Configure {
                    control: "model".into(),
                    value: ControlValue::Text("new".into())
                }]
            );
            assert_eq!(h.state().composer.text, "keep 한글 draft");
            assert!(h.state().settings_blocked());
            h.state_mut().submit();
            assert_eq!(h.state().sent.len(), 1);
            // A transport ACK alone cannot clear the native barrier.
            let id = h.state().local_intents.iter().next().unwrap().clone();
            h.state_mut()
                .transcript
                .commands
                .insert(id, CommandPhase::Accepted);
            h.state_mut().transcript.controls.outcomes.insert(
                "native".into(),
                ControlOutcome {
                    operation: "native".into(),
                    control: "model".into(),
                    requested: ControlValue::Text("new".into()),
                    effective: None,
                    state: ApplicationState::Pending,
                    settled: false,
                    message: None,
                },
            );
            h.state_mut().reconcile();
            assert!(h.state().settings_blocked());
            h.run();
            let stop = h.get_by_label("중지").rect();
            h.get_by_label("중지").click();
            h.run();
            assert!(
                matches!(h.state().sent.last(), Some(Command::Interrupt)),
                "Stop {stop:?}, geometry {:?}, sent {:?}, width {width}",
                h.state().composer_geometry,
                h.state().sent
            );
            h.state_mut()
                .transcript
                .controls
                .outcomes
                .get_mut("native")
                .unwrap()
                .settled = true;
            h.state_mut().transcript.controls.options[0].current =
                Some(ControlValue::Text("new".into()));
            h.run();
            assert!(!h.state().settings_blocked());
            for name in ["모델", "추론 강도", "모드", "보내기"] {
                let rect = h.get_by_label(name).rect();
                assert!(
                    rect.left() >= 0. && rect.right() <= width,
                    "{name}: {rect:?} at {width}"
                );
                assert!(rect.bottom() < 820.);
            }
            h.render().expect("actual native widget render");
        }
    }
}
#[test]
fn task6_palette_inserts_native_skill_preserves_draft_attachment_and_sends_explicitly() {
    for width in [320., 900.] {
        for dark in [false, true] {
            let mut p = fixture(Tool::Codex);
            let attachment = Attachment {
                path: "/fixture/image".into(),
                name: "image.png".into(),
                mime: "image/png".into(),
                sha256: "fixture".into(),
            };
            p.composer.attachments.push(attachment.clone());
            let mut skill = entry("review-code");
            skill.path = Some("/fixture/skills/review/SKILL.md".into());
            p.transcript.controls.skills = Some(vec![skill]);
            p.transcript.controls.commands = Some(vec![]);
            let mut h = harness(p, width, dark);
            h.run();
            h.get_by_label("명령 및 스킬").click();
            h.run();
            h.get_by_label("초안에 삽입").click();
            h.run();
            assert!(h.state().sent.is_empty());
            assert_eq!(h.state().composer.text, "keep 한글 draft\n$review-code ");
            assert_eq!(h.state().composer.attachments, vec![attachment.clone()]);
            assert_eq!(h.state().draft().composer.skills.len(), 1);
            h.get_by_label("보내기").click();
            h.run();
            assert!(
                matches!(h.state().sent.last(),Some(Command::SubmitWithSkills{text,attachments,skills,..}) if text=="keep 한글 draft\n$review-code " && attachments==&vec![attachment.clone()] && skills[0].path=="/fixture/skills/review/SKILL.md")
            );
        }
    }
}
#[test]
fn task6_unknown_empty_catalogs_and_model_choices_replace_actual_widgets() {
    let mut p = fixture(Tool::Antigravity);
    p.controls_ui.surface = Surface::Commands;
    let mut h = harness(p, 900., true);
    h.run();
    assert_eq!(
        h.query_all_by_label("목록을 아직 받지 못했습니다.").count(),
        2
    );
    h.state_mut().transcript.controls.commands = Some(vec![entry("compact")]);
    h.run();
    assert!(h.query_by_label("초안에 삽입").is_some());
    h.state_mut().transcript.controls.commands = Some(vec![]);
    h.run();
    assert!(h.query_by_label("초안에 삽입").is_none());
    assert!(h.query_by_label("제공된 항목이 없습니다.").is_some());
    h.state_mut().controls_ui.surface = Surface::Conversation;
    h.state_mut().transcript.controls.options_reported = true;
    h.state_mut().transcript.controls.options =
        vec![select("thought_level", "low", &["low", "high"])];
    h.run();
    h.get_by_label("추론 강도").click();
    h.run();
    assert!(h.query_by_label("high").is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    h.state_mut().transcript.controls.options =
        vec![select("thought_level", "medium", &["medium"])];
    h.run();
    h.get_by_label("추론 강도").click();
    h.run();
    assert!(h.query_by_label("high").is_none());
    assert!(h.query_by_label("medium").is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    h.state_mut().transcript.controls.options.clear();
    h.run();
    assert!(h.query_by_label("추론 강도").is_none());
}
#[test]
fn task6_launch_form_creates_typed_new_conversation_without_replay() {
    for tool in [Tool::Claude, Tool::Codex, Tool::Antigravity] {
        let p = fixture(tool);
        let mut initialized = false;
        let mut h = Harness::builder().with_size([320., 1200.]).build_ui_state(
            move |ui, p: &mut AgentPane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                p.launch_ui(ui);
            },
            p,
        );
        h.run();
        h.get_by_label("새 대화 옵션 지정").click();
        h.run();
        match tool {
            Tool::Claude => {
                h.get_by_label("최대 턴 수").click();
            }
            Tool::Codex => {
                h.get_by_label("컨텍스트 창").click();
            }
            Tool::Antigravity => {
                h.get_by_label("샌드박스 요청").click();
            }
        }
        h.run();
        h.get_by_label("새 대화 열기").click();
        h.run();
        let options = h.state_mut().take_launch_request().unwrap();
        assert!(options.validate(tool).is_ok());
        match options {
            LaunchOptions::Claude(v) => assert_eq!(v.max_turns, Some(1)),
            LaunchOptions::Codex(v) => assert_eq!(v.context_window, Some(1)),
            LaunchOptions::Antigravity(v) => assert!(v.sandbox),
            _ => panic!("typed launch missing"),
        }
        assert!(h.state().sent.is_empty());
        assert_eq!(h.state().composer.text, "keep 한글 draft");
        assert!(h.state_mut().take_launch_request().is_none());
    }
}
#[test]
fn task6_commands_and_settings_never_interpret_preedit_as_committed_slash() {
    let mut p = fixture(Tool::Claude);
    p.composer.text = "keep /com한".into();
    p.composing = true;
    p.committed_during_preedit = Some("keep /com".into());
    p.insert_entry(&entry("compact"), false, true);
    assert_eq!(p.composer.text, "keep /com한");
    assert_eq!(p.draft().composer.text, "keep /com");
    assert!(p.sent.is_empty());
    p.composing = false;
    p.ime_frame = false;
    p.composer.text = "keep /com".into();
    p.insert_entry(&entry("compact"), false, true);
    assert_eq!(p.composer.text, "keep /compact ");
    assert!(p.sent.is_empty());
}
#[test]
fn task6_restore_waits_for_actual_background_replay_and_keeps_stop() {
    let mut p = fixture(Tool::Codex);
    p.restore_draft(SavedDraft {
        composer: Draft {
            text: "restored".into(),
            ..Default::default()
        },
        ..Default::default()
    });
    p.submit();
    assert!(p.sent.is_empty());
    p.transcript.lifecycle = Lifecycle::Running;
    let mut h = harness(p, 320., true);
    h.run();
    h.get_by_label("중지").click();
    h.run();
    assert_eq!(h.state().sent, vec![Command::Interrupt]);
    assert!(h.state().replay_required);
}

#[test]
fn task6_locales_relabel_existing_tool_headers_and_approval_without_mutating_history() {
    use kiln_common::i18n::{Language, tr, with_language};
    for provider in [Tool::Claude, Tool::Codex, Tool::Antigravity] {
        for width in [320., 900.] {
            for dark in [false, true] {
                let mut p = fixture(provider);
                let mut tool = ToolCall {
                    id: "native-id".into(),
                    name: "AskUserQuestion".into(),
                    input: serde_json::json!({"question":"unchanged 한글?"}),
                    output: "exact output".into(),
                    diff: "+ exact diff".into(),
                    ..Default::default()
                };
                if provider == Tool::Codex {
                    tool.name = "kiln_qa / echo".into();
                    tool.input = serde_json::json!({"id":"native-id","type":"mcpToolCall","server":"kiln_qa","tool":"echo","arguments":{"text":"unchanged 한글"}});
                }
                if provider == Tool::Antigravity {
                    tool.name = "Reading fixture.txt".into();
                    tool.presentation = Some(ToolPresentation {
                        kind: Some("read".into()),
                        title: Some("Reading fixture.txt".into()),
                        native_name: Some("read_file".into()),
                    });
                }
                p.transcript.entries.push(Entry::Tool(tool.clone()));
                let before = serde_json::to_string(&tool).unwrap();
                let mut initialized = false;
                let mut h = Harness::builder().with_size([width, 480.]).build_ui_state(
                    move |ui, p: &mut AgentPane| {
                        if !initialized {
                            kiln_common::fonts::install(ui.ctx());
                            initialized = true;
                            return;
                        }
                        Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
                        Theme::current().apply(ui.ctx());
                        if let Entry::Tool(tool) = &p.transcript.entries[0] {
                            p.tool_ui(ui, tool);
                        }
                    },
                    p,
                );
                for language in Language::ALL {
                    with_language(language, || {
                        h.run_steps(3);
                        let title = super::super::approval::tool_title(&tool, provider);
                        let header_label = format!("{title} · {}", tr("대기 중"));
                        let header = h.get_by_label(&header_label);
                        assert!(header.rect().right() <= width);
                        header.click();
                        h.run();
                        assert!(h.query_by_label(&tool.name).is_some());
                        let after = match &h.state().transcript.entries[0] {
                            Entry::Tool(t) => serde_json::to_string(t).unwrap(),
                            _ => unreachable!(),
                        };
                        assert_eq!(before, after);
                        h.get_by_label(&format!("{title} · {}", tr("대기 중")))
                            .click();
                        h.run();
                    });
                }
            }
        }
    }
    for language in Language::ALL {
        with_language(language, || {
            let unknown = ToolCall {
                name: "mcp__private_server__unknown_operation".into(),
                ..Default::default()
            };
            assert_eq!(
                super::super::approval::tool_title(&unknown, Tool::Claude),
                format!("Private Server · {}", tr("외부 도구"))
            );
            let old:ToolCall=serde_json::from_value(serde_json::json!({"id":"old","name":"Reading something","input":{},"output":"","diff":"","status":"Completed"})).unwrap();
            assert_eq!(
                super::super::approval::tool_title(&old, Tool::Antigravity),
                tr("외부 도구")
            );
        });
    }
}

#[test]
fn task6_explicit_native_review_uses_typed_target() {
    let mut p = fixture(Tool::Codex);
    p.transcript.controls.capabilities = vec![NativeCapability {
        id: "review/start".into(),
        label: "Review".into(),
        scope: ControlScope::Live,
        available: true,
        description: String::new(),
        documentation: String::new(),
    }];
    let mut h = Harness::builder().with_size([320., 800.]).build_ui_state(
        |ui, p: &mut AgentPane| {
            p.actions_ui(ui);
        },
        p,
    );
    h.run();
    h.get_by_label("검토").click();
    h.run();
    assert!(h.state().sent.is_empty());
    h.get_by_label("검토 시작").click();
    h.run();
    assert_eq!(
        h.state().sent,
        vec![Command::NativeAction(NativeAction::Codex(
            CodexAction::Review(ReviewTarget::UncommittedChanges)
        ))]
    );
}

#[test]
fn task6_extension_state_is_localized_without_claiming_native_loaded() {
    use kiln_common::i18n::{Language, with_language};
    for language in Language::ALL {
        with_language(language, || {
            assert_ne!(settings_ui::extension_state("retained"), "retained");
            assert_ne!(
                settings_ui::extension_state("prepared"),
                settings_ui::extension_state("retained")
            );
            assert!(
                !settings_ui::extension_reason("PRIVATE_VERSION_RETAINED_FOR_READERS")
                    .contains("PRIVATE_")
            );
            assert!(!settings_ui::extension_reason("FUTURE_UNKNOWN_REASON").contains("FUTURE_"));
        });
    }
}

#[test]
fn task6_goal_queue_and_current_turn_forms_require_explicit_action() {
    let mut p = fixture(Tool::Codex);
    p.transcript.controls.capabilities = [
        "thread/goal/set",
        "thread/queue/add",
        "turn/settings/update",
    ]
    .into_iter()
    .map(|id| NativeCapability {
        id: id.into(),
        label: id.into(),
        scope: ControlScope::Live,
        available: true,
        description: String::new(),
        documentation: String::new(),
    })
    .collect();
    p.transcript.controls.queue = Some(vec![QueuedTask {
        id: "queue-1".into(),
        text: "queued fixture".into(),
        editable: true,
    }]);
    p.transcript.controls.options = vec![
        select("model", "m", &["m", "n"]),
        select("effort", "low", &["low", "high"]),
    ];
    let mut initialized = false;
    let mut h = Harness::builder().with_size([900., 1000.]).build_ui_state(
        move |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            p.actions_ui(ui);
        },
        p,
    );
    h.run();
    h.get_by_label("목표").click();
    h.run();
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .click();
    h.run();
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .type_text("explicit objective");
    h.run();
    assert!(h.state().sent.is_empty());
    h.get_by_label("목표 설정").click();
    h.run();
    assert_eq!(
        h.state().sent,
        vec![Command::NativeAction(NativeAction::Codex(
            CodexAction::SetGoal {
                objective: Some("explicit objective".into()),
                status: None,
                token_budget: None
            }
        ))]
    );
    h.state_mut().local_intents.clear();
    h.get_by_label("목표").click();
    h.run();
    h.get_by_label("대기 작업").click();
    h.run();
    h.state_mut().transcript.lifecycle = Lifecycle::Running;
    h.run();
    assert!(
        h.get_by_label("대기 작업 시작")
            .accesskit_node()
            .is_disabled()
    );
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .click();
    h.run();
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .type_text("queued while running");
    h.run();
    h.get_by_label("대기 작업 추가").click();
    h.run();
    assert_eq!(
        h.state().sent.last(),
        Some(&Command::NativeAction(NativeAction::Codex(
            CodexAction::AddQueuedTask {
                text: "queued while running".into()
            }
        )))
    );
    h.state_mut().local_intents.clear();
    h.state_mut().transcript.lifecycle = Lifecycle::Ready;
    h.run();
    h.get_by_label("대기 작업 시작").click();
    h.run();
    assert_eq!(
        h.state().sent.last(),
        Some(&Command::NativeAction(NativeAction::Codex(
            CodexAction::StartQueuedTask {
                id: "queue-1".into()
            }
        )))
    );
    h.state_mut().local_intents.clear();
    h.state_mut().transcript.lifecycle = Lifecycle::Running;
    h.state_mut().transcript.controls.status.insert(
        "active_turn_settings".into(),
        NativeStatus {
            subject: "active_turn_settings".into(),
            fields: BTreeMap::from([
                ("turnId".into(), ControlValue::Text("turn-exact".into())),
                ("model".into(), ControlValue::Text("m".into())),
            ]),
            unavailable: None,
        },
    );
    h.get_by_label("대기 작업").click();
    h.run();
    h.get_by_label("현재 턴 설정").click();
    h.run();
    // The control selector is a real keyboard-accessible native combo.
    let combo = h
        .query_all_by_role(egui::accesskit::Role::ComboBox)
        .next()
        .unwrap();
    combo.click();
    h.run();
    h.get_by_label("모델").click();
    h.run();
    h.get_by_label("현재 턴 설정 값").click();
    h.run();
    h.get_by_label("n").click();
    h.run();
    let before = h.state().sent.len();
    h.get_by_label("현재 턴에 적용").click();
    h.run();
    assert_eq!(h.state().sent.len(), before + 1);
    assert_eq!(
        h.state().sent.last(),
        Some(&Command::NativeAction(NativeAction::Codex(
            CodexAction::UpdateActiveTurn {
                turn_id: "turn-exact".into(),
                control: "model".into(),
                value: ControlValue::Text("n".into())
            }
        )))
    );
    assert_eq!(h.state().composer.text, "keep 한글 draft");
}

#[test]
fn task6_permission_selection_preserves_scope_and_cannot_reuse_changed_request() {
    let mut p = fixture(Tool::Codex);
    p.transcript.lifecycle = Lifecycle::Waiting;
    let request = Request {
        id: "permission-id".into(),
        title: "item/permissions/requestApproval".into(),
        questions: vec![],
        options: vec![],
        details: serde_json::json!({"permissions":{"fileSystem":{"read":["/fixture/read"],"write":["/fixture/write"]},"network":{"enabled":true}}}),
    };
    p.transcript
        .requests
        .insert(request.id.clone(), request.clone());
    let mut initialized = false;
    let mut h = Harness::builder().with_size([320., 800.]).build_ui_state(
        move |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            let r = p.transcript.requests.values().next().unwrap().clone();
            p.request_ui(ui, &r);
        },
        p,
    );
    h.run();
    assert!(h.query_by_label("한 번 허용").is_none());
    h.get_by_label("읽기 경로: /fixture/read").click();
    h.run();
    assert!(h.state().sent.is_empty());
    h.get_by_label("선택한 권한 허용").click();
    h.run();
    assert!(
        matches!(h.state().sent.last(),Some(Command::Respond{request_id,decision:Decision::Grant(g),..})if request_id=="permission-id" && g.read_indices==vec![0] && g.write_indices.is_empty() && !g.network && g.scope==PermissionScope::Turn && g.strict_review)
    );
    h.state_mut().responding.clear();
    h.state_mut()
        .transcript
        .requests
        .get_mut("permission-id")
        .unwrap()
        .details = serde_json::json!({"permissions":{"fileSystem":{"read":["/different"]}}});
    h.run();
    assert!(
        h.state().controls_ui.grants["permission-id"]
            .1
            .read_indices
            .is_empty()
    );
}

#[test]
fn task6_settings_search_preserves_unknown_options_scope_and_draft() {
    for width in [320., 900.] {
        for dark in [true, false] {
            let mut p = fixture(Tool::Antigravity);
            p.controls_ui.surface = Surface::Settings;
            p.transcript.controls.options_reported = true;
            let mut option = select("future_native_option", "one", &["one", "two"]);
            option.label = "Future native option".into();
            option.category = Some("new-category".into());
            p.transcript.controls.options = vec![option];
            let mut h = harness(p, width, dark);
            h.run();
            h.get_by_label("설정 검색").click();
            h.run();
            h.get_by_label("설정 검색").type_text("future");
            h.run();
            assert!(h.query_by_label("new-category").is_some());
            h.query_all_by_role(egui::accesskit::Role::ComboBox)
                .next()
                .unwrap()
                .click();
            h.run();
            h.get_by_label("two").click();
            h.run();
            assert_eq!(
                h.state().sent,
                vec![Command::Configure {
                    control: "future_native_option".into(),
                    value: ControlValue::Text("two".into())
                }]
            );
            assert_eq!(h.state().composer.text, "keep 한글 draft");
            assert!(h.state().settings_blocked());
        }
    }
}

#[test]
fn task6_terminal_only_feature_opens_separate_terminal_intent_without_sending() {
    let mut p = fixture(Tool::Claude);
    p.controls_ui.surface = Surface::Settings;
    let mut option = select("native-terminal-setting", "one", &["one"]);
    option.scope = ControlScope::Terminal;
    option.available = false;
    p.transcript.controls.options_reported = true;
    p.transcript.controls.options = vec![option];
    let mut h = harness(p, 900., true);
    h.run();
    h.get_by_label("터미널에서 새 대화").click();
    h.run();
    assert!(h.state_mut().take_terminal_request());
    assert!(!h.state_mut().take_terminal_request());
    assert_eq!(h.state().composer.text, "keep 한글 draft");
    assert!(h.state().sent.is_empty());
}

#[test]
fn task6_navigation_preserves_committed_ime_text_and_attachments() {
    for surface in ["대화 설정", "명령 및 스킬"] {
        for width in [320., 900.] {
            for dark in [true, false] {
                let mut p = fixture(Tool::Claude);
                p.composer.text.clear();
                let image = Attachment {
                    path: "/fixture/image".into(),
                    name: "draft.png".into(),
                    mime: "image/png".into(),
                    sha256: "fixture".into(),
                };
                p.composer.attachments.push(image.clone());
                let mut h = harness(p, width, dark);
                h.run();
                h.get_by_label("메시지").click();
                h.run();
                let preedit = |text: &str| {
                    egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: text.into(),
                        active_range_chars: Some(text.chars().count()..text.chars().count()),
                    })
                };
                h.input_mut().events.extend([
                    preedit("한"),
                    preedit(""),
                    egui::Event::Ime(egui::ImeEvent::Commit("한".into())),
                    preedit("글"),
                ]);
                h.run();
                assert_eq!(h.state().draft().composer.text, "한");
                assert!(h.state().composing);
                h.get_by_label(surface).click();
                h.run();
                assert_eq!(h.state().draft().composer.text, "한");
                assert_eq!(h.state().composer.attachments, vec![image]);
                assert!(h.state().sent.is_empty());
            }
        }
    }
}

#[test]
fn task6_settings_and_commands_start_at_top_instead_of_history_bottom() {
    let mut p = fixture(Tool::Claude);
    p.transcript.controls.options_reported = true;
    p.transcript.controls.options = vec![
        select("model", "one", &["one"]),
        select("effortLevel", "low", &["low"]),
        select("permissionMode", "default", &["default"]),
        select("ultracode", "off", &["off"]),
    ];
    p.transcript.controls.commands =
        Some((0..40).map(|i| entry(&format!("command-{i}"))).collect());
    p.transcript.entries = (0..80)
        .map(|i| {
            Entry::Message(Message {
                id: format!("message-{i}"),
                role: Role::Assistant,
                text: "history line".repeat(10),
                attachments: vec![],
            })
        })
        .collect();
    let mut h = harness(p, 900., true);
    h.run();
    h.get_by_label("대화 설정").click();
    h.run();
    let search = h.get_by_label("설정 검색").rect();
    assert!(
        search.top() > 60. && search.top() < 220.,
        "settings must start at its heading: {search:?}"
    );
    h.get_by_label("명령 및 스킬").click();
    h.run();
    let search = h.get_by_label("명령 및 스킬 검색").rect();
    assert!(
        search.top() > 60. && search.top() < 220.,
        "commands must start at its heading: {search:?}"
    );
}

fn fix1_form(p: AgentPane, launch: bool, width: f32, dark: bool) -> Harness<'static, AgentPane> {
    let mut initialized = false;
    Harness::builder().with_size([width, 1400.]).build_ui_state(
        move |ui, p: &mut AgentPane| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                initialized = true;
                return;
            }
            Theme::set_current(if dark { "kiln-dark" } else { "kiln-light" });
            Theme::current().apply(ui.ctx());
            if launch {
                p.launch_ui(ui);
            } else {
                let option = p.transcript.controls.options[0].clone();
                p.control_widget(ui, &option, false);
            }
        },
        p,
    )
}
fn fix1_input<'a>(h: &'a Harness<'_, AgentPane>, index: usize) -> egui_kittest::Node<'a> {
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .chain(h.query_all_by_role(egui::accesskit::Role::MultilineTextInput))
        .nth(index)
        .expect("editable native value")
}
fn fix1_two_lines(h: &mut Harness<'_, AgentPane>, index: usize, first: &str, second: &str) {
    fix1_input(h, index).click();
    h.run();
    fix1_input(h, index).type_text(first);
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    h.run_steps(2);
    assert_eq!(
        fix1_input(h, index).accesskit_node().value(),
        Some(format!("{first}\n"))
    );
    fix1_input(h, index).type_text(second);
    h.run();
    assert_eq!(
        fix1_input(h, index).accesskit_node().value(),
        Some(format!("{first}\n{second}"))
    );
    assert!(h.state().sent.is_empty());
    assert_eq!(h.state().composer.text, "keep 한글 draft");
}
#[test]
fn task6_fix1_launch_lists_accept_sequential_lines_before_explicit_launch() {
    for (tool, first, second) in [
        (Tool::Claude, "Read", "Grep"),
        (Tool::Codex, "/one", "/two"),
        (Tool::Antigravity, "/one", "/two"),
    ] {
        for (width, dark) in [(320., false), (900., true)] {
            let mut h = fix1_form(fixture(tool), true, width, dark);
            h.run();
            h.get_by_label("새 대화 옵션 지정").click();
            h.run();
            fix1_two_lines(&mut h, 0, first, second);
            assert!(h.state_mut().take_launch_request().is_none());
            h.get_by_label("새 대화 열기").click();
            h.run();
            let value = h
                .state_mut()
                .take_launch_request()
                .expect("explicit launch");
            match value {
                LaunchOptions::Claude(v) => assert_eq!(v.allowed_tools, [first, second]),
                LaunchOptions::Codex(v) => assert_eq!(
                    v.workspace_roots,
                    [PathBuf::from(first), PathBuf::from(second)]
                ),
                LaunchOptions::Antigravity(v) => assert_eq!(
                    v.additional_directories,
                    [PathBuf::from(first), PathBuf::from(second)]
                ),
                _ => panic!("typed launch"),
            }
        }
    }
}
#[test]
fn task6_fix1_live_lists_accept_sequential_lines_before_explicit_apply() {
    for sandbox in [false, true] {
        for (width, dark) in [(320., false), (900., true)] {
            let mut p = fixture(if sandbox { Tool::Codex } else { Tool::Claude });
            let mut option = select(
                if sandbox {
                    "sandboxPolicy"
                } else {
                    "fallbackModel"
                },
                "",
                &[],
            );
            option.kind = if sandbox {
                ControlKind::Sandbox
            } else {
                ControlKind::Strings
            };
            option.current = Some(if sandbox {
                ControlValue::Sandbox(NativeSandbox::WorkspaceWrite {
                    network_access: false,
                    writable_roots: vec![],
                    exclude_slash_tmp: false,
                    exclude_tmpdir_env_var: false,
                })
            } else {
                ControlValue::Strings(vec![])
            });
            p.transcript.controls.options = vec![option];
            let mut h = fix1_form(p, false, width, dark);
            h.run();
            fix1_two_lines(&mut h, 0, "/one", "/two");
            h.get_by_label("적용").click();
            h.run();
            let expected = if sandbox {
                ControlValue::Sandbox(NativeSandbox::WorkspaceWrite {
                    network_access: false,
                    writable_roots: vec!["/one".into(), "/two".into()],
                    exclude_slash_tmp: false,
                    exclude_tmpdir_env_var: false,
                })
            } else {
                ControlValue::Strings(vec!["/one".into(), "/two".into()])
            };
            assert_eq!(
                h.state().sent,
                vec![Command::Configure {
                    control: if sandbox {
                        "sandboxPolicy"
                    } else {
                        "fallbackModel"
                    }
                    .into(),
                    value: expected
                }]
            );
        }
    }
}
#[test]
fn task6_fix1_reported_default_remains_editable_without_implicit_configure() {
    for strings in [false, true] {
        let mut p = fixture(Tool::Claude);
        let mut option = select(if strings { "fallbackModel" } else { "language" }, "", &[]);
        option.source = Some("claude".into());
        option.kind = if strings {
            ControlKind::Strings
        } else {
            ControlKind::Text
        };
        option.current = Some(ControlValue::Default);
        p.transcript.controls.options = vec![option];
        let mut h = fix1_form(p, false, 320., true);
        h.run();
        assert!(h.state().sent.is_empty());
        assert_eq!(
            h.state().transcript.controls.options[0].current,
            Some(ControlValue::Default)
        );
        fix1_input(&h, 0).click();
        h.run();
        fix1_input(&h, 0).type_text("concrete");
        h.run();
        assert!(h.state().sent.is_empty());
        h.get_by_label("적용").click();
        h.run();
        assert_eq!(
            h.state().sent,
            vec![Command::Configure {
                control: if strings { "fallbackModel" } else { "language" }.into(),
                value: if strings {
                    ControlValue::Strings(vec!["concrete".into()])
                } else {
                    ControlValue::Text("concrete".into())
                }
            }]
        );
    }
}
#[test]
fn task6_fix1_acp_coarse_data_categories_are_truthful_in_four_locales() {
    use kiln_common::i18n::{Language, tr, with_language};
    for (kind, name, labels) in [
        (
            "read",
            "read_database",
            ["Read data", "데이터 읽기", "データを読む", "读取数据"],
        ),
        (
            "search",
            "search_contacts",
            ["Search information", "정보 검색", "情報を検索", "搜索信息"],
        ),
        (
            "fetch",
            "fetch_metrics",
            [
                "Fetch external data",
                "외부 데이터 가져오기",
                "外部データを取得",
                "获取外部数据",
            ],
        ),
    ] {
        let tool = ToolCall {
            name: "Original invocation".into(),
            input: serde_json::json!({"table":"contacts"}),
            presentation: Some(ToolPresentation {
                kind: Some(kind.into()),
                native_name: Some(name.into()),
                title: Some("Original invocation".into()),
            }),
            ..Default::default()
        };
        let before = serde_json::to_value(&tool).unwrap();
        let mut p = fixture(Tool::Antigravity);
        p.transcript.entries.push(Entry::Tool(tool.clone()));
        let mut h = harness(p, 320., true);
        for (language, expected) in [
            Language::English,
            Language::Korean,
            Language::Japanese,
            Language::ChineseSimplified,
        ]
        .into_iter()
        .zip(labels)
        {
            with_language(language, || {
                assert_eq!(
                    super::super::approval::tool_title(&tool, Tool::Antigravity),
                    expected
                );
                h.run_steps(3);
                let header = format!("{expected} · {}", tr("대기 중"));
                h.get_by_label(&header);
            });
        }
        assert_eq!(serde_json::to_value(&tool).unwrap(), before);
    }
}

fn fix1_readback(h: &mut Harness<'_, AgentPane>, value: ControlValue, kind: Option<ControlKind>) {
    h.state_mut().local_intents.clear();
    let mut option = h.state().transcript.controls.options[0].clone();
    option.current = Some(value);
    if let Some(kind) = kind {
        option.kind = kind;
    }
    let seq = h.state().transcript.sequence + 1;
    h.state_mut().apply(&Envelope {
        sequence: seq,
        generation: "fixture".into(),
        event: Event::Controls(ControlEvent::Options(vec![option])),
    });
    h.run();
}
#[test]
fn task6_fix1_default_reset_and_kind_changes_rebuild_independent_editors() {
    for strings in [false, true] {
        let mut p = fixture(Tool::Claude);
        let mut option = select(if strings { "fallbackModel" } else { "language" }, "", &[]);
        option.source = Some("claude".into());
        option.kind = if strings {
            ControlKind::Strings
        } else {
            ControlKind::Text
        };
        option.current = Some(if strings {
            ControlValue::Strings(vec!["old".into()])
        } else {
            ControlValue::Text("old".into())
        });
        p.transcript.controls.options = vec![option];
        let mut h = fix1_form(p, false, 320., false);
        h.run();
        h.get_by_label("네이티브 기본값으로 재설정").click();
        h.run();
        assert!(matches!(
            &h.state().sent[..],
            [Command::Configure {
                value: ControlValue::Default,
                ..
            }]
        ));
        fix1_readback(&mut h, ControlValue::Default, None);
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some(String::new())
        );
        fix1_input(&h, 0).click();
        h.run();
        fix1_input(&h, 0).type_text("again");
        h.run();
        assert_eq!(h.state().sent.len(), 1);
        h.get_by_label("적용").click();
        h.run();
        let expected = if strings {
            ControlValue::Strings(vec!["again".into()])
        } else {
            ControlValue::Text("again".into())
        };
        assert!(
            matches!(h.state().sent.last(), Some(Command::Configure {value,..}) if *value==expected)
        );
        // A replacement catalog must not reuse a Text editor for a Strings option (or vice versa).
        fix1_readback(
            &mut h,
            ControlValue::Default,
            Some(if strings {
                ControlKind::Text
            } else {
                ControlKind::Strings
            }),
        );
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some(String::new())
        );
        assert_eq!(h.state().sent.len(), 2);
        fix1_input(&h, 0).click();
        h.run();
        fix1_input(&h, 0).type_text("changed-kind");
        h.run();
        h.get_by_label("적용").click();
        h.run();
        let expected = if strings {
            ControlValue::Text("changed-kind".into())
        } else {
            ControlValue::Strings(vec!["changed-kind".into()])
        };
        assert!(
            matches!(h.state().sent.last(),Some(Command::Configure {value,..}) if *value==expected)
        );
    }
}
#[test]
fn task6_fix1_list_row_deletion_and_ime_cancellation_never_commit_preedit() {
    for form in [0, 1, 2] {
        let launch = form == 0;
        let mut p = fixture(if form == 2 { Tool::Codex } else { Tool::Claude });
        if !launch {
            let mut option = select(
                if form == 1 {
                    "fallbackModel"
                } else {
                    "sandboxPolicy"
                },
                "",
                &[],
            );
            option.kind = if form == 1 {
                ControlKind::Strings
            } else {
                ControlKind::Sandbox
            };
            option.current = Some(if form == 1 {
                ControlValue::Strings(vec![])
            } else {
                ControlValue::Sandbox(NativeSandbox::WorkspaceWrite {
                    network_access: false,
                    writable_roots: vec![],
                    exclude_slash_tmp: false,
                    exclude_tmpdir_env_var: false,
                })
            });
            p.transcript.controls.options = vec![option];
        }
        let mut h = fix1_form(p, launch, 320., true);
        h.run();
        if launch {
            h.get_by_label("새 대화 옵션 지정").click();
            h.run();
        }
        fix1_two_lines(&mut h, 0, "/one", "/two");
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Home);
        h.run();
        h.key_press(egui::Key::Backspace);
        h.run();
        h.key_press(egui::Key::Backspace);
        h.run();
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some("/one".into())
        );
        h.input_mut()
            .events
            .push(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "한".into(),
                active_range_chars: Some(1..1),
            }));
        h.run();
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some("/one한".into())
        );
        assert!(h.state().sent.is_empty());
        assert!(h.state_mut().take_launch_request().is_none());
        h.input_mut()
            .events
            .push(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: String::new(),
                active_range_chars: None,
            }));
        h.run();
        h.run_steps(2);
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some("/one".into())
        );
        // Clicking the action during preedit cancels focus composition, but cannot apply it.
        h.input_mut()
            .events
            .push(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "미완성".into(),
                active_range_chars: Some(3..3),
            }));
        h.run();
        h.get_by_label(if launch {
            "새 대화 열기"
        } else {
            "적용"
        })
        .click();
        h.run();
        assert!(h.state().sent.is_empty());
        assert!(h.state_mut().take_launch_request().is_none());
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some("/one".into())
        );
        h.get_by_label(if launch {
            "새 대화 열기"
        } else {
            "적용"
        })
        .click();
        h.run();
        if launch {
            assert!(
                matches!(h.state_mut().take_launch_request(),Some(LaunchOptions::Claude(v)) if v.allowed_tools==["/one"])
            );
        } else {
            match h.state().sent.last().expect("explicit Apply") {
                Command::Configure {
                    value: ControlValue::Strings(v),
                    ..
                } => assert_eq!(v, &["/one"]),
                Command::Configure {
                    value:
                        ControlValue::Sandbox(NativeSandbox::WorkspaceWrite { writable_roots, .. }),
                    ..
                } => assert_eq!(writable_roots, &[PathBuf::from("/one")]),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(h.state().composer.text, "keep 한글 draft");
    }
}

#[test]
fn task6_fix1_launch_validates_committed_path_buffer_without_losing_edits() {
    let mut h = fix1_form(fixture(Tool::Codex), true, 320., false);
    h.run();
    h.get_by_label("새 대화 옵션 지정").click();
    h.run();
    fix1_input(&h, 0).click();
    h.run();
    fix1_input(&h, 0).type_text("relative-path");
    h.run();
    h.get_by_label("새 대화 열기").click();
    h.run();
    assert!(h.state_mut().take_launch_request().is_none());
    assert_eq!(
        fix1_input(&h, 0).accesskit_node().value(),
        Some("relative-path".into())
    );
    fix1_input(&h, 0).click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    fix1_input(&h, 0).type_text("/valid-path");
    h.run();
    h.get_by_label("새 대화 열기").click();
    h.run();
    assert!(
        matches!(h.state_mut().take_launch_request(),Some(LaunchOptions::Codex(v)) if v.workspace_roots==[PathBuf::from("/valid-path")])
    );
    assert!(h.state().sent.is_empty());
    assert_eq!(h.state().composer.text, "keep 한글 draft");
}

#[test]
fn task6_visual_core_settings_fit_and_quick_labels_remain_readable() {
    fn text_shapes<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
        match shape {
            egui::Shape::Text(text) => out.push(text),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text_shapes(shape, out);
                }
            }
            _ => {}
        }
    }
    for dark in [false, true] {
        for width in [320., 1048.] {
            let mut p = fixture(Tool::Claude);
            p.controls_ui.surface = Surface::Settings;
            p.transcript.controls.options_reported = true;
            p.transcript.controls.options = vec![
                select(
                    "model",
                    "claude-opus-5-5",
                    &["claude-opus-5-5", "claude-sonnet-4"],
                ),
                select("effort", "medium", &["medium", "high"]),
                select("ultracode", "off", &["off", "on"]),
                select("permissionMode", "default", &["default", "plan"]),
            ];
            for option in &mut p.transcript.controls.options {
                option.source = Some("claude".into());
                option.documentation = Some("https://code.claude.com/docs/en/settings".into());
            }
            let mut h = harness(p, width, dark);
            h.run();
            let mut texts = vec![];
            for shape in &h.output().shapes {
                text_shapes(&shape.shape, &mut texts);
            }
            if width > 900. {
                for needle in ["모델: claude-opus-5-5", "추론 강도: medium"] {
                    let text = texts
                        .iter()
                        .find(|s| s.galley.text() == needle)
                        .expect("quick selected label painted");
                    assert!(
                        !text.galley.elided,
                        "wide quick value must be readable: {needle}"
                    );
                }
                let combos = h
                    .query_all_by_role(egui::accesskit::Role::ComboBox)
                    .collect::<Vec<_>>();
                let model = combos
                    .iter()
                    .find(|n| n.accesskit_node().label().as_deref() == Some("모델"))
                    .unwrap()
                    .rect();
                let mode = combos
                    .iter()
                    .find(|n| n.accesskit_node().label().as_deref() == Some("모드"))
                    .unwrap()
                    .rect();
                println!(
                    "settings width={width} dark={dark}: first={model:?} last={mode:?}, composer_top={}",
                    h.get_by_label("메시지").rect().top()
                );
                assert!(
                    mode.top() - model.top() < 210.,
                    "core controls must fit compact rows: {model:?} {mode:?}"
                );
                assert!(
                    mode.bottom() < h.get_by_label("메시지").rect().top(),
                    "all core values above composer"
                );
            }
            for combo in h.query_all_by_role(egui::accesskit::Role::ComboBox) {
                assert!(
                    combo.rect().right() <= width + 1.,
                    "selector overflows: {:?}",
                    combo.rect()
                );
            }
            assert_eq!(h.state().composer.text, "keep 한글 draft");
            assert!(h.state().sent.is_empty());
            h.query_all_by_label("세부 정보").next().unwrap().click();
            h.run();
            h.get_by_label("공식 문서");
            h.get_by_label("네이티브 기본값으로 재설정").click();
            h.run();
            assert!(
                matches!(&h.state().sent[..], [Command::Configure {control, value:ControlValue::Default}] if control=="model")
            );
            assert_eq!(h.state().composer.text, "keep 한글 draft");
        }
    }
}

#[test]
fn task6_visual_command_cards_are_compact_and_details_preserve_native_text() {
    for dark in [false, true] {
        for width in [320., 1048.] {
            let mut p = fixture(Tool::Claude);
            p.controls_ui.surface = Surface::Commands;
            p.transcript.controls.commands = Some(
                (0..6)
                    .map(|i| {
                        let mut e = entry(&format!("native-{i}"));
                        e.description =
                            "Long native description with complete searchable instructions. "
                                .repeat(12);
                        e.argument_hint = Some("   ".into());
                        e.origin = Some(format!("native-origin-{i}"));
                        e
                    })
                    .collect(),
            );
            let original = p.transcript.controls.commands.clone();
            let mut h = harness(p, width, dark);
            h.run();
            let insert = h.query_all_by_label("초안에 삽입").collect::<Vec<_>>();
            println!(
                "commands width={width} dark={dark}: row interval={}",
                insert[1].rect().top() - insert[0].rect().top()
            );
            assert!(
                insert[1].rect().top() - insert[0].rect().top() < 125.,
                "command rows too tall: {:?} {:?}",
                insert[0].rect(),
                insert[1].rect()
            );
            assert!(
                h.query_by_label("입력 안내:    ").is_none(),
                "blank hints must not consume a line"
            );
            if width > 900. {
                assert!(
                    insert[2].rect().bottom() < h.get_by_label("메시지").rect().top(),
                    "three commands visible above composer"
                );
            }
            h.query_all_by_label("세부 정보")
                .next()
                .expect("explicit details")
                .click();
            h.run();
            h.get_by_label("native-origin-0");
            assert_eq!(h.state().transcript.controls.commands, original);
            assert!(h.state().sent.is_empty());
            h.key_press(egui::Key::Escape);
            h.run();
            h.query_all_by_label("초안에 삽입").next().unwrap().click();
            h.run();
            assert!(h.state().composer.text.ends_with("/native-0 "));
            assert!(h.state().sent.is_empty());
        }
    }
}
