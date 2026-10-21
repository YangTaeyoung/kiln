//! Opt-in native continuations discovered within open workspace folders.
use kiln_common::paths::Canonical;
mod codex_index;
mod canonical;
mod checkpoint;
mod context;
mod discovery;
mod engine;
mod format;
pub(super) mod handover;
mod hub;
mod migrate;
mod portable;
mod reader;
mod storage;
#[cfg(test)]
mod tests;
mod titles;
mod tools;
mod worker;

/// Entry point of the hidden `kiln sync-worker` command.
pub fn serve_worker() -> Result<()> {
    worker::serve()
}

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
                root: root.canonical()?,
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
        .canonical()?;
    let path = storage::logical(record.tool, &under_root(&root, Path::new(transcript)));
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
    .canonical()?
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

/// `path` restated under `root` when one of its folders is `root` spelled another way:
/// letter case, short names, separators, a verbatim prefix or a symbolic link above it.
fn under_root(root: &Path, path: &Path) -> PathBuf {
    path.ancestors()
        .find(|folder| folder.canonical().is_ok_and(|folder| folder == root))
        .and_then(|folder| path.strip_prefix(folder).ok())
        .map_or_else(|| path.to_path_buf(), |relative| root.join(relative))
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
    #[cfg(windows)]
    kiln_common::access::restrict_to_user(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Unsafe sync directory"
    );
    Ok(())
}
fn safe_path(root: &Path, path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()),
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
    sync_dir(parent)?;
    Ok(())
}
/// Flushes the entries of `dir` to disk on Unix; does nothing on Windows.
fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(windows)]
    let _ = dir;
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
    sync_dir(parent)?;
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum Phase {
    Queued,
    Reading,
    Copying,
    Synced,
    Waiting,
    Failed,
}
#[derive(Clone, Serialize, Deserialize)]
struct Conversation {
    workspace: Option<PathBuf>,
    source: Source,
    title: String,
    phase: Phase,
    detail: String,
    target: Option<Continuation>,
    copies: Vec<Continuation>,
    /// Archived continuations that can be restored as separate conversations.
    #[serde(default)]
    branches: Vec<BranchRow>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct BranchRow {
    id: String,
    events: usize,
    at: SystemTime,
}
/// A failure the user can resolve; every other failure is shown as information.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Attention {
    /// A connected account directory is missing or unusable.
    Account,
    /// Storage is full, read-only or not writable.
    Storage,
    /// Codex CLI, needed to register Codex /resume entries, is not installed.
    CodexCli,
}
fn classify(detail: &str) -> Option<Attention> {
    if detail.contains("Connected account directory is missing") || detail.contains("Transcript outside connected account") {
        Some(Attention::Account)
    } else if ["No space left on device", "Permission denied", "Read-only file system", "Disk quota exceeded"]
        .iter()
        .any(|m| detail.contains(m))
    {
        Some(Attention::Storage)
    } else if detail.contains("Install Codex CLI") {
        Some(Attention::CodexCli)
    } else {
        None
    }
}
impl Conversation {
    /// A failed row whose linked histories diverged; nothing is left for the user to resolve.
    fn diverged(&self) -> bool {
        self.phase == Phase::Failed && self.detail.contains(DIVERGED)
    }
    fn attention(&self) -> Option<Attention> {
        (self.phase == Phase::Failed).then(|| classify(&self.detail)).flatten()
    }
    #[cfg(test)]
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
            branches: Vec::new(),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
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
    read_rows_offsets_titled(source, title).map(|(rows, _, messages)| (rows, messages))
}
/// `read_rows_titled` with the byte offset at the end of each row.
fn read_rows_offsets_titled(
    source: &Source,
    title: &mut String,
) -> Result<(Vec<serde_json::Value>, Vec<u64>, Vec<format::Message>)> {
    let preferred = if title.is_empty() {
        titles::Cache::default().name(source)?.unwrap_or_default()
    } else {
        title.clone()
    };
    let (rows, offsets) = reader::rows_from(source, title, 0)?;
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
                }
            }
        }
    }
    let messages = format::parse(source.tool, &rows)?;
    *title = titles::clean(title);
    if title.trim().is_empty() {
        *title = titles::fallback(&messages);
    }
    Ok((rows, offsets, messages))
}
#[cfg(test)]
fn destination(
    pair: &Pair,
    source: &Source,
    id: &str,
    date: chrono::DateTime<chrono::Utc>,
) -> Result<Source> {
    let tool = opposite(source.tool);
    let root = pair.endpoint(tool).root.clone();
    let path = native_path(tool, &root, &source.cwd, id, date)?;
    Ok(Source {
        tool,
        root,
        path,
        cwd: source.cwd.clone(),
        id: id.into(),
    })
}
/// Native transcript path of a session in a CLI namespace.
fn native_path(
    tool: Tool,
    root: &Path,
    cwd: &str,
    id: &str,
    date: chrono::DateTime<chrono::Utc>,
) -> Result<PathBuf> {
    let path = if tool == Tool::Claude {
        let encoded = cwd
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
            .join(date.format("%Y").to_string())
            .join(date.format("%m").to_string())
            .join(date.format("%d").to_string())
            .join(format!(
                "rollout-{}-{id}.jsonl",
                date.format("%Y-%m-%dT%H-%M-%S")
            ))
    };
    safe_path(root, &path)?;
    Ok(path)
}

