use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn pane(tool: Tool, cwd: &Path, draft: &str) -> AgentPane {
    let mut pane = AgentPane::fixture(Descriptor {
        id: "path-mention-fixture".into(),
        tool,
        cwd: cwd.into(),
        account_label: "Fixture".into(),
    });
    pane.transcript.lifecycle = Lifecycle::Ready;
    pane.transcript.generation = "fixture".into();
    pane.composer.text = draft.into();
    pane
}
fn harness(pane: AgentPane, width: f32) -> Harness<'static, AgentPane> {
    let mut initialized = false;
    Harness::builder().with_size([width, 760.]).build_ui_state(
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
fn open(h: &mut Harness<'_, AgentPane>) {
    h.run();
    h.get_by_label("메시지").focus();
    h.run();
}
fn loaded(h: &mut Harness<'_, AgentPane>) {
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        h.run_steps(2);
        if h.state().mentions.loaded == h.state().mentions.query
            && h.state().mentions.query.is_some()
        {
            break;
        }
        assert!(Instant::now() < until, "path search did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn capture_if_requested(h: &mut Harness<'_, AgentPane>, name: &str) {
    if let Some(directory) = std::env::var_os("KILN_MENTIONS_RENDER_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        h.render().unwrap().save(directory.join(name)).unwrap();
    }
}

#[test]
fn path_mentions_parse_cursor_unicode_quoted_spaces_and_ignore_email() {
    for text in [
        "alice@example.com",
        "hello alice@example.com",
        "review @src/file.rs next",
        "@\"complete file\" ",
    ] {
        assert!(query_at(text, text.chars().count()).is_none(), "{text}");
    }
    let text = "한글 앞 @src/한글.rs 뒤";
    let cursor = "한글 앞 @src/한".chars().count();
    let query = query_at(text, cursor).unwrap();
    assert_eq!(query.needle, "src/한");
    assert_eq!(&text[query.start..query.end], "@src/한글.rs");
    let text = "read @\"folder name/file.rs\" after";
    let query = query_at(text, "read @\"folder name/fi".chars().count()).unwrap();
    assert_eq!(query.needle, "folder name/fi");
    assert_eq!(&text[query.start..query.end], "@\"folder name/file.rs\"");
}

#[test]
fn path_mentions_keep_closing_punctuation_outside_the_replaced_token() {
    for (text, prefix, expected) in [
        ("read (@fo) suffix", "read (@fo", "@fo"),
        ("read [@fo] suffix", "read [@fo", "@fo"),
        ("read @fo, next", "read @fo", "@fo"),
        (
            "read [@\"folder name/fi\"] suffix",
            "read [@\"folder name/fi",
            "@\"folder name/fi\"",
        ),
    ] {
        let query = query_at(text, prefix.chars().count()).unwrap();
        assert_eq!(&text[query.start..query.end], expected, "{text}");
    }
}

#[test]
fn path_mentions_mouse_click_inserts_the_visible_selection_without_sending() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("selected.txt");
    std::fs::write(&file, "fixture only").unwrap();
    let mut h = harness(pane(Tool::Claude, root.path(), "@sel"), 520.);
    open(&mut h);
    loaded(&mut h);
    capture_if_requested(&mut h, "mention-before-mouse-click.png");
    h.get_by_label("selected.txt").click();
    h.run();
    assert_eq!(
        h.state().composer.text,
        format!(
            "@{} ",
            serde_json::to_string(file.to_str().unwrap()).unwrap()
        )
    );
    assert!(h.state().sent.is_empty());
    capture_if_requested(&mut h, "mention-after-mouse-click.png");
}

#[test]
fn path_mentions_touch_press_and_release_keeps_the_popup_until_selection() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("selected.txt");
    std::fs::write(&file, "fixture only").unwrap();
    let mut h = harness(pane(Tool::Claude, root.path(), "@sel"), 520.);
    open(&mut h);
    loaded(&mut h);
    let position = h.get_by_label("selected.txt").rect().center();
    h.event(egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Start,
        pos: position,
        force: None,
    });
    h.event(egui::Event::PointerMoved(position));
    h.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(
        h.state().composer.text,
        "@sel",
        "A press must wait for its release"
    );
    h.get_by_label("selected.txt");
    h.event(egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::End,
        pos: position,
        force: None,
    });
    h.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(
        h.state().composer.text,
        format!(
            "@{} ",
            serde_json::to_string(file.to_str().unwrap()).unwrap()
        )
    );
    assert!(h.state().sent.is_empty());
}

