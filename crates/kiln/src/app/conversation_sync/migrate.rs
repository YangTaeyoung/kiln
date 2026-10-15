//! One-time migration of pairwise copies recorded by earlier versions into hub conversations.
use super::hub::{Hub, NativeRef, Projection, Record, conversation_id, shared};
use super::*;
use std::collections::HashSet;

struct Member {
    entry: Entry,
    messages: Option<Vec<format::Message>>,
    title: String,
    stamp: (u64, SystemTime),
}

/// Migrates each legacy state file once; an interrupted run resumes on the next scan.
pub(super) fn run(
    config: &Path,
    legacy: &[PathBuf],
    gate: &Gate,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<()> {
    let mut hub = Hub::open(config)?;
    for path in legacy {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if hub.state.migrated.contains(&name) || !path.exists() {
            continue;
        }
        let state: State = serde_json::from_slice(&bounded(path, 256 * 1024 * 1024)?)?;
        let complete = migrate_state(config, &mut hub, &state, gate, metadata)?;
        if complete {
            hub.state.migrated.insert(name);
        }
        hub.save()?;
        if !gate.load(Ordering::Acquire) {
            break;
        }
    }
    Ok(())
}

fn migrate_state(
    config: &Path,
    hub: &mut Hub,
    state: &State,
    gate: &Gate,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<bool> {
    let links = canonical::Links::new(state);
    let mut groups = HashSet::new();
    for key in state.entries.keys() {
        if !gate.load(Ordering::Acquire) {
            return Ok(false);
        }
        let Some((slot, members)) = links.group(key) else { continue };
        if groups.insert(slot) {
            // A group that cannot be migrated stays untouched for the regular scan.
            let _ = migrate_group(config, hub, state, members, metadata);
        }
    }
    Ok(true)
}

fn migrate_group(
    config: &Path,
    hub: &mut Hub,
    state: &State,
    members: &std::collections::BTreeSet<String>,
    metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
) -> Result<()> {
    let known: HashSet<NativeRef> = hub
        .state
        .conversations
        .values()
        .flat_map(|r| {
            r.projections
                .iter()
                .map(|p| NativeRef::of(&p.source))
                .chain(r.aliases.iter().cloned())
        })
        .collect();
    let mut live = Vec::new();
    for key in members {
        let entry = &state.entries[key];
        if state.archived.contains_key(key) || !storage::exists(&entry.source) {
            continue;
        }
        if known.contains(&NativeRef::of(&entry.source)) {
            return Ok(());
        }
        let mut title = entry.title.clone();
        let messages = read_messages_titled(&entry.source, &mut title)
            .ok()
            .filter(|m| m.iter().any(|m| m.role == "user"));
        let meta = storage::metadata(&entry.source)?;
        live.push(Member { entry: entry.clone(), messages, title, stamp: (meta.len(), meta.modified()?) });
    }
    let Some(winner) = live
        .iter()
        .filter(|m| m.messages.is_some())
        .max_by_key(|m| (m.stamp.1, !m.entry.imported))
    else {
        return Ok(());
    };
    let events = winner.messages.clone().unwrap();
    let origin = live.iter().find(|m| !m.entry.imported).unwrap_or(winner);
    let cid = conversation_id(&NativeRef::of(&origin.entry.source));
    if hub.state.conversations.contains_key(&cid) {
        return Ok(());
    }
    let title = if winner.title.is_empty() { origin.title.clone() } else { winner.title.clone() };
    let mut record = Record::new(&title, &winner.entry.source.cwd);
    record.updated = winner.stamp.1;
    for key in members {
        if state.archived.contains_key(key) {
            record.aliases.push(NativeRef::of(&state.entries[key].source));
        }
    }
    let mut extras = Vec::new();
    let mut namespaces: BTreeMap<(String, PathBuf), Vec<&Member>> = BTreeMap::new();
    for member in &live {
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
        let Some(chosen) = chosen else {
            extras.extend(candidates.iter().copied());
            continue;
        };
        let parsed = chosen.messages.as_ref();
        record.projections.push(Projection {
            source: chosen.entry.source.clone(),
            synced: parsed.map_or(0, Vec::len),
            stamp: parsed.map(|_| chosen.stamp),
            origin: !chosen.entry.imported,
            removed: false,
            title: if parsed.is_some() { chosen.title.clone() } else { String::new() },
            checkpoint: None,
        });
        extras.extend(
            candidates
                .iter()
                .copied()
                .filter(|m| m.messages.is_some() && m.entry.source.path != chosen.entry.source.path),
        );
    }
    hub.state.conversations.insert(cid.clone(), record);
    hub.append(&cid, &events)?;
    hub.state.conversations.get_mut(&cid).unwrap().updated = winner.stamp.1;
    for member in extras {
        let source = &member.entry.source;
        let archived = canonical::archive_native(config, source, &cid, metadata)?;
        let native = NativeRef::of(source);
        if let Some(messages) = &member.messages {
            let base = shared(&events, messages);
            if base < messages.len() {
                hub.save_branch(&cid, base, &messages[base..], Some(native.clone()), Some(archived))?;
            }
        }
        hub.state.conversations.get_mut(&cid).unwrap().aliases.push(native);
    }
    Ok(())
}