/// Cancellation never blocks the UI on filesystem IO. An in-progress atomic publish may finish.
type Gate = Arc<AtomicBool>;
/// File-stamped results and retry delays survive a sync worker restart.
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    messages: HashMap<PathBuf, (u64, SystemTime, String)>,
    metadata: discovery::MetadataCache,
    titles: titles::Cache,
    rows: BTreeMap<String, Conversation>,
    /// Failed hub conversations keyed by conversation ID: projection fingerprint, attempt
    /// time, consecutive failures and whether only a changed history can resolve it.
    #[serde(default)]
    hub: HashMap<String, (u64, SystemTime, u32, bool)>,
    /// Kiln build that recorded the failures; another build reads failed conversations again.
    #[serde(default)]
    build: String,
    /// Sources modified within this window are left until their writer pauses.
    #[serde(skip)]
    quiet: Duration,
}
impl Cache {
    fn new() -> Self {
        Self::default()
    }
    /// Retries are immediate on the next scan; cached results of unchanged files are kept.
    fn expedite(&mut self) {
        self.hub.clear();
    }
}
/// Retry delay for an unchanged source after `failures` consecutive unsettled attempts.
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(30 << failures.saturating_sub(1).min(2))
}
/// A failed attempt whose linked histories are all unchanged is retried after this delay.
const FAILED_RECHECK: Duration = Duration::from_secs(60 * 60);
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
    let pair_state = dir.join(format!("{}.json", digest(&serde_json::to_vec(pair)?)));
    let legacy = if scope.is_some() {
        vec![dir.join("workspace-state.json"), pair_state]
    } else {
        vec![pair_state]
    };
    let build = kiln_daemon::build_id().to_string();
    if cache.build != build {
        cache.hub.clear();
        cache.build = build;
    }
    let mut hub = hub::Hub::open(config)?;
    migrate::run(&mut hub, &legacy, gate)?;
    let mut status = Status::default();
    let sources: Vec<Source> = if let Some(scope) = scope {
        status.workspaces = scope.workspaces.len();
        status.discovering = true;
        notify(&status);
        let found = discovery::find(config, scope, gate, &mut cache.metadata)?;
        status.discovering = false;
        status.discovered = found.len();
        found
    } else {
        let tracked = dir.join("tracked");
        let mut sources = Vec::new();
        if tracked.exists() {
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
                if source.root == pair.endpoint(source.tool).root {
                    sources.push(source);
                }
            }
        }
        sources
    };
    let namespaces = scope.map(|s| s.roots.clone()).unwrap_or_else(|| {
        Tool::ALL
            .into_iter()
            .map(|tool| (tool, pair.endpoint(tool).root.clone()))
            .collect()
    });
    let hub = engine::Sync::new(config, scope, namespaces, gate, cache, &mut status, &mut metadata, &notify)
        .run(hub, sources)?;
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
        handover::publish(config, &engine::links(&hub, scope))?;
    }
    status.groups = status.rows.keys().map(|k| (k.clone(), k.clone())).collect();
    notify(&status);
    Ok(status)
}

