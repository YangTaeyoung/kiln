//! Stable provider/native-ID/scope protection across versioned payload targets.
use super::*;
fn inferred(tool: Tool, e: &Entry) -> Option<String> {
    if e.target.path == "plugins/installed_plugins.json" && e.target.leaf.len() == 2 {
        return Some(format!("claude:user:{}", e.target.leaf[1]));
    }
    if e.kind == "plugin_activation" && e.target.leaf.len() >= 2 {
        return Some(format!("{}:user:{}", tool.key(), e.target.leaf[1]));
    }
    let parts = e.target.path.split('/').collect::<Vec<_>>();
    if parts.len() == 5 && parts[..2] == ["plugins", "cache"] && e.kind == "plugin" {
        return Some(format!("{}:user:{}@{}", tool.key(), parts[3], parts[2]));
    }
    None
}
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}
pub(super) fn protect_plugin_units(
    tool: Tool,
    root: &Dir,
    state: &Dir,
    m: &mut Manifest,
    scan: &mut Scan,
) -> R<BTreeSet<String>> {
    // Older v1 manifests predate explicit unit identity. Existing path ownership
    // is migrated only where the verified native cache layout proves the ID.
    for e in m.entries.values_mut() {
        if e.unit.is_none() {
            e.unit = scan
                .assets
                .iter()
                .find(|a| a.target == e.target)
                .and_then(|a| a.unit.clone())
                .or_else(|| inferred(tool, e));
        }
    }
    let units = scan
        .assets
        .iter()
        .filter_map(|a| a.unit.clone())
        .chain(m.entries.values().filter_map(|e| e.unit.clone()))
        .collect::<BTreeSet<_>>();
    let mut blocked = BTreeMap::new();
    for unit in &units {
        for e in m.entries.values_mut().filter(|e| {
            e.unit.as_ref() == Some(unit)
                && ["plugin", "plugin_source", "skills_plugin"].contains(&e.kind.as_str())
        }) {
            let now = current(root, &e.target)?;
            if e.state == "deleted" {
                blocked.insert(unit.clone(), "profile_deleted");
                break;
            }
            if e.state == "override" {
                blocked.insert(unit.clone(), "profile_override");
                break;
            }
            if now != e.written {
                let state = if now.is_none() { "deleted" } else { "override" };
                e.state = state.into();
                blocked.insert(
                    unit.clone(),
                    if now.is_none() {
                        "profile_deleted"
                    } else {
                        "profile_override"
                    },
                );
                break;
            }
        }
        // Initial adoption must not activate a new version over an unowned one.
        for a in scan
            .assets
            .iter()
            .filter(|a| a.unit.as_ref() == Some(unit) && a.kind == "plugin")
        {
            let base = a
                .target
                .path
                .rsplit_once('/')
                .ok_or("INVALID_ASSET_PATH")?
                .0;
            let (parent, name) = match root.parent(base, false) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if parent.kind(&name)?.is_none() {
                continue;
            }
            let versions = parent.dir(&name, false)?;
            for name in versions.list()? {
                if versions.kind(&name)? != Some(Kind::Dir) {
                    continue;
                }
                let t = Target::file(format!("{base}/{name}"));
                if !m.entries.contains_key(&t.key()) {
                    let now = current(root, &t)?;
                    m.entries.insert(
                        t.key(),
                        Entry {
                            unit: Some(unit.clone()),
                            target: t,
                            kind: "plugin".into(),
                            source: String::new(),
                            written: now,
                            state: "override".into(),
                        },
                    );
                    blocked.insert(unit.clone(), "profile_override");
                } else if tool == Tool::Codex && t.path != a.target.path {
                    let requested = a.target.path.rsplit('/').next().unwrap();
                    if name == "local"
                        || requested != "local" && version_order(&name, requested).is_gt()
                    {
                        blocked.insert(unit.clone(), "selection_conflict");
                    }
                }
            }
        }
    }
    // Count retained immutable payloads with the proposed inventory. No implicit
    // cache GC: refusing an over-budget new version leaves existing readers safe.
    let selected = scan
        .assets
        .iter()
        .map(|a| a.target.key())
        .collect::<BTreeSet<_>>();
    let mut budget = Budget::new();
    budget.files = scan
        .assets
        .iter()
        .map(|a| a.snapshot.files.len() + a.snapshot.dirs.len())
        .sum();
    budget.bytes = scan
        .assets
        .iter()
        .map(|a| {
            a.snapshot
                .files
                .iter()
                .map(|f| f.bytes.len())
                .sum::<usize>()
        })
        .sum();
    let mut count = scan.assets.len();
    for e in m
        .entries
        .values()
        .filter(|e| e.kind == "plugin" && !selected.contains(&e.target.key()))
    {
        if current(root, &e.target)?.is_none() {
            continue;
        }
        let (p, n) = root.parent(&e.target.path, false)?;
        if let Err(reason) = scan::snapshot(&p, &n, true, &mut budget, false).and_then(|s| {
            budget.files += s.files.len() + s.dirs.len();
            budget.bytes += s.files.iter().map(|f| f.bytes.len()).sum::<usize>();
            count += 1;
            if count > TOTAL_ASSETS || budget.files > TOTAL_FILES || budget.bytes > TOTAL_BYTES {
                Err("RETAINED_PLUGIN_CAPACITY")
            } else {
                Ok(())
            }
        }) {
            for unit in &units {
                blocked.insert(unit.clone(), "retained_capacity");
            }
            scan.report.push(
                "plugins",
                "Retained private versions",
                "unsupported",
                reason,
            );
            break;
        }
    }
    // Native dependency closure was validated during inventory. A protected old
    // dependency must also protect its dependents before any new pointer advances.
    loop {
        let mut added = false;
        for (unit, deps) in &scan.dependencies {
            if !blocked.contains_key(unit) && deps.iter().any(|dep| blocked.contains_key(dep)) {
                blocked.insert(unit.clone(), "dependency_override");
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let mut protected = BTreeSet::new();
    for (unit, state_name) in &blocked {
        protected.extend(
            m.entries
                .values()
                .filter(|e| e.unit.as_ref() == Some(unit))
                .map(|e| e.target.key()),
        );
        scan.report.push(
            "plugin",
            unit,
            if [
                "selection_conflict",
                "retained_capacity",
                "dependency_override",
            ]
            .contains(state_name)
            {
                "unsupported"
            } else {
                state_name
            },
            match *state_name {
                "selection_conflict" => "NATIVE_VERSION_SELECTION_CONFLICT",
                "retained_capacity" => "RETAINED_PLUGIN_CAPACITY",
                "dependency_override" => "DEPENDENCY_PROFILE_OVERRIDE",
                _ => "STABLE_PLUGIN_PROFILE_INTENT",
            },
        );
    }
    // Shared native selection metadata belongs to every protected unit that
    // references it, even when the source no longer proposes that marketplace.
    let mut markets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for unit in blocked.keys() {
        if let Some((name, market)) = unit
            .strip_prefix("claude:user:")
            .and_then(|id| id.split_once('@'))
        {
            markets
                .entry(market.into())
                .or_default()
                .insert(name.into());
        }
    }
    for (market, names) in markets {
        budget.check()?;
        let Some(registration) = m
            .entries
            .values()
            .find(|e| e.kind == "marketplace_registry" && e.target.leaf == [market.clone()])
        else {
            continue;
        };
        protected.insert(registration.target.key());
        // Keep the prior origin/install location binding. Source updates for other
        // plugins merge into the descriptor referenced by this private registration.
        scan.assets.retain(|a| a.target != registration.target);
        let doc = document(root, &registration.target)?;
        let Some(record) = leaf(&doc, &registration.target.leaf) else {
            continue;
        };
        let Some(location) = record.get("installLocation").and_then(Value::as_str) else {
            continue;
        };
        let relative = Path::new(location)
            .strip_prefix(&root.path)
            .map_err(|_| "EXTERNAL_PLUGIN_ROOT")?;
        let relative = relative
            .components()
            .map(|part| part.as_os_str().to_str().ok_or("INVALID_ASSET_PATH"))
            .collect::<R<Vec<_>>>()?
            .join("/");
        let target = Target::file(format!("{relative}/.claude-plugin/marketplace.json"));
        target.valid()?;
        if !m
            .entries
            .get(&target.key())
            .is_some_and(|e| e.kind == "marketplace")
        {
            continue;
        }
        protected.insert(target.key());
        let Some(asset) = scan.assets.iter_mut().find(|a| a.target == target) else {
            continue;
        };
        if current(root, &target)?.is_none() {
            continue;
        }
        let old = scan::read_json(root, &target.path)?;
        let mut proposed: Value = serde_json::from_slice(&asset.snapshot.files[0].bytes)
            .map_err(|_| "INVALID_METADATA")?;
        let old_entries = old
            .get("plugins")
            .and_then(Value::as_array)
            .ok_or("INVALID_METADATA")?;
        let entries = proposed
            .get_mut("plugins")
            .and_then(Value::as_array_mut)
            .ok_or("INVALID_METADATA")?;
        // Merge from the old side too: retain_mut alone cannot restore an entry
        // removed from the new catalog while its private payload stays protected.
        entries.retain(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| !names.contains(name))
        });
        entries.extend(
            old_entries
                .iter()
                .filter(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| names.contains(name))
                })
                .cloned(),
        );
        if entries.len() > MAX_RECORDS {
            return Err("REGISTRY_RECORDS_1024");
        }
        let bytes = serde_json::to_vec(&proposed).map_err(|_| "INVALID_METADATA")?;
        if bytes.len() > METADATA_BYTES {
            return Err("METADATA_BYTES_1048576");
        }
        budget.bytes = budget
            .bytes
            .saturating_sub(asset.snapshot.files[0].bytes.len())
            + bytes.len();
        if budget.bytes > TOTAL_BYTES {
            return Err("TOTAL_BYTES_268435456");
        }
        asset.snapshot = Snapshot::one(bytes);
    }
    scan.assets
        .retain(|a| a.unit.as_ref().is_none_or(|u| !blocked.contains_key(u)));
    persist(state, m)?;
    Ok(protected)
}

/// Recheck the selected stable unit after staging, immediately before publishing
/// either a new native version or its registry/activation pointer.
pub(super) fn publication_guard(root: &Dir, m: &Manifest, a: &Asset) -> R<()> {
    let Some(unit) = &a.unit else {
        return Ok(());
    };
    for e in m.entries.values().filter(|e| {
        e.unit.as_ref() == Some(unit)
            && ["plugin", "plugin_source", "skills_plugin"].contains(&e.kind.as_str())
    }) {
        if e.state == "override" || e.state == "deleted" || current(root, &e.target)? != e.written {
            return Err("CONCURRENT_PLUGIN_PROFILE_EDIT");
        }
    }
    if a.kind == "plugin" && a.target.path.rsplit('/').next() != Some("local") {
        if let Some(e) = m.entries.get(&a.target.key()) {
            if ["owned", "retained"].contains(&e.state.as_str())
                && e.written.as_deref() != Some(a.fingerprint().as_str())
            {
                return Err("NATIVE_VERSION_PAYLOAD_CHANGED");
            }
        }
    }
    Ok(())
}
