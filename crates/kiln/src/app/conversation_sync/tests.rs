use super::*;
use format::Message;
use serde_json::json;

fn messages() -> Vec<Message> {
    vec![
        Message {
            role: "user".into(),
            text: "Remember the exact token kiln-orchid-731.\nKeep this second line.".into(),
        },
        Message {
            role: "assistant".into(),
            text: "Recorded kiln-orchid-731.\n```rust\nlet exact = 731;\n```".into(),
        },
    ]
}
fn rows(bytes: &[u8]) -> Vec<serde_json::Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}
struct Fixture {
    _temp: tempfile::TempDir,
    config: PathBuf,
    pair: Pair,
    source: Source,
    gate: Gate,
}
impl Fixture {
    fn new(tool: Tool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let config = root.join("kiln");
        private_dir(&config).unwrap();
        let pair = Pair {
            claude: Endpoint {
                root: root.join("claude"),
                label: "A".into(),
            },
            codex: Endpoint {
                root: root.join("codex"),
                label: "B".into(),
            },
        };
        private_dir(&pair.claude.root).unwrap();
        private_dir(&pair.codex.root).unwrap();
        let seed = Source {
            tool: opposite(tool),
            root: PathBuf::new(),
            path: PathBuf::new(),
            cwd: root.to_string_lossy().into_owned(),
            id: String::new(),
        };
        let source = destination(&pair, &seed, &stable_id("fixture"), chrono::Utc::now()).unwrap();
        publish(
            &source.path,
            &format::render(
                tool,
                &source.id,
                &source.cwd,
                "2026-10-13T00:00:00.000Z",
                &messages(),
            )
            .unwrap(),
        )
        .unwrap();
        let tracked = config.join("conversation-sync/tracked");
        private_dir(&tracked).unwrap();
        publish(
            &tracked.join("test.json"),
            &serde_json::to_vec(&source).unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            config,
            pair,
            source,
            gate: Arc::new(AtomicBool::new(true)),
        }
    }
    fn scan(&self) -> Status {
        scan(
            &self.config,
            &self.pair,
            &self.gate,
            &mut Cache::new(),
            |_| {},
        )
        .unwrap()
    }
    fn state(&self) -> State {
        let path = self.config.join("conversation-sync").join(format!(
            "{}.json",
            digest(&serde_json::to_vec(&self.pair).unwrap())
        ));
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }
}
#[test]
fn native_formats_preserve_text_and_visible_items() {
    for tool in Tool::ALL {
        let original = messages();
        let bytes = format::render(
            tool,
            &stable_id("test"),
            "/tmp/project",
            "2026-10-13T00:00:00.000Z",
            &original,
        )
        .unwrap();
        let parsed = rows(&bytes);
        assert_eq!(format::parse(tool, &parsed).unwrap(), original);
        if tool == Tool::Codex {
            assert_eq!(
                parsed
                    .iter()
                    .filter(|r| r["payload"]["type"] == "item_completed")
                    .count(),
                original.len()
            );
        }
    }
}
#[test]
fn bidirectional_changes_are_native_and_do_not_echo() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let first = f.scan();
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(first.published, 1);
        assert_eq!(f.scan().published, 0);
        let state = f.state();
        let imported = state
            .entries
            .values()
            .find(|e| e.source.tool == opposite(tool))
            .unwrap()
            .source
            .clone();
        assert_eq!(read_messages(&imported).unwrap(), messages());
        let mut extended = messages();
        extended.push(Message {
            role: "user".into(),
            text: "Continue from the other CLI.".into(),
        });
        extended.push(Message {
            role: "assistant".into(),
            text: "The exact token is kiln-orchid-731.".into(),
        });
        fs::write(
            &imported.path,
            format::render(
                imported.tool,
                &imported.id,
                &imported.cwd,
                "2026-10-13T00:01:00.000Z",
                &extended,
            )
            .unwrap(),
        )
        .unwrap();
        let second = f.scan();
        assert!(second.errors.is_empty(), "{:?}", second.errors);
        assert_eq!(second.published, 1);
        assert_eq!(f.scan().published, 0);
        assert_eq!(
            read_messages(&f.source).unwrap(),
            messages(),
            "original session is immutable"
        );
        let state = f.state();
        let latest = &state.recent[0];
        assert_eq!(latest.tool, tool);
        let back = state
            .entries
            .values()
            .find(|e| e.source.id == latest.id)
            .unwrap();
        assert_eq!(read_messages(&back.source).unwrap(), extended);
    }
}
#[test]
fn partial_writes_attachments_versions_and_disable_are_safe() {
    let f = Fixture::new(Tool::Claude);
    let original = fs::read(&f.source.path).unwrap();
    fs::write(&f.source.path, &original[..original.len() - 1]).unwrap();
    assert_eq!(f.scan().published, 0);
    assert!(!f.scan().errors.is_empty());
    let mut unsupported = rows(&original);
    unsupported[0]["message"]["content"] =
        json!([{"type":"image","source":{"type":"base64","data":"fixture"}}]);
    let serialize = |rows: &Vec<serde_json::Value>| {
        rows.iter()
            .map(|r| format!("{}\n", serde_json::to_string(r).unwrap()))
            .collect::<String>()
    };
    fs::write(&f.source.path, serialize(&unsupported)).unwrap();
    assert_eq!(f.scan().published, 0);
    assert!(f.scan().errors[0].contains("Unsupported content"));
    let mut unknown = rows(&original);
    unknown[0]["version"] = json!("9.0.0");
    fs::write(&f.source.path, serialize(&unknown)).unwrap();
    assert!(f.scan().errors[0].contains("version"));
    fs::write(&f.source.path, original).unwrap();
    f.gate.store(false, Ordering::Release);
    assert_eq!(f.scan().published, 0);
    f.gate.store(true, Ordering::Release);
    assert_eq!(f.scan().published, 1);
}
#[test]
fn account_isolation_and_symlink_rejection() {
    let mut f = Fixture::new(Tool::Claude);
    f.pair.claude.root = f._temp.path().join("unrelated");
    assert_eq!(f.scan().watched, 0);
    #[cfg(unix)]
    {
        let f = Fixture::new(Tool::Codex);
        let external = f._temp.path().join("external");
        fs::create_dir(&external).unwrap();
        std::os::unix::fs::symlink(&external, f.pair.claude.root.join("projects")).unwrap();
        let s = f.scan();
        assert_eq!(s.published, 0);
        assert!(s.errors[0].contains("Symlink"));
        assert_eq!(fs::read_dir(external).unwrap().count(), 0);
    }
}
#[test]
fn completed_turns_only_and_provider_instructions_do_not_cross() {
    let f = Fixture::new(Tool::Codex);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    data.insert(1,json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"secret provider instructions"}]}}));
    data.push(json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"partial output"}]}}));
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), messages());
}
#[test]
fn settings_default_is_opt_in_and_pair_roundtrips() {
    let f = Fixture::new(Tool::Claude);
    let mut settings: super::super::state::Settings = serde_json::from_str("{}").unwrap();
    assert!(!settings.conversation_sync_enabled);
    assert!(settings.conversation_sync_pair.is_none());
    settings.conversation_sync_pair = Some(f.pair);
    settings.conversation_sync_enabled = true;
    let restored: super::super::state::Settings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    assert!(restored == settings);
}

