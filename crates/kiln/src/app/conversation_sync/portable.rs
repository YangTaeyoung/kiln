//! Readable historical tool records, with a lossless local representation.
use serde_json::{Value, json};
use std::borrow::Cow;

pub(super) fn content<'a>(row: &'a Value, value: &'a Value) -> Cow<'a, Value> {
    let metadata = &row["kiln_sync"];
    if metadata["revision"] == 4 {
        if let Some(original) = metadata["original_text"].as_str() {
            let shown = value.as_str().map(str::to_owned).or_else(|| {
                let blocks = value.as_array()?;
                blocks
                    .iter()
                    .all(|b| matches!(b["type"].as_str(), Some("text" | "output_text")))
                    .then(|| {
                        blocks
                            .iter()
                            .filter_map(|b| b["text"].as_str())
                            .collect::<String>()
                    })
            });
            if shown
                .is_some_and(|text| metadata["display_sha256"] == super::digest(text.as_bytes()))
            {
                return Cow::Owned(json!(original));
            }
        }
    }
    Cow::Borrowed(value)
}

pub(super) fn annotate(row: &mut Value, original: &str, shown: &str) {
    if original != shown {
        row["kiln_sync"] = json!({"revision":4,"original_text":original,
            "display_sha256":super::digest(shown.as_bytes())});
    }
}

fn fenced(text: &str, language: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}{language}\n{text}\n{fence}")
}

// Decode complete JSON values only. Malformed/truncated output remains verbatim.
fn json_stream(text: &str, depth: usize) -> Option<String> {
    if depth > 6 {
        return None;
    }
    let text = text.trim();
    if !text.starts_with(['{', '[']) {
        return None;
    }
    let values = serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.is_empty() {
        return None;
    }
    Some(
        values
            .iter()
            .map(|v| readable_value(v, depth + 1))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}
fn readable_value(value: &Value, depth: usize) -> String {
    if depth > 6 {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    }
    match value {
        Value::String(text) => json_stream(text, depth).unwrap_or_else(|| text.clone()),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(i, v)| format!("[{}]\n{}", i + 1, readable_value(v, depth + 1)))
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) if text.contains('\n') || text.starts_with(['{', '[']) => {
                    format!("{key}:\n{}", readable_value(value, depth + 1))
                }
                Value::Object(_) | Value::Array(_) => {
                    format!("{key}:\n{}", readable_value(value, depth + 1))
                }
                Value::String(text) => format!("{key}: {text}"),
                _ => format!("{key}: {value}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => value.to_string(),
    }
}

pub(super) fn display(text: &str) -> String {
    if let Some(rest) = text.strip_prefix("[Tool call: ")
        && let Some((name, body)) = rest.split_once("]\n")
    {
        return call_display(name, body);
    }
    if let Some(rest) = text.strip_prefix("[Tool result: ")
        && let Some((_, body)) = rest.split_once("]\n")
    {
        return result_display(body);
    }
    citations(text)
}

/// The labeled layout of tool records written before `display`.
pub(super) fn labeled(text: &str) -> String {
    for (prefix, label) in [
        ("[Tool call: ", "Recorded tool call"),
        ("[Tool result: ", "Recorded tool result"),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            if let Some((name, body)) = rest.split_once("]\n") {
                let readable = if let Some((status, output)) = body.split_once("\nOutput:\n") {
                    format!(
                        "{status}\nOutput:\n{}",
                        json_stream(output, 0).unwrap_or_else(|| output.into())
                    )
                } else {
                    json_stream(body, 0).unwrap_or_else(|| body.into())
                };
                return format!(
                    "{label}\n{}\n\n{}",
                    fenced(name, "text"),
                    fenced(&readable, "text")
                );
            }
        }
    }
    citations(text)
}

/// Input fields shown in a tool call header, in order of preference.
const SUMMARY: [&str; 10] = ["command", "cmd", "pattern", "query", "url", "file_path", "path", "title", "description", "prompt"];

/// The first line of `value`, at most 80 characters, with an ellipsis when shortened.
fn short(value: &str) -> String {
    let line = value.lines().next().unwrap_or("");
    let mut text: String = line.chars().take(80).collect();
    if line.chars().count() > 80 || value.lines().nth(1).is_some() {
        text.push('…');
    }
    text
}

/// `name(summary)` followed by the other input fields in a code block.
fn call_display(name: &str, body: &str) -> String {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return if body.trim().is_empty() {
            name.into()
        } else {
            format!("{name}\n{}", fenced(body.trim_end(), "text"))
        };
    };
    let summary = SUMMARY
        .iter()
        .find_map(|key| fields.get(*key).and_then(Value::as_str).map(|value| (*key, value)));
    let header = match summary {
        Some((_, value)) => format!("{name}({})", short(value)),
        None => name.into(),
    };
    let rest: Vec<(&String, &Value)> = fields
        .iter()
        .filter(|(key, _)| Some(key.as_str()) != summary.map(|(k, _)| k) && key.as_str() != "description")
        .collect();
    let body = match rest.as_slice() {
        [] => return header,
        [(_, Value::String(value))] => value.clone(),
        _ => rest
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) if text.contains('\n') => format!("{key}:\n{text}"),
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    format!("{header}\n{}", fenced(&body, "text"))
}

/// The output of a tool result: a Codex code-mode result shows the output (and a non-zero
/// exit code) of each command it ran.
fn result_output(body: &str) -> String {
    if body.starts_with("Script ")
        && let Some(split) = body.find("Output:\n").map(|i| i + "Output:\n".len())
        && let Ok(values) = serde_json::Deserializer::from_str(&body[split..])
            .into_iter::<Value>()
            .collect::<Result<Vec<_>, _>>()
        && !values.is_empty()
    {
        return values
            .iter()
            .map(|value| match (value.get("output").and_then(Value::as_str), value.get("exit_code").and_then(Value::as_i64)) {
                (Some(output), Some(code)) if code != 0 => format!("Exit code {code}\n{output}"),
                (Some(output), _) => output.to_owned(),
                _ => readable_value(value, 1),
            })
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    if let Some((status, output)) = body.split_once("\nOutput:\n") {
        return format!("{status}\nOutput:\n{}", json_stream(output, 0).unwrap_or_else(|| output.into()));
    }
    json_stream(body, 0).unwrap_or_else(|| body.into())
}

/// The result output after a `⎿` marker, in a code block.
fn result_display(body: &str) -> String {
    let output = result_output(body);
    let output = output.trim_end_matches('\n');
    let output = if output.trim().is_empty() { "(No output)" } else { output };
    let lines: Vec<String> = output
        .lines()
        .enumerate()
        .map(|(i, line)| if i == 0 { format!("⎿  {line}") } else { format!("   {line}") })
        .collect();
    fenced(&lines.join("\n"), "text")
}

fn citations(text: &str) -> String {
    if let Some((answer, citations)) = text.split_once("<oai-mem-citation>") {
        if citations.trim_end().ends_with("</oai-mem-citation>") {
            return format!(
                "{}\n\nSource references\n{}",
                answer.trim_end(),
                fenced(&format!("<oai-mem-citation>{citations}"), "xml")
            );
        }
    }
    text.into()
}
