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

/// Where a native history's last completed turn ends: the index of that row and, for
/// Claude, the UUID of the message that completed it.
pub(super) fn completion(tool: Tool, rows: &[Value]) -> Result<Option<(usize, Option<String>)>> {
    match tool {
        Tool::Claude => Ok(claude_chain(rows, None)?.and_then(|(_, row)| {
            let row = row?;
            let index = rows.iter().position(|r| std::ptr::eq(r, row))?;
            Some((index, row["uuid"].as_str().map(str::to_owned)))
        })),
        Tool::Codex => Ok(rows
            .iter()
            .rposition(|r| {
                r["type"] == "event_msg"
                    && matches!(r["payload"]["type"].as_str(), Some("task_complete" | "turn_complete" | "turn_aborted"))
            })
            .map(|index| (index, None))),
    }
}

/// Messages of rows appended after a completed turn, given the Claude message that ended it
/// or the Codex session metadata; `None` when the rows cannot be read on their own.
pub(super) fn parse_appended(
    tool: Tool,
    leaf: Option<&str>,
    meta: Option<&Value>,
    rows: &[Value],
) -> Result<Option<(Vec<Message>, Option<(usize, Option<String>)>)>> {
    match tool {
        Tool::Claude => {
            let Some(leaf) = leaf else { return Ok(None) };
            let Some((messages, row)) = claude_chain(rows, Some(leaf))? else {
                return Ok(None);
            };
            let end = row.and_then(|row| {
                let index = rows.iter().position(|r| std::ptr::eq(r, row))?;
                Some((index, row["uuid"].as_str().map(str::to_owned)))
            });
            Ok(Some((messages, end)))
        }
        Tool::Codex => {
            let Some(meta) = meta else { return Ok(None) };
            if rows.iter().any(|r| r["type"] == "event_msg" && r["payload"]["type"] == "thread_rolled_back") {
                return Ok(None);
            }
            let mut all = Vec::with_capacity(rows.len() + 1);
            all.push(meta.clone());
            all.extend_from_slice(rows);
            let messages = codex(&all)?;
            Ok(Some((messages, completion(Tool::Codex, rows)?)))
        }
    }
}

pub fn parse(tool: Tool, rows: &[Value]) -> Result<Vec<Message>> {
    match tool {
        Tool::Claude => claude(rows),
        Tool::Codex => codex(rows),
    }
}

pub(super) fn claude_interruption(row: &Value) -> Option<&str> {
    if row["type"] != "user" || row["promptId"].as_str().is_none() { return None; }
    let content = &row["message"]["content"];
    let text = content.as_str().or_else(|| {
        let blocks = content.as_array()?;
        (blocks.len() == 1).then(|| blocks[0]["text"].as_str()).flatten()
    })?;
    matches!(text, "[Request interrupted by user]" | "[Request interrupted by user for tool use]").then_some(text)
}

/// The canonical text Kiln recorded beside a native tool record it wrote, while the record
/// is unchanged.
fn tool_original(row: &Value) -> Result<Option<&str>> {
    let metadata = &row["kiln_sync"];
    if metadata["revision"] != 5 {
        return Ok(None);
    }
    let content = serde_json::to_string(&row["message"]["content"])?;
    Ok(metadata["original_text"]
        .as_str()
        .filter(|_| metadata["content_sha256"].as_str() == Some(super::digest(content.as_bytes()).as_str())))
}

/// Records the canonical text of a native tool record whose own text differs from it.
fn annotate_tool(row: &mut Value, original: &str) -> Result<()> {
    let content = &row["message"]["content"];
    let own = text_blocks(content, "assistant")?
        .into_iter()
        .map(|m| m.text)
        .collect::<Vec<_>>()
        .join("\n");
    if own != original {
        let digest = super::digest(serde_json::to_string(content)?.as_bytes());
        row["kiln_sync"] = json!({"revision": 5, "original_text": original, "content_sha256": digest});
    }
    Ok(())
}

/// Tool calls rendered as native records, by the index of their call message: a call and its
/// result followed by another assistant message, outside a group of parallel calls.
fn tool_pairs(messages: &[Message]) -> HashMap<usize, super::tools::Pair> {
    let mut pairs = HashMap::new();
    let mut i = 0;
    while i + 2 < messages.len() {
        let (call, result) = (&messages[i], &messages[i + 1]);
        let candidate = call.role == "assistant"
            && result.role == "assistant"
            && messages[i + 2].role == "assistant"
            && call.images.is_empty()
            && result.images.is_empty()
            && !(i > 0 && messages[i - 1].text.starts_with("[Tool call: "))
            && !messages[i + 2].text.starts_with("[Tool result: ");
        if candidate && let Some(pair) = super::tools::pair(&call.text, &result.text) {
            pairs.insert(i, pair);
            i += 2;
        } else {
            i += 1;
        }
    }
    pairs
}

fn claude(rows: &[Value]) -> Result<Vec<Message>> {
    Ok(claude_chain(rows, None)?.map(|(messages, _)| messages).unwrap_or_default())
}