/// Creates ONLY synthetic transcripts in a caller-owned directory for installed-CLI validation.
#[test]
#[ignore]
fn export_native_cli_fixtures() {
    let dir = PathBuf::from(
        std::env::var_os("KILN_SYNC_FIXTURE_DIR").expect("explicit fixture directory"),
    );
    assert!(dir.join("owned-fixture").is_file());
    let dir = dir.canonicalize().unwrap();
    let pair = Pair {
        claude: Endpoint {
            root: dir.join("claude"),
            label: "fixture".into(),
        },
        codex: Endpoint {
            root: dir.join("codex"),
            label: "fixture".into(),
        },
    };
    private_dir(&pair.claude.root).unwrap();
    private_dir(&pair.codex.root).unwrap();
    for tool in Tool::ALL {
        let seed = Source {
            tool: opposite(tool),
            root: PathBuf::new(),
            path: PathBuf::new(),
            cwd: dir.to_string_lossy().into_owned(),
            id: String::new(),
        };
        let id = stable_id(tool.key());
        let source = destination(&pair, &seed, &id, chrono::Utc::now()).unwrap();
        publish(
            &source.path,
            &format::render(
                tool,
                &id,
                &source.cwd,
                &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                &messages(),
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(
            dir.join(format!("{}.json", tool.key())),
            serde_json::to_vec_pretty(&source).unwrap(),
        )
        .unwrap();
        println!("{} {id}", tool.key());
    }
}

#[test]
fn progress_reports_copy_then_durable_completion_and_failure() {
    use std::cell::RefCell;
    let f = Fixture::new(Tool::Claude);
    let phases = RefCell::new(Vec::new());
    let status = scan(&f.config, &f.pair, &f.gate, &mut Cache::new(), |s| {
        phases.borrow_mut().extend(s.rows.values().map(|r| r.phase))
    })
    .unwrap();
    assert!(phases.borrow().contains(&Phase::Copying));
    assert_eq!(status.rows.len(), 1);
    let row = status.rows.values().next().unwrap();
    assert_eq!(row.phase, Phase::Synced);
    assert!(row.target.is_some());
    assert!(row.title.contains("kiln-orchid-731"));
    let again = f.scan();
    assert_eq!(
        again.rows.len(),
        1,
        "unmodified imported snapshots are hidden"
    );
    assert_eq!(again.rows.values().next().unwrap().phase, Phase::Synced);
    fs::write(&f.source.path, b"broken json\n").unwrap();
    let failed = f.scan();
    assert!(failed.rows.values().any(|r| r.phase == Phase::Failed));
}

#[test]
fn branches_and_restart_cache_preserve_every_source() {
    let f = Fixture::new(Tool::Codex);
    let mut cache = Cache::new();
    assert_eq!(
        scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {})
            .unwrap()
            .published,
        1
    );
    assert_eq!(
        scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {})
            .unwrap()
            .published,
        0
    );
    let state = f.state();
    let imported = state
        .entries
        .values()
        .find(|e| e.source.tool == Tool::Claude)
        .unwrap()
        .source
        .clone();
    for (source, text) in [(&f.source, "Branch A"), (&imported, "Branch B")] {
        let mut ms = messages();
        ms.push(Message {
            role: "user".into(),
            text: text.into(),
        });
        ms.push(Message {
            role: "assistant".into(),
            text: format!("Reply {text}"),
        });
        fs::write(
            &source.path,
            format::render(
                source.tool,
                &source.id,
                &source.cwd,
                "2026-10-13T00:00:00.000Z",
                &ms,
            )
            .unwrap(),
        )
        .unwrap();
    }
    let changed = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(changed.published, 2);
    assert!(changed.errors.is_empty());
    assert_eq!(changed.rows.len(), 2);
    assert_eq!(f.scan().published, 0);
    assert!(
        read_messages(&f.source)
            .unwrap()
            .last()
            .unwrap()
            .text
            .contains("Branch A")
    );
    assert!(
        read_messages(&imported)
            .unwrap()
            .last()
            .unwrap()
            .text
            .contains("Branch B")
    );
}

