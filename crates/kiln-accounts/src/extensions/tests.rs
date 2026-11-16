use super::*;
use std::fs;
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let source = root.join("source");
    let dest = root.join("profile");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    (tmp, source, dest)
}
fn put(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
#[test]
fn imports_whole_asset_updates_and_distinguishes_deletion_tombstones() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/nested/demo/SKILL.md",
        "---\nname: demo\ndescription: demo\n---\none",
    );
    put(&source, "skills/nested/demo/helper.txt", "support");
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(
        fs::read_to_string(dest.join("skills/nested/demo/helper.txt")).unwrap(),
        "support"
    );
    put(&source, "skills/nested/demo/helper.txt", "two");
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(
        fs::read_to_string(dest.join("skills/nested/demo/helper.txt")).unwrap(),
        "two"
    );
    fs::remove_dir_all(dest.join("skills/nested/demo")).unwrap();
    reconcile(Tool::Claude, &source, &dest);
    fs::remove_dir_all(source.join("skills/nested/demo")).unwrap();
    reconcile(Tool::Claude, &source, &dest);
    put(&source, "skills/nested/demo/SKILL.md", "reappeared");
    reconcile(Tool::Claude, &source, &dest);
    assert!(!dest.join("skills/nested/demo").exists());
}
#[test]
fn source_removal_and_profile_override_are_distinct() {
    let (_tmp, source, dest) = fixture();
    put(&source, "agents/helper.md", "one");
    reconcile(Tool::Claude, &source, &dest);
    fs::remove_file(source.join("agents/helper.md")).unwrap();
    reconcile(Tool::Claude, &source, &dest);
    assert!(!dest.join("agents/helper.md").exists());
    put(&source, "agents/helper.md", "two");
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(
        fs::read_to_string(dest.join("agents/helper.md")).unwrap(),
        "two"
    );
    put(&dest, "agents/helper.md", "local");
    put(&source, "agents/helper.md", "three");
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(
        fs::read_to_string(dest.join("agents/helper.md")).unwrap(),
        "local"
    );
}
#[test]
fn excludes_account_state_and_preserves_native_codex_home_skill_scope() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/local/SKILL.md", "local");
    put(&source, "skills/.system/secret/SKILL.md", "SYSTEM_CANARY");
    put(&source, "config.toml", "token = 'CONFIG_CANARY'");
    put(&source, "auth.json", "AUTH_CANARY");
    reconcile(Tool::Codex, &source, &dest);
    assert!(dest.join("skills/local/SKILL.md").exists());
    assert!(!dest.join("skills/.system").exists());
    assert!(!dest.join("config.toml").exists());
    assert!(!dest.join("auth.json").exists());
}
fn plugin_fixture(source: &Path) {
    put(
        source,
        "plugins/cache/test-market/inert/1.0.0/.claude-plugin/plugin.json",
        r#"{"name":"inert","version":"1.0.0","description":"Inert fixture"}"#,
    );
    put(
        source,
        "plugins/cache/test-market/inert/1.0.0/skills/plugin-proof/SKILL.md",
        "---\nname: plugin-proof\ndescription: Inert plugin discovery only\n---\nNever execute anything.\n",
    );
    put(
        source,
        "plugins/marketplaces/test-market/inert/.claude-plugin/plugin.json",
        r#"{"name":"inert","version":"1.0.0","description":"Inert fixture"}"#,
    );
    put(
        source,
        "plugins/marketplaces/test-market/inert/skills/plugin-proof/SKILL.md",
        "---\nname: plugin-proof\ndescription: Inert plugin discovery only\n---\nNever execute anything.\n",
    );
    put(
        source,
        "plugins/marketplaces/test-market/.claude-plugin/marketplace.json",
        r#"{"name":"test-market","owner":{"name":"Fixture"},"plugins":[{"name":"inert","source":"./inert","version":"1.0.0"}]}"#,
    );
    put(source,"plugins/known_marketplaces.json",&serde_json::json!({"test-market":{"source":{"source":"directory","path":source.join("plugins/marketplaces/test-market")},"installLocation":source.join("plugins/marketplaces/test-market"),"lastUpdated":"2026-11-15T00:00:00Z","autoUpdate":false}}).to_string());
    put(source,"plugins/installed_plugins.json",&serde_json::json!({"version":2,"plugins":{"inert@test-market":[{"scope":"user","installPath":source.join("plugins/cache/test-market/inert/1.0.0"),"version":"1.0.0","installedAt":"2026-11-15T00:00:00Z","lastUpdated":"2026-11-15T00:00:00Z"}]}}).to_string());
    put(
        source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true},"env":{"TOKEN":"SETTINGS_CANARY"}}"#,
    );
}
#[test]
fn installed_plugin_keeps_identity_paths_false_and_local_leaves() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    put(
        &dest,
        "settings.json",
        r#"{"enabledPlugins":{"local@own":false},"env":{"OWN":"PROFILE_CANARY"}}"#,
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        !report.entries.iter().any(|r| r.state == "failed"),
        "{report:?}"
    );
    let registry: Value =
        serde_json::from_slice(&fs::read(dest.join("plugins/installed_plugins.json")).unwrap())
            .unwrap();
    assert_eq!(
        registry["plugins"]["inert@test-market"][0]["installPath"],
        dest.join("plugins/cache/test-market/inert/1.0.0")
            .to_string_lossy()
            .as_ref()
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["enabledPlugins"]["local@own"], false);
    assert_eq!(settings["enabledPlugins"]["inert@test-market"], true);
    assert!(settings["env"].get("TOKEN").is_none());
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":false}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    let settings: Value =
        serde_json::from_slice(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["enabledPlugins"]["inert@test-market"], false);
    let mut settings = settings;
    settings["enabledPlugins"]
        .as_object_mut()
        .unwrap()
        .remove("inert@test-market");
    put(&dest, "settings.json", &settings.to_string());
    reconcile(Tool::Claude, &source, &dest);
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    let settings: Value =
        serde_json::from_slice(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
    assert!(
        settings["enabledPlugins"]
            .get("inert@test-market")
            .is_none()
    );
    let manifest = fs::read_to_string(dest.join(STATE).join(MANIFEST)).unwrap();
    for canary in ["SETTINGS_CANARY", "PROFILE_CANARY", "Never execute"] {
        assert!(!manifest.contains(canary));
    }
}
#[test]
fn missing_manifest_and_corruption_never_reset_tracking() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "one");
    reconcile(Tool::Claude, &source, &dest);
    fs::remove_dir_all(dest.join("skills/demo")).unwrap();
    fs::remove_file(dest.join(STATE).join(MANIFEST)).unwrap();
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "MISSING_TRACKED_MANIFEST")
    );
    assert!(!dest.join("skills/demo").exists());
    put(&dest, &format!("{STATE}/{MANIFEST}"), "broken");
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "CORRUPT_MANIFEST")
    );
    assert!(!dest.join("skills/demo").exists());
}
#[test]
fn source_partial_failure_never_removes_previous_good_assets() {
    let (_tmp, source, dest) = fixture();
    put(&source, "agents/demo.md", "one");
    reconcile(Tool::Claude, &source, &dest);
    fs::remove_file(source.join("agents/demo.md")).unwrap();
    put(
        &source,
        "skills/huge/SKILL.md",
        &"x".repeat(METADATA_BYTES + 1),
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.entries.iter().any(|e| e.reason == "FILE_SIZE_LIMIT"));
    assert!(dest.join("agents/demo.md").exists());
    assert!(!dest.join("skills/huge").exists());
}
#[cfg(unix)]
#[test]
fn no_write_or_read_through_destination_or_source_links() {
    use std::os::unix::fs::symlink;
    let (tmp, source, dest) = fixture();
    let outside = tmp.path().canonicalize().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    put(&outside, "SKILL.md", "OUTSIDE_CANARY");
    fs::create_dir(source.join("skills")).unwrap();
    symlink(&outside, source.join("skills/escape")).unwrap();
    put(&source, "agents/demo.md", "safe");
    symlink(&outside, dest.join("agents")).unwrap();
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.entries.iter().any(|e| e.state == "failed"));
    assert!(!outside.join("demo.md").exists());
    assert!(!dest.join("skills/escape").exists());
}
#[test]
fn plugin_unknown_metadata_external_root_and_missing_dependency_are_visible() {
    for mode in ["unknown", "external", "dependency"] {
        let (_tmp, source, dest) = fixture();
        plugin_fixture(&source);
        if mode == "dependency" {
            for path in [
                "plugins/cache/test-market/inert/1.0.0/.claude-plugin/plugin.json",
                "plugins/marketplaces/test-market/inert/.claude-plugin/plugin.json",
            ] {
                put(
                    &source,
                    path,
                    r#"{"name":"inert","dependencies":["missing"]}"#,
                );
            }
        } else {
            let path = source.join("plugins/installed_plugins.json");
            let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let record = &mut value["plugins"]["inert@test-market"][0];
            if mode == "external" {
                record["installPath"] = "/outside/plugin".into();
            } else {
                record["futureProvenance"] = "unsafe".into();
            }
            fs::write(path, value.to_string()).unwrap();
        }
        let report = reconcile(Tool::Claude, &source, &dest);
        assert!(report.entries.iter().any(|e| e.state == "failed"), "{mode}");
        assert!(!dest.join("plugins/cache").exists());
        assert!(!dest.join("settings.json").exists());
    }
}
#[test]
fn account_owned_and_unverified_types_are_not_misrepresented() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/synced/remote/SKILL.md", "REMOTE_CANARY");
    put(&source, "routines/a.md", "ROUTINE_CANARY");
    put(&source, "scheduled-tasks/a/SKILL.md", "SCHEDULE_CANARY");
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "NATIVE_ACCOUNT_SYNC")
    );
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "UNVERIFIED_ROUTINE_FORMAT")
    );
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "NATIVE_ACCOUNT_SCHEDULER")
    );
    assert!(!dest.join("skills/synced").exists());
    assert!(!dest.join("routines").exists());
}
/// Explicit opt-in native inert parser proof. Never sends a user/model request.
#[test]
#[ignore = "requires installed official Claude CLI; isolated HOME, no hooks/account/model"]
fn native_inert_claude_discovery_with_source_unavailable() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    let (tmp, source, dest) = fixture();
    plugin_fixture(&source);
    for base in [
        "plugins/cache/test-market/inert/1.0.0",
        "plugins/marketplaces/test-market/inert",
    ] {
        put(
            &source,
            &format!("{base}/.claude-plugin/plugin.json"),
            r#"{"name":"inert","version":"1.0.0","commands":{"extra-proof":{"content":"Never execute anything.","description":"Inert inline command"}},"hooks":{},"mcpServers":{},"lspServers":{}}"#,
        );
    }
    put(
        &source,
        "skills/disabled/.claude-plugin/plugin.json",
        r#"{"name":"disabled","defaultEnabled":true,"commands":{"disabled-proof":{"content":"Never execute anything.","description":"Disabled fixture"}}}"#,
    );
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true,"disabled@skills-dir":false}}"#,
    );
    put(
        &source,
        "skills/plain-proof/SKILL.md",
        "---\nname: plain-proof\ndescription: Plain inert discovery fixture\n---\nDo not run anything.\n",
    );
    put(
        &source,
        "commands/legacy-proof.md",
        "---\ndescription: Legacy inert discovery fixture\n---\nDo not run anything.\n",
    );
    put(
        &source,
        "agents/agent-proof.md",
        "---\nname: agent-proof\ndescription: Agent inert discovery fixture\n---\nDo not run anything.\n",
    );
    put(
        &source,
        "output-styles/style-proof.md",
        "---\nname: style-proof\ndescription: Style inert discovery fixture\n---\nBe concise.\n",
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        !report.entries.iter().any(|e| e.state == "failed"),
        "{report:?}"
    );
    let hidden = source.with_file_name("source-unavailable");
    fs::rename(&source, &hidden).unwrap();
    assert!(!source.exists());
    let home = tmp.path().canonicalize().unwrap().join("home");
    let workspace = tmp.path().canonicalize().unwrap().join("workspace");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&workspace).unwrap();
    let cli = std::env::var("KILN_TEST_CLAUDE_BIN").expect("explicit official CLI path");
    let mut command = Command::new(cli);
    command
        .env_clear()
        .env("HOME", &home)
        .env("CLAUDE_CONFIG_DIR", &dest)
        .env("CLAUDE_CODE_PLUGIN_CACHE_DIR", dest.join("plugins"))
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("ANTHROPIC_API_KEY", "inert-fixture-not-a-key")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .current_dir(&workspace);
    command.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--setting-sources",
        "user,project",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--no-session-persistence",
    ]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let result = (|| -> Result<Value, String> {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin,"{}",serde_json::json!({"type":"control_request","request_id":"inert-initialize","request":{"subtype":"initialize"}})).unwrap();
        stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let v = rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            if v["type"] == "control_response" && v["response"]["request_id"] == "inert-initialize"
            {
                return Ok(v);
            }
        }
        Err("initialize timeout".into())
    })();
    let _ = child.kill();
    let _ = child.wait();
    let response = result.unwrap();
    let data = &response["response"]["response"];
    let names = |key: &str| {
        data.get(key).and_then(Value::as_array).map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("name").or_else(|| r.get("type")))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let safe = serde_json::json!({"commands":names("commands"),"agents":names("agents"),"styles":data.get("available_output_styles"),"plugin_errors":data.get("plugin_errors")});
    let diagnostic = Command::new(std::env::var("KILN_TEST_CLAUDE_BIN").unwrap())
        .env_clear()
        .env("HOME", &home)
        .env("CLAUDE_CONFIG_DIR", &dest)
        .env("CLAUDE_CODE_PLUGIN_CACHE_DIR", dest.join("plugins"))
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .current_dir(&workspace)
        .args(["plugin", "list", "--json"])
        .output()
        .unwrap();
    println!(
        "Synthetic plugin list: {}",
        String::from_utf8_lossy(&diagnostic.stdout)
    );
    println!(
        "Synthetic plugin errors: {}",
        String::from_utf8_lossy(&diagnostic.stderr)
    );
    println!("Native inert discovery (synthetic only): {safe}");
    let text = safe.to_string();
    for name in [
        "plain-proof",
        "legacy-proof",
        "agent-proof",
        "style-proof",
        "inert:plugin-proof",
        "inert:extra-proof",
    ] {
        assert!(text.contains(name), "native catalog missing {name}");
    }
    assert!(
        !text.contains("disabled-proof"),
        "source false activation must stay disabled"
    );
    assert!(!text.contains(source.to_string_lossy().as_ref()));
}
#[test]
fn codex_chooses_exact_native_local_version_and_only_typed_activation() {
    let (_tmp, source, dest) = fixture();
    for version in ["1.9.0", "1.10.0", "local"] {
        put(
            &source,
            &format!("plugins/cache/test-market/inert/{version}/.codex-plugin/plugin.json"),
            r#"{"name":"inert","version":"1.0.0"}"#,
        );
        put(
            &source,
            &format!("plugins/cache/test-market/inert/{version}/skills/proof/SKILL.md"),
            "proof",
        );
    }
    put(
        &source,
        "config.toml",
        "provider = 'SOURCE_CANARY'\n[plugins.\"inert@test-market\"]\nenabled = false\n",
    );
    put(&dest, "config.toml", "provider = 'PROFILE_CANARY'\n");
    let report = reconcile(Tool::Codex, &source, &dest);
    assert!(
        !report.entries.iter().any(|e| e.state == "failed"),
        "{report:?}"
    );
    assert!(
        dest.join("plugins/cache/test-market/inert/local/skills/proof/SKILL.md")
            .exists()
    );
    assert!(!dest.join("plugins/cache/test-market/inert/1.10.0").exists());
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(dest.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["plugins"]["inert@test-market"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(config["provider"].as_str(), Some("PROFILE_CANARY"));
}
#[test]
#[ignore = "requires installed official Codex CLI; isolated HOME, no account/model/plugin API"]
fn native_inert_codex_skill_scopes_and_local_plugin() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    let (tmp, source, dest) = fixture();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let workspace = root.join("workspace");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&workspace).unwrap();
    for (base, path, name) in [
        (&source, "skills/legacy/SKILL.md", "legacy-proof"),
        (&home, ".agents/skills/home-proof/SKILL.md", "home-proof"),
        (
            &workspace,
            ".agents/skills/project-proof/SKILL.md",
            "project-proof",
        ),
        (
            &source,
            "plugins/cache/test-market/inert/local/skills/plugin-proof/SKILL.md",
            "plugin-proof",
        ),
    ] {
        put(
            base,
            path,
            &format!(
                "---\nname: {name}\ndescription: Inert discovery only\n---\nNever run anything.\n"
            ),
        );
    }
    put(
        &source,
        "plugins/cache/test-market/inert/local/.codex-plugin/plugin.json",
        r#"{"name":"inert","version":"1.0.0","interface":{"displayName":"Inert Native Fixture"},"hooks":{"hooks":{}},"mcpServers":{}}"#,
    );
    put(
        &source,
        "plugins/cache/test-market/inert/local/plugin.json",
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"inert","extensions":{"com.openai":{"interface":{"displayName":"Inert Native Fixture"},"hooks":{"hooks":{}}}}}"#,
    );
    put(
        &source,
        "plugins/cache/test-market/inert/local/mcp.json",
        r#"{"mcpServers":{}}"#,
    );
    put(
        &source,
        "config.toml",
        "[plugins.\"inert@test-market\"]\nenabled = true\n",
    );
    let second = root.join("profile-b");
    fs::create_dir(&second).unwrap();
    for target in [&dest, &second] {
        let report = reconcile(Tool::Codex, &source, target);
        assert!(
            !report.entries.iter().any(|e| e.state == "failed"),
            "{report:?}"
        );
    }
    fs::rename(&source, root.join("source-unavailable")).unwrap();
    for target in [&dest, &second] {
        let mut child =
            Command::new(std::env::var("KILN_TEST_CODEX_BIN").expect("explicit official CLI path"))
                .env_clear()
                .env("HOME", &home)
                .env("CODEX_HOME", target)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("RUST_LOG", "error")
                .current_dir(&workspace)
                .args(["app-server", "--listen", "stdio://"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let result = (|| -> Result<Value, String> {
            let stdin = child.stdin.as_mut().unwrap();
            writeln!(stdin,"{}",serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"kiln-inert-fixture","version":"1.0"}}})).unwrap();
            stdin.flush().unwrap();
            loop {
                let v = rx
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| e.to_string())?;
                if v["id"] == 1 {
                    break;
                }
            }
            writeln!(stdin, "{}", serde_json::json!({"method":"initialized"})).unwrap();
            writeln!(stdin,"{}",serde_json::json!({"id":2,"method":"skills/list","params":{"cwds":[workspace],"forceReload":true}})).unwrap();
            stdin.flush().unwrap();
            loop {
                let v = rx
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| e.to_string())?;
                if v["id"] == 2 {
                    return Ok(v);
                }
            }
        })();
        let _ = child.kill();
        let _ = child.wait();
        let response = result.unwrap();
        let skills = response["result"]["data"][0]["skills"]
            .as_array()
            .expect("native skills list");
        let names = skills
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect::<Vec<_>>();
        println!("Native Codex inert skill names: {names:?}");
        for name in [
            "legacy-proof",
            "home-proof",
            "project-proof",
            "plugin-proof",
        ] {
            assert!(names.iter().any(|n| n.contains(name)), "missing {name}");
        }
        assert_eq!(names.iter().filter(|n| **n == "home-proof").count(), 1);
        assert!(!target.join("skills/home-proof").exists());
    }
}
#[test]
fn crash_recovery_preserves_old_new_and_concurrent_local_states() {
    for step in ["intent", "backup", "published", "manifest", "local"] {
        let (_tmp, source, dest) = fixture();
        put(&source, "skills/demo/SKILL.md", "old");
        reconcile(Tool::Claude, &source, &dest);
        let root = Dir::open(&dest).unwrap();
        let state = root.dir(STATE, false).unwrap();
        let mut m: Manifest =
            serde_json::from_slice(&state.read(MANIFEST, METADATA_BYTES).unwrap().0).unwrap();
        let target = Target::file("skills/demo");
        let before = physical(&root, &target).unwrap();
        put(&source, "skills/demo/SKILL.md", "new");
        let src = Dir::open(&source).unwrap();
        let skills = src.dir("skills", false).unwrap();
        let snap = scan::snapshot(&skills, "demo", false, &mut Budget::new(), true).unwrap();
        let after = Some(snap.fingerprint());
        let stage = "asset-crash";
        let backup = "backup-crash";
        snap.stage(&state, stage).unwrap();
        let entry = Entry {
            unit: None,
            target: target.clone(),
            kind: "skills".into(),
            source: after.clone().unwrap(),
            written: after.clone(),
            state: "owned".into(),
        };
        let intent = Intent {
            entry: entry.clone(),
            before,
            after,
            stage: stage.into(),
            backup: backup.into(),
        };
        state
            .atomic("intent.json", &serde_json::to_vec(&intent).unwrap())
            .unwrap();
        let (parent, name) = root.parent(&target.path, false).unwrap();
        if step != "intent" {
            parent.rename(&name, &state, backup).unwrap();
        }
        if ["published", "manifest", "local"].contains(&step) {
            state.rename(stage, &parent, &name).unwrap();
        }
        if step == "manifest" {
            m.entries.insert(target.key(), entry);
            persist(&state, &m).unwrap();
        }
        if step == "local" {
            put(&dest, "skills/demo/SKILL.md", "local");
        }
        recover(&root, &state, &mut m).unwrap();
        recover(&root, &state, &mut m).unwrap();
        let expected = if step == "local" {
            "local"
        } else if ["intent", "backup"].contains(&step) {
            "old"
        } else {
            "new"
        };
        assert_eq!(
            fs::read_to_string(dest.join("skills/demo/SKILL.md")).unwrap(),
            expected,
            "{step}"
        );
        assert!(!dest.join(STATE).join("intent.json").exists());
        if step == "local" {
            assert_eq!(m.entries[&target.key()].state, "override");
        }
    }
}
#[test]
fn local_added_empty_directory_and_executable_bit_protect_whole_asset() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "old");
    reconcile(Tool::Claude, &source, &dest);
    fs::create_dir(dest.join("skills/demo/local-empty")).unwrap();
    put(&source, "skills/demo/SKILL.md", "new");
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(
        fs::read_to_string(dest.join("skills/demo/SKILL.md")).unwrap(),
        "old"
    );
}
#[test]
fn collision_and_invalid_manifest_target_are_rejected() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "a");
    put(&source, "skills/DEMO/SKILL.md", "b");
    let report = reconcile(Tool::Claude, &source, &dest);
    // On a case-insensitive filesystem these are one native directory. Else
    // both candidates must be rejected; iteration order cannot pick a winner.
    if fs::read_dir(source.join("skills")).unwrap().count() == 2 {
        assert!(
            report
                .entries
                .iter()
                .any(|e| e.reason == "CASE_OR_UNICODE_COLLISION")
        );
        assert!(!dest.join("skills/demo").exists());
    }
    assert!(Target::file("auth.json").valid().is_err());
    assert!(Target::file("../escape").valid().is_err());
}
#[test]
fn resource_depth_count_bytes_and_deadline_are_explicit() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "good");
    reconcile(Tool::Claude, &source, &dest);
    let deep = format!("skills/demo/{}/x", vec!["d"; MAX_DEPTH + 1].join("/"));
    put(&source, &deep, "deep");
    let r = reconcile(Tool::Claude, &source, &dest);
    assert!(r.entries.iter().any(|e| e.reason == "DEPTH_16"));
    assert!(!dest.join(deep).exists());
    let b = Budget {
        start: Instant::now() - Duration::from_secs(6),
        files: 0,
        bytes: 0,
    };
    assert_eq!(b.check(), Err("RECONCILE_DEADLINE_5_SECONDS"));
    let src = Dir::open(&source).unwrap();
    let skills = src.dir("skills", false).unwrap();
    fs::remove_dir_all(source.join("skills/demo/d")).unwrap();
    let mut b = Budget {
        start: Instant::now(),
        files: TOTAL_FILES,
        bytes: 0,
    };
    assert_eq!(
        scan::snapshot(&skills, "demo", false, &mut b, true).err(),
        Some("TOTAL_FILES_16384")
    );
    let mut b = Budget {
        start: Instant::now(),
        files: 0,
        bytes: TOTAL_BYTES,
    };
    assert_eq!(
        scan::snapshot(&skills, "demo", false, &mut b, true).err(),
        Some("TOTAL_BYTES_268435456")
    );
}
#[test]
fn removal_of_entire_tracking_directory_does_not_reset_local_deletions() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "one");
    reconcile(Tool::Claude, &source, &dest);
    fs::remove_dir_all(dest.join("skills/demo")).unwrap();
    fs::remove_dir_all(dest.join(STATE)).unwrap();
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "MISSING_TRACKED_MANIFEST")
    );
    assert!(!dest.join("skills/demo").exists());
}
#[cfg(unix)]
#[test]
fn imports_are_private_distinct_inodes_and_sources_are_unchanged() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "fixture");
    put(&source, "skills/demo/run.sh", "#!/bin/sh\nexit 0\n");
    fs::set_permissions(
        source.join("skills/demo/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let before = fs::metadata(source.join("skills/demo/run.sh")).unwrap();
    let source_bytes = fs::read(source.join("skills/demo/run.sh")).unwrap();
    reconcile(Tool::Claude, &source, &dest);
    let copied = fs::metadata(dest.join("skills/demo/run.sh")).unwrap();
    assert_ne!((before.dev(), before.ino()), (copied.dev(), copied.ino()));
    assert_eq!(copied.mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(source.join("skills/demo/run.sh"))
            .unwrap()
            .mode(),
        before.mode()
    );
    assert_eq!(
        fs::read(source.join("skills/demo/run.sh")).unwrap(),
        source_bytes
    );
    fs::set_permissions(
        dest.join("skills/demo/run.sh"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    put(&source, "skills/demo/SKILL.md", "changed");
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.entries.iter().any(|e| e.state == "profile_override"));
    assert_eq!(
        fs::read_to_string(dest.join("skills/demo/SKILL.md")).unwrap(),
        "fixture"
    );
}
#[test]
fn prepared_report_is_bounded_typed_and_contains_no_asset_contents() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "CONTENT_CANARY");
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.validated());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("CONTENT_CANARY")
    );
    let root = Dir::open(&dest).unwrap();
    let private = root.dir("report-private", true).unwrap();
    report
        .write_prepared(&private.path.join("extension-report.json"))
        .unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&private.path, dest.join("report-link")).unwrap();
        assert!(
            report
                .write_prepared(&dest.join("report-link/blocked.json"))
                .is_err()
        );
        assert!(!private.path.join("blocked.json").exists());
    }
    assert_eq!(
        ExtensionReport::read_prepared(&private.path.join("extension-report.json")).unwrap(),
        report
    );
    let mut invalid = report;
    invalid.entries[0].reason = "raw token:secret".into();
    assert!(!invalid.validated());
    assert!(
        invalid
            .write_prepared(&private.path.join("invalid.json"))
            .is_err()
    );
}
#[test]
fn dependency_identity_version_and_profile_disable_are_validated() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    for path in [
        "plugins/cache/test-market/inert/1.0.0/.claude-plugin/plugin.json",
        "plugins/marketplaces/test-market/inert/.claude-plugin/plugin.json",
    ] {
        put(
            &source,
            path,
            r#"{"name":"inert","dependencies":[{"name":"dependency","version":"1.2.3"}]}"#,
        );
    }
    for path in [
        "plugins/cache/test-market/dependency/1.2.3/.claude-plugin/plugin.json",
        "plugins/marketplaces/test-market/dependency/.claude-plugin/plugin.json",
    ] {
        put(&source, path, r#"{"name":"dependency","version":"1.2.3"}"#);
    }
    let registry_path = source.join("plugins/installed_plugins.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    registry["plugins"]["dependency@test-market"] = serde_json::json!([{"scope":"user","version":"1.2.3","installPath":source.join("plugins/cache/test-market/dependency/1.2.3")}]);
    fs::write(registry_path, registry.to_string()).unwrap();
    let catalog_path =
        source.join("plugins/marketplaces/test-market/.claude-plugin/marketplace.json");
    let mut catalog: Value = serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
    catalog["plugins"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"name":"dependency","source":"./dependency","version":"1.2.3"}));
    fs::write(catalog_path, catalog.to_string()).unwrap();
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true,"dependency@test-market":true}}"#,
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        !report.entries.iter().any(|e| e.state == "failed"),
        "{report:?}"
    );
    let manifest: Value = serde_json::from_slice(
        &fs::read(dest.join("plugins/marketplaces/test-market/inert/.claude-plugin/plugin.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["dependencies"][0]["name"], "dependency");
    assert_eq!(manifest["dependencies"][0]["version"], "1.2.3");
    // A source-dependent update cannot activate over an old private dependency.
    let prior_registry = fs::read(dest.join("plugins/installed_plugins.json")).unwrap();
    put(
        &dest,
        "plugins/cache/test-market/dependency/1.2.3/private.txt",
        "private dependency edit",
    );
    claude_version(&source, "2.0.0");
    for path in [
        "plugins/cache/test-market/inert/2.0.0/.claude-plugin/plugin.json",
        "plugins/marketplaces/test-market/inert/.claude-plugin/plugin.json",
    ] {
        put(
            &source,
            path,
            r#"{"name":"inert","version":"2.0.0","dependencies":[{"name":"dependency","version":"1.2.3"}]}"#,
        );
    }
    // claude_version is a single-plugin helper; restore this fixture's dependency.
    registry["plugins"]["inert@test-market"][0]["installPath"] = source
        .join("plugins/cache/test-market/inert/2.0.0")
        .to_string_lossy()
        .as_ref()
        .into();
    registry["plugins"]["inert@test-market"][0]["version"] = "2.0.0".into();
    put(
        &source,
        "plugins/installed_plugins.json",
        &registry.to_string(),
    );
    catalog["plugins"][0]["version"] = "2.0.0".into();
    put(
        &source,
        "plugins/marketplaces/test-market/.claude-plugin/marketplace.json",
        &catalog.to_string(),
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "DEPENDENCY_PROFILE_OVERRIDE"),
        "{report:?}"
    );
    assert!(!dest.join("plugins/cache/test-market/inert/2.0.0").exists());
    assert_eq!(
        fs::read(dest.join("plugins/installed_plugins.json")).unwrap(),
        prior_registry
    );

    put(
        &dest,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true,"dependency@test-market":false}}"#,
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.name == "inert@test-market" && e.reason == "DEPENDENCY_PROFILE_OVERRIDE")
    );
}
#[test]
fn unsafe_nested_marketplace_metadata_never_becomes_a_payload() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    let path = source.join("plugins/marketplaces/test-market/.claude-plugin/marketplace.json");
    let mut catalog: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    catalog["metadata"] = serde_json::json!({"headers":{"Authorization":"METADATA_CANARY"}});
    fs::write(path, catalog.to_string()).unwrap();
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.entries.iter().any(|e| e.state == "failed"));
    assert!(!dest.join("plugins/cache").exists());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("METADATA_CANARY")
    );
}
#[test]
fn explicit_marketplace_declarations_are_not_silently_reinterpreted() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    let preserved = r#"{ "enabledPlugins": {"inert@test-market":true,"other@other-market":true}, "extraKnownMarketplaces":{"test-market":{"source":{"source":"directory","path":"/external/author-root"}}}, "unrelated":"CONFIG_CANARY" }"#;
    put(&source, "settings.json", preserved);
    put(&dest, "settings.json", preserved);
    put(&source, "skills/unrelated/SKILL.md", "unrelated fixture");
    put(
        &source,
        "plugins/cache/other-market/other/1.0.0/.claude-plugin/plugin.json",
        r#"{"name":"other","version":"1.0.0"}"#,
    );
    put(
        &source,
        "plugins/marketplaces/other-market/.claude-plugin/marketplace.json",
        r#"{"name":"other-market","owner":{"name":"Fixture"},"plugins":[{"name":"other","source":"./other"}]}"#,
    );
    let path = source.join("plugins/installed_plugins.json");
    let mut installed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    installed["plugins"]["other@other-market"] = serde_json::json!([{"scope":"user","installPath":source.join("plugins/cache/other-market/other/1.0.0"),"version":"1.0.0"}]);
    fs::write(path, installed.to_string()).unwrap();
    let path = source.join("plugins/known_marketplaces.json");
    let mut known: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    known["other-market"] = serde_json::json!({"source":{"source":"github","repo":"fixture/other"},"installLocation":source.join("plugins/marketplaces/other-market"),"lastUpdated":"2026-11-15T00:00:00Z","autoUpdate":false});
    fs::write(path, known.to_string()).unwrap();
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(report.entries.iter().any(|e| e.name == "inert@test-market"
        && e.state == "unsupported"
        && e.reason == "EXPLICIT_MARKETPLACE_DECLARATION_REQUIRES_NATIVE_SETUP"));
    assert!(!dest.join("plugins/cache/test-market").exists());
    assert!(
        dest.join("plugins/cache/other-market/other/1.0.0/.claude-plugin/plugin.json")
            .exists()
    );
    assert!(dest.join("skills/unrelated/SKILL.md").exists());
    assert_eq!(
        fs::read_to_string(dest.join("settings.json")).unwrap(),
        preserved,
        "unowned native declaration and unrelated configuration remain byte-identical"
    );
}
fn codex_fixture(source: &Path, version: &str, manifest: &str) {
    put(
        source,
        &format!(
            "plugins/cache/fixture/demo/{version}/{}",
            if manifest.contains("$schema") {
                "plugin.json"
            } else {
                ".codex-plugin/plugin.json"
            }
        ),
        manifest,
    );
    put(
        source,
        &format!("plugins/cache/fixture/demo/{version}/skills/proof/SKILL.md"),
        "---\nname: proof\ndescription: inert\n---\ninert",
    );
    put(
        source,
        "config.toml",
        "[plugins.\"demo@fixture\"]\nenabled = true\n",
    );
}
const V1_MANIFEST: &str =
    r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo"}"#;
