use super::*;

// Synthetic values shaped like actual agy 1.4.7 read/edit/command records.
// No user transcripts, paths, credentials or model reasoning are fixtures.
fn agy_history(name: &str, input: serde_json::Value, output: &str) -> Vec<Message> {
    shell_turn(
        &format!("[Tool call: {name}]\n{input}"),
        &format!("[Tool result: agy-7-0]\n{output}"),
    )
}

fn rendered(tool: Tool, history: &[Message]) -> Vec<serde_json::Value> {
    rows(&format::render_titled(tool, &stable_id("agy-native-tools"), "/workspace", "2026-10-26T10:00:00Z", history, "Agy tools").unwrap())
}

#[test]
fn agy_147_read_and_edit_are_native_claude_tools_and_roundtrip() {
    for (name, input, output, expected, expected_input) in [
        ("read_file", json!({"Path":"/workspace/probe.txt"}),
         "Showing lines 1-2 of 2\n1: KILN_TOOL_PROBE\n2: value=before", "Read",
         json!({"file_path":"/workspace/probe.txt"})),
        ("edit_file", json!({"Path":"/workspace/probe.txt","Target":"value=before","Replacement":"value=after"}),
         "Made 1 replacement in /workspace/probe.txt starting at line 2 (+1/-1 lines).", "Edit",
         json!({"file_path":"/workspace/probe.txt","old_string":"value=before","new_string":"value=after","replace_all":false})),
    ] {
        let history = agy_history(name, input, output);
        let data = rendered(Tool::Claude, &history);
        let call = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_use")
            .unwrap_or_else(|| panic!("{name} must be a native Claude tool, not an assistant text bubble"));
        assert_eq!(call["message"]["content"][0]["name"], expected);
        assert_eq!(call["message"]["content"][0]["input"], expected_input);
        let result = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").unwrap();
        assert_eq!(result["message"]["content"][0]["tool_use_id"], call["message"]["content"][0]["id"]);
        assert_eq!(result["message"]["content"][0]["is_error"], false);
        if expected == "Edit" {
            assert!(!result["toolUseResult"]["structuredPatch"].as_array().unwrap().is_empty(), "Edit needs a visible native diff");
        }
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history, "{name} must retain the original Agy fields");
    }
}

#[test]
fn agy_147_commands_render_native_command_output_in_both_clis() {
    let history = agy_history("run_command", json!({"CommandLine":"printf tool-marker","WaitMsBeforeAsync":5000,"toolSummary":"Print marker"}),
        "Command exited with code 0.\n\nCommand output:\ntool-marker");
    let claude = rendered(Tool::Claude, &history);
    let call = claude.iter().find(|r| r["message"]["content"][0]["type"] == "tool_use").expect("native Bash");
    assert_eq!(call["message"]["content"][0]["name"], "Bash");
    assert_eq!(call["message"]["content"][0]["input"]["command"], "printf tool-marker");
    let result = claude.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").unwrap();
    assert_eq!(result["toolUseResult"]["stdout"], "tool-marker");
    assert_eq!(format::parse(Tool::Claude, &claude).unwrap(), history);
    let codex = rendered(Tool::Codex, &history);
    let cell = codex.iter().map(|r| &r["payload"]["item"]).find(|i| i["type"] == "CommandExecution").expect("native Codex command");
    assert_eq!(cell["command"], json!(["/bin/zsh","-lc","printf tool-marker"]));
    assert_eq!(cell["cwd"], "file:///workspace");
    assert_eq!(cell["aggregated_output"], "tool-marker");
    assert_eq!(cell["exit_code"], 0);
    assert_eq!(cell["status"], "completed");
    assert_eq!(format::parse(Tool::Codex, &codex).unwrap(), history);
}

