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
    /// Parser position after the last completed turn of the native history.
    #[serde(default)]
    pub checkpoint: Option<super::checkpoint::Checkpoint>,
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
    /// File holding the branch events when it differs from the branch ID.
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Record {
    pub title: String,
    pub cwd: String,
    pub created: SystemTime,
    /// Modification time of the native history whose change the canonical history last took.
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
    /// Events file generation; each rewrite writes the next one.
    #[serde(default)]
    pub generation: u64,
    /// Native sessions of this conversation that still have to leave their resume list.
    #[serde(default)]
    pub pending: Vec<Source>,
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
            generation: 0,
            pending: Vec::new(),
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
    /// Files replaced since the last save; removed once the state no longer refers to them.
    obsolete: Vec<PathBuf>,
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
        Ok(Self { dir, state, obsolete: Vec::new() })
    }
    pub fn save(&mut self) -> Result<()> {
        replace_changed(&self.dir.join("state.json"), &serde_json::to_vec(&self.state)?)?;
        for path in self.obsolete.drain(..) {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
    fn events_file(&self, cid: &str, generation: u64) -> PathBuf {
        let name = if generation == 0 { format!("{cid}.jsonl") } else { format!("{cid}.{generation}.jsonl") };
        self.dir.join("events").join(name)
    }
    fn events_path(&self, cid: &str) -> PathBuf {
        self.events_file(cid, self.state.conversations.get(cid).map_or(0, |r| r.generation))
    }
    fn branch_path(&self, id: &str) -> PathBuf {
        self.dir.join("branches").join(format!("{id}.jsonl"))
    }
    fn branch_file(&self, branch: &Branch) -> PathBuf {
        match &branch.file {
            Some(name) => self.dir.join("branches").join(name),
            None => self.branch_path(&branch.id),
        }
    }
    fn record(&mut self, cid: &str) -> Result<&mut Record> {
        self.state.conversations.get_mut(cid).context("Unknown conversation")
    }
    pub fn events(&self, cid: &str) -> Result<Vec<format::Message>> {
        let record = self.state.conversations.get(cid).context("Unknown conversation")?;
        decode(&self.events_path(cid), record.events)
    }
    /// Events from index `from` to the end, decoding only those events.
    pub fn events_from(&self, cid: &str, from: usize) -> Result<Vec<format::Message>> {
        let record = self.state.conversations.get(cid).context("Unknown conversation")?;
        if from >= record.events {
            return Ok(Vec::new());
        }
        let mut reader = BufReader::new(fs::File::open(self.events_path(cid))?);
        let mut line = Vec::new();
        for _ in 0..from {
            line.clear();
            ensure!(reader.read_until(b'\n', &mut line)? > 0, "Conversation history is incomplete");
        }
        let mut events = Vec::with_capacity(record.events - from);
        while events.len() < record.events - from {
            line.clear();
            ensure!(reader.read_until(b'\n', &mut line)? > 0 && line.ends_with(b"\n"), "Conversation history is incomplete");
            let event: Event = serde_json::from_slice(&line)?;
            ensure!(event.seq == from + events.len(), "Conversation history is out of order");
            events.push(event.message);
        }
        Ok(events)
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
        Ok(())
    }
    /// Replaces the history after `base` with `tail` and returns the replaced events. The new
    /// history goes to the next events file generation; branches that continued after `base`
    /// move to files that also hold the replaced events they share.
    pub fn rewrite(&mut self, cid: &str, base: usize, tail: &[format::Message]) -> Result<Vec<format::Message>> {
        let mut events = self.events(cid)?;
        ensure!(base <= events.len(), "Conversation history is shorter than the rewrite base");
        let replaced = events.split_off(base);
        events.extend_from_slice(tail);
        let record = self.state.conversations.get(cid).context("Unknown conversation")?;
        let mut moved = Vec::new();
        for (j, branch) in record.branches.iter().enumerate().filter(|(_, b)| !b.restored && b.base > base) {
            let shared = branch.base - base;
            ensure!(shared <= replaced.len(), "Conversation history is shorter than a branch base");
            let mut kept = replaced[..shared].to_vec();
            kept.extend(decode(&self.branch_file(branch), branch.events)?);
            let name = format!("{}.{base}.jsonl", branch.id);
            replace(&self.dir.join("branches").join(&name), &encode(&kept, 0)?)?;
            moved.push((j, name, kept.len(), self.branch_file(branch)));
        }
        let bytes = encode(&events, 0)?;
        let (old, path) = (self.events_path(cid), self.events_file(cid, record.generation + 1));
        private_dir(path.parent().unwrap())?;
        replace(&path, &bytes)?;
        self.obsolete.push(old);
        let record = self.record(cid)?;
        for (j, name, count, _) in &moved {
            let branch = &mut record.branches[*j];
            branch.base = base;
            branch.events = *count;
            branch.file = Some(name.clone());
        }
        record.generation += 1;
        record.events = events.len();
        record.events_bytes = bytes.len() as u64;
        self.obsolete.extend(moved.into_iter().map(|(_, _, _, old)| old));
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
                file: None,
            });
        }
        Ok(id)
    }
    pub fn branch_events(&self, id: &str) -> Result<Vec<format::Message>> {
        let branch = self
            .state
            .conversations
            .values()
            .flat_map(|r| &r.branches)
            .find(|b| b.id == id)
            .context("Unknown branch")?;
        decode(&self.branch_file(branch), branch.events)
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

#[derive(Serialize, Deserialize)]
pub(super) struct Restore {
    pub branch: String,
    pub title: String,
}
fn restore_dir(config: &Path) -> PathBuf {
    config.join("conversation-sync/hub/restore")
}
/// Asks the next scan to restore an archived branch as a separate conversation.
pub(super) fn request_restore(config: &Path, branch: &str, title: &str) -> Result<()> {
    private_dir(&restore_dir(config))?;
    let path = restore_dir(config).join(format!("{}.json", digest(branch.as_bytes())));
    replace_changed(&path, &serde_json::to_vec(&Restore { branch: branch.into(), title: title.into() })?)
}
/// Pending restore requests and the files holding them.
pub(super) fn restore_requests(config: &Path) -> Vec<(PathBuf, Restore)> {
    let Ok(entries) = fs::read_dir(restore_dir(config)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let request = serde_json::from_slice(&bounded(&path, 64 * 1024).ok()?).ok()?;
            Some((path, request))
        })
        .collect()
}
impl Hub {
    /// Creates a conversation from the shared events of a branch and the branch itself.
    pub fn restore(&mut self, request: &Restore) -> Result<Option<String>> {
        let Some((cid, base, cwd)) = self.state.conversations.iter().find_map(|(cid, r)| {
            r.branches
                .iter()
                .find(|b| b.id == request.branch && !b.restored)
                .map(|b| (cid.clone(), b.base, r.cwd.clone()))
        }) else {
            return Ok(None);
        };
        let mut events = self.events(&cid)?;
        events.truncate(base);
        events.extend(self.branch_events(&request.branch)?);
        let restored = stable_id(&format!("{cid}:restore:{}", request.branch));
        self.state.conversations.insert(restored.clone(), Record::new(&request.title, &cwd));
        self.append(&restored, &events)?;
        let record = self.record(&cid)?;
        if let Some(branch) = record.branches.iter_mut().find(|b| b.id == request.branch) {
            branch.restored = true;
        }
        Ok(Some(restored))
    }
}