#[test]
fn review_r1_effective_v1_mcp_and_alternative_config_reject_credentials() {
    let (_tmp, source, dest) = fixture();
    codex_fixture(&source, "local", V1_MANIFEST);
    put(
        &source,
        "plugins/cache/fixture/demo/local/mcp.json",
        r#"{"mcpServers":{"demo":{"url":"https://example.invalid","headers":{"Authorization":"R1_TOKEN_CANARY"}}}}"#,
    );
    let report = reconcile(Tool::Codex, &source, &dest);
    assert!(
        !dest.join("plugins/cache/fixture/demo/local").exists(),
        "must reject whole credential-bearing bundle"
    );
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.reason == "PLUGIN_REQUIRES_CONFIGURATION")
    );
}
#[test]
fn review_r2_plugin_identity_preserves_override_and_deletion_across_versions() {
    for deleted in [false, true] {
        let (_tmp, source, dest) = fixture();
        codex_fixture(&source, "1.0.0", V1_MANIFEST);
        reconcile(Tool::Codex, &source, &dest);
        if deleted {
            fs::remove_dir_all(dest.join("plugins/cache/fixture/demo/1.0.0")).unwrap();
        } else {
            put(
                &dest,
                "plugins/cache/fixture/demo/1.0.0/skills/proof/SKILL.md",
                "private edit",
            );
        }
        fs::remove_dir_all(source.join("plugins/cache/fixture/demo/1.0.0")).unwrap();
        codex_fixture(&source, "2.0.0", V1_MANIFEST);
        reconcile(Tool::Codex, &source, &dest);
        assert!(
            !dest.join("plugins/cache/fixture/demo/2.0.0").exists(),
            "stable identity must protect old version intent"
        );
    }
}
#[test]
fn review_r3_skills_directory_false_activation_and_changes_are_inherited() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/demo/.claude-plugin/plugin.json",
        r#"{"name":"demo","defaultEnabled":true}"#,
    );
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"demo@skills-dir":false}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    let read =
        || serde_json::from_slice::<Value>(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
    assert_eq!(read()["enabledPlugins"]["demo@skills-dir"], false);
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"demo@skills-dir":true}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    assert_eq!(read()["enabledPlugins"]["demo@skills-dir"], true);
}
#[test]
fn review_r4_known_safe_command_interface_and_components_are_preserved() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/demo/.claude-plugin/plugin.json",
        r#"{"name":"demo","commands":{"proof":{"content":"Never execute anything.","description":"Inert proof"}},"hooks":{},"mcpServers":{},"lspServers":{}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    assert!(dest.join("skills/demo/.claude-plugin/plugin.json").exists());
    let (_tmp, source, dest) = fixture();
    codex_fixture(
        &source,
        "local",
        r#"{"$schema":"https://agent-plugins.org/schemas/1.0.0/plugin.schema.json","name":"demo","extensions":{"com.openai":{"interface":{"displayName":"Inert Demo"},"hooks":{"hooks":{}}}}}"#,
    );
    put(
        &source,
        "plugins/cache/fixture/demo/local/mcp.json",
        r#"{"mcpServers":{}}"#,
    );
    reconcile(Tool::Codex, &source, &dest);
    assert!(
        dest.join("plugins/cache/fixture/demo/local/plugin.json")
            .exists()
    );
}
#[cfg(unix)]
#[test]
fn review_r6_staged_directories_are_synced_before_intent_and_publication() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "inert");
    put(&source, "skills/demo/nested/helper.txt", "helper");
    super::fs::TEST_SYNC.with(|t| t.borrow_mut().clear());
    reconcile(Tool::Claude, &source, &dest);
    let trace = super::fs::TEST_SYNC.with(|t| t.borrow().clone());
    let nested = trace
        .iter()
        .position(|s| s.contains("asset-") && s.ends_with("/nested"))
        .expect("nested staged directory sync");
    let staged = trace
        .iter()
        .enumerate()
        .find(|(i, s)| *i > nested && s.contains("asset-") && !s.ends_with("/nested"))
        .map(|(i, _)| i)
        .expect("staged root sync after child");
    let published = trace
        .iter()
        .position(|s| s == "publish:demo")
        .expect("publication trace");
    let intent = trace
        .iter()
        .position(|s| s == "publish:intent.json")
        .unwrap();
    assert!(staged < intent && intent < published);
    assert_eq!(trace[published + 1], dest.join(STATE).to_string_lossy());
    assert_eq!(trace[published + 2], dest.join("skills").to_string_lossy());
    let committed = trace
        .iter()
        .enumerate()
        .find(|(i, s)| *i > published && s.as_str() == format!("publish:{MANIFEST}"))
        .unwrap()
        .0;
    let cleanup = trace
        .iter()
        .position(|s| s == "remove:intent.json")
        .unwrap();
    assert!(committed > published + 2 && cleanup > committed + 2);
}
#[cfg(unix)]
#[test]
fn review_r7_partial_enumeration_and_plugin_root_errors_do_not_remove_imports() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "inert");
    reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_READDIR_FAULT.with(|t| *t.borrow_mut() = Some((source.join("skills"), 0)));
    let report = reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_READDIR_FAULT.with(|t| *t.borrow_mut() = None);
    assert!(report.entries.iter().any(|e| e.state == "failed"));
    assert!(dest.join("skills/demo/SKILL.md").exists());
}