#[test]
fn agy_147_edits_show_native_codex_file_changes_and_reads_keep_their_origin() {
    let history = agy_history("edit_file", json!({"Path":"/workspace/probe.txt","Target":"value=before","Replacement":"value=after"}),
        "Made 1 replacement in /workspace/probe.txt starting at line 2 (+1/-1 lines).");
    let data = rendered(Tool::Codex, &history);
    let cell = data.iter().map(|r| &r["payload"]["item"]).find(|i| i["type"] == "FileChange").expect("native file diff, not an unrelated MCP call");
    assert!(cell.to_string().contains("value=before") && cell.to_string().contains("value=after"));
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    let read = agy_history("read_file", json!({"Path":"/workspace/probe.txt"}), "Showing lines 1-1 of 1\n1: value=after");
    let data = rendered(Tool::Codex, &read);
    let cell = data.iter().map(|r| &r["payload"]["item"]).find(|i| i["type"] == "McpToolCall").unwrap();
    assert_eq!(cell["server"], "antigravity", "Agy tools must not be attributed to Codex");
    assert_eq!(cell["tool"], "read_file");
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), read);
}

#[test]
fn agy_147_permission_denial_is_a_failed_tool_not_success_or_a_fabricated_exit() {
    let history = agy_history("run_command", json!({"CommandLine":"printf tool-marker","WaitMsBeforeAsync":5000}),
        "Encountered error in step execution: permission check failed: user denied permission to run command");
    let claude = rendered(Tool::Claude, &history);
    let result = claude.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").expect("denied native tool result");
    assert_eq!(result["message"]["content"][0]["is_error"], true);
    assert_eq!(format::parse(Tool::Claude, &claude).unwrap(), history);
    let codex = rendered(Tool::Codex, &history);
    let cell = codex.iter().map(|r| &r["payload"]["item"]).find(|i| i["type"] == "CommandExecution").expect("failed command cell");
    assert_eq!(cell["status"], "failed");
    assert!(cell["exit_code"].is_null(), "a denied command never produced a process exit code");
    assert_eq!(format::parse(Tool::Codex, &codex).unwrap(), history);
}

