//! Bounded metadata discovery; never reads credentials or modifies source sessions.
use kiln_common::paths::Canonical;
use super::*;
use std::io::{BufRead, BufReader};

pub(super) struct Scope {
    pub workspaces: Vec<PathBuf>,
    pub limits: BTreeMap<PathBuf, usize>,
    /// Projected namespaces and read-only Antigravity data folders.
    pub(super) roots: Vec<(Tool, PathBuf)>,
}
impl Scope {
    pub fn new(pair: &Pair, workspaces: Vec<PathBuf>) -> Result<Self> {
        let mut roots = Vec::new();
        for tool in PROJECTED {
            let default = default_root(tool).context("Missing home directory")?;
            let connected = pair.endpoint(tool).context("No namespace")?.root.clone();
            for r in [default, connected] {
                private_dir(&r)?;
                let r = r.canonical()?;
                if !roots.contains(&(tool, r.clone())) {
                    roots.push((tool, r));
                }
            }
        }
        // Read only: the folder is never created or modified.
        if let Some(root) = default_root(Tool::Antigravity).and_then(|r| r.canonical().ok())
            && root.is_dir()
        {
            roots.push((Tool::Antigravity, root));
        }
        let mut workspaces: Vec<_> = workspaces
            .into_iter()
            .filter_map(|p| p.canonical().ok())
            .collect();
        workspaces.sort();
        workspaces.dedup();
        Ok(Self { workspaces, roots, limits: BTreeMap::new() })
    }
    pub fn contains(&self, source: &Source) -> bool {
        self.roots.contains(&(source.tool, source.root.clone())) && self.matches_cwd(&source.cwd)
    }
    pub(super) fn workspace(&self, cwd: &str) -> Option<&PathBuf> {
        let cwd = Path::new(cwd).canonical().ok()?;
        self.workspaces
            .iter()
            .filter(|w| cwd.starts_with(w))
            .max_by_key(|w| w.components().count())
    }
    fn matches_cwd(&self, cwd: &str) -> bool {
        self.workspace(cwd).is_some()
    }
}

#[derive(Serialize, Deserialize)]
struct Import {
    source: Source,
    fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visible_fingerprint: Option<String>,
}
fn import_path(config: &Path, source: &Source) -> PathBuf {
    config.join("conversation-sync/imports").join(format!(
        "{}.json",
        digest(source.path.to_string_lossy().as_bytes())
    ))
}
pub(super) fn remember_import(config: &Path, source: &Source, fingerprint: &str) -> Result<()> {
    remember_projection(config, source, fingerprint, None)
}
pub(super) fn remember_projection(
    config: &Path,
    source: &Source,
    fingerprint: &str,
    messages: Option<&[format::Message]>,
) -> Result<()> {
    let path = import_path(config, source);
    if !path.exists() {
        let visible_fingerprint = messages
            .map(|messages| {
                let visible: Vec<_> = messages
                    .iter()
                    .map(|m| format::Message {
                        role: m.role.clone(),
                        images: m.images.clone(),
                        text: if m.role == "assistant" {
                            portable::display(&m.text)
                        } else {
                            m.text.clone()
                        },
                    })
                    .collect();
                serde_json::to_vec(&visible).map(|bytes| digest(&bytes))
            })
            .transpose()?;
        publish(
            &path,
            &serde_json::to_vec(&Import {
                source: source.clone(),
                fingerprint: fingerprint.into(),
                visible_fingerprint,
            })?,
        )?;
    }
    Ok(())
}
pub(super) fn imported(config: &Path, source: &Source) -> Result<Option<String>> {
    let path = import_path(config, source);
    if !path.exists() {
        return Ok(None);
    }
    let imported: Import = serde_json::from_slice(&bounded(&path, 32 * 1024)?)?;
    ensure!(
        imported.source.path == source.path
            && imported.source.id == source.id
            && imported.source.root == source.root,
        "Imported conversation identity changed"
    );
    Ok(Some(imported.fingerprint))
}