#[test]
fn review_r1_alternative_inline_overlay_and_safe_mcp_paths() {
    let credential = r#"{"mcpServers":{"fixture":{"url":"https://example.invalid","headers":{"Authorization":"ALTERNATIVE_CANARY"}}}}"#;
    for (manifest, overlay, path) in [
        (
            r#"{"name":"demo","mcpServers":"./config/servers.json"}"#,
            None,
            "config/servers.json",
        ),
        (
            V1_MANIFEST,
            Some(r#"{"name":"demo","mcpServers":["./config/servers.json"]}"#),
            "config/servers.json",
        ),
        (
            r#"{"name":"demo","mcpServers":{"fixture":{"url":"https://example.invalid","headers":{"Authorization":"INLINE_CANARY"}}}}"#,
            None,
            ".mcp.json",
        ),
    ] {
        let (_tmp, source, dest) = fixture();
        codex_fixture(&source, "local", manifest);
        let base = "plugins/cache/fixture/demo/local";
        if let Some(overlay) = overlay {
            put(
                &source,
                &format!("{base}/.codex-plugin/plugin.json"),
                overlay,
            );
        }
        put(&source, &format!("{base}/{path}"), credential);
        let report = reconcile(Tool::Codex, &source, &dest);
        assert!(!dest.join(base).exists(), "{report:?}");
        assert!(
            report
                .entries
                .iter()
                .any(|r| r.reason == "PLUGIN_REQUIRES_CONFIGURATION"),
            "{report:?}"
        );
    }
    let (_tmp, source, dest) = fixture();
    codex_fixture(
        &source,
        "local",
        r#"{"name":"demo","mcpServers":"./config/servers.json","interface":{"displayName":"Safe fixture"},"extensions":{"com.openai":{"onboardingSkill":"skills/proof/SKILL.md"}}}"#,
    );
    put(
        &source,
        "plugins/cache/fixture/demo/local/config/servers.json",
        r#"{"mcpServers":{"fixture":{"type":"http","url":"https://example.invalid","headers":{"Accept":"application/json"}}}}"#,
    );
    let report = reconcile(Tool::Codex, &source, &dest);
    assert!(
        dest.join("plugins/cache/fixture/demo/local/config/servers.json")
            .exists(),
        "{report:?}"
    );
}

fn claude_version(source: &Path, version: &str) {
    let old = source.join("plugins/cache/test-market/inert/1.0.0");
    let new = source.join(format!("plugins/cache/test-market/inert/{version}"));
    fs::rename(old, &new).unwrap();
    for base in [new, source.join("plugins/marketplaces/test-market/inert")] {
        put(
            &base,
            ".claude-plugin/plugin.json",
            &serde_json::json!({"name":"inert","version":version}).to_string(),
        );
    }
    put(source,"plugins/installed_plugins.json",&serde_json::json!({"version":2,"plugins":{"inert@test-market":[{"scope":"user","installPath":source.join(format!("plugins/cache/test-market/inert/{version}")),"version":version,"installedAt":"2026-11-15T00:00:00Z","lastUpdated":"2026-11-15T00:00:00Z"}]}}).to_string());
    put(source,"plugins/marketplaces/test-market/.claude-plugin/marketplace.json",&serde_json::json!({"name":"test-market","owner":{"name":"Fixture"},"plugins":[{"name":"inert","source":"./inert","version":version}]}).to_string());
}
#[test]
fn review_r2_claude_version_override_deletion_and_catalog_pointer_stay_effective() {
    for deleted in [false, true] {
        let (_tmp, source, dest) = fixture();
        plugin_fixture(&source);
        reconcile(Tool::Claude, &source, &dest);
        let registry = fs::read(dest.join("plugins/installed_plugins.json")).unwrap();
        let catalog =
            fs::read(dest.join("plugins/marketplaces/test-market/.claude-plugin/marketplace.json"))
                .unwrap();
        if deleted {
            fs::remove_dir_all(dest.join("plugins/cache/test-market/inert/1.0.0")).unwrap();
        } else {
            put(
                &dest,
                "plugins/cache/test-market/inert/1.0.0/skills/plugin-proof/SKILL.md",
                "private edit",
            );
        }
        claude_version(&source, "2.0.0");
        for _ in 0..2 {
            let report = reconcile(Tool::Claude, &source, &dest);
            assert!(
                report
                    .entries
                    .iter()
                    .any(|r| ["profile_override", "profile_deleted"].contains(&r.state.as_str())),
                "{report:?}"
            );
        }
        assert!(!dest.join("plugins/cache/test-market/inert/2.0.0").exists());
        assert_eq!(
            fs::read(dest.join("plugins/installed_plugins.json")).unwrap(),
            registry
        );
        assert_eq!(
            fs::read(dest.join("plugins/marketplaces/test-market/.claude-plugin/marketplace.json"))
                .unwrap(),
            catalog
        );
    }
}
#[test]
fn review_r2_immutable_versions_are_retained_and_source_removal_disables_next_launch() {
    let (_tmp, source, dest) = fixture();
    codex_fixture(&source, "1.0.0", V1_MANIFEST);
    reconcile(Tool::Codex, &source, &dest);
    let old = fs::read(dest.join("plugins/cache/fixture/demo/1.0.0/plugin.json")).unwrap();
    fs::remove_dir_all(source.join("plugins/cache/fixture/demo/1.0.0")).unwrap();
    codex_fixture(&source, "2.0.0", V1_MANIFEST);
    let report = reconcile(Tool::Codex, &source, &dest);
    assert!(
        report.entries.iter().any(|r| r.state == "retained"),
        "{report:?}"
    );
    assert_eq!(
        fs::read(dest.join("plugins/cache/fixture/demo/1.0.0/plugin.json")).unwrap(),
        old
    );
    assert!(
        dest.join("plugins/cache/fixture/demo/2.0.0/plugin.json")
            .exists()
    );
    fs::remove_dir_all(source.join("plugins/cache")).unwrap();
    fs::remove_file(source.join("config.toml")).unwrap();
    reconcile(Tool::Codex, &source, &dest);
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(dest.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["plugins"]["demo@fixture"]["enabled"].as_bool(),
        Some(false)
    );
    assert!(
        dest.join("plugins/cache/fixture/demo/2.0.0/plugin.json")
            .exists()
    );
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    reconcile(Tool::Claude, &source, &dest);
    claude_version(&source, "2.0.0");
    reconcile(Tool::Claude, &source, &dest);
    assert!(
        dest.join("plugins/cache/test-market/inert/1.0.0/.claude-plugin/plugin.json")
            .exists()
    );
    let registry: Value =
        serde_json::from_slice(&fs::read(dest.join("plugins/installed_plugins.json")).unwrap())
            .unwrap();
    assert!(
        registry["plugins"]["inert@test-market"][0]["installPath"]
            .as_str()
            .unwrap()
            .ends_with("2.0.0")
    );
}
#[test]
fn review_r3_activation_local_override_and_tombstone_survive_source_changes() {
    for deleted in [false, true] {
        let (_tmp, source, dest) = fixture();
        put(
            &source,
            "skills/demo/.claude-plugin/plugin.json",
            r#"{"name":"demo","defaultEnabled":true}"#,
        );
        put(
            &source,
            "settings.json",
            r#"{"enabledPlugins":{"demo@skills-dir":false}}"#,
        );
        reconcile(Tool::Claude, &source, &dest);
        put(
            &dest,
            "settings.json",
            if deleted {
                r#"{"enabledPlugins":{}}"#
            } else {
                r#"{"enabledPlugins":{"demo@skills-dir":true}}"#
            },
        );
        for value in [false, true, false] {
            put(
                &source,
                "settings.json",
                &serde_json::json!({"enabledPlugins":{"demo@skills-dir":value}}).to_string(),
            );
            reconcile(Tool::Claude, &source, &dest);
        }
        let settings: Value =
            serde_json::from_slice(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            settings["enabledPlugins"].get("demo@skills-dir"),
            if deleted {
                None
            } else {
                Some(&Value::Bool(true))
            }
        );
    }
}
#[cfg(unix)]
#[test]
fn review_r6_sync_failure_preserves_old_asset_and_publication_recovery() {
    let (_tmp, source, dest) = fixture();
    put(&source, "skills/demo/SKILL.md", "old");
    reconcile(Tool::Claude, &source, &dest);
    put(&source, "skills/demo/SKILL.md", "new");
    put(&source, "skills/demo/nested/helper.txt", "helper");
    super::fs::TEST_SYNC_FAULT.with(|t| *t.borrow_mut() = Some("/nested".into()));
    let report = reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_SYNC_FAULT.with(|t| *t.borrow_mut() = None);
    assert!(report.entries.iter().any(|r| r.state == "failed"));
    assert_eq!(
        fs::read_to_string(dest.join("skills/demo/SKILL.md")).unwrap(),
        "old"
    );
    // Destination parent failure occurs after exchange. The intent survives and
    // recovery durably flushes both namespaces before committing ownership.
    super::fs::TEST_SYNC_FAULT
        .with(|t| *t.borrow_mut() = Some(dest.join("skills").to_string_lossy().into_owned()));
    let report = reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_SYNC_FAULT.with(|t| *t.borrow_mut() = None);
    assert!(report.entries.iter().any(|r| r.state == "failed"));
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        !report.entries.iter().any(|r| r.state == "failed"),
        "{report:?}"
    );
    assert_eq!(
        fs::read_to_string(dest.join("skills/demo/SKILL.md")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(dest.join("skills/demo/nested/helper.txt")).unwrap(),
        "helper"
    );
}
#[cfg(unix)]
#[test]
fn review_r7_errors_after_rows_and_plugin_kind_failure_preserve_owned_assets() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    for name in ["one", "two", "three"] {
        put(&source, &format!("skills/{name}/SKILL.md"), name);
    }
    reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_READDIR_FAULT.with(|t| *t.borrow_mut() = Some((source.join("skills"), 1)));
    let report = reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_READDIR_FAULT.with(|t| *t.borrow_mut() = None);
    assert!(
        report
            .entries
            .iter()
            .any(|r| r.reason == "DIRECTORY_ENUMERATION_FAILED")
    );
    for name in ["one", "two", "three"] {
        assert!(dest.join(format!("skills/{name}/SKILL.md")).exists());
    }
    super::fs::TEST_KIND_FAULT.with(|t| *t.borrow_mut() = Some(source.join("plugins")));
    let report = reconcile(Tool::Claude, &source, &dest);
    super::fs::TEST_KIND_FAULT.with(|t| *t.borrow_mut() = None);
    assert!(
        report
            .entries
            .iter()
            .any(|r| r.state == "failed" && r.reason == "FILESYSTEM_ACCESS"),
        "{report:?}"
    );
    assert!(
        dest.join("plugins/cache/test-market/inert/1.0.0/.claude-plugin/plugin.json")
            .exists()
    );
}

