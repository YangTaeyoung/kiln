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
