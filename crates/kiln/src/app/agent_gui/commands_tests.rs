use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn entry(name: &str, description: &str) -> NativeEntry {
    NativeEntry {
        name: name.into(),
        description: description.into(),
        argument_hint: Some("exact target".into()),
        path: None,
        terminal_only: false,
        interface: BTreeMap::new(),
        builtin: Some(false),
        enabled: Some(true),
        scope: Some("project".into()),
        origin: Some(format!("origin-{name}")),
        model: None,
        aliases: vec![],
    }
}
fn pane(tool: Tool) -> AgentPane {
    let mut pane = AgentPane::fixture(Descriptor {
        id: "command-ux-fixture".into(),
        tool,
        cwd: "/fixture".into(),
        account_label: "Fixture".into(),
    });
    pane.transcript.lifecycle = Lifecycle::Ready;
    pane.transcript.generation = "fixture".into();
    pane.transcript.controls.commands = Some(vec![
        entry("compact", "Summarize the conversation"),
        entry("review", "Review project changes"),
    ]);
    pane.transcript.controls.skills = Some(vec![]);
    pane
}
fn harness(pane: AgentPane, width: f32, height: f32) -> Harness<'static, AgentPane> {
    let mut initialized = false;
    Harness::builder()
        .with_size([width, height])
        .build_ui_state(
            move |ui, pane| {
                if !initialized {
                    kiln_common::fonts::install(ui.ctx());
                    initialized = true;
                    return;
                }
                Theme::set_current("kiln-dark");
                Theme::current().apply(ui.ctx());
                pane.ui(ui);
            },
            pane,
        )
}
fn focus_editor(h: &mut Harness<'_, AgentPane>) {
    h.get_by_label("메시지").focus();
    h.run();
}

#[test]
fn command_ux_search_clear_and_expand_hide_terminal_items_without_mutating_catalog() {
    for width in [320., 900.] {
        let mut p = pane(Tool::Codex);
        p.controls_ui.surface = Surface::Commands;
        let mut terminal = entry("terminal-secret", "Terminal-only fixture");
        terminal.terminal_only = true;
        p.transcript
            .controls
            .commands
            .as_mut()
            .unwrap()
            .push(terminal);
        p.transcript.controls.skills =
            Some(vec![entry("missing-path", "Cannot be invoked in GUI")]);
        let original = p.transcript.controls.clone();
        let mut h = harness(p, width, 820.);
        h.run();
        assert!(h.query_by_label("/terminal-secret").is_none());
        assert!(h.query_by_label("$missing-path").is_none());
        assert!(h.query_by_label("origin-compact").is_none());
        h.get_by_label("/compact").click();
        h.run();
        h.get_by_label("origin-compact");
        h.get_by_label("입력 안내: exact target");
        h.get_by_label("명령 및 스킬 검색").focus();
        h.get_by_label("명령 및 스킬 검색")
            .type_text("Review project");
        h.run();
        assert!(h.query_by_label("/compact").is_none());
        h.get_by_label("/review");
        h.get_by_label("검색 초기화").click();
        h.run();
        assert!(h.state().controls_ui.command_search.is_empty());
        h.get_by_label("/compact");
        assert_eq!(h.state().transcript.controls.commands, original.commands);
        assert_eq!(h.state().transcript.controls.skills, original.skills);
        assert!(h.state().sent.is_empty());
    }
}

#[test]
fn command_ux_large_catalog_scrolls_inside_fixed_search_and_composer() {
    let mut p = pane(Tool::Claude);
    p.controls_ui.surface = Surface::Commands;
    p.transcript.controls.commands = Some(
        (0..40)
            .map(|i| entry(&format!("native-{i}"), &"Long description ".repeat(30)))
            .collect(),
    );
    let mut h = harness(p, 900., 820.);
    h.run();
    let search_before = h.get_by_label("명령 및 스킬 검색").rect();
    let editor_before = h.get_by_label("메시지").rect();
    let first = h.get_by_label("/native-0").rect();
    let second = h.get_by_label("/native-1").rect();
    assert!(
        second.top() - first.top() < 70.,
        "Compact list row: {first:?}, {second:?}"
    );
    h.get_by_label("/native-39").scroll_to_me();
    h.run();
    assert_eq!(h.get_by_label("명령 및 스킬 검색").rect(), search_before);
    assert_eq!(h.get_by_label("메시지").rect(), editor_before);
    assert!(h.get_by_label("/native-39").rect().bottom() < editor_before.top());
    assert!(h.state().sent.is_empty());
}

