//! Parser checkpoints: where a native history's last completed turn ends, so later reads
//! parse only the records appended after it.
use super::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    /// Byte offset just after the record that completed the last turn.
    pub offset: u64,
    /// SHA-256 of up to `WINDOW` bytes of the history ending at `offset`.
    pub window: String,
    /// Claude: UUID of the message that completed the last turn.
    pub leaf: Option<String>,
    /// Codex: the session metadata record.
    pub meta: Option<Value>,
}

const WINDOW: u64 = 64 * 1024;

pub(super) fn window_of(bytes: &[u8], offset: u64) -> String {
    let end = offset as usize;
    digest(&bytes[end.saturating_sub(WINDOW as usize)..end])
}

fn window(source: &Source, offset: u64) -> Result<String> {
    let physical = storage::physical(source);
    let start = offset.saturating_sub(WINDOW);
    let (input, _) = storage::open_at(&source.root, &physical, start)?;
    let mut bytes = Vec::new();
    input.take(offset - start).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == offset - start,
        "Transcript changed while reading; retrying"
    );
    Ok(digest(&bytes))
}

fn at(
    source: &Source,
    offsets: &[u64],
    end: Option<(usize, Option<String>)>,
    leaf: Option<String>,
    meta: Option<Value>,
) -> Result<Option<Checkpoint>> {
    let Some((index, completed)) = end else { return Ok(None) };
    let offset = offsets[index];
    Ok(Some(Checkpoint { offset, window: window(source, offset)?, leaf: completed.or(leaf), meta }))
}

/// Reads the whole history and the checkpoint after its last completed turn.
pub(super) fn read(source: &Source, title: &mut String) -> Result<(Vec<format::Message>, Option<Checkpoint>)> {
    let (rows, offsets, messages) = read_rows_offsets_titled(source, title)?;
    if storage::compressed(&storage::physical(source)) {
        return Ok((messages, None));
    }
    let meta = (source.tool == Tool::Codex)
        .then(|| rows.iter().find(|r| r["type"] == "session_meta").cloned())
        .flatten();
    let end = format::completion(source.tool, &rows)?;
    Ok((messages, at(source, &offsets, end, None, meta)?))
}

/// Messages completed after `checkpoint`, the newest Claude custom title among the appended
/// records, and the next checkpoint; `None` when the history changed before the checkpoint
/// or the appended records need a full read.
pub(super) fn since(
    source: &Source,
    checkpoint: &Checkpoint,
) -> Result<Option<(Vec<format::Message>, Checkpoint)>> {
    Ok(since_titled(source, checkpoint)?.map(|(messages, _, next)| (messages, next)))
}
pub(super) fn since_titled(
    source: &Source,
    checkpoint: &Checkpoint,
) -> Result<Option<(Vec<format::Message>, Option<String>, Checkpoint)>> {
    if storage::compressed(&storage::physical(source)) {
        return Ok(None);
    }
    let len = storage::metadata(source)?.len();
    if len < checkpoint.offset || window(source, checkpoint.offset)? != checkpoint.window {
        return Ok(None);
    }
    let (rows, offsets) = reader::rows_from(source, &mut String::new(), checkpoint.offset)?;
    if source.tool == Tool::Claude
        && rows
            .iter()
            .any(|r| matches!(r["type"].as_str(), Some("user" | "assistant")) && r["sessionId"] != source.id)
    {
        return Ok(None);
    }
    let Some((messages, end)) = format::parse_appended(
        source.tool,
        checkpoint.leaf.as_deref(),
        checkpoint.meta.as_ref(),
        &rows,
    )?
    else {
        return Ok(None);
    };
    let title = (source.tool == Tool::Claude)
        .then(|| {
            rows.iter()
                .rev()
                .find_map(|r| r["customTitle"].as_str().map(titles::clean).filter(|t| !t.is_empty()))
        })
        .flatten();
    let next = match end {
        Some(_) => at(source, &offsets, end, checkpoint.leaf.clone(), checkpoint.meta.clone())?.unwrap(),
        None => checkpoint.clone(),
    };
    Ok(Some((messages, title, next)))
}

/// The checkpoint of a history Kiln has just written in full.
pub(super) fn of_bytes(tool: Tool, bytes: &[u8]) -> Result<Option<Checkpoint>> {
    let mut rows = Vec::new();
    let mut offsets = Vec::new();
    let mut position = 0u64;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        position += line.len() as u64;
        if line != b"\n" {
            rows.push(serde_json::from_slice::<Value>(line)?);
            offsets.push(position);
        }
    }
    let meta = (tool == Tool::Codex)
        .then(|| rows.iter().find(|r| r["type"] == "session_meta").cloned())
        .flatten();
    Ok(format::completion(tool, &rows)?.map(|(index, leaf)| Checkpoint {
        offset: offsets[index],
        window: window_of(bytes, offsets[index]),
        leaf,
        meta,
    }))
}