fn external_import_baselines(
    config: &Path,
    root: &Path,
    scope: &Scope,
) -> Result<HashMap<String, String>> {
    let path = root.join("external_agent_session_imports.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    safe_path(root, &path)?;
    #[derive(Deserialize)]
    struct Record {
        source_path: PathBuf,
        imported_thread_id: String,
    }
    #[derive(Deserialize)]
    struct Ledger {
        records: Vec<Record>,
    }
    let ledger: Ledger = serde_json::from_slice(&bounded(&path, 8 * 1024 * 1024)?)?;
    let mut baselines = HashMap::new();
    for record in ledger.records {
        let path = config.join("conversation-sync/imports").join(format!(
            "{}.json",
            digest(record.source_path.to_string_lossy().as_bytes())
        ));
        if !path.exists() {
            continue;
        }
        let origin: Import = serde_json::from_slice(&bounded(&path, 32 * 1024)?)?;
        if origin.source.tool == Tool::Claude
            && origin.source.path == record.source_path
            && scope.contains(&origin.source)
        {
            baselines.insert(
                record.imported_thread_id,
                origin.visible_fingerprint.unwrap_or(origin.fingerprint),
            );
        }
    }
    Ok(baselines)
}

fn meaningful_prompt(content: &serde_json::Value) -> bool {
    if let Some(text) = content.as_str() {
        let text = text.trim_start();
        return !text.is_empty()
            && !text.starts_with("<local-command-")
            && !text.starts_with("<command-name>");
    }
    content.as_array().is_some_and(|blocks| {
        blocks.iter().any(|b| {
            matches!(
                b["type"].as_str(),
                Some("image" | "input_image" | "local_image")
            ) || meaningful_prompt(&b["text"])
        })
    })
}

/// Keep directory traversal bounded and do not follow links or recurse into subagent folders.
fn files(
    root: &Path,
    dir: &Path,
    depth: usize,
    budget: &mut usize,
    out: &mut Vec<PathBuf>,
    gate: &Gate,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    safe_path(root, dir)?;
    for entry in fs::read_dir(dir)? {
        if !gate.load(Ordering::Acquire) {
            break;
        }
        ensure!(
            *budget > 0,
            "Session discovery limit reached (100000 directory entries)"
        );
        *budget -= 1;
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ty.is_dir() && depth > 0 {
            files(root, &path, depth - 1, budget, out, gate)?;
        } else if ty.is_file() && (path.extension().is_some_and(|e| e == "jsonl")
            || storage::compressed(&path) && !path.with_extension("").exists()) {
            out.push(path);
        }
    }
    Ok(())
}

/// The full transcripts of agy conversations that have a conversation database:
/// `brain/<id>/.system_generated/logs/transcript_full.jsonl`.
fn antigravity_files(root: &Path, budget: &mut usize, out: &mut Vec<PathBuf>, gate: &Gate) -> Result<()> {
    let brain = root.join("brain");
    if !brain.is_dir() {
        return Ok(());
    }
    safe_path(root, &brain)?;
    for entry in fs::read_dir(&brain)? {
        if !gate.load(Ordering::Acquire) {
            break;
        }
        ensure!(*budget > 0, "Session discovery limit reached (100000 directory entries)");
        *budget -= 1;
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() || !uuid(&id) {
            continue;
        }
        let transcript = entry.path().join(".system_generated").join("logs").join("transcript_full.jsonl");
        let database = root.join("conversations").join(format!("{id}.db"));
        if fs::symlink_metadata(&transcript).is_ok_and(|m| m.is_file()) && database.is_file() {
            out.push(transcript);
        }
    }
    Ok(())
}

/// Workspace folder and parent conversation of each agy conversation, from its summary index.
pub(super) type Summaries = HashMap<String, (Option<String>, bool)>;
/// Reads a private copy of agy's summary index and its write-ahead log.
pub(super) fn antigravity_summaries(root: &Path) -> Result<Summaries> {
    use rusqlite::{Connection, OpenFlags};
    let path = root.join("conversation_summaries.db");
    let mut summaries = HashMap::new();
    if !path.is_file() {
        return Ok(summaries);
    }
    let copy = tempfile::Builder::new().prefix("kiln-agy-summaries-").tempdir()?;
    for suffix in ["", "-wal"] {
        let source = root.join(format!("conversation_summaries.db{suffix}"));
        safe_path(root, &source)?;
        if fs::symlink_metadata(&source).is_ok_and(|m| m.is_file()) {
            fs::write(copy.path().join(format!("summaries.db{suffix}")), bounded(&source, 64 * 1024 * 1024)?)?;
        }
    }
    let db = Connection::open_with_flags(copy.path().join("summaries.db"), OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let mut query = db.prepare("SELECT conversation_id, workspace_uris, parent_conversation_id FROM conversation_summaries")?;
    let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
    for row in rows {
        let (id, uris, parent) = row?;
        let cwd = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|uris| uris.into_iter().next())
            .and_then(|uri| url::Url::parse(&uri).ok()?.to_file_path().ok())
            .map(|path| path.to_string_lossy().into_owned());
        summaries.insert(id, (cwd, !parent.is_empty()));
    }
    Ok(summaries)
}

/// The conversation of an agy transcript: its workspace folder from the summary index, when it
/// is not a subagent and its first records hold a prompt.
fn antigravity_metadata(root: &Path, path: &Path, summaries: &Summaries) -> Result<Option<Source>> {
    safe_path(root, path)?;
    let Some(id) = path.ancestors().nth(3).and_then(|dir| dir.file_name()).map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(None);
    };
    let Some((Some(cwd), false)) = summaries.get(&id) else {
        return Ok(None);
    };
    let mut input = BufReader::new(storage::open(root, path)?.0.take(2 * 1024 * 1024));
    let mut line = Vec::new();
    for _ in 0..64 {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            break;
        }
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line) else { continue };
        if row["type"] == "USER_INPUT" && row["content"].as_str().is_some_and(|c| !format::antigravity_prompt(c).trim().is_empty()) {
            return Ok(Some(Source { tool: Tool::Antigravity, root: root.into(), path: path.into(), cwd: cwd.clone(), id }));
        }
    }
    Ok(None)
}