#[test]
fn path_mentions_mid_token_insertion_preserves_brackets_and_quoted_suffixes() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file with spaces.txt");
    std::fs::write(&file, "fixture only").unwrap();
    for (text, prefix, leading, trailing) in [
        (
            "Review (@README.md) next",
            "Review (@READ",
            "Review (",
            ") next",
        ),
        (
            "Review [@README.md] next",
            "Review [@READ",
            "Review [",
            "] next",
        ),
        (
            "Review [@\"folder name/README.md\"] next",
            "Review [@\"folder name/READ",
            "Review [",
            "] next",
        ),
    ] {
        let mut inserted = false;
        let entry = Entry {
            path: file.clone(),
            label: "file with spaces.txt".into(),
            directory: false,
            rank: 0,
        };
        let mut h = Harness::builder().build_ui_state(
            |ui, pane: &mut AgentPane| {
                let id = ui.id().with("message");
                egui::TextEdit::multiline(&mut pane.composer.text)
                    .id(id)
                    .show(ui);
                if inserted {
                    return;
                }
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(prefix.chars().count()),
                    )));
                state.store(ui.ctx(), id);
                pane.mentions.query = query_at(text, prefix.chars().count());
                pane.insert_mention(ui, &entry, false);
                inserted = true;
            },
            pane(Tool::Claude, root.path(), text),
        );
        h.run();
        assert_eq!(
            h.state().composer.text,
            format!(
                "{leading}@{}{trailing}",
                serde_json::to_string(file.to_str().unwrap()).unwrap()
            )
        );
        assert!(h.state().sent.is_empty());
    }
}

#[test]
fn path_mentions_search_prefers_prefix_and_keeps_kind_relative_path_and_bounds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("read-folder")).unwrap();
    std::fs::create_dir(root.path().join("node_modules")).unwrap();
    for name in ["README.md", "spread.md", "한글 file.txt"] {
        std::fs::write(root.path().join(name), "fixture").unwrap();
    }
    std::fs::write(root.path().join("node_modules/reader-secret"), "ignored").unwrap();
    let result = scan(
        root.path(),
        Query {
            start: 0,
            end: 3,
            needle: "rea".into(),
        },
    );
    assert!(result.entries[0].directory);
    assert_eq!(result.entries[1].label, "README.md");
    assert!(
        result
            .entries
            .iter()
            .all(|entry| !entry.label.contains("node_modules"))
    );
    for index in 0..80 {
        std::fs::write(root.path().join(format!("fixture-{index}")), "").unwrap();
    }
    assert_eq!(
        scan(
            root.path(),
            Query {
                start: 0,
                end: 1,
                needle: String::new()
            }
        )
        .entries
        .len(),
        MAX_RESULTS
    );
}

#[test]
fn path_mentions_stale_async_results_cannot_replace_a_new_query() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("new-file"), "fixture").unwrap();
    let mut pane = pane(Tool::Claude, root.path(), "@new");
    pane.mentions.running = Some(Task::ready(SearchResult {
        query: Query {
            start: 0,
            end: 4,
            needle: "old".into(),
        },
        entries: vec![Entry {
            path: root.path().join("old"),
            label: "old-private-result".into(),
            directory: false,
            rank: 0,
        }],
        error: None,
    }));
    let mut h = harness(pane, 520.);
    open(&mut h);
    loaded(&mut h);
    assert_eq!(h.state().mentions.entries.len(), 1);
    assert_eq!(h.state().mentions.entries[0].label, "new-file");
    assert!(h.query_by_label("old-private-result").is_none());
}

