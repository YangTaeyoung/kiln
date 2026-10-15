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

/// Moves a whole native transcript out of its CLI's resume list with an integrity manifest
/// and returns the archived path. Codex uses its native archive; Claude moves into Kiln's archive.
pub(super) fn archive_native(
    config: &Path,
    source: &Source,
    canonical_id: &str,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<PathBuf> {
    let key = source.path.to_string_lossy().into_owned();
    let manifest = config
        .join("conversation-sync/archives")
        .join(format!("{}.json", digest(key.as_bytes())));
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
        fs::File::open(physical.parent().unwrap())?.sync_all()?;
        fs::File::open(destination.parent().unwrap())?.sync_all()?;
    }
    ensure!(
        digest(&bounded(&destination, COPY_LIMIT)?) == plan.sha256,
        "Archived history integrity check failed"
    );
    Ok(destination)
}



/// Prepare a suffix with native IDs/parent links, preserving the complete original
/// log (including provider-only records) byte for byte. Publication is atomic.
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
    if current.len() > messages.len() && current.starts_with(messages) {
        anyhow::bail!("Linked conversation has newer messages; retrying from its current endpoint");
    }
    ensure!(messages.starts_with(&current), DIVERGED);
    if current == messages
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
    )?;
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
        read_messages(&check)? == messages,
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
    } else {
        tmp.as_file().sync_all()?;
        tmp.persist(&physical).map_err(|e| e.error)?;
    }
    fs::File::open(source.path.parent().unwrap())?.sync_all()?;
    Ok(true)
}
