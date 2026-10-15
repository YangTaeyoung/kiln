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

pub(super) fn groups(state: &State) -> BTreeMap<String, String> {
    let links = Links::new(state);
    let mut groups = BTreeMap::new();
    for key in state.entries.keys() {
        if groups.contains_key(key) {
            continue;
        }
        let (_, members) = links.group(key).unwrap();
        let anchor = members
            .iter()
            .min_by_key(|key| (state.entries[*key].imported, *key))
            .unwrap();
        for member in members {
            groups.insert(member.clone(), anchor.clone());
        }
    }
    groups
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

pub(super) fn members(state: &State, key: &str) -> BTreeSet<String> {
    Links::new(state).members(key)
}

pub(super) fn destination(state: &State, key: &str, tool: Tool, root: &Path) -> Option<Source> {
    destination_in(state, &members(state, key), tool, root)
}

/// `destination` for a group whose members are already known.
pub(super) fn destination_in(
    state: &State,
    members: &BTreeSet<String>,
    tool: Tool,
    root: &Path,
) -> Option<Source> {
    let anchor = members
        .iter()
        .min_by_key(|key| (state.entries[*key].imported, *key))?;
    let preferred = state.entries[anchor]
        .copies
        .iter()
        .find(|c| c.tool == tool && c.root == root)
        .map(|c| &c.id);
    members
        .iter()
        .filter_map(|key| state.entries.get(key))
        .filter(|entry| {
            entry.source.tool == tool && entry.source.root == root && storage::exists(&entry.source)
        })
        // Keep the original native conversation when returning from the other CLI.
        .min_by_key(|entry| {
            (
                entry.imported,
                Some(&entry.source.id) != preferred,
                &entry.source.path,
            )
        })
        .map(|entry| entry.source.clone())
}

#[derive(Serialize, Deserialize)]
struct Archive {
    source: Source,
    destination: PathBuf,
    sha256: String,
    canonical_id: String,
}

pub(super) fn retire(
    config: &Path,
    state_path: &Path,
    state: &mut State,
    scope: Option<&discovery::Scope>,
    gate: &Gate,
    status: &mut Status,
    cache: &mut Cache,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<()> {
    let keys: Vec<_> = state.entries.keys().cloned().collect();
    // Groups and destinations are computed once for the pass; archiving leaves publication
    // edges unchanged and never moves a destination.
    let links = Links::new(state);
    let mut destinations: HashMap<(usize, Tool, PathBuf), Option<Source>> = HashMap::new();
    for key in keys {
        if !gate.load(Ordering::Acquire) {
            break;
        }
        let entry = &state.entries[&key];
        if !entry.imported
            || state.archived.contains_key(&key)
            || scope.is_some_and(|s| !s.contains(&entry.source))
        {
            continue;
        }
        let source = entry.source.clone();
        let (slot, members) = links.group(&key).unwrap();
        let Some(target) = destinations
            .entry((slot, source.tool, source.root.clone()))
            .or_insert_with(|| destination_in(state, members, source.tool, &source.root))
            .clone()
        else {
            continue;
        };
        if source.path == target.path {
            continue;
        }
        let stamps: Vec<_> = [&source, &target]
            .into_iter()
            .map(|s| {
                storage::metadata(s)
                    .ok()
                    .and_then(|m| Some((m.len(), m.modified().ok()?)))
                    .unwrap_or((0, SystemTime::UNIX_EPOCH))
            })
            .collect();
        if let Some((seen, at, failures, settled)) = cache.retirements.get(&key)
            && *seen == stamps
            && {
                let waited = at.elapsed().unwrap_or_default();
                waited < retry_delay(*failures) || (*settled && waited < FAILED_RECHECK)
            }
        {
            if let Some(row) = cache.rows.get(&key) {
                status.rows.entry(key.clone()).or_insert_with(|| row.clone());
            }
            continue;
        }
        let manifest = config
            .join("conversation-sync/archives")
            .join(format!("{}.json", digest(key.as_bytes())));
        let result = (|| -> Result<Option<PathBuf>> {
            let _guard = handover::write_guard(config, &source)?;
            if !storage::exists(&source) {
                if !manifest.exists() {
                    return Ok(None);
                }
                let archived: Archive = serde_json::from_slice(&bounded(&manifest, 32 * 1024)?)?;
                ensure!(
                    archived.source.path == source.path && archived.source.id == source.id,
                    "Archive recovery identity changed"
                );
                let archive_root = if source.tool == Tool::Codex {
                    &source.root
                } else {
                    config
                };
                safe_path(archive_root, &archived.destination)?;
                ensure!(
                    digest(&bounded(&archived.destination, COPY_LIMIT)?) == archived.sha256,
                    "Archived history integrity check failed"
                );
                return Ok(Some(archived.destination));
            }
            // A baseline proves Kiln/native-import ownership; it is never permission
            // to discard a new branch. Compare the actual complete history as well.
            if discovery::imported(config, &source)?.is_none() {
                return Ok(None);
            }
            let messages = read_messages(&source)?;
            let current = read_messages(&target)?;
            if messages.len() > current.len() && messages.starts_with(&current) {
                anyhow::bail!("Canonical history is catching up; retrying before archive");
            }
            // A native importer can retain the displayed text but omit Kiln's
            // lossless annotation. Proven lineage plus exact visible-prefix
            // equality permits archival; the raw alias is retained in the archive.
            let visible_prefix = current.len() >= messages.len()
                && current.iter().zip(&messages).all(|(a, b)| {
                    a.role == b.role
                        && a.images == b.images
                        && (a.text == b.text
                            || a.role == "assistant" && portable::display(&a.text) == b.text)
                });
            ensure!(visible_prefix, DIVERGED);
            let physical = storage::physical(&source);
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
                canonical_id: target.id.clone(),
            };
            private_dir(manifest.parent().unwrap())?;
            replace(&manifest, &serde_json::to_vec(&plan)?)?;
            ensure!(
                storage::physical(&source) == physical && bounded(&physical, COPY_LIMIT)? == bytes,
                "Transcript changed before archive; retrying"
            );
            if source.tool == Tool::Codex {
                let actual = metadata(&source, Metadata::Archive)?
                    .context("Native archive returned no path")?;
                ensure!(
                    Path::new(&actual) == destination,
                    "Native archive returned a different destination"
                );
            } else {
                fs::rename(&source.path, &destination)?;
                fs::File::open(source.path.parent().unwrap())?.sync_all()?;
                fs::File::open(destination.parent().unwrap())?.sync_all()?;
            }
            ensure!(
                digest(&bounded(&destination, COPY_LIMIT)?) == plan.sha256,
                "Archived history integrity check failed"
            );
            Ok(Some(destination))
        })();
        match &result {
            Ok(_) => {
                cache.retirements.remove(&key);
            }
            Err(error) => {
                let failures = cache.retirements.get(&key).map_or(0, |(_, _, n, _)| *n) + 1;
                // A failure without "retrying" waits for a stamp change or FAILED_RECHECK.
                let settled = !error.to_string().contains("retrying");
                cache
                    .retirements
                    .insert(key.clone(), (stamps, SystemTime::now(), failures, settled));
            }
        }
        match result {
            Ok(Some(path)) => {
                state.archived.insert(key.clone(), path);
                status.rows.remove(&key);
                state
                    .recent
                    .retain(|c| c.id != source.id || c.root != source.root);
                replace(state_path, &serde_json::to_vec(state)?)?;
            }
            Ok(None) => {}
            Err(error) => {
                let row = status.rows.entry(key).or_insert_with(|| {
                    Conversation::from_entry(
                        &state.entries[&source.path.to_string_lossy().into_owned()],
                        scope,
                    )
                });
                row.phase = if error.to_string().contains("retrying") {
                    Phase::Waiting
                } else {
                    Phase::Failed
                };
                row.detail = error.to_string();
            }
        }
    }
    Ok(())
}

/// Recover pre-canonical edges by reproducing the old ID formula, not by
/// equating similar conversation text. Imported baselines are immutable evidence.
pub(super) fn migrate(
    config: &Path,
    state_path: &Path,
    legacy_path: &Path,
    state: &mut State,
    scope: Option<&discovery::Scope>,
) -> Result<()> {
    let keys: Vec<_> = state.entries.keys().cloned().collect();
    for key in &keys {
        let entry = &state.entries[key];
        if !entry.imported
            || state.lineage_checked.contains(key)
            || scope.is_some_and(|s| !s.contains(&entry.source))
        {
            continue;
        }
        let Some(fingerprint) = discovery::imported(config, &entry.source)? else {
            continue;
        };
        let mut names = vec![
            entry.title.clone(),
            titles::clean(&entry.title),
            String::new(),
        ];
        if storage::exists(&entry.source) {
            if let Some(raw) = titles::raw_projection(&entry.source)? {
                names.push(raw.clone());
                names.push(titles::clean(&raw));
            }
        }
        names.sort();
        names.dedup();
        let mut parent = None;
        'search: for source_key in &keys {
            let source = &state.entries[source_key].source;
            if source.tool == entry.source.tool || source.cwd != entry.source.cwd {
                continue;
            }
            for path in [state_path, legacy_path] {
                let prefix = path.display();
                let root = entry.source.root.display();
                let mut ids = vec![
                    format!("{prefix}:{source_key}:{fingerprint}"),
                    format!("{prefix}:{source_key}:{fingerprint}:{root}"),
                    format!("{prefix}:v3:{source_key}:{fingerprint}:{root}"),
                ];
                for name in &names {
                    let hash = digest(name.as_bytes());
                    ids.push(format!("{prefix}:v4:{source_key}:{fingerprint}:{hash}"));
                    ids.push(format!(
                        "{prefix}:v4:{source_key}:{fingerprint}:{hash}:{root}"
                    ));
                }
                if ids
                    .iter()
                    .any(|identity| stable_id(identity) == entry.source.id)
                {
                    parent = Some(source_key.clone());
                    break 'search;
                }
            }
        }
        if let Some(parent) = parent {
            state.lineage.insert(key.clone(), parent);
        }
        state.lineage_checked.insert(key.clone());
    }
    if let Some(scope) = scope {
        for (tool, root) in &scope.roots {
            if *tool != Tool::Codex {
                continue;
            }
            let path = root.join("external_agent_session_imports.json");
            if !path.exists() {
                continue;
            }
            safe_path(root, &path)?;
            let value: serde_json::Value =
                serde_json::from_slice(&bounded(&path, 8 * 1024 * 1024)?)?;
            for record in value["records"].as_array().into_iter().flatten() {
                let (Some(source), Some(id)) = (
                    record["source_path"].as_str(),
                    record["imported_thread_id"].as_str(),
                ) else {
                    continue;
                };
                if state
                    .entries
                    .get(source)
                    .is_none_or(|e| e.source.tool != Tool::Claude || !scope.contains(&e.source))
                {
                    continue;
                }
                if let Some((target, _)) = state.entries.iter().find(|(_, e)| {
                    e.source.tool == Tool::Codex && e.source.root == *root && e.source.id == id
                }) {
                    state.lineage.insert(target.clone(), source.into());
                }
            }
        }
    }
    Ok(())
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
