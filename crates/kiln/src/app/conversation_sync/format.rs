//! Versioned native transcript adapters. No provider instructions or credentials cross the bridge.
use anyhow::{Context, Result, bail, ensure};
use kiln_accounts::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}

fn message(role: &str, text: String) -> Option<Message> {
    (!text.trim().is_empty()).then(|| Message {
        role: role.into(),
        text,
    })
}

fn text_blocks(content: &Value, role: &str) -> Result<Vec<Message>> {
    if let Some(text) = content.as_str() {
        return Ok(message(role, text.into()).into_iter().collect());
    }
    let blocks = content.as_array().context("Unsupported message content")?;
    let mut messages = Vec::new();
    let mut text = String::new();
    for block in blocks {
        match block["type"].as_str().unwrap_or("") {
            "text" | "input_text" | "output_text" => {
                text.push_str(block["text"].as_str().context("Missing message text")?);
            }
            // Thinking/signatures belong to the originating model, not portable conversation.
            "thinking" | "redacted_thinking" => {}
            "tool_use" => {
                if let Some(m) = message(role, std::mem::take(&mut text)) {
                    messages.push(m);
                }
                messages.push(Message {
                    role: "assistant".into(),
                    text: format!(
                        "[Tool call: {}]\n{}",
                        block["name"].as_str().unwrap_or("tool"),
                        serde_json::to_string(&block["input"])?
                    ),
                });
            }
            "tool_result" => {
                if let Some(m) = message(role, std::mem::take(&mut text)) {
                    messages.push(m);
                }
                let result = text_blocks(&block["content"], "assistant")?;
                messages.push(Message {
                    role: "assistant".into(),
                    text: format!(
                        "[Tool result: {}]\n{}",
                        block["tool_use_id"].as_str().unwrap_or("tool"),
                        result
                            .into_iter()
                            .map(|m| m.text)
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                });
            }
            kind => bail!("Unsupported content ({kind}); this conversation was not synchronized"),
        }
    }
    if let Some(m) = message(role, text) {
        messages.push(m);
    }
    Ok(messages)
}

pub fn parse(tool: Tool, rows: &[Value]) -> Result<Vec<Message>> {
    match tool {
        Tool::Claude => claude(rows),
        Tool::Codex => codex(rows),
    }
}

fn claude(rows: &[Value]) -> Result<Vec<Message>> {
    let by_id: HashMap<&str, &Value> = rows
        .iter()
        .filter_map(|r| r["uuid"].as_str().map(|id| (id, r)))
        .collect();
    let Some(mut current) = rows.iter().rev().find(|r| {
        matches!(r["type"].as_str(), Some("user" | "assistant")) && r["isSidechain"] != true
    }) else {
        return Ok(vec![]);
    };
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let id = current["uuid"]
            .as_str()
            .context("Missing Claude message UUID")?;
        ensure!(seen.insert(id), "Cyclic Claude message chain");
        chain.push(current);
        let Some(parent) = current["parentUuid"].as_str() else {
            break;
        };
        current = by_id
            .get(parent)
            .copied()
            .context("Incomplete Claude message chain; waiting for a complete transcript")?;
    }
    chain.reverse();
    let Some(last) = chain.iter().rposition(|r| {
        r["type"] == "assistant"
            && matches!(
                r["message"]["stop_reason"].as_str(),
                Some("end_turn" | "stop_sequence")
            )
    }) else {
        return Ok(vec![]);
    };
    let mut messages = Vec::new();
    for row in &chain[..=last] {
        if row["isMeta"] == true || row["isSidechain"] == true {
            continue;
        }
        let role = match row["type"].as_str() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let content = &row["message"]["content"];
        // CLI commands and their local output are not user prompts to another provider.
        if role == "user"
            && content.as_str().is_some_and(|s| {
                s.starts_with("<local-command-") || s.starts_with("<command-name>")
            })
        {
            continue;
        }
        messages.extend(text_blocks(content, role)?);
    }
    Ok(messages)
}

