//! Opt-in native continuations discovered within open workspace folders.
mod codex_index;
mod canonical;
mod discovery;
mod format;
pub(super) mod handover;
mod portable;
mod reader;
mod storage;
#[cfg(test)]
mod tests;
mod titles;

use anyhow::{Context, Result, ensure};
use kiln_accounts::{AccountManager, Tool};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

const LIMIT: u64 = 64 * 1024 * 1024;
const PORTABLE_LIMIT: u64 = 512 * 1024 * 1024;
const COPY_LIMIT: u64 = 1024 * 1024 * 1024;
const MAX_SOURCES: usize = 4096;
const PROJECTION_REVISION: u8 = 5;

enum Metadata<'a> {
    Read,
    Name(&'a str),
    Archive,
}
impl<'a> Metadata<'a> {
    fn title(&self) -> Option<&'a str> {
        match self { Self::Name(title) => Some(title), _ => None }
    }
}

fn has_current_native_projection(source: &Source, title: &str) -> Result<bool> {
    Ok(titles::raw_projection(source)?.as_deref() == Some(title))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub root: PathBuf,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub claude: Endpoint,
    pub codex: Endpoint,
}
impl Pair {
    fn endpoint(&self, tool: Tool) -> &Endpoint {
        match tool {
            Tool::Claude => &self.claude,
            Tool::Codex => &self.codex,
        }
    }
    fn connect(accounts: &AccountManager) -> Result<Self> {
        let endpoint = |tool| -> Result<Endpoint> {
            let env = accounts.launch_environment(tool)?;
            let key = env_key(tool);
            let root = env
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| PathBuf::from(v))
                .or_else(|| std::env::var_os(key).map(PathBuf::from))
                .unwrap_or_else(|| {
                    directories::BaseDirs::new()
                        .expect("home directory")
                        .home_dir()
                        .to_path_buf()
                        .join(if tool == Tool::Claude {
                            ".claude"
                        } else {
                            ".codex"
                        })
                });
            fs::create_dir_all(&root)?;
            let active = env
                .iter()
                .find(|(k, _)| k == "KILN_AGENT_PROFILE")
                .map(|(_, v)| v.clone());
            let label = accounts
                .profiles(tool)
                .into_iter()
                .find(|p| Some(&p.id) == active.as_ref())
                .map(|p| p.label)
                .unwrap_or_else(|| "Default CLI".into());
            Ok(Endpoint {
                root: root.canonicalize()?,
                label,
            })
        };
        Ok(Self {
            claude: endpoint(Tool::Claude)?,
            codex: endpoint(Tool::Codex)?,
        })
    }
}
fn env_key(tool: Tool) -> &'static str {
    if tool == Tool::Claude {
        "CLAUDE_CONFIG_DIR"
    } else {
        "CODEX_HOME"
    }
}
fn opposite(tool: Tool) -> Tool {
    if tool == Tool::Claude {
        Tool::Codex
    } else {
        Tool::Claude
    }
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        text.push(DIGITS[usize::from(b >> 4)] as char);
        text.push(DIGITS[usize::from(b & 15)] as char);
    }
    text
}
fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
/// Hashes the compact JSON encoding without materializing it; equal to `digest(&to_vec(value))`.
fn json_digest(value: &impl Serialize) -> Result<String> {
    struct Hasher(Sha256);
    impl Write for Hasher {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hasher = Hasher(Sha256::new());
    serde_json::to_writer(&mut hasher, value)?;
    Ok(hex(&hasher.0.finalize()))
}
/// Length of the compact JSON encoding, without allocating it.
fn json_len(value: &impl Serialize) -> Result<u64> {
    struct Counter(u64);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}
fn stable_id(text: &str) -> String {
    let mut bytes = Sha256::digest(text.as_bytes());
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h = bytes[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Source {
    tool: Tool,
    root: PathBuf,
    path: PathBuf,
    cwd: String,
    id: String,
}
/// Called only after the existing generation/ID/subagent checks have succeeded.
pub(super) fn track(
    config: &Path,
    record: &super::agent_runtime::Record,
    transcript: &str,
) -> Result<()> {
    let root = record
        .spec
        .env
        .iter()
        .find(|(k, _)| k == env_key(record.tool))
        .map(|(_, v)| PathBuf::from(v))
        .or_else(|| std::env::var_os(env_key(record.tool)).map(PathBuf::from))
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .expect("home directory")
                .home_dir()
                .to_path_buf()
                .join(if record.tool == Tool::Claude {
                    ".claude"
                } else {
                    ".codex"
                })
        })
        .canonicalize()?;
    let path = storage::logical(record.tool, Path::new(transcript));
    safe_path(&root, &path)?;
    let id = record
        .conversation
        .clone()
        .context("Unconfirmed conversation")?;
    ensure!(
        path.extension().is_some_and(|e| e == "jsonl")
            && path
                .file_name()
                .is_some_and(|f| f.to_string_lossy().contains(&id)),
        "Transcript identity mismatch"
    );
    let cwd = PathBuf::from(
        record
            .spec
            .cwd
            .as_deref()
            .context("Missing working directory")?,
    )
    .canonicalize()?
    .to_string_lossy()
    .into_owned();
    let source = Source {
        tool: record.tool,
        root,
        path,
        cwd,
        id,
    };
    let dir = config.join("conversation-sync/tracked");
    private_dir(&dir)?;
    let path = dir.join(format!(
        "{}.json",
        digest(source.path.to_string_lossy().as_bytes())
    ));
    // Repeated hooks carry no conversation text and need not rewrite the reference.
    if !path.exists() {
        publish(&path, &serde_json::to_vec(&source)?)?;
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            fs::symlink_metadata(path)?.is_dir(),
            "Unsafe sync directory"
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Unsafe sync directory"
    );
    Ok(())
}
fn safe_path(root: &Path, path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(root)?.is_dir(),
        "Connected account directory is missing or symlinked"
    );
    let relative = path
        .strip_prefix(root)
        .context("Transcript outside connected account")?;
    ensure!(
        relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "Unsafe transcript path"
    );
    let mut current = root.to_owned();
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "Symlinked transcript path is not supported"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "Expected a regular file"
    );
    let mut file = fs::File::open(path)?;
    let before = file.metadata()?;
    ensure!(
        before.len() <= limit,
        "Conversation exceeds the 64 MiB sync limit"
    );
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() as u64 <= limit
            && before.len() == after.len()
            && before.modified()? == after.modified()?,
        "Transcript is being written; retrying"
    );
    Ok(bytes)
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing destination parent")?;
    private_dir(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path).map_err(|e| e.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
/// `replace`, skipped when the file already holds exactly these bytes.
fn replace_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    let unchanged = fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64)
        && fs::read(path).is_ok_and(|current| current == bytes);
    if unchanged {
        return Ok(());
    }
    replace(path, bytes)
}
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing state parent")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    entries: BTreeMap<String, Entry>,
    recent: Vec<Continuation>,
    #[serde(default)]
    lineage: BTreeMap<String, String>,
    #[serde(default)]
    lineage_checked: std::collections::BTreeSet<String>,
    #[serde(default)]
    archived: BTreeMap<String, PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    source: Source,
    seen: String,
    #[serde(default)]
    imported: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    copied_to: Option<Continuation>,
    #[serde(default)]
    copies: Vec<Continuation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Continuation {
    tool: Tool,
    id: String,
    cwd: String,
    #[serde(default)]
    root: PathBuf,
    #[serde(default)]
    revision: u8,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Queued,
    Reading,
    Copying,
    Synced,
    Waiting,
    Failed,
}
#[derive(Clone)]
struct Conversation {
    workspace: Option<PathBuf>,
    source: Source,
    title: String,
    phase: Phase,
    detail: String,
    target: Option<Continuation>,
    copies: Vec<Continuation>,
}
impl Conversation {
    fn from_entry(entry: &Entry, scope: Option<&discovery::Scope>) -> Self {
        Self {
            workspace: scope.and_then(|s| s.workspace(&entry.source.cwd)).cloned(),
            source: entry.source.clone(),
            title: entry.title.clone(),
            phase: if entry.copied_to.is_some() {
                Phase::Synced
            } else {
                Phase::Queued
            },
            detail: String::new(),
            target: entry.copied_to.clone(),
            copies: entry.copies.clone(),
        }
    }
}
#[derive(Clone, Default)]
struct Status {
    groups: BTreeMap<String, String>,
    settled: std::collections::BTreeSet<String>,
    watched: usize,
    published: usize,
    errors: Vec<String>,
    recent: Vec<Continuation>,
    checked: usize,
    rows: BTreeMap<String, Conversation>,
    workspaces: usize,
    discovered: usize,
    discovering: bool,
}

