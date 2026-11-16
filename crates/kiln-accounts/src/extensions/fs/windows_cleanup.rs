//! Immutable cleanup ownership plan plus a compact postorder progress cursor.
//! The parent publication intent records completed targets before these metadata
//! files are retired, so a crash during metadata removal cannot reauthorize data.
use super::super::{Budget, MAX_DEPTH, TOTAL_BYTES, TOTAL_FILES};
use super::{
    R, component, relative,
    windows::{CleanupNode, Dir},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

pub(super) const RECORD: &str = "cleanup.json";
pub(super) const PROGRESS: &str = "cleanup-progress.json";
pub(super) const RECORD_LIMIT: usize = 16 * 1024 * 1024;
const PROGRESS_LIMIT: usize = 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    path: String,
    directory: bool,
    identity: (u32, u64, u64), // volume, index, creation time
    size: usize,
    content: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    intent: String,
    target: String,
    expected: String,
    nodes: Vec<Node>, // immutable postorder; root last
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    version: u32,
    intent: String,
    target: String,
    expected: String,
    plan: String,
    total: usize,
    done: usize,
    pending: Option<usize>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn ownership(node: &CleanupNode) -> R<Node> {
    let directory = node.directory()?;
    let bytes = if directory { Vec::new() } else { node.bytes()? };
    Ok(Node {
        path: node.path.clone(),
        directory,
        identity: node.identity()?,
        size: bytes.len(),
        content: (!directory).then(|| digest(&bytes)),
    })
}
impl Record {
    fn validate(&self, intent: &str, target: &str, expected: &str) -> R<()> {
        if self.version != 1
            || self.intent != intent
            || !hash_valid(&self.intent)
            || !hash_valid(&self.expected)
            || self.target != target
            || self.expected != expected
            || self.nodes.is_empty()
            || self.nodes.len() > TOTAL_FILES
        {
            return Err("INVALID_CLEANUP_RECORD");
        }
        component(&self.target)?;
        let mut seen = BTreeMap::new();
        let mut total = 0usize;
        for (i, node) in self.nodes.iter().enumerate() {
            if node.path.is_empty() {
                if i != self.nodes.len() - 1 {
                    return Err("INVALID_CLEANUP_RECORD");
                }
            } else {
                relative(&node.path)?;
                if node.path.split('/').count() > MAX_DEPTH {
                    return Err("DEPTH_16");
                }
            }
            if seen.insert(node.path.as_str(), i).is_some()
                || (node.directory && (node.size != 0 || node.content.is_some()))
                || (!node.directory && !node.content.as_deref().is_some_and(hash_valid))
                || node.size > super::super::FILE_BYTES
            {
                return Err("INVALID_CLEANUP_RECORD");
            }
            total = total
                .checked_add(node.size)
                .ok_or("INVALID_CLEANUP_RECORD")?;
            if total > TOTAL_BYTES {
                return Err("TOTAL_BYTES_268435456");
            }
        }
        if self.nodes.last().is_none_or(|n| !n.path.is_empty()) {
            return Err("INVALID_CLEANUP_RECORD");
        }
        for (i, node) in self.nodes.iter().enumerate() {
            if node.path.is_empty() {
                continue;
            }
            let parent = node.path.rsplit_once('/').map_or("", |v| v.0);
            let p = *seen.get(parent).ok_or("INVALID_CLEANUP_RECORD")?;
            if p <= i || !self.nodes[p].directory {
                return Err("INVALID_CLEANUP_RECORD");
            }
        }
        Ok(())
    }
}
impl Progress {
    fn validate(&self, intent: &str, target: &str, expected: &str) -> R<()> {
        if self.version != 1
            || self.intent != intent
            || self.target != target
            || self.expected != expected
            || !hash_valid(&self.plan)
            || !hash_valid(&self.intent)
            || !hash_valid(&self.expected)
            || self.total == 0
            || self.total > TOTAL_FILES
            || self.done > self.total
            || self
                .pending
                .is_some_and(|n| n != self.done || n >= self.total)
        {
            return Err("INVALID_CLEANUP_PROGRESS");
        }
        Ok(())
    }
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("cleanup metadata limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode<T: Serialize>(value: &T, limit: usize) -> R<Vec<u8>> {
    let mut bytes = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut bytes, value).map_err(|_| "CLEANUP_METADATA_SIZE_LIMIT")?;
    Ok(bytes.bytes)
}
fn save_progress(state: &Dir, progress: &Progress) -> R<()> {
    state.atomic(PROGRESS, &encode(progress, PROGRESS_LIMIT)?)
}
fn load_record(state: &Dir) -> R<Option<(Record, String)>> {
    if state.kind(RECORD)?.is_none() {
        return Ok(None);
    }
    let bytes = state.read_private(RECORD, RECORD_LIMIT)?;
    let record = serde_json::from_slice(&bytes).map_err(|_| "INVALID_CLEANUP_RECORD")?;
    Ok(Some((record, digest(&bytes))))
}
fn load_progress(state: &Dir) -> R<Option<Progress>> {
    if state.kind(PROGRESS)?.is_none() {
        return Ok(None);
    }
    serde_json::from_slice(&state.read_private(PROGRESS, PROGRESS_LIMIT)?)
        .map(Some)
        .map_err(|_| "INVALID_CLEANUP_PROGRESS")
}
fn run(
    state: &Dir,
    record: &Record,
    mut progress: Progress,
    current: Vec<CleanupNode>,
    budget: &Budget,
) -> R<()> {
    let mut handles = BTreeMap::new();
    for node in current {
        budget.check()?;
        if handles.insert(node.path.clone(), node).is_some() {
            return Err("CLEANUP_OWNERSHIP_CHANGED");
        }
    }
    let expected_paths = record
        .nodes
        .iter()
        .map(|n| n.path.as_str())
        .collect::<BTreeSet<_>>();
    if handles.keys().any(|p| !expected_paths.contains(p.as_str())) {
        return Err("CLEANUP_OWNERSHIP_CHANGED");
    }
    // Entire remaining inventory is checked before any further deletion. Only
    // the flushed cursor's done nodes and its one pending node may be absent.
    for (i, expected) in record.nodes.iter().enumerate() {
        budget.check()?;
        match handles.get(&expected.path) {
            None if i >= progress.done && progress.pending != Some(i) => {
                return Err("CLEANUP_UNRECORDED_ABSENCE");
            }
            None => (),
            Some(_) if i < progress.done => return Err("CLEANUP_OWNERSHIP_CHANGED"),
            Some(node) => {
                let actual = ownership(node)?;
                if actual.directory != expected.directory
                    || actual.identity != expected.identity
                    || actual.size != expected.size
                    || actual.content != expected.content
                {
                    return Err("CLEANUP_OWNERSHIP_CHANGED");
                }
            }
        }
    }
    // A plan persisted without its initial progress file authorizes no absence.
    // Persist that initial cursor only after the intact preflight succeeds.
    save_progress(state, &progress)?;
    for index in progress.done..record.nodes.len() {
        budget.check()?;
        if let Some(node) = handles.remove(&record.nodes[index].path) {
            progress.pending = Some(index);
            save_progress(state, &progress)?; // <=1KiB, flushed before disposition
            // The flush can yield to other actors. Check this same held object's
            // type/content/identity again at the disposition boundary.
            let actual = ownership(&node)?;
            let expected = &record.nodes[index];
            if actual.directory != expected.directory
                || actual.identity != expected.identity
                || actual.size != expected.size
                || actual.content != expected.content
            {
                return Err("CLEANUP_OWNERSHIP_CHANGED");
            }
            node.dispose()?;
            #[cfg(test)]
            super::windows_tests::cleanup_after_dispose()?;
        }
        progress.done = index + 1;
        progress.pending = None;
        save_progress(state, &progress)?;
    }
    state.sync()
}

pub(in crate::extensions) fn finish_one(
    state: &Dir,
    intent_bytes: &[u8],
    target: &str,
    expected: Option<&str>,
) -> R<()> {
    let intent = digest(intent_bytes);
    let budget = Budget::new();
    let (record, plan_hash, nodes) = if let Some((record, hash)) = load_record(state)? {
        record.validate(&intent, target, expected.ok_or("CORRUPT_INTENT")?)?;
        let nodes = state
            .cleanup_nodes(target, &budget, false)?
            .map_or_else(Vec::new, |(_, n)| n);
        (record, hash, nodes)
    } else {
        // An orphan cursor is legal only after the enclosing intent durably
        // records completion; that path goes through retire(), never here.
        if state.kind(PROGRESS)?.is_some() {
            return Err("INVALID_CLEANUP_PROGRESS");
        }
        let Some((fingerprint, nodes)) = state.cleanup_nodes(target, &budget, true)? else {
            return Ok(());
        };
        let expected = expected.ok_or("CORRUPT_INTENT")?;
        if fingerprint.as_deref() != Some(expected) {
            return Err("LEGACY_CLEANUP_OWNERSHIP_UNPROVEN");
        }
        let record = Record {
            version: 1,
            intent: intent.clone(),
            target: target.into(),
            expected: expected.into(),
            nodes: nodes.iter().map(ownership).collect::<R<Vec<_>>>()?,
        };
        record.validate(&intent, target, expected)?;
        let bytes = encode(&record, RECORD_LIMIT)?;
        let hash = digest(&bytes);
        state.atomic(RECORD, &bytes)?; // exactly once; no node has been deleted
        (record, hash, nodes)
    };
    let progress = load_progress(state)?.unwrap_or(Progress {
        version: 1,
        intent: intent.clone(),
        target: target.into(),
        expected: record.expected.clone(),
        plan: plan_hash.clone(),
        total: record.nodes.len(),
        done: 0,
        pending: None,
    });
    progress.validate(&intent, target, &record.expected)?;
    if progress.plan != plan_hash || progress.total != record.nodes.len() {
        return Err("INVALID_CLEANUP_PROGRESS");
    }
    run(state, &record, progress, nodes, &budget)
}

// Called only for a target durably marked complete in the publication intent.
// Data reappearance is a foreign object, including byte-identical replacements.
pub(in crate::extensions) fn retire(
    state: &Dir,
    intent_bytes: &[u8],
    target: &str,
    expected: Option<&str>,
) -> R<()> {
    if state.kind(target)?.is_some() {
        return Err("CLEANUP_COMPLETED_TARGET_REAPPEARED");
    }
    let intent = digest(intent_bytes);
    let record = load_record(state)?;
    let progress = load_progress(state)?;
    // A previously completed target can precede the currently active target.
    if record.as_ref().is_some_and(|(r, _)| r.target != target)
        || progress.as_ref().is_some_and(|p| p.target != target)
    {
        return Ok(());
    }
    if let Some((record, hash)) = &record {
        record.validate(&intent, target, expected.ok_or("CORRUPT_INTENT")?)?;
        if let Some(p) = &progress {
            if p.plan != *hash || p.total != record.nodes.len() {
                return Err("INVALID_CLEANUP_PROGRESS");
            }
        }
    }
    if let Some(p) = &progress {
        p.validate(&intent, target, expected.ok_or("CORRUPT_INTENT")?)?;
        if p.done != p.total || p.pending.is_some() {
            return Err("INVALID_CLEANUP_PROGRESS");
        }
    }
    // Completed-target intent phase makes either interrupted removal restartable.
    if progress.is_some() {
        state.remove(PROGRESS)?;
        #[cfg(test)]
        super::windows_tests::cleanup_after_progress_removal()?;
    }
    if record.is_some() {
        state.remove(RECORD)?;
    }
    Ok(())
}
pub(in crate::extensions) fn ensure_retired(state: &Dir) -> R<()> {
    if state.kind(RECORD)?.is_some() || state.kind(PROGRESS)?.is_some() {
        return Err("INVALID_CLEANUP_RECORD");
    }
    Ok(())
}