enum Event {
    Progress(Gate, Status),
    Connected(Result<Pair, String>),
    Scanned(Gate, Result<Status, String>),
}
/// Both linked histories continued separately; each is kept as its own conversation.
const DIVERGED: &str = "Linked histories diverged; both conversations were preserved";
fn failure_reason(detail: &str) -> String {
    use kiln_common::i18n::tr;
    match classify(detail) {
        Some(Attention::Account) => return tr("연결 계정 폴더를 찾을 수 없습니다. 설정에서 계정을 다시 연결하면 자동으로 이어집니다.").into(),
        Some(Attention::Storage) => return tr("저장 공간이 부족하거나 쓰기 권한이 없습니다. 공간이나 권한을 확인한 뒤 다시 시도하세요.").into(),
        Some(Attention::CodexCli) => return tr("Codex /resume 목록에 등록하려면 Codex CLI가 필요합니다. 설치하면 자동으로 다시 시도합니다.").into(),
        None => {}
    }
    if detail.contains(DIVERGED) {
        tr("두 CLI에서 각각 이어 써서 하나로 합치지 않았습니다. 두 대화 모두 그대로 있으며 각 CLI의 /resume에서 따로 열 수 있습니다.").into()
    } else if detail.contains("Unsupported content") || detail.contains("Unsupported Codex item") || detail.contains("Unsupported visible") {
        tr("Kiln이 아직 모르는 기록 형식이 있어 이 대화를 보류합니다. Kiln이 업데이트되면 자동으로 다시 확인합니다. 원본은 그대로 보관됩니다.").into()
    } else if detail.contains("limit") || detail.contains("MiB") {
        tr("대화 용량이 연동 한도를 넘었습니다. 원본은 그대로 보관됩니다.").into()
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
    worker: Option<worker::Worker>,
    /// Executable and build identity the worker is started from, captured at startup.
    worker_exe: (PathBuf, String),
    scan_id: u64,
    worker_scan: Option<u64>,
    /// The next worker scan clears retry delays / starts from an empty cache.
    expedite_next: bool,
    reset_next: bool,
    /// Ignored warnings by row key, with the condition that was ignored.
    ignored: BTreeMap<String, Attention>,
    /// Where ignored warnings are saved; tests keep them in memory.
    ignored_path: Option<PathBuf>,
    /// Asks the app to open the account settings.
    pub(super) open_accounts: bool,
    show_ignored: bool,
}
/// Ignore key of a failure of the whole scan.
const SCAN_KEY: &str = "scan";
/// The resolving action and Ignore button of a warning.
fn attention_buttons(ui: &mut egui::Ui, key: &str, kind: Attention, actions: &mut Vec<RowAction>) {
    use kiln_common::i18n::tr;
    ui.horizontal_wrapped(|ui| {
        let label = match kind {
            Attention::Account => tr("계정 설정 열기"),
            Attention::Storage | Attention::CodexCli => tr("다시 시도"),
        };
        if ui.small_button(label).clicked() {
            actions.push(match kind {
                Attention::Account => RowAction::OpenAccounts,
                _ => RowAction::Retry,
            });
        }
        if ui.small_button(tr("무시")).on_hover_text(tr("이 문제는 경고에서 빼고, 다른 문제가 생기면 다시 알립니다.")).clicked() {
            actions.push(RowAction::Ignore(key.to_owned(), kind));
        }
    });
}
/// An action chosen in the status dialog.
enum RowAction {
    OpenAccounts,
    Retry,
    Ignore(String, Attention),
    Unignore(String),
    ToggleIgnored,
    Restore { branch: String, title: String },
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
            worker: None,
            worker_exe: {
                let exe = super::conn::daemon_exe();
                let build = kiln_daemon::exe_build_id(&exe);
                (exe, build)
            },
            scan_id: 0,
            worker_scan: None,
            expedite_next: false,
            reset_next: false,
            ignored_path: (!cfg!(test)).then(|| kiln_common::paths::config_dir().join("conversation-sync/ignored.json")),
            ignored: if cfg!(test) {
                BTreeMap::new()
            } else {
                fs::read(kiln_common::paths::config_dir().join("conversation-sync/ignored.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default()
            },
            open_accounts: false,
            show_ignored: false,
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}
impl Manager {
    fn is_ignored(&self, key: &str, row: &Conversation) -> bool {
        row.attention().is_some_and(|kind| self.ignored.get(key) == Some(&kind))
    }
    /// The resolvable condition of a failed scan, unless it was ignored.
    fn scan_attention(&self) -> Option<Attention> {
        self.error
            .as_deref()
            .and_then(classify)
            .filter(|kind| self.ignored.get(SCAN_KEY) != Some(kind))
    }
    fn save_ignored(&mut self) {
        self.revision += 1;
        if let Some(path) = &self.ignored_path
            && let Ok(bytes) = serde_json::to_vec(&self.ignored)
        {
            let _ = private_dir(path.parent().unwrap()).and_then(|_| replace(path, &bytes));
        }
    }
    fn ignore(&mut self, key: &str, kind: Attention) {
        self.ignored.insert(key.to_owned(), kind);
        self.save_ignored();
    }
    fn unignore(&mut self, key: &str) {
        self.ignored.remove(key);
        self.save_ignored();
    }
    /// The next scan starts now and retries failed conversations without their delay.
    fn retry_now(&mut self) {
        self.cache.lock().unwrap().expedite();
        self.expedite_next = true;
        self.urgent = true;
        self.next = Instant::now();
    }
    fn apply(&mut self, actions: Vec<RowAction>) {
        for action in actions {
            match action {
                RowAction::OpenAccounts => {
                    self.open_accounts = true;
                    self.detail = None;
                }
                RowAction::Retry => self.retry_now(),
                RowAction::Ignore(key, kind) => self.ignore(&key, kind),
                RowAction::Unignore(key) => self.unignore(&key),
                RowAction::ToggleIgnored => self.show_ignored = !self.show_ignored,
                RowAction::Restore { branch, title } => {
                    if hub::request_restore(&kiln_common::paths::config_dir(), &branch, &title).is_ok() {
                        self.retry_now();
                    }
                }
            }
        }
    }
    pub(super) fn refresh_for_handover(&mut self) -> Option<u64> {
        if self.busy { return None; }
        // Keeps results cached per file length and modification time; clears retry delays.
        self.cache.lock().unwrap().expedite();
        self.expedite_next = true;
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
                // A scan still running may publish links once more; clear again after it ends.
                self.links_cleared = handover::disable(&kiln_common::paths::config_dir()).is_ok() && !self.busy;
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
            self.cancel_worker_scan();
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
        let quiet = if std::mem::take(&mut self.urgent) { Duration::ZERO } else { QUIET };
        if self.scan_in_worker(&pair, quiet, ctx) {
            return;
        }
        let workspaces = self.workspaces.clone();
        let gate = self.gate.clone();
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        let registry = self.registry.clone();
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
    /// Sends the scan to the sync worker process; false when no worker can run it,
    /// in which case the scan runs on a thread of this process.
    fn scan_in_worker(&mut self, pair: &Pair, quiet: Duration, ctx: &egui::Context) -> bool {
        // An executable replaced on disk (an installed update) may not speak this protocol.
        let (exe, build) = &self.worker_exe;
        if cfg!(test) || kiln_daemon::exe_build_id(exe) != *build {
            return false;
        }
        if !self.worker.as_mut().is_some_and(worker::Worker::alive) {
            self.worker = worker::Worker::start(exe, self.tx.clone(), ctx.clone()).ok();
        }
        let Some(worker) = self.worker.as_mut() else {
            return false;
        };
        self.scan_id += 1;
        let (expedite, reset) = (std::mem::take(&mut self.expedite_next), std::mem::take(&mut self.reset_next));
        match worker.scan(self.scan_id, self.gate.clone(), pair, &self.workspaces, quiet, expedite, reset) {
            Ok(()) => {
                self.worker_scan = Some(self.scan_id);
                true
            }
            Err(_) => {
                self.worker = None;
                self.expedite_next |= expedite;
                self.reset_next |= reset;
                false
            }
        }
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
    fn cancel_worker_scan(&mut self) {
        if let (Some(worker), Some(id)) = (self.worker.as_mut(), self.worker_scan.take()) {
            worker.cancel(id);
        }
    }
    pub(super) fn cancel_scan(&mut self) {
        self.gate.store(false, Ordering::Release);
        self.cancel_worker_scan();
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
        let badge = if self.scan_attention().is_some()
            || status.rows.iter().any(|(key, row)| row.attention().is_some() && !self.is_ignored(key, row))
        {
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
        let mut actions = Vec::new();
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
                actions = self.progress_status_ui(ui, enabled, &status, list_height);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(tr("연동 범위와 지원 형식")).on_hover_text(tr("이 워크스페이스와 하위 폴더의 최근 대화 20개부터 연결합니다. 이후 새 대화와 변경된 대화를 자동 동기화합니다."))
                        .on_hover_text(tr("이미지를 포함한 원문을 연동합니다. 전달할 수 없는 첨부가 있으면 완료로 표시하지 않습니다."));
                    if ui.button(tr("다시 확인")).clicked() {
                        self.cache = Arc::new(Mutex::new(Cache::new()));
                        self.reset_next = true;
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
        self.apply(actions);
    }
    #[cfg(test)]
    fn progress_ui(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let status = std::mem::take(&mut self.status);
        let actions = self.progress_status_ui(ui, enabled, &status, 280.0);
        self.status = status;
        self.apply(actions);
    }
    fn progress_status_ui(
        &self,
        ui: &mut egui::Ui,
        enabled: bool,
        status: &Status,
        list_height: f32,
    ) -> Vec<RowAction> {
        use kiln_common::{
            Theme, fonts,
            i18n::tr,
            icons::{self, Icon},
        };
        let theme = Theme::current();
        let mut actions = Vec::new();
        let active = |key: &String, row: &Conversation| row.attention().is_some() && !self.is_ignored(key, row);
        let failed = status.rows.iter().filter(|(k, r)| active(k, r)).count();
        let ignored = status.rows.iter().filter(|(k, r)| self.is_ignored(k, r)).count();
        let held = status
            .rows
            .values()
            .filter(|r| r.phase == Phase::Failed && r.attention().is_none())
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
        let scan_attention = self.scan_attention().is_some();
        let (icon, color, label) = if !enabled {
            (Icon::Stop, theme.text_dim, tr("동기화 일시 중지"))
        } else if failed > 0 || scan_attention {
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
                if held > 0 {
                    ui.label(egui::RichText::new(kiln_common::trf!("보류된 대화 {}개 · 원본은 그대로 보관됨", held)).color(theme.text_dim));
                }
                if ignored > 0 {
                    let label = if self.show_ignored { tr("무시한 경고 숨기기") } else { tr("무시한 경고 보기") };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(kiln_common::trf!("무시한 경고 {}개", ignored)).color(theme.text_dim));
                        if ui.small_button(label).clicked() {
                            actions.push(RowAction::ToggleIgnored);
                        }
                    });
                }
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
                    let color = if scan_attention { theme.orange } else { theme.text_dim };
                    ui.add(egui::Label::new(egui::RichText::new(failure_reason(error)).color(color)).wrap());
                    match (self.scan_attention(), classify(error)) {
                        (Some(kind), _) => attention_buttons(ui, SCAN_KEY, kind, &mut actions),
                        (None, Some(_)) => {
                            if ui.small_button(tr("다시 표시")).clicked() {
                                actions.push(RowAction::Unignore(SCAN_KEY.into()));
                            }
                        }
                        (None, None) => {}
                    }
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
                            let is_ignored = self.is_ignored(key, row);
                            if is_ignored && !self.show_ignored {
                                continue;
                            }
                            let attention = row.attention().filter(|_| !is_ignored);
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
                                        Phase::Failed if attention.is_some() => {
                                            (Icon::Warning, theme.orange, tr("동기화 실패"))
                                        }
                                        Phase::Failed if is_ignored => {
                                            (Icon::Info, theme.text_dim, tr("무시됨"))
                                        }
                                        Phase::Failed if row.diverged() => {
                                            (Icon::Info, theme.text_dim, tr("따로 이어짐"))
                                        }
                                        Phase::Failed => (Icon::Info, theme.text_dim, tr("보류됨")),
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
                                    let mut tools = vec![row.source.tool];
                                    tools.extend(row.copies.iter().map(|c| c.tool));
                                    tools.dedup();
                                    tools.sort_by_key(|t| t.key());
                                    tools.dedup();
                                    ui.label(tools.iter().map(|t| t.display_name()).collect::<Vec<_>>().join(" · "));
                                });
                                if !row.detail.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(failure_reason(&row.detail))
                                                .color(if attention.is_some() { theme.orange } else { theme.text_dim }),
                                        )
                                        .wrap(),
                                    );
                                }
                                if let Some(kind) = attention {
                                    attention_buttons(ui, key, kind, &mut actions);
                                } else if is_ignored && ui.small_button(tr("다시 표시")).clicked() {
                                    actions.push(RowAction::Unignore(key.clone()));
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
                                let mut targets = vec![Continuation {
                                    tool: row.source.tool,
                                    id: row.source.id.clone(),
                                    cwd: row.source.cwd.clone(),
                                    root: row.source.root.clone(),
                                    revision: PROJECTION_REVISION,
                                }];
                                targets.extend(row.copies.iter().cloned());
                                let account = |target: &Continuation| {
                                    self.configured.as_ref()
                                        .filter(|p| p.endpoint(target.tool).root == target.root)
                                        .map(|p| p.endpoint(target.tool).label.clone())
                                        .unwrap_or_else(|| tr("기본 CLI").to_owned())
                                };
                                if row.phase == Phase::Synced && !row.copies.is_empty() {
                                    let places: Vec<_> = targets
                                        .iter()
                                        .map(|t| format!("{} ({})", t.tool.display_name(), account(t)))
                                        .collect();
                                    ui.add(egui::Label::new(egui::RichText::new(kiln_common::trf!(
                                        "/resume으로 이어가기 · {}",
                                        places.join(", ")
                                    )).color(theme.text_dim)).wrap());
                                }
                                for branch in &row.branches {
                                    ui.push_id(&branch.id, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(egui::RichText::new(kiln_common::trf!("보관된 갈래 · 메시지 {}개", branch.events)).color(theme.text_dim));
                                            if ui
                                                .small_button(tr("복구"))
                                                .on_hover_text(tr("이 갈래를 별도 대화로 되살려 모든 에이전트에 다시 연결합니다."))
                                                .clicked()
                                            {
                                                actions.push(RowAction::Restore {
                                                    branch: branch.id.clone(),
                                                    title: format!("{} ({})", title, tr("복구됨")),
                                                });
                                            }
                                        });
                                    });
                                }
                                ui.collapsing(tr("대화 정보"), |ui| {
                                    ui.add(egui::Label::new(&row.source.cwd).wrap());
                                    if !row.detail.is_empty() {
                                        ui.add(egui::Label::new(&row.detail).wrap());
                                        if ui.small_button(tr("오류 정보 복사")).clicked() {
                                            ui.ctx().copy_text(format!("{}\n{}\n{}", row.source.tool.display_name(), row.source.id, row.detail));
                                        }
                                    }
                                    for target in &targets {
                                        ui.push_id(&target.id, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                if ui
                                                    .small_button(kiln_common::trf!("{} 대화 ID 복사", target.tool.display_name()))
                                                    .on_hover_text(kiln_common::trf!(
                                                        "/resume 목록에서 찾기 어려우면 이 ID로 직접 엽니다.\n{}\n{}",
                                                        target.id,
                                                        target.root.display()
                                                    ))
                                                    .clicked()
                                                {
                                                    ui.ctx().copy_text(target.id.clone());
                                                }
                                                ui.label(egui::RichText::new(account(target)).color(theme.text_dim))
                                                    .on_hover_text(target.root.display().to_string());
                                            });
                                        });
                                    }
                                });
                            });
                        }
                    });
            });
        actions
    }
}
