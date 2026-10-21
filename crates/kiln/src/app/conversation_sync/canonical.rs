//! One native endpoint per linked conversation and namespace. Never infer lineage
//! from a title or shared prompt: only recorded publication edges establish it.
use super::*;
use std::collections::BTreeSet;

/// Linked-conversation groups of a state, computed in one pass over its
/// publication edges (lineage and recorded copies).
pub(super) struct Links {
    group: HashMap<String, usize>,
    members: Vec<BTreeSet<String>>,
}
impl Links {
    pub(super) fn new(state: &State) -> Self {
        let keys: Vec<&String> = state.entries.keys().collect();
        let index: HashMap<&str, usize> =
            keys.iter().enumerate().map(|(i, key)| (key.as_str(), i)).collect();
        let ids: HashMap<_, _> = state
            .entries
            .iter()
            .map(|(key, entry)| ((entry.source.tool, &entry.source.root, &entry.source.id), key))
            .collect();
        let mut parent: Vec<usize> = (0..keys.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let join = |parent: &mut Vec<usize>, a: usize, b: usize| {
            let (a, b) = (root(parent, a), root(parent, b));
            if a != b {
                parent[b] = a;
            }
        };
        for (source, target) in &state.lineage {
            if let (Some(&a), Some(&b)) = (index.get(source.as_str()), index.get(target.as_str())) {
                join(&mut parent, a, b);
            }
        }
        for (source, entry) in &state.entries {
            for copy in entry.copies.iter().chain(entry.copied_to.iter()) {
                if let Some(target) = ids.get(&(copy.tool, &copy.root, &copy.id)) {
                    join(&mut parent, index[source.as_str()], index[target.as_str()]);
                }
            }
        }
        let mut group = HashMap::with_capacity(keys.len());
        let mut members: Vec<BTreeSet<String>> = Vec::new();
        let mut slots: HashMap<usize, usize> = HashMap::new();
        for (i, key) in keys.iter().enumerate() {
            let r = root(&mut parent, i);
            let slot = *slots.entry(r).or_insert_with(|| {
                members.push(BTreeSet::new());
                members.len() - 1
            });
            members[slot].insert((*key).clone());
            group.insert((*key).clone(), slot);
        }
        Self { group, members }
    }
    /// Keys linked to `key`, including `key` itself.
    #[cfg(test)]
    pub(super) fn members(&self, key: &str) -> BTreeSet<String> {
        self.group(key)
            .map(|(_, members)| members.clone())
            .unwrap_or_else(|| BTreeSet::from([key.to_owned()]))
    }
    /// Index and members of the group containing a key of the state.
    pub(super) fn group(&self, key: &str) -> Option<(usize, &BTreeSet<String>)> {
        self.group.get(key).map(|slot| (*slot, &self.members[*slot]))
    }
}



/// UI-only grouping: retain per-endpoint diagnostics and cache state internally.
pub(super) fn group_rows(
    rows: BTreeMap<String, Conversation>,
    groups: &BTreeMap<String, String>,
) -> BTreeMap<String, Conversation> {
    let priority = |phase| match phase {
        Phase::Failed => 6,
        Phase::Waiting => 5,
        Phase::Copying => 4,
        Phase::Reading => 3,
        Phase::Queued => 2,
        Phase::Synced => 1,
    };
    let mut grouped: BTreeMap<String, Conversation> = BTreeMap::new();
    for (key, row) in rows {
        let group = groups.get(&key).cloned().unwrap_or(key);
        match grouped.entry(group) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if priority(row.phase) > priority(entry.get().phase) {
                    entry.insert(row);
                }
            }
        }
    }
    grouped
}






#[derive(Serialize, Deserialize)]
struct Archive {
    source: Source,
    destination: PathBuf,
    sha256: String,
    canonical_id: String,
}

fn manifest_path(config: &Path, source: &Source) -> PathBuf {
    let key = source.path.to_string_lossy().into_owned();
    config
        .join("conversation-sync/archives")
        .join(format!("{}.json", digest(key.as_bytes())))
}