#[test]
fn progress_ui_shows_complete_paused_and_failed_states() {
    use egui_kittest::{Harness, kittest::Queryable};
    let f = Fixture::new(Tool::Claude);
    let status = f.scan();
    let mut initialized = false;
    let mut manager = Manager::default();
    manager.status = status;
    let mut h = Harness::builder().with_size([540.0, 470.0]).build_ui_state(
        |ui, state: &mut (Manager, bool)| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                kiln_common::Theme::current().apply(ui.ctx());
                initialized = true;
                return;
            }
            state.0.progress_ui(ui, state.1);
        },
        (manager, true),
    );
    h.run_steps(3);
    assert!(h.query_by_label("완료된 대화 모두 동기화됨").is_some());
    assert!(h.query_by_label("동기화 완료").is_some());
    assert!(
        h.ctx
            .content_rect()
            .contains_rect(h.get_by_label("이어갈 대화 ID 복사").rect())
    );
    h.render()
        .unwrap()
        .save("/tmp/kiln-sync-progress-complete.png")
        .unwrap();
    h.state_mut().1 = false;
    h.run_steps(2);
    assert!(h.query_by_label("동기화 일시 중지").is_some());
    h.state_mut().1 = true;
    let row = h.state_mut().0.status.rows.values_mut().next().unwrap();
    row.phase = Phase::Failed;
    row.detail = "Unsupported image; original transcript preserved".into();
    h.run_steps(2);
    assert!(h.query_by_label("동기화 실패").is_some());
    assert!(h.query_by_label("확인이 필요한 대화가 있습니다").is_some());
    h.render()
        .unwrap()
        .save("/tmp/kiln-sync-progress-failed.png")
        .unwrap();
}

#[test]
#[ignore]
fn verify_native_cli_continuations() {
    let dir = PathBuf::from(
        std::env::var_os("KILN_SYNC_FIXTURE_DIR").expect("explicit fixture directory"),
    );
    assert!(dir.join("owned-fixture").is_file());
    let config = dir.join("sync-state");
    private_dir(&config).unwrap();
    let sources: Vec<Source> = Tool::ALL
        .into_iter()
        .map(|tool| {
            serde_json::from_slice(&fs::read(dir.join(format!("{}.json", tool.key()))).unwrap())
                .unwrap()
        })
        .collect();
    let pair = Pair {
        claude: Endpoint {
            root: sources[0].root.clone(),
            label: "fixture".into(),
        },
        codex: Endpoint {
            root: sources[1].root.clone(),
            label: "fixture".into(),
        },
    };
    for source in &sources {
        let messages = read_messages(source).unwrap();
        assert!(
            messages
                .iter()
                .any(|m| m.text.contains("Remember the exact token"))
        );
        assert!(
            messages
                .iter()
                .any(|m| m.text.contains("Mock continuation"))
        );
        let record = super::super::agent_runtime::Record {
            tool: source.tool,
            spec: kiln_proto::SpawnSpec {
                cwd: Some(source.cwd.clone()),
                env: vec![(
                    env_key(source.tool).into(),
                    source.root.to_string_lossy().into_owned(),
                )],
                ..Default::default()
            },
            version: "fixture".into(),
            conversation: Some(source.id.clone()),
            generation: "fixture".into(),
            confirmed: true,
            request_started: false,
        };
        track(&config, &record, source.path.to_str().unwrap()).unwrap();
    }
    let gate = Arc::new(AtomicBool::new(true));
    let status = scan(&config, &pair, &gate, &mut Cache::new(), |_| {}).unwrap();
    assert_eq!(status.published, 2, "{:?}", status.errors);
    assert!(status.errors.is_empty());
}

