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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

fn message(role: &str, text: String) -> Option<Message> {
    (!text.trim().is_empty()).then(|| Message {
        role: role.into(),
        text,
        images: Vec::new(),
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
            "image" | "input_image" => {
                if let Some(m) = message(role, std::mem::take(&mut text)) {
                    messages.push(m);
                }
                messages.push(Message {
                    role: "user".into(),
                    text: String::new(),
                    images: vec![image_url(block)?],
                });
            }
            "tool_reference" => {
                text.push_str(&format!(
                    "[Tool reference: {}]",
                    block["tool_name"]
                        .as_str()
                        .context("Missing tool reference name")?
                ));
            }
            "tool_use" => {
                if let Some(m) = message(role, std::mem::take(&mut text)) {
                    messages.push(m);
                }
                messages.push(Message {
                    images: Vec::new(),
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
                let images: Vec<_> = result
                    .iter()
                    .filter(|m| !m.images.is_empty())
                    .cloned()
                    .collect();
                messages.push(Message {
                    images: Vec::new(),
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
                messages.extend(images);
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
    let mut original_by_id = HashMap::new();
    for row in rows {
        if let Some(id) = row["uuid"].as_str() {
            original_by_id.entry(id).or_insert(row);
        }
    }
    let mut restoring_originals = false;
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
        ensure!(
            seen.insert((id, restoring_originals)),
            "Cyclic Claude message chain"
        );
        // A preserved message can be visited on both sides of a compaction boundary.
        // Keep its original position, using its pre-relink parent from the append-only log.
        if restoring_originals {
            chain.retain(|r: &&Value| r["uuid"] != id);
        }
        chain.push(current);
        let parent = if current["subtype"] == "compact_boundary" {
            restoring_originals = true;
            let boundary_index = rows
                .iter()
                .position(|r| std::ptr::eq(r, current))
                .context("Missing compaction boundary")?;
            let logical = current["logicalParentUuid"].as_str().and_then(|id| {
                original_by_id
                    .get(id)
                    .filter(|r| {
                        rows[..boundary_index]
                            .iter()
                            .any(|before| std::ptr::eq(before, **r))
                    })
                    .map(|_| id)
            });
            // Some native logs relink logicalParentUuid into the new preserved segment.
            // Recover the pre-boundary active branch from its last original message.
            logical
                .or_else(|| {
                    rows[..boundary_index]
                        .iter()
                        .rev()
                        .find(|r| {
                            matches!(r["type"].as_str(), Some("user" | "assistant"))
                                && r["isSidechain"] != true
                                && r["isCompactSummary"] != true
                        })
                        .and_then(|r| r["uuid"].as_str())
                })
                .map(Some)
                .context("Claude compacted history requires a complete original conversation")?
        } else {
            current["parentUuid"].as_str()
        };
        let Some(parent) = parent else {
            break;
        };
        current = (if restoring_originals {
            &original_by_id
        } else {
            &by_id
        })
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
        if row["isCompactSummary"] == true {
            ensure!(
                chain.iter().any(|r| r["subtype"] == "compact_boundary"),
                "Claude compacted history requires a complete original conversation"
            );
            continue;
        }
        if row["isMeta"] == true || row["isSidechain"] == true {
            continue;
        }
        let role = match row["type"].as_str() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let content = super::portable::content(row, &row["message"]["content"]);
        // CLI commands and their local output are not user prompts to another provider.
        if role == "user"
            && content.as_str().is_some_and(|s| {
                s.starts_with("<local-command-") || s.starts_with("<command-name>")
            })
        {
            // Preserve the immutable prefix of older Kiln copies so upgrading the
            // renderer cannot look like a user continuation and echo it back.
            let generated = rows
                .iter()
                .position(|r| std::ptr::eq(r, *row))
                .is_some_and(|index| {
                    row["sessionId"]
                        .as_str()
                        .is_some_and(|id| row["uuid"] == super::stable_id(&format!("{id}:{index}")))
                });
            if !generated {
                continue;
            }
        }
        messages.extend(text_blocks(&content, role)?);
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
    let mut expected_images: HashMap<String, usize> = HashMap::new();
    let mut copied_images: HashMap<String, usize> = HashMap::new();
    let mut raw_answers: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut raw_ids = HashSet::new();
    let mut turn = 0;
    for row in rows {
        let p = &row["payload"];
        if row["type"] == "event_msg" && p["type"] == "task_started" {
            turn += 1;
        }
        if row["type"] == "response_item" && p["type"] == "message" && p["role"] == "assistant" {
            let text = p["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default();
            if let Some(id) = p["id"].as_str() {
                raw_ids.insert((turn, id));
            } else {
                raw_answers.entry(turn).or_default().insert(text);
            }
        }
        if row["type"] == "event_msg" && p["type"] == "user_message" {
            if let Some(t) = p["message"].as_str() {
                user_text.insert(t.to_owned());
                let count = ["images", "local_images"]
                    .iter()
                    .map(|key| p[key].as_array().map_or(0, Vec::len))
                    .sum();
                expected_images
                    .entry(t.to_owned())
                    .and_modify(|n| *n = (*n).max(count))
                    .or_insert(count);
            }
        }
        if row["type"] == "event_msg"
            && p["type"] == "item_completed"
            && p["item"]["type"] == "UserMessage"
        {
            if let Some(blocks) = p["item"]["content"].as_array() {
                let text = blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<String>();
                let count = blocks
                    .iter()
                    .filter(|b| matches!(b["type"].as_str(), Some("image" | "local_image")))
                    .count();
                expected_images
                    .entry(text.clone())
                    .and_modify(|n| *n = (*n).max(count))
                    .or_insert(count);
                user_text.insert(text);
            }
        }
    }
    let mut messages = Vec::new();
    let mut completed_len = 0;
    let mut boundaries = Vec::new();
    turn = 0;
    let mut visible_ids = HashSet::new();
    for row in rows {
        let p = &row["payload"];
        if row["type"] == "event_msg" {
            match p["type"].as_str() {
                Some("task_started") => turn += 1,
                Some("item_completed") if p["item"]["type"] == "AgentMessage" => {
                    let id = p["item"]["id"]
                        .as_str()
                        .context("Missing visible assistant message ID")?;
                    let content = p["item"]["content"]
                        .as_array()
                        .context("Missing visible assistant message content")?;
                    ensure!(
                        content.iter().all(|b| b["type"] == "Text"),
                        "Unsupported visible assistant content"
                    );
                    let text = content
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<String>();
                    // Some native/imported logs persist a visible answer without a Responses
                    // message. Preserve that original answer; never duplicate both projections.
                    if visible_ids.insert(id)
                        && !raw_ids.contains(&(turn, id))
                        && !raw_answers
                            .get(&turn)
                            .is_some_and(|answers| answers.contains(&text))
                    {
                        if let Some(m) = message("assistant", text) {
                            messages.push(m);
                        }
                    }
                }
                Some("task_complete" | "turn_complete") => {
                    completed_len = messages.len();
                    boundaries.push(completed_len);
                }
                Some("thread_rolled_back") => {
                    let n = p["num_turns"].as_u64().context("Unsupported rollback")? as usize;
                    boundaries.truncate(boundaries.len().saturating_sub(n));
                    messages.truncate(boundaries.last().copied().unwrap_or(0));
                    completed_len = messages.len();
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
                    let plain = p["content"]
                        .as_array()
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter(|b| {
                                    matches!(b["type"].as_str(), Some("text" | "input_text"))
                                })
                                .filter_map(|b| b["text"].as_str())
                                .collect::<String>()
                        })
                        .unwrap_or_default();
                    let matched = p["content"]
                        .as_array()
                        .and_then(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|b| b["text"].as_str())
                                .find(|text| user_text.contains(*text))
                        })
                        .map(str::to_owned)
                        .or_else(|| user_text.contains(&plain).then_some(plain));
                    if let Some(matched) = matched {
                        let parts = text_blocks(&p["content"], "user")?;
                        *copied_images.entry(matched).or_default() +=
                            parts.iter().map(|m| m.images.len()).sum::<usize>();
                        messages.extend(parts);
                    }
                }
                Some("assistant") => messages.extend(text_blocks(
                    &super::portable::content(row, &p["content"]),
                    "assistant",
                )?),
                _ => {} // system/developer instructions stay with their provider.
            },
            Some("function_call" | "custom_tool_call") => messages.push(Message {
                images: Vec::new(),
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
                let result = text_blocks(&p["output"], "assistant")?;
                let output = result
                    .iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                messages.push(Message {
                    images: Vec::new(),
                    role: "assistant".into(),
                    text: format!(
                        "[Tool result: {}]\n{output}",
                        p["call_id"].as_str().unwrap_or("tool")
                    ),
                });
                messages.extend(result.into_iter().filter(|m| !m.images.is_empty()));
            }
            // Encrypted collaboration records belong to the originating provider.
            Some("agent_message") => {
                if p["content"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|b| b["type"] == "encrypted_content"))
                {
                    continue;
                }
                messages.extend(text_blocks(&p["content"], "assistant")?);
            }
            Some("reasoning" | "compaction") => {}
            Some(kind) => {
                bail!("Unsupported Codex item ({kind}); this conversation was not synchronized")
            }
            None => bail!("Missing Codex response item type"),
        }
    }
    ensure!(
        expected_images
            .iter()
            .all(|(text, count)| copied_images.get(text).copied().unwrap_or(0) >= *count),
        "An image is referenced by the conversation but its original bytes are absent; this conversation was not synchronized"
    );
    messages.truncate(completed_len);
    Ok(messages)
}

/// Emit a new immutable session. Native IDs are derived from the snapshot ID, never reused.
pub fn render(
    tool: Tool,
    id: &str,
    cwd: &str,
    timestamp: &str,
    messages: &[Message],
) -> Result<Vec<u8>> {
    let title = super::titles::fallback(messages);
    render_titled(tool, id, cwd, timestamp, messages, &title)
}

pub fn render_titled(
    tool: Tool,
    id: &str,
    cwd: &str,
    timestamp: &str,
    messages: &[Message],
    title: &str,
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
                let shown = if m.role == "assistant" {
                    super::portable::display(&m.text)
                } else {
                    m.text.clone()
                };
                let payload = if m.role == "user" {
                    json!({"role":"user","content":claude_content(m)?})
                } else {
                    json!({"id":format!("msg_{mid}"),"type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":shown}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}})
                };
                rows.push(json!({"type":m.role,"uuid":mid,"parentUuid":parent,"isSidechain":false,"userType":"external","cwd":cwd,"sessionId":id,"timestamp":timestamp,"version":"2.1.296","message":payload}));
                super::portable::annotate(rows.last_mut().unwrap(), &m.text, &shown);
                parent = Some(mid);
            }
            // Native pickers inspect bounded head/tail windows. A long initial
            // message can fill the head before a complete prompt is available.
            rows.push(
                json!({"type":"custom-title","sessionId":id,"customTitle":title,
                "uuid":super::stable_id(&format!("{id}:title")),"timestamp":timestamp,"kiln_projection":4}),
            );
        }
        Tool::Codex => {
            rows.push(json!({"timestamp":timestamp,"ordinal":0,"type":"session_meta","payload":{"id":id,"session_id":id,"timestamp":timestamp,"cwd":cwd,"originator":"kiln","cli_version":"0.162.1","source":"cli","model_provider":"openai","history_mode":"paginated"}}));
            let turn = super::stable_id(&format!("{id}:turn"));
            rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_started","turn_id":turn,"model_context_window":null,"collaboration_mode_kind":"default"}}));
            for (i, m) in messages.iter().enumerate() {
                let mid = super::stable_id(&format!("{id}:{i}"));
                let shown = if m.role == "assistant" {
                    super::portable::display(&m.text)
                } else {
                    m.text.clone()
                };
                let mut content = Vec::new();
                if !m.text.is_empty() {
                    content.push(json!({"type":if m.role=="user"{"input_text"}else{"output_text"},"text":shown}));
                }
                content.extend(
                    m.images
                        .iter()
                        .map(|url| json!({"type":"input_image","image_url":url})),
                );
                let mut p = json!({"type":"message","id":mid,"role":m.role,"content":content});
                if m.role == "assistant" {
                    p["phase"] = json!("final_answer");
                }
                rows.push(json!({"timestamp":timestamp,"type":"response_item","payload":p,"metadata":{"client_authored":true}}));
                super::portable::annotate(rows.last_mut().unwrap(), &m.text, &shown);
                let item = if m.role == "user" {
                    {
                        let mut content = Vec::new();
                        if !m.text.is_empty() {
                            content.push(json!({"type":"text","text":m.text,"text_elements":[]}));
                        }
                        content.extend(
                            m.images
                                .iter()
                                .map(|url| json!({"type":"image","image_url":url})),
                        );
                        json!({"type":"UserMessage","id":mid,"content":content})
                    }
                } else {
                    json!({"type":"AgentMessage","id":mid,"content":[{"type":"Text","text":shown}],"phase":"final_answer"})
                };
                rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"item_completed","thread_id":id,"turn_id":turn,"item":item,"completed_at_ms":0}}));
            }
            rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"last_agent_message":null}}));
            rows.last_mut().unwrap()["kiln_projection"] = json!(4);
            rows.last_mut().unwrap()["kiln_title"] = json!(title);
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