#[test]
fn review_r4_nonempty_typed_author_components_are_copied_without_execution() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/demo/.claude-plugin/plugin.json",
        r#"{"name":"demo","hooks":"./config/hooks.json","lspServers":"./config/lsp.json","mcpServers":["./config/mcp.json"],"commands":{"proof":{"source":"./commands/proof.md","description":"Author command"}}}"#,
    );
    put(
        &source,
        "skills/demo/scripts/inert.sh",
        "#!/bin/sh\nexit 0\n",
    );
    put(
        &source,
        "skills/demo/config/hooks.json",
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"${CLAUDE_PLUGIN_ROOT}/scripts/inert.sh","timeout":1}]}]}}"#,
    );
    put(
        &source,
        "skills/demo/config/lsp.json",
        r#"{"fixture":{"command":"${CLAUDE_PLUGIN_ROOT}/scripts/inert.sh","extensionToLanguage":{".fixture":"fixture"},"restartOnCrash":false}}"#,
    );
    put(
        &source,
        "skills/demo/config/mcp.json",
        r#"{"mcpServers":{"fixture":{"command":"${CLAUDE_PLUGIN_ROOT}/scripts/inert.sh","args":[],"env":{"LOG_LEVEL":"error"}}}}"#,
    );
    put(
        &source,
        "skills/demo/commands/proof.md",
        "Do not execute anything.",
    );
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report.entries.iter().any(|r| r.state == "prepared"),
        "{report:?}"
    );
    for path in [
        ".claude-plugin/plugin.json",
        "scripts/inert.sh",
        "config/hooks.json",
        "config/lsp.json",
        "config/mcp.json",
        "commands/proof.md",
    ] {
        assert_eq!(
            fs::read(source.join("skills/demo").join(path)).unwrap(),
            fs::read(dest.join("skills/demo").join(path)).unwrap()
        );
    }
}

