//! Durable ownership for native resumes launched inside Kiln.
use super::*;
use crate::app::agent_runtime::{self, Record};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Link {
    pub tool: Tool,
    pub root: PathBuf,
    pub id: String,
    pub group: String,
    pub canonical_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Owner {
    pub record: PathBuf,
    pub generation: String,
    pub id: String,
    pub canonical_id: String,
    pub ready: bool,
    pub sequence: u128,
}
pub(crate) type Owners = BTreeMap<String, Owner>;

pub(crate) fn pending_matches(owner: &Owner, path: &Path, record: &Record) -> bool {
    !owner.ready
        && owner.record == path
        && (owner.generation == record.generation
            || !record.confirmed && record.conversation.as_deref() == Some(&owner.canonical_id))
}

fn lock(config: &Path) -> Result<fs::File> {
    let dir = config.join("conversation-sync");
    private_dir(&dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("ownership.lock"))?;
    file.try_lock()
        .context("Conversation ownership is changing; retrying")?;
    Ok(file)
}
fn group_lock(config: &Path, group: &str) -> Result<fs::File> {
    let dir = config.join("conversation-sync/write-locks");
    private_dir(&dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(digest(group.as_bytes())))?;
    file.try_lock()
        .context("Linked conversation is being synchronized; retrying")?;
    Ok(file)
}
pub(crate) fn links(config: &Path) -> Result<Vec<Link>> {
    let path = config.join("conversation-sync/native-links.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    Ok(serde_json::from_slice(&bounded(&path, 8 * 1024 * 1024)?)?)
}
pub(crate) fn owners(config: &Path) -> Result<Owners> {
    let path = config.join("conversation-sync/owners.json");
    if !path.exists() {
        return Ok(Owners::new());
    }
    Ok(serde_json::from_slice(&bounded(&path, 1024 * 1024)?)?)
}
fn save(config: &Path, owners: &Owners) -> Result<()> {
    replace(
        &config.join("conversation-sync/owners.json"),
        &serde_json::to_vec(owners)?,
    )
}
pub(super) fn disable(config: &Path) -> Result<()> {
    let path = config.join("conversation-sync/native-links.json");
    if path.exists() && bounded(&path, 8 * 1024 * 1024)? != b"[]" {
        let _lock = lock(config)?;
        replace(&path, b"[]")?;
    }
    Ok(())
}
fn root(record: &Record) -> Option<PathBuf> {
    record
        .spec
        .env
        .iter()
        .find(|(key, _)| key == env_key(record.tool))
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os(env_key(record.tool)).map(PathBuf::from))
        .or_else(|| {
            directories::BaseDirs::new().map(|d| {
                d.home_dir().join(if record.tool == Tool::Claude {
                    ".claude"
                } else {
                    ".codex"
                })
            })
        })
        .and_then(|root| root.canonicalize().ok())
}
pub(crate) fn link<'a>(links: &'a [Link], record: &Record) -> Option<&'a Link> {
    let root = root(record)?;
    links.iter().find(|link| {
        link.tool == record.tool
            && link.root == root
            && Some(&link.id) == record.conversation.as_ref()
    })
}
pub(super) fn publish_links(
    config: &Path,
    state: &State,
    scope: Option<&discovery::Scope>,
) -> Result<()> {
    let mut links = vec![];
    for (key, entry) in &state.entries {
        if scope.is_some_and(|s| !s.contains(&entry.source)) {
            continue;
        }
        let members = canonical::members(state, key);
        if members.len() < 2 {
            continue;
        }
        let anchor = members
            .iter()
            .min_by_key(|key| (state.entries[*key].imported, *key))
            .unwrap();
        let Some(target) =
            canonical::destination(state, key, entry.source.tool, &entry.source.root)
        else {
            continue;
        };
        links.push(Link {
            tool: entry.source.tool,
            root: entry.source.root.clone(),
            id: entry.source.id.clone(),
            group: digest(anchor.as_bytes()),
            canonical_id: target.id,
        });
    }
    let _lock = lock(config)?;
    let path = config.join("conversation-sync/native-links.json");
    let bytes = serde_json::to_vec(&links)?;
    if !path.exists() || bounded(&path, 8 * 1024 * 1024)? != bytes {
        replace(&path, &bytes)?;
    }
    Ok(())
}

