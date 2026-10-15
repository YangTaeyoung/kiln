//! Native resume titles. Only title metadata is pinned; transcript bodies are immutable.
use super::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};

/// Removes CLI command elements (`<command-…>`, `<local-command-…>`) and image
/// placeholders (`<image …>`, `[Image #n]`) that are not part of a readable title.
fn strip_markup(value: &str) -> String {
    let mut text = value.to_owned();
    for prefix in ["<command-", "<local-command-"] {
        while let Some(start) = text.find(prefix) {
            let name_end = text[start + 1..]
                .find(|c: char| c == '>' || c.is_whitespace())
                .map_or(text.len(), |i| start + 1 + i);
            let close = format!("</{}>", &text[start + 1..name_end]);
            let end = text[start..]
                .find(&close)
                .map(|i| start + i + close.len())
                .or_else(|| text[start..].find('>').map(|i| start + i + 1))
                .unwrap_or(text.len());
            text.replace_range(start..end, " ");
        }
    }
    for (open, close) in [("<image", ">"), ("</image", ">"), ("[Image #", "]")] {
        while let Some(start) = text.find(open) {
            let end = text[start..].find(close).map_or(text.len(), |i| start + i + close.len());
            text.replace_range(start..end, " ");
        }
    }
    text
}

pub(super) fn clean(value: &str) -> String {
    strip_markup(value)
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .into()
}
pub(super) fn fallback(messages: &[format::Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == "user")
        .flat_map(|m| strip_markup(&m.text).lines().map(clean).collect::<Vec<_>>())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "Linked conversation".into())
}
fn sidecar(source: &Source) -> Result<PathBuf> {
    let path = source
        .path
        .parent()
        .context("Missing project directory")?
        .join(&source.id)
        .join("custom-title.json");
    safe_path(&source.root, &path)?;
    Ok(path)
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Cache {
    indexes: HashMap<PathBuf, (u64, SystemTime, HashMap<String, String>)>,
}
impl Cache {
    pub fn name(&mut self, source: &Source) -> Result<Option<String>> {
        if source.tool == Tool::Claude {
            let path = sidecar(source)?;
            return if path.exists() {
                let row: Value = serde_json::from_slice(&bounded(&path, 16 * 1024)?)?;
                Ok(row["customTitle"]
                    .as_str()
                    .map(clean)
                    .filter(|s| !s.is_empty()))
            } else {
                Ok(None)
            };
        }
        let path = source.root.join("session_index.jsonl");
        safe_path(&source.root, &path)?;
        if !path.exists() {
            self.indexes.remove(&source.root);
            return Ok(None);
        }
        let meta = fs::metadata(&path)?;
        let modified = meta.modified()?;
        if !self
            .indexes
            .get(&source.root)
            .is_some_and(|(len, stamp, _)| *len == meta.len() && *stamp == modified)
        {
            ensure!(
                meta.len() <= 64 * 1024 * 1024,
                "Native title index exceeds its reading limit"
            );
            let mut reader = BufReader::new(fs::File::open(&path)?.take(meta.len()));
            let mut names = HashMap::new();
            let mut line = Vec::new();
            loop {
                line.clear();
                let count = reader
                    .by_ref()
                    .take(64 * 1024 + 1)
                    .read_until(b'\n', &mut line)?;
                if count == 0 || !line.ends_with(b"\n") {
                    break;
                }
                ensure!(
                    line.len() <= 64 * 1024,
                    "Native title index record exceeds its reading limit"
                );
                let Ok(row) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                if let (Some(id), Some(name)) = (row["id"].as_str(), row["thread_name"].as_str()) {
                    names.insert(id.into(), clean(name));
                }
            }
            self.indexes
                .insert(source.root.clone(), (meta.len(), modified, names));
        }
        Ok(self.indexes[&source.root]
            .2
            .get(&source.id)
            .filter(|s| !s.is_empty())
            .cloned())
    }
}

pub(super) fn from_rows(source: &Source, rows: &[Value], preferred: &str) -> String {
    if source.tool == Tool::Claude {
        let valid = |r: &&Value| r["sessionId"].is_null() || r["sessionId"] == source.id;
        if let Some(title) = rows.iter().rev().filter(valid).find_map(|r| {
            r["customTitle"]
                .as_str()
                .map(clean)
                .filter(|s| !s.is_empty())
        }) {
            return title;
        }
        if !preferred.is_empty() {
            return preferred.into();
        }
        if let Some(title) = rows
            .iter()
            .rev()
            .filter(valid)
            .find_map(|r| r["aiTitle"].as_str().map(clean).filter(|s| !s.is_empty()))
        {
            return title;
        }
    } else if !preferred.is_empty() {
        return preferred.into();
    } else if let Some(title) = rows
        .iter()
        .rev()
        .find_map(|r| r["kiln_title"].as_str().map(clean))
    {
        return title;
    }
    String::new()
}

/// Writes the title to the Claude title sidecar.
pub(super) fn pin_claude(source: &Source, title: &str) -> Result<()> {
    if source.tool != Tool::Claude {
        return Ok(());
    }
    let path = sidecar(source)?;
    private_dir(path.parent().context("Missing title directory")?)?;
    replace_changed(&path, &serde_json::to_vec(&json!({"customTitle":title}))?)
}

pub(super) fn projection(source: &Source) -> Result<Option<String>> {
    Ok(raw_projection(source)?.map(|s| clean(&s)))
}
pub(super) fn raw_projection(source: &Source) -> Result<Option<String>> {
    let tail = storage::tail(source, 4096)?;
    Ok(tail
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|r| matches!(r["kiln_projection"].as_u64(), Some(4 | 5)))
        .filter_map(|r| {
            r[if source.tool == Tool::Claude {
                "customTitle"
            } else {
                "kiln_title"
            }]
            .as_str()
            .map(str::to_owned)
        })
        .last())
}