fn read_messages(source: &Source) -> Result<Vec<format::Message>> {
    read_messages_titled(source, &mut String::new())
}
fn read_messages_titled(source: &Source, title: &mut String) -> Result<Vec<format::Message>> {
    read_rows_titled(source, title).map(|(_, messages)| messages)
}
/// Validated native rows and their portable messages from one read of the transcript.
fn read_rows_titled(
    source: &Source,
    title: &mut String,
) -> Result<(Vec<serde_json::Value>, Vec<format::Message>)> {
    let preferred = if title.is_empty() {
        titles::Cache::default().name(source)?.unwrap_or_default()
    } else {
        title.clone()
    };
    let rows = reader::rows(source, title)?;
    let native_title = titles::from_rows(source, &rows, &preferred);
    if !native_title.is_empty() {
        *title = native_title;
    }
    match source.tool {
        Tool::Codex => {
            let meta = rows
                .iter()
                .find(|r| r["type"] == "session_meta")
                .context("Missing Codex metadata")?;
            ensure!(
                meta["payload"]["id"] == source.id && meta["payload"]["cwd"] == source.cwd,
                "Codex transcript identity changed"
            );
            ensure!(
                matches!(
                    meta["payload"]["cli_version"].as_str(),
                    Some(
                        "0.154.0"
                            | "0.155.0-alpha.9.2"
                            | "0.155.1"
                            | "0.160.0"
                            | "0.160.1"
                            | "0.161.0"
                            | "0.162.0-alpha.17.2"
                            | "0.162.1"
                    )
                ),
                "Unsupported Codex transcript version: {}",
                meta["payload"]["cli_version"]
            );
        }
        Tool::Claude => {
            for row in &rows {
                if matches!(row["type"].as_str(), Some("user" | "assistant")) {
                    ensure!(
                        row["sessionId"] == source.id
                            && row["cwd"]
                                .as_str()
                                .is_some_and(|cwd| Path::new(cwd).is_absolute()),
                        "Claude transcript identity changed"
                    );
                    ensure!(
                        matches!(
                            row["version"].as_str(),
                            Some(
                                "2.1.272"
                                    | "2.1.281"
                                    | "2.1.289"
                                    | "2.1.291"
                                    | "2.1.292"
                                    | "2.1.295"
                                    | "2.1.296"
                                    | "2.1.299"
                            )
                        ),
                        "Unsupported Claude transcript version: {}",
                        row["version"]
                    );
                }
            }
        }
    }
    let messages = format::parse(source.tool, &rows)?;
    *title = titles::clean(title);
    if title.trim().is_empty() {
        *title = titles::fallback(&messages);
    }
    Ok((rows, messages))
}
fn destination(
    pair: &Pair,
    source: &Source,
    id: &str,
    date: chrono::DateTime<chrono::Utc>,
) -> Result<Source> {
    let tool = opposite(source.tool);
    let root = pair.endpoint(tool).root.clone();
    let path = if tool == Tool::Claude {
        let encoded = source
            .cwd
            // Claude's /[^a-zA-Z0-9]/g operates on UTF-16 code units.
            .encode_utf16()
            .map(|c| {
                if c <= 127 && (c as u8).is_ascii_alphanumeric() {
                    c as u8 as char
                } else {
                    '-'
                }
            })
            .collect::<String>();
        ensure!(
            encoded.len() <= 200,
            "Claude project paths longer than 200 characters are not supported"
        );
        root.join("projects")
            .join(encoded)
            .join(format!("{id}.jsonl"))
    } else {
        root.join("sessions")
            .join(date.format("%Y/%m/%d").to_string())
            .join(format!(
                "rollout-{}-{id}.jsonl",
                date.format("%Y-%m-%dT%H-%M-%S")
            ))
    };
    safe_path(&root, &path)?;
    Ok(Source {
        tool,
        root,
        path,
        cwd: source.cwd.clone(),
        id: id.into(),
    })
}