#[test]
fn review_r2_source_disappearance_preserves_private_plugin_intent() {
    for tool in [Tool::Claude, Tool::Codex] {
        let (_tmp, source, dest) = fixture();
        let (payload, metadata) = if tool == Tool::Claude {
            plugin_fixture(&source);
            (
                "plugins/cache/test-market/inert/1.0.0/skills/plugin-proof/SKILL.md",
                "plugins/installed_plugins.json",
            )
        } else {
            codex_fixture(&source, "1.0.0", V1_MANIFEST);
            (
                "plugins/cache/fixture/demo/1.0.0/skills/proof/SKILL.md",
                "config.toml",
            )
        };
        reconcile(tool, &source, &dest);
        put(&dest, payload, "private edit");
        let before = fs::read(dest.join(metadata)).unwrap();
        fs::remove_dir_all(source.join("plugins")).unwrap();
        if tool == Tool::Codex {
            fs::remove_file(source.join("config.toml")).unwrap();
        } else {
            fs::remove_file(source.join("settings.json")).unwrap();
        }
        let report = reconcile(tool, &source, &dest);
        assert!(
            report.entries.iter().any(|r| r.state == "profile_override"),
            "{report:?}"
        );
        assert_eq!(fs::read(dest.join(metadata)).unwrap(), before);
        assert_eq!(
            fs::read_to_string(dest.join(payload)).unwrap(),
            "private edit"
        );
    }
}

