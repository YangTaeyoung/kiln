//! Opt-in native continuations. Only lifecycle-confirmed transcripts are watched.
mod format;
#[cfg(test)]
mod tests;

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
const MAX_SOURCES: usize = 4096;

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
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
    let path = PathBuf::from(transcript);
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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Continuation {
    tool: Tool,
    id: String,
    cwd: String,
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
    source: Source,
    title: String,
    phase: Phase,
    detail: String,
    target: Option<Continuation>,
}
impl Conversation {
    fn from_entry(entry: &Entry) -> Self {
        Self {
            source: entry.source.clone(),
            title: entry.title.clone(),
            phase: if entry.copied_to.is_some() {
                Phase::Synced
            } else {
                Phase::Queued
            },
            detail: String::new(),
            target: entry.copied_to.clone(),
        }
    }
}
#[derive(Clone, Default)]
struct Status {
    watched: usize,
    published: usize,
    errors: Vec<String>,
    recent: Vec<Continuation>,
    checked: usize,
    rows: BTreeMap<String, Conversation>,
}

fn read_messages(source: &Source) -> Result<Vec<format::Message>> {
    safe_path(&source.root, &source.path)?;
    let bytes = bounded(&source.path, LIMIT)?;
    ensure!(
        bytes.ends_with(b"\n"),
        "Transcript write is incomplete; retrying"
    );
    let rows: Vec<serde_json::Value> = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(serde_json::from_slice)
        .collect::<std::result::Result<_, _>>()?;
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
                meta["payload"]["cli_version"] == "0.162.1",
                "Unsupported Codex transcript version (supported: 0.162.1)"
            );
        }
        Tool::Claude => {
            for row in &rows {
                if matches!(row["type"].as_str(), Some("user" | "assistant")) {
                    ensure!(
                        row["sessionId"] == source.id && row["cwd"] == source.cwd,
                        "Claude transcript identity changed"
                    );
                    ensure!(
                        row["version"] == "2.1.296",
                        "Unsupported Claude transcript version (supported: 2.1.296)"
                    );
                }
            }
        }
    }
    format::parse(source.tool, &rows)
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
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
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
type Cache = HashMap<PathBuf, (u64, SystemTime, String)>;
fn scan(
    config: &Path,
    pair: &Pair,
    gate: &Gate,
    cache: &mut Cache,
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
    let state_path = dir.join(format!("{}.json", digest(&serde_json::to_vec(pair)?)));
    let mut state: State = if state_path.exists() {
        serde_json::from_slice(&bounded(&state_path, 8 * 1024 * 1024)?)?
    } else {
        State::default()
    };
    let tracked = dir.join("tracked");
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
            if source.root != pair.endpoint(source.tool).root {
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
                });
        }
    }
    ensure!(
        state.entries.len() <= MAX_SOURCES,
        "Sync source limit reached (4096); reconnect with a fresh namespace"
    );
    let mut status = Status {
        watched: state.entries.len(),
        recent: state.recent.clone(),
        rows: state
            .entries
            .iter()
            .filter(|(_, e)| !e.imported || e.copied_to.is_some())
            .map(|(k, e)| (k.clone(), Conversation::from_entry(e)))
            .collect(),
        ..Default::default()
    };
    notify(&status);
    for (key, entry) in state.entries.clone() {
        if !gate.load(Ordering::Acquire) {
            break;
        }
        status.checked += 1;
        if !entry.source.path.exists() {
            if let Some(row) = status.rows.get_mut(&key) {
                row.phase = Phase::Waiting;
                row.detail = "Transcript not available yet".into();
            }
            continue;
        }
        let result = (|| -> Result<()> {
            safe_path(&entry.source.root, &entry.source.path)?;
            let meta = fs::metadata(&entry.source.path)?;
            let stamp = (meta.len(), meta.modified()?, entry.seen.clone());
            if cache.get(&entry.source.path) == Some(&stamp) {
                return Ok(());
            }
            if let Some(row) = status.rows.get_mut(&key) {
                row.phase = Phase::Reading;
            }
            notify(&status);
            let messages = read_messages(&entry.source)?;
            if messages.is_empty() || !messages.iter().any(|m| m.role == "user") {
                if let Some(row) = status.rows.get_mut(&key) {
                    row.phase = Phase::Waiting;
                }
                return Ok(());
            }
            let fingerprint = digest(&serde_json::to_vec(&messages)?);
            if fingerprint == entry.seen {
                if let Some(row) = status.rows.get_mut(&key) {
                    row.phase = Phase::Synced;
                }
                cache.insert(entry.source.path.clone(), stamp);
                return Ok(());
            }
            let title = messages
                .iter()
                .find(|m| m.role == "user")
                .map(|m| {
                    m.text
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(96)
                        .collect::<String>()
                })
                .unwrap_or_default();
            let row = status
                .rows
                .entry(key.clone())
                .or_insert_with(|| Conversation::from_entry(&entry));
            row.title = title.clone();
            row.phase = Phase::Copying;
            notify(&status);
            // Reuse this exact plan after a crash, even across a UTC date boundary.
            let id = stable_id(&format!("{}:{key}:{fingerprint}", state_path.display()));
            let plan_path = dir.join(format!("pending-{id}.json"));
            #[derive(Serialize, Deserialize)]
            struct Plan {
                source: Source,
                timestamp: String,
            }
            let plan: Plan = if plan_path.exists() {
                serde_json::from_slice(&bounded(&plan_path, 16 * 1024)?)?
            } else {
                let now = chrono::Utc::now();
                let plan = Plan {
                    source: destination(pair, &entry.source, &id, now)?,
                    timestamp: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                };
                publish(&plan_path, &serde_json::to_vec(&plan)?)?;
                plan
            };
            ensure!(
                plan.source.root == pair.endpoint(plan.source.tool).root,
                "Destination namespace changed"
            );
            safe_path(&plan.source.root, &plan.source.path)?;
            let bytes = format::render(
                plan.source.tool,
                &id,
                &entry.source.cwd,
                &plan.timestamp,
                &messages,
            )?;
            ensure!(
                bytes.len() as u64 <= LIMIT,
                "Converted conversation exceeds the 64 MiB sync limit"
            );
            if !gate.load(Ordering::Acquire) {
                return Ok(());
            }
            if plan.source.path.exists() {
                // A CLI may already have resumed a successfully published copy before a crash.
                ensure!(
                    bounded(&plan.source.path, LIMIT)?.starts_with(&bytes),
                    "Continuation collision; existing conversation was preserved"
                );
            } else {
                publish(&plan.source.path, &bytes)?;
            }
            state.entries.insert(
                plan.source.path.to_string_lossy().into_owned(),
                Entry {
                    source: plan.source.clone(),
                    seen: fingerprint.clone(),
                    imported: true,
                    title: title.clone(),
                    copied_to: None,
                },
            );
            let continuation = Continuation {
                tool: plan.source.tool,
                id: id.clone(),
                cwd: entry.source.cwd.clone(),
            };
            let saved = state.entries.get_mut(&key).unwrap();
            saved.seen = fingerprint.clone();
            saved.title = title;
            saved.copied_to = Some(continuation.clone());
            state.recent.insert(
                0,
                Continuation {
                    tool: plan.source.tool,
                    id,
                    cwd: entry.source.cwd.clone(),
                },
            );
            state.recent.truncate(8);
            replace(&state_path, &serde_json::to_vec(&state)?)?;
            cache.insert(entry.source.path.clone(), (stamp.0, stamp.1, fingerprint));
            let row = status.rows.get_mut(&key).unwrap();
            row.phase = Phase::Synced;
            row.target = Some(continuation);
            status.published += 1;
            let _ = fs::remove_file(plan_path);
            Ok(())
        })();
        if let Err(error) = result {
            let row = status
                .rows
                .entry(key.clone())
                .or_insert_with(|| Conversation::from_entry(&entry));
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
    }
    status.recent = state.recent;
    notify(&status);
    Ok(status)
}