#[test]
fn recovery_reuses_publication_plan_and_never_clobbers_a_collision() {
    for collision in [false, true] {
        let f = Fixture::new(Tool::Codex);
        let dir = f.config.join("conversation-sync");
        let state_path = dir.join(format!(
            "{}.json",
            digest(&serde_json::to_vec(&f.pair).unwrap())
        ));
        let fingerprint = digest(&serde_json::to_vec(&messages()).unwrap());
        let key = f.source.path.to_string_lossy();
        let id = stable_id(&format!("{}:{key}:{fingerprint}", state_path.display()));
        let date = chrono::DateTime::parse_from_rfc3339("2026-10-12T23:59:59.000Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let target = destination(&f.pair, &f.source, &id, date).unwrap();
        let timestamp = date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        publish(
            &dir.join(format!("pending-{id}.json")),
            &serde_json::to_vec(&json!({"source":target,"timestamp":timestamp})).unwrap(),
        )
        .unwrap();
        let mut ms = messages();
        ms.push(Message {
            role: "user".into(),
            text: "Already continued after crash".into(),
        });
        ms.push(Message {
            role: "assistant".into(),
            text: "Native reply".into(),
        });
        let bytes = if collision {
            b"unrelated conversation\n".to_vec()
        } else {
            format::render(target.tool, &id, &target.cwd, &timestamp, &ms).unwrap()
        };
        publish(&target.path, &bytes).unwrap();
        let status = f.scan();
        assert_eq!(fs::read(&target.path).unwrap(), bytes);
        if collision {
            assert_eq!(status.published, 0);
            assert!(status.errors[0].contains("collision"));
        } else {
            assert_eq!(status.published, 1);
            assert_eq!(f.scan().published, 1);
            assert_eq!(f.scan().published, 0);
        }
    }
}

#[test]
fn claude_project_encoding_matches_utf16_and_compaction_is_explicit() {
    let f = Fixture::new(Tool::Codex);
    let mut source = f.source.clone();
    source.cwd = "/tmp/🦒한글/a_b.c".into();
    let target = destination(&f.pair, &source, &stable_id("unicode"), chrono::Utc::now()).unwrap();
    assert_eq!(
        target.path.parent().unwrap().file_name().unwrap(),
        "-tmp------a-b-c"
    );
    let f = Fixture::new(Tool::Claude);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    data[0]["isCompactSummary"] = json!(true);
    assert!(
        format::parse(Tool::Claude, &data)
            .unwrap_err()
            .to_string()
            .contains("compacted history")
    );
}

fn workspace_scope(f: &Fixture) -> discovery::Scope {
    discovery::Scope {
        workspaces: vec![PathBuf::from(&f.source.cwd)],
        roots: Tool::ALL
            .into_iter()
            .map(|t| (t, f.pair.endpoint(t).root.clone()))
            .collect(),
    }
}
fn workspace_scan(f: &Fixture, scope: &discovery::Scope) -> Status {
    scan_scoped(
        &f.config,
        &f.pair,
        &f.gate,
        &mut Cache::new(),
        Some(scope),
        |_| {},
    )
    .unwrap()
}
fn workspace_state(f: &Fixture) -> State {
    serde_json::from_slice(
        &fs::read(f.config.join("conversation-sync/workspace-state.json")).unwrap(),
    )
    .unwrap()
}
#[test]
fn existing_sessions_are_discovered_without_hooks_and_without_echo() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
        let original = fs::read(&f.source.path).unwrap();
        let scope = workspace_scope(&f);
        let first = workspace_scan(&f, &scope);
        assert_eq!(first.published, 1);
        assert_eq!(first.rows.len(), 1);
        assert!(first.errors.is_empty());
        let again = workspace_scan(&f, &scope);
        assert_eq!(again.published, 0);
        assert_eq!(again.rows.len(), 1);
        assert_eq!(fs::read(&f.source.path).unwrap(), original);
        let state = workspace_state(&f);
        let copy = state.entries.values().find(|e| e.imported).unwrap();
        assert_eq!(read_messages(&copy.source).unwrap(), messages());
    }
}
#[test]
fn default_and_connected_namespaces_receive_copies_and_continued_copies_sync_back() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let mut scope = workspace_scope(&f);
    let defaults = f._temp.path().join("defaults");
    let mut default_pair = f.pair.clone();
    default_pair.claude.root = defaults.join("claude");
    default_pair.codex.root = defaults.join("codex");
    for tool in Tool::ALL {
        private_dir(&default_pair.endpoint(tool).root).unwrap();
        scope
            .roots
            .push((tool, default_pair.endpoint(tool).root.clone()));
    }
    let default_source = destination(
        &default_pair,
        &Source {
            tool: Tool::Codex,
            ..f.source.clone()
        },
        &stable_id("external-existing"),
        chrono::Utc::now(),
    )
    .unwrap();
    publish(
        &default_source.path,
        &fs::read(&f.source.path)
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // The native session identity, rather than its filename alone, is authoritative.
    fs::write(
        &default_source.path,
        format::render(
            Tool::Claude,
            &default_source.id,
            &default_source.cwd,
            "2026-10-13T00:00:00Z",
            &messages(),
        )
        .unwrap(),
    )
    .unwrap();
    fs::remove_file(&f.source.path).unwrap();
    let first = workspace_scan(&f, &scope);
    assert_eq!(first.published, 2);
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.rows.values().next().unwrap().copies.len(), 2);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    let state = workspace_state(&f);
    let copy = state
        .entries
        .values()
        .find(|e| e.imported && e.source.root == default_pair.codex.root)
        .unwrap();
    let mut continued = messages();
    continued.extend([
        Message {
            role: "user".into(),
            text: "Continue in the already-open default CLI".into(),
        },
        Message {
            role: "assistant".into(),
            text: "Continued exact history".into(),
        },
    ]);
    fs::write(
        &copy.source.path,
        format::render(
            Tool::Codex,
            &copy.source.id,
            &copy.source.cwd,
            "2026-10-13T00:00:00Z",
            &continued,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(workspace_scan(&f, &scope).published, 2);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    let state = workspace_state(&f);
    for root in [&f.pair.claude.root, &default_pair.claude.root] {
        assert!(state.entries.values().any(|e| e.imported
            && &e.source.root == root
            && read_messages(&e.source).unwrap() == continued));
    }
}
#[test]
fn workspace_filter_includes_children_excludes_neighbors_and_pauses_closed_folders() {
    let f = Fixture::new(Tool::Codex);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let project = f._temp.path().join("project");
    let child = project.join("child");
    let neighbor = f._temp.path().join("project-other");
    for path in [&project, &child, &neighbor] {
        fs::create_dir_all(path).unwrap();
    }
    let mut scope = workspace_scope(&f);
    scope.workspaces = vec![project.canonicalize().unwrap()];
    for (index, cwd) in [&project, &child, &neighbor].into_iter().enumerate() {
        let seed = Source {
            tool: Tool::Claude,
            cwd: cwd.to_string_lossy().into_owned(),
            ..f.source.clone()
        };
        let source = destination(
            &f.pair,
            &seed,
            &stable_id(&format!("folder-{index}")),
            chrono::Utc::now(),
        )
        .unwrap();
        publish(
            &source.path,
            &format::render(
                Tool::Codex,
                &source.id,
                &source.cwd,
                "2026-10-13T00:00:00Z",
                &messages(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    assert_eq!(workspace_scan(&f, &scope).published, 2);
    scope.workspaces.clear();
    let closed = workspace_scan(&f, &scope);
    assert_eq!(closed.published, 0);
    assert_eq!(closed.watched, 0);
}
#[test]
fn initial_sync_is_latest_twenty_per_workspace_then_only_new_or_changed_sessions() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    fs::remove_file(&f.source.path).unwrap();
    let scope = workspace_scope(&f);
    let mut sources = Vec::new();
    for index in 0..27 {
        let tool = if index % 2 == 0 {
            Tool::Claude
        } else {
            Tool::Codex
        };
        let source = destination(
            &f.pair,
            &Source {
                tool: opposite(tool),
                ..f.source.clone()
            },
            &stable_id(&format!("history-{index}")),
            chrono::Utc::now(),
        )
        .unwrap();
        publish(
            &source.path,
            &format::render(
                tool,
                &source.id,
                &source.cwd,
                "2026-10-13T00:00:00Z",
                &messages(),
            )
            .unwrap(),
        )
        .unwrap();
        fs::File::open(&source.path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100 + index)),
            )
            .unwrap();
        sources.push(source);
    }
    let first = workspace_scan(&f, &scope);
    assert_eq!(first.published, 20);
    assert_eq!(first.rows.len(), 20);
    assert!(sources[..7].iter().all(|s| {
        !first
            .rows
            .contains_key(&s.path.to_string_lossy().into_owned())
    }));
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    // Deletion cannot make the initial backfill drift further into older history.
    fs::remove_file(&sources[26].path).unwrap();
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    // An older already-open session is picked up as soon as it saves another update.
    fs::File::open(&sources[0].path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))
        .unwrap();
    let changed = workspace_scan(&f, &scope);
    assert_eq!(changed.published, 1);
    assert_eq!(changed.rows.len(), 21);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    // A newly opened child workspace has its own initial limit, even though its
    // files are newer than the existing parent's first discovery boundary.
    let child = PathBuf::from(&f.source.cwd).join("new-workspace");
    fs::create_dir(&child).unwrap();
    for index in 0..25 {
        let seed = Source {
            tool: Tool::Codex,
            cwd: child.to_string_lossy().into_owned(),
            ..f.source.clone()
        };
        let source = destination(
            &f.pair,
            &seed,
            &stable_id(&format!("new-folder-{index}")),
            chrono::Utc::now(),
        )
        .unwrap();
        publish(
            &source.path,
            &format::render(
                Tool::Claude,
                &source.id,
                &source.cwd,
                "2026-10-13T00:00:00Z",
                &messages(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut scope = scope;
    scope.workspaces.push(child);
    assert_eq!(workspace_scan(&f, &scope).published, 20);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
}

#[test]
#[ignore]
fn inspect_existing_workspace_metadata_read_only() {
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            std::env::var_os("KILN_SYNC_INSPECT_MANIFEST").expect("explicit metadata manifest"),
        )
        .unwrap(),
    )
    .unwrap();
    let config = tempfile::tempdir().unwrap();
    private_dir(&config.path().join("conversation-sync")).unwrap();
    let scope = discovery::Scope {
        workspaces: serde_json::from_value(manifest["workspaces"].clone()).unwrap(),
        roots: serde_json::from_value(manifest["roots"].clone()).unwrap(),
    };
    let start = Instant::now();
    let sources = discovery::find(
        config.path(),
        &scope,
        &Arc::new(AtomicBool::new(true)),
        &mut discovery::MetadataCache::new(),
    )
    .unwrap();
    eprintln!(
        "Discovery: {} sources in {:?}",
        sources.len(),
        start.elapsed()
    );
    for source in sources {
        // No prompts, content, session IDs or account identities enter diagnostics.
        match read_messages(&source) {
            Ok(messages) => eprintln!(
                "{}: {} portable messages",
                source.tool.key(),
                messages.len()
            ),
            Err(error) => eprintln!("{}: {}", source.tool.key(), error),
        }
    }
}

#[test]
fn multiple_destination_retry_retains_the_successful_publication_plan() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let mut scope = workspace_scope(&f);
    let unavailable = PathBuf::from(&f.source.cwd).join("not-created-yet");
    scope.roots.push((Tool::Codex, unavailable.clone()));
    let first = workspace_scan(&f, &scope);
    assert_eq!(first.published, 1);
    assert_eq!(first.rows.values().next().unwrap().phase, Phase::Failed);
    let plans: Vec<_> = fs::read_dir(f.config.join("conversation-sync"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("pending-"))
        .collect();
    assert_eq!(plans.len(), 1);
    let state = workspace_state(&f);
    let successful = state
        .entries
        .values()
        .find(|e| e.imported)
        .unwrap()
        .source
        .clone();
    let bytes = fs::read(&successful.path).unwrap();
    private_dir(&unavailable).unwrap();
    let recovered = workspace_scan(&f, &scope);
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.rows.values().next().unwrap().copies.len(), 2);
    assert_eq!(fs::read(&successful.path).unwrap(), bytes);
    assert!(!plans[0].path().exists());
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    assert_eq!(
        workspace_state(&f)
            .entries
            .values()
            .filter(|e| e.imported)
            .count(),
        2
    );
}

#[test]
fn workspace_discovery_skips_symlinks_and_subagents_and_surfaces_bad_versions() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(Tool::Codex);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let scope = workspace_scope(&f);
    let original = fs::read(&f.source.path).unwrap();
    let linked = f
        .source
        .path
        .parent()
        .unwrap()
        .join(format!("rollout-link-{}.jsonl", f.source.id));
    symlink(&f.source.path, &linked).unwrap();
    let subagent = destination(
        &f.pair,
        &Source {
            tool: Tool::Claude,
            ..f.source.clone()
        },
        &stable_id("subagent"),
        chrono::Utc::now(),
    )
    .unwrap();
    let mut sub_rows = rows(
        &format::render(
            Tool::Codex,
            &subagent.id,
            &subagent.cwd,
            "2026-10-13T00:00:00Z",
            &messages(),
        )
        .unwrap(),
    );
    sub_rows[0]["payload"]["parent_thread_id"] = json!(f.source.id);
    let bytes = sub_rows
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect::<String>();
    publish(&subagent.path, bytes.as_bytes()).unwrap();
    assert_eq!(workspace_scan(&f, &scope).rows.len(), 1);
    fs::write(
        &f.source.path,
        String::from_utf8(original)
            .unwrap()
            .replace("0.162.1", "99.0.0"),
    )
    .unwrap();
    let failed = workspace_scan(&f, &scope);
    assert_eq!(failed.rows.len(), 1);
    assert_eq!(failed.rows.values().next().unwrap().phase, Phase::Failed);
    assert!(
        failed
            .rows
            .values()
            .next()
            .unwrap()
            .detail
            .contains("Unsupported Codex transcript version")
    );
}

#[test]
#[ignore]
fn export_discovered_workspace_fixtures() {
    export_native_cli_fixtures();
    let dir = PathBuf::from(std::env::var_os("KILN_SYNC_FIXTURE_DIR").unwrap())
        .canonicalize()
        .unwrap();
    let pair = Pair {
        claude: Endpoint {
            root: dir.join("claude"),
            label: "fixture".into(),
        },
        codex: Endpoint {
            root: dir.join("codex"),
            label: "fixture".into(),
        },
    };
    let scope = discovery::Scope {
        workspaces: vec![dir.clone()],
        roots: Tool::ALL
            .into_iter()
            .map(|t| (t, pair.endpoint(t).root.clone()))
            .collect(),
    };
    let config = dir.join("workspace-sync");
    let result = scan_scoped(
        &config,
        &pair,
        &Arc::new(AtomicBool::new(true)),
        &mut Cache::new(),
        Some(&scope),
        |_| {},
    )
    .unwrap();
    assert_eq!(result.published, 2);
    assert!(result.errors.is_empty());
    assert!(!config.join("conversation-sync/tracked").exists());
    let state: State = serde_json::from_slice(
        &fs::read(config.join("conversation-sync/workspace-state.json")).unwrap(),
    )
    .unwrap();
    let copies: Vec<_> = state
        .entries
        .values()
        .filter(|e| e.imported)
        .map(|e| e.source.clone())
        .collect();
    fs::write(
        dir.join("continuations.json"),
        serde_json::to_vec_pretty(&copies).unwrap(),
    )
    .unwrap();
}

#[test]
fn workspace_progress_distinguishes_discovery_empty_and_completed_batches() {
    use egui_kittest::{Harness, kittest::Queryable};
    let f = Fixture::new(Tool::Claude);
    let scope = workspace_scope(&f);
    let status = workspace_scan(&f, &scope);
    let mut manager = Manager::default();
    manager.configured = Some(f.pair.clone());
    manager.status = status;
    let mut initialized = false;
    let mut h = Harness::builder().with_size([580.0, 490.0]).build_ui_state(
        |ui, manager: &mut Manager| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                kiln_common::Theme::current().apply(ui.ctx());
                initialized = true;
                return;
            }
            manager.progress_ui(ui, true);
        },
        manager,
    );
    h.run_steps(3);
    assert!(h.query_by_label("워크스페이스 1개 · 대화 1개").is_some());
    assert!(h.query_by_label("완료된 대화 모두 동기화됨").is_some());
    h.render()
        .unwrap()
        .save("/tmp/kiln-workspace-sync-complete.png")
        .unwrap();
    h.state_mut().status.discovering = true;
    h.run_steps(2);
    assert!(h.query_by_label("워크스페이스 대화 찾는 중").is_some());
    assert!(
        h.query_by_label("동기화 완료").is_some(),
        "discovery retains previous rows"
    );
    h.state_mut().status = Status {
        workspaces: 1,
        ..Default::default()
    };
    h.run_steps(2);
    assert!(
        h.query_by_label("연결됨 · 해당 폴더에서 대화를 찾지 못함")
            .is_some()
    );
    h.render()
        .unwrap()
        .save("/tmp/kiln-workspace-sync-empty.png")
        .unwrap();
}