fn codex(rows: &[Value]) -> Result<Vec<Message>> {
    let meta = rows
        .iter()
        .find(|r| r["type"] == "session_meta")
        .context("Missing Codex session metadata")?;
    ensure!(
        meta["payload"]["history_base"].is_null(),
        "Codex inherited history requires a complete local transcript"
    );
    ensure!(
        meta["payload"]["parent_thread_id"].is_null(),
        "Subagent transcripts are not synchronized"
    );
    let mut user_text = HashSet::new();
    for row in rows {
        let p = &row["payload"];
        if row["type"] == "event_msg" && p["type"] == "user_message" {
            ensure!(
                p["images"].as_array().is_none_or(Vec::is_empty)
                    && p["local_images"].as_array().is_none_or(Vec::is_empty),
                "Images are not yet supported by conversation sync"
            );
            if let Some(t) = p["message"].as_str() {
                user_text.insert(t.to_owned());
            }
        }
        if row["type"] == "event_msg"
            && p["type"] == "item_completed"
            && p["item"]["type"] == "UserMessage"
        {
            for m in text_blocks(&p["item"]["content"], "user")? {
                user_text.insert(m.text);
            }
        }
    }
    let mut messages = Vec::new();
    let mut completed = Vec::new();
    let mut boundaries = Vec::new();
    for row in rows {
        let p = &row["payload"];
        if row["type"] == "event_msg" {
            match p["type"].as_str() {
                Some("task_complete" | "turn_complete") => {
                    completed = messages.clone();
                    boundaries.push(completed.len());
                }
                Some("thread_rolled_back") => {
                    let n = p["num_turns"].as_u64().context("Unsupported rollback")? as usize;
                    boundaries.truncate(boundaries.len().saturating_sub(n));
                    messages.truncate(boundaries.last().copied().unwrap_or(0));
                    completed = messages.clone();
                }
                _ => {}
            }
        }
        if row["type"] != "response_item" {
            continue;
        }
        match p["type"].as_str() {
            Some("message") => match p["role"].as_str() {
                Some("user") => {
                    let parts = text_blocks(&p["content"], "user")?;
                    if parts.iter().any(|m| user_text.contains(&m.text)) {
                        messages.extend(parts);
                    }
                }
                Some("assistant") => messages.extend(text_blocks(&p["content"], "assistant")?),
                _ => {} // system/developer instructions stay with their provider.
            },
            Some("function_call" | "custom_tool_call") => messages.push(Message {
                role: "assistant".into(),
                text: format!(
                    "[Tool call: {}]\n{}",
                    p["name"].as_str().unwrap_or("tool"),
                    p["arguments"]
                        .as_str()
                        .or_else(|| p["input"].as_str())
                        .unwrap_or("")
                ),
            }),
            Some("function_call_output" | "custom_tool_call_output") => {
                let output = if let Some(s) = p["output"].as_str() {
                    s.to_owned()
                } else {
                    text_blocks(&p["output"], "assistant")?
                        .into_iter()
                        .map(|m| m.text)
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                messages.push(Message {
                    role: "assistant".into(),
                    text: format!(
                        "[Tool result: {}]\n{output}",
                        p["call_id"].as_str().unwrap_or("tool")
                    ),
                });
            }
            Some("reasoning" | "compaction") => {}
            Some(kind) => {
                bail!("Unsupported Codex item ({kind}); this conversation was not synchronized")
            }
            None => bail!("Missing Codex response item type"),
        }
    }
    Ok(completed)
}

/// Emit a new immutable session. Native IDs are derived from the snapshot ID, never reused.
pub fn render(
    tool: Tool,
    id: &str,
    cwd: &str,
    timestamp: &str,
    messages: &[Message],
) -> Result<Vec<u8>> {
    ensure!(
        !messages.is_empty() && messages.iter().any(|m| m.role == "user"),
        "No completed user conversation"
    );
    let mut rows = Vec::new();
    match tool {
        Tool::Claude => {
            let mut parent: Option<String> = None;
            for (i, m) in messages.iter().enumerate() {
                let mid = super::stable_id(&format!("{id}:{i}"));
                let payload = if m.role == "user" {
                    json!({"role":"user","content":m.text})
                } else {
                    json!({"id":format!("msg_{mid}"),"type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":m.text}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}})
                };
                rows.push(json!({"type":m.role,"uuid":mid,"parentUuid":parent,"isSidechain":false,"userType":"external","cwd":cwd,"sessionId":id,"timestamp":timestamp,"version":"2.1.296","message":payload}));
                parent = Some(mid);
            }
        }
        Tool::Codex => {
            rows.push(json!({"timestamp":timestamp,"ordinal":0,"type":"session_meta","payload":{"id":id,"session_id":id,"timestamp":timestamp,"cwd":cwd,"originator":"kiln","cli_version":"0.162.1","source":"cli","model_provider":"openai","history_mode":"paginated"}}));
            let turn = super::stable_id(&format!("{id}:turn"));
            rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_started","turn_id":turn,"model_context_window":null,"collaboration_mode_kind":"default"}}));
            for (i, m) in messages.iter().enumerate() {
                let mid = super::stable_id(&format!("{id}:{i}"));
                let mut p = json!({"type":"message","id":mid,"role":m.role,"content":[{"type":if m.role=="user"{"input_text"}else{"output_text"},"text":m.text}]});
                if m.role == "assistant" {
                    p["phase"] = json!("final_answer");
                }
                rows.push(json!({"timestamp":timestamp,"type":"response_item","payload":p,"metadata":{"client_authored":true}}));
                let item = if m.role == "user" {
                    json!({"type":"UserMessage","id":mid,"content":[{"type":"text","text":m.text,"text_elements":[]}]})
                } else {
                    json!({"type":"AgentMessage","id":mid,"content":[{"type":"Text","text":m.text}],"phase":"final_answer"})
                };
                rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"item_completed","thread_id":id,"turn_id":turn,"item":item,"completed_at_ms":0}}));
            }
            rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"last_agent_message":messages.last().map(|m|m.text.as_str())}}));
            for (i, row) in rows.iter_mut().enumerate() {
                row["ordinal"] = json!(i);
            }
        }
    }
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, &row)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