/// Read only bounded leading rows until the native session identity appears.
fn metadata(tool: Tool, root: &Path, path: &Path) -> Result<Option<Source>> {
    if tool == Tool::Claude && storage::compressed(path) { return Ok(None); }
    safe_path(root, path)?;
    // Native CLIs create metadata-only session files before a first prompt. These
    // are not conversations and must not consume the initial twenty slots.
    if fs::metadata(path)?.len() < 2 * 1024 * 1024 {
        let mut input = BufReader::new(storage::open(root, path)?.0.take(2 * 1024 * 1024));
        let mut line = Vec::new();
        let mut has_prompt = false;
        let mut saw_user = false;
        let mut head_bytes = 0;
        for index in 0..64 {
            line.clear();
            if input.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            head_bytes += line.len();
            if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line) {
                let content =
                    if tool == Tool::Claude && row["type"] == "user" && row["isMeta"] != true {
                        Some(&row["message"]["content"])
                    } else if tool == Tool::Codex
                        && row["type"] == "response_item"
                        && row["payload"]["role"] == "user"
                    {
                        Some(&row["payload"]["content"])
                    } else if tool == Tool::Codex
                        && row["type"] == "event_msg"
                        && row["payload"]["type"] == "user_message"
                    {
                        Some(&row["payload"]["message"])
                    } else if tool == Tool::Codex
                        && row["type"] == "event_msg"
                        && row["payload"]["item"]["type"] == "UserMessage"
                    {
                        Some(&row["payload"]["item"]["content"])
                    } else {
                        None
                    };
                saw_user |= content.is_some();
                has_prompt = content.is_some_and(meaningful_prompt)
                    || (saw_user
                        && (row["type"] == "assistant" || row["payload"]["role"] == "assistant"));
            }
            if has_prompt || index == 63 {
                has_prompt = true;
                break;
            }
        }
        if !has_prompt && head_bytes < 2 * 1024 * 1024 {
            return Ok(None);
        }
    }
    let mut reader = BufReader::new(storage::open(root, path)?.0.take(2 * 1024 * 1024));
    let mut line = Vec::new();
    for _ in 0..64 {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            return Ok(None);
        }
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line) else {
            continue;
        };
        let meta = match tool {
            Tool::Codex if row["type"] == "session_meta" => {
                if !row["payload"]["parent_thread_id"].is_null()
                    || row["payload"]["source"].is_object()
                {
                    return Ok(None);
                }
                &row["payload"]
            }
            Tool::Claude if matches!(row["type"].as_str(), Some("user" | "assistant")) => {
                if row["isSidechain"] == true {
                    return Ok(None);
                }
                &row
            }
            _ => continue,
        };
        let id = meta[if tool == Tool::Claude {
            "sessionId"
        } else {
            "id"
        }]
        .as_str();
        let Some((id, cwd)) = id.zip(meta["cwd"].as_str()) else {
            return Ok(None);
        };
        if !uuid(id)
            || !path
                .file_name()
                .is_some_and(|p| p.to_string_lossy().contains(id))
        {
            return Ok(None);
        }
        return Ok(Some(Source {
            tool,
            root: root.into(),
            path: storage::logical(tool, path),
            cwd: cwd.into(),
            id: id.into(),
        }));
    }
    Ok(None)
}