#[test]
#[ignore]
fn verify_discovered_workspace_cli_continuations() {
    let dir = PathBuf::from(
        std::env::var_os("KILN_SYNC_FIXTURE_DIR").expect("explicit fixture directory"),
    )
    .canonicalize()
    .unwrap();
    assert!(dir.join("owned-fixture").is_file());
    let sources: Vec<Source> =
        serde_json::from_slice(&fs::read(dir.join("continuations.json")).unwrap()).unwrap();
    for source in &sources {
        let parsed = read_messages(source).unwrap();
        assert_eq!(&parsed[..2], &messages());
        assert!(
            parsed
                .iter()
                .any(|m| m.text == "Mock continuation: kiln-orchid-731.")
        );
    }
    let pair = Pair {
        claude: Endpoint {
            root: dir.join("claude"),
            label: "fixture".into(),
        },
        codex: Endpoint {
            root: dir.join("codex"),
            label: "fixture".into(),
        },
    };
    let scope = discovery::Scope {
        workspaces: vec![dir.clone()],
        roots: Tool::ALL
            .into_iter()
            .map(|t| (t, pair.endpoint(t).root.clone()))
            .collect(),
    };
    let gate = Arc::new(AtomicBool::new(true));
    let config = dir.join("workspace-sync");
    let result = scan_scoped(
        &config,
        &pair,
        &gate,
        &mut Cache::new(),
        Some(&scope),
        |_| {},
    )
    .unwrap();
    assert_eq!(result.published, 2);
    assert!(result.errors.is_empty());
    let again = scan_scoped(
        &config,
        &pair,
        &gate,
        &mut Cache::new(),
        Some(&scope),
        |_| {},
    )
    .unwrap();
    assert_eq!(again.published, 0);
}

