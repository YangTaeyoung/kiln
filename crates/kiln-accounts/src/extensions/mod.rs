//! Private native author assets. Reconciled only by the launch preparation worker.
//! The manifest contains fingerprints, never payload/configuration values.
mod components;
mod fs;
mod ownership;
mod plugins;
mod scan;
use crate::Tool;
use fs::{Dir, Kind};
use ownership::protect_plugin_units;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};
use unicode_normalization::UnicodeNormalization;
thread_local! {static RECONCILE_START:std::cell::Cell<Option<Instant>>=const {std::cell::Cell::new(None)};}
struct DeadlineGuard(Option<Instant>);
impl DeadlineGuard {
    fn enter() -> Self {
        Self(RECONCILE_START.replace(Some(Instant::now())))
    }
}
impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        RECONCILE_START.set(self.0);
    }
}
type R<T> = Result<T, &'static str>;
const METADATA_BYTES: usize = 1024 * 1024;
const FILE_BYTES: usize = 8 * 1024 * 1024;
const ASSET_FILES: usize = 2048;
const ASSET_BYTES: usize = 16 * 1024 * 1024;
const PLUGIN_FILES: usize = 4096;
const PLUGIN_BYTES: usize = 64 * 1024 * 1024;
const TOTAL_ASSETS: usize = 512;
const TOTAL_FILES: usize = 16384;
const TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_DEPTH: usize = 16;
const MAX_RECORDS: usize = 1024;
const MAX_DEPENDENCY_HOPS: usize = 32;
const MAX_DEPENDENCY_PLUGINS: usize = 128;
const DEADLINE: Duration = Duration::from_secs(5);
pub const MAX_REPORT_BYTES: usize = 512 * 1024;
const STATE: &str = "kiln-extension-state";
const MANIFEST: &str = "manifest.json";
const TRACKED: &str = "kiln-extensions-tracked";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionStatus {
    pub kind: String,
    pub name: String,
    pub state: String,
    pub reason: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionReport {
    pub entries: Vec<ExtensionStatus>,
}
impl ExtensionReport {
    /// Persist a bounded safe handoff under an already-created private launch directory.
    pub fn write_prepared(&self, path: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(self.validated(), "invalid prepared extension report");
        let bytes = serde_json::to_vec(self)?;
        anyhow::ensure!(
            bytes.len() <= MAX_REPORT_BYTES,
            "oversized prepared extension report"
        );
        let parent = Dir::open(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("missing report directory"))?,
        )
        .map_err(anyhow::Error::msg)?;
        parent.private().map_err(anyhow::Error::msg)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid report name"))?;
        fs::component(name).map_err(anyhow::Error::msg)?;
        parent.atomic(name, &bytes).map_err(anyhow::Error::msg)
    }
    /// Reads only an owner-private, bounded prepared report; does not rescan assets.
    pub fn read_prepared(path: &Path) -> anyhow::Result<Self> {
        let parent = Dir::open(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("missing report directory"))?,
        )
        .map_err(anyhow::Error::msg)?;
        parent.private().map_err(anyhow::Error::msg)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid report name"))?;
        let bytes = parent
            .read_private(name, MAX_REPORT_BYTES)
            .map_err(anyhow::Error::msg)?;
        let report: Self = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(report.validated(), "invalid prepared extension report");
        Ok(report)
    }
    fn push(&mut self, kind: &str, name: &str, state: &str, reason: &str) {
        if self.entries.len() == TOTAL_ASSETS + 7 {
            self.entries.push(ExtensionStatus {
                kind: "extensions".into(),
                name: "Additional extension diagnostics".into(),
                state: "unsupported".into(),
                reason: "REPORT_ROWS_520".into(),
            });
            return;
        }
        if self.entries.len() < TOTAL_ASSETS + 8 {
            self.entries.push(ExtensionStatus {
                kind: clean(kind, 32),
                name: clean(name, 160),
                state: clean(state, 32),
                reason: clean(reason, 96),
            });
        }
    }
    /// Validate a prepared, owner-only launch report before it crosses into the journal/UI.
    pub fn validated(&self) -> bool {
        self.entries.len() <= TOTAL_ASSETS + 8
            && self.entries.iter().all(|e| {
                [
                    "prepared",
                    "failed",
                    "unsupported",
                    "native",
                    "native_account",
                    "profile_override",
                    "profile_deleted",
                    "source_removed",
                    "absent",
                    "retained",
                ]
                .contains(&e.state.as_str())
                    && e.reason
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                    && e.kind == clean(&e.kind, 32)
                    && e.name == clean(&e.name, 160)
                    && e.state == clean(&e.state, 32)
                    && e.reason == clean(&e.reason, 96)
            })
    }
}
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn nonce() -> String {
    let mut b = [0; 12];
    getrandom::fill(&mut b).expect("OS randomness");
    b.iter().map(|v| format!("{v:02x}")).collect()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Target {
    path: String,
    #[serde(default)]
    leaf: Vec<String>,
    #[serde(default)]
    toml: bool,
}
impl Target {
    fn file(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            leaf: vec![],
            toml: false,
        }
    }
    fn key(&self) -> String {
        format!("{}#{}", self.path, self.leaf.join("/"))
    }
    fn valid(&self) -> R<()> {
        fs::relative(&self.path)?;
        let asset = self.leaf.is_empty()
            && [
                "skills/",
                "commands/",
                "agents/",
                "output-styles/",
                "workflows/",
                "plugins/cache/",
                "plugins/marketplaces/",
            ]
            .iter()
            .any(|p| self.path.starts_with(p));
        let metadata = match (self.path.as_str(), self.leaf.as_slice(), self.toml) {
            ("settings.json", [field, _], false) => field == "enabledPlugins",
            ("config.toml", [field, _, enabled], true) => {
                field == "plugins" && enabled == "enabled"
            }
            ("plugins/installed_plugins.json", [field, _], false) => field == "plugins",
            ("plugins/known_marketplaces.json", [_], false) => true,
            _ => false,
        };
        if !asset && !metadata {
            return Err("INVALID_MANIFEST_TARGET");
        }
        if self.leaf.len() > 4
            || self
                .leaf
                .iter()
                .any(|s| s.len() > 240 || s.chars().any(char::is_control))
        {
            return Err("INVALID_MANIFEST");
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    #[serde(default)]
    unit: Option<String>,
    target: Target,
    kind: String,
    source: String,
    written: Option<String>,
    state: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    root: String,
    entries: BTreeMap<String, Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    entry: Entry,
    before: Option<String>,
    after: Option<String>,
    stage: String,
    #[serde(default)]
    backup: String,
}
#[derive(Clone)]
struct FileData {
    path: String,
    bytes: Vec<u8>,
    exec: bool,
}
#[derive(Clone)]
struct Snapshot {
    directory: bool,
    files: Vec<FileData>,
    dirs: Vec<String>,
}
impl Snapshot {
    fn fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update([self.directory as u8]);
        for d in &self.dirs {
            h.update((d.len() as u64).to_le_bytes());
            h.update(d.as_bytes());
        }
        for f in &self.files {
            h.update((f.path.len() as u64).to_le_bytes());
            h.update(f.path.as_bytes());
            h.update([f.exec as u8]);
            h.update((f.bytes.len() as u64).to_le_bytes());
            h.update(&f.bytes);
        }
        format!("{:x}", h.finalize())
    }
    fn one(bytes: Vec<u8>) -> Self {
        Self {
            directory: false,
            files: vec![FileData {
                path: String::new(),
                bytes,
                exec: false,
            }],
            dirs: vec![],
        }
    }
    fn stage(&self, dir: &Dir, name: &str) -> R<()> {
        if self.directory {
            let out = dir.dir(name, true)?;
            for d in &self.dirs {
                let (p, n) = out.parent(d, true)?;
                p.dir(&n, true)?;
            }
            for f in &self.files {
                Budget::new().check()?;
                let (p, n) = out.parent(&f.path, true)?;
                p.write_new(&n, &f.bytes, f.exec)?;
            }
            out.sync_tree()?;
        } else {
            let f = &self.files[0];
            dir.write_new(name, &f.bytes, f.exec)?;
        }
        dir.sync()?;
        Ok(())
    }
}
struct Budget {
    start: Instant,
    files: usize,
    bytes: usize,
}
impl Budget {
    fn new() -> Self {
        Self {
            start: RECONCILE_START.get().unwrap_or_else(Instant::now),
            files: 0,
            bytes: 0,
        }
    }
    fn check(&self) -> R<()> {
        if self.start.elapsed() > DEADLINE {
            Err("RECONCILE_DEADLINE_5_SECONDS")
        } else {
            Ok(())
        }
    }
}
#[derive(Clone)]
struct Asset {
    unit: Option<String>,
    kind: String,
    target: Target,
    snapshot: Snapshot,
    leaf: Option<Value>,
    source_fingerprint: Option<String>,
}
impl Asset {
    fn tree(kind: &str, path: String, snapshot: Snapshot) -> Self {
        Self {
            unit: None,
            kind: kind.into(),
            target: Target::file(path),
            source_fingerprint: Some(snapshot.fingerprint()),
            snapshot,
            leaf: None,
        }
    }
    fn fingerprint(&self) -> String {
        self.leaf.as_ref().map_or_else(
            || self.snapshot.fingerprint(),
            |v| hash(&serde_json::to_vec(v).unwrap()),
        )
    }
}
struct Scan {
    assets: Vec<Asset>,
    dependencies: BTreeMap<String, BTreeSet<String>>,
    report: ExtensionReport,
    complete: bool,
    seen: BTreeSet<String>,
}
impl Scan {
    fn new() -> Self {
        Self {
            assets: vec![],
            dependencies: BTreeMap::new(),
            report: ExtensionReport::default(),
            complete: true,
            seen: BTreeSet::new(),
        }
    }
    fn error(&mut self, kind: &str, name: &str, error: &str) {
        self.complete = false;
        self.report.push(kind, name, "failed", error);
    }
    fn add(&mut self, asset: Asset) -> R<()> {
        if self.assets.len() >= TOTAL_ASSETS {
            return Err("TOTAL_ASSETS_512");
        }
        let key = asset.target.key().nfc().collect::<String>().to_lowercase();
        if !self.seen.insert(key.clone()) {
            self.assets
                .retain(|a| a.target.key().nfc().collect::<String>().to_lowercase() != key);
            return Err("CASE_OR_UNICODE_COLLISION");
        }
        self.assets.push(asset);
        Ok(())
    }
}

/// Source roots are captured before the selected child's account environment is built.
/// This does not run in validation/reload polling and never invokes native hooks.
pub fn reconcile(tool: Tool, source: &Path, destination: &Path) -> ExtensionReport {
    let mut report = ExtensionReport::default();
    if source == destination || tool == Tool::Antigravity {
        return report;
    }
    match run(tool, source, destination, true) {
        Ok(r) => r,
        Err(e) => {
            report.push("extensions", "Personal extensions", "failed", e);
            report
        }
    }
}
pub(crate) fn reconcile_with_overrides(
    tool: Tool,
    source: &Path,
    destination: &Path,
    cache: Option<&Path>,
    dirs: bool,
    seed: bool,
) -> ExtensionReport {
    let external = tool == Tool::Claude && cache.is_some_and(|p| p != source.join("plugins"));
    let mut report = match run(tool, source, destination, !external) {
        Ok(r) => r,
        Err(e) => {
            let mut r = ExtensionReport::default();
            r.push("extensions", "Personal extensions", "failed", e);
            r
        }
    };
    if external {
        report.push(
            "plugins",
            "Plugin cache override",
            "unsupported",
            "EXTERNAL_PLUGIN_ROOT",
        );
    }
    if dirs {
        report.push(
            "plugins",
            "Explicit plugin directories",
            "unsupported",
            "EXTERNAL_PLUGIN_ROOT",
        );
    }
    if seed {
        report.push(
            "plugins",
            "Plugin seed directory",
            "unsupported",
            "EXTERNAL_PLUGIN_ROOT",
        );
    }
    report
}
fn run(tool: Tool, source: &Path, destination: &Path, include_plugins: bool) -> R<ExtensionReport> {
    let _deadline = DeadlineGuard::enter();
    let root = Dir::open(destination)?;
    let state = root.dir(STATE, true)?;
    state.private()?;
    // OS lock protects independent AccountManager/process instances as well.
    if state.kind("lock")?.is_none() {
        match state.write_new("lock", b"", false) {
            Ok(()) => {}
            Err(e) if state.kind("lock")?.is_some() => {
                let _ = e;
            }
            Err(e) => return Err(e),
        }
    }
    let lock = state.lock("lock")?;
    let _lock = lock;
    let identity = hash(source.as_os_str().as_encoded_bytes());
    let mut manifest = if state.kind(MANIFEST)?.is_some() {
        let (bytes, _) = state.read(MANIFEST, METADATA_BYTES)?;
        let m: Manifest = serde_json::from_slice(&bytes).map_err(|_| "CORRUPT_MANIFEST")?;
        if m.version != 1 || m.root != identity || m.entries.len() > MAX_RECORDS * 4 {
            return Err("INVALID_MANIFEST");
        }
        for (k, e) in &m.entries {
            e.target.valid()?;
            if k != &e.target.key()
                || !["owned", "override", "deleted", "source_removed", "retained"]
                    .contains(&e.state.as_str())
            {
                return Err("INVALID_MANIFEST");
            }
        }
        m
    } else {
        if state.kind("tracked")?.is_some()
            || state.kind("intent.json")?.is_some()
            || root.kind(TRACKED)?.is_some()
        {
            return Err("MISSING_TRACKED_MANIFEST");
        }
        Manifest {
            version: 1,
            root: identity,
            entries: BTreeMap::new(),
        }
    };
    persist(&state, &manifest)?;
    if root.kind(TRACKED)?.is_none() {
        root.write_new(TRACKED, b"1", false)?;
    }
    if state.kind("tracked")?.is_none() {
        state.write_new("tracked", b"1", false)?;
    }
    recover(&root, &state, &mut manifest)?;
    for name in state.list()? {
        if name.starts_with("asset-") || name.starts_with("backup-") || name.starts_with("write-") {
            state.remove(&name)?;
        }
    }
    // An unavailable source is not evidence of deletion.
    let src = Dir::open(source).map_err(|_| "SOURCE_UNAVAILABLE")?;
    let mut budget = Budget::new();
    let mut scan = scan::collect(tool, &src, &root, &manifest, &mut budget, include_plugins);
    for a in &mut scan.assets {
        if let Some(value) = &mut a.leaf {
            plugins::project_paths(value, &root.path);
        }
    }
    let protected = protect_plugin_units(tool, &root, &state, &mut manifest, &mut scan)?;
    let mut keys = scan
        .assets
        .iter()
        .map(|a| a.target.key())
        .collect::<BTreeSet<_>>();
    keys.extend(protected);
    let mut failed_units = BTreeSet::new();
    for asset in &scan.assets {
        if asset
            .unit
            .as_ref()
            .is_some_and(|u| failed_units.contains(u))
        {
            scan.report.push(
                &asset.kind,
                &asset.target.path,
                "failed",
                "PLUGIN_UNIT_PREPARATION_FAILED",
            );
            continue;
        }
        if let Err(e) = budget.check() {
            scan.report
                .push(&asset.kind, &asset.target.path, "failed", e);
            scan.complete = false;
            break;
        }
        let source_valid = (|| -> R<()> {
            src.revalidate()?;
            if let Some(expected) = &asset.source_fingerprint {
                let (p, n) = src.parent(&asset.target.path, false)?;
                let snap = scan::snapshot(
                    &p,
                    &n,
                    asset.kind.contains("plugin"),
                    &mut Budget::new(),
                    true,
                )?;
                if &snap.fingerprint() != expected {
                    return Err("SOURCE_CHANGED_DURING_READ");
                }
            }
            Ok(())
        })();
        if let Err(e) = source_valid {
            if let Some(unit) = &asset.unit {
                failed_units.insert(unit.clone());
            }
            scan.report
                .push(&asset.kind, &asset.target.path, "failed", e);
            scan.complete = false;
            continue;
        }
        match reconcile_one(
            &root,
            &state,
            &mut manifest,
            Some(asset),
            &asset.target,
            &asset.kind,
        ) {
            Ok(s) => scan.report.push(&asset.kind, &asset.target.path, s, ""),
            Err(e) => {
                if let Some(unit) = &asset.unit {
                    failed_units.insert(unit.clone());
                }
                scan.report
                    .push(&asset.kind, &asset.target.path, "failed", e);
                scan.complete = false;
                // An unresolved intent must never be replaced by another asset.
                if let Err(reason) = recover(&root, &state, &mut manifest) {
                    scan.report
                        .push("extensions", "Interrupted publication", "failed", reason);
                    break;
                }
            }
        }
    }
    if scan.complete {
        let stale = manifest
            .entries
            .iter()
            .filter(|(k, _)| !keys.contains(*k))
            .map(|(_, e)| e.clone())
            .collect::<Vec<_>>();
        for e in stale {
            if e.kind == "plugin" && e.target.path.starts_with("plugins/cache/") {
                let mut retained = e.clone();
                let now = current(&root, &e.target)?;
                if ["owned", "retained"].contains(&e.state.as_str()) && now == e.written {
                    retained.state = "retained".into();
                } else if now != e.written {
                    retained.state = if now.is_none() { "deleted" } else { "override" }.into();
                }
                let status = match retained.state.as_str() {
                    "override" => "profile_override",
                    "deleted" => "profile_deleted",
                    _ => "retained",
                };
                let reason = if status == "retained" {
                    "PRIVATE_VERSION_RETAINED_FOR_READERS"
                } else {
                    "STABLE_PLUGIN_PROFILE_INTENT"
                };
                scan.report.push("plugin", &e.target.path, status, reason);
                manifest.entries.insert(e.target.key(), retained);
                persist(&state, &manifest)?;
                continue;
            }
            if let Err(reason) = budget.check() {
                scan.report.push(&e.kind, &e.target.path, "failed", reason);
                break;
            }
            match reconcile_one(&root, &state, &mut manifest, None, &e.target, &e.kind) {
                Ok(s) => scan.report.push(&e.kind, &e.target.path, s, ""),
                Err(reason) => {
                    scan.report.push(&e.kind, &e.target.path, "failed", reason);
                    if let Err(reason) = recover(&root, &state, &mut manifest) {
                        scan.report
                            .push("extensions", "Interrupted removal", "failed", reason);
                        break;
                    }
                }
            }
        }
    }
    if tool == Tool::Codex {
        scan.report.push(
            "skills",
            "HOME/.agents/skills",
            "native",
            "ALREADY_NATIVE_NO_DUPLICATION",
        );
    }
    Ok(scan.report)
}
fn persist(state: &Dir, manifest: &Manifest) -> R<()> {
    let bytes = serde_json::to_vec(manifest).map_err(|_| "INVALID_MANIFEST")?;
    if bytes.len() > METADATA_BYTES {
        return Err("METADATA_BYTES_1048576");
    }
    state.atomic(MANIFEST, &bytes)
}
fn document(root: &Dir, target: &Target) -> R<Value> {
    let (p, n) = root.parent(&target.path, true)?;
    if p.kind(&n)?.is_none() {
        return Ok(serde_json::json!({}));
    }
    let (bytes, _) = p.read(&n, METADATA_BYTES)?;
    if target.toml {
        let text = std::str::from_utf8(&bytes).map_err(|_| "INVALID_METADATA")?;
        let v: toml::Value = toml::from_str(text).map_err(|_| "INVALID_METADATA")?;
        serde_json::to_value(v).map_err(|_| "INVALID_METADATA")
    } else {
        serde_json::from_slice(&bytes).map_err(|_| "INVALID_METADATA")
    }
}
fn leaf<'a>(value: &'a Value, keys: &[String]) -> Option<&'a Value> {
    let mut v = value;
    for k in keys {
        v = v.get(k)?;
    }
    Some(v)
}
fn set_leaf(value: &mut Value, keys: &[String], new: Option<Value>) -> R<()> {
    let mut v = value;
    for k in &keys[..keys.len() - 1] {
        if v.get(k).is_none() {
            if new.is_none() {
                return Ok(());
            }
            v.as_object_mut()
                .ok_or("INVALID_PROFILE_METADATA")?
                .insert(k.clone(), serde_json::json!({}));
        }
        v = v.get_mut(k).ok_or("INVALID_PROFILE_METADATA")?;
    }
    let map = v.as_object_mut().ok_or("INVALID_PROFILE_METADATA")?;
    let k = keys.last().unwrap();
    if let Some(new) = new {
        map.insert(k.clone(), new);
    } else {
        map.remove(k);
    }
    Ok(())
}
fn current(root: &Dir, t: &Target) -> R<Option<String>> {
    if !t.leaf.is_empty() {
        let doc = document(root, t)?;
        return Ok(leaf(&doc, &t.leaf).map(|v| hash(&serde_json::to_vec(v).unwrap())));
    }
    let (p, n) = match root.parent(&t.path, false) {
        Ok(v) => v,
        Err(_) => {
            // A missing ancestor is absence; links/errors are not.
            let mut dir = Dir::open(&root.path)?;
            let mut parts = t.path.split('/').collect::<Vec<_>>();
            parts.pop();
            for c in parts {
                if dir.kind(c)?.is_none() {
                    return Ok(None);
                }
                dir = dir.dir(c, false)?;
            }
            return Err("DESTINATION_UNSAFE");
        }
    };
    if p.kind(&n)?.is_none() {
        return Ok(None);
    }
    let mut b = Budget::new();
    Ok(Some(
        scan::snapshot(&p, &n, true, &mut b, false)?.fingerprint(),
    ))
}
fn physical(root: &Dir, t: &Target) -> R<Option<String>> {
    current(root, &Target::file(t.path.clone()))
}
fn reconcile_one(
    root: &Dir,
    state: &Dir,
    m: &mut Manifest,
    a: Option<&Asset>,
    target: &Target,
    kind: &str,
) -> R<&'static str> {
    let key = target.key();
    let now = current(root, target)?;
    let old = m.entries.get(&key).cloned();
    if let Some(ref e) = old {
        if e.state == "override" || e.state == "deleted" {
            return Ok(if e.state == "override" {
                "profile_override"
            } else {
                "profile_deleted"
            });
        }
        if e.written != now {
            let mut e = e.clone();
            e.state = if now.is_none() { "deleted" } else { "override" }.into();
            m.entries.insert(key, e);
            persist(state, m)?;
            return Ok(if now.is_none() {
                "profile_deleted"
            } else {
                "profile_override"
            });
        }
    } else if now.is_some() {
        m.entries.insert(
            key,
            Entry {
                unit: a.and_then(|a| a.unit.clone()),
                target: target.clone(),
                kind: kind.into(),
                source: String::new(),
                written: now,
                state: "override".into(),
            },
        );
        persist(state, m)?;
        return Ok("profile_override");
    }
    let disable_removed = a.is_none()
        && target.toml
        && target.path == "config.toml"
        && target.leaf.len() == 3
        && old.is_some();
    let next = if disable_removed {
        Some(hash(b"false"))
    } else {
        a.map(Asset::fingerprint)
    };
    if old.as_ref().is_some_and(|e| e.written == next) {
        return Ok(if a.is_some() {
            "prepared"
        } else {
            "source_removed"
        });
    }
    if a.is_none() && old.is_none() {
        return Ok("absent");
    }
    let entry = Entry {
        unit: a
            .and_then(|a| a.unit.clone())
            .or_else(|| old.as_ref().and_then(|e| e.unit.clone())),
        target: target.clone(),
        kind: kind.into(),
        source: next.clone().unwrap_or_default(),
        written: next.clone(),
        state: if a.is_some() {
            "owned"
        } else {
            "source_removed"
        }
        .into(),
    };
    let mut proposed = m.clone();
    proposed.entries.insert(key.clone(), entry.clone());
    if proposed.entries.len() > MAX_RECORDS * 4
        || serde_json::to_vec(&proposed)
            .map_err(|_| "INVALID_MANIFEST")?
            .len()
            > METADATA_BYTES
    {
        return Err("MANIFEST_CAPACITY_1048576");
    }
    let before = physical(root, target)?;
    let stage = format!("asset-{}", nonce());
    let payload = if target.leaf.is_empty() {
        a.map(|a| a.snapshot.clone())
    } else {
        let mut doc = document(root, target)?;
        set_leaf(
            &mut doc,
            &target.leaf,
            if disable_removed {
                Some(Value::Bool(false))
            } else {
                a.and_then(|a| a.leaf.clone())
            },
        )?;
        // installed_plugins schema wrapper is a recognized constant, not a config copy.
        if target.path.ends_with("installed_plugins.json") {
            if let Some(v) = doc.get("version") {
                if v != 2 {
                    return Err("UNKNOWN_REGISTRY_VERSION");
                }
            }
            doc["version"] = 2.into();
        }
        let bytes = if target.toml {
            toml::to_string(&doc)
                .map_err(|_| "INVALID_METADATA")?
                .into_bytes()
        } else {
            serde_json::to_vec(&doc).map_err(|_| "INVALID_METADATA")?
        };
        if bytes.len() > METADATA_BYTES {
            return Err("METADATA_BYTES_1048576");
        }
        Some(Snapshot::one(bytes))
    };
    if let Some(payload) = &payload {
        payload.stage(state, &stage)?;
    }
    Budget::new().check()?;
    if let Some(a) = a {
        ownership::publication_guard(root, m, a)?;
    }
    let after = payload.as_ref().map(Snapshot::fingerprint);
    let intent = Intent {
        entry,
        before: before.clone(),
        after,
        stage: stage.clone(),
        backup: format!("backup-{}", nonce()),
    };
    state.atomic(
        "intent.json",
        &serde_json::to_vec(&intent).map_err(|_| "INVALID_MANIFEST")?,
    )?;
    if current(root, target)? != now || physical(root, target)? != before {
        return Err("CONCURRENT_PROFILE_EDIT");
    }
    let (p, n) = root.parent(&target.path, true)?;
    match (before.is_some(), payload.is_some()) {
        (true, true) => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            state.swap(&stage, &p, &n)?;
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                p.rename_checked(
                    &n,
                    state,
                    &intent.backup,
                    before.as_deref().ok_or("CORRUPT_INTENT")?,
                )?;
                if let Err(e) = state.rename_checked(
                    &stage,
                    &p,
                    &n,
                    intent.after.as_deref().ok_or("CORRUPT_INTENT")?,
                ) {
                    let _ = state.rename_checked(
                        &intent.backup,
                        &p,
                        &n,
                        before.as_deref().ok_or("CORRUPT_INTENT")?,
                    );
                    return Err(e);
                }
            }
        }
        (false, true) => {
            #[cfg(windows)]
            state.rename_checked(
                &stage,
                &p,
                &n,
                intent.after.as_deref().ok_or("CORRUPT_INTENT")?,
            )?;
            #[cfg(not(windows))]
            state.rename(&stage, &p, &n)?;
        }
        (true, false) => {
            #[cfg(windows)]
            p.rename_checked(
                &n,
                state,
                &intent.backup,
                before.as_deref().ok_or("CORRUPT_INTENT")?,
            )?;
            #[cfg(not(windows))]
            p.rename(&n, state, &intent.backup)?;
        }
        (false, false) => {}
    }
    m.entries.insert(key, intent.entry.clone());
    persist(state, m)?;
    #[cfg(windows)]
    cleanup_windows_intent(state, &intent)?;
    #[cfg(not(windows))]
    {
        state.remove("intent.json")?;
        state.remove(&stage)?;
        state.remove(&intent.backup)?;
    }
    Ok(if a.is_some() {
        "prepared"
    } else {
        "source_removed"
    })
}
fn recover(root: &Dir, state: &Dir, m: &mut Manifest) -> R<()> {
    if state.kind("intent.json")?.is_none() {
        return Ok(());
    }
    let (bytes, _) = state.read("intent.json", METADATA_BYTES)?;
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| "CORRUPT_INTENT")?;
    intent.entry.target.valid()?;
    fs::component(&intent.stage)?;
    fs::component(&intent.backup)?;
    if !intent.stage.starts_with("asset-") || !intent.backup.starts_with("backup-") {
        return Err("CORRUPT_INTENT");
    }
    let now = physical(root, &intent.entry.target)?;
    if now == intent.after {
        let (parent, _) = root.parent(&intent.entry.target.path, false)?;
        parent.sync()?;
        state.sync()?;
        m.entries
            .insert(intent.entry.target.key(), intent.entry.clone());
        persist(state, m)?;
    } else if now.is_none() && intent.before.is_some() && state.kind(&intent.backup)?.is_some() {
        let (p, n) = root.parent(&intent.entry.target.path, true)?;
        #[cfg(windows)]
        state.rename_checked(
            &intent.backup,
            &p,
            &n,
            intent.before.as_deref().ok_or("CORRUPT_INTENT")?,
        )?;
        #[cfg(not(windows))]
        state.rename(&intent.backup, &p, &n)?;
    } else if now != intent.before {
        #[cfg(windows)]
        if intent.before.is_some()
            && state.kind(&intent.backup)?.is_some()
            && (intent.after.is_none() || state.kind(&intent.stage)?.is_some())
        {
            return Err("RECOVERY_DESTINATION_COLLISION");
        }
        let mut e = intent.entry.clone();
        e.state = if now.is_none() { "deleted" } else { "override" }.into();
        e.written = current(root, &e.target)?;
        m.entries.insert(e.target.key(), e);
        persist(state, m)?;
    }
    #[cfg(windows)]
    cleanup_windows_intent(state, &intent)?;
    #[cfg(not(windows))]
    {
        state.remove("intent.json")?;
        state.remove(&intent.stage)?;
        state.remove(&intent.backup)?;
    }
    Ok(())
}
#[cfg(windows)]
fn cleanup_windows_intent(state: &Dir, intent: &Intent) -> R<()> {
    for (name, expected) in [
        (&intent.stage, &intent.after),
        (&intent.backup, &intent.before),
    ] {
        if state.kind(name)?.is_some() {
            state.remove_checked(name, expected.as_deref().ok_or("CORRUPT_INTENT")?)?;
        }
    }
    // A failed ownership check or supported flush preserves the recovery intent.
    state.remove("intent.json")
}
#[cfg(test)]
mod tests;