/// Whether every conversation record of a native transcript was written by Kiln: no Claude
/// assistant record from a model and no Codex turn context.
pub(super) fn kiln_written(source: &Source) -> Result<bool> {
    use std::io::BufRead;
    let (input, _) = storage::open_at(&source.root, &storage::physical(source), 0)?;
    let contains = |line: &[u8], needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    for line in std::io::BufReader::new(input).split(b'\n') {
        let line = line?;
        let native = match source.tool {
            Tool::Claude => contains(&line, b"\"type\":\"assistant\"") && !contains(&line, b"\"model\":\"<synthetic>\""),
            Tool::Codex => contains(&line, b"\"type\":\"turn_context\""),
        };
        if native {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether Kiln moved this native transcript into an archive.
pub(super) fn archived(config: &Path, source: &Source) -> bool {
    bounded(&manifest_path(config, source), 64 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Archive>(&bytes).ok())
        .is_some_and(|plan| plan.destination.exists())
}

/// Moves a whole native transcript out of its CLI's resume list with an integrity manifest
/// and returns the archived path. Codex uses its native archive; Claude moves into Kiln's archive.
pub(super) fn archive_native(
    config: &Path,
    source: &Source,
    canonical_id: &str,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<PathBuf> {
    let key = source.path.to_string_lossy().into_owned();
    let manifest = manifest_path(config, source);
    let _guard = handover::write_guard(config, source)?;
    let physical = storage::physical(source);
    safe_path(&source.root, &physical)?;
    let bytes = bounded(&physical, COPY_LIMIT)?;
    let destination = if source.tool == Tool::Codex {
        source
            .root
            .join("archived_sessions")
            .join(physical.file_name().context("Missing source filename")?)
    } else {
        config
            .join("conversation-sync/archives")
            .join(format!("{}.jsonl", digest(key.as_bytes())))
    };
    ensure!(
        !destination.exists(),
        "Archive destination collision; original preserved"
    );
    let plan = Archive {
        source: source.clone(),
        destination: destination.clone(),
        sha256: digest(&bytes),
        canonical_id: canonical_id.into(),
    };
    private_dir(manifest.parent().unwrap())?;
    replace(&manifest, &serde_json::to_vec(&plan)?)?;
    ensure!(
        storage::physical(source) == physical && bounded(&physical, COPY_LIMIT)? == bytes,
        "Transcript changed before archive; retrying"
    );
    if source.tool == Tool::Codex {
        let actual = metadata(source, Metadata::Archive)?
            .context("Native archive returned no path")?;
        ensure!(
            Path::new(&actual) == destination,
            "Native archive returned a different destination"
        );
    } else {
        fs::rename(&physical, &destination)?;
        sync_dir(physical.parent().unwrap())?;
        sync_dir(destination.parent().unwrap())?;
    }
    ensure!(
        digest(&bounded(&destination, COPY_LIMIT)?) == plan.sha256,
        "Archived history integrity check failed"
    );
    Ok(destination)
}



/// Appends `suffix` in place to an uncompressed transcript that still has `length` bytes and,
/// when given, the modification time `modified`.
fn append_in_place(source: &Source, physical: &Path, length: u64, modified: Option<SystemTime>, suffix: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    // Opens a link itself; its metadata then fails the regular-file check below.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(physical)?;
    let now = file.metadata()?;
    ensure!(
        storage::physical(source) == physical
            && now.is_file()
            && now.len() == length
            && modified.is_none_or(|m| now.modified().is_ok_and(|n| n == m)),
        "Transcript changed before publication; retrying"
    );
    file.write_all(suffix)?;
    file.sync_all()?;
    Ok(())
}

/// Appends `tail` and `title` after a checkpoint without parsing the earlier history: the
/// existing bytes are kept and the appended records must read back to exactly `tail`.
/// Returns the next checkpoint, or `None` when only a full update can append safely.
pub(super) fn append_since(
    source: &Source,
    checkpoint: &super::checkpoint::Checkpoint,
    tail: &[format::Message],
    title: &str,
    timestamp: &str,
) -> Result<Option<super::checkpoint::Checkpoint>> {
    use super::checkpoint::{Checkpoint, window_of};
    let physical = storage::physical(source);
    if storage::compressed(&physical) {
        return Ok(None);
    }
    // A boundary for Claude Code's context is written by a full update, which reads the history.
    let appended = tail.iter().map(super::context::size).sum::<usize>() as u64;
    if source.tool == Tool::Claude
        && checkpoint.context.is_none_or(|c| c + appended > super::context::LIMIT as u64)
    {
        return Ok(None);
    }
    let before = storage::metadata(source)?;
    let old = storage::bytes(source, COPY_LIMIT)?;
    ensure!(old.ends_with(b"\n"), "Transcript is being written; retrying");
    let offset = checkpoint.offset as usize;
    if old.len() < offset || window_of(&old, checkpoint.offset) != checkpoint.window {
        return Ok(None);
    }
    let parse = |bytes: &[u8]| -> Option<Vec<serde_json::Value>> {
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).ok())
            .collect()
    };
    let Some(trailing) = parse(&old[offset..]) else { return Ok(None) };
    // Records after the checkpoint may only be metadata, never part of an unfinished turn.
    let unfinished = trailing.iter().any(|r| match source.tool {
        Tool::Claude => matches!(r["type"].as_str(), Some("user" | "assistant")),
        Tool::Codex => r["type"] == "response_item" || r["payload"]["type"] == "user_message" || r["payload"]["type"] == "task_started",
    });
    if unfinished {
        return Ok(None);
    }
    let last_ordinal = if source.tool == Tool::Codex {
        let start = offset.saturating_sub(64 * 1024);
        let window = &old[start..offset];
        let window = if start == 0 { window } else { &window[window.iter().position(|b| *b == b'\n').map_or(window.len(), |i| i + 1)..] };
        let found = parse(window)
            .into_iter()
            .flatten()
            .chain(trailing.iter().cloned())
            .filter_map(|r| r["ordinal"].as_u64())
            .max();
        let Some(found) = found else { return Ok(None) };
        Some(found)
    } else {
        None
    };
    let existing = match &checkpoint.leaf {
        Some(leaf) => vec![serde_json::json!({"type":"assistant","uuid":leaf,"isSidechain":false})],
        None => Vec::new(),
    };
    let seed = format!("{}:{}", old.len(), checkpoint.window);
    let suffix = format::render_suffix(
        source.tool,
        &source.id,
        &source.cwd,
        timestamp,
        tail,
        title,
        &existing,
        &seed,
        last_ordinal,
        None,
    )?;
    let Some(rows) = parse(&suffix) else { anyhow::bail!("Appended conversation could not be read back") };
    let Some((messages, end)) =
        format::parse_appended(source.tool, checkpoint.leaf.as_deref(), checkpoint.meta.as_ref(), &rows)?
    else {
        anyhow::bail!("Appended conversation could not be read back");
    };
    ensure!(
        messages == tail,
        "Appended conversation did not retain every message and attachment"
    );
    let mut ends = Vec::with_capacity(rows.len());
    let mut position = old.len() as u64;
    for line in suffix.split_inclusive(|b| *b == b'\n') {
        position += line.len() as u64;
        if line != b"\n" {
            ends.push(position);
        }
    }
    let mut candidate = old;
    candidate.extend_from_slice(&suffix);
    ensure!(
        candidate.len() as u64 <= COPY_LIMIT,
        "Converted conversation exceeds the 1 GiB sync limit"
    );
    let next = match end {
        Some((index, leaf)) => Checkpoint {
            offset: ends[index],
            window: window_of(&candidate, ends[index]),
            leaf: leaf.or_else(|| checkpoint.leaf.clone()),
            meta: checkpoint.meta.clone(),
            context: checkpoint.context.map(|c| c + super::context::records(&rows[..=index]) as u64),
        },
        None => checkpoint.clone(),
    };
    append_in_place(source, &physical, before.len(), Some(before.modified()?), &suffix)?;
    Ok(Some(next))
}

/// Prepare a suffix with native IDs/parent links, preserving the complete original
/// log (including provider-only records) byte for byte. An uncompressed transcript receives
/// the suffix in place; a compressed one is replaced atomically.
pub(super) fn update(
    source: &Source,
    messages: &[format::Message],
    title: &str,
    timestamp: &str,
) -> Result<bool> {
    let physical = storage::physical(source);
    let old = storage::bytes(source, COPY_LIMIT)?;
    ensure!(
        old.ends_with(b"\n"),
        "Transcript is being written; retrying"
    );
    let mut old_title = String::new();
    let (rows, current) = read_rows_titled(source, &mut old_title)?;
    let common = super::hub::shared(messages, &current);
    if current.len() > messages.len() && common == messages.len() {
        anyhow::bail!("Linked conversation has newer messages; retrying from its current endpoint");
    }
    ensure!(common == current.len(), DIVERGED);
    if current.len() == messages.len()
        && old_title == title
        && titles::raw_projection(source)?.is_none_or(|raw| raw == title)
    {
        return Ok(false);
    }
    #[derive(Deserialize)]
    struct Ordinal { ordinal: Option<u64> }
    let last_ordinal = old.split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Ordinal>(line).ok())
        .filter_map(|row| row.ordinal).max();
    let context = (source.tool == Tool::Claude)
        .then(|| (super::context::after_boundary(&rows), &messages[..current.len()]));
    let suffix = format::render_suffix(
        source.tool,
        &source.id,
        &source.cwd,
        timestamp,
        &messages[current.len()..],
        title,
        &rows,
        &digest(&old),
        last_ordinal,
        context,
    )?;
    let old_count = current.len();
    // The parsed endpoint is released before the prospective bytes are parsed again.
    drop((rows, current));
    let old_len = old.len();
    let mut candidate = old;
    candidate.extend_from_slice(&suffix);
    ensure!(
        candidate.len() as u64 <= COPY_LIMIT,
        "Converted conversation exceeds the 1 GiB sync limit"
    );
    // A sibling fixture path gives the normal version/identity/image parser the
    // exact prospective bytes, without altering the native resume entry.
    let mut tmp = tempfile::NamedTempFile::new_in(
        source.path.parent().context("Missing transcript parent")?,
    )?;
    tmp.write_all(&candidate)?;
    let check = Source {
        path: tmp.path().into(),
        ..source.clone()
    };
    ensure!(
        read_messages(&check).is_ok_and(|read| {
            read.len() == messages.len()
                && super::hub::shared(messages, &read) == read.len()
                && read[old_count..] == messages[old_count..]
        }),
        "Appended conversation did not retain every message and attachment"
    );
    ensure!(
        storage::physical(source) == physical && storage::equals(source, &candidate[..old_len])?,
        "Transcript changed before publication; retrying"
    );
    if storage::compressed(&physical) {
        let mut packed = tempfile::NamedTempFile::new_in(physical.parent().unwrap())?;
        zstd::stream::copy_encode(candidate.as_slice(), &mut packed, 3)?;
        ensure!(
            packed.as_file().metadata()?.len() <= COPY_LIMIT,
            "Compressed transcript exceeds its writing limit"
        );
        ensure!(
            storage::physical(source) == physical && storage::equals(source, &candidate[..old_len])?,
            "Transcript changed before publication; retrying"
        );
        packed.as_file().sync_all()?;
        packed.persist(&physical).map_err(|e| e.error)?;
        sync_dir(source.path.parent().unwrap())?;
    } else {
        drop(tmp);
        append_in_place(source, &physical, old_len as u64, None, &candidate[old_len..])?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_place_append_refuses_a_symlinked_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jsonl");
        fs::write(&target, b"{}\n").unwrap();
        let link = dir.path().join("link.jsonl");
        super::super::tests::symlink_file(&target, &link).unwrap();
        let source = Source {
            tool: Tool::Claude,
            root: dir.path().into(),
            path: link.clone(),
            cwd: dir.path().to_string_lossy().into_owned(),
            id: "linked".into(),
        };
        assert!(append_in_place(&source, &link, 3, None, b"{}\n").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"{}\n");
    }
}
