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