#[test]
fn failed_native_registration_is_not_reported_as_synchronized_and_retries_same_copy() {
    let f = Fixture::new(Tool::Claude);
    let scope = workspace_scope(&f);
    let failed = scan_with_index(
        &f.config,
        &f.pair,
        &f.gate,
        &mut Cache::new(),
        Some(&scope),
        |_| anyhow::bail!("fixture index unavailable"),
        |_| {},
    )
    .unwrap();
    assert_eq!(failed.published, 0);
    assert_eq!(failed.rows.values().next().unwrap().phase, Phase::Failed);
    let plans: Vec<_> = fs::read_dir(f.config.join("conversation-sync"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("pending-"))
        .collect();
    assert_eq!(plans.len(), 1);
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(plans[0].path()).unwrap()).unwrap();
    let target: Source = serde_json::from_value(plan["source"].clone()).unwrap();
    let original = fs::read(&target.path).unwrap();
    let complete = scan_with_index(
        &f.config,
        &f.pair,
        &f.gate,
        &mut Cache::new(),
        Some(&scope),
        |source| {
            assert_eq!(source.path, target.path);
            assert_eq!(fs::read(&source.path).unwrap(), original);
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(complete.published, 1);
    assert_eq!(complete.rows.values().next().unwrap().phase, Phase::Synced);
    assert!(!plans[0].path().exists());
}

#[test]
fn workspace_opt_in_defaults_off_and_round_trips_without_global_migration() {
    let old: super::super::state::Persist = serde_json::from_value(serde_json::json!({
        "settings": {"conversation_sync_enabled": true},
        "workspaces": [{"name":"A","root":"/example/a"}, {"name":"B","root":"/example/b"}]
    })).unwrap();
    assert!(old.settings.conversation_sync_enabled);
    assert!(old.workspaces.iter().all(|w| !w.conversation_sync));
    let mut selected = old;
    selected.workspaces[0].conversation_sync = true;
    let restored: super::super::state::Persist = serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap();
    assert!(restored.workspaces[0].conversation_sync);
    assert!(!restored.workspaces[1].conversation_sync);
}

#[test]
fn no_opted_in_workspaces_cancel_inflight_and_ignore_stale_results() {
    let f = Fixture::new(Tool::Claude);
    let mut manager = Manager::default();
    manager.gate.store(true, Ordering::Release);
    let old_gate = manager.gate.clone();
    manager.busy = true;
    let mut settings = super::super::state::Settings::default();
    settings.conversation_sync_enabled = true;
    settings.conversation_sync_pair = Some(f.pair.clone());
    manager.tick(&mut settings, vec![], &egui::Context::default());
    assert!(!old_gate.load(Ordering::Acquire));
    assert!(manager.workspaces.is_empty());
    manager.tx.send(Event::Progress(old_gate.clone(), f.scan())).unwrap();
    manager.tx.send(Event::Scanned(old_gate, Err("stale cancelled scan".into()))).unwrap();
    manager.tick(&mut settings, vec![], &egui::Context::default());
    assert!(!manager.busy);
    assert!(manager.status.rows.is_empty());
    assert!(manager.error.is_none());
}

#[test]
fn workspace_status_and_badges_do_not_mix_other_folders() {
    let f = Fixture::new(Tool::Claude);
    let mut manager = Manager::default();
    manager.status = f.scan();
    let original = manager.status.rows.values().next().unwrap().clone();
    manager.workspaces = vec!["/example/a".into(), "/example/b".into(), "/example/a/child".into()];
    manager.status.rows.clear();
    for (path, phase) in [("/example/a", Phase::Synced), ("/example/b", Phase::Failed), ("/example/a/child", Phase::Copying)] {
        let mut row = original.clone(); row.source.cwd = path.into(); row.phase = phase;
        manager.status.rows.insert(path.into(), row);
    }
    assert_eq!(manager.workspace_status(Path::new("/example/a")).rows.len(), 1);
    assert_eq!(manager.badge(Path::new("/example/a")).0, kiln_common::icons::Icon::Check);
    assert_eq!(manager.badge(Path::new("/example/b")).0, kiln_common::icons::Icon::Warning);
    assert_eq!(manager.badge(Path::new("/example/a/child")).0, kiln_common::icons::Icon::Refresh);
}

#[test]
#[cfg(unix)]
fn workspace_status_uses_canonical_assignment_for_aliased_working_directories() {
    let f = Fixture::new(Tool::Claude);
    let root = PathBuf::from(&f.source.cwd);
    let alias = root.join("workspace-alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let scope = discovery::Scope { workspaces: vec![root.clone()], roots: vec![] };
    let mut source = f.source.clone(); source.cwd = alias.to_string_lossy().into_owned();
    let row = Conversation::from_entry(&Entry { source, seen: String::new(), imported: false, title: "Aliased working directory".into(), copied_to: None, copies: vec![] }, Some(&scope));
    assert_eq!(row.workspace.as_ref(), Some(&root));
    let mut manager = Manager::default(); manager.workspaces = vec![root.clone()];
    manager.status.rows.insert("fixture".into(), row);
    assert_eq!(manager.workspace_status(&root).rows.len(), 1);
}