#[test]
fn agy_commands_become_codex_native_calls_without_losing_canonical_fields() {
    let history = agy_history("run_command", json!({"CommandLine":"printf marker","Cwd":"/workspace/sub","WaitMsBeforeAsync":5000}),
        "Command exited with code 0.\n\nCommand output:\nmarker");
    let mut data = rendered(Tool::Codex, &history);
    let call = data.iter().find(|r| r["payload"]["type"] == "function_call").expect("a real Codex tool call in model history");
    assert_eq!(call["payload"]["name"], "exec_command");
    let input: serde_json::Value = serde_json::from_str(call["payload"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(input, json!({"cmd":"printf marker","workdir":"/workspace/sub"}));
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    for row in &mut data { row.as_object_mut().unwrap().remove("kiln_sync"); }
    let native = format::parse(Tool::Codex, &data).unwrap();
    assert!(tools::same(&native[1].text, &history[1].text));
    assert!(tools::same(&native[2].text, &history[2].text));
    let mut tampered = rendered(Tool::Codex, &history);
    let row = tampered.iter_mut().find(|r| r["payload"]["type"] == "function_call").unwrap();
    row["payload"]["arguments"] = json!("{\"cmd\":\"a different command\"}");
    let changed = format::parse(Tool::Codex, &tampered).unwrap();
    assert!(changed[1].text.contains("a different command"), "stale original metadata must never hide an edit");
}

#[test]
fn agy_error_status_is_preserved_even_when_the_output_has_no_error_words() {
    let records = vec![
        agy_user(0, "Read the file"),
        agy_calls(1, &[("read_file", json!({"Path":"/workspace/probe.txt"}))]),
        json!({"step_index":2,"type":"GENERIC","source":"MODEL","status":"ERROR","content":"The file is unavailable."}),
        agy_answer(3, "Cannot read it."),
    ];
    let history = format::parse(Tool::Antigravity, &records).unwrap();
    let data = rendered(Tool::Claude, &history);
    let result = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").unwrap();
    assert_eq!(result["message"]["content"][0]["is_error"], true);
    assert_eq!(result["message"]["content"][0]["content"], "The file is unavailable.");
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
}

#[test]
fn agy_tool_copies_are_upgraded_in_both_clis_without_touching_the_origin() {
    let f = Fixture::new(Tool::Claude);
    let (root, scope) = agy_workspace(&f);
    let records = vec![
        agy_user(0, "Run the probe"),
        agy_calls(1, &[("run_command", json!({"CommandLine":"printf marker","WaitMsBeforeAsync":5000}))]),
        agy_result(2, "Command exited with code 0.\n\nCommand output:\nmarker"),
        agy_answer(3, "Done."),
    ];
    Agy::new(&root, "native-tools", &f.source.cwd, None, &records);
    let before = agy_snapshot(&root);
    assert!(workspace_scan(&f, &scope).errors.is_empty());
    let old: Vec<_> = f.live().into_iter().filter(|p| !p.origin).map(|p| p.source).collect();
    assert_eq!(old.len(), 2);
    for source in &old { age_projection(&f, &source.path); }
    for _ in 0..3 { assert!(workspace_scan(&f, &scope).errors.is_empty()); }
    assert!(old.iter().all(|s| !s.path.exists()), "both earlier native copies must leave the resume list");
    let copies: Vec<_> = f.live().into_iter().filter(|p| !p.origin).map(|p| p.source).collect();
    for copy in &copies {
        assert_eq!(read_messages(copy).unwrap(), format::parse(Tool::Antigravity, &records).unwrap());
    }
    assert_eq!(agy_snapshot(&root), before);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
}

#[test]
fn agy_unknown_arguments_keep_the_complete_record_instead_of_dropping_semantics() {
    let history = agy_history("run_command", json!({"CommandLine":"printf marker","FutureExecutionOption":true}),
        "Command exited with code 0.\n\nCommand output:\nmarker");
    for tool in PROJECTED {
        let data = rendered(tool, &history);
        assert_eq!(format::parse(tool, &data).unwrap(), history);
        assert!(data.iter().all(|r| r["payload"]["type"] != "function_call"));
    }
}

#[test]
fn agy_denied_command_before_the_next_user_turn_still_has_a_native_failed_record() {
    let mut history = agy_history("run_command", json!({"CommandLine":"printf marker"}),
        "[Tool error]\nPermission denied.");
    history.remove(3);
    history.extend(turns(&["Try again", "Done."]));
    for tool in PROJECTED {
        let data = rendered(tool, &history);
        let failed = data.iter().any(|r| r["message"]["content"][0]["is_error"] == true
            || r["payload"]["item"]["status"] == "failed");
        assert!(failed, "a failed Agy tool remains native before a new user turn: {tool:?}");
        assert_eq!(format::parse(tool, &data).unwrap(), history);
    }
}

#[test]
fn agy_completed_nonzero_commands_keep_recorded_output_without_inventing_stderr() {
    let history = agy_history("run_command", json!({"CommandLine":"probe","Cwd":"/workspace/sub"}),
        "Command exited with code 7.\n\nCommand output:\npartial output");
    let data = rendered(Tool::Claude, &history);
    let result = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").unwrap();
    assert_eq!(result["message"]["content"][0]["is_error"], true);
    assert_eq!(result["toolUseResult"]["stdout"], "partial output");
    assert_eq!(result["toolUseResult"]["stderr"], "");
    let codex = rendered(Tool::Codex, &history);
    let cell = codex.iter().map(|r| &r["payload"]["item"]).find(|i| i["type"] == "CommandExecution").unwrap();
    assert_eq!(cell["exit_code"], 7);
    assert_eq!(cell["status"], "failed");
    assert_eq!(format::parse(Tool::Codex, &codex).unwrap(), history);
}
