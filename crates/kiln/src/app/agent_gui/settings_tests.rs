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
fn open_composer_options(h: &mut Harness<'_, AgentPane>) {
    let compact = h.query_by_label(kiln_common::i18n::tr("대화 옵션")).is_some();
    let name = kiln_common::i18n::tr(if compact { "대화 옵션" } else { "사고 설정" });
    if let Some(button) = h.query_by_label(name) { button.click(); h.run(); }
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
            open_composer_options(&mut h);
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
            h.get_by_label("명령 및 스킬 검색").click();
            h.run();
            h.get_by_label("명령 및 스킬 검색").type_text("review-code");
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
fn task6_native_catalogs_replace_widgets_without_removing_gui_commands() {
    let mut p = fixture(Tool::Antigravity);
    p.controls_ui.surface = Surface::Commands;
    let mut h = harness(p, 900., true);
    h.run();
    assert_eq!(
        h.query_all_by_label("목록을 아직 받지 못했습니다.").count(),
        1
    );
    h.get_by_label("/mcp");
    h.state_mut().transcript.controls.commands = Some(vec![entry("fixture-provider-command")]);
    h.run();
    h.get_by_label("/fixture-provider-command");
    h.state_mut().transcript.controls.commands = Some(vec![]);
    h.state_mut().transcript.controls.skills = Some(vec![]);
    h.run();
    assert!(h.query_by_label("/fixture-provider-command").is_none());
    h.get_by_label("/mcp");
    assert!(h.query_by_label("제공된 항목이 없습니다.").is_some());
    h.state_mut().controls_ui.surface = Surface::Conversation;
    h.state_mut().transcript.controls.options_reported = true;
    h.state_mut().transcript.controls.options =
        vec![select("thought_level", "low", &["low", "high"])];
    h.run();
    open_composer_options(&mut h);
    assert_eq!(h.get_by_label("추론 강도").accesskit_node().role(), egui::accesskit::Role::ComboBox);
    assert!(h.query_by_label("추론 강도").is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    h.state_mut().transcript.controls.options =
        vec![select("thought_level", "medium", &["medium"])];
    h.run();
    open_composer_options(&mut h);
    assert!(h.query_by_label("추론 강도").is_some());
    assert!(h.query_by_label("추론 강도").is_some(), "a single supported level remains readable with its description");
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
            assert!(h.query_by_label("추가 설정").is_some());
            h.query_all_by_role(egui::accesskit::Role::ComboBox)
                .next()
                .unwrap()
                .click();
            h.run();
            h.get_by_label("사용자 정의 · two").click();
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
fn task6_terminal_only_feature_is_hidden_from_gui_without_sending() {
    let mut p = fixture(Tool::Claude);
    p.controls_ui.surface = Surface::Settings;
    let mut option = select("native-terminal-setting", "one", &["one"]);
    option.scope = ControlScope::Terminal;
    option.available = false;
    p.transcript.controls.options_reported = true;
    p.transcript.controls.options = vec![option];
    let mut h = harness(p, 900., true);
    h.run();
    assert!(h.query_by_label("터미널 전용").is_none());
    assert!(h.query_by_label("native-terminal-setting").is_none());
    assert!(h.query_by_label("터미널에서 새 대화").is_none());
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

// These fields require absolute paths, including a drive prefix on Windows.
fn fix1_absolute(name: &str) -> String {
    if cfg!(windows) {
        format!("C:/{name}")
    } else {
        format!("/{name}")
    }
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
    let one = fix1_absolute("one");
    let two = fix1_absolute("two");
    for (tool, first, second) in [
        (Tool::Claude, "Read", "Grep"),
        (Tool::Codex, one.as_str(), two.as_str()),
        (Tool::Antigravity, one.as_str(), two.as_str()),
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
    let one = fix1_absolute("one");
    let two = fix1_absolute("two");
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
            fix1_two_lines(&mut h, 0, &one, &two);
            h.get_by_label("적용").click();
            h.run();
            let expected = if sandbox {
                ControlValue::Sandbox(NativeSandbox::WorkspaceWrite {
                    network_access: false,
                    writable_roots: vec![one.clone().into(), two.clone().into()],
                    exclude_slash_tmp: false,
                    exclude_tmpdir_env_var: false,
                })
            } else {
                ControlValue::Strings(vec![one.clone().into(), two.clone().into()])
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
    let one = fix1_absolute("one");
    let two = fix1_absolute("two");
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
        fix1_two_lines(&mut h, 0, &one, &two);
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Home);
        h.run();
        h.key_press(egui::Key::Backspace);
        h.run();
        h.key_press(egui::Key::Backspace);
        h.run();
        assert_eq!(
            fix1_input(&h, 0).accesskit_node().value(),
            Some(one.clone())
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
            Some(format!("{one}한"))
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
            Some(one.clone())
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
            Some(one.clone())
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
                matches!(h.state_mut().take_launch_request(),Some(LaunchOptions::Claude(v)) if v.allowed_tools==[one.clone()])
            );
        } else {
            match h.state().sent.last().expect("explicit Apply") {
                Command::Configure {
                    value: ControlValue::Strings(v),
                    ..
                } => assert_eq!(v, &[one.clone()]),
                Command::Configure {
                    value:
                        ControlValue::Sandbox(NativeSandbox::WorkspaceWrite { writable_roots, .. }),
                    ..
                } => assert_eq!(writable_roots, &[PathBuf::from(&one)]),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(h.state().composer.text, "keep 한글 draft");
    }
}

#[test]
fn task6_fix1_launch_validates_committed_path_buffer_without_losing_edits() {
    let valid = fix1_absolute("valid-path");
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
    fix1_input(&h, 0).type_text(&valid);
    h.run();
    h.get_by_label("새 대화 열기").click();
    h.run();
    assert!(
        matches!(h.state_mut().take_launch_request(),Some(LaunchOptions::Codex(v)) if v.workspace_roots==[PathBuf::from(&valid)])
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
            assert!(h.query_all_by_role(egui::accesskit::Role::ComboBox).any(|n| n.accesskit_node().label().as_deref() == Some("추론 강도")));
            assert!(texts.iter().any(|s| s.galley.text() == "Opus 5.5"));
            assert_eq!(h.query_all_by_role(egui::accesskit::Role::Slider).count(), 0, "settings use described choices for every effort level");
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

#[test]
fn ux_localized_mode_values_preserve_native_identifiers() {
    use kiln_common::i18n::{Language, with_language};
    for (language, expected) in [(Language::Korean,"자동"),(Language::English,"Auto"),(Language::Japanese,"自動"),(Language::ChineseSimplified,"自动")] {
        with_language(language, || {
            let o = select("permissionMode", "auto", &["auto", "plan"]);
            let mut h = harness(fixture(Tool::Claude), 520., true);
            h.state_mut().transcript.controls.options_reported = true;
            h.state_mut().transcript.controls.options = vec![o];
            h.run();
            open_composer_options(&mut h);
            assert!(h.get_by_label(kiln_common::i18n::tr("모드")).accesskit_node().value().unwrap().contains(expected));
            assert_eq!(h.state().transcript.controls.options[0].current,Some(ControlValue::Text("auto".into())));
            assert!(h.state().sent.is_empty());
        });
    }
}

#[test]
fn ux_effort_selection_commits_once_and_unknown_never_autowrites() {
    let mut p=fixture(Tool::Claude);
    p.transcript.controls.options_reported=true;
    p.transcript.controls.options=vec![select("effortLevel","low", &["low","medium","high","xhigh","max"])];
    let mut h=harness(p,900.,true);h.run();
    open_composer_options(&mut h);
    h.get_by_label("추론 강도").click();h.run();
    assert!(h.state().sent.is_empty(),"opening the list never changes the provider");
    h.get_by_label("최대").click();h.run();
    assert_eq!(h.state().sent,vec![Command::Configure{control:"effortLevel".into(),value:ControlValue::Text("max".into())}]);
    let mut p=fixture(Tool::Claude);p.transcript.controls.options_reported=true;
    let mut option=select("effortLevel","future-level", &["low","high"]);option.current=None;
    p.transcript.controls.options=vec![option];
    let mut h=harness(p,520.,false);h.run();assert!(h.state().sent.is_empty());
}

#[test]
fn ux_ultracode_is_an_independent_toggle_and_terminal_controls_are_hidden() {
    let mut p=fixture(Tool::Claude);p.transcript.controls.options_reported=true;
    let mut toggle=select("ultracode","", &[]);toggle.kind=ControlKind::Boolean;toggle.current=Some(ControlValue::Bool(false));
    p.transcript.controls.options=vec![toggle, select("effortLevel","high", &["low","high"])];
    let mut h=harness(p,520.,true);h.run();open_composer_options(&mut h);h.get_by_label("Ultracode").click();h.run();
    assert_eq!(h.state().sent,vec![Command::Configure{control:"ultracode".into(),value:ControlValue::Bool(true)}]);
    assert_eq!(h.state().transcript.controls.options[1].current,Some(ControlValue::Text("high".into())));
    assert_eq!(h.state().composer.text,"keep 한글 draft");
}

#[test]
fn ux_visual_review_matrix() {
    for dark in [true,false] { for width in [320.,520.,1100.] {
        let mut p=fixture(Tool::Claude);p.composer.text=String::new();p.transcript.selected_model=Some("claude-opus-5-5".into());
        p.transcript.controls.options_reported=true;
        let mut model=select("model","claude-opus-5-5", &["default","opus","claude-fable-5-2[1m]","sonnet","haiku"]);
        for c in &mut model.choices { c.label=match c.value.as_str(){"default"=>"기본 모델","opus"=>"Opus 5.5","claude-fable-5-2[1m]"=>"Fable 5.2","sonnet"=>"Sonnet 5.5",_=>"Haiku 5.5"}.into();c.description="Native model description · coding tasks".into(); }
        let mut toggle=select("ultracode","", &[]);toggle.kind=ControlKind::Boolean;toggle.current=Some(ControlValue::Bool(true));
        p.transcript.controls.options=vec![model,select("effortLevel","high", &["low","medium","high","xhigh","max"]),select("permissionMode","auto", &["default","plan","acceptEdits","auto","dontAsk"]),toggle];
        p.transcript.controls.models=vec![ModelMetadata{id:"opus".into(),display_name:"Opus 5.5".into(),resolved_model:Some("claude-opus-5-5".into()),..Default::default()}];
        p.transcript.controls.commands=Some((0..30).map(|i|{let mut e=entry(&format!("command-{i:02}"));e.description="Review the workspace and explain the changes before applying them.".into();e}).collect());
        p.transcript.controls.skills=Some(vec![entry("code-review"),entry("frontend-design")]);
        let mut h=harness(p,width,dark);h.run_steps(3);
        for (surface,name) in [(Surface::Conversation,"conversation"),(Surface::Settings,"settings"),(Surface::Commands,"commands")] {
            h.state_mut().controls_ui.surface=surface;h.run_steps(3);
            let frame=h.state().composer_geometry.unwrap();assert!(frame.1.contains_rect(frame.0),"composer fully visible {name} {width} {frame:?}");
            let rendered=h.render().unwrap();
            if let Some(dir)=std::env::var_os("KILN_VISUAL_CAPTURE_DIR"){let dir=std::path::PathBuf::from(dir);std::fs::create_dir_all(&dir).unwrap();rendered.save(dir.join(format!("ux-{name}-{}-{width:.0}.png",if dark{"dark"}else{"light"}))).unwrap();}
        }
        h.state_mut().controls_ui.surface=Surface::Conversation;h.run_steps(3);
        open_composer_options(&mut h);
        assert!(h.query_by_label("추론 강도").is_some());
        assert!(h.query_by_label("Ultracode").is_some());
        if let Some(dir)=std::env::var_os("KILN_VISUAL_CAPTURE_DIR") { h.render().unwrap().save(std::path::PathBuf::from(dir).join(format!("ux-options-{}-{width:.0}.png",if dark{"dark"}else{"light"}))).unwrap(); }
        h.key_press(egui::Key::Escape);h.run_steps(3);
        h.state_mut().composer.text="/command".into();h.state_mut().focus=true;h.run_steps(3);
        if let Some(dir)=std::env::var_os("KILN_VISUAL_CAPTURE_DIR") { h.render().unwrap().save(std::path::PathBuf::from(dir).join(format!("ux-completion-{}-{width:.0}.png",if dark{"dark"}else{"light"}))).unwrap(); }
        assert!(h.state().sent.is_empty());
    }}
}

#[test]
fn ux_settings_search_never_exposes_native_schema_or_noninteractive_controls() {
    for (id,scope,available,kind) in [("$schema",ControlScope::Inherited,false,ControlKind::Unsupported("schema".into())),("apiKeyHelper",ControlScope::Inherited,false,ControlKind::Text),("no_gui_support",ControlScope::Live,false,ControlKind::Text),("unknown_widget",ControlScope::Live,true,ControlKind::Unsupported("future".into()))] {
        let mut p=fixture(Tool::Claude);p.controls_ui.surface=Surface::Settings;p.controls_ui.search=id.into();
        let mut o=select(id,"",&[]);o.scope=scope;o.available=available;o.kind=kind;
        p.transcript.controls.options_reported=true;p.transcript.controls.options=vec![o];
        let mut h=harness(p,900.,true);h.run();
        assert!(h.query_by_label(id).is_none(),"noninteractive inventory must not be presented as settings: {id}");
        assert!(h.query_by_label("확인 대기").is_none());
    }
}

#[test]
fn inflight_composer_exposes_send_and_stop_and_sends_without_interrupt() {
    for tool in [Tool::Claude, Tool::Codex, Tool::Antigravity] {
        for state in [Lifecycle::Running, Lifecycle::Waiting] {
            let mut p = fixture(tool);
            p.transcript.lifecycle = state;
            let mut h = harness(p, 420., true);
            h.run();
            assert!(!h.get_by_label("보내기").accesskit_node().is_disabled());
            assert!(!h.get_by_label("중지").accesskit_node().is_disabled());
            let send = h.get_by_label("보내기").rect();
            let stop = h.get_by_label("중지").rect();
            assert!(!send.intersects(stop));
            if let Some(dir) = std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {
                std::fs::create_dir_all(&dir).unwrap(); h.render().unwrap().save(std::path::PathBuf::from(dir).join(format!("inflight-{:?}-{:?}.png", tool, h.state().transcript.lifecycle))).unwrap();
            }
            h.get_by_label("보내기").click();
            h.run();
            assert_eq!(h.state().sent.len(), 1);
            assert!(matches!(&h.state().sent[0], Command::Submit{text,..} if text=="keep 한글 draft"));
            assert!(h.state().pending.is_some());
        }
    }
}

#[test]
fn inflight_settings_preserve_draft_and_keep_handoff_and_destructive_actions_gated() {
    for state in [Lifecycle::Running, Lifecycle::Waiting] {
        for control in ["model", "effort", "permissionMode", "collaborationMode"] {
            let mut p = fixture(Tool::Codex);
            p.transcript.lifecycle = state.clone();
            let option = select(control, "first", &["first", "second"]);
            p.configure(&option, ControlValue::Text("second".into()));
            assert_eq!(p.sent.len(), 1);
            assert_eq!(p.composer.text, "keep 한글 draft");
            assert!(!p.editing_allowed());
            assert!(matches!(&p.sent[0],Command::Configure{control:key,..} if key==control));
        }
    }
}

#[test]
fn inflight_model_selector_is_clickable_in_the_actual_composer() {
    let mut p = fixture(Tool::Claude);
    p.transcript.lifecycle = Lifecycle::Running;
    p.transcript.controls.options_reported = true;
    p.transcript.controls.options = vec![select("model", "old", &["old", "new"])];
    let mut h = harness(p, 320., true);
    h.run();
    h.get_by_label("모델").click(); h.run();
    h.get_by_label("new").click(); h.run();
    assert!(matches!(h.state().sent.as_slice(), [Command::Configure {control, value: ControlValue::Text(value)}] if control=="model" && value=="new"));
    assert_eq!(h.state().composer.text, "keep 한글 draft");
}

#[test]
fn inflight_enter_sends_without_stopping_the_current_response() {
    let mut p = fixture(Tool::Claude);
    p.transcript.lifecycle = Lifecycle::Running;
    let mut h = harness(p, 320., false);
    h.run();
    h.get_by_label("메시지").click(); h.run();
    h.key_press(egui::Key::Enter); h.run();
    assert!(matches!(h.state().sent.as_slice(), [Command::Submit {..}]));
}

#[test]
fn inflight_settings_remain_editable_while_the_first_message_receipt_is_pending() {
    let mut p = fixture(Tool::Claude);
    p.submit();
    assert!(p.pending.is_some());
    p.transcript.lifecycle = Lifecycle::Running;
    p.configure(&select("model", "first", &["first", "second"]), ControlValue::Text("second".into()));
    assert!(matches!(p.sent.as_slice(), [Command::Submit{..}, Command::Configure{..}]));
    assert!(p.pending.is_some());
}

#[test]
fn described_choices_all_providers_keep_wire_values_and_show_explanations() {
    for (tool, id, old, new, title, help) in [
        (Tool::Codex, "approvalPolicy", "never", "on-request", "필요할 때 승인 요청", "AI가 허용 범위를 넘는 작업에 대해 승인을 요청할 수 있습니다."),
        (Tool::Codex, "permissions", ":read-only", ":workspace", "작업 폴더에 쓰기", "작업 폴더와 추가로 허용한 경로에 파일을 쓸 수 있습니다."),
        (Tool::Claude, "permissionMode", "default", "dontAsk", "승인 요청 자동 거절", "이미 허용된 작업은 실행하고, 추가 승인이 필요한 작업은 묻지 않고 거절합니다."),
        (Tool::Antigravity, "custom-mode", "ask", "code", "코딩", "코드를 작성하고 수정하는 작업 모드입니다."),
    ] {
        for width in [320., 760.] {
            let mut p = fixture(tool);
            p.transcript.controls.options_reported = true;
            let mut option = select(id, old, &[old, new]);
            if tool == Tool::Antigravity { option.category = Some("mode".into()); option.label = "Session Mode".into(); }
            let mut initialized = false;
            let mut h = Harness::builder().with_size([width, 480.]).build_ui_state(move |ui, p: &mut AgentPane| {
                if !initialized { kiln_common::fonts::install(ui.ctx()); initialized = true; return; }
                p.control_widget(ui, &option, false);
            }, p);
            h.run();
            h.query_all_by_role(egui::accesskit::Role::ComboBox).next().unwrap().click();h.run();
            let mut text=String::new();
            fn collect(shape:&egui::Shape,text:&mut String) {match shape {egui::Shape::Text(t)=>text.push_str(t.galley.text()),egui::Shape::Vec(v)=>for s in v {collect(s,text)},_=>{}}}
            for shape in &h.output().shapes {collect(&shape.shape,&mut text);}
            assert!(text.contains(help),"visible explanation for {tool:?} at {width}: {text}");
            let row = h.get_by_label(title);assert!(row.rect().right()<=width+1., "popup at {width}: {:?}", row.rect());row.click();h.run();
            assert_eq!(h.state().sent,vec![Command::Configure{control:id.into(),value:ControlValue::Text(new.into())}]);
            assert_eq!(h.state().composer.text,"keep 한글 draft");
        }
    }
}

/// Reproducible public illustrations, always synthetic and isolated from real sessions.
#[test]
#[ignore = "explicit site illustration export"]
fn export_agent_site_illustrations() {
    use kiln_common::i18n::{Language, with_language};
    let dir = PathBuf::from(std::env::var_os("KILN_VISUAL_CAPTURE_DIR").expect("explicit output directory"));
    std::fs::create_dir_all(&dir).unwrap();
    for (lang, name, account, prompt, answer) in [
        (Language::Korean, "ko", "개인 계정", "인사말 예제 코드를 검토해 주세요", "문자열과 출력 코드를 확인했습니다. 예제 코드는 실행할 준비가 되었습니다."),
        (Language::English, "en", "Personal account", "Review the greeting example", "The string and output code look correct. The example is ready to run."),
        (Language::Japanese, "ja", "個人アカウント", "挨拶のサンプルコードを確認してください", "文字列と出力コードを確認しました。サンプルを実行できます。"),
        (Language::ChineseSimplified, "zh-CN", "个人账户", "请检查问候语示例代码", "已检查字符串和输出代码。示例已准备好运行。"),
    ] {
        with_language(lang, || {
            let mut p=fixture(Tool::Claude);p.descriptor.account_label=account.into();p.composer.text.clear();
            p.transcript.selected_model=Some("claude-opus-5-5".into());p.transcript.selected_mode=Some("auto".into());
            p.transcript.controls.options_reported=true;
            let mut model=select("model","opus", &["opus","fable","sonnet"]);model.choices[0].label="Opus 5.5".into();model.choices[1].label="Fable 5.2".into();model.choices[2].label="Sonnet 5.5".into();
            let mut ultra=select("ultracode","", &[]);ultra.kind=ControlKind::Boolean;ultra.current=Some(ControlValue::Bool(true));
            p.transcript.controls.options=vec![model,select("effortLevel","high", &["low","medium","high","xhigh","max"]),select("permissionMode","auto", &["default","plan","acceptEdits","auto","dontAsk"]),ultra];
            p.transcript.entries=vec![
                Entry::Message(Message{id:"site-user".into(),role:Role::User,text:prompt.into(),attachments:vec![]}),
                Entry::Tool(ToolCall{id:"site-read".into(),name:"Read".into(),status:ToolStatus::Completed,input:serde_json::json!({"file_path":"src/main.rs"}),output:"fn main() {\n    let greeting = \"Hello, Kiln!\";\n    println!(\"{greeting}\");\n}\n".into(),..Default::default()}),
                Entry::Message(Message{id:"site-assistant".into(),role:Role::Assistant,text:answer.into(),attachments:vec![]}),
            ];
            let mut h=harness(p,1280.,true);h.run();
            h.render().unwrap().save(dir.join(format!("agents-{name}.png"))).unwrap();
            h.state_mut().transcript.lifecycle=Lifecycle::Running;
            h.state_mut().transcript.activity.apply(&super::super::activity::parse(&serde_json::json!({"subtype":"background_tasks_changed","tasks":[{"task_id":"site-shell","task_type":"local_bash","description":"python -m unittest tests/example.py"},{"task_id":"site-monitor","task_type":"monitor","description":"Watch test output","ambient":true}]})).unwrap());
            h.state_mut().history_end=Some(2);h.run();
            h.render().unwrap().save(dir.join(format!("agent-activity-{name}.png"))).unwrap();
            h.state_mut().transcript.lifecycle=Lifecycle::Ready;
            h.state_mut().transcript.activity=Default::default();h.state_mut().history_end=None;
            h.state_mut().controls_ui.surface=Surface::Settings;h.run();
            h.render().unwrap().save(dir.join(format!("agent-settings-{name}.png"))).unwrap();
        });
    }
}

#[test]
fn activity_strip_shows_shells_monitors_and_jumps_to_latest_without_changing_draft() {
    use kiln_common::i18n::{Language, with_language, tr};
    for lang in [Language::Korean,Language::English,Language::Japanese,Language::ChineseSimplified] {
      with_language(lang,||{
       for width in [320.,760.] { for dark in [true,false] {
        let mut p=fixture(Tool::Claude);p.transcript.lifecycle=Lifecycle::Running;
        p.transcript.entries=(0..80).map(|i|Entry::Message(Message{id:format!("history-{i}"),role:if i%2==0{Role::User}else{Role::Assistant},text:format!("Example message {i}: review the isolated fixture and report the result."),attachments:vec![]})).collect();
        p.transcript.activity.apply(&super::super::activity::parse(&serde_json::json!({"subtype":"background_tasks_changed","tasks":[{"task_id":"shell","task_type":"local_bash","description":"python -m unittest tests/example.py"},{"task_id":"watch","task_type":"monitor","description":"Watch test output","ambient":true}]})).unwrap());
        p.history_end=Some(30);
        let mut h=harness(p,width,dark);h.run();
        assert!(h.query_by_label(tr("응답 작성 중")).is_some());
        let button=h.query_by_label(tr("↓ 최신 메시지로 이동")).expect("jump action is fixed outside scrolled history");
        button.click();h.run();assert!(h.state().history_end.is_none());assert!(!h.state().activity_ui.away);assert!(h.state().activity_ui.offset>100.);
        assert_eq!(h.state().composer.text,"keep 한글 draft");assert!(h.state().sent.is_empty());
        // A real wheel scroll in the transcript keeps the reader in place and reveals the shortcut.
        h.event(egui::Event::PointerMoved(egui::pos2(width/2.,190.)));
        h.event(egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,phase:egui::TouchPhase::Move,delta:egui::vec2(0.,500.),modifiers:egui::Modifiers::NONE});h.run();
        assert!(h.state().activity_ui.away);
        h.get_by_label(tr("↓ 최신 메시지로 이동")).click();h.run();assert!(!h.state().activity_ui.away);
        if let Some(dir)=std::env::var_os("KILN_VISUAL_CAPTURE_DIR") {let dir=PathBuf::from(dir);std::fs::create_dir_all(&dir).unwrap();h.render().unwrap().save(dir.join(format!("activity-{}-{width}-{dark}.png",lang.code()))).unwrap();}
        h.state_mut().transcript.lifecycle=Lifecycle::Ready;h.run();assert!(h.query_by_label(tr("응답 완료 · 백그라운드 작업 실행 중")).is_some());
        h.state_mut().transcript.activity.apply(&super::super::activity::parse(&serde_json::json!({"subtype":"background_tasks_changed","tasks":[{"task_id":"watch","task_type":"monitor","description":"Watch test output","ambient":true}]})).unwrap());h.run();assert!(h.query_by_label(tr("응답 완료")).is_some());
       }}
      });
    }
}

#[test]
fn activity_task_output_is_a_read_only_request_with_the_confirmed_task_identity() {
    let mut p=fixture(Tool::Claude);p.transcript.activity.apply(&super::super::activity::parse(&serde_json::json!({"subtype":"background_tasks_changed","tasks":[{"task_id":"owned-shell","run_id":"run-2","task_type":"local_bash","description":"watch-tests"}]})).unwrap());
    let mut h=harness(p,760.,true);h.run();h.get_by_label("셸 · watch-tests").click();h.run();
    h.get_by_label("출력 보기 · 새로고침").click();h.run();
    assert_eq!(h.state().sent,vec![Command::Discover(Discovery::TaskOutput{task_id:"owned-shell".into(),run_id:"run-2".into()})]);
    assert_eq!(h.state().composer.text,"keep 한글 draft");
    h.state_mut().transcript.activity.apply(&super::super::activity::Event::Output{task_id:"owned-shell".into(),run_id:"run-2".into(),text:"waiting for test output".into(),truncated:true,unavailable:false});h.run();
    assert!(h.query_by_label("waiting for test output").is_some());assert!(h.query_by_label("출력의 마지막 부분을 표시합니다.").is_some());
}

#[test]
fn activity_without_a_native_task_inventory_does_not_invent_zero_shell_counts() {
    let mut p=fixture(Tool::Codex);p.transcript.lifecycle=Lifecycle::Running;
    p.transcript.entries.push(Entry::Tool(ToolCall{id:"running-command".into(),name:"commandExecution".into(),input:serde_json::json!({"command":"cargo test"}),status:ToolStatus::Running,..Default::default()}));
    let mut h=harness(p,760.,true);h.run();assert!(h.query_by_label("도구 실행 중: 1").is_some());assert!(h.query_by_label("셸 0 · Monitor 0 · 기타 작업 0").is_none());
}