#[test]
fn path_mentions_selection_preserves_provider_native_path_and_only_explicit_send_submits() {
    for tool in Tool::ALL {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("한글 file.txt");
        std::fs::write(&file, "fixture marker").unwrap();
        let mut h = harness(pane(tool, root.path(), "@한"), 900.);
        open(&mut h);
        loaded(&mut h);
        let row = h.get_by_label("한글 file.txt").rect();
        let editor = h.get_by_label("메시지").rect();
        assert!(
            row.width() <= 456. && row.bottom() <= editor.top(),
            "{row:?} {editor:?}"
        );
        h.key_press(egui::Key::Enter);
        h.run();
        let quoted = serde_json::to_string(file.to_str().unwrap()).unwrap();
        let expected = format!("{}{quoted} ", if tool == Tool::Codex { "" } else { "@" });
        assert_eq!(h.state().composer.text, expected);
        assert!(h.state().sent.is_empty());
        let saved = h.state().draft();
        assert_eq!(saved.composer.text, expected);
        h.state_mut().submit();
        assert!(
            matches!(h.state().sent.last(),Some(Command::Submit {text,..}) if text == &expected)
        );
    }
}

#[test]
fn path_mentions_native_adapters_preserve_exact_selected_reference_on_wire() {
    use crate::app::agent_gui::providers::Adapter;
    use serde_json::json;
    for tool in Tool::ALL {
        let text = format!(
            "Inspect {}\"/fixture/한글 file.txt\"",
            if tool == Tool::Codex { "" } else { "@" }
        );
        let mut adapter = Adapter::new(tool, "/fixture".into(), None);
        match tool {
            Tool::Claude => {
                adapter.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{"models":[]}}})).unwrap();
                adapter.ingest(json!({"type":"system","subtype":"init","session_id":"native","model":"fixture"})).unwrap();
            }
            Tool::Codex => {
                adapter.ingest(json!({"id":"thread","result":{"thread":{"id":"native"},"model":"fixture"}})).unwrap();
            }
            Tool::Antigravity => {
                adapter
                    .ingest(json!({"id":"session","result":{"sessionId":"native"}}))
                    .unwrap();
            }
        }
        let effects = adapter
            .command(
                &Command::Submit {
                    text: text.clone(),
                    attachments: vec![],
                    model: None,
                },
                "fixture",
            )
            .unwrap();
        let pointer = match tool {
            Tool::Claude => "/message/content/0/text",
            Tool::Codex => "/params/input/0/text",
            Tool::Antigravity => "/params/prompt/0/text",
        };
        assert!(
            effects.writes.iter().any(|write| write
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                == Some(&text)),
            "{tool:?}: {:?}",
            effects.writes
        );
    }
}

#[test]
fn path_mentions_arrow_navigation_selects_the_highlighted_file_without_send() {
    let root = tempfile::tempdir().unwrap();
    for name in ["alpha.txt", "alpine.txt"] {
        std::fs::write(root.path().join(name), "fixture").unwrap();
    }
    let mut h = harness(pane(Tool::Claude, root.path(), "@al"), 520.);
    open(&mut h);
    loaded(&mut h);
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert_eq!(h.state().mentions.active, 1);
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert_eq!(h.state().mentions.active, 0);
    h.key_press(egui::Key::ArrowDown);
    h.run();
    h.key_press(egui::Key::Tab);
    h.run();
    assert!(h.state().composer.text.contains("alpine.txt"));
    assert!(h.state().sent.is_empty());
}

#[test]
fn path_mentions_tab_browses_spaced_directory_and_escape_or_ime_never_sends() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder name")).unwrap();
    std::fs::write(root.path().join("folder name/child.txt"), "fixture").unwrap();
    let mut h = harness(pane(Tool::Claude, root.path(), "@fol"), 320.);
    open(&mut h);
    loaded(&mut h);
    h.key_press(egui::Key::Tab);
    h.run();
    loaded(&mut h);
    h.get_by_label("folder name/child.txt");
    assert!(h.state().sent.is_empty());
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(h.query_by_label("folder name/child.txt").is_none());
    h.state_mut().composer.text = "@missing-path".into();
    h.state_mut().focus = true;
    h.run();
    loaded(&mut h);
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(
        h.state().sent.is_empty(),
        "Empty results must not send the draft"
    );
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "한".into(),
        active_range_chars: Some(1..1),
    }));
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(h.state().sent.is_empty());
    assert!(h.state().composing);
}
