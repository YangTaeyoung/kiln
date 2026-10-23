//! Select bounded logical conversations, not their per-agent files. Self-written
//! projection stamps must never make an old conversation appear newly active.
use super::*;
use std::collections::HashSet;

type Selection = (Vec<Source>, Option<HashSet<String>>);
pub(super) fn choose(
    scope: Option<&discovery::Scope>,
    hub: &hub::Hub,
    sources: &[Source],
    metadata: &discovery::MetadataCache,
    previous: &BTreeMap<String, Conversation>,
) -> Result<Selection> {
    let Some(scope) = scope else {
        return Ok((sources.to_vec(), None));
    };
    let mut assignments: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut workspace_for = |cwd: &str| {
        assignments
            .entry(cwd.to_owned())
            .or_insert_with(|| scope.workspace(cwd).cloned())
            .clone()
    };
    let cached_stamp = |source: &Source| {
        metadata
            .get(&source.path)
            .or_else(|| metadata.get(&source.path.with_extension("jsonl.zst")))
            .map(|(len, modified, _)| (*len, *modified))
    };
    let mut index = HashMap::new();
    let mut candidates: HashMap<String, (PathBuf, SystemTime)> = HashMap::new();
    for (cid, record) in &hub.state.conversations {
        let Some(workspace) = workspace_for(&record.cwd) else {
            continue;
        };
        let mut activity = record.updated;
        for p in &record.projections {
            index.insert(hub::NativeRef::of(&p.source), cid.clone());
            if !p.removed
                && let Some(current) = if previous.contains_key(cid) {
                    engine::source_stamp(&p.source).ok()
                } else {
                    cached_stamp(&p.source)
                }
                && p.stamp != Some(current)
            {
                activity = activity.max(current.1);
            }
        }
        for alias in &record.aliases {
            index.insert(alias.clone(), cid.clone());
        }
        for pending in &record.pending {
            index.insert(hub::NativeRef::of(pending), cid.clone());
        }
        candidates.insert(cid.clone(), (workspace.clone(), activity));
    }
    let mut source_keys = HashMap::new();
    for source in sources {
        let Some(workspace) = workspace_for(&source.cwd) else {
            continue;
        };
        let native = hub::NativeRef::of(source);
        let key = index
            .get(&native)
            .cloned()
            .unwrap_or_else(|| hub::conversation_id(&native));
        // Known projections were ranked above against their post-write stamp.
        // Native alias continuations may rejoin the conversation after a later edit.
        let activity = if let Some((_, at)) = candidates.get(&key) {
            let record = &hub.state.conversations[&key];
            if record
                .projections
                .iter()
                .any(|p| p.source.path == source.path)
            {
                *at
            } else {
                cached_stamp(source).map_or(*at, |stamp| (*at).max(stamp.1))
            }
        } else {
            match cached_stamp(source) {
                Some(stamp) => stamp.1,
                None => continue,
            }
        };
        candidates
            .entry(key.clone())
            .and_modify(|(_, at)| *at = (*at).max(activity))
            .or_insert((workspace.clone(), activity));
        source_keys.insert(source.path.clone(), key);
    }
    let mut by_workspace: BTreeMap<PathBuf, Vec<(SystemTime, String)>> = BTreeMap::new();
    for (key, (workspace, at)) in candidates {
        by_workspace.entry(workspace).or_default().push((at, key));
    }
    let mut selected = HashSet::new();
    for (workspace, mut rows) in by_workspace {
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let limit = scope
            .limits
            .get(&workspace)
            .copied()
            .unwrap_or(DEFAULT_SYNC_LIMIT)
            .clamp(1, MAX_SOURCES);
        selected.extend(rows.into_iter().take(limit).map(|(_, key)| key));
    }
    let sources = sources
        .iter()
        .filter(|s| {
            source_keys
                .get(&s.path)
                .is_some_and(|key| selected.contains(key))
        })
        .cloned()
        .collect();
    Ok((sources, Some(selected)))
}
