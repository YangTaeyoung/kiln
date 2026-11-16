use super::*;
fn excluded(name: &str) -> bool {
    matches!(
        name,
        "auth.json"
            | "settings.json"
            | "settings.local.json"
            | "config.toml"
            | ".claude.json"
            | ".config.json"
            | "agent-memory"
            | "agent-memory-local"
            | "sessions"
            | "history.jsonl"
            | ".install-manifests"
            | ".codex-remote-plugin-install.json"
            | "scheduled_tasks.json"
            | ".state"
            | ".env"
            | ".trash"
            | ".git"
    ) || name.starts_with(".credentials")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with(".env.")
}
pub(super) fn snapshot(
    parent: &Dir,
    name: &str,
    plugin: bool,
    budget: &mut Budget,
    source: bool,
) -> R<Snapshot> {
    let mut files = vec![];
    let mut dirs = vec![];
    let mut bytes = 0;
    let mut normalized = BTreeSet::new();
    let kind = parent.kind(name)?.ok_or("SOURCE_CHANGED_DURING_READ")?;
    let directory = kind == Kind::Dir;
    if directory {
        let dir = parent.dir(name, false)?;
        walk(
            &dir,
            "",
            &mut files,
            &mut dirs,
            &mut bytes,
            &mut normalized,
            plugin,
            budget,
            source,
            0,
        )?;
    } else if kind == Kind::File {
        let (data, exec) = parent.read(
            name,
            if name.ends_with(".md") || name.ends_with(".json") || name.ends_with(".toml") {
                METADATA_BYTES
            } else {
                FILE_BYTES
            },
        )?;
        bytes = data.len();
        files.push(FileData {
            path: String::new(),
            bytes: data,
            exec,
        });
    } else {
        return Err(if kind == Kind::Link {
            "UNSAFE_SYMBOLIC_LINK"
        } else {
            "SPECIAL_FILE"
        });
    }
    if source {
        budget.files += files.len() + dirs.len();
        budget.bytes += bytes;
        if budget.files > TOTAL_FILES {
            return Err("TOTAL_FILES_16384");
        }
        if budget.bytes > TOTAL_BYTES {
            return Err("TOTAL_BYTES_268435456");
        }
    }
    Ok(Snapshot {
        directory,
        files,
        dirs,
    })
}
fn walk(
    dir: &Dir,
    prefix: &str,
    out: &mut Vec<FileData>,
    dirs: &mut Vec<String>,
    bytes: &mut usize,
    names: &mut BTreeSet<String>,
    plugin: bool,
    b: &Budget,
    source: bool,
    depth: usize,
) -> R<()> {
    b.check()?;
    if depth > MAX_DEPTH {
        return Err("DEPTH_16");
    }
    for name in dir.list()? {
        fs::component(&name)?;
        if source && excluded(&name) {
            return Err("EXCLUDED_ACCOUNT_OR_SECRET_STATE");
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if !names.insert(path.nfc().collect::<String>().to_lowercase()) {
            return Err("CASE_OR_UNICODE_COLLISION");
        }
        match dir.kind(&name)?.ok_or("SOURCE_CHANGED_DURING_READ")? {
            Kind::Dir => {
                dirs.push(path.clone());
                walk(
                    &dir.dir(&name, false)?,
                    &path,
                    out,
                    dirs,
                    bytes,
                    names,
                    plugin,
                    b,
                    source,
                    depth + 1,
                )?;
            }
            Kind::File => {
                let (data, exec) = dir.read(
                    &name,
                    if name.ends_with(".md") || name.ends_with(".json") || name.ends_with(".toml") {
                        METADATA_BYTES
                    } else {
                        FILE_BYTES
                    },
                )?;
                *bytes += data.len();
                out.push(FileData {
                    path,
                    bytes: data,
                    exec,
                });
            }
            Kind::Link => return Err("UNSAFE_SYMBOLIC_LINK"),
            Kind::Special => return Err("SPECIAL_FILE"),
        }
        if out.len() + dirs.len() > if plugin { PLUGIN_FILES } else { ASSET_FILES } {
            return Err(if plugin {
                "PLUGIN_FILES_4096"
            } else {
                "ASSET_FILES_2048"
            });
        }
        if *bytes > if plugin { PLUGIN_BYTES } else { ASSET_BYTES } {
            return Err(if plugin {
                "PLUGIN_BYTES_67108864"
            } else {
                "ASSET_BYTES_16777216"
            });
        }
    }
    Ok(())
}
pub(super) fn read_json(root: &Dir, path: &str) -> R<Value> {
    let (p, n) = root.parent(path, false)?;
    let (b, _) = p.read(&n, METADATA_BYTES)?;
    serde_json::from_slice(&b).map_err(|_| "INVALID_METADATA")
}
pub(super) fn collect(
    tool: Tool,
    source: &Dir,
    destination: &Dir,
    manifest: &Manifest,
    budget: &mut Budget,
    include_plugins: bool,
) -> Scan {
    let mut scan = Scan::new();
    let activation: R<Value> = if tool == Tool::Claude {
        (|| {
            if source.kind("settings.json")?.is_none() {
                Ok(serde_json::json!({}))
            } else {
                read_json(source, "settings.json")
            }
        })()
    } else {
        Ok(serde_json::json!({}))
    };
    match source.kind("skills") {
        Ok(Some(Kind::Dir)) => match source
            .dir("skills", false)
            .and_then(|dir| skills(&dir, "skills", tool, &activation, &mut scan, budget, 0))
        {
            Ok(()) => {}
            Err(e) => scan.error("skills", "skills", e),
        },
        Ok(None) => {}
        Ok(_) => scan.error("skills", "skills", "UNSAFE_SKILL_ROOT"),
        Err(e) => scan.error("skills", "skills", e),
    }
    if tool == Tool::Claude {
        for kind in ["commands", "agents", "output-styles", "workflows"] {
            let result = (|| -> R<()> {
                if source.kind(kind)?.is_none() {
                    return Ok(());
                }
                let dir = source.dir(kind, false)?;
                let list = dir.list()?;
                // A directory with helpers is one validated containing asset. No
                // independent file copy can leave its relative helper behind.
                let extension = if kind == "workflows" { "js" } else { "md" };
                let bundle = list.iter().any(|n| {
                    dir.kind(n).ok() == Some(Some(Kind::File))
                        && !n.ends_with(&format!(".{extension}"))
                });
                if bundle {
                    return Err("ROOT_HELPER_BUNDLE_REQUIRES_SUBDIRECTORY");
                }
                for name in list {
                    let result = (|| -> R<()> {
                        budget.check()?;
                        if excluded(&name) {
                            return Err("EXCLUDED_ACCOUNT_OR_SECRET_STATE");
                        }
                        let snap = snapshot(&dir, &name, false, budget, true)?;
                        validate_portable(&snap, &source.path)?;
                        scan.add(Asset::tree(kind, format!("{kind}/{name}"), snap))
                    })();
                    if let Err(e) = result {
                        scan.error(kind, &name, e);
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                scan.error(kind, kind, e);
            }
        }
        for (name, reason, state) in [
            ("routines", "UNVERIFIED_ROUTINE_FORMAT", "unsupported"),
            (
                "scheduled-tasks",
                "NATIVE_ACCOUNT_SCHEDULER",
                "native_account",
            ),
            (
                "scheduled_tasks.json",
                "NATIVE_ACCOUNT_SCHEDULER",
                "native_account",
            ),
        ] {
            if source.kind(name).ok().flatten().is_some() {
                scan.report.push(name, name, state, reason);
            }
        }
        if include_plugins {
            plugins::claude(source, destination, manifest, &mut scan, budget);
        } else {
            scan.complete = false;
        }
    } else if tool == Tool::Codex {
        plugins::codex(source, &mut scan, budget);
    }
    scan
}
fn skills(
    dir: &Dir,
    prefix: &str,
    tool: Tool,
    activation: &R<Value>,
    scan: &mut Scan,
    b: &mut Budget,
    depth: usize,
) -> R<()> {
    if depth > MAX_DEPTH {
        return Err("DEPTH_16");
    }
    b.check()?;
    for name in dir.list()? {
        if name == "synced" {
            scan.report.push(
                "skills",
                &format!("{prefix}/{name}"),
                "native_account",
                "NATIVE_ACCOUNT_SYNC",
            );
            continue;
        }
        if name == ".system" || name == ".trash" || name == ".install-manifests" {
            continue;
        }
        fs::component(&name)?;
        let path = format!("{prefix}/{name}");
        let result = (|| -> R<()> {
            let child = dir.dir(&name, false)?;
            let plugin = tool == Tool::Claude && child.kind(".claude-plugin")?.is_some();
            if child.kind("SKILL.md")?.is_some() || plugin {
                let snap = snapshot(dir, &name, plugin, b, true)?;
                validate_portable(&snap, &dir.path)?;
                let mut asset = Asset::tree(
                    if plugin { "skills_plugin" } else { "skills" },
                    path.clone(),
                    snap,
                );
                if plugin {
                    let manifest = plugins::validate_bundle(&asset.snapshot)?;
                    let id = format!(
                        "{}@skills-dir",
                        manifest["name"].as_str().ok_or("INVALID_PLUGIN_MANIFEST")?
                    );
                    let settings = activation.as_ref().map_err(|e| *e)?;
                    let leaf = plugins::activation_leaf(settings, &id)?;
                    if scan.assets.len() + 1 + usize::from(leaf.is_some()) > TOTAL_ASSETS {
                        return Err("TOTAL_ASSETS_512");
                    }
                    asset.unit = Some(format!("claude:user:{id}"));
                    // Establish activation before exposing a default-enabled bundle.
                    // A failed leaf blocks this unit's later payload; explicit local
                    // overrides/tombstones still use the normal ownership semantics.
                    if let Some(mut leaf) = leaf {
                        leaf.unit = Some(format!("claude:user:{id}"));
                        scan.add(leaf)?;
                    }
                    scan.add(asset)?;
                } else {
                    scan.add(asset)?;
                }
            } else {
                skills(&child, &path, tool, activation, scan, b, depth + 1)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            scan.error("skills", &path, e);
        }
    }
    Ok(())
}
/// Explicit old-root/helper references cannot be repaired by rewriting instructions.
/// This is a portability check, not a claim to detect all embedded secrets.
pub(super) fn validate_portable(snapshot: &Snapshot, source: &Path) -> R<()> {
    let prefix = source.to_string_lossy();
    for file in &snapshot.files {
        if let Ok(text) = std::str::from_utf8(&file.bytes) {
            if !snapshot.directory
                && [
                    "from './",
                    "from \"./",
                    "require('./",
                    "require(\"./",
                    "](./",
                    "](../",
                ]
                .iter()
                .any(|p| text.contains(p))
            {
                return Err("RELATIVE_HELPER_REQUIRES_BUNDLE");
            }
            if text.contains(prefix.as_ref())
                || text.contains("${CLAUDE_PLUGIN_DATA}")
                || text.contains("${user_config.")
            {
                return Err("NONPORTABLE_ASSET_REFERENCE");
            }
        }
    }
    Ok(())
}
