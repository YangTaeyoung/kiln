use super::*;

fn definitions() -> serde_json::Value {
    json!({"type":"response_item","payload":{"type":"additional_tools","id":"tool-definitions-fixture","role":"developer",
        "tools":[{"type":"function","name":"fixture_command","description":"Provider-local instructions must not be copied.","parameters":{"type":"object","properties":{}}}]}})
}

#[test]
fn codex_tool_definitions_do_not_hide_completed_messages_or_native_calls() {
    let history = shell_turn(CODEX_CALL, CODEX_RESULT);
    let mut data = rows(&format::render(Tool::Codex, &stable_id("tool-definitions"), "/workspace", "2026-10-27T00:00:00Z", &history).unwrap());
    data.insert(2, definitions());
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    let rendered = format::render(Tool::Claude, &stable_id("tool-definitions-copy"), "/workspace", "2026-10-27T00:00:00Z", &history).unwrap();
    assert!(!String::from_utf8_lossy(&rendered).contains("Provider-local instructions"));
}

#[test]
fn codex_tool_definitions_sync_without_modifying_the_native_file_or_echoing() {
    let f = Fixture::new(Tool::Codex);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    data.insert(2, definitions());
    let bytes = data.iter().map(|r| format!("{r}\n")).collect::<String>();
    fs::write(&f.source.path, &bytes).unwrap();
    let before = fs::metadata(&f.source.path).unwrap().modified().unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(read_messages(&f.peer()).unwrap(), messages());
    assert_eq!(fs::read_to_string(&f.source.path).unwrap(), bytes);
    assert_eq!(fs::metadata(&f.source.path).unwrap().modified().unwrap(), before);
    assert_eq!(f.scan().published, 0);
}

#[test]
fn malformed_or_content_bearing_tool_definitions_remain_visible_failures() {
    for value in [json!("not a tool list"), json!(null)] {
        let mut data = rows(&format::render(Tool::Codex, &stable_id("invalid-definitions"), "/workspace", "2026-10-27T00:00:00Z", &messages()).unwrap());
        let mut definition = definitions();definition["payload"]["tools"] = value;data.insert(2, definition);
        assert!(format::parse(Tool::Codex, &data).is_err());
    }
    let mut data = rows(&format::render(Tool::Codex, &stable_id("mixed-definitions"), "/workspace", "2026-10-27T00:00:00Z", &messages()).unwrap());
    let mut definition = definitions();definition["payload"]["content"] = json!("This content must never silently disappear.");data.insert(2, definition);
    assert!(format::parse(Tool::Codex, &data).is_err());
}

#[test]
fn codex_tool_definitions_after_a_checkpoint_keep_the_new_turn_and_sync_once() {
    let f = Fixture::new(Tool::Codex);
    assert!(f.scan().errors.is_empty());
    let (_, checkpoint) = checkpoint::read(&f.source, &mut String::new()).unwrap();
    let later = turns(&["Continue after the tool catalog changed.", "The earlier context is retained."]);
    let mut tail = rows(&format::render(Tool::Codex, &stable_id("later-tool-definitions"), &f.source.cwd, "2026-10-27T01:00:00Z", &later).unwrap());
    tail.retain(|r| r["type"] != "session_meta");
    tail.insert(1, definitions());
    for row in &mut tail {
        if row["payload"].get("thread_id").is_some() { row["payload"]["thread_id"] = json!(f.source.id); }
    }
    let mut file = fs::OpenOptions::new().append(true).open(&f.source.path).unwrap();
    for row in tail { writeln!(file, "{row}").unwrap(); }
    drop(file);
    let (appended, _, _) = checkpoint::since_titled(&f.source, &checkpoint.unwrap()).unwrap().unwrap();
    assert_eq!(appended, later);
    let status = f.scan();assert!(status.errors.is_empty(), "{:?}", status.errors);
    let mut expected = messages();expected.extend(later);
    assert_eq!(read_messages(&f.peer()).unwrap(), expected);
    assert_eq!(f.scan().published, 0);
}