#[test]
fn fix2_r2_directory_override_keeps_shared_bindings_after_source_disappears() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    reconcile(Tool::Claude, &source, &dest);
    let payload = "plugins/marketplaces/test-market/inert/skills/plugin-proof/SKILL.md";
    put(&dest, payload, "private directory override");
    let paths = [
        "plugins/installed_plugins.json",
        "plugins/known_marketplaces.json",
        "plugins/marketplaces/test-market/.claude-plugin/marketplace.json",
    ];
    let prior = paths.map(|p| fs::read(dest.join(p)).unwrap());
    fs::remove_dir_all(source.join("plugins")).unwrap();
    fs::remove_file(source.join("settings.json")).unwrap();
    for _ in 0..2 {
        let report = reconcile(Tool::Claude, &source, &dest);
        assert!(
            report.entries.iter().any(|e| e.state == "profile_override"),
            "{report:?}"
        );
        for (path, bytes) in paths.iter().zip(&prior) {
            assert_eq!(fs::read(dest.join(path)).unwrap(), *bytes, "{path}");
        }
        assert_eq!(
            fs::read_to_string(dest.join(payload)).unwrap(),
            "private directory override"
        );
    }
}

#[test]
fn fix2_r2_removed_protected_catalog_entry_merges_with_unrelated_source_update() {
    let (_tmp, source, dest) = fixture();
    plugin_fixture(&source);
    for base in [
        "plugins/cache/test-market/other/1.0.0",
        "plugins/marketplaces/test-market/other",
    ] {
        put(
            &source,
            &format!("{base}/.claude-plugin/plugin.json"),
            r#"{"name":"other","version":"1.0.0"}"#,
        );
        put(
            &source,
            &format!("{base}/skills/proof/SKILL.md"),
            "other original",
        );
    }
    let registry_path = "plugins/installed_plugins.json";
    let catalog_path = "plugins/marketplaces/test-market/.claude-plugin/marketplace.json";
    let mut registry: Value =
        serde_json::from_slice(&fs::read(source.join(registry_path)).unwrap()).unwrap();
    registry["plugins"]["other@test-market"] = serde_json::json!([{"scope":"user","version":"1.0.0","installPath":source.join("plugins/cache/test-market/other/1.0.0")}]);
    put(&source, registry_path, &registry.to_string());
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(source.join(catalog_path)).unwrap()).unwrap();
    catalog["plugins"].as_array_mut().unwrap().push(serde_json::json!({"name":"other","source":"./other","version":"1.0.0","description":"old description"}));
    put(&source, catalog_path, &catalog.to_string());
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"inert@test-market":true,"other@test-market":true}}"#,
    );
    reconcile(Tool::Claude, &source, &dest);
    put(
        &dest,
        "plugins/marketplaces/test-market/inert/skills/plugin-proof/SKILL.md",
        "private directory override",
    );
    let known = fs::read(dest.join("plugins/known_marketplaces.json")).unwrap();
    let old: Value = serde_json::from_slice(&fs::read(dest.join(catalog_path)).unwrap()).unwrap();
    let protected = old["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "inert")
        .unwrap()
        .clone();
    registry["plugins"]
        .as_object_mut()
        .unwrap()
        .remove("inert@test-market");
    put(&source, registry_path, &registry.to_string());
    catalog["plugins"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["name"] != "inert");
    catalog["plugins"][0]["description"] = "new unrelated description".into();
    put(&source, catalog_path, &catalog.to_string());
    fs::remove_dir_all(source.join("plugins/marketplaces/test-market/inert")).unwrap();
    fs::remove_dir_all(source.join("plugins/cache/test-market/inert")).unwrap();
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"other@test-market":true}}"#,
    );
    for _ in 0..2 {
        let report = reconcile(Tool::Claude, &source, &dest);
        assert!(
            !report.entries.iter().any(|e| e.state == "failed"),
            "{report:?}"
        );
        let merged: Value =
            serde_json::from_slice(&fs::read(dest.join(catalog_path)).unwrap()).unwrap();
        let entries = merged["plugins"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.iter().find(|e| e["name"] == "inert"),
            Some(&protected)
        );
        assert_eq!(
            entries.iter().find(|e| e["name"] == "other").unwrap()["description"],
            "new unrelated description"
        );
        assert_eq!(
            fs::read(dest.join("plugins/known_marketplaces.json")).unwrap(),
            known
        );
        let private_registry: Value =
            serde_json::from_slice(&fs::read(dest.join(registry_path)).unwrap()).unwrap();
        assert!(private_registry["plugins"]["inert@test-market"].is_array());
        assert!(private_registry["plugins"]["other@test-market"].is_array());
        assert_eq!(
            fs::read_to_string(
                dest.join("plugins/marketplaces/test-market/inert/skills/plugin-proof/SKILL.md")
            )
            .unwrap(),
            "private directory override"
        );
    }
}

