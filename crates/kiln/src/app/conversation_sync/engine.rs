//! Hub synchronization: adopt native conversations, ingest changed projections, reconcile
//! divergence and project the canonical history and title into every namespace.
use super::hub::{Hub, NativeRef, Outcome, Projection, Record, conversation_id, reconcile, shared};
use super::*;

pub(super) struct Sync<'a, M> {
    config: &'a Path,
    scope: Option<&'a discovery::Scope>,
    namespaces: Vec<(Tool, PathBuf)>,
    gate: &'a Gate,
    cache: &'a mut Cache,
    status: &'a mut Status,
    metadata: M,
    notify: &'a dyn Fn(&Status),
    workspaces: HashMap<String, Option<PathBuf>>,
    /// Codex state database stamps by namespace, taken when the scan starts.
    databases: HashMap<PathBuf, u64>,
    /// Projections written again by this scan for the current renderer.
    refreshed: usize,
}

/// Revision of the renderer that writes projections.
pub(super) const RENDER: u32 = 2;
/// Projections written again for the current renderer in one scan.
const REFRESH_LIMIT: usize = 4;
/// A projection is written again for the current renderer after it was unchanged this long.
const REFRESH_IDLE: Duration = Duration::from_secs(10 * 60);

/// Native session ID of the projection of `cid` in `root`; each archived projection
/// in that namespace advances the generation.
pub(super) fn projection_id(cid: &str, root: &Path, generation: usize) -> String {
    stable_id(&format!("hub:{cid}:{}:{generation}", root.display()))
}

/// Hash of the lengths and modification times of the Codex state database files and title
/// index of a namespace.
fn database_stamp(root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut files = Vec::new();
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let database = name.starts_with("state_") && (name.ends_with(".sqlite") || name.ends_with(".sqlite-wal"));
        if (database || name == "session_index.jsonl")
            && let Ok(meta) = entry.metadata()
        {
            files.push((name, meta.len(), meta.modified().ok()));
        }
    }
    files.sort();
    let mut hasher = std::hash::DefaultHasher::new();
    files.hash(&mut hasher);
    hasher.finish()
}

fn stamp(source: &Source) -> Result<(u64, SystemTime)> {
    let meta = storage::metadata(source)?;
    Ok((meta.len(), meta.modified()?))
}

fn continuation(source: &Source) -> Continuation {
    Continuation {
        tool: source.tool,
        id: source.id.clone(),
        cwd: source.cwd.clone(),
        root: source.root.clone(),
        revision: PROJECTION_REVISION,
    }
}

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

const WRITING: &str = "Transcript is being written; retrying";

/// Codex Desktop imports of Kiln-written Claude transcripts in one Codex namespace:
/// imported thread → source path.
fn import_ledger(root: &Path) -> Result<HashMap<NativeRef, PathBuf>> {
    let mut ledger = HashMap::new();
    let path = root.join("external_agent_session_imports.json");
    if !path.exists() {
        return Ok(ledger);
    }
    safe_path(root, &path)?;
    let value: serde_json::Value = serde_json::from_slice(&bounded(&path, 8 * 1024 * 1024)?)?;
    for record in value["records"].as_array().into_iter().flatten() {
        if let (Some(source), Some(id)) = (
            record["source_path"].as_str(),
            record["imported_thread_id"].as_str(),
        ) {
            ledger.insert(
                NativeRef { tool: Tool::Codex, root: root.to_owned(), id: id.into() },
                PathBuf::from(source),
            );
        }
    }
    Ok(ledger)
}

/// Handover links: every projection and alias of a linked conversation maps to the current
/// projection of its namespace.
pub(super) fn links(hub: &Hub, scope: Option<&discovery::Scope>) -> Vec<handover::Link> {
    let mut links = Vec::new();
    for (cid, record) in &hub.state.conversations {
        let live: Vec<&Projection> = record.projections.iter().filter(|p| !p.removed).collect();
        if live.len() < 2 || scope.is_some_and(|s| s.workspace(&record.cwd).is_none()) {
            continue;
        }
        for p in &live {
            links.push(handover::Link {
                tool: p.source.tool,
                root: p.source.root.clone(),
                id: p.source.id.clone(),
                group: cid.clone(),
                canonical_id: p.source.id.clone(),
            });
        }
        for alias in &record.aliases {
            if let Some(current) = live
                .iter()
                .find(|p| p.source.tool == alias.tool && p.source.root == alias.root)
            {
                links.push(handover::Link {
                    tool: alias.tool,
                    root: alias.root.clone(),
                    id: alias.id.clone(),
                    group: cid.clone(),
                    canonical_id: current.source.id.clone(),
                });
            }
        }
    }
    links
}

