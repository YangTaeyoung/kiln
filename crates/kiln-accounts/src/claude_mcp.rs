//! Inherit native user/local MCP registrations without sharing account identities.
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

// Only fingerprints are saved here: MCP definitions can contain credentials.
// A null fingerprint records an account override, including a local deletion.
type Imports = BTreeMap<String, Option<String>>;
const MANIFEST: &str = "kiln-mcp-imports.json";
const PREFERENCES: [&str; 4] = [
    "disabledMcpServers",
    "enabledMcpServers",
    "disabledMcpjsonServers",
    "enabledMcpjsonServers",
];

fn fingerprint(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}

fn read(path: &Path) -> Result<Value> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("Invalid Claude configuration: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(error)
            .with_context(|| format!("Cannot read Claude configuration: {}", path.display())),
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn scope(doc: &Value, prefix: &str, leaves: &mut BTreeMap<String, Value>) -> Result<()> {
    ensure!(doc.is_object(), "Invalid Claude MCP scope");
    if let Some(servers) = doc.get("mcpServers") {
        let servers = servers
            .as_object()
            .context("Invalid Claude MCP server map")?;
        for (name, value) in servers {
            leaves.insert(
                format!("{prefix}/mcpServers/{}", escape(name)),
                value.clone(),
            );
        }
    }
    for field in PREFERENCES {
        if let Some(value) = doc.get(field) {
            ensure!(value.is_array(), "Invalid Claude MCP preference");
            leaves.insert(format!("{prefix}/{field}"), value.clone());
        }
    }
    Ok(())
}

fn projection(doc: &Value) -> Result<BTreeMap<String, Value>> {
    let mut leaves = BTreeMap::new();
    scope(doc, "", &mut leaves)?;
    if let Some(projects) = doc.get("projects") {
        for (project, value) in projects.as_object().context("Invalid Claude project map")? {
            scope(
                value,
                &format!("/projects/{}", escape(project)),
                &mut leaves,
            )?;
        }
    }
    Ok(leaves)
}

fn set(doc: &mut Value, pointer: &str, value: Option<&Value>) -> Result<()> {
    let keys: Vec<_> = pointer
        .split('/')
        .skip(1)
        .map(|key| key.replace("~1", "/").replace("~0", "~"))
        .collect();
    let (last, parents) = keys.split_last().context("Invalid MCP import path")?;
    let mut parent = doc;
    for key in parents {
        let map = parent
            .as_object_mut()
            .context("Invalid account MCP configuration")?;
        if value.is_none() && !map.contains_key(key) {
            return Ok(());
        }
        parent = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
    }
    let map = parent
        .as_object_mut()
        .context("Invalid account MCP configuration")?;
    if let Some(value) = value {
        map.insert(last.clone(), value.clone());
    } else {
        map.remove(last);
    }
    Ok(())
}

fn valid_import(pointer: &str) -> bool {
    let parts: Vec<_> = pointer.split('/').collect();
    match parts.as_slice() {
        ["", "mcpServers", _] | ["", "projects", _, "mcpServers", _] => true,
        ["", field] | ["", "projects", _, field] => PREFERENCES.contains(field),
        _ => false,
    }
}

/// Refresh inherited entries, preserving profile edits and all non-MCP fields.
/// Missing source files leave previous imports intact; an explicit empty map removes
/// unchanged imports. Project paths remain scoped by the official CLI, not flattened.
pub(crate) fn inherit(source: &Path, runtime: &Path) -> Result<()> {
    if !source.try_exists()? {
        return Ok(());
    }
    let source_doc = read(source)?;
    let incoming = projection(&source_doc)?;
    fs::create_dir_all(runtime)?;
    let lock_path = runtime.join("kiln-mcp-imports.lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lock_path)?;
    lock.lock()?;
    let manifest = runtime.join(MANIFEST);
    let previous: Imports = serde_json::from_value(read(&manifest)?)?;
    ensure!(
        previous.keys().all(|pointer| valid_import(pointer)),
        "Invalid MCP import manifest"
    );
    let destination = crate::claude::config_path(runtime);
    let mut doc = read(&destination)?;
    let before = doc.clone();
    let paths: BTreeSet<_> = previous.keys().chain(incoming.keys()).collect();
    let mut next = Imports::new();
    for pointer in paths {
        let current = doc.pointer(pointer);
        let inherited = match previous.get(pointer) {
            Some(Some(hash)) => current.map(fingerprint).as_ref() == Some(hash),
            Some(None) => false,
            None => current.is_none(),
        };
        if inherited {
            set(&mut doc, pointer, incoming.get(pointer))?;
            if let Some(value) = incoming.get(pointer) {
                next.insert(pointer.clone(), Some(fingerprint(value)));
            }
        } else {
            next.insert(pointer.clone(), None);
        }
    }
    if doc != before {
        crate::store::write_secret_atomic(&destination, &serde_json::to_vec(&doc)?)?;
    }
    if next != previous {
        crate::store::write_secret_atomic(&manifest, &serde_json::to_vec(&next)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(path: &Path, doc: &Value) {
        crate::store::write_secret_atomic(path, &serde_json::to_vec(doc).unwrap()).unwrap();
    }

    #[test]
    fn refreshes_inherited_servers_but_preserves_account_edits_deletions_and_identity() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.json");
        let runtime = root.path().join("runtime");
        let config = runtime.join(".claude.json");
        let first = json!({"mcpServers": {
            "updated": {"command": "old"}, "removed": {"command": "old"},
            "edited": {"command": "old"}, "deleted": {"command": "old"},
            "preexisting": {"command": "source"}
        }});
        write(&source, &first);
        write(
            &config,
            &json!({"oauthAccount": {"emailAddress": "account@example.test"},
                "mcpServers": {"preexisting": {"command": "account"}},
                "projects": {"/work/~project": {"hasTrustDialogAccepted": false}}
            }),
        );
        inherit(&source, &runtime).unwrap();
        let mut edited = read(&config).unwrap();
        edited["mcpServers"]["edited"]["command"] = "account".into();
        edited["mcpServers"]
            .as_object_mut()
            .unwrap()
            .remove("deleted");
        write(&config, &edited);
        let second = json!({"mcpServers": {
            "updated": {"command": "new", "env": {"SECRET": "fixture-secret"}},
            "edited": {"command": "new"}, "deleted": {"command": "new"},
            "preexisting": {"command": "new"}
        }, "projects": {"/work/~project": {
            "mcpServers": {"local/~server": {"command": "local"}},
            "disabledMcpServers": ["local/~server"], "hasTrustDialogAccepted": true
        }}, "oauthAccount": {"emailAddress": "source@example.test"}});
        write(&source, &second);
        inherit(&source, &runtime).unwrap();
        let actual = read(&config).unwrap();
        assert_eq!(
            actual["mcpServers"]["updated"],
            second["mcpServers"]["updated"]
        );
        assert!(actual["mcpServers"].get("removed").is_none());
        assert!(actual["mcpServers"].get("deleted").is_none());
        assert_eq!(actual["mcpServers"]["edited"]["command"], "account");
        assert_eq!(actual["mcpServers"]["preexisting"]["command"], "account");
        assert_eq!(
            actual["projects"]["/work/~project"]["mcpServers"],
            second["projects"]["/work/~project"]["mcpServers"]
        );
        assert_eq!(
            actual["projects"]["/work/~project"]["hasTrustDialogAccepted"],
            false
        );
        assert_eq!(
            actual["oauthAccount"]["emailAddress"],
            "account@example.test"
        );
        assert!(
            !fs::read_to_string(runtime.join(MANIFEST))
                .unwrap()
                .contains("fixture-secret")
        );
        let before = fs::metadata(&config).unwrap().modified().unwrap();
        inherit(&source, &runtime).unwrap();
        assert_eq!(
            fs::metadata(&config).unwrap().modified().unwrap(),
            before,
            "unchanged launches do not rewrite CLI state"
        );
        fs::remove_file(&source).unwrap();
        inherit(&source, &runtime).unwrap();
        assert_eq!(read(&config).unwrap(), actual);
        write(&source, &json!({}));
        inherit(&source, &runtime).unwrap();
        let cleared = read(&config).unwrap();
        assert!(cleared["mcpServers"].get("updated").is_none());
        assert_eq!(cleared["mcpServers"]["edited"]["command"], "account");
        assert!(
            cleared["projects"]["/work/~project"]["mcpServers"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&config, &runtime.join(MANIFEST)] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn legacy_config_is_preserved_and_malformed_input_cannot_replace_account_state() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.json");
        let runtime = root.path().join("runtime");
        let config = runtime.join(".config.json");
        write(
            &config,
            &json!({"oauthAccount": {"emailAddress": "kept@example.test"}}),
        );
        write(
            &source,
            &json!({"mcpServers": {"fixture": {"command": "fixture"}}}),
        );
        inherit(&source, &runtime).unwrap();
        assert!(!runtime.join(".claude.json").exists());
        let before = fs::read(&config).unwrap();
        fs::write(&source, b"{invalid").unwrap();
        assert!(inherit(&source, &runtime).is_err());
        assert_eq!(fs::read(&config).unwrap(), before);
        write(&source, &json!({}));
        write(&runtime.join(MANIFEST), &json!({"/oauthAccount": "bad"}));
        assert!(inherit(&source, &runtime).is_err());
        assert_eq!(fs::read(&config).unwrap(), before);
    }
}