#[test]
fn command_ux_completion_keyboard_wraps_and_enter_tab_only_edit_draft() {
    for (navigate, completion, expected) in [
        (egui::Key::ArrowDown, egui::Key::Enter, "한글 안내 /review "),
        (egui::Key::ArrowUp, egui::Key::Tab, "한글 안내 /review "),
    ] {
        let mut p = pane(Tool::Claude);
        p.composer.text = "한글 안내 /".into();
        let mut h = harness(p, 520., 760.);
        h.run();
        focus_editor(&mut h);
        h.get_by_label("/compact");
        h.get_by_label("/review");
        let before = h.get_by_label("메시지").rect();
        h.key_press(navigate);
        h.run();
        h.key_press(completion);
        h.run();
        assert_eq!(h.state().composer.text, expected);
        assert!(h.state().sent.is_empty(), "Completion must never submit");
        assert!(h.state().pending.is_none());
        assert_eq!(
            h.get_by_label("메시지").rect(),
            before,
            "Popup must not resize the composer"
        );
        // The next character must land after the completed command, not in its old prefix.

        h.get_by_label("메시지").type_text("target");
        h.run();
        assert_eq!(h.state().composer.text, format!("{expected}target"));
    }
}

#[test]
fn command_ux_escape_dismisses_until_query_changes_and_click_does_not_send() {
    let mut p = pane(Tool::Claude);
    p.composer.text = "/".into();
    let mut h = harness(p, 320., 540.);
    h.run();
    focus_editor(&mut h);
    let editor = h.get_by_label("메시지").rect();
    let popup_row = h.get_by_label("/compact").rect();
    assert!(
        popup_row.bottom() <= editor.top(),
        "Popup must float above the editor: {popup_row:?} {editor:?}"
    );

    h.key_press(egui::Key::Escape);
    h.run();

    assert!(h.query_by_label("/compact").is_none());
    assert_eq!(h.state().composer.text, "/");
    h.get_by_label("메시지").type_text("c");
    h.run();
    h.get_by_label("/compact").click();
    h.run();
    assert_eq!(h.state().composer.text, "/compact ");
    assert!(h.state().sent.is_empty());
}

#[test]
fn command_ux_codex_skill_completion_preserves_native_path_and_ime_guard() {
    let mut p = pane(Tool::Codex);
    let mut skill = entry("review-code", "Review with a native skill");
    skill.path = Some("/fixture/exact path/SKILL.md".into());
    p.transcript.controls.skills = Some(vec![skill]);
    p.composer.text = "$rev".into();
    let mut h = harness(p, 520., 760.);
    h.run();
    focus_editor(&mut h);
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.state().composer.text, "$review-code ");
    assert_eq!(
        h.state().composer.skills,
        vec![NativeSkillInput {
            name: "review-code".into(),
            path: "/fixture/exact path/SKILL.md".into()
        }]
    );
    assert!(h.state().sent.is_empty());

    h.state_mut().composer.text = "/com".into();
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "한".into(),
        active_range_chars: Some(1..1),
    }));
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.state().composing);
    assert!(h.state().sent.is_empty());
    assert!(!h.state().composer.text.contains("/compact"));
    assert!(h.query_by_label("/compact").is_none());
}

#[test]
fn command_ux_composer_highlight_marks_known_commands_and_keeps_exact_text() {
    let mut p = pane(Tool::Claude);
    let source = "plain /compact argument /unknown /fixture/path 한글";
    p.composer.text = source.into();
    let mut h = Harness::builder().build_ui_state(
        |ui, p: &mut AgentPane| {
            let mut layout = p.command_layouter();
            let galley = layout(ui, &p.composer.text, 300.);
            assert_eq!(galley.job.text, source);
            let marked = galley
                .job
                .sections
                .iter()
                .filter(|section| section.format.color == Theme::current().accent)
                .map(|section| {
                    &galley.job.text[section.byte_range.start.0..section.byte_range.end.0]
                })
                .collect::<Vec<_>>();
            assert_eq!(marked, vec!["/compact"]);
        },
        p,
    );
    h.run();
    assert!(h.state().sent.is_empty());
}

#[test]
fn command_ux_completion_scrolls_beyond_first_page_and_filters_unusable_entries() {
    let mut p = pane(Tool::Claude);
    p.transcript.controls.commands = Some(
        (0..12)
            .map(|index| entry(&format!("native-{index}"), "Command description"))
            .collect(),
    );
    let mut terminal = entry("native-terminal", "Terminal only");
    terminal.terminal_only = true;
    let mut disabled = entry("native-disabled", "Disabled");
    disabled.enabled = Some(false);
    p.transcript
        .controls
        .commands
        .as_mut()
        .unwrap()
        .extend([terminal, disabled]);
    p.composer.text = "/native-".into();
    let mut h = harness(p, 520., 760.);
    h.run();
    focus_editor(&mut h);
    assert!(h.query_by_label("/native-terminal").is_none());
    assert!(h.query_by_label("/native-disabled").is_none());
    for _ in 0..9 {
        h.key_press(egui::Key::ArrowDown);
        h.run();
    }
    let selected = h.get_by_label("/native-9").rect();
    assert!(selected.bottom() <= h.get_by_label("메시지").rect().top());
    assert!(selected.top() > 0.);
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(h.state().composer.text, "/native-9 ");
    assert!(h.state().sent.is_empty());
}