impl<'a, M: FnMut(&Source, Metadata<'_>) -> Result<Option<String>>> Sync<'a, M> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &'a Path,
        scope: Option<&'a discovery::Scope>,
        namespaces: Vec<(Tool, PathBuf)>,
        gate: &'a Gate,
        cache: &'a mut Cache,
        status: &'a mut Status,
        metadata: M,
        notify: &'a dyn Fn(&Status),
    ) -> Self {
        let databases = namespaces
            .iter()
            .filter(|(tool, _)| *tool == Tool::Codex)
            .map(|(_, root)| (root.clone(), database_stamp(root)))
            .collect();
        Self { config, scope, namespaces, gate, cache, status, metadata, notify, workspaces: HashMap::new(), databases, refreshed: 0 }
    }

    fn workspace(&mut self, cwd: &str) -> Option<PathBuf> {
        let scope = self.scope?;
        self.workspaces
            .entry(cwd.to_owned())
            .or_insert_with(|| scope.workspace(cwd).cloned())
            .clone()
    }
    fn in_scope(&mut self, record: &Record) -> bool {
        self.scope.is_none() || self.workspace(&record.cwd).is_some()
    }
    fn live(&self) -> bool {
        self.gate.load(Ordering::Acquire)
    }

    pub fn run(&mut self, mut hub: Hub, sources: Vec<Source>) -> Result<Hub> {
        let mut ledger = HashMap::new();
        for (tool, root) in self.namespaces.clone() {
            if tool != Tool::Codex {
                continue;
            }
            match import_ledger(&root) {
                Ok(entries) => ledger.extend(entries),
                Err(error) => {
                    let path = root.join("external_agent_session_imports.json");
                    let source = Source { tool, root, path: path.clone(), cwd: String::new(), id: String::new() };
                    let mut row = self.unadopted(&source, Phase::Failed, "");
                    row.title = "external_agent_session_imports.json".into();
                    self.report(&path.to_string_lossy(), row, &error);
                }
            }
        }
        let mut index: HashMap<NativeRef, String> = HashMap::new();
        let mut paths: HashMap<PathBuf, String> = HashMap::new();
        for (cid, record) in &hub.state.conversations {
            for p in &record.projections {
                index.insert(NativeRef::of(&p.source), cid.clone());
                paths.insert(p.source.path.clone(), cid.clone());
            }
            for alias in &record.aliases {
                index.insert(alias.clone(), cid.clone());
            }
            for source in &record.pending {
                index.insert(NativeRef::of(source), cid.clone());
                paths.insert(source.path.clone(), cid.clone());
            }
        }
        // A conversation adopted from a Codex Desktop import before its ledger existed joins
        // the conversation it was imported from.
        let mut unmerged = Vec::new();
        for (native, imported_from) in &ledger {
            if let (Some(own), Some(target)) = (index.get(native).cloned(), paths.get(imported_from).cloned())
                && own != target
            {
                match self.merge(&mut hub, &own, &target) {
                    Ok(true) => {
                        for (_, cid) in index.iter_mut().filter(|(_, cid)| **cid == own) {
                            *cid = target.clone();
                        }
                    }
                    Ok(false) => {}
                    Err(error) => unmerged.push((own, error)),
                }
            }
        }
        let mut restored = Vec::new();
        for (path, request) in super::hub::restore_requests(self.config) {
            let key = format!("restore:{}", request.branch);
            let fingerprint = Self::fingerprint(&[fs::metadata(&path).ok().and_then(|m| Some((m.len(), m.modified().ok()?)))], 0);
            if self.deferred(&key, fingerprint) {
                if let Some(row) = self.cache.rows.get(&key) {
                    self.status.rows.insert(key, row.clone());
                }
                continue;
            }
            match hub.restore(&request) {
                Ok(_) => {
                    self.cache.hub.remove(&key);
                    restored.push(path);
                }
                Err(error) => {
                    let owner = hub.state.conversations.values().find(|r| r.branches.iter().any(|b| b.id == request.branch));
                    let mut row = match owner.cloned() {
                        Some(record) => self.row(&record, Phase::Failed, String::new()),
                        None => continue,
                    };
                    row.title = request.title.clone();
                    row.branches.clear();
                    self.fail(&key, row, &error, fingerprint);
                }
            }
        }
        let mut active = Vec::new();
        let cids: Vec<String> = hub.state.conversations.keys().cloned().collect();
        for cid in cids {
            let record = hub.state.conversations[&cid].clone();
            if self.in_scope(&record) {
                if let Some(row) = self.cache.rows.get(&cid) {
                    self.status.rows.insert(cid.clone(), row.clone());
                }
                active.push(cid);
            }
        }
        self.status.watched = active.len();
        for source in &sources {
            let key = source.path.to_string_lossy().into_owned();
            if let Some(row) = self.cache.rows.get(&key) {
                self.status.rows.insert(key, row.clone());
            }
        }
        for source in sources {
            if !self.live() {
                break;
            }
            if !storage::exists(&source) {
                continue;
            }
            let native = NativeRef::of(&source);
            let known = index.get(&native).cloned();
            let is_alias = known
                .as_ref()
                .is_some_and(|cid| hub.state.conversations[cid].aliases.contains(&native));
            let joined = match (&known, ledger.get(&native).and_then(|p| paths.get(p))) {
                (Some(cid), _) if is_alias => Some(cid.clone()),
                (None, Some(target)) => Some(target.clone()),
                _ => None,
            };
            if let Some(cid) = joined {
                let key = source.path.to_string_lossy().into_owned();
                if let Err(error) = self.join(&mut hub, &cid, source.clone()) {
                    let row = self.unadopted(&source, Phase::Failed, "");
                    self.fail(&key, row, &error, Self::fingerprint(&[stamp(&source).ok()], 0));
                }
                index.insert(native, cid);
                continue;
            }
            if known.is_some() {
                continue;
            }
            if let Some(cid) = self.adopt(&mut hub, source)? {
                index.insert(native, cid.clone());
                self.status.watched += 1;
                active.push(cid);
            }
        }
        for cid in active {
            if !self.live() {
                break;
            }
            if !hub.state.conversations.contains_key(&cid) {
                continue;
            }
            self.status.checked += 1;
            self.conversation(&mut hub, &cid);
            (self.notify)(self.status);
        }
        for (own, error) in unmerged {
            if let Some(record) = hub.state.conversations.get(&own).cloned() {
                let row = self.row(&record, Phase::Failed, String::new());
                self.report(&own, row, &error);
            }
        }
        hub.save()?;
        for path in restored {
            let _ = fs::remove_file(path);
        }
        Ok(hub)
    }

    fn native_title(&mut self, source: &Source) -> Result<String> {
        let name = if source.tool == Tool::Codex {
            (self.metadata)(source, Metadata::Read)?.or(self.cache.titles.name(source)?)
        } else {
            self.cache.titles.name(source)?
        };
        Ok(name.unwrap_or_default())
    }

    fn unadopted(&mut self, source: &Source, phase: Phase, detail: &str) -> Conversation {
        Conversation {
            workspace: self.workspace(&source.cwd),
            source: source.clone(),
            title: String::new(),
            phase,
            detail: detail.into(),
            target: None,
            copies: Vec::new(),
            branches: Vec::new(),
        }
    }

    /// Starts a conversation from a native transcript that is not yet part of the hub.
    fn adopt(&mut self, hub: &mut Hub, source: Source) -> Result<Option<String>> {
        let key = source.path.to_string_lossy().into_owned();
        // Kiln copies made before the hub existed are migrated, never adopted as originals.
        if discovery::imported(self.config, &source)?.is_some() || !storage::exists(&source) {
            return Ok(None);
        }
        let Ok(current) = stamp(&source) else {
            return Ok(None);
        };
        if recently_modified(current.1, self.cache.quiet) {
            let row = self.unadopted(&source, Phase::Waiting, WRITING);
            self.status.rows.entry(key).or_insert(row);
            return Ok(None);
        }
        let fingerprint = Self::fingerprint(&[Some(current)], 0);
        if self.deferred(&key, fingerprint) {
            if let Some(row) = self.cache.rows.get(&key) {
                self.status.rows.insert(key, row.clone());
            }
            return Ok(None);
        }
        let mut title = self.native_title(&source)?;
        match checkpoint::read(&source, &mut title) {
            Ok((messages, checkpoint)) if messages.iter().any(|m| m.role == "user") => {
                let cid = conversation_id(&NativeRef::of(&source));
                let mut record = Record::new(&title, &source.cwd);
                record.projections.push(Projection {
                    source: source.clone(),
                    synced: messages.len(),
                    stamp: Some(current),
                    origin: true,
                    removed: false,
                    title: title.clone(),
                    checkpoint,
                    database: None,
                    rendered: 0,
                });
                hub.state.conversations.insert(cid.clone(), record);
                hub.append(&cid, &messages)?;
                hub.state.conversations.get_mut(&cid).unwrap().updated = current.1;
                self.status.rows.remove(&key);
                self.cache.hub.remove(&key);
                Ok(Some(cid))
            }
            Ok(_) => {
                let row = self.unadopted(&source, Phase::Waiting, "");
                self.status.rows.insert(key, row);
                Ok(None)
            }
            Err(error) => {
                let mut row = self.unadopted(&source, Phase::Failed, "");
                row.title = title;
                self.fail(&key, row, &error, fingerprint);
                Ok(None)
            }
        }
    }

    /// A native session copied from a projection outside Kiln: an unchanged copy, or one
    /// without a native turn, leaves the resume list; a continued copy becomes its namespace's
    /// projection.
    fn join(&mut self, hub: &mut Hub, cid: &str, source: Source) -> Result<()> {
        if recently_modified(stamp(&source)?.1, self.cache.quiet) {
            return Ok(());
        }
        let messages = read_messages(&source)?;
        let events = hub.events(cid)?;
        let unchanged = shared(&events, &messages) == messages.len() || canonical::kiln_written(&source)?;
        let record = hub.state.conversations.get_mut(cid).unwrap();
        if unchanged {
            if !record.pending.iter().any(|s| s.path == source.path) {
                record.pending.push(source);
            }
            return Ok(());
        }
        record.displace(source);
        Ok(())
    }

    /// Merges conversation `own` into `target` when its history is contained in the target's;
    /// its live projections leave their resume lists with the target's next synchronization.
    fn merge(&mut self, hub: &mut Hub, own: &str, target: &str) -> Result<bool> {
        let (own_events, target_events) = (hub.events(own)?, hub.events(target)?);
        if shared(&target_events, &own_events) != own_events.len() {
            return Ok(false);
        }
        let record = hub.state.conversations.remove(own).context("Unknown conversation")?;
        let target_record = hub.state.conversations.get_mut(target).context("Unknown conversation")?;
        for p in record.projections {
            if p.removed {
                target_record.aliases.push(NativeRef::of(&p.source));
            } else {
                target_record.pending.push(p.source);
            }
        }
        target_record.aliases.extend(record.aliases);
        target_record.pending.extend(record.pending);
        self.status.rows.remove(own);
        Ok(true)
    }

    /// Moves native sessions left over from earlier copies out of their resume lists; one whose
    /// native turns continued past the canonical history is kept as a branch, or becomes its
    /// namespace's projection again when it changed after the canonical history did.
    fn archive_pending(&mut self, hub: &mut Hub, cid: &str, events: &mut Option<Vec<format::Message>>) -> Result<()> {
        let pending = hub.state.conversations[cid].pending.clone();
        let mut failure = None;
        for source in pending {
            let native = NativeRef::of(&source);
            if storage::exists(&source) {
                let modified = stamp(&source)?.1;
                if recently_modified(modified, self.cache.quiet) {
                    continue;
                }
                let (messages, native_turns) = match read_messages(&source).and_then(|m| Ok((m, !canonical::kiln_written(&source)?))) {
                    Ok(read) => read,
                    Err(error) => { failure.get_or_insert(error); continue; }
                };
                let mut base = messages.len();
                if native_turns && !messages.is_empty() {
                    if events.is_none() {
                        *events = Some(hub.events(cid)?);
                    }
                    base = shared(events.as_ref().unwrap(), &messages);
                }
                let record = hub.state.conversations.get_mut(cid).unwrap();
                if base < messages.len() && modified > record.updated {
                    record.displace(source);
                    hub.save()?;
                    continue;
                }
                let archived = match canonical::archive_native(self.config, &source, cid, &mut self.metadata) {
                    Ok(archived) => archived,
                    Err(error) => { failure.get_or_insert(error); continue; }
                };
                if base < messages.len() {
                    hub.save_branch(cid, base, &messages[base..], Some(native.clone()), Some(archived))?;
                }
            }
            let record = hub.state.conversations.get_mut(cid).unwrap();
            record.pending.retain(|s| s.path != source.path);
            if !record.aliases.contains(&native) {
                record.aliases.push(native);
            }
            hub.save()?;
        }
        match failure { Some(error) => Err(error), None => Ok(()) }
    }

    fn fingerprint(stamps: &[Option<(u64, SystemTime)>], events: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        stamps.hash(&mut hasher);
        events.hash(&mut hasher);
        hasher.finish()
    }
    /// An unchanged conversation that failed recently waits for its retry delay.
    fn deferred(&self, key: &str, fingerprint: u64) -> bool {
        self.cache.hub.get(key).is_some_and(|(seen, at, failures, settled)| {
            let waited = at.elapsed().unwrap_or_default();
            *seen == fingerprint && (waited < retry_delay(*failures) || (*settled && waited < FAILED_RECHECK))
        })
    }
    /// Shows a failure on a row without delaying the next attempt.
    fn report(&mut self, key: &str, mut row: Conversation, error: &anyhow::Error) {
        let detail = error.to_string();
        row.phase = if detail.contains("retrying") { Phase::Waiting } else { Phase::Failed };
        row.detail = detail;
        self.status.rows.insert(key.to_owned(), row);
    }
    fn fail(&mut self, key: &str, row: Conversation, error: &anyhow::Error, fingerprint: u64) {
        let detail = error.to_string();
        let retrying = detail.contains("retrying");
        // A condition the user can resolve keeps the short retry delays.
        let settled = !retrying && classify(&detail).is_none();
        self.report(key, row, error);
        let failures = self.cache.hub.get(key).map_or(0, |(_, _, n, _)| *n) + 1;
        self.cache
            .hub
            .insert(key.to_owned(), (fingerprint, SystemTime::now(), failures, settled));
    }

    fn row(&mut self, record: &Record, phase: Phase, detail: String) -> Conversation {
        let live: Vec<&Projection> = record.projections.iter().filter(|p| !p.removed).collect();
        // A conversation whose first projection is still being written shows its namespace.
        let origin = live
            .iter()
            .find(|p| p.origin)
            .or(live.first())
            .map(|p| &p.source)
            .or(record.projections.first().map(|p| &p.source))
            .cloned()
            .unwrap_or_else(|| {
                let (tool, root) = self.namespaces.first().cloned().unwrap_or((Tool::Claude, PathBuf::new()));
                Source { tool, root, path: PathBuf::new(), cwd: record.cwd.clone(), id: String::new() }
            });
        let copies: Vec<Continuation> = live
            .iter()
            .filter(|p| p.source.path != origin.path)
            .map(|p| continuation(&p.source))
            .collect();
        Conversation {
            workspace: self.workspace(&record.cwd),
            source: origin,
            title: record.title.clone(),
            phase,
            detail,
            target: copies.first().cloned(),
            copies,
            branches: record
                .branches
                .iter()
                .filter(|b| !b.restored)
                .map(|b| BranchRow { id: b.id.clone(), events: b.events, at: b.at })
                .collect(),
        }
    }
    fn copying(&mut self, hub: &Hub, cid: &str) {
        let row = self.row(&hub.state.conversations[cid], Phase::Copying, String::new());
        self.status.rows.insert(cid.to_owned(), row);
        (self.notify)(self.status);
    }

    fn stamps(record: &Record) -> Vec<Option<(u64, SystemTime)>> {
        record
            .projections
            .iter()
            .map(|p| if p.removed { None } else { stamp(&p.source).ok() })
            .collect()
    }

    fn conversation(&mut self, hub: &mut Hub, cid: &str) {
        let fingerprint = {
            let record = &hub.state.conversations[cid];
            Self::fingerprint(&Self::stamps(record), record.events)
        };
        if self.deferred(cid, fingerprint) {
            if let Some(row) = self.cache.rows.get(cid) {
                self.status.rows.insert(cid.to_owned(), row.clone());
            }
            return;
        }
        let result = self.synchronize(hub, cid);
        let record = hub.state.conversations[cid].clone();
        match result {
            Ok(None) => {
                self.cache.hub.remove(cid);
                let row = self.row(&record, Phase::Synced, String::new());
                self.status.rows.insert(cid.to_owned(), row);
                self.status.settled.insert(cid.to_owned());
            }
            Ok(Some(reason)) => {
                self.cache.hub.remove(cid);
                let row = self.row(&record, Phase::Waiting, reason);
                self.status.rows.insert(cid.to_owned(), row);
            }
            Err(error) => {
                let row = self.row(&record, Phase::Failed, String::new());
                let fingerprint = Self::fingerprint(&Self::stamps(&record), record.events);
                self.fail(cid, row, &error, fingerprint);
            }
        }
    }

    /// Ingests changed projections, follows native renames, then brings every namespace up
    /// to the canonical history and title. Returns why a projection is still waiting.
    fn synchronize(&mut self, hub: &mut Hub, cid: &str) -> Result<Option<String>> {
        let mut waiting: Option<String> = None;
        let mut events: Option<Vec<format::Message>> = None;
        let mut renames: Vec<(SystemTime, String)> = Vec::new();
        let mut read_titles = std::collections::HashSet::new();
        // Events appended by this scan's incremental reads, starting at the given index.
        let mut fresh: Option<(usize, Vec<format::Message>)> = None;
        // A projection invalidated by a newer branch is reconciled again in the next pass.
        for _pass in 0..3 {
            let mut changed = Vec::new();
            let mut moved = Vec::new();
            for p in hub.state.conversations.get_mut(cid).unwrap().projections.iter_mut() {
                if p.removed {
                    continue;
                }
                if !storage::exists(&p.source) {
                    // A projection Kiln archived before its state was saved is an earlier session.
                    if canonical::archived(self.config, &p.source) {
                        moved.push(NativeRef::of(&p.source));
                    } else {
                        p.removed = true;
                    }
                    continue;
                }
                let current = stamp(&p.source)?;
                if Some(current) == p.stamp {
                    continue;
                }
                if recently_modified(current.1, self.cache.quiet) {
                    waiting.get_or_insert_with(|| WRITING.into());
                    continue;
                }
                changed.push((current.1, NativeRef::of(&p.source), current));
            }
            if !moved.is_empty() {
                let record = hub.state.conversations.get_mut(cid).unwrap();
                record.projections.retain(|p| !moved.contains(&NativeRef::of(&p.source)));
                for native in moved {
                    if !record.aliases.contains(&native) {
                        record.aliases.push(native);
                    }
                }
            }
            if changed.is_empty() {
                break;
            }
            // The most recently changed history is reconciled last and wins over older ones.
            changed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
            for (_, native_ref, current) in changed {
                if !self.live() {
                    return Ok(Some(String::new()));
                }
                let position = |hub: &Hub| {
                    hub.state.conversations[cid]
                        .projections
                        .iter()
                        .position(|p| NativeRef::of(&p.source) == native_ref)
                };
                let Some(i) = position(hub) else { continue };
                let source = hub.state.conversations[cid].projections[i].source.clone();
                let mut title = self.native_title(&source)?;
                read_titles.insert(native_ref.clone());
                let (synced, total, known) = {
                    let record = &hub.state.conversations[cid];
                    (record.projections[i].synced, record.events, record.projections[i].checkpoint.clone())
                };
                // Records appended after a completed turn are read on their own when the
                // projection held the whole canonical history.
                if let Some(cp) = &known
                    && let Some((appended, renamed, next)) = checkpoint::since_titled(&source, cp)?
                    && (appended.is_empty() || synced == total)
                {
                    let record = hub.state.conversations.get_mut(cid).unwrap();
                    let p = &mut record.projections[i];
                    // A Claude title is the newest title record of the transcript, then its sidecar.
                    let title = match renamed {
                        Some(renamed) => renamed,
                        None if source.tool == Tool::Claude && !p.title.is_empty() => p.title.clone(),
                        None => title,
                    };
                    if !title.is_empty() && title != p.title {
                        if !p.title.is_empty() || p.origin {
                            renames.push((current.1, title.clone()));
                        }
                        p.title = title;
                    }
                    p.synced += appended.len();
                    p.stamp = Some(current);
                    p.checkpoint = Some(next);
                    if !appended.is_empty() {
                        record.updated = current.1;
                        if let Some(canonical) = events.as_mut() {
                            canonical.extend(appended.iter().cloned());
                        }
                        fresh.get_or_insert_with(|| (total, Vec::new())).1.extend(appended.iter().cloned());
                        hub.append(cid, &appended)?;
                    }
                    continue;
                }
                let (native, read_checkpoint) = checkpoint::read(&source, &mut title)?;
                if events.is_none() {
                    events = Some(hub.events(cid)?);
                }
                let canonical = events.as_mut().unwrap();
                let updated = hub.state.conversations[cid].updated;
                {
                    let p = &mut hub.state.conversations.get_mut(cid).unwrap().projections[i];
                    if !title.is_empty() && title != p.title {
                        if !p.title.is_empty() || p.origin {
                            renames.push((current.1, title.clone()));
                        }
                        p.title = title;
                    }
                }
                match reconcile(canonical, synced, &native, current.1 > updated) {
                    Outcome::Unchanged | Outcome::Behind => {
                        let p = &mut hub.state.conversations.get_mut(cid).unwrap().projections[i];
                        p.synced = native.len();
                        p.stamp = Some(current);
                        p.checkpoint = read_checkpoint;
                    }
                    Outcome::Append(count) => {
                        hub.append(cid, &native[canonical.len()..])?;
                        // Shared events keep their canonical form; only the continuation is new.
                        let start = canonical.len();
                        canonical.extend_from_slice(&native[start..]);
                        let record = hub.state.conversations.get_mut(cid).unwrap();
                        record.updated = current.1;
                        record.projections[i].synced = count;
                        record.projections[i].stamp = Some(current);
                        record.projections[i].checkpoint = read_checkpoint;
                    }
                    Outcome::NativeWins { base } => {
                        hub.save_branch(cid, base, &canonical[base..], None, None)?;
                        hub.rewrite(cid, base, &native[base..])?;
                        canonical.truncate(base);
                        canonical.extend_from_slice(&native[base..]);
                        let record = hub.state.conversations.get_mut(cid).unwrap();
                        record.updated = current.1;
                        record.projections[i].synced = canonical.len();
                        record.projections[i].stamp = Some(current);
                        record.projections[i].checkpoint = read_checkpoint;
                        for (j, q) in record.projections.iter_mut().enumerate() {
                            if j != i && !q.removed && q.synced > base {
                                q.stamp = None;
                                q.checkpoint = None;
                            }
                        }
                    }
                    Outcome::CanonicalWins { base } => {
                        let archived = canonical::archive_native(self.config, &source, cid, &mut self.metadata)?;
                        hub.save_branch(cid, base, &native[base..], Some(native_ref.clone()), Some(archived))?;
                        let record = hub.state.conversations.get_mut(cid).unwrap();
                        record.projections.remove(i);
                        record.aliases.push(native_ref);
                        hub.save()?;
                    }
                }
            }
        }
        // Codex names can change in its database without a transcript change.
        let codex: Vec<(usize, Source, bool)> = hub.state.conversations[cid]
            .projections
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.removed && p.source.tool == Tool::Codex)
            .map(|(i, p)| (i, p.source.clone(), read_titles.contains(&NativeRef::of(&p.source))))
            .collect();
        for (i, source, read) in codex {
            let database = self.databases.get(&source.root).copied();
            let known = hub.state.conversations[cid].projections[i].database;
            if read || (database.is_some() && known == database) {
                hub.state.conversations.get_mut(cid).unwrap().projections[i].database = database;
                continue;
            }
            let title = self.native_title(&source)?;
            let p = &mut hub.state.conversations.get_mut(cid).unwrap().projections[i];
            p.database = database;
            if !title.is_empty() && title != p.title {
                if !p.title.is_empty() || p.origin {
                    renames.push((SystemTime::now(), title.clone()));
                }
                p.title = title;
            }
        }
        if let Some((_, title)) = renames.into_iter().max_by(|a, b| a.0.cmp(&b.0)) {
            hub.state.conversations.get_mut(cid).unwrap().title = title;
        }
        // Project into every namespace.
        let namespaces = self.namespaces.clone();
        for (tool, root) in namespaces {
            if !self.live() {
                return Ok(Some(String::new()));
            }
            let record = &hub.state.conversations[cid];
            let existing = record
                .projections
                .iter()
                .position(|p| p.source.tool == tool && p.source.root == root);
            match existing {
                Some(i) if record.projections[i].removed => {}
                Some(i)
                    if record.projections[i].synced >= record.events
                        && record.projections[i].title == record.title => {}
                Some(i) => {
                    let from = record.projections[i].synced.min(record.events);
                    let tail = match (&events, &fresh) {
                        (Some(all), _) => all[from..].to_vec(),
                        (None, Some((start, recent))) if from >= *start => recent[from - start..].to_vec(),
                        _ => hub.events_from(cid, from)?,
                    };
                    if let Some(reason) = self.append(hub, cid, i, &tail, &mut events)? {
                        waiting.get_or_insert(reason);
                    }
                }
                None => {
                    if events.is_none() {
                        events = Some(hub.events(cid)?);
                    }
                    self.create(hub, cid, tool, &root, events.as_ref().unwrap())?;
                }
            }
        }
        self.archive_pending(hub, cid, &mut events)?;
        if waiting.is_none() {
            self.refresh(hub, cid, &mut events)?;
        }
        Ok(waiting)
    }

    /// Writes a Claude projection Kiln made with an earlier renderer again when the canonical
    /// history exceeds Claude Code's context budget and the projection holds only Kiln's
    /// records and has been unchanged for `REFRESH_IDLE`: the earlier session leaves its resume
    /// list and the namespace receives a new projection. Other projections are marked current.
    fn refresh(&mut self, hub: &mut Hub, cid: &str, events: &mut Option<Vec<format::Message>>) -> Result<()> {
        let record = &hub.state.conversations[cid];
        let stale: Vec<usize> = record
            .projections
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.origin && !p.removed && p.rendered < RENDER && p.synced >= record.events)
            .filter(|(_, p)| self.namespaces.iter().any(|(tool, root)| *tool == p.source.tool && *root == p.source.root))
            .map(|(i, _)| i)
            .collect();
        if stale.is_empty() || self.refreshed >= REFRESH_LIMIT {
            return Ok(());
        }
        if events.is_none() {
            *events = Some(hub.events(cid)?);
        }
        let all = events.as_ref().unwrap();
        let large = all.iter().map(super::context::size).sum::<usize>() > super::context::BUDGET;
        let current = |hub: &mut Hub, i: usize| hub.state.conversations.get_mut(cid).unwrap().projections[i].rendered = RENDER;
        let mut rewrite = None;
        for i in stale {
            let p = &hub.state.conversations[cid].projections[i];
            let (source, known) = (p.source.clone(), p.stamp);
            if !large || source.tool != Tool::Claude || discovery::imported(self.config, &source)?.is_none() {
                current(hub, i);
                continue;
            }
            let stamped = stamp(&source)?;
            if rewrite.is_some() || known != Some(stamped) || recently_modified(stamped.1, REFRESH_IDLE) {
                continue;
            }
            if canonical::kiln_written(&source)? {
                rewrite = Some(i);
            } else {
                current(hub, i);
            }
        }
        let Some(i) = rewrite else { return Ok(()) };
        let source = hub.state.conversations[cid].projections[i].source.clone();
        match canonical::archive_native(self.config, &source, cid, &mut self.metadata) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("retrying") => return Ok(()),
            Err(_) => {
                current(hub, i);
                return Ok(());
            }
        }
        let record = hub.state.conversations.get_mut(cid).unwrap();
        record.projections.remove(i);
        record.aliases.push(NativeRef::of(&source));
        hub.save()?;
        self.refreshed += 1;
        self.create(hub, cid, source.tool, &source.root, all)
    }

    /// Appends the missing canonical events (`tail`) and title; returns why a busy projection
    /// waits. `events` holds the whole canonical history once a full update needs it.
    fn append(
        &mut self,
        hub: &mut Hub,
        cid: &str,
        i: usize,
        tail: &[format::Message],
        events: &mut Option<Vec<format::Message>>,
    ) -> Result<Option<String>> {
        let (source, known, title, checkpoint, total) = {
            let record = &hub.state.conversations[cid];
            let p = &record.projections[i];
            (p.source.clone(), p.stamp, record.title.clone(), p.checkpoint.clone(), record.events)
        };
        let current = stamp(&source)?;
        if recently_modified(current.1, self.cache.quiet) || known != Some(current) {
            return Ok(Some(WRITING.into()));
        }
        let guard = match handover::write_guard(self.config, &source) {
            Ok(guard) => guard,
            Err(error) if error.to_string().contains("retrying") => return Ok(Some(error.to_string())),
            Err(error) => return Err(error),
        };
        self.copying(hub, cid);
        let quick = match &checkpoint {
            Some(cp) => canonical::append_since(&source, cp, tail, &title, &timestamp(), &file_changes(hub, cid, tail))?,
            None => None,
        };
        let (changed, next) = match quick {
            Some(next) => (true, Some(next)),
            None => {
                if events.is_none() {
                    *events = Some(hub.events(cid)?);
                }
                let all = events.as_ref().unwrap();
                let changes = file_changes(hub, cid, &all[source_len(all, tail)..]);
                let changed = canonical::update(&source, all, &title, &timestamp(), &changes)?;
                (changed, checkpoint::read(&source, &mut title.clone())?.1)
            }
        };
        if source.tool == Tool::Codex {
            (self.metadata)(&source, Metadata::Name(&title))?;
        }
        titles::pin_claude(&source, &title)?;
        drop(guard);
        let p = &mut hub.state.conversations.get_mut(cid).unwrap().projections[i];
        p.synced = total;
        p.stamp = Some(stamp(&source)?);
        p.title = title;
        p.checkpoint = next;
        if changed {
            self.status.published += 1;
            self.status.recent.retain(|c| c.id != source.id || c.root != source.root);
            self.status.recent.insert(0, continuation(&source));
        }
        Ok(None)
    }

    /// Writes a new projection of the canonical history into a namespace and registers it.
    fn create(&mut self, hub: &mut Hub, cid: &str, tool: Tool, root: &Path, events: &[format::Message]) -> Result<()> {
        let record = &hub.state.conversations[cid];
        if events.is_empty() || !events.iter().any(|m| m.role == "user") {
            return Ok(());
        }
        let generation = record
            .aliases
            .iter()
            .filter(|a| a.tool == tool && a.root == root)
            .count();
        let id = projection_id(cid, root, generation);
        let date = chrono::DateTime::<chrono::Utc>::from(record.created);
        let source = Source {
            tool,
            root: root.to_owned(),
            path: native_path(tool, root, &record.cwd, &id, date)?,
            cwd: record.cwd.clone(),
            id,
        };
        let title = record.title.clone();
        let (synced, known, written) = if storage::exists(&source) {
            // A projection published before an interrupted state save is adopted; a history
            // continued since then is ingested on the next pass.
            let collision = || anyhow::anyhow!("Continuation collision; existing conversation was preserved");
            let existing = read_messages(&source).map_err(|_| collision())?;
            if super::hub::shared(events, &existing) == existing.len() {
                (existing.len(), Some(stamp(&source)?), None)
            } else if super::hub::shared(events, &existing) == events.len() {
                (events.len(), None, None)
            } else {
                return Err(collision());
            }
        } else {
            let changes = file_changes(hub, cid, events);
            let bytes = format::render_changed(tool, &source.id, &source.cwd, &timestamp(), events, &title, &changes)?;
            let converted: Vec<serde_json::Value> = bytes
                .split(|b| *b == b'\n')
                .filter(|b| !b.is_empty())
                .map(serde_json::from_slice)
                .collect::<std::result::Result<_, _>>()?;
            ensure!(
                format::parse(tool, &converted)? == events,
                "Converted conversation did not retain every message and attachment"
            );
            drop(converted);
            ensure!(
                bytes.len() as u64 <= COPY_LIMIT,
                "Converted conversation exceeds the 1 GiB sync limit"
            );
            discovery::remember_projection(self.config, &source, &json_digest(&events)?, Some(events))?;
            if !self.live() {
                return Ok(());
            }
            self.copying(hub, cid);
            publish(&source.path, &bytes)?;
            (events.len(), Some(stamp(&source)?), checkpoint::of_bytes(tool, &bytes)?)
        };
        (self.metadata)(&source, Metadata::Name(&title))?;
        titles::pin_claude(&source, &title)?;
        self.status.published += 1;
        self.status.recent.retain(|c| c.id != source.id || c.root != source.root);
        self.status.recent.insert(0, continuation(&source));
        hub.state.conversations.get_mut(cid).unwrap().projections.push(Projection {
            source,
            synced,
            stamp: known,
            origin: false,
            removed: false,
            title,
            checkpoint: written,
            database: None,
            rendered: RENDER,
        });
        Ok(())
    }
}