enum Event {
    Progress(Status),
    Connected(Result<Pair, String>),
    Scanned(Result<Status, String>),
}
pub struct Manager {
    tx: mpsc::SyncSender<Event>,
    rx: mpsc::Receiver<Event>,
    busy: bool,
    connecting: bool,
    gate: Gate,
    next: Instant,
    status: Status,
    error: Option<String>,
    configured: Option<Pair>,
    cache: Arc<Mutex<Cache>>,
}
impl Default for Manager {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        Self {
            tx,
            rx,
            busy: false,
            connecting: false,
            gate: Arc::new(AtomicBool::new(false)),
            next: Instant::now(),
            status: Status::default(),
            error: None,
            configured: None,
            cache: Arc::new(Mutex::new(Cache::new())),
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}
impl Manager {
    pub fn tick(&mut self, settings: &mut super::state::Settings, ctx: &egui::Context) {
        // Poll quickly only while working; all filesystem/account operations stay off the UI thread.
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Progress(status) => {
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
                Event::Scanned(result) => {
                    self.busy = false;
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
        let enabled =
            settings.conversation_sync_enabled && settings.conversation_sync_pair.is_some();
        self.gate.store(enabled, Ordering::Release);
        if !enabled {
            return;
        }
        if self.configured != settings.conversation_sync_pair {
            self.status = Status::default();
            self.configured = settings.conversation_sync_pair.clone();
            self.cache = Arc::new(Mutex::new(Cache::new()));
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
        self.next = Instant::now() + Duration::from_secs(3);
        let pair = settings.conversation_sync_pair.clone().unwrap();
        let gate = self.gate.clone();
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        std::thread::spawn(move || {
            let last_progress = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
            let result = scan(
                &kiln_common::paths::config_dir(),
                &pair,
                &gate,
                &mut cache.lock().unwrap(),
                |status| {
                    if last_progress.get().elapsed() >= Duration::from_millis(100) {
                        last_progress.set(Instant::now());
                        let _ = tx.try_send(Event::Progress(status.clone()));
                    }
                },
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(Event::Scanned(result));
        });
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        s: &mut super::state::Settings,
        accounts: &AccountManager,
    ) {
        use kiln_common::{Theme, fonts, i18n::tr};
        let theme = Theme::current();
        ui.add_space(16.0);
        kiln_common::widgets::divider(ui);
        ui.add_space(12.0);
        ui.label(egui::RichText::new(tr("대화 이어가기 · 실험 기능")).font(fonts::semibold(16.0)));
        ui.add(egui::Label::new(tr("Kiln이 열려 있는 동안 완료된 대화를 상대 CLI의 /resume에 복사합니다. 원문 대화와 텍스트 도구 기록을 전달합니다.")).wrap());
        if let Some(pair) = &s.conversation_sync_pair {
            ui.label(format!(
                "Claude Code: {} ↔ Codex: {}",
                pair.claude.label, pair.codex.label
            ));
            if ui
                .checkbox(&mut s.conversation_sync_enabled, tr("자동으로 대화 동기화"))
                .changed()
            {
                self.gate
                    .store(s.conversation_sync_enabled, Ordering::Release);
            }
        }
        if ui
            .add_enabled(!self.busy, egui::Button::new(tr("현재 두 계정 연결")))
            .clicked()
        {
            self.gate.store(false, Ordering::Release);
            s.conversation_sync_enabled = false;
            self.busy = true;
            self.connecting = true;
            let accounts = accounts.clone();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Connected(
                    Pair::connect(&accounts).map_err(|e| e.to_string()),
                ));
            });
        }
        if self.busy {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.add(egui::Label::new(egui::RichText::new(tr("Kiln에서 연 CLI 대화가 대상입니다. 계정을 바꿔도 연결은 유지됩니다. 같은 프로젝트에서 /resume으로 최신 사본을 선택하세요.")).color(theme.text_dim)).wrap());
        ui.add(egui::Label::new(egui::RichText::new(tr("응답마다 새 사본이 생깁니다. 열린 세션은 자동으로 바뀌지 않습니다. 이미지·첨부 파일은 지원하지 않습니다. Codex 0.162.1 · Claude Code 2.1.296 지원.")).color(theme.text_dim)).wrap());
        self.progress_ui(ui, s.conversation_sync_enabled);
    }
    fn progress_ui(&self, ui: &mut egui::Ui, enabled: bool) {
        use kiln_common::{
            Theme, fonts,
            i18n::tr,
            icons::{self, Icon},
        };
        let theme = Theme::current();
        let failed = self
            .status
            .rows
            .values()
            .filter(|r| r.phase == Phase::Failed)
            .count();
        let waiting = self
            .status
            .rows
            .values()
            .filter(|r| matches!(r.phase, Phase::Waiting | Phase::Queued))
            .count();
        let copying = self
            .status
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
        } else if copying {
            (Icon::Refresh, theme.blue, tr("대화 동기화 중"))
        } else if self.status.rows.is_empty() {
            (
                Icon::Plug,
                theme.text_dim,
                tr("연결됨 · 대화를 기다리는 중"),
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
                if enabled && self.busy && self.status.watched > 0 {
                    let fraction = self.status.checked as f32 / self.status.watched as f32;
                    ui.add(egui::ProgressBar::new(fraction).text(kiln_common::trf!(
                        "기록 확인 {}/{} · 새 사본 {}개",
                        self.status.checked,
                        self.status.watched,
                        self.status.published
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
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for (key, row) in &self.status.rows {
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
                                ui.label(
                                    egui::RichText::new(
                                        Path::new(&row.source.cwd)
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy(),
                                    )
                                    .color(theme.text_dim),
                                )
                                .on_hover_text(&row.source.cwd);
                                if !row.detail.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&row.detail).color(theme.orange),
                                        )
                                        .wrap(),
                                    );
                                }
                                if let Some(target) = &row.target {
                                    if ui
                                        .small_button(tr("이어갈 대화 ID 복사"))
                                        .on_hover_text(&target.id)
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(target.id.clone());
                                    }
                                }
                            });
                        }
                    });
            });
    }
}