#[test]
fn command_ux_every_catalog_row_keeps_actions_and_left_aligned_names_within_bounds() {
    for width in [320., 520., 1100.] {
        let mut p = pane(Tool::Claude);
        p.controls_ui.surface = Surface::Commands;
        p.transcript.controls.commands = Some(
            (0..30)
                .map(|index| {
                    entry(
                        &format!("command-{index:02}"),
                        "Review the workspace and explain the changes before applying them.",
                    )
                })
                .collect(),
        );
        let mut h = harness(p, width, 820.);
        h.run();
        let names = (0..30)
            .map(|index| h.get_by_label(&format!("/command-{index:02}")).rect())
            .collect::<Vec<_>>();
        let actions = h
            .query_all_by_label("초안에 삽입")
            .map(|node| node.rect())
            .collect::<Vec<_>>();
        let expand = h
            .query_all_by_label("세부 정보")
            .map(|node| node.rect())
            .collect::<Vec<_>>();
        assert_eq!(actions.len(), 30);
        assert_eq!(expand.len(), 30);
        // Inspect the painted text origin: a full-width accessibility rect alone
        // cannot distinguish centered text from text aligned to the leading edge.
        fn command_text_positions(shape: &egui::Shape, out: &mut Vec<f32>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text().starts_with("/command-") => {
                    out.push(text.pos.x)
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        command_text_positions(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut painted_left = vec![];
        for shape in &h.output().shapes {
            command_text_positions(&shape.shape, &mut painted_left);
        }
        assert!(!painted_left.is_empty());
        for left in painted_left {
            assert!(
                (left - names[0].left()).abs() < 0.1,
                "Name glyphs must paint at the leading edge, width={width}, x={left}"
            );
        }

        for ((name, action), arrow) in names.iter().zip(&actions).zip(&expand) {
            assert!(
                (name.left() - names[0].left()).abs() < 0.1,
                "Names must share a fixed left edge: width={width} {name:?}"
            );
            assert!(
                (action.right() - actions[0].right()).abs() < 0.1,
                "Actions drift across consecutive rows: width={width} {action:?}"
            );
            assert!(
                arrow.left() >= 0. && action.right() <= width && name.right() <= action.left(),
                "Bounded row at width={width}: {arrow:?} {name:?} {action:?}"
            );
            assert!(arrow.right() < name.left());
        }
    }
}

#[test]
fn command_ux_completion_popup_is_narrow_and_paints_both_lines_at_the_leading_edge() {
    for width in [320., 520., 1100.] {
        for dark in [true, false] {
            let mut p = pane(Tool::Claude);
            p.composer.text = "/command".into();
            p.transcript.controls.commands = Some(
                (0..30)
                    .map(|index| {
                        entry(
                            &format!("command-{index:02}"),
                            "Review the workspace and explain the changes before applying them.",
                        )
                    })
                    .collect(),
            );
            let mut initialized = false;
            let mut h = Harness::builder().with_size([width, 820.]).build_ui_state(
                move |ui, p: &mut AgentPane| {
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
            );
            h.run();
            focus_editor(&mut h);
            let editor = h.get_by_label("메시지").rect();
            let first = h.get_by_label("/command-00").rect();
            assert!(
                first.width() <= 456.,
                "Completion must stay a compact list at width {width}: {first:?}"
            );
            assert!(
                (first.left() - editor.left()).abs() <= 5.,
                "Completion must anchor to the editor's left edge: {first:?} {editor:?}"
            );
            assert!(first.left() >= 0. && first.right() <= width);
            fn collect(
                shape: &egui::Shape,
                name: &mut Vec<egui::Pos2>,
                description: &mut Vec<egui::Pos2>,
            ) {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == "/command-00" => {
                        name.push(text.pos)
                    }
                    egui::Shape::Text(text)
                        if text.galley.text().starts_with("Review the workspace") =>
                    {
                        description.push(text.pos)
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, name, description);
                        }
                    }
                    _ => {}
                }
            }
            let mut names = vec![];
            let mut descriptions = vec![];
            for shape in &h.output().shapes {
                collect(&shape.shape, &mut names, &mut descriptions);
            }
            let name = names.first().expect("Painted command name");
            let description = descriptions.first().expect("Painted short description");
            assert!(
                (name.x - first.left() - 8.).abs() < 0.1,
                "Actual name paint must be left aligned: {name:?} {first:?}"
            );
            assert!(
                (description.x - name.x).abs() < 0.1,
                "Description must share the name's left edge: {name:?} {description:?}"
            );
            assert!(
                description.y > name.y + 10.,
                "Description must be a separate second line"
            );
            if let Some(dir) = std::env::var_os("KILN_COMMAND_COMPLETION_CAPTURE_DIR") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                h.render()
                    .unwrap()
                    .save(dir.join(format!(
                        "ux-completion-{}-{width:.0}.png",
                        if dark { "dark" } else { "light" }
                    )))
                    .unwrap();
            }
            // The second-line description remains part of the same insertion target.
            let click = first.left_top() + egui::vec2(24., 30.);
            h.hover_at(click);
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            h.run();
            assert_eq!(h.state().composer.text, "/command-00 ");
            assert!(h.state().sent.is_empty());
        }
    }
}