#[cfg(unix)]
#[test]
fn fix2_r3_activation_failure_never_exposes_default_enabled_payload() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/demo/.claude-plugin/plugin.json",
        r#"{"name":"demo","defaultEnabled":true}"#,
    );
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"demo@skills-dir":false}}"#,
    );
    put(
        &source,
        "skills/unrelated/SKILL.md",
        "unrelated remains usable",
    );
    let outside = dest.with_file_name("unowned-settings.json");
    fs::write(&outside, b"{}").unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("settings.json")).unwrap();
    for _ in 0..2 {
        let report = reconcile(Tool::Claude, &source, &dest);
        assert!(
            report.entries.iter().any(|e| e.state == "failed"),
            "{report:?}"
        );
        assert!(
            !dest.join("skills/demo").exists(),
            "failed false activation must gate plugin publication"
        );
        assert!(
            !report
                .entries
                .iter()
                .any(|e| e.kind == "skills_plugin" && e.state == "prepared")
        );
        assert_eq!(fs::read(&outside).unwrap(), b"{}");
        assert!(dest.join("skills/unrelated/SKILL.md").exists());
        assert!(
            fs::symlink_metadata(dest.join("settings.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    fs::remove_file(dest.join("settings.json")).unwrap();
    super::fs::TEST_SYNC.with(|t| t.borrow_mut().clear());
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        dest.join("skills/demo/.claude-plugin/plugin.json").exists(),
        "{report:?}"
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(dest.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["enabledPlugins"]["demo@skills-dir"], false);
    let trace = super::fs::TEST_SYNC.with(|t| t.borrow().clone());
    assert!(
        trace
            .iter()
            .position(|s| s == "publish:settings.json")
            .unwrap()
            < trace.iter().position(|s| s == "publish:demo").unwrap()
    );
    assert_eq!(fs::read(&outside).unwrap(), b"{}");
}

#[test]
fn fix2_r3_explicit_private_activation_override_remains_authoritative() {
    let (_tmp, source, dest) = fixture();
    put(
        &source,
        "skills/demo/.claude-plugin/plugin.json",
        r#"{"name":"demo","defaultEnabled":true}"#,
    );
    put(
        &source,
        "settings.json",
        r#"{"enabledPlugins":{"demo@skills-dir":false}}"#,
    );
    let private = r#"{ "enabledPlugins": {"demo@skills-dir": true}, "private": "UNCHANGED" }"#;
    put(&dest, "settings.json", private);
    let report = reconcile(Tool::Claude, &source, &dest);
    assert!(
        report
            .entries
            .iter()
            .any(|e| e.kind == "plugin_activation" && e.state == "profile_override"),
        "{report:?}"
    );
    assert_eq!(
        fs::read_to_string(dest.join("settings.json")).unwrap(),
        private
    );
    assert!(dest.join("skills/demo/.claude-plugin/plugin.json").exists());
}
