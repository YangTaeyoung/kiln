//! One-time migration of pairwise copies recorded by earlier versions into hub conversations.
//! Migration only records conversations; the regular scan archives the copies it leaves.
use super::hub::{Hub, NativeRef, Projection, Record, conversation_id, shared};
use super::*;
use std::collections::{BTreeSet, HashSet};

struct Member {
    entry: Entry,
    messages: Option<Vec<format::Message>>,
    title: String,
    stamp: (u64, SystemTime),
}

/// A legacy state file with a group that could not be read is read again after this delay.
const RETRY: Duration = Duration::from_secs(60);

/// Migrates each legacy state file once; a cancelled run resumes on the next scan.
pub(super) fn run(hub: &mut Hub, legacy: &[PathBuf], gate: &Gate) -> Result<()> {
    for path in legacy {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if hub.state.migrated.contains(&name) || !path.exists() {
            continue;
        }
        if hub
            .state
            .migration_retry
            .get(&name)
            .is_some_and(|at| at.elapsed().unwrap_or_default() < RETRY)
        {
            continue;
        }
        let state: State = serde_json::from_slice(&bounded(path, 256 * 1024 * 1024)?)?;
        let complete = migrate_state(hub, &state, gate);
        if !gate.load(Ordering::Acquire) {
            hub.save()?;
            break;
        }
        if complete {
            hub.state.migration_retry.remove(&name);
            hub.state.migrated.insert(name);
        } else {
            hub.state.migration_retry.insert(name, SystemTime::now());
        }
        hub.save()?;
    }
    Ok(())
}

/// Whether every group of the state was migrated.
fn migrate_state(hub: &mut Hub, state: &State, gate: &Gate) -> bool {
    let links = canonical::Links::new(state);
    let mut owners: HashMap<NativeRef, String> = HashMap::new();
    for (cid, record) in &hub.state.conversations {
        let natives = record
            .projections
            .iter()
            .map(|p| NativeRef::of(&p.source))
            .chain(record.aliases.iter().cloned())
            .chain(record.pending.iter().map(NativeRef::of));
        for native in natives {
            owners.insert(native, cid.clone());
        }
    }
    let mut groups = HashSet::new();
    let mut complete = true;
    for key in state.entries.keys() {
        if !gate.load(Ordering::Acquire) {
            return false;
        }
        let Some((slot, members)) = links.group(key) else { continue };
        if groups.insert(slot) && migrate_group(hub, &mut owners, state, members).is_err() {
            complete = false;
        }
    }
    complete
}

fn migrate_group(
    hub: &mut Hub,
    owners: &mut HashMap<NativeRef, String>,
    state: &State,
    members: &BTreeSet<String>,
) -> Result<()> {
    let live: Vec<&Entry> = members
        .iter()
        .filter(|key| !state.archived.contains_key(*key))
        .map(|key| &state.entries[key])
        .filter(|entry| storage::exists(&entry.source))
        .collect();
    // A group already migrated from another legacy state receives its other copies.
    if let Some(cid) = live.iter().find_map(|e| owners.get(&NativeRef::of(&e.source))).cloned() {
        let record = hub.state.conversations.get_mut(&cid).context("Unknown conversation")?;
        for entry in live {
            let native = NativeRef::of(&entry.source);
            if owners.contains_key(&native) {
                continue;
            }
            let source = &entry.source;
            let free = !record
                .projections
                .iter()
                .any(|p| !p.removed && p.source.tool == source.tool && p.source.root == source.root);
            if free {
                record.projections.push(Projection {
                    source: source.clone(),
                    synced: 0,
                    stamp: None,
                    origin: !entry.imported,
                    removed: false,
                    title: String::new(),
                    checkpoint: None,
                    database: None,
                });
            } else {
                record.pending.push(source.clone());
            }
            owners.insert(native, cid.clone());
        }
        return Ok(());
    }
    let mut read = Vec::new();
    for entry in live {
        let mut title = entry.title.clone();
        let messages = read_messages_titled(&entry.source, &mut title)
            .ok()
            .filter(|m| m.iter().any(|m| m.role == "user"));
        let meta = storage::metadata(&entry.source)?;
        read.push(Member { entry: entry.clone(), messages, title, stamp: (meta.len(), meta.modified()?) });
    }
    let Some(winner) = read
        .iter()
        .filter(|m| m.messages.is_some())
        .max_by_key(|m| (m.stamp.1, !m.entry.imported))
    else {
        return Ok(());
    };
    let events = winner.messages.clone().unwrap();
    let origin = read.iter().find(|m| !m.entry.imported).unwrap_or(winner);
    let cid = conversation_id(&NativeRef::of(&origin.entry.source));
    if hub.state.conversations.contains_key(&cid) {
        return Ok(());
    }
    let title = if winner.title.is_empty() { origin.title.clone() } else { winner.title.clone() };
    let mut record = Record::new(&title, &winner.entry.source.cwd);
    for key in members {
        if state.archived.contains_key(key) {
            record.aliases.push(NativeRef::of(&state.entries[key].source));
        }
    }
    let mut namespaces: BTreeMap<(String, PathBuf), Vec<&Member>> = BTreeMap::new();
    for member in &read {
        let source = &member.entry.source;
        namespaces.entry((source.tool.key().to_owned(), source.root.clone())).or_default().push(member);
    }
    for candidates in namespaces.values() {
        let prefix = |m: &&&Member| m.messages.as_ref().is_some_and(|msgs| shared(&events, msgs) == msgs.len());
        let chosen = candidates
            .iter()
            .filter(prefix)
            .max_by_key(|m| (m.messages.as_ref().map_or(0, Vec::len), !m.entry.imported, m.stamp.1))
            .or_else(|| candidates.iter().find(|m| m.messages.is_none()));
        let extras = candidates
            .iter()
            .filter(|m| m.messages.is_some() && chosen.is_none_or(|c| c.entry.source.path != m.entry.source.path));
        record.pending.extend(extras.map(|m| m.entry.source.clone()));
        let Some(chosen) = chosen else { continue };
        let parsed = chosen.messages.as_ref();
        record.projections.push(Projection {
            source: chosen.entry.source.clone(),
            synced: parsed.map_or(0, Vec::len),
            stamp: parsed.map(|_| chosen.stamp),
            origin: !chosen.entry.imported,
            removed: false,
            title: if parsed.is_some() { chosen.title.clone() } else { String::new() },
            checkpoint: None,
            database: None,
        });
    }
    for native in record
        .projections
        .iter()
        .map(|p| NativeRef::of(&p.source))
        .chain(record.aliases.iter().cloned())
        .chain(record.pending.iter().map(NativeRef::of))
    {
        owners.insert(native, cid.clone());
    }
    hub.state.conversations.insert(cid.clone(), record);
    hub.append(&cid, &events)?;
    hub.state.conversations.get_mut(&cid).unwrap().updated = winner.stamp.1;
    Ok(())
}
