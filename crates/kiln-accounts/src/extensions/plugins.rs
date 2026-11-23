use super::*;
use scan::{read_json, snapshot, validate_portable};
pub(super) fn obj<'a>(v: &'a Value, allowed: &[&str]) -> R<&'a serde_json::Map<String, Value>> {
    let map = v.as_object().ok_or("UNKNOWN_PLUGIN_SCHEMA")?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err("UNKNOWN_PLUGIN_SCHEMA");
    }
    Ok(map)
}
pub(super) fn text<'a>(v: &'a Value, k: &str) -> R<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or("UNKNOWN_PLUGIN_SCHEMA")
}
pub(super) fn strings(v: &Value, keys: &[&str]) -> R<()> {
    for key in keys {
        if let Some(value) = v.get(*key) {
            if !value.is_string() {
                return Err("UNKNOWN_PLUGIN_SCHEMA");
            }
        }
    }
    Ok(())
}
pub(super) fn boolean(v: &Value, key: &str) -> R<()> {
    if v.get(key).is_some_and(|v| !v.is_boolean()) {
        return Err("UNKNOWN_PLUGIN_SCHEMA");
    }
    Ok(())
}
fn author(v: &Value) -> R<()> {
    obj(v, &["name", "email", "url"])?;
    strings(v, &["name", "email", "url"])
}
fn plugin_id(id: &str) -> R<(&str, &str)> {
    let (n, m) = id.split_once('@').ok_or("INVALID_PLUGIN_ID")?;
    fs::component(n)?;
    fs::component(m)?;
    if ["inline", "synced", "skills-dir"].contains(&m) {
        return Err("NATIVE_ACCOUNT_OR_INLINE_PLUGIN");
    }
    Ok((n, m))
}
pub(super) fn validate_bundle(s: &Snapshot) -> R<Value> {
    let f = s
        .files
        .iter()
        .find(|f| f.path == ".claude-plugin/plugin.json")
        .ok_or("PLUGIN_MANIFEST_MISSING")?;
    let v: Value = serde_json::from_slice(&f.bytes).map_err(|_| "INVALID_PLUGIN_MANIFEST")?;
    obj(
        &v,
        &[
            "$schema",
            "name",
            "displayName",
            "version",
            "description",
            "author",
            "homepage",
            "repository",
            "license",
            "keywords",
            "defaultEnabled",
            "dependencies",
            "skills",
            "commands",
            "agents",
            "outputStyles",
            "workflows",
            "hooks",
            "mcpServers",
            "lspServers",
            "userConfig",
        ],
    )?;
    fs::component(text(&v, "name")?)?;
    strings(
        &v,
        &[
            "$schema",
            "name",
            "displayName",
            "version",
            "description",
            "homepage",
            "repository",
            "license",
        ],
    )?;
    boolean(&v, "defaultEnabled")?;
    if let Some(v) = v.get("author") {
        author(v)?;
    }
    components::claude(s, &v)?;
    Ok(v)
}
pub(super) fn activation_leaf(settings: &Value, id: &str) -> R<Option<Asset>> {
    let Some(map) = settings.get("enabledPlugins") else {
        return Ok(None);
    };
    let map = map.as_object().ok_or("UNKNOWN_ACTIVATION_SCHEMA")?;
    if map.len() > MAX_RECORDS {
        return Err("ACTIVATION_RECORDS_1024");
    }
    match map.get(id) {
        None => Ok(None),
        Some(Value::Bool(v)) => Ok(Some(leaf_asset(
            "plugin_activation",
            "settings.json",
            vec!["enabledPlugins".into(), id.into()],
            Value::Bool(*v),
            false,
        ))),
        _ => Err("UNKNOWN_ACTIVATION_SCHEMA"),
    }
}
fn leaf_asset(kind: &str, path: &str, keys: Vec<String>, value: Value, toml: bool) -> Asset {
    Asset {
        unit: None,
        kind: kind.into(),
        target: Target {
            path: path.into(),
            leaf: keys,
            toml,
        },
        snapshot: Snapshot::one(vec![]),
        leaf: Some(value),
        source_fingerprint: None,
    }
}
#[derive(Clone)]
struct Plugin {
    id: String,
    rel: String,
    snapshot: Snapshot,
    manifest: Value,
    record: Value,
    market: String,
    market_record: Value,
    market_manifest: Value,
    market_rel: String,
    market_payload: Option<(String, Snapshot)>,
    enabled: Option<bool>,
}
pub(super) fn claude(
    source: &Dir,
    destination: &Dir,
    ownership: &Manifest,
    scan: &mut Scan,
    b: &mut Budget,
) {
    match source.kind("plugins") {
        Ok(None) => return,
        Ok(Some(_)) => {}
        Err(e) => {
            scan.error("plugins", "Installed plugins", e);
            return;
        }
    }
    let result = (|| -> R<()> {
        let root = source.dir("plugins", false)?;
        if root.kind("synced")?.is_some() {
            scan.report
                .push("plugins", "synced", "native_account", "NATIVE_ACCOUNT_SYNC");
        }
        if root.kind("installed_plugins.json")?.is_none() {
            return Ok(());
        }
        let registry = read_json(&root, "installed_plugins.json")?;
        obj(&registry, &["version", "plugins"])?;
        if registry.get("version") != Some(&Value::from(2)) {
            return Err("UNKNOWN_REGISTRY_VERSION");
        }
        let installed = registry
            .get("plugins")
            .and_then(Value::as_object)
            .ok_or("UNKNOWN_PLUGIN_SCHEMA")?;
        if installed.len() > MAX_RECORDS {
            return Err("REGISTRY_RECORDS_1024");
        }
        let known = if root.kind("known_marketplaces.json")?.is_some() {
            read_json(&root, "known_marketplaces.json")?
        } else {
            serde_json::json!({})
        };
        let settings = if source.kind("settings.json")?.is_some() {
            read_json(source, "settings.json")?
        } else {
            serde_json::json!({})
        };
        let activation = settings.get("enabledPlugins").and_then(Value::as_object);
        if settings.get("enabledPlugins").is_some() && activation.is_none() {
            return Err("UNKNOWN_ACTIVATION_SCHEMA");
        }
        if activation.is_some_and(|a| a.len() > MAX_RECORDS) {
            return Err("ACTIVATION_RECORDS_1024");
        }
        let mut plugins = BTreeMap::new();
        let mut count = 0;
        for (id, records) in installed {
            let result = (|| -> R<Plugin> {
                let (name, market) = plugin_id(id)?;
                // A preexisting scoped declaration can cause the native loader
                // to re-fetch outside our private registry. Do not silently
                // rewrite an unowned profile declaration or claim cache parity.
                if settings
                    .get("extraKnownMarketplaces")
                    .and_then(|v| v.get(market))
                    .is_some()
                {
                    return Err("EXPLICIT_MARKETPLACE_DECLARATION_REQUIRES_NATIVE_SETUP");
                }
                let records = records.as_array().ok_or("UNKNOWN_PLUGIN_SCHEMA")?;
                count += records.len();
                if count > MAX_RECORDS {
                    return Err("REGISTRY_RECORDS_1024");
                }
                let users = records
                    .iter()
                    .filter(|r| r.get("scope").and_then(Value::as_str) == Some("user"))
                    .collect::<Vec<_>>();
                if users.len() != 1 {
                    return Err("PROJECT_OR_UNKNOWN_INSTALL_SCOPE");
                }
                let record = users[0];
                obj(
                    record,
                    &[
                        "scope",
                        "installPath",
                        "version",
                        "installedAt",
                        "lastUpdated",
                        "gitCommitSha",
                        "resolvedVersion",
                        "auto",
                        "copyHash",
                    ],
                )?;
                strings(
                    record,
                    &[
                        "scope",
                        "installPath",
                        "version",
                        "installedAt",
                        "lastUpdated",
                        "gitCommitSha",
                        "resolvedVersion",
                        "copyHash",
                    ],
                )?;
                boolean(record, "auto")?;
                let path = Path::new(text(record, "installPath")?);
                let rel = fs::registry_relative(&root.path, path)?;
                if !rel.starts_with("cache/") {
                    return Err("EXTERNAL_PLUGIN_ROOT");
                }
                let (p, n) = root.parent(&rel, false)?;
                let snapshot = snapshot(&p, &n, true, b, true)?;
                validate_portable(&snapshot, &source.path)?;
                let mut manifest = validate_bundle(&snapshot)?;
                let market_record = known.get(market).ok_or("MARKETPLACE_METADATA_MISSING")?;
                obj(
                    market_record,
                    &["source", "installLocation", "lastUpdated", "autoUpdate"],
                )?;
                strings(market_record, &["installLocation", "lastUpdated"])?;
                boolean(market_record, "autoUpdate")?;
                let source_descriptor = market_record
                    .get("source")
                    .ok_or("UNSAFE_MARKETPLACE_METADATA")?;
                validate_market_source(source_descriptor)?;
                let location = Path::new(text(market_record, "installLocation")?);
                let market_rel = fs::registry_relative(&root.path, location)?;
                if !market_rel.starts_with("marketplaces/") {
                    return Err("EXTERNAL_PLUGIN_ROOT");
                }
                let market_manifest = read_json(
                    &root,
                    &format!("{market_rel}/.claude-plugin/marketplace.json"),
                )?;
                obj(
                    &market_manifest,
                    &[
                        "name",
                        "owner",
                        "plugins",
                        "metadata",
                        "allowCrossMarketplaceDependenciesOn",
                        "$schema",
                        "description",
                        "version",
                    ],
                )?;
                strings(
                    &market_manifest,
                    &["name", "$schema", "description", "version"],
                )?;
                author(
                    market_manifest
                        .get("owner")
                        .ok_or("UNKNOWN_PLUGIN_SCHEMA")?,
                )?;
                if let Some(metadata) = market_manifest.get("metadata") {
                    obj(metadata, &["description", "version", "pluginRoot"])?;
                    strings(metadata, &["description", "version", "pluginRoot"])?;
                    if let Some(path) = metadata.get("pluginRoot").and_then(Value::as_str) {
                        fs::relative(path.trim_start_matches("./"))?;
                    }
                }
                if let Some(names) = market_manifest.get("allowCrossMarketplaceDependenciesOn") {
                    for name in names.as_array().ok_or("UNKNOWN_PLUGIN_SCHEMA")? {
                        fs::component(name.as_str().ok_or("UNKNOWN_PLUGIN_SCHEMA")?)?;
                    }
                }
                if text(&market_manifest, "name")? != market {
                    return Err("MARKETPLACE_IDENTITY_MISMATCH");
                }
                let entries = market_manifest
                    .get("plugins")
                    .and_then(Value::as_array)
                    .ok_or("UNKNOWN_PLUGIN_SCHEMA")?;
                if entries.len() > MAX_RECORDS {
                    return Err("REGISTRY_RECORDS_1024");
                }
                let matches = entries
                    .iter()
                    .filter(|e| e.get("name").and_then(Value::as_str) == Some(name))
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err("MARKETPLACE_IDENTITY_MISMATCH");
                }
                let entry = matches[0];
                obj(
                    entry,
                    &[
                        "name",
                        "source",
                        "version",
                        "description",
                        "author",
                        "homepage",
                        "repository",
                        "license",
                        "keywords",
                        "strict",
                        "dependencies",
                    ],
                )?;
                // Only a self-contained relative marketplace entry is portable;
                // fetch producers, URLs and install modes require distinct policy.
                strings(
                    entry,
                    &[
                        "name",
                        "source",
                        "version",
                        "description",
                        "homepage",
                        "repository",
                        "license",
                    ],
                )?;
                boolean(entry, "strict")?;
                if let Some(v) = entry.get("author") {
                    author(v)?;
                }
                let plugin_source = text(entry, "source")?
                    .strip_prefix("./")
                    .ok_or("UNSAFE_MARKETPLACE_METADATA")?;
                fs::relative(plugin_source)?;
                let market_payload = if source_descriptor["source"] == "directory" {
                    if fs::registry_relative(
                        &root.path,
                        Path::new(text(source_descriptor, "path")?),
                    )? != market_rel
                    {
                        return Err("EXTERNAL_PLUGIN_ROOT");
                    }
                    let relative = format!("{market_rel}/{plugin_source}");
                    let (parent, name) = root.parent(&relative, false)?;
                    let actual = scan::snapshot(&parent, &name, true, b, true)?;
                    validate_portable(&actual, &source.path)?;
                    manifest = validate_bundle(&actual)?;
                    Some((format!("plugins/{relative}"), actual))
                } else {
                    None
                };
                let enabled = match activation.and_then(|a| a.get(id)) {
                    None => None,
                    Some(Value::Bool(b)) => Some(*b),
                    _ => return Err("UNKNOWN_ACTIVATION_SCHEMA"),
                };
                Ok(Plugin {
                    id: id.clone(),
                    rel: format!("plugins/{rel}"),
                    snapshot,
                    manifest,
                    record: record.clone(),
                    market: market.into(),
                    market_record: market_record.clone(),
                    market_manifest,
                    market_rel: format!("plugins/{market_rel}"),
                    market_payload,
                    enabled,
                })
            })();
            match result {
                Ok(p) => {
                    plugins.insert(id.clone(), p);
                }
                Err("EXPLICIT_MARKETPLACE_DECLARATION_REQUIRES_NATIVE_SETUP") => {
                    scan.complete = false;
                    scan.report.push(
                        "plugin",
                        id,
                        "unsupported",
                        "EXPLICIT_MARKETPLACE_DECLARATION_REQUIRES_NATIVE_SETUP",
                    );
                }
                Err(e) => scan.error("plugin", id, e),
            }
        }
        let mut valid = BTreeSet::new();
        for id in plugins.keys() {
            let mut visited = BTreeSet::new();
            let mut stack = BTreeSet::new();
            match closure(id, &plugins, &mut visited, &mut stack, 0) {
                Ok(()) => {
                    let mut blocked = false;
                    for dependency in visited.iter().filter(|dep| *dep != id) {
                        let target = Target {
                            path: "settings.json".into(),
                            leaf: vec!["enabledPlugins".into(), dependency.clone()],
                            toml: false,
                        };
                        let doc = document(destination, &target)?;
                        let tombstoned = ownership
                            .entries
                            .get(&target.key())
                            .is_some_and(|e| e.state == "deleted");
                        let disabled = leaf(&doc, &target.leaf).is_some_and(|v| v == false);
                        let registry = Target {
                            path: "plugins/installed_plugins.json".into(),
                            leaf: vec!["plugins".into(), dependency.clone()],
                            toml: false,
                        };
                        let removed = ownership
                            .entries
                            .get(&registry.key())
                            .is_some_and(|e| e.state == "deleted");
                        if tombstoned || disabled || removed {
                            blocked = true;
                            break;
                        }
                    }
                    if blocked {
                        scan.error("plugin", id, "DEPENDENCY_PROFILE_OVERRIDE");
                    } else {
                        scan.dependencies.insert(
                            format!("claude:user:{id}"),
                            visited
                                .iter()
                                .filter(|dep| *dep != id)
                                .map(|dep| format!("claude:user:{dep}"))
                                .collect(),
                        );
                        valid.insert(id.clone());
                    }
                }
                Err(e) => scan.error("plugin", id, e),
            }
        }
        let mut markets = BTreeSet::new();
        for id in &valid {
            let p = &plugins[id];
            let group_start = scan.assets.len();
            // Native path fields refer only to the selected private namespace.
            let mut record = p.record.clone();
            record["installPath"] = Value::String("$DESTINATION".to_string() + "/" + &p.rel);
            scan.add(Asset::tree("plugin", p.rel.clone(), p.snapshot.clone()))?;
            if let Some((path, snapshot)) = &p.market_payload {
                scan.add(Asset::tree("plugin_source", path.clone(), snapshot.clone()))?;
            }
            scan.add(leaf_asset(
                "plugin_registry",
                "plugins/installed_plugins.json",
                vec!["plugins".into(), p.id.clone()],
                Value::Array(vec![record]),
                false,
            ))?;
            if let Some(enabled) = p.enabled {
                scan.add(leaf_asset(
                    "plugin_activation",
                    "settings.json",
                    vec!["enabledPlugins".into(), p.id.clone()],
                    Value::Bool(enabled),
                    false,
                ))?;
            }
            for asset in &mut scan.assets[group_start..] {
                asset.unit = Some(format!("claude:user:{id}"));
            }
            if markets.insert(p.market.clone()) {
                let mut manifest = p.market_manifest.clone();
                let entries = manifest["plugins"]
                    .as_array_mut()
                    .ok_or("UNKNOWN_PLUGIN_SCHEMA")?;
                entries.retain(|e| {
                    e.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| valid.contains(&format!("{n}@{}", p.market)))
                });
                // Keep source descriptors/IDs unchanged. Only install locations
                // are path projections, never an @inline identity conversion.
                let mut record = p.market_record.clone();
                record["installLocation"] =
                    Value::String("$DESTINATION".to_string() + "/" + &p.market_rel);
                if record["source"]["source"] == "directory" {
                    let original = text(&record["source"], "path")?;
                    if original != text(&p.market_record, "installLocation")? {
                        return Err("EXTERNAL_PLUGIN_ROOT");
                    }
                    record["source"]["path"] =
                        Value::String("$DESTINATION".to_string() + "/" + &p.market_rel);
                }
                let mut descriptor = Asset::tree(
                    "marketplace",
                    format!("{}/.claude-plugin/marketplace.json", p.market_rel),
                    Snapshot::one(serde_json::to_vec(&manifest).map_err(|_| "INVALID_METADATA")?),
                );
                descriptor.source_fingerprint = None;
                scan.add(descriptor)?;
                scan.add(leaf_asset(
                    "marketplace_registry",
                    "plugins/known_marketplaces.json",
                    vec![p.market.clone()],
                    record,
                    false,
                ))?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        scan.error("plugins", "Installed plugins", e);
    }
}
fn validate_market_source(source: &Value) -> R<()> {
    strings(source, &["source", "path", "repo", "ref", "url"])?;
    match text(source, "source")? {
        "directory" => {
            obj(source, &["source", "path"])?;
            if !Path::new(text(source, "path")?).is_absolute() {
                return Err("UNSAFE_MARKETPLACE_METADATA");
            }
        }
        "github" => {
            obj(source, &["source", "repo", "ref", "path"])?;
            let repo = text(source, "repo")?;
            if repo.split('/').count() != 2 || repo.split('/').any(|p| fs::component(p).is_err()) {
                return Err("UNSAFE_MARKETPLACE_METADATA");
            }
        }
        "git" => {
            obj(source, &["source", "url", "ref", "path"])?;
            let url =
                url::Url::parse(text(source, "url")?).map_err(|_| "UNSAFE_MARKETPLACE_METADATA")?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
            {
                return Err("UNSAFE_MARKETPLACE_METADATA");
            }
        }
        _ => return Err("UNSAFE_MARKETPLACE_METADATA"),
    }
    Ok(())
}
fn closure(
    id: &str,
    plugins: &BTreeMap<String, Plugin>,
    visited: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    depth: usize,
) -> R<()> {
    if depth > MAX_DEPENDENCY_HOPS || visited.len() >= MAX_DEPENDENCY_PLUGINS {
        return Err("DEPENDENCY_CLOSURE_LIMIT");
    }
    if stack.contains(id) {
        return Err("DEPENDENCY_CYCLE");
    }
    if visited.contains(id) {
        return Ok(());
    }
    stack.insert(id.into());
    let p = plugins.get(id).ok_or("DEPENDENCY_UNRESOLVED")?;
    let entry = p.market_manifest["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"].as_str() == id.split_once('@').map(|(n, _)| n))
        .unwrap();
    for declarations in [p.manifest.get("dependencies"), entry.get("dependencies")]
        .into_iter()
        .flatten()
    {
        for dependency in declarations.as_array().ok_or("DEPENDENCY_UNRESOLVED")? {
            let (dep, version) = if let Some(s) = dependency.as_str() {
                (
                    if s.contains('@') {
                        s.into()
                    } else {
                        format!("{s}@{}", p.market)
                    },
                    None,
                )
            } else {
                obj(dependency, &["name", "version", "marketplace"])?;
                (
                    format!(
                        "{}@{}",
                        text(dependency, "name")?,
                        dependency
                            .get("marketplace")
                            .and_then(Value::as_str)
                            .unwrap_or(&p.market)
                    ),
                    dependency.get("version").and_then(Value::as_str),
                )
            };
            plugin_id(&dep)?;
            let target = plugins.get(&dep).ok_or("DEPENDENCY_UNRESOLVED")?;
            if target.enabled != Some(true) {
                return Err("DEPENDENCY_DISABLED_OR_UNDECLARED");
            }
            if let Some(version) = version {
                let normalized = if version.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                    semver::Version::parse(version).map_err(|_| "UNSUPPORTED_DEPENDENCY_RANGE")?;
                    format!("={version}")
                } else {
                    version.to_owned()
                };
                let range = semver::VersionReq::parse(&normalized)
                    .map_err(|_| "UNSUPPORTED_DEPENDENCY_RANGE")?;
                let installed = semver::Version::parse(text(&target.record, "version")?)
                    .map_err(|_| "DEPENDENCY_UNRESOLVED")?;
                if !range.matches(&installed) {
                    return Err("DEPENDENCY_VERSION_MISMATCH");
                }
            }
            closure(&dep, plugins, visited, stack, depth + 1)?;
        }
    }
    stack.remove(id);
    visited.insert(id.into());
    Ok(())
}
pub(super) fn codex(source: &Dir, scan: &mut Scan, b: &mut Budget) {
    let result = (|| -> R<()> {
        if source.kind("config.toml")?.is_none() {
            return Ok(());
        }
        let (bytes, _) = source.read("config.toml", METADATA_BYTES)?;
        let config: toml::Value =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|_| "INVALID_METADATA")?)
                .map_err(|_| "INVALID_METADATA")?;
        let Some(plugins) = config.get("plugins").and_then(toml::Value::as_table) else {
            return Ok(());
        };
        if plugins.len() > MAX_RECORDS {
            return Err("REGISTRY_RECORDS_1024");
        }
        for (id, configuration) in plugins {
            if id == codex_chrome::PLUGIN {
                // The reserved marketplace and its native runtime belong to Codex.
                continue;
            }
            let result = (|| -> R<()> {
                let (name, market) = plugin_id(id)?;
                let fields = configuration
                    .as_table()
                    .ok_or("UNKNOWN_ACTIVATION_SCHEMA")?;
                if fields.keys().any(|k| k != "enabled") {
                    return Err("NATIVE_ACCOUNT_OR_CONFIGURED_PLUGIN");
                }
                let enabled = fields.get("enabled").and_then(toml::Value::as_bool);
                if fields.contains_key("enabled") && enabled.is_none() {
                    return Err("UNKNOWN_ACTIVATION_SCHEMA");
                }
                let base = format!("plugins/cache/{market}/{name}");
                let (parent, leaf) = source.parent(&base, false)?;
                let cache = parent.dir(&leaf, false)?;
                let mut versions = Vec::new();
                for v in cache.list()? {
                    if v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
                        && cache.kind(&v)? == Some(Kind::Dir)
                    {
                        versions.push(v);
                    }
                }
                versions.sort_by(|a, b| {
                    match (semver::Version::parse(a), semver::Version::parse(b)) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        _ => a.cmp(b),
                    }
                });
                let version = if versions.iter().any(|v| v == "local") {
                    "local".into()
                } else {
                    versions.pop().ok_or("PLUGIN_PAYLOAD_MISSING")?
                };
                let dir = cache.dir(&version, false)?;
                if dir.kind(".codex-remote-plugin-install.json")?.is_some() {
                    scan.report
                        .push("plugin", id, "native_account", "NATIVE_ACCOUNT_SYNC");
                    return Ok(());
                }
                let snap = snapshot(&cache, &version, true, b, true)?;
                validate_portable(&snap, &source.path)?;
                validate_codex_bundle(&snap)?;
                // The exact native store chooses local, otherwise highest parsed
                // version. Do not mirror unused versions or any remote sidecar.
                let group_start = scan.assets.len();
                scan.add(Asset::tree("plugin", format!("{base}/{version}"), snap))?;
                if let Some(enabled) = enabled {
                    scan.add(leaf_asset(
                        "plugin_activation",
                        "config.toml",
                        vec!["plugins".into(), id.clone(), "enabled".into()],
                        Value::Bool(enabled),
                        true,
                    ))?;
                }
                for asset in &mut scan.assets[group_start..] {
                    asset.unit = Some(format!("codex:user:{id}"));
                }
                Ok(())
            })();
            if let Err(e) = result {
                scan.error("plugin", id, e);
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        scan.error("plugins", "Codex extensions", e);
    }
}
fn validate_codex_bundle(s: &Snapshot) -> R<()> {
    let root = s.files.iter().find(|f| f.path == "plugin.json");
    let legacy = s
        .files
        .iter()
        .find(|f| f.path == ".codex-plugin/plugin.json");
    let v: Value = serde_json::from_slice(&root.or(legacy).ok_or("PLUGIN_MANIFEST_MISSING")?.bytes)
        .map_err(|_| "INVALID_PLUGIN_MANIFEST")?;
    if root.is_some() {
        if v["$schema"] != "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json" {
            return Err("UNKNOWN_PLUGIN_SCHEMA");
        }
        obj(
            &v,
            &[
                "$schema",
                "name",
                "version",
                "description",
                "author",
                "homepage",
                "repository",
                "license",
                "keywords",
                "extensions",
            ],
        )?;
    } else {
        obj(
            &v,
            &[
                "name",
                "version",
                "description",
                "keywords",
                "skills",
                "commands",
                "interface",
                "extensions",
                "mcpServers",
                "hooks",
                "apps",
            ],
        )?;
    }
    fs::component(text(&v, "name")?)?;
    components::codex(s, &v, root.is_some())?;
    Ok(())
}

pub(super) fn project_paths(value: &mut Value, destination: &Path) {
    if let Some(records) = value.as_array_mut() {
        for record in records {
            project_paths(record, destination);
        }
    }
    if let Some(map) = value.as_object_mut() {
        for key in ["installPath", "installLocation", "path"] {
            if let Some(Value::String(path)) = map.get_mut(key) {
                if let Some(relative) = path.strip_prefix("$DESTINATION/") {
                    *path = destination.join(relative).to_string_lossy().into_owned();
                }
            }
        }
        if let Some(source) = map.get_mut("source") {
            project_paths(source, destination);
        }
    }
}