fn image_url(block: &Value) -> Result<String> {
    let url = if block["type"] == "input_image" {
        block["image_url"]
            .as_str()
            .context("Provider file-ID image cannot be transferred without its original bytes")?
            .to_owned()
    } else {
        let source = &block["source"];
        match source["type"].as_str() {
            Some("base64") => format!(
                "data:{};base64,{}",
                source["media_type"]
                    .as_str()
                    .context("Missing image media type")?,
                source["data"].as_str().context("Missing image bytes")?
            ),
            Some("url") => source["url"]
                .as_str()
                .context("Missing image URL")?
                .to_owned(),
            _ => bail!("Provider file-ID image cannot be transferred without its original bytes"),
        }
    };
    if url.starts_with("data:") {
        use base64::Engine;
        let (mime, data) = url[5..]
            .split_once(";base64,")
            .context("Unsupported inline image encoding")?;
        ensure!(
            matches!(
                mime,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            ),
            "Unsupported image media type"
        );
        ensure!(
            !base64::engine::general_purpose::STANDARD
                .decode(data)?
                .is_empty(),
            "Missing image bytes"
        );
    } else {
        ensure!(
            url.starts_with("https://") || url.starts_with("http://"),
            "Unsupported image URL"
        );
    }
    Ok(url)
}
fn claude_content(m: &Message) -> Result<Value> {
    if m.images.is_empty()
        && !m.text.starts_with("<local-command-")
        && !m.text.starts_with("<command-name>")
    {
        return Ok(json!(m.text));
    }
    let mut blocks = Vec::new();
    if !m.text.is_empty() {
        blocks.push(json!({"type":"text","text":m.text}));
    }
    for url in &m.images {
        let source = if let Some(data) = url.strip_prefix("data:") {
            let (mime, data) = data
                .split_once(";base64,")
                .context("Unsupported image encoding")?;
            json!({"type":"base64","media_type":mime,"data":data})
        } else {
            json!({"type":"url","url":url})
        };
        blocks.push(json!({"type":"image","source":source}));
    }
    Ok(json!(blocks))
}