pub(super) type MetadataCache = HashMap<PathBuf, (u64, SystemTime, Option<Source>)>;

pub(super) fn find(
    config: &Path,
    scope: &Scope,
    gate: &Gate,
    cache: &mut MetadataCache,
) -> Result<Vec<Source>> {
    if scope.workspaces.is_empty() {
        return Ok(Vec::new());
    }
    if cache.len() > 100_000 {
        cache.clear();
    }
    let mut selected = BTreeMap::new();
    let mut budget = 100_000;
    for (tool, root) in &scope.roots {
        let external = if *tool == Tool::Codex {
            external_import_baselines(config, root, scope)?
        } else {
            HashMap::new()
        };
        let mut paths = Vec::new();
        let mut summaries = None;
        if *tool == Tool::Antigravity {
            // An unreadable agy folder is skipped; the other CLIs still synchronize.
            if let Err(error) = antigravity_files(root, &mut budget, &mut paths, gate) {
                log::warn!("conversation sync: Antigravity conversations skipped: {error:#}");
                continue;
            }
        } else {
            files(
                root,
                &root.join(if *tool == Tool::Claude { "projects" } else { "sessions" }),
                if *tool == Tool::Claude { 1 } else { 3 },
                &mut budget,
                &mut paths,
                gate,
            )?;
        }
        for path in paths {
            if !gate.load(Ordering::Acquire) {
                break;
            }
            let meta = fs::metadata(&path)?;
            let modified = meta.modified()?;
            let source = if let Some((size, stamp, source)) = cache.get(&path)
                && *size == meta.len()
                && *stamp == modified
            {
                source.clone()
            } else {
                let source = if *tool == Tool::Antigravity {
                    let summaries = summaries.get_or_insert_with(|| {
                        antigravity_summaries(root)
                            .inspect_err(|error| log::warn!("conversation sync: Antigravity summary index unreadable: {error:#}"))
                            .ok()
                    });
                    // Without the summary index nothing is decided or cached.
                    let Some(summaries) = summaries else { continue };
                    match antigravity_metadata(root, &path, summaries) {
                        Ok(source) => source,
                        Err(error) => {
                            log::warn!("conversation sync: {} skipped: {error:#}", path.display());
                            continue;
                        }
                    }
                } else {
                    metadata(*tool, root, &path)?
                };
                cache.insert(path.clone(), (meta.len(), modified, source.clone()));
                source
            };
            let Some(source) = source else {
                continue;
            };
            let Some(_workspace) = scope.workspace(&source.cwd) else {
                continue;
            };
            if let Some(baseline) = external.get(&source.id) {
                remember_import(config, &source, baseline)?;
            }
            let is_import = imported(config, &source)?.is_some();
            if is_import {
                selected.insert(source.path.clone(), source);
                continue;
            }
            selected.insert(source.path.clone(), source);
        }
    }
    ensure!(selected.len() <= 100_000, "Session discovery limit reached");
    Ok(selected.into_values().collect())
}