/// Messages of the active Claude chain up to its last completed turn, with the row that
/// completed it. With `stop_at`, `rows` are appended after that message: the chain must
/// continue from it without compaction or interruption records, otherwise `None`.
fn claude_chain<'a>(rows: &'a [Value], stop_at: Option<&str>) -> Result<Option<(Vec<Message>, Option<&'a Value>)>> {
    if stop_at.is_some()
        && rows.iter().any(|r| {
            r["subtype"] == "compact_boundary" || r["isCompactSummary"] == true || claude_interruption(r).is_some()
        })
    {
        return Ok(None);
    }
    let interruptions: HashMap<&str, &Value> = rows.iter().filter(|row| claude_interruption(row).is_some())
        .filter_map(|row| Some((row["promptId"].as_str()?, row))).collect();
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
        return Ok(Some((vec![], None)));
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
            let parent = current["parentUuid"].as_str();
            if claude_interruption(current).is_some() && parent.is_some_and(|id| !by_id.contains_key(id)) {
                // Native 2.1.299 can flush the interruption before its prompt and
                // reference an assistant UUID which was never persisted. promptId
                // still identifies the original user record without text matching.
                rows.iter().rev().find(|row| row["type"] == "user" && row["promptId"] == current["promptId"] && claude_interruption(row).is_none())
                    .and_then(|row| row["uuid"].as_str()).context("Interrupted Claude prompt is missing")?.into()
            } else { parent }
        };
        if let Some(stop) = stop_at {
            match parent {
                Some(parent) if parent == stop => break,
                Some(parent) if by_id.contains_key(parent) => {}
                _ => return Ok(None),
            }
        }
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
        (r["type"] == "assistant"
            && matches!(
                r["message"]["stop_reason"].as_str(),
                Some("end_turn" | "stop_sequence")
            )) || r["promptId"].as_str().is_some_and(|id| interruptions.contains_key(id))
    }) else {
        return Ok(Some((vec![], None)));
    };
    let mut messages = Vec::new();
    let mut emitted_interruptions = HashSet::new();
    for (index, row) in chain[..=last].iter().enumerate() {
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
        if let Some(original) = tool_original(row)? {
            messages.push(Message { role: "assistant".into(), text: original.into(), images: vec![] });
            continue;
        }
        let content = super::portable::content(row, &row["message"]["content"]);
        // CLI commands and their local output are not user prompts to another provider.
        if role == "user"
            && content.as_str().is_some_and(|s| {
                s.starts_with("<local-command-") || s.starts_with("<command-name>")
            })
        {
            // Preserve the immutable prefix of older Kiln copies so upgrading the
            // renderer cannot look like a user continuation and echo it back.
            let generated = stop_at.is_none() && rows
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
        if let Some(prompt) = row["promptId"].as_str()
            && let Some(interruption) = interruptions.get(prompt)
            && (index == last || chain[index + 1]["promptId"].as_str() != Some(prompt))
            && emitted_interruptions.insert(prompt)
        {
            if !chain[..=index].iter().any(|r| std::ptr::eq(*r, *interruption)) {
                messages.extend(text_blocks(&interruption["message"]["content"], "user")?);
            }
            messages.push(Message { role:"assistant".into(), text:format!("[Session event: interrupted]\n{}", claude_interruption(interruption).unwrap()), images:vec![] });
        }
    }
    Ok(Some((messages, Some(chain[last]))))
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
                Some("turn_aborted") => {
                    messages.push(Message { role:"assistant".into(), text:format!("[Session event: turn_aborted]\n{p}"), images:vec![] });
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
#[cfg(test)]
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
    render_messages(tool, id, cwd, timestamp, messages, title)
}

fn render_messages(
    tool: Tool, id: &str, cwd: &str, timestamp: &str,
    messages: &[Message], title: &str,
) -> Result<Vec<u8>> {
    let mut rows = Vec::new();
    match tool {
        Tool::Claude => {
            let pairs = tool_pairs(messages);
            let mut parent: Option<String> = None;
            let mut skip = false;
            for (i, m) in messages.iter().enumerate() {
                if std::mem::take(&mut skip) {
                    continue;
                }
                let mid = super::stable_id(&format!("{id}:{i}"));
                if let Some((call, result, structured)) =
                    pairs.get(&i).and_then(|pair| pair.claude(&format!("{id}:{i}:tool"), cwd))
                {
                    let rid = super::stable_id(&format!("{id}:{}", i + 1));
                    let mut row = json!({"type":"assistant","uuid":mid,"parentUuid":parent,"isSidechain":false,"userType":"external","cwd":cwd,"sessionId":id,"timestamp":timestamp,"version":"2.1.296",
                        "message":{"id":format!("msg_{mid}"),"type":"message","role":"assistant","model":"<synthetic>","content":[call],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}});
                    annotate_tool(&mut row, &m.text)?;
                    rows.push(row);
                    let mut row = json!({"type":"user","uuid":rid,"parentUuid":mid,"isSidechain":false,"userType":"external","cwd":cwd,"sessionId":id,"timestamp":timestamp,"version":"2.1.296",
                        "message":{"role":"user","content":[result]},"sourceToolAssistantUUID":mid});
                    if let Some(structured) = structured {
                        row["toolUseResult"] = structured;
                    }
                    annotate_tool(&mut row, &messages[i + 1].text)?;
                    rows.push(row);
                    parent = Some(rid);
                    skip = true;
                    continue;
                }
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
                "uuid":super::stable_id(&format!("{id}:title")),"timestamp":timestamp,"kiln_projection":super::PROJECTION_REVISION}),
            );
        }
        Tool::Codex => {
            rows.push(json!({"timestamp":timestamp,"ordinal":0,"type":"session_meta","payload":{"id":id,"session_id":id,"timestamp":timestamp,"cwd":cwd,"originator":"kiln","cli_version":"0.162.1","source":"cli","model_provider":"openai","history_mode":"paginated"}}));
            let turn = super::stable_id(&format!("{id}:turn"));
            rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_started","turn_id":turn,"model_context_window":null,"collaboration_mode_kind":"default"}}));
            let pairs = tool_pairs(messages);
            for (i, m) in messages.iter().enumerate() {
                let mid = super::stable_id(&format!("{id}:{i}"));
                // A tool call and its result: native items when Codex recorded them, and one
                // history item in place of their two agent messages.
                let pair = pairs
                    .get(&i)
                    .map(|pair| (pair, true))
                    .or_else(|| i.checked_sub(1).and_then(|j| pairs.get(&j)).map(|pair| (pair, false)));
                if let Some((pair, first)) = pair
                    && let Some((call, output)) = pair.codex_items()
                {
                    rows.push(json!({"timestamp":timestamp,"type":"response_item","payload":if first { call } else { output }}));
                    if !first {
                        let exec = format!("exec-{}", super::stable_id(&format!("{id}:{i}:exec")));
                        rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"item_completed","thread_id":id,"turn_id":turn,"item":pair.codex_cell(&exec, cwd),"started_at_ms":0,"completed_at_ms":0}}));
                    }
                    continue;
                }
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
                if let Some((pair, first)) = pair {
                    if !first {
                        let exec = format!("exec-{}", super::stable_id(&format!("{id}:{i}:exec")));
                        rows.push(json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"item_completed","thread_id":id,"turn_id":turn,"item":pair.codex_cell(&exec, cwd),"started_at_ms":0,"completed_at_ms":0}}));
                    }
                    continue;
                }
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
            rows.last_mut().unwrap()["kiln_projection"] = json!(super::PROJECTION_REVISION);
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

