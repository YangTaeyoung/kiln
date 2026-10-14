//! Stream native logs without retaining reasoning or provider instructions.
use super::*;
use serde_json::Value;
use std::io::{BufRead, BufReader};

pub(super) fn rows(source: &Source, title: &mut String) -> Result<Vec<Value>> {
    rows_before_validation(source, title, || {})
}

pub(super) fn rows_before_validation(
    source: &Source,
    title: &mut String,
    before_validation: impl FnOnce(),
) -> Result<Vec<Value>> {
    let physical = storage::physical(source);
    let (file, before) = storage::open(&source.root, &physical)?;
    let compressed = storage::compressed(&physical);
    ensure!(
        before.len() <= 1024 * 1024 * 1024,
        "Transcript exceeds the 1 GiB reading limit"
    );
    let mut reader = BufReader::new(file);
    let mut rows = Vec::new();
    let mut line = Vec::new();
    let mut retained = 0;
    let mut decoded = 0u64;
    let mut snapshot_digest = Sha256::new();
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(LIMIT + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        ensure!(
            line.len() as u64 <= LIMIT,
            "Transcript row exceeds the 64 MiB reading limit"
        );
        decoded += line.len() as u64;
        ensure!(decoded <= COPY_LIMIT, "Decoded transcript exceeds the 1 GiB reading limit");
        snapshot_digest.update(&line);
        // A writer can be in the middle of its next record. Only complete records
        // in the initial bounded prefix participate in this checkpoint.
        if !line.ends_with(b"\n") {
            break;
        }
        if line == b"\n" {
            continue;
        }
        let mut row: Value = serde_json::from_slice(&line)?;
        if source.tool == Tool::Codex {
            match row["type"].as_str() {
                Some("session_meta") => {
                    if let Some(p) = row["payload"].as_object_mut() {
                        p.remove("base_instructions");
                    }
                }
                Some("response_item") if row["payload"]["type"] == "reasoning" => continue,
                Some("response_item")
                    if row["payload"]["type"] == "agent_message"
                        && row["payload"]["content"].as_array().is_some_and(|c| {
                            c.iter().any(|b| b["type"] == "encrypted_content")
                        }) =>
                {
                    continue;
                }
                Some("response_item")
                    if row["payload"]["type"] == "message"
                        && !matches!(
                            row["payload"]["role"].as_str(),
                            Some("user" | "assistant")
                        ) =>
                {
                    continue;
                }
                Some("response_item") => {}
                Some("event_msg")
                    if row["payload"]["type"] == "item_completed"
                        && !matches!(
                            row["payload"]["item"]["type"].as_str(),
                            Some("UserMessage" | "AgentMessage")
                        ) =>
                {
                    continue;
                }
                Some("event_msg")
                    if matches!(
                        row["payload"]["type"].as_str(),
                        Some(
                            "user_message"
                                | "task_started"
                                | "item_completed"
                                | "task_complete"
                                | "turn_complete"
                                | "turn_aborted"
                                | "thread_rolled_back"
                        )
                    ) => {}
                _ => continue,
            }
        }
        if row["type"] == "event_msg" {
            // User event attachments are evidence; bytes are carried by the paired response item.
            for pointer in ["/payload/images", "/payload/local_images"] {
                if let Some(Value::Array(images)) = row.pointer_mut(pointer) {
                    for image in images {
                        *image = Value::Null;
                    }
                }
            }
            if let Some(Value::Array(blocks)) = row.pointer_mut("/payload/item/content") {
                for block in blocks {
                    if matches!(block["type"].as_str(), Some("image" | "local_image")) {
                        *block = serde_json::json!({"type":"image"});
                    }
                }
            }
        }
        // Only content blocks are transformed: arbitrary tool input/output JSON stays verbatim.
        for pointer in [
            "/message/content",
            "/payload/content",
            "/payload/output",
            "/payload/item/content",
        ] {
            if let Some(content) = row.pointer_mut(pointer) {
                strip_media(content);
            }
        }
        if title.is_empty() {
            *title = row_title(source.tool, &row);
        }
        retained += serde_json::to_vec(&row)?.len() as u64;
        ensure!(
            retained <= PORTABLE_LIMIT,
            "Portable conversation exceeds the 512 MiB sync limit"
        );
        rows.push(row);
    }
    before_validation();
    ensure!(storage::physical(source) == physical, "Transcript storage changed; retrying");
    if compressed {
        storage::unchanged(source, &physical, &before)?;
        return Ok(rows);
    }
    let after = storage::metadata(source)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev() && before.ino() == after.ino(),
            "Transcript changed while reading; retrying"
        );
    }
    ensure!(
        after.len() >= before.len(),
        "Transcript changed while reading; retrying"
    );
    if before.len() != after.len() || before.modified()? != after.modified()? {
        // Permit appends only after verifying every byte of the captured prefix.
        // Rewrites/truncations must never be mistaken for a completed checkpoint.
        let mut check = fs::File::open(&physical)?.take(before.len());
        let mut verified = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut count = 0;
        loop {
            let read = check.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            verified.update(&buffer[..read]);
            count += read as u64;
        }
        ensure!(
            count == before.len() && verified.finalize() == snapshot_digest.finalize(),
            "Transcript changed while reading; retrying"
        );
    }
    Ok(rows)
}

fn strip_media(content: &mut Value) {
    let Some(blocks) = content.as_array_mut() else {
        return;
    };
    for block in blocks {
        match block["type"].as_str() {
            Some("thinking" | "redacted_thinking") => {
                *block = serde_json::json!({"type":"thinking"});
            }
            Some("tool_result") => strip_media(&mut block["content"]),
            _ => {}
        }
    }
}

fn row_title(tool: Tool, row: &Value) -> String {
    let content = match tool {
        Tool::Claude
            if row["type"] == "user"
                && row["isMeta"] != true
                && row["isCompactSummary"] != true
                && super::format::claude_interruption(row).is_none() =>
        {
            &row["message"]["content"]
        }
        Tool::Codex if row["type"] == "event_msg" && row["payload"]["type"] == "user_message" => {
            &row["payload"]["message"]
        }
        Tool::Codex
            if row["type"] == "event_msg" && row["payload"]["item"]["type"] == "UserMessage" =>
        {
            &row["payload"]["item"]["content"]
        }
        _ => return String::new(),
    };
    let text = content
        .as_str()
        .or_else(|| content.as_array()?.iter().find_map(|b| b["text"].as_str()))
        .unwrap_or_default();
    if text.starts_with("<local-command-") || text.starts_with("<command-name>") {
        return String::new();
    }
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(96)
        .collect()
}
