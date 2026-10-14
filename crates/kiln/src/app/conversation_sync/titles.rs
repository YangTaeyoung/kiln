//! Native resume titles. Only title metadata is pinned; transcript bodies are immutable.
use super::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Seek, SeekFrom};

pub(super) fn clean(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .into()
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
#[derive(Default)]
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

pub(super) fn pin_claude(source: &Source, title: &str) -> Result<()> {
    if source.tool != Tool::Claude {
        return Ok(());
    }
    let path = sidecar(source)?;
    if !path.exists() {
        match publish(&path, &serde_json::to_vec(&json!({"customTitle":title}))?) {
            Ok(()) => {}
            Err(_) if path.exists() => {} // A concurrent native rename wins.
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn projection(source: &Source) -> Result<Option<String>> {
    let mut file = fs::File::open(&source.path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
    let mut tail = Vec::new();
    file.take(4096).read_to_end(&mut tail)?;
    Ok(tail
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|r| r["kiln_projection"] == 4)
        .filter_map(|r| {
            r[if source.tool == Tool::Claude {
                "customTitle"
            } else {
                "kiln_title"
            }]
            .as_str()
            .map(clean)
        })
        .last())
}