/// A separate seed makes appended message IDs distinct from every earlier batch.
/// Session identity stays fixed, and Claude's first new parent is the actual
/// native leaf, not a reconstructed/imported message ID.
pub(super) fn render_suffix(
    tool: Tool, id: &str, cwd: &str, timestamp: &str, messages: &[Message],
    title: &str, existing: &[Value], checkpoint: &str, last_ordinal: Option<u64>,
) -> Result<Vec<u8>> {
    let seed = super::stable_id(&format!("{id}:append:{checkpoint}"));
    // An existing conversation can append only recorded assistant/tool output.
    // Validate the full history in canonical::update, without inventing a user.
    let mut rows: Vec<Value> = if messages.is_empty() {
        vec![]
    } else {
        let bytes = render_messages(tool, &seed, cwd, timestamp, messages, title)?;
        bytes.split(|b| *b == b'\n').filter(|b| !b.is_empty())
            .map(serde_json::from_slice).collect::<std::result::Result<_, _>>()?
    };
    match tool {
        Tool::Claude => {
            let parent = existing.iter().rev().find(|r| {
                matches!(r["type"].as_str(), Some("user" | "assistant")) && r["isSidechain"] != true
            }).and_then(|r| r["uuid"].as_str()).context("Missing native Claude leaf")?;
            if let Some(first) = rows.first_mut() {
                first["parentUuid"] = json!(parent);
            } else {
                rows.push(json!({"type":"custom-title", "customTitle":title, "kiln_projection":super::PROJECTION_REVISION,
                    "uuid":super::stable_id(&format!("{seed}:title")), "timestamp":timestamp}));
            }
            for row in &mut rows { row["sessionId"] = json!(id); }
        }
        Tool::Codex => {
            rows.retain(|r| r["type"] != "session_meta");
            if rows.is_empty() {
                rows.push(json!({"timestamp":timestamp,"type":"event_msg",
                    "payload":{"type":"task_complete","turn_id":seed,"last_agent_message":null},
                    "kiln_projection":super::PROJECTION_REVISION,"kiln_title":title}));
            }
            let ordinal = last_ordinal.unwrap_or(0).checked_add(1).context("Native transcript ordinal overflow")?;
            for (i, row) in rows.iter_mut().enumerate() {
                row["ordinal"] = json!(ordinal.checked_add(i as u64).context("Native transcript ordinal overflow")?);
                if row["payload"]["type"] == "item_completed" { row["payload"]["thread_id"] = json!(id); }
            }
        }
    }
    let mut bytes = Vec::new();
    for row in rows { serde_json::to_writer(&mut bytes, &row)?; bytes.push(b'\n'); }
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