pub(super) fn settled(
    config: &Path,
    state: &State,
    cache: &Cache,
    status: &Status,
) -> Result<std::collections::BTreeSet<String>> {
    let links = links(config)?;
    let mut groups: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for link in links.iter().filter(|l| l.id == l.canonical_id) {
        if let Some(entry) = state.entries.values().find(|e| {
            e.source.tool == link.tool && e.source.root == link.root && e.source.id == link.id
        }) {
            groups.entry(link.group.clone()).or_default().push(entry);
        }
    }
    Ok(groups
        .into_iter()
        .filter_map(|(group, entries)| {
            let first = entries.first()?;
            (!status.rows.values().any(|row| {
                matches!(row.phase, Phase::Failed | Phase::Waiting)
                    && links.iter().any(|link| {
                        link.group == group
                            && link.tool == row.source.tool
                            && link.root == row.source.root
                            && link.id == row.source.id
                    })
            }) && entries.iter().any(|e| e.source.tool != first.source.tool)
                && !first.seen.is_empty()
                && entries.iter().all(|entry| {
                    entry.seen == first.seen
                        && entry.title == first.title
                        && cache.messages.get(&entry.source.path).is_some_and(
                            |(len, modified, fingerprint)| {
                                fingerprint == &entry.seen
                                    && storage::metadata(&entry.source).is_ok_and(|m| {
                                        m.len() == *len && m.modified().ok() == Some(*modified)
                                    })
                            },
                        )
                }))
            .then_some(group)
        })
        .collect())
}

/// SessionStart requests a transfer. UserPromptSubmit can never steal ownership.
/// The UI grants the new launch only after exit, durable sync, and exact-ID reload.
pub(crate) fn event(
    config: &Path,
    path: &Path,
    record: &Record,
    event: &str,
) -> Result<Option<serde_json::Value>> {
    let links = links(config)?;
    let Some(link) = link(&links, record) else {
        return Ok(None);
    };
    let _group_lock = group_lock(config, &link.group)?;
    let _lock = lock(config)?;
    let mut owners = owners(config)?;
    let granted = owners.get(&link.group).is_some_and(|owner| {
        owner.record == path
            && owner.generation == record.generation
            && Some(&owner.id) == record.conversation.as_ref()
            && owner.ready
    });
    if event == "SessionStart" && !granted {
        owners.insert(
            link.group.clone(),
            Owner {
                record: path.into(),
                generation: record.generation.clone(),
                id: link.id.clone(),
                canonical_id: link.canonical_id.clone(),
                ready: false,
                sequence: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_nanos(),
            },
        );
        save(config, &owners)?;
    }
    if event == "UserPromptSubmit" && !granted {
        return Ok(Some(serde_json::json!({"decision":"block","reason":
            "Kiln is transferring this conversation. Wait for the linked session to reopen; the other agent must stop and its history must finish syncing first."})));
    }
    Ok(None)
}

pub(crate) fn blocked(config: &Path, path: &Path, record: &Record) -> bool {
    let Ok(links) = links(config) else {
        return true;
    };
    let Some(link) = link(&links, record) else {
        return false;
    };
    let Ok(owners) = owners(config) else {
        return true;
    };
    owners.get(&link.group).is_some_and(|owner| {
        !owner.ready || owner.record != path || owner.generation != record.generation
    })
}

pub(crate) fn grant(
    config: &Path,
    group: &str,
    sequence: u128,
    path: &Path,
    record: &Record,
) -> Result<()> {
    let _lock = lock(config)?;
    let mut owners = owners(config)?;
    let owner = owners.get_mut(group).context("Transfer was removed")?;
    ensure!(
        owner.sequence == sequence && owner.record == path,
        "Transfer was superseded"
    );
    ensure!(
        record.conversation.as_deref() == Some(&owner.canonical_id),
        "Transfer selected a different conversation"
    );
    owner.id = owner.canonical_id.clone();
    owner.generation = record.generation.clone();
    owner.ready = true;
    save(config, &owners)
}

pub(crate) fn live_lease(path: &Path) -> Result<fs::File> {
    let path = path.with_extension("ui-live");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.try_lock()
        .context("This managed agent is already running")?;
    Ok(file)
}

pub(super) fn write_guard(config: &Path, source: &Source) -> Result<fs::File> {
    let links = links(config)?;
    let group = links
        .iter()
        .find(|l| l.tool == source.tool && l.root == source.root && l.id == source.id)
        .map(|l| l.group.clone())
        .unwrap_or_else(|| digest(source.path.to_string_lossy().as_bytes()));
    let guard = group_lock(config, &group)?;
    let dir = config.join("agent-sessions");
    if dir.exists() {
        for entry in fs::read_dir(&dir)?.take(10000) {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(record) = agent_runtime::read(&path) else {
                continue;
            };
            if record.tool != source.tool
                || record.conversation.as_deref() != Some(&source.id)
                || root(&record).as_ref() != Some(&source.root)
            {
                continue;
            }
            ensure!(
                live_lease(&path).is_ok(),
                "Linked agent is still open; retrying after conversation handover"
            );
        }
    }
    Ok(guard)
}