/// Whether a tool call record edits files with Codex `apply_patch`.
fn edits_files(message: &format::Message) -> bool {
    message.text.starts_with("[Tool call: apply_patch]\n")
        || (message.text.starts_with("[Tool call: exec]\n") && message.text.contains("tools.apply_patch("))
}

/// The index in `all` where `tail` starts, when `all` ends with it; otherwise 0.
fn source_len(all: &[format::Message], tail: &[format::Message]) -> usize {
    all.len().checked_sub(tail.len()).filter(|start| all[*start..] == *tail).unwrap_or(0)
}

/// The file changes Codex recorded for the editing calls among `messages`, read from the
/// conversation's Codex histories: its original first. A history that cannot be read
/// contributes none, so its calls keep their readable form.
fn file_changes(hub: &Hub, cid: &str, messages: &[format::Message]) -> super::tools::FileChanges {
    let mut wanted: std::collections::HashSet<&str> = Default::default();
    for (call, result) in messages.iter().zip(messages.iter().skip(1)) {
        if edits_files(call)
            && let Some((id, _)) = result.text.strip_prefix("[Tool result: ").and_then(|rest| rest.split_once("]\n"))
        {
            wanted.insert(id);
        }
    }
    let mut changes = super::tools::FileChanges::new();
    if wanted.is_empty() {
        return changes;
    }
    let record = &hub.state.conversations[cid];
    let mut sources: Vec<(bool, &Source)> = record
        .projections
        .iter()
        .filter(|p| !p.removed && p.source.tool == Tool::Codex)
        .map(|p| (p.origin, &p.source))
        .chain(record.pending.iter().filter(|s| s.tool == Tool::Codex).map(|s| (false, s)))
        .collect();
    sources.sort_by_key(|(origin, _)| !*origin);
    for (_, source) in sources {
        if wanted.iter().all(|id| changes.contains_key(*id)) {
            break;
        }
        let Ok((input, _)) = storage::open_at(&source.root, &storage::physical(source), 0) else { continue };
        let Ok(found) = super::tools::file_changes(std::io::BufReader::new(input)) else { continue };
        for (call, items) in found {
            if wanted.contains(call.as_str()) {
                changes.entry(call).or_insert(items);
            }
        }
    }
    changes
}
