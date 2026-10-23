//! Select bounded logical conversations, not their per-agent files. Self-written
//! projection stamps must never make an old conversation appear newly active.
use super::*;
use std::collections::HashSet;

type Selection = (Vec<Source>, Option<HashSet<String>>);
pub(super) fn choose(
    scope: Option<&discovery::Scope>,
    hub: &hub::Hub,
    sources: &[Source],
    cache: &mut Cache,
    ledger: &HashMap<hub::NativeRef, PathBuf>,
) -> Result<Selection> {
    let Some(scope) = scope else {
        return Ok((sources.to_vec(), None));
    };
    let mut assignments: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut workspace_for = |cwd: &str| {
        assignments
            .entry(cwd.to_owned())
            .or_insert_with(|| scope.workspace(cwd).filter(|w| !scope.paused.contains(w)).cloned())
            .clone()
    };
    fn cached_stamp(cache: &Cache, source: &Source) -> Option<(u64, SystemTime)> {
        cache.metadata
            .get(&source.path)
            .or_else(|| cache.metadata.get(&source.path.with_extension("jsonl.zst")))
            .map(|(len, modified, _)| (*len, *modified))
    }
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
                && let Some(current) = if cache.rows.contains_key(cid) {
                    engine::source_stamp(&p.source).ok()
                } else {
                    cached_stamp(cache, &p.source)
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
    let paths: HashMap<_, _> = hub
        .state
        .conversations
        .iter()
        .flat_map(|(cid, r)| {
            r.projections
                .iter()
                .map(|p| (&p.source.path, cid.clone()))
                .chain(r.pending.iter().map(|p| (&p.path, cid.clone())))
        })
        .collect();
    for (native, path) in ledger {
        if let Some(cid) = paths.get(path) {
            index.insert(native.clone(), cid.clone());
        }
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
            if let Some(record) = hub.state.conversations.get(&key) {
            if record
                .projections
                .iter()
                .any(|p| p.source.path == source.path)
            {
                *at
                } else {
                    match cached_stamp(cache, source) {
                        Some(stamp)
                            if stamp.1 > *at
                                && native_activity(hub, &key, source, stamp, cache)? =>
                        {
                            stamp.1
                        }
                        _ => *at,
                    }
                }
            } else {
                cached_stamp(cache, source).map_or(*at, |stamp| (*at).max(stamp.1))
            }
        } else {
            match cached_stamp(cache, source) {
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

/// Copies and pending projections do not acquire recency merely by being written.
/// Verify a changed alias once per source stamp and canonical generation.
fn native_activity(
    hub: &hub::Hub,
    cid: &str,
    source: &Source,
    stamp: (u64, SystemTime),
    cache: &mut Cache,
) -> Result<bool> {
    let record = &hub.state.conversations[cid];
    let signature = (
        stamp.0,
        stamp.1,
        cid.to_owned(),
        record.generation,
        record.events,
    );
    if let Some((saved, active)) = cache.activity.get(&source.path)
        && *saved == signature
    {
        return Ok(*active);
    }
    let active = if canonical::kiln_written(source)? {
        false
    } else {
        let messages = read_messages(source)?;
        hub::shared(&hub.events(cid)?, &messages) < messages.len()
    };
    cache
        .activity
        .insert(source.path.clone(), (signature, active));
    Ok(active)
}