/// Cancellation never blocks the UI on filesystem IO. An in-progress atomic publish may finish.
type Gate = Arc<AtomicBool>;
#[derive(Default)]
struct Cache {
    messages: HashMap<PathBuf, (u64, SystemTime, String)>,
    metadata: discovery::MetadataCache,
    titles: titles::Cache,
    rows: BTreeMap<String, Conversation>,
    attempts: HashMap<PathBuf, (u64, SystemTime, Instant)>,
    /// Consecutive unsettled attempts per unchanged source; selects its retry delay.
    failures: HashMap<PathBuf, u32>,
    /// Snapshot-retirement attempts keyed by source, stamped with both histories.
    retirements: HashMap<String, (Vec<(u64, SystemTime)>, Instant, u32)>,
    /// Sources modified within this window are left until their writer pauses.
    quiet: Duration,
}
impl Cache {
    fn new() -> Self {
        Self::default()
    }
    /// Retries are immediate on the next scan; cached results of unchanged files are kept.
    fn expedite(&mut self) {
        self.attempts.clear();
        self.failures.clear();
        self.retirements.clear();
    }
}
/// Retry delay for an unchanged source after `failures` consecutive unsettled attempts.
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(30 << failures.saturating_sub(1).min(2))
}
/// Whether a file was modified within `quiet` of now; future timestamps count as settled.
fn recently_modified(modified: SystemTime, quiet: Duration) -> bool {
    !quiet.is_zero()
        && SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age < quiet)
}
#[cfg(test)]
fn scan(
    config: &Path,
    pair: &Pair,
    gate: &Gate,
    cache: &mut Cache,
    notify: impl Fn(&Status),
) -> Result<Status> {
    scan_scoped(config, pair, gate, cache, None, notify)
}
#[cfg(test)]
fn scan_scoped(
    config: &Path,
    pair: &Pair,
    gate: &Gate,
    cache: &mut Cache,
    scope: Option<&discovery::Scope>,
    notify: impl Fn(&Status),
) -> Result<Status> {
    scan_with_index(config, pair, gate, cache, scope, |_| Ok(()), notify)
}
#[cfg(test)]
fn scan_with_index(
    config: &Path,
    pair: &Pair,
    gate: &Gate,
    cache: &mut Cache,
    scope: Option<&discovery::Scope>,
    mut index: impl FnMut(&Source) -> Result<()>,
    notify: impl Fn(&Status),
) -> Result<Status> {
    scan_with_metadata(
        config,
        pair,
        gate,
        cache,
        scope,
        |source, request| {
            if matches!(request, Metadata::Archive) {
                let physical = storage::physical(source);
                let path = source.root.join("archived_sessions").join(physical.file_name().unwrap());
                private_dir(path.parent().unwrap())?;
                fs::rename(&physical, &path)?;
                return Ok(Some(path.to_string_lossy().into_owned()));
            }
            if request.title().is_some() {
                index(source)?;
            }
            Ok(None)
        },
        notify,
    )
}
fn scan_with_metadata(
    config: &Path,
    pair: &Pair,
    gate: &Gate,
    cache: &mut Cache,
    scope: Option<&discovery::Scope>,
    mut metadata: impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>,
    notify: impl Fn(&Status),
) -> Result<Status> {
    let dir = config.join("conversation-sync");
    private_dir(&dir)?;
    let lock_path = dir.join("scan.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    lock.try_lock()
        .context("Another Kiln window is synchronizing")?;
    let state_path = dir.join(if scope.is_some() {
        "workspace-state.json".into()
    } else {
        format!("{}.json", digest(&serde_json::to_vec(pair)?))
    });
    let legacy_path = dir.join(format!("{}.json", digest(&serde_json::to_vec(pair)?)));
    let mut state: State = if state_path.exists() {
        serde_json::from_slice(&bounded(&state_path, 8 * 1024 * 1024)?)?
    } else {
        if scope.is_some() && legacy_path.exists() {
            serde_json::from_slice(&bounded(&legacy_path, 8 * 1024 * 1024)?)?
        } else {
            State::default()
        }
    };
    for entry in state.entries.values().filter(|e| e.imported) {
        discovery::remember_import(config, &entry.source, &entry.seen)?;
    }
    let tracked = dir.join("tracked");
    if scope.is_none() && tracked.exists() {
        for (index, entry) in fs::read_dir(tracked)?.enumerate() {
            ensure!(
                index < MAX_SOURCES,
                "Tracked conversation limit reached (4096)"
            );
            let path = entry?.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let source: Source = serde_json::from_slice(&bounded(&path, 16 * 1024)?)?;
            if source.root != pair.endpoint(source.tool).root
                || scope.is_some_and(|s| !s.contains(&source))
            {
                continue;
            }
            state
                .entries
                .entry(source.path.to_string_lossy().into_owned())
                .or_insert(Entry {
                    source,
                    seen: String::new(),
                    imported: false,
                    title: String::new(),
                    copied_to: None,
                    copies: Vec::new(),
                });
        }
    }
    let mut discovery_status = Status::default();
    let mut discovered_paths = std::collections::HashSet::new();
    if let Some(scope) = scope {
        discovery_status.workspaces = scope.workspaces.len();
        discovery_status.discovering = true;
        notify(&discovery_status);
        let found = discovery::find(config, scope, gate, &mut cache.metadata)?;
        discovery_status.discovered = found.len();
        discovered_paths.extend(found.iter().map(|s| s.path.clone()));
        for source in found {
            let key = source.path.to_string_lossy().into_owned();
            if let Some(entry) = state.entries.get_mut(&key) {
                if !entry.imported && discovery::imported(config, &source)?.is_some() {
                    entry.imported = true;
                }
            }
            if !state.entries.contains_key(&key) {
                // Migration/recovery also recognizes copies created by earlier connections.
                let imported = discovery::imported(config, &source)?;
                state.entries.insert(
                    key,
                    Entry {
                        source,
                        seen: imported.clone().unwrap_or_default(),
                        imported: imported.is_some(),
                        title: String::new(),
                        copied_to: None,
                        copies: Vec::new(),
                    },
                );
            }
        }
        // Persist discoveries even before an incomplete or incompatible response can be copied.
        replace_changed(&state_path, &serde_json::to_vec(&state)?)?;
    }
    canonical::migrate(config, &state_path, &legacy_path, &mut state, scope)?;
    state.archived.retain(|key, _| state.entries.get(key).is_none_or(|e| !storage::exists(&e.source)));
    replace_changed(&state_path, &serde_json::to_vec(&state)?)?;
    let active: BTreeMap<_, _> = state
        .entries
        .iter()
        .filter(|(_, e)| {
            scope.is_none_or(|s| {
                s.contains(&e.source)
                    && (discovered_paths.contains(&e.source.path)
                        || (!storage::exists(&e.source) && e.copied_to.is_some()))
            })
        })
        .filter(|(key, _)| !state.archived.contains_key(*key))
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect();
    ensure!(
        active.len() <= MAX_SOURCES,
        "Sync source limit reached (4096); reconnect with a fresh namespace"
    );
    let mut status = Status {
        groups: canonical::groups(&state),
        watched: active.len(),
        workspaces: discovery_status.workspaces,
        discovered: discovery_status.discovered,
        recent: state.recent.clone(),
        rows: active
            .iter()
            .filter(|(_, e)| !e.imported || e.copied_to.is_some())
            .map(|(k, e)| {
                (
                    k.clone(),
                    cache
                        .rows
                        .get(k)
                        .cloned()
                        .unwrap_or_else(|| Conversation::from_entry(e, scope)),
                )
            })
            .collect(),
        ..Default::default()
    };
    notify(&status);
    for (key, _) in active {
        // Earlier endpoints in this scan may have updated this exact conversation.
        let entry = state.entries[&key].clone();
        if !gate.load(Ordering::Acquire) {
            break;
        }
        status.checked += 1;
        if !storage::exists(&entry.source) {
            if let Some(row) = status.rows.get_mut(&key) {
                row.phase = Phase::Waiting;
                row.detail = "Transcript not available yet".into();
            }
            continue;
        }
        // Skipped checks keep the source's retry count; completed ones update it.
        let mut deferred = false;
        let result = (|| -> Result<()> {
            safe_path(&entry.source.root, &storage::physical(&entry.source))?;
            let meta = storage::metadata(&entry.source)?;
            let stamp = (meta.len(), meta.modified()?, entry.seen.clone());
            if let Some((len, modified, at)) = cache.attempts.get(&entry.source.path)
                && *len == stamp.0
                && *modified == stamp.1
                && cache
                    .failures
                    .get(&entry.source.path)
                    .is_some_and(|failures| at.elapsed() < retry_delay(*failures))
            {
                // Imported copies have no initial row; keep their last reported outcome visible.
                if !status.rows.contains_key(&key)
                    && let Some(row) = cache.rows.get(&key)
                {
                    status.rows.insert(key.clone(), row.clone());
                }
                deferred = true;
                return Ok(());
            }
            let destinations = scope
                .map(|s| s.destinations(pair, entry.source.tool))
                .unwrap_or_else(|| vec![pair.clone()]);
            let native_title = if entry.source.tool == Tool::Codex {
                metadata(&entry.source, Metadata::Read)?.or(cache.titles.name(&entry.source)?)
            } else {
                cache.titles.name(&entry.source)?
            };
            let renamed = native_title
                .as_ref()
                .is_some_and(|name| name != &entry.title);
            let mut routes_unchanged = !renamed
                && (entry.imported && entry.copied_to.is_none()
                    || destinations.iter().all(|p| {
                        entry.copies.iter().any(|c| {
                            c.root == p.endpoint(opposite(entry.source.tool)).root
                                && c.revision == PROJECTION_REVISION
                        })
                    }));
            if routes_unchanged && cache.messages.get(&entry.source.path) == Some(&stamp) {
                return Ok(());
            }
            if recently_modified(stamp.1, cache.quiet) {
                if let Some(row) = status.rows.get_mut(&key) {
                    row.phase = Phase::Waiting;
                    row.detail = "Transcript is being written; retrying".into();
                }
                deferred = true;
                return Ok(());
            }
            cache.attempts.insert(
                entry.source.path.clone(),
                (stamp.0, stamp.1, Instant::now()),
            );
            if let Some(row) = status.rows.get_mut(&key) {
                if row.phase == Phase::Queued {
                    row.phase = Phase::Reading;
                }
            }
            notify(&status);
            let mut title = native_title.unwrap_or_default();
            let result = read_messages_titled(&entry.source, &mut title);
            if let Some(row) = status.rows.get_mut(&key) {
                if !title.is_empty() {
                    row.title = title.clone();
                }
            }
            let messages = result?;
            if title != entry.title {
                routes_unchanged = false;
            }
            if let Some(row) = status.rows.get_mut(&key) {
                row.detail.clear();
            }
            if messages.is_empty() || !messages.iter().any(|m| m.role == "user") {
                if let Some(row) = status.rows.get_mut(&key) {
                    row.phase = Phase::Waiting;
                }
                return Ok(());
            }
            let fingerprint = json_digest(&messages)?;
            if entry.imported
                && (entry.title.is_empty() || title == titles::clean(&entry.title))
                && discovery::imported(config, &entry.source)?.as_deref() == Some(&fingerprint)
            {
                // Older parsers could mistake their own projection for a new turn.
                // The immutable import baseline proves this copy was not continued.
                // Retire only that bookkeeping; preserve every native session file.
                if entry.copied_to.is_some() || entry.seen != fingerprint || entry.title.is_empty()
                {
                    let saved = state.entries.get_mut(&key).unwrap();
                    saved.seen = fingerprint;
                    saved.title = title;
                    saved.copied_to = None;
                    saved.copies.clear();
                    replace(&state_path, &serde_json::to_vec(&state)?)?;
                }
                status.rows.remove(&key);
                cache.messages.insert(entry.source.path.clone(), stamp);
                return Ok(());
            }
            if fingerprint == entry.seen && !routes_unchanged && !entry.copies.is_empty() {
                let mut verified = Vec::new();
                for destination in &destinations {
                    let root = &destination.endpoint(opposite(entry.source.tool)).root;
                    if let Some(copy) = entry.copies.iter().find(|c| &c.root == root) {
                        if let Some(target) = state
                            .entries
                            .values()
                            .find(|e| e.source.id == copy.id && e.source.root == copy.root)
                        {
                            if has_current_native_projection(&target.source, &title)?
                                && read_messages(&target.source).is_ok_and(|m| m == messages)
                            {
                                metadata(&target.source, Metadata::Name(&title))?;
                                let mut copy = copy.clone();
                                copy.revision = PROJECTION_REVISION;
                                verified.push(copy);
                            }
                        }
                    }
                }
                // Adopt intact old copies without duplicating a large history during upgrade.
                if verified.len() == destinations.len() {
                    let saved = state.entries.get_mut(&key).unwrap();
                    saved.title = title.clone();
                    saved.copied_to = verified.first().cloned();
                    saved.copies = verified.clone();
                    replace(&state_path, &serde_json::to_vec(&state)?)?;
                    if let Some(row) = status.rows.get_mut(&key) {
                        row.target = verified.first().cloned();
                        row.copies = verified;
                    }
                    routes_unchanged = true;
                }
            }
            if fingerprint == entry.seen && routes_unchanged {
                if let Some(row) = status.rows.get_mut(&key) {
                    row.phase = Phase::Synced;
                }
                cache.messages.insert(entry.source.path.clone(), stamp);
                return Ok(());
            }
            let row = status
                .rows
                .entry(key.clone())
                .or_insert_with(|| Conversation::from_entry(&entry, scope));
            row.title = title.clone();
            row.phase = Phase::Copying;
            notify(&status);
            let mut copies = Vec::new();
            let mut pending_plans = Vec::new();
            for destination_pair in destinations {
                // Include the namespace in workspace mode; both default and connected CLIs
                // must see a continuation even when the other CLI was opened outside Kiln.
                let target_root = &destination_pair.endpoint(opposite(entry.source.tool)).root;
                let existing = canonical::destination(&state, &key, opposite(entry.source.tool), target_root);
                let id = existing.as_ref().map(|s| s.id.clone()).unwrap_or_else(|| stable_id(&format!(
                    "{}:canonical:{key}:{}", state_path.display(), target_root.display())));
                let plan_path = dir.join(format!("pending-{id}.json"));
                #[derive(Serialize, Deserialize)]
                struct Plan {
                    source: Source,
                    timestamp: String,
                }
                let plan: Plan = if let Some(source) = existing {
                    Plan { source, timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }
                } else if plan_path.exists() {
                    serde_json::from_slice(&bounded(&plan_path, 16 * 1024)?)?
                } else {
                    let now = chrono::Utc::now();
                    let plan = Plan {
                        source: destination(&destination_pair, &entry.source, &id, now)?,
                        timestamp: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    };
                    publish(&plan_path, &serde_json::to_vec(&plan)?)?;
                    plan
                };
                ensure!(
                    plan.source.root == destination_pair.endpoint(plan.source.tool).root,
                    "Destination namespace changed"
                );
                safe_path(&plan.source.root, &plan.source.path)?;
                let existed = storage::exists(&plan.source);
                // Only an explicit stored edge permits updating an existing native ID.
                let linked = existed
                    && state.entries.contains_key(&plan.source.path.to_string_lossy().into_owned());
                // A linked endpoint is appended and verified by `canonical::update`;
                // a complete conversion is rendered only for a new copy or a collision check.
                let bytes = if linked {
                    Vec::new()
                } else {
                    let bytes = format::render_titled(
                        plan.source.tool,
                        &id,
                        &entry.source.cwd,
                        &plan.timestamp,
                        &messages,
                        &title,
                    )?;
                    let converted: Vec<serde_json::Value> = bytes
                        .split(|b| *b == b'\n')
                        .filter(|b| !b.is_empty())
                        .map(serde_json::from_slice)
                        .collect::<std::result::Result<_, _>>()?;
                    ensure!(
                        format::parse(plan.source.tool, &converted)? == messages,
                        "Converted conversation did not retain every message and attachment"
                    );
                    drop(converted);
                    ensure!(
                        bytes.len() as u64 <= COPY_LIMIT,
                        "Converted conversation exceeds the 1 GiB sync limit"
                    );
                    bytes
                };
                if !gate.load(Ordering::Acquire) {
                    return Ok(());
                }
                if linked {
                    let _write_guard = handover::write_guard(config, &plan.source)?;
                    canonical::update(&plan.source, &messages, &title, &plan.timestamp)?;
                } else if existed {
                    ensure!(storage::bytes(&plan.source, COPY_LIMIT)?.starts_with(&bytes),
                        "Continuation collision; existing conversation was preserved");
                }
                // Record the baseline before publication so a crash cannot make discovery echo it.
                discovery::remember_projection(
                    config,
                    &plan.source,
                    &fingerprint,
                    Some(&messages),
                )?;
                if !existed {
                    publish(&plan.source.path, &bytes)?;
                }
                metadata(&plan.source, Metadata::Name(&title))?;
                let copied_meta = storage::metadata(&plan.source)?;
                cache.messages.insert(
                    plan.source.path.clone(),
                    (
                        copied_meta.len(),
                        copied_meta.modified()?,
                        fingerprint.clone(),
                    ),
                );
                status.published += 1;
                let target_entry = state
                    .entries
                    .entry(plan.source.path.to_string_lossy().into_owned())
                    .or_insert(Entry {
                        source: plan.source.clone(),
                        seen: fingerprint.clone(),
                        imported: true,
                        title: title.clone(),
                        copied_to: None,
                        copies: Vec::new(),
                    });
                target_entry.seen = fingerprint.clone();
                target_entry.title = title.clone();
                state.lineage.entry(plan.source.path.to_string_lossy().into_owned()).or_insert_with(|| key.clone());
                let continuation = Continuation {
                    tool: plan.source.tool,
                    id,
                    cwd: entry.source.cwd.clone(),
                    root: plan.source.root.clone(),
                    revision: PROJECTION_REVISION,
                };
                state.recent.retain(|c| c.id != continuation.id || c.root != continuation.root);
                state.recent.insert(0, continuation.clone());
                state.recent.truncate(8);
                copies.push(continuation);
                replace(&state_path, &serde_json::to_vec(&state)?)?;
                pending_plans.push(plan_path);
            }
            if entry.source.tool == Tool::Codex {
                metadata(&entry.source, Metadata::Name(&title))?;
            }
            titles::pin_claude(&entry.source, &title)?;
            let saved = state.entries.get_mut(&key).unwrap();
            saved.seen = fingerprint.clone();
            saved.title = title;
            saved.copied_to = copies.first().cloned();
            saved.copies = copies.clone();
            replace(&state_path, &serde_json::to_vec(&state)?)?;
            for path in pending_plans {
                let _ = fs::remove_file(path);
            }
            cache
                .messages
                .insert(entry.source.path.clone(), (stamp.0, stamp.1, fingerprint));
            let row = status.rows.get_mut(&key).unwrap();
            row.phase = Phase::Synced;
            row.target = copies.first().cloned();
            row.copies = copies;
            Ok(())
        })();
        let settled = result.is_ok()
            && status
                .rows
                .get(&key)
                .is_none_or(|r| !matches!(r.phase, Phase::Failed | Phase::Waiting));
        if let Err(error) = result {
            let row = status
                .rows
                .entry(key.clone())
                .or_insert_with(|| Conversation::from_entry(&entry, scope));
            row.phase = if error.to_string().contains("retrying") {
                Phase::Waiting
            } else {
                Phase::Failed
            };
            row.detail = error.to_string();
            status.errors.push(format!(
                "{} {}: {error}",
                entry.source.tool.display_name(),
                entry.source.id
            ));
        }
        if !deferred {
            if settled {
                cache.failures.remove(&entry.source.path);
            } else {
                *cache.failures.entry(entry.source.path.clone()).or_default() += 1;
            }
        }
    }
    canonical::retire(config, &state_path, &mut state, scope, gate, &mut status, cache, &mut metadata)?;
    status.errors = status
        .rows
        .values()
        .filter(|r| r.phase == Phase::Failed)
        .map(|r| {
            format!(
                "{} {}: {}",
                r.source.tool.display_name(),
                r.source.id,
                r.detail
            )
        })
        .collect();
    cache.rows = status.rows.clone();
    if gate.load(Ordering::Acquire) {
        handover::publish_links(config, &state, scope)?;
        status.settled = handover::settled(config, &state, cache, &status)?;
    }
    status.groups = canonical::groups(&state);
    status.recent = state.recent;
    notify(&status);
    Ok(status)
}

enum Event {
    Progress(Gate, Status),
    Connected(Result<Pair, String>),
    Scanned(Gate, Result<Status, String>),
}
fn failure_reason(detail: &str) -> String {
    use kiln_common::i18n::tr;
    if detail.contains("limit") || detail.contains("MiB") {
        tr("대화 용량이 연동 한도를 넘었습니다. 원본은 그대로 보관됩니다.").into()
    } else if detail.contains("version") {
        tr("아직 확인되지 않은 CLI 기록 형식입니다. 원본은 그대로 보관됩니다.").into()
    } else if detail.contains("compacted")
        || detail.contains("chain")
        || detail.contains("inherited")
    {
        tr("전체 원문 연결을 확인할 수 없습니다. 요약본으로 대체하지 않고 연동을 보류합니다.")
            .into()
    } else if detail.contains("image") || detail.contains("content") || detail.contains("encoding")
    {
        tr("첨부 또는 메시지를 온전히 전달할 수 없어 연동을 보류합니다. 대화 정보에서 원인을 확인하세요.").into()
    } else if detail.contains("retrying") {
        tr("기록이 작성 중입니다. 저장이 끝나면 자동으로 다시 확인합니다.").into()
    } else {
        detail.to_owned()
    }
}
#[derive(Clone, Copy)]
enum Badge {
    Attention,
    Working { checked: usize, watched: usize },
    Empty,
    Waiting,
    Synced(usize),
}
pub struct Manager {
    /// Increases whenever the status, error, activity flags or workspace list change.
    revision: u64,
    badges: std::cell::RefCell<HashMap<PathBuf, (u64, Badge)>>,
    completed: u64,
    pub(super) detail: Option<PathBuf>,
    tx: mpsc::SyncSender<Event>,
    rx: mpsc::Receiver<Event>,
    busy: bool,
    connecting: bool,
    gate: Gate,
    next: Instant,
    status: Status,
    error: Option<String>,
    configured: Option<Pair>,
    workspaces: Vec<PathBuf>,
    cache: Arc<Mutex<Cache>>,
    registry: Arc<Mutex<codex_index::Registry>>,
    /// The next scan neither waits for active writers nor applies retry delays.
    urgent: bool,
    /// Native links were emptied after synchronization stopped.
    links_cleared: bool,
}
/// Background scans leave a transcript until it has been unchanged this long.
const QUIET: Duration = Duration::from_secs(10);
impl Default for Manager {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        Self {
            revision: 0,
            badges: Default::default(),
            completed: 0,
            detail: None,
            tx,
            rx,
            busy: false,
            connecting: false,
            gate: Arc::new(AtomicBool::new(false)),
            next: Instant::now(),
            status: Status::default(),
            error: None,
            configured: None,
            workspaces: Vec::new(),
            cache: Arc::new(Mutex::new(Cache::new())),
            registry: Default::default(),
            urgent: false,
            links_cleared: false,
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}
impl Manager {
    pub(super) fn refresh_for_handover(&mut self) -> Option<u64> {
        if self.busy { return None; }
        // Keeps results cached per file length and modification time; clears retry delays.
        self.cache.lock().unwrap().expedite();
        self.urgent = true;
        self.next = Instant::now();
        Some(self.completed)
    }
    pub(super) fn handover_ready(&self, after: u64, group: &str) -> bool {
        self.completed > after && self.error.is_none() && self.status.settled.contains(group)
    }
    pub fn tick(
        &mut self,
        settings: &mut super::state::Settings,
        workspaces: Vec<PathBuf>,
        ctx: &egui::Context,
    ) {
        // Poll quickly only while working; all filesystem/account operations stay off the UI thread.
        while let Ok(event) = self.rx.try_recv() {
            self.revision += 1;
            match event {
                Event::Progress(gate, status) => {
                    if !Arc::ptr_eq(&gate, &self.gate) || !gate.load(Ordering::Acquire) {
                        continue;
                    }
                    if status.discovering && !self.status.rows.is_empty() {
                        continue;
                    }
                    self.status = status;
                }
                Event::Connected(result) => {
                    self.busy = false;
                    self.connecting = false;
                    match result {
                        Ok(pair) => {
                            settings.conversation_sync_pair = Some(pair);
                            settings.conversation_sync_enabled = true;
                            self.next = Instant::now();
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Event::Scanned(gate, result) => {
                    self.busy = false;
                    if !Arc::ptr_eq(&gate, &self.gate) || !gate.load(Ordering::Acquire) {
                        continue;
                    }
                    self.completed += 1;
                    match result {
                        Ok(status) => {
                            self.status = status;
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
            }
        }
        // Connecting or finishing a cancelled scan must still deliver its result when
        // the settings panel has closed and there are no terminal repaint events.
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let enabled = settings.conversation_sync_enabled
            && settings.conversation_sync_pair.is_some()
            && !workspaces.is_empty();
        if !enabled {
            self.cancel_scan();
            if !self.links_cleared {
                self.links_cleared = handover::disable(&kiln_common::paths::config_dir()).is_ok();
            }
            if self.workspaces != workspaces {
                self.revision += 1;
                self.workspaces = workspaces;
            }
            return;
        }
        self.links_cleared = false;
        if self.configured != settings.conversation_sync_pair
            || self.workspaces != workspaces
            || !self.gate.load(Ordering::Acquire)
        {
            self.gate.store(false, Ordering::Release);
            self.gate = Arc::new(AtomicBool::new(true));
            self.revision += 1;
            self.workspaces = workspaces;
            self.status = Status::default();
            self.configured = settings.conversation_sync_pair.clone();
            self.cache = Arc::new(Mutex::new(Cache::new()));
            let registry = std::mem::take(&mut self.registry);
            // Stopping metadata readers waits for their exit off the UI thread.
            std::thread::spawn(move || drop(registry));
            self.next = Instant::now();
        }
        ctx.request_repaint_after(if self.busy {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(3)
        });
        if self.busy || Instant::now() < self.next {
            return;
        }
        self.busy = true;
        self.revision += 1;
        self.next = Instant::now() + Duration::from_secs(3);
        let pair = settings.conversation_sync_pair.clone().unwrap();
        let workspaces = self.workspaces.clone();
        let gate = self.gate.clone();
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        let registry = self.registry.clone();
        let quiet = if std::mem::take(&mut self.urgent) { Duration::ZERO } else { QUIET };
        std::thread::spawn(move || {
            let last_progress = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
            let result = discovery::Scope::new(&pair, workspaces)
                .and_then(|scope| {
                    let mut registry = registry.lock().unwrap();
                    registry.begin_scan();
                    let mut cache = cache.lock().unwrap();
                    cache.quiet = quiet;
                    scan_with_metadata(
                        &kiln_common::paths::config_dir(),
                        &pair,
                        &gate,
                        &mut cache,
                        Some(&scope),
                        |source, request| registry.apply(source, &gate, request),
                        |status| {
                            if last_progress.get().elapsed() >= Duration::from_millis(100) {
                                last_progress.set(Instant::now());
                                let _ = tx.try_send(Event::Progress(gate.clone(), status.clone()));
                            }
                        },
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Scanned(gate, result));
        });
    }
    pub(super) fn accounts_ready(accounts: &AccountManager) -> bool {
        [Tool::Claude, Tool::Codex].into_iter().all(|tool| {
            accounts.active(tool).is_some_and(|id| {
                !accounts.pending_login(tool, &id)
                    && !accounts.login_required(tool, &id)
                    && accounts.profiles(tool).iter().any(|p| p.id == id)
            })
        })
    }
    pub(super) fn cancel_scan(&mut self) {
        self.gate.store(false, Ordering::Release);
        self.next = Instant::now();
    }
    pub(super) fn enable(
        &mut self,
        settings: &mut super::state::Settings,
        accounts: &AccountManager,
    ) {
        if settings.conversation_sync_pair.is_some() {
            settings.conversation_sync_enabled = true;
            return;
        }
        if self.connecting || self.busy {
            return;
        }
        self.busy = true;
        self.connecting = true;
        self.error = None;
        self.revision += 1;
        let accounts = accounts.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = if Self::accounts_ready(&accounts) {
                Pair::connect(&accounts).map_err(|e| e.to_string())
            } else {
                Err(
                    kiln_common::i18n::tr("설정에서 Claude Code와 Codex 계정을 먼저 연결하세요.")
                        .to_owned(),
                )
            };
            let _ = tx.send(Event::Connected(result));
        });
    }
    fn workspace_status(&self, root: &Path) -> Status {
        let rows: BTreeMap<_, _> = self
            .status
            .rows
            .iter()
            .filter(|(_, row)| {
                if let Some(workspace) = &row.workspace {
                    return workspace == root;
                }
                let cwd = Path::new(&row.source.cwd);
                cwd.starts_with(root)
                    && !self.workspaces.iter().any(|other| {
                        other != root && other.starts_with(root) && cwd.starts_with(other)
                    })
            })
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect();
        let rows = canonical::group_rows(rows, &self.status.groups);
        Status {
            watched: rows.len(),
            checked: rows
                .values()
                .filter(|r| matches!(r.phase, Phase::Synced | Phase::Failed | Phase::Waiting))
                .count(),
            published: rows
                .values()
                .filter(|r| r.phase == Phase::Synced)
                .map(|r| r.copies.len().max(usize::from(r.target.is_some())))
                .sum(),
            rows,
            workspaces: 1,
            discovering: self.status.discovering || self.connecting,
            ..Status::default()
        }
    }
    fn badge_state(&self, root: &Path) -> Badge {
        if let Some((revision, badge)) = self.badges.borrow().get(root)
            && *revision == self.revision
        {
            return *badge;
        }
        let status = self.workspace_status(root);
        let badge = if self.error.is_some() || status.rows.values().any(|r| r.phase == Phase::Failed) {
            Badge::Attention
        } else if self.connecting
            || (status.discovering && status.rows.is_empty())
            || status
                .rows
                .values()
                .any(|r| matches!(r.phase, Phase::Queued | Phase::Reading | Phase::Copying))
            || (self.busy && status.rows.is_empty())
        {
            Badge::Working { checked: status.checked, watched: status.watched }
        } else if status.rows.is_empty() {
            Badge::Empty
        } else if status.rows.values().any(|r| r.phase == Phase::Waiting) {
            Badge::Waiting
        } else {
            Badge::Synced(status.rows.len())
        };
        self.badges.borrow_mut().insert(root.to_path_buf(), (self.revision, badge));
        badge
    }
    /// Workspace badges are recomputed only after the sync status changes.
    pub(super) fn badge(&self, root: &Path) -> (kiln_common::icons::Icon, egui::Color32, String) {
        use kiln_common::{Theme, i18n::tr, icons::Icon};
        let theme = Theme::current();
        match self.badge_state(root) {
            Badge::Attention => (Icon::Warning, theme.orange, tr("대화 연동 · 확인 필요").into()),
            Badge::Working { checked, watched } => (
                Icon::Refresh,
                theme.blue,
                kiln_common::trf!("대화 연동 중 · {}/{}", checked, watched),
            ),
            Badge::Empty => (Icon::Plug, theme.text_dim, tr("대화 연동됨 · 대화 대기").into()),
            Badge::Waiting => (Icon::History, theme.blue, tr("대화 연동 · 응답 완료 대기").into()),
            Badge::Synced(count) => (
                Icon::Check,
                theme.green,
                kiln_common::trf!("대화 연동 완료 · {}개", count),
            ),
        }
    }
    pub(super) fn dialog(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        root: &Path,
        enabled: bool,
        settings: &mut super::state::Settings,
        accounts: &AccountManager,
    ) {
        use kiln_common::{Theme, fonts, i18n::tr};
        let theme = Theme::current();
        let mut close = false;
        let status = self.workspace_status(root);
        let modal = egui::Modal::new(egui::Id::new("workspace-conversation-sync"))
            .frame(egui::Frame::new().fill(theme.bg_panel).stroke(egui::Stroke::new(1.0, theme.border)).corner_radius(14).inner_margin(20))
            .show(ctx, |ui| {
                ui.set_width(600.0_f32.min((ctx.content_rect().width() - 64.0).max(200.0)));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tr("대화 연동")).font(fonts::semibold(18.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = kiln_common::widgets::icon_button(ui, kiln_common::icons::Icon::Close, 28.0, false, tr("닫기 (Esc)")).clicked();
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(name).font(fonts::semibold(14.0)));
                    ui.add(egui::Label::new(egui::RichText::new(root.display().to_string()).color(theme.text_dim)).truncate()).on_hover_text(root.display().to_string());
                });
                if let Some(pair) = &settings.conversation_sync_pair {
                    ui.add(egui::Label::new(format!("Claude Code: {} ↔ Codex: {}", pair.claude.label, pair.codex.label)).truncate());
                }
                // A single scroll surface; fixed summary and footer never reflow with polling.
                let list_height = (ctx.content_rect().height() - 340.0).clamp(120.0, 520.0);
                self.progress_status_ui(ui, enabled, &status, list_height);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(tr("연동 범위와 지원 형식")).on_hover_text(tr("이 워크스페이스와 하위 폴더의 최근 대화 20개부터 연결합니다. 이후 새 대화와 변경된 대화를 자동 동기화합니다."))
                        .on_hover_text(tr("이미지를 포함한 원문을 연동합니다. 전달할 수 없는 첨부가 있으면 완료로 표시하지 않습니다."));
                    if ui.button(tr("다시 확인")).clicked() {
                        self.cache = Arc::new(Mutex::new(Cache::new()));
                        self.urgent = true;
                        self.next = Instant::now();
                    }
                    if enabled && settings.conversation_sync_pair.is_none() && !self.connecting && Self::accounts_ready(accounts)
                        && ui.button(tr("계정 연결 다시 시도")).clicked() { self.enable(settings, accounts); }
                });
                ui.add(egui::Label::new(egui::RichText::new(tr("같은 폴더에서 /resume으로 정본을 여세요. Kiln 안에서는 이전 에이전트를 중지한 뒤 이어갑니다.")).color(theme.text_dim)).wrap());
            });
        if close || modal.should_close() {
            self.detail = None;
        }
    }
    #[cfg(test)]
    fn progress_ui(&self, ui: &mut egui::Ui, enabled: bool) {
        self.progress_status_ui(ui, enabled, &self.status, 280.0);
    }
    fn progress_status_ui(
        &self,
        ui: &mut egui::Ui,
        enabled: bool,
        status: &Status,
        list_height: f32,
    ) {
        use kiln_common::{
            Theme, fonts,
            i18n::tr,
            icons::{self, Icon},
        };
        let theme = Theme::current();
        let failed = status
            .rows
            .values()
            .filter(|r| r.phase == Phase::Failed)
            .count();
        let waiting = status
            .rows
            .values()
            .filter(|r| matches!(r.phase, Phase::Waiting | Phase::Queued))
            .count();
        let copying = status
            .rows
            .values()
            .any(|r| matches!(r.phase, Phase::Reading | Phase::Copying));
        let (icon, color, label) = if !enabled {
            (Icon::Stop, theme.text_dim, tr("동기화 일시 중지"))
        } else if failed > 0 || self.error.is_some() {
            (
                Icon::Warning,
                theme.orange,
                tr("확인이 필요한 대화가 있습니다"),
            )
        } else if status.discovering && status.rows.is_empty() {
            (Icon::Refresh, theme.blue, tr("워크스페이스 대화 찾는 중"))
        } else if copying {
            (Icon::Refresh, theme.blue, tr("대화 동기화 중"))
        } else if status.rows.is_empty() {
            (
                Icon::Plug,
                theme.text_dim,
                tr("연결됨 · 해당 폴더에서 대화를 찾지 못함"),
            )
        } else if waiting > 0 {
            (Icon::History, theme.blue, tr("응답 완료를 기다리는 중"))
        } else {
            (Icon::Check, theme.green, tr("완료된 대화 모두 동기화됨"))
        };
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(theme.bg_elevated)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(10)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    icons::paint(ui.painter(), rect.shrink(2.0), icon, color);
                    ui.label(
                        egui::RichText::new(label)
                            .color(color)
                            .font(fonts::semibold(13.0)),
                    );
                });
                if status.workspaces > 0 {
                    ui.label(kiln_common::trf!("워크스페이스 {}개 · 대화 {}개", status.workspaces, status.rows.len()));
                } else if enabled && !self.busy {
                    ui.label(tr("동기화할 워크스페이스 폴더를 여세요."));
                }
                if enabled && status.rows.is_empty() && !self.busy && status.workspaces > 0 {
                    ui.add(egui::Label::new(tr("해당 폴더에서 Claude Code 또는 Codex 대화를 시작하면 자동으로 연결됩니다.")).wrap());
                }
                ui.label(kiln_common::trf!("완료 {}개 · 확인 필요 {}개 · 대기 {}개", status.rows.values().filter(|r| r.phase == Phase::Synced).count(), failed, waiting));
                if enabled && status.watched > 0 {
                    let fraction = status.checked as f32 / status.watched as f32;
                    ui.add(egui::ProgressBar::new(fraction).text(kiln_common::trf!(
                        "기록 확인 {}/{} · 연결된 사본 {}개",
                        status.checked,
                        status.watched,
                        status.published
                    )));
                }
                if let Some(error) = &self.error {
                    ui.add(egui::Label::new(egui::RichText::new(error).color(theme.orange)).wrap());
                }
                if !enabled {
                    ui.label(
                        egui::RichText::new(tr("아래는 마지막 동기화 결과입니다."))
                            .color(theme.text_dim),
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("conversation-sync-progress")
                    .max_height(list_height)
                    .min_scrolled_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (key, row) in &status.rows {
                            ui.push_id(key, |ui| {
                                ui.add_space(8.0);
                                kiln_common::widgets::divider(ui);
                                ui.add_space(8.0);
                                ui.horizontal_wrapped(|ui| {
                                    let (icon, color, label) = match row.phase {
                                        Phase::Queued => {
                                            (Icon::History, theme.text_dim, tr("대기"))
                                        }
                                        Phase::Waiting => {
                                            (Icon::History, theme.text_dim, tr("응답 완료 대기"))
                                        }
                                        Phase::Reading => {
                                            (Icon::Refresh, theme.blue, tr("대화 확인 중"))
                                        }
                                        Phase::Copying => {
                                            (Icon::Refresh, theme.blue, tr("동기화 중"))
                                        }
                                        Phase::Synced => {
                                            (Icon::Check, theme.green, tr("동기화 완료"))
                                        }
                                        Phase::Failed => {
                                            (Icon::Warning, theme.orange, tr("동기화 실패"))
                                        }
                                    };
                                    if matches!(row.phase, Phase::Reading | Phase::Copying)
                                        && enabled
                                    {
                                        ui.add(egui::Spinner::new().size(14.0));
                                    } else {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(16.0, 16.0),
                                            egui::Sense::hover(),
                                        );
                                        icons::paint(ui.painter(), rect, icon, color);
                                    }
                                    ui.label(egui::RichText::new(label).color(color));
                                    ui.label(format!(
                                        "{} → {}",
                                        row.source.tool.display_name(),
                                        opposite(row.source.tool).display_name()
                                    ));
                                });
                                if !row.detail.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(failure_reason(&row.detail)).color(theme.orange),
                                        )
                                        .wrap(),
                                    );
                                }
                                let title = if row.title.is_empty() {
                                    row.source.id.as_str()
                                } else {
                                    row.title.as_str()
                                };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(title).font(fonts::semibold(12.0)),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(title);
                                ui.collapsing(tr("대화 정보"), |ui| {
                                    ui.add(egui::Label::new(&row.source.cwd).wrap());
                                    ui.label(&row.source.id);
                                    ui.add(egui::Label::new(&row.detail).wrap());
                                    if ui.small_button(tr("오류 정보 복사")).clicked() {
                                        ui.ctx().copy_text(format!("{}\n{}\n{}", row.source.tool.display_name(), row.source.id, row.detail));
                                    }
                                });
                                let targets = if row.copies.is_empty() {
                                    row.target.iter().collect::<Vec<_>>()
                                } else { row.copies.iter().collect::<Vec<_>>() };
                                for target in targets {
                                    ui.push_id(&target.id, |ui| {
                                    if ui
                                        .small_button(tr("이어갈 대화 ID 복사"))
                                        .on_hover_text(format!("{}\n{}", target.id, target.root.display()))
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(target.id.clone());
                                    }
                                    if row.copies.len() > 1 {
                                        let label = self.configured.as_ref()
                                            .filter(|p| p.endpoint(target.tool).root == target.root)
                                            .map(|p| p.endpoint(target.tool).label.clone())
                                            .unwrap_or_else(|| tr("기본 CLI").to_owned());
                                        ui.label(egui::RichText::new(label).color(theme.text_dim))
                                            .on_hover_text(target.root.display().to_string());
                                    }
                                    });
                                }
                            });
                        }
                    });
            });
    }
}
