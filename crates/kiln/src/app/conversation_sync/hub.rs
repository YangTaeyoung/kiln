//! Canonical conversations shared by every agent namespace, and their native projections.
use super::*;
use std::io::{BufRead, BufReader, Seek};

/// One native session: a tool, its account root and the session ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct NativeRef {
    pub tool: Tool,
    pub root: PathBuf,
    pub id: String,
}
impl NativeRef {
    pub fn of(source: &Source) -> Self {
        Self { tool: source.tool, root: source.root.clone(), id: source.id.clone() }
    }
}

/// The native session holding a conversation in one namespace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Projection {
    pub source: Source,
    /// Number of canonical events this native history contains.
    pub synced: usize,
    /// Length and modification time when the history was last read or written.
    pub stamp: Option<(u64, SystemTime)>,
    /// The agent's own original transcript.
    pub origin: bool,
    /// The native history was deleted or archived outside Kiln; the namespace is not written again.
    #[serde(default)]
    pub removed: bool,
    /// Title last pinned to or read from this native session.
    #[serde(default)]
    pub title: String,
}

/// An archived continuation that lost to the canonical history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Branch {
    pub id: String,
    /// Number of canonical events shared before the branch.
    pub base: usize,
    pub events: usize,
    /// Archived native transcript, when the branch was a projection.
    pub archived: Option<PathBuf>,
    pub native: Option<NativeRef>,
    pub at: SystemTime,
    #[serde(default)]
    pub restored: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    pub title: String,
    pub cwd: String,
    pub created: SystemTime,
    /// Time the canonical history last changed.
    pub updated: SystemTime,
    pub projections: Vec<Projection>,
    /// Earlier native sessions of this conversation; opening one redirects to the projection.
    #[serde(default)]
    pub aliases: Vec<NativeRef>,
    #[serde(default)]
    pub branches: Vec<Branch>,
    /// Number of events and bytes in the events file.
    #[serde(default)]
    pub events: usize,
    #[serde(default)]
    pub events_bytes: u64,
}
impl Record {
    pub fn new(title: &str, cwd: &str) -> Self {
        let now = SystemTime::now();
        Self {
            title: title.into(),
            cwd: cwd.into(),
            created: now,
            updated: now,
            projections: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            events: 0,
            events_bytes: 0,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct HubState {
    pub conversations: BTreeMap<String, Record>,
    /// Legacy state files whose copies were migrated into the hub.
    #[serde(default)]
    pub migrated: std::collections::BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Event {
    seq: usize,
    #[serde(flatten)]
    message: format::Message,
}

pub(super) struct Hub {
    dir: PathBuf,
    pub state: HubState,
}

/// Conversation identity derived from the first native session.
pub(super) fn conversation_id(first: &NativeRef) -> String {
    stable_id(&format!("hub:{}:{}:{}", first.tool.key(), first.root.display(), first.id))
}

fn encode(events: &[format::Message], first: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for (i, message) in events.iter().enumerate() {
        serde_json::to_writer(&mut bytes, &Event { seq: first + i, message: message.clone() })?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
fn decode(path: &Path, count: usize) -> Result<Vec<format::Message>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut events = Vec::with_capacity(count);
    let mut line = Vec::new();
    while events.len() < count {
        line.clear();
        ensure!(reader.read_until(b'\n', &mut line)? > 0 && line.ends_with(b"\n"), "Conversation history is incomplete");
        let event: Event = serde_json::from_slice(&line)?;
        ensure!(event.seq == events.len(), "Conversation history is out of order");
        events.push(event.message);
    }
    Ok(events)
}

impl Hub {
    pub fn open(config: &Path) -> Result<Self> {
        let dir = config.join("conversation-sync/hub");
        private_dir(&dir)?;
        let path = dir.join("state.json");
        let state = if path.exists() {
            serde_json::from_slice(&bounded(&path, 256 * 1024 * 1024)?)?
        } else {
            HubState::default()
        };
        Ok(Self { dir, state })
    }
    pub fn save(&self) -> Result<()> {
        replace_changed(&self.dir.join("state.json"), &serde_json::to_vec(&self.state)?)
    }
    fn events_path(&self, cid: &str) -> PathBuf {
        self.dir.join("events").join(format!("{cid}.jsonl"))
    }
    fn branch_path(&self, id: &str) -> PathBuf {
        self.dir.join("branches").join(format!("{id}.jsonl"))
    }
    fn record(&mut self, cid: &str) -> Result<&mut Record> {
        self.state.conversations.get_mut(cid).context("Unknown conversation")
    }
    pub fn events(&self, cid: &str) -> Result<Vec<format::Message>> {
        let record = self.state.conversations.get(cid).context("Unknown conversation")?;
        decode(&self.events_path(cid), record.events)
    }
    /// Appends after the recorded end; bytes left by an interrupted append are discarded.
    pub fn append(&mut self, cid: &str, messages: &[format::Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let path = self.events_path(cid);
        private_dir(path.parent().unwrap())?;
        let (count, len) = {
            let record = self.record(cid)?;
            (record.events, record.events_bytes)
        };
        let bytes = encode(messages, count)?;
        let mut file = fs::OpenOptions::new().create(true).write(true).truncate(false).open(&path)?;
        file.set_len(len)?;
        file.seek(std::io::SeekFrom::End(0))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let record = self.record(cid)?;
        record.events += messages.len();
        record.events_bytes = len + bytes.len() as u64;
        record.updated = SystemTime::now();
        Ok(())
    }
    /// Replaces the history after `base` with `tail` and returns the replaced events.
    pub fn rewrite(&mut self, cid: &str, base: usize, tail: &[format::Message]) -> Result<Vec<format::Message>> {
        let mut events = self.events(cid)?;
        ensure!(base <= events.len(), "Conversation history is shorter than the rewrite base");
        let replaced = events.split_off(base);
        events.extend_from_slice(tail);
        let bytes = encode(&events, 0)?;
        let path = self.events_path(cid);
        private_dir(path.parent().unwrap())?;
        replace(&path, &bytes)?;
        let record = self.record(cid)?;
        record.events = events.len();
        record.events_bytes = bytes.len() as u64;
        record.updated = SystemTime::now();
        Ok(replaced)
    }
    /// Stores `tail` (the events after `base`) as an archived branch of the conversation.
    pub fn save_branch(
        &mut self,
        cid: &str,
        base: usize,
        tail: &[format::Message],
        native: Option<NativeRef>,
        archived: Option<PathBuf>,
    ) -> Result<String> {
        let bytes = encode(tail, 0)?;
        let id = stable_id(&format!("{cid}:branch:{base}:{}", digest(&bytes)));
        let path = self.branch_path(&id);
        if !path.exists() {
            publish(&path, &bytes)?;
        }
        let record = self.record(cid)?;
        if let Some(branch) = record.branches.iter_mut().find(|b| b.id == id) {
            if branch.archived.is_none() {
                branch.archived = archived;
                branch.native = native;
            }
        } else {
            record.branches.push(Branch {
                id: id.clone(),
                base,
                events: tail.len(),
                archived,
                native,
                at: SystemTime::now(),
                restored: false,
            });
        }
        Ok(id)
    }
    #[cfg(test)]
    pub fn branch_events(&self, id: &str) -> Result<Vec<format::Message>> {
        let count = self
            .state
            .conversations
            .values()
            .flat_map(|r| &r.branches)
            .find(|b| b.id == id)
            .context("Unknown branch")?
            .events;
        decode(&self.branch_path(id), count)
    }
    /// The conversation whose projection or alias is this native session.
    #[cfg(test)]
    pub fn find(&self, native: &NativeRef) -> Option<&str> {
        self.state.conversations.iter().find_map(|(cid, record)| {
            (record.projections.iter().any(|p| NativeRef::of(&p.source) == *native)
                || record.aliases.contains(native))
            .then_some(cid.as_str())
        })
    }
}

/// How a native history relates to the canonical history it last received.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Unchanged,
    /// The native history extends the canonical one to this many events.
    Append(usize),
    /// The native history lacks later canonical events and only needs them appended.
    Behind,
    /// The native side is newer: canonical events after `base` become a branch.
    NativeWins { base: usize },
    /// The canonical side is newer or equal: native events after `base` become a branch.
    CanonicalWins { base: usize },
}

/// Compares a projection's native history with the canonical history; `synced` is the
/// number of canonical events the projection last held and `native_newer` whether its
/// file changed after the canonical history did.
/// Whether two messages show the same content; a native rewrite that keeps only the
/// displayed form of a recorded tool message still matches its original.
pub(super) fn same(a: &format::Message, b: &format::Message) -> bool {
    a == b
        || (a.role == b.role
            && a.images == b.images
            && a.role == "assistant"
            && portable::display(&a.text) == portable::display(&b.text))
}
/// Number of leading messages the two histories share.
pub(super) fn shared(a: &[format::Message], b: &[format::Message]) -> usize {
    a.iter().zip(b).take_while(|(a, b)| same(a, b)).count()
}

pub(super) fn reconcile(
    canonical: &[format::Message],
    synced: usize,
    native: &[format::Message],
    native_newer: bool,
) -> Outcome {
    let base = shared(canonical, native);
    if base == native.len() {
        return if native.len() == canonical.len() {
            Outcome::Unchanged
        } else if native_newer && native.len() < synced {
            Outcome::NativeWins { base }
        } else {
            Outcome::Behind
        };
    }
    if base == canonical.len() {
        return Outcome::Append(native.len());
    }
    if native_newer {
        Outcome::NativeWins { base }
    } else {
        Outcome::CanonicalWins { base }
    }
}
