use super::*;
use format::Message;
use serde_json::json;

fn messages() -> Vec<Message> {
    vec![
        Message {
            images: Vec::new(),
            role: "user".into(),
            text: "Remember the exact token kiln-orchid-731.\nKeep this second line.".into(),
        },
        Message {
            images: Vec::new(),
            role: "assistant".into(),
            text: "Recorded kiln-orchid-731.\n```rust\nlet exact = 731;\n```".into(),
        },
    ]
}

#[test]
fn active_append_preserves_checkpoint_but_rewrite_and_replacement_retry() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let expected = reader::rows(&f.source, &mut String::new()).unwrap();
        let actual = reader::rows_before_validation(&f.source, &mut String::new(), || {
            fs::OpenOptions::new()
                .append(true)
                .open(&f.source.path)
                .unwrap()
                .write_all(b"{\"type\":\"unfinished")
                .unwrap();
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(read_messages(&f.source).unwrap(), messages());
        let changed = reader::rows_before_validation(&f.source, &mut String::new(), || {
            fs::write(&f.source.path, b"{}\n").unwrap();
        });
        assert!(
            changed
                .unwrap_err()
                .to_string()
                .contains("changed while reading")
        );
        let replaced = reader::rows_before_validation(&f.source, &mut String::new(), || {
            let replacement = f.source.path.with_extension("replacement");
            fs::write(&replacement, b"{}\n").unwrap();
            fs::rename(replacement, &f.source.path).unwrap();
        });
        assert!(
            replaced
                .unwrap_err()
                .to_string()
                .contains("changed while reading")
        );
    }
}

#[test]
fn historical_versions_directory_changes_and_tool_references_preserve_identity() {
    let f = Fixture::new(Tool::Claude);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    data[0]["version"] = json!("2.1.289");
    data[1]["version"] = json!("2.1.299");
    data[1]["cwd"] = json!("/tmp/changed-directory");
    data[1]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool_reference","tool_name":"fixture_lookup"}));
    let bytes = data.iter().map(|r| format!("{r}\n")).collect::<String>();
    fs::write(&f.source.path, &bytes).unwrap();
    assert_eq!(f.scan().published, 1);
    assert_eq!(fs::read_to_string(&f.source.path).unwrap(), bytes);
    data[1]["sessionId"] = json!(stable_id("different-session"));
    fs::write(
        &f.source.path,
        data.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    assert!(f.scan().errors[0].contains("identity"));
}

#[test]
fn native_images_roundtrip_without_data_loss_or_sync_echo() {
    // Public one-pixel PNG fixture, not a user's attachment.
    let url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let mut expected = messages();
        expected.insert(
            1,
            Message {
                role: "user".into(),
                text: String::new(),
                images: vec![url.into()],
            },
        );
        let bytes = format::render(
            tool,
            &f.source.id,
            &f.source.cwd,
            "2026-10-13T00:00:00Z",
            &expected,
        )
        .unwrap();
        fs::write(&f.source.path, &bytes).unwrap();
        assert_eq!(read_messages(&f.source).unwrap(), expected);
        let status = f.scan();
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert_eq!(status.published, 1);
        let state = f.state();
        let copy = state.entries.values().find(|e| e.imported).unwrap();
        assert_eq!(read_messages(&copy.source).unwrap(), expected);
        assert_eq!(f.scan().published, 0);
        assert_eq!(fs::read(&f.source.path).unwrap(), bytes);
    }
}

#[test]
fn missing_image_bytes_never_marks_a_conversation_complete() {
    let f = Fixture::new(Tool::Codex);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    let user = data
        .iter_mut()
        .find(|r| r["payload"]["item"]["type"] == "UserMessage")
        .unwrap();
    user["payload"]["item"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"local_image","path":"/missing/attachment.png"}));
    fs::write(
        &f.source.path,
        data.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let status = f.scan();
    assert_eq!(status.published, 0);
    assert_eq!(status.rows.values().next().unwrap().phase, Phase::Failed);
    assert!(status.errors[0].contains("original bytes"));
}

#[test]
fn obsolete_echo_bookkeeping_is_retired_only_for_unchanged_imports() {
    let f = Fixture::new(Tool::Codex);
    assert_eq!(f.scan().published, 1);
    let mut state = f.state();
    let entry = state.entries.values_mut().find(|e| e.imported).unwrap();
    let copy = entry.source.clone();
    let before = fs::read(&copy.path).unwrap();
    entry.seen = "obsolete-parser-fingerprint".into();
    entry.copied_to = Some(Continuation {
        tool: Tool::Codex,
        id: f.source.id.clone(),
        cwd: f.source.cwd.clone(),
        root: f.source.root.clone(),
        revision: 0,
    });
    entry.copies = vec![entry.copied_to.clone().unwrap()];
    let path = f.config.join("conversation-sync").join(format!(
        "{}.json",
        digest(&serde_json::to_vec(&f.pair).unwrap())
    ));
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let status = f.scan();
    assert_eq!(status.published, 0);
    assert_eq!(status.rows.len(), 1);
    assert_eq!(fs::read(&copy.path).unwrap(), before);
    assert!(
        f.state()
            .entries
            .values()
            .find(|e| e.imported)
            .unwrap()
            .copied_to
            .is_none()
    );
}

#[test]
fn cli_looking_original_text_is_preserved_as_content_in_both_destinations() {
    let mut original = messages();
    original[0].text = "<command-name>/fixture</command-name>\nThis is original history.".into();
    for tool in Tool::ALL {
        let bytes = format::render(
            tool,
            &stable_id("reserved-text"),
            "/tmp/project",
            "2026-10-13T00:00:00Z",
            &original,
        )
        .unwrap();
        let data = rows(&bytes);
        assert_eq!(format::parse(tool, &data).unwrap(), original);
        if tool == Tool::Claude {
            assert!(data[0]["message"]["content"].is_array());
        }
    }
}

#[test]
fn codex_visible_only_answer_is_retained_without_duplicate_projection() {
    let f = Fixture::new(Tool::Codex);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), messages());
    data.retain(|r| !(r["type"] == "response_item" && r["payload"]["role"] == "assistant"));
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), messages());
}

#[test]
fn sync_dialog_keeps_error_visible_and_identical_across_idle_polls() {
    use egui_kittest::{Harness, kittest::Queryable};
    let f = Fixture::new(Tool::Claude);
    let mut manager = Manager::default();
    manager.configured = Some(f.pair.clone());
    manager.status = f.scan();
    let mut row = manager.status.rows.values().next().unwrap().clone();
    row.phase = Phase::Failed;
    row.detail = "Provider file-ID image cannot be transferred without its original bytes".into();
    row.title = "Review the project screenshots and preserve every attachment".into();
    manager.status.rows.clear();
    for i in 0..20 {
        manager
            .status
            .rows
            .insert(format!("fixture-{i:02}"), row.clone());
    }
    let accounts = AccountManager::with_env(kiln_accounts::Env::sandbox(f._temp.path(), false).0);
    let mut settings = super::super::state::Settings::default();
    settings.conversation_sync_pair = Some(f.pair.clone());
    let mut initialized = false;
    let mut h = Harness::builder().with_size([760.0, 620.0]).build_ui_state(
        |ui, manager: &mut Manager| {
            if !initialized {
                kiln_common::fonts::install(ui.ctx());
                kiln_common::Theme::current().apply(ui.ctx());
                initialized = true;
                return;
            }
            manager.dialog(
                ui.ctx(),
                "Workspace fixture",
                Path::new(&f.source.cwd),
                true,
                &mut settings,
                &accounts,
            );
        },
        manager,
    );
    h.run_steps(4);
    let reason = failure_reason(&row.detail);
    let visible = h
        .query_all_by_label(&reason)
        .find(|n| h.ctx.content_rect().contains_rect(n.rect()))
        .expect("error reason visible without scrolling")
        .rect();
    let footer = h.get_by_label("다시 확인").rect();
    assert!(h.ctx.content_rect().contains_rect(footer));
    h.render()
        .unwrap()
        .save("/tmp/kiln-0125-sync-dialog.png")
        .unwrap();
    for busy in [true, false, true, false] {
        h.state_mut().busy = busy;
        h.state_mut().status.discovering = busy;
        h.run_steps(2);
        assert_eq!(h.get_by_label("다시 확인").rect(), footer);
        assert_eq!(
            h.query_all_by_label(&reason).next().unwrap().rect(),
            visible
        );
        assert!(h.query_by_label("워크스페이스 대화 찾는 중").is_none());
    }
}

#[test]
fn background_poll_keeps_failed_rows_and_does_not_retry_unchanged_files() {
    let f = Fixture::new(Tool::Claude);
    let original = fs::read(&f.source.path).unwrap();
    let mut data = rows(&original);
    data[0]["message"]["content"] = json!([{"type":"document"}]);
    fs::write(
        &f.source.path,
        data.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let mut cache = Cache::new();
    let first = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    let at = cache.attempts[&f.source.path].2;
    assert_eq!(first.rows.values().next().unwrap().phase, Phase::Failed);
    let second = scan(&f.config, &f.pair, &f.gate, &mut cache, |s| {
        assert_eq!(s.rows.values().next().unwrap().phase, Phase::Failed);
    })
    .unwrap();
    assert_eq!(first.errors, second.errors);
    assert_eq!(cache.attempts[&f.source.path].2, at);
    // A changed source is retried immediately, without waiting for the backoff.
    fs::write(&f.source.path, original).unwrap();
    assert_eq!(
        scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {})
            .unwrap()
            .published,
        1
    );
}

#[test]
fn streaming_reader_does_not_apply_transcript_limit_to_private_reasoning() {
    let f = Fixture::new(Tool::Codex);
    let original = fs::read(&f.source.path).unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&f.source.path)
        .unwrap();
    let padding = "x".repeat(33 * 1024 * 1024);
    for _ in 0..2 {
        writeln!(file, "{}", json!({"type":"response_item","payload":{"type":"reasoning","encrypted_content":padding}})).unwrap();
    }
    drop(file);
    assert!(fs::metadata(&f.source.path).unwrap().len() > LIMIT);
    assert_eq!(
        read_messages(&f.source).unwrap(),
        format::parse(Tool::Codex, &rows(&original)).unwrap()
    );
}

#[test]
fn compaction_recovers_originals_when_native_logical_parent_points_forward() {
    let f = Fixture::new(Tool::Claude);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    let boundary = stable_id("compact-boundary");
    let summary = stable_id("compact-summary");
    let user = stable_id("after-compact-user");
    data.push(json!({"type":"system","subtype":"compact_boundary","uuid":boundary,"parentUuid":null,"logicalParentUuid":summary}));
    let mut row = data[0].clone();
    row["uuid"] = json!(summary);
    row["parentUuid"] = json!(boundary);
    row["isCompactSummary"] = json!(true);
    row["message"]["content"] = json!("Summary must never substitute for original history");
    data.push(row);
    let mut row = data[0].clone();
    row["uuid"] = json!(user);
    row["parentUuid"] = json!(summary);
    row["message"]["content"] = json!("Continue after compaction");
    data.push(row);
    let mut row = data[1].clone();
    row["uuid"] = json!(stable_id("after-compact-assistant"));
    row["parentUuid"] = json!(user);
    row["message"]["content"] = json!([{"type":"text","text":"Original continuation"}]);
    data.push(row);
    let actual = format::parse(Tool::Claude, &data).unwrap();
    assert_eq!(&actual[..2], &messages());
    assert_eq!(actual.len(), 4);
    assert_eq!(actual[2].text, "Continue after compaction");
    assert!(!actual.iter().any(|m| m.text.contains("Summary must")));
}

#[test]
fn initial_selection_migrates_past_metadata_only_sessions() {
    let f = Fixture::new(Tool::Codex);
    let scope = workspace_scope(&f);
    let mut empty_paths = Vec::new();
    for i in 0..25 {
        let mut source = f.source.clone();
        source.id = stable_id(&format!("empty-{i}"));
        source.path = f
            .source
            .path
            .with_file_name(format!("rollout-{}.jsonl", source.id));
        let mut meta = rows(&fs::read(&f.source.path).unwrap())[0].clone();
        meta["payload"]["id"] = json!(source.id);
        fs::write(&source.path, format!("{meta}\n")).unwrap();
        empty_paths.push(source.path);
    }
    let key = digest(&serde_json::to_vec(&(scope.workspaces[0].clone(), &scope.roots)).unwrap());
    let seeds = json!({key:{"started":SystemTime::now(),"paths":empty_paths[..20]}});
    fs::write(
        f.config.join("conversation-sync/workspace-discovery.json"),
        serde_json::to_vec(&seeds).unwrap(),
    )
    .unwrap();
    let found = discovery::find(
        &f.config,
        &scope,
        &f.gate,
        &mut discovery::MetadataCache::new(),
    )
    .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, f.source.id);
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
            images: Vec::new(),
            role: "user".into(),
            text: "Continue from the other CLI.".into(),
        });
        extended.push(Message {
            images: Vec::new(),
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
            extended,
            "the original native ID receives the continued history"
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
fn repeated_resume_updates_the_same_two_native_ids_and_preserves_existing_bytes() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        assert_eq!(f.scan().published, 1);
        let peer = f.state().entries.values().find(|e| e.source.tool == opposite(tool)).unwrap().source.clone();
        let original = fs::read(&f.source.path).unwrap();
        let mut expected = messages();
        for round in 0..3 {
            let (writer, receiver) = if round % 2 == 0 { (&peer, &f.source) } else { (&f.source, &peer) };
            let before = fs::read(&receiver.path).unwrap();
            expected.push(Message { role: "user".into(), text: format!("Question {round}"), images: vec![] });
            expected.push(Message { role: "assistant".into(), text: format!("Answer {round}"), images: vec![] });
            fs::write(&writer.path, format::render(writer.tool, &writer.id, &writer.cwd, "2026-10-14T00:00:00Z", &expected).unwrap()).unwrap();
            let status = f.scan();
            assert!(status.errors.is_empty(), "{:?}", status.errors);
            assert_eq!(f.state().entries.len(), 2, "a continuation must not create a third native session");
            assert_eq!(read_messages(receiver).unwrap(), expected);
            assert!(fs::read(&receiver.path).unwrap().starts_with(&before), "native records must survive byte for byte");
            assert_eq!(f.scan().published, 0, "the appended history must not echo back");
        }
        assert!(!original.is_empty());
    }
}

#[test]
fn divergent_linked_histories_are_preserved_and_report_a_conflict() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.state().entries.values().find(|e| e.imported).unwrap().source.clone();
    for source in [&f.source, &peer] {
        let mut ms = messages();
        ms.push(Message { role: "user".into(), text: source.tool.key().into(), images: vec![] });
        ms.push(Message { role: "assistant".into(), text: "Independent reply".into(), images: vec![] });
        fs::write(&source.path, format::render(source.tool, &source.id, &source.cwd, "2026-10-14T00:00:00Z", &ms).unwrap()).unwrap();
    }
    let before = [fs::read(&f.source.path).unwrap(), fs::read(&peer.path).unwrap()];
    let status = f.scan();
    assert!(status.errors.iter().any(|e| e.contains("diverged")), "{:?}", status.errors);
    assert_eq!(f.state().entries.len(), 2);
    assert_eq!(fs::read(&f.source.path).unwrap(), before[0]);
    assert_eq!(fs::read(&peer.path).unwrap(), before[1]);
}

#[test]
fn unchanged_conflicting_original_and_continued_copy_wait_for_the_retry_delay() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.state().entries.values().find(|e| e.imported).unwrap().source.clone();
    for source in [&f.source, &peer] {
        let mut ms = messages();
        ms.push(Message { role: "user".into(), text: source.tool.key().into(), images: vec![] });
        ms.push(Message { role: "assistant".into(), text: "Independent reply".into(), images: vec![] });
        fs::write(&source.path, format::render(source.tool, &source.id, &source.cwd, "2026-10-14T00:00:00Z", &ms).unwrap()).unwrap();
    }
    let mut cache = Cache::new();
    let first = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(first.errors.len(), 2, "{:?}", first.errors);
    let read_at = |cache: &Cache| [&f.source, &peer].map(|s| cache.attempts[&s.path].2);
    let attempted = read_at(&cache);
    let second = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(read_at(&cache), attempted, "neither history is read again");
    assert_eq!(second.errors, first.errors, "both conflicts stay reported");
    assert_eq!(cache.failures[&peer.path], 1);
    cache.expedite();
    scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert!(read_at(&cache).iter().zip(attempted).all(|(now, before)| *now > before));
}

#[test]
fn retry_delay_grows_from_thirty_seconds_to_two_minutes() {
    assert_eq!([1, 2, 3, 9].map(retry_delay), [30, 60, 120, 120].map(Duration::from_secs));
}

#[test]
fn transcript_written_within_the_quiet_window_waits_without_being_read() {
    let f = Fixture::new(Tool::Claude);
    let mut cache = Cache::new();
    cache.quiet = Duration::from_secs(3600);
    let waiting = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(waiting.published, 0);
    let row = waiting.rows.values().next().unwrap();
    assert_eq!(row.phase, Phase::Waiting);
    assert!(row.detail.contains("being written"), "{}", row.detail);
    assert!(!cache.attempts.contains_key(&f.source.path));
    assert!(!cache.failures.contains_key(&f.source.path));
    cache.quiet = Duration::ZERO;
    assert_eq!(scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap().published, 1);
}

#[test]
fn streamed_hashes_and_lengths_equal_the_encoded_bytes() {
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let value = json!({"text": "줄\n\"quoted\"", "messages": messages()});
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(json_digest(&value).unwrap(), digest(&bytes));
    assert_eq!(json_len(&value).unwrap(), bytes.len() as u64);
    assert_eq!(json_digest(&messages()).unwrap(), digest(&serde_json::to_vec(&messages()).unwrap()));
}

#[test]
fn open_managed_endpoint_defers_updates_until_the_daemon_exit_lease_releases() {
    let f = Fixture::new(Tool::Codex);
    f.scan();
    let peer = f.state().entries.values().find(|e| e.imported).unwrap().source.clone();
    let path = f.config.join("agent-sessions/peer.json");
    private_dir(path.parent().unwrap()).unwrap();
    let record = super::super::agent_runtime::Record { tool: peer.tool, spec: kiln_proto::SpawnSpec {
        cwd: Some(peer.cwd.clone()), env: vec![(env_key(peer.tool).into(), peer.root.to_string_lossy().into_owned())], ..Default::default()
    }, version: String::new(), generation: "fixture".into(), conversation: Some(peer.id.clone()), confirmed: true, request_started: false };
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let lease = handover::live_lease(&path).unwrap();
    let mut extended = messages();
    extended.push(Message { role:"user".into(),text:"A new turn".into(),images:vec![] });
    extended.push(Message { role:"assistant".into(),text:"New answer".into(),images:vec![] });
    fs::write(&f.source.path, format::render(f.source.tool, &f.source.id, &f.source.cwd, "2026-10-14T00:00:00Z", &extended).unwrap()).unwrap();
    let before = fs::read(&peer.path).unwrap();
    let waiting = f.scan();
    assert_eq!(waiting.published, 0);
    assert!(waiting.rows.values().any(|r| r.phase == Phase::Waiting && r.detail.contains("still open")));
    assert_eq!(fs::read(&peer.path).unwrap(), before);
    assert!(waiting.settled.is_empty());
    drop(lease);
    let completed = f.scan();
    assert!(completed.errors.is_empty(), "{:?}", completed.errors);
    assert_eq!(read_messages(&peer).unwrap(), extended);
    assert_eq!(completed.settled.len(), 1);
}

#[test]
fn prompts_continue_while_their_history_is_being_synchronized() {
    let f = Fixture::new(Tool::Codex);
    f.scan();
    let record = super::super::agent_runtime::Record { tool: Tool::Codex, spec: kiln_proto::SpawnSpec {
        cwd: Some(f.source.cwd.clone()), env:vec![(env_key(Tool::Codex).into(),f.source.root.to_string_lossy().into_owned())], ..Default::default()
    }, version:String::new(), generation:"panel".into(), conversation:Some(f.source.id.clone()), confirmed:true, request_started:false };
    assert!(handover::link(&handover::links(&f.config).unwrap(), &record).is_some());
    let _syncing = handover::write_guard(&f.config, &f.source).unwrap();
    let _publishing = handover::lock(&f.config).unwrap();
    assert!(handover::event(&f.config, &f.config.join("panel.json"), &record, "UserPromptSubmit", None).unwrap().is_none());
}

#[test]
fn syncing_one_conversation_does_not_lock_unrelated_native_starts() {
    let f = Fixture::new(Tool::Codex);
    f.scan();
    let _guard = handover::write_guard(&f.config, &f.source).unwrap();
    let other = super::super::agent_runtime::Record { tool: Tool::Codex, spec: kiln_proto::SpawnSpec {
        cwd: Some(f.source.cwd.clone()), env:vec![(env_key(Tool::Codex).into(),f.source.root.to_string_lossy().into_owned())], ..Default::default()
    }, version:String::new(), generation:"other".into(), conversation:Some(stable_id("unrelated-native")), confirmed:true, request_started:false };
    let mut links = handover::links(&f.config).unwrap();
    links.push(handover::Link { tool:Tool::Codex, root:f.source.root.clone(), id:other.conversation.clone().unwrap(), group:"unrelated-group".into(), canonical_id:other.conversation.clone().unwrap() });
    fs::write(f.config.join("conversation-sync/native-links.json"),serde_json::to_vec(&links).unwrap()).unwrap();
    assert!(handover::event(&f.config,&f.config.join("other.json"),&other,"SessionStart",None).is_ok());
    assert!(handover::write_guard(&f.config,&f.source).is_err(), "same conversation remains locked");
}
#[test]
fn old_snapshot_identity_recovers_lineage_without_matching_unrelated_text() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let state_path = f.config.join("conversation-sync").join(format!("{}.json", digest(&serde_json::to_vec(&f.pair).unwrap())));
    let fingerprint = digest(&serde_json::to_vec(&messages()).unwrap());
    let title = titles::fallback(&messages());
    let key = f.source.path.to_string_lossy().into_owned();
    let old_id = stable_id(&format!("{}:v4:{key}:{fingerprint}:{}", state_path.display(), digest(title.as_bytes())));
    let old = destination(&f.pair, &f.source, &old_id, chrono::Utc::now()).unwrap();
    publish(&old.path, &format::render_titled(old.tool, &old.id, &old.cwd, "2026-10-14T00:00:00Z", &messages(), &title).unwrap()).unwrap();
    discovery::remember_projection(&f.config, &old, &fingerprint, Some(&messages())).unwrap();
    let mut state = f.state();
    state.entries.insert(old.path.to_string_lossy().into_owned(), Entry { source:old.clone(), seen:fingerprint, imported:true, title, copied_to:None, copies:vec![] });
    replace(&state_path, &serde_json::to_vec(&state).unwrap()).unwrap();
    f.scan();
    let state = f.state();
    assert!(canonical::members(&state, &key).contains(&old.path.to_string_lossy().into_owned()), "the exact historical publication hash proves this edge");
}

#[test]
fn redundant_projection_is_archived_with_recoverable_bytes_only_after_prefix_verification() {
    for (diverged, compressed) in [(false, false), (true, false), (false, true), (true, true)] {
        let f = Fixture::new(Tool::Claude);
        f.scan();
        let mut state = f.state();
        let peer = state.entries.values().find(|e| e.imported).unwrap().source.clone();
        let alias = destination(&f.pair, &f.source, &stable_id("old-alias"), chrono::Utc::now()).unwrap();
        let mut ms = messages();
        if diverged { ms[1].text = "A different branch".into(); }
        let bytes = format::render(alias.tool, &alias.id, &alias.cwd, "2026-10-14T00:00:00Z", &ms).unwrap();
        publish(&alias.path, &bytes).unwrap();
        let physical = if compressed {
            let path = alias.path.with_extension("jsonl.zst");
            fs::write(&path, zstd::stream::encode_all(bytes.as_slice(),3).unwrap()).unwrap();
            fs::remove_file(&alias.path).unwrap();
            path
        } else { alias.path.clone() };
        let archived_bytes = fs::read(&physical).unwrap();
        let fingerprint = digest(&serde_json::to_vec(&ms).unwrap());
        discovery::remember_projection(&f.config, &alias, &fingerprint, Some(&ms)).unwrap();
        let key = alias.path.to_string_lossy().into_owned();
        state.entries.insert(key.clone(), Entry { source:alias.clone(), seen:fingerprint, imported:true, title:titles::fallback(&ms), copied_to:None, copies:vec![] });
        state.lineage.insert(key.clone(), f.source.path.to_string_lossy().into_owned());
        let path = f.config.join("conversation-sync").join(format!("{}.json", digest(&serde_json::to_vec(&f.pair).unwrap())));
        replace(&path, &serde_json::to_vec(&state).unwrap()).unwrap();
        f.scan();
        let state = f.state();
        assert!(peer.path.exists());
        if diverged {
            assert_eq!(fs::read(&physical).unwrap(), archived_bytes);
            assert!(!state.archived.contains_key(&key));
        } else {
            assert!(!storage::exists(&alias), "only the canonical entry should remain in the native picker");
            assert_eq!(fs::read(&state.archived[&key]).unwrap(), archived_bytes);
            assert_eq!(f.scan().published, 0);
        }
    }
}

#[test]
fn native_interruption_keeps_the_unanswered_prompt_and_explicit_abort_event() {
    let f = Fixture::new(Tool::Codex);
    let prompt = "Owned fixture interrupted prompt: preserve exactly.";
    let event = json!({"type":"turn_aborted","root_turn_id":"fixture-turn","turn_id":"fixture-turn","reason":"interrupted","started_at":1791956448_u64,"completed_at":1791956448_u64,"duration_ms":52});
    let mut bytes = fs::read(&f.source.path).unwrap();
    for row in [json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"fixture-turn"}}),
        json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":f.source.id,"turn_id":"fixture-turn","item":{"type":"UserMessage","id":"fixture-user","content":[{"type":"text","text":prompt,"text_elements":[]}]}}}),
        json!({"type":"response_item","payload":{"type":"message","id":"fixture-user","role":"user","content":[{"type":"input_text","text":prompt}]}}),
        json!({"type":"event_msg","payload":event})] { bytes.extend_from_slice(format!("{row}\n").as_bytes()); }
    fs::write(&f.source.path, bytes).unwrap();
    let ms = read_messages(&f.source).unwrap();
    assert!(ms.iter().any(|m| m.role == "user" && m.text == prompt), "interrupted work must not lose its latest request");
    assert_eq!(ms.last().unwrap().text, format!("[Session event: turn_aborted]\n{event}"));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let state = f.state();
    assert_eq!(read_messages(&state.entries.values().find(|e| e.imported).unwrap().source).unwrap(), ms);
}

#[test]
fn claude_interruption_written_before_its_prompt_keeps_both_native_records() {
    let f = Fixture::new(Tool::Claude);
    let parent = rows(&fs::read(&f.source.path).unwrap())[1]["uuid"].clone();
    let marker = json!({"type":"user","uuid":"interrupted-marker","parentUuid":"unpersisted-assistant","promptId":"interrupted-prompt","sessionId":f.source.id,"cwd":f.source.cwd,"version":"2.1.299","timestamp":"2026-10-14T05:42:50.373Z","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}});
    let prompt = json!({"type":"user","uuid":"actual-user","parentUuid":parent,"promptId":"interrupted-prompt","sessionId":f.source.id,"cwd":f.source.cwd,"version":"2.1.299","timestamp":"2026-10-14T05:42:50.312Z","message":{"role":"user","content":"Keep this interrupted Claude request"}});
    let mut bytes = fs::read(&f.source.path).unwrap();
    bytes.extend_from_slice(format!("{marker}\n{prompt}\n").as_bytes());
    fs::write(&f.source.path, bytes).unwrap();
    let ms = read_messages(&f.source).unwrap();
    assert!(ms.iter().any(|m| m.text == "Keep this interrupted Claude request"));
    assert!(ms.iter().any(|m| m.role == "user" && m.text == "[Request interrupted by user]"));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let state = f.state();
    let peer = state.entries.values().find(|e| e.imported).unwrap();
    assert_eq!(read_messages(&peer.source).unwrap(), ms);
    assert_eq!(f.scan().published, 0);
    let mut file = fs::OpenOptions::new().append(true).open(&f.source.path).unwrap();
    for row in [json!({"type":"user","uuid":"next-user","parentUuid":"interrupted-marker","promptId":"next-prompt","sessionId":f.source.id,"cwd":f.source.cwd,"version":"2.1.299","message":{"role":"user","content":"Continue the same work"}}),
        json!({"type":"assistant","uuid":"next-answer","parentUuid":"next-user","promptId":"next-prompt","sessionId":f.source.id,"cwd":f.source.cwd,"version":"2.1.299","message":{"role":"assistant","content":[{"type":"text","text":"Continued"}],"stop_reason":"end_turn"}})] {
        writeln!(file, "{row}").unwrap();
    }
    let continued = read_messages(&f.source).unwrap();
    assert!(continued.starts_with(&ms), "native continuation must preserve the interrupted checkpoint");
    assert_eq!(continued.len(), ms.len() + 2);
}

#[test]
fn canonical_append_accepts_assistant_only_continuations_without_inventing_a_prompt() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let original = fs::read(&f.source.path).unwrap();
        let mut messages = read_messages(&f.source).unwrap();
        messages.push(format::Message { role:"assistant".into(), text:"An additional recorded result".into(), images:vec![] });
        canonical::update(&f.source, &messages, "Shared title", "2026-10-14T06:00:00Z").unwrap();
        assert!(fs::read(&f.source.path).unwrap().starts_with(&original));
        assert_eq!(read_messages(&f.source).unwrap(), messages);
    }
}

#[test]
fn native_append_continues_ordinals_after_provider_only_records() {
    let f = Fixture::new(Tool::Codex);
    let mut file = fs::OpenOptions::new().append(true).open(&f.source.path).unwrap();
    writeln!(file, "{}", json!({"ordinal":9000,"type":"response_item","payload":{"type":"reasoning","encrypted_content":"owned-fixture-only"}})).unwrap();
    let mut ms = messages();
    ms.push(Message { role:"assistant".into(), text:"New recorded answer".into(), images:vec![] });
    let before = fs::read(&f.source.path).unwrap();
    canonical::update(&f.source,&ms,"Shared title","2026-10-14T07:00:00Z").unwrap();
    let after = fs::read(&f.source.path).unwrap();
    assert!(after.starts_with(&before));
    assert!(rows(&after[before.len()..]).iter().all(|r| r["ordinal"].as_u64().unwrap() > 9000));
    assert_eq!(read_messages(&f.source).unwrap(),ms);
}
#[test]
fn compressed_codex_history_is_discovered_and_appended_without_changing_its_id() {
    let f = Fixture::new(Tool::Codex);
    let original = fs::read(&f.source.path).unwrap();
    let compressed = f.source.path.with_extension("jsonl.zst");
    fs::write(&compressed, zstd::stream::encode_all(original.as_slice(), 3).unwrap()).unwrap();
    fs::remove_file(&f.source.path).unwrap();
    assert_eq!(read_messages(&f.source).unwrap(), messages());
    let scope = workspace_scope(&f);
    let first = workspace_scan(&f, &scope);
    assert_eq!(first.published, 1);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert!(workspace_state(&f).entries.contains_key(&f.source.path.to_string_lossy().into_owned()));
    let mut continued = messages();
    continued.push(Message { role:"user".into(), text:"Continue the compressed canonical history".into(), images:vec![] });
    continued.push(Message { role:"assistant".into(), text:"The original ID is retained".into(), images:vec![] });
    canonical::update(&f.source, &continued, "Compressed native title", "2026-10-14T07:00:00Z").unwrap();
    assert!(!f.source.path.exists(), "keep a single native representation");
    let decoded = zstd::stream::decode_all(fs::File::open(&compressed).unwrap()).unwrap();
    assert!(decoded.starts_with(&original));
    assert_eq!(read_messages(&f.source).unwrap(), continued);
    assert_eq!(titles::projection(&f.source).unwrap().as_deref(), Some("Compressed native title"));
    assert!(workspace_scan(&f, &scope).errors.is_empty());
    assert_eq!(workspace_scan(&f, &scope).published, 0);
}
#[test]
fn compressed_history_rejects_corruption_symlinks_and_oversized_decoding() {
    let f = Fixture::new(Tool::Codex);
    let raw = fs::read(&f.source.path).unwrap();
    let path = f.source.path.with_extension("jsonl.zst");
    let packed = zstd::stream::encode_all(raw.as_slice(), 3).unwrap();
    fs::write(&path, &packed).unwrap();
    fs::remove_file(&f.source.path).unwrap();
    assert!(storage::bytes(&f.source, 32).unwrap_err().to_string().contains("limit"));
    fs::write(&path, &packed[..packed.len()-4]).unwrap();
    assert!(read_messages(&f.source).is_err());
    #[cfg(unix)] {
        fs::remove_file(&path).unwrap();
        let outside = f._temp.path().join("private-transcript");
        fs::write(&outside, packed).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(read_messages(&f.source).unwrap_err().to_string().contains("Symlink"));
    }
}
#[test]
fn compression_after_initial_selection_keeps_the_selected_conversation() {
    let f = Fixture::new(Tool::Codex);
    let scope = workspace_scope(&f);
    assert_eq!(workspace_scan(&f, &scope).published, 1);
    let packed = f.source.path.with_extension("jsonl.zst");
    fs::write(&packed, zstd::stream::encode_all(fs::read(&f.source.path).unwrap().as_slice(), 3).unwrap()).unwrap();
    fs::remove_file(&f.source.path).unwrap();
    let found = discovery::find(&f.config, &scope, &f.gate, &mut HashMap::new()).unwrap();
    assert_eq!(found.iter().filter(|s| s.id == f.source.id).count(), 1);
    assert!(found.iter().any(|s| s.path == f.source.path));
    // A materialized plain log wins if the native CLI has not yet removed its compressed sibling.
    fs::write(&f.source.path, b"not a valid native transcript\n").unwrap();
    assert!(read_messages(&f.source).is_err());
}
#[test]
fn partial_writes_attachments_versions_and_disable_are_safe() {
    let f = Fixture::new(Tool::Claude);
    let original = fs::read(&f.source.path).unwrap();
    let native = rows(&original);
    let partial = format!("{}\n{}", native[0], native[1]);
    fs::write(&f.source.path, partial).unwrap();
    assert_eq!(f.scan().published, 0);
    assert_eq!(f.scan().rows.values().next().unwrap().phase, Phase::Waiting);
    let mut unsupported = rows(&original);
    unsupported[0]["message"]["content"] =
        json!([{"type":"document","source":{"type":"base64","data":"fixture"}}]);
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
    let mut fixture_messages = messages();
    if std::env::var_os("KILN_SYNC_FIXTURE_IMAGES").is_some() {
        fixture_messages.insert(1, Message { role: "user".into(), text: String::new(), images: vec!["data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC".into()] });
    }
    if std::env::var_os("KILN_SYNC_FIXTURE_TOOLS").is_some() {
        fixture_messages.push(Message { role: "assistant".into(), images: vec![], text: "[Tool result: fixture_call]\nScript completed\nOutput:\n{\"output\":\"첫째 줄\\n둘째 줄 <content>\",\"exit_code\":0}{\"content\":[{\"type\":\"text\",\"text\":\"결과: 성공\"}]}".into() });
    }
    let fixture_title = std::env::var("KILN_SYNC_FIXTURE_TITLE")
        .unwrap_or_else(|_| messages()[0].text.lines().next().unwrap().into());
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
        let append_only = std::env::var_os("KILN_SYNC_FIXTURE_APPEND_ONLY").is_some();
        let source = if append_only {
            let saved: Source = serde_json::from_slice(&fs::read(dir.join(format!("{}.json", tool.key()))).unwrap()).unwrap();
            assert_eq!(saved.root, pair.endpoint(tool).root);
            assert_eq!(saved.cwd, dir.to_string_lossy());
            assert_eq!(saved.id, id);
            saved
        } else { source };
        if !append_only {
        publish(
            &source.path,
            &format::render_titled(
                tool,
                &id,
                &source.cwd,
                &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                &fixture_messages,
                &fixture_title,
            )
            .unwrap(),
        )
        .unwrap();
        }
        if tool == Tool::Codex && std::env::var_os("KILN_SYNC_FIXTURE_COMPRESSED").is_some() && source.path.exists() {
            let packed = source.path.with_extension("jsonl.zst");
            let bytes = fs::read(&source.path).unwrap();
            fs::write(&packed, zstd::stream::encode_all(bytes.as_slice(), 3).unwrap()).unwrap();
            fs::remove_file(&source.path).unwrap();
        }
        if std::env::var_os("KILN_SYNC_FIXTURE_CONTINUATIONS").is_some() {
            let mut continued = fixture_messages.clone();
            for index in 1..=3 {
                continued.push(Message { role:"user".into(), text:format!("Canonical continuation {index}"), images:vec![] });
                continued.push(Message { role:"assistant".into(), text:format!("Recorded canonical continuation {index}"), images:vec![] });
                canonical::update(&source, &continued, &fixture_title, &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).unwrap();
            }
        }
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
            images: Vec::new(),
            role: "user".into(),
            text: text.into(),
        });
        ms.push(Message {
            images: Vec::new(),
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
    assert_eq!(changed.published, 0);
    assert!(changed.errors.iter().all(|e| e.contains("diverged")));
    assert!(!changed.errors.is_empty());
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
        let key = f.source.path.to_string_lossy();
        let id = stable_id(&format!(
            "{}:canonical:{key}:{}",
            state_path.display(),
            f.pair.claude.root.display()
        ));
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
            images: Vec::new(),
            role: "user".into(),
            text: "Already continued after crash".into(),
        });
        ms.push(Message {
            images: Vec::new(),
            role: "assistant".into(),
            text: "Native reply".into(),
        });
        let bytes = if collision {
            b"unrelated conversation\n".to_vec()
        } else {
            // A native continuation appends after existing picker metadata; it
            // does not re-render or move the original immutable prefix.
            let mut bytes =
                format::render(target.tool, &id, &target.cwd, &timestamp, &messages()).unwrap();
            let extended = format::render(target.tool, &id, &target.cwd, &timestamp, &ms).unwrap();
            for row in rows(&extended)
                .iter()
                .skip(messages().len())
                .filter(|r| r["type"] != "custom-title")
            {
                bytes.extend_from_slice(format!("{row}\n").as_bytes());
            }
            bytes
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
fn native_import_of_a_kiln_copy_does_not_echo_but_new_turns_sync() {
    let f = Fixture::new(Tool::Codex);
    let scope = workspace_scope(&f);
    assert_eq!(workspace_scan(&f, &scope).published, 1);
    let state = workspace_state(&f);
    let copied = &state.entries.values().find(|e| e.imported).unwrap().source;
    assert!(
        has_current_native_projection(copied, &titles::projection(copied).unwrap().unwrap())
            .unwrap()
    );
    let external = destination(
        &f.pair,
        copied,
        &stable_id("native-import"),
        chrono::Utc::now(),
    )
    .unwrap();
    let bytes = format::render(
        Tool::Codex,
        &external.id,
        &external.cwd,
        "2026-10-13T00:00:00Z",
        &messages(),
    )
    .unwrap();
    publish(&external.path, &bytes).unwrap();
    // Reproduce an older Kiln already tracking this as a separate conversation.
    assert_eq!(workspace_scan(&f, &scope).published, 1);
    fs::write(
        f.pair
            .codex
            .root
            .join("external_agent_session_imports.json"),
        serde_json::to_vec(
            &json!({"records":[{"source_path":copied.path,"imported_thread_id":external.id}]}),
        )
        .unwrap(),
    )
    .unwrap();
    let again = workspace_scan(&f, &scope);
    assert_eq!(again.published, 0);
    assert_eq!(again.rows.len(), 1);
    let archived = workspace_state(&f).archived[&external.path.to_string_lossy().into_owned()].clone();
    assert_eq!(fs::read(&archived).unwrap(), bytes);
    // A native unarchive restores the old endpoint; new turns still join its group.
    fs::rename(&archived, &external.path).unwrap();
    let mut continued = messages();
    continued.push(Message {
        role: "user".into(),
        text: "New native turn".into(),
        images: vec![],
    });
    continued.push(Message {
        role: "assistant".into(),
        text: "Native answer".into(),
        images: vec![],
    });
    fs::write(
        &external.path,
        format::render(
            Tool::Codex,
            &external.id,
            &external.cwd,
            "2026-10-13T00:00:00Z",
            &continued,
        )
        .unwrap(),
    )
    .unwrap();
    let changed = workspace_scan(&f, &scope);
    assert_eq!(changed.published, 1);
    assert!(changed.errors.is_empty());
    assert_eq!(changed.rows.len(), 2);
}

#[test]
fn codex_imported_local_commands_do_not_consume_initial_conversation_slots() {
    let f = Fixture::new(Tool::Codex);
    let scope = workspace_scope(&f);
    let mut ms = vec![Message {
        role: "user".into(),
        text: "<command-name>/mcp</command-name>".into(),
        images: vec![],
    }];
    fs::write(
        &f.source.path,
        format::render(
            Tool::Codex,
            &f.source.id,
            &f.source.cwd,
            "2026-10-13T00:00:00Z",
            &ms,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(workspace_scan(&f, &scope).rows.is_empty());
    // A literal command-looking prompt with an actual answer is conversation.
    ms.push(Message {
        role: "assistant".into(),
        text: "An actual reply".into(),
        images: vec![],
    });
    fs::write(
        &f.source.path,
        format::render(
            Tool::Codex,
            &f.source.id,
            &f.source.cwd,
            "2026-10-13T00:00:00Z",
            &ms,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(workspace_scan(&f, &scope).published, 1);
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
            images: Vec::new(),
            role: "user".into(),
            text: "Continue in the already-open default CLI".into(),
        },
        Message {
            images: Vec::new(),
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
    if let Some(path) = manifest["imports_dir"].as_str() {
        let target = config.path().join("conversation-sync/imports");
        private_dir(&target).unwrap();
        for entry in fs::read_dir(path).unwrap().flatten() {
            if entry.path().extension().is_some_and(|e| e == "json") {
                fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
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
    let mut report = Vec::new();
    for source in sources {
        if discovery::imported(config.path(), &source)
            .unwrap()
            .is_some()
        {
            continue;
        }
        let result = match read_messages(&source) {
            Ok(messages) => {
                let images: usize = messages.iter().map(|m| m.images.len()).sum();
                let bytes = serde_json::to_vec(&messages).unwrap().len();
                // A newly opened live conversation can have no completed checkpoint yet.
                for tool in Tool::ALL.into_iter().filter(|_| !messages.is_empty()) {
                    let rendered = format::render_titled(tool, &source.id, &source.cwd, "2026-10-14T00:00:00Z", &messages, "Read-only integrity check").unwrap();
                    let parsed_rows: Vec<serde_json::Value> = rendered.split(|b| *b == b'\n').filter(|b| !b.is_empty()).map(serde_json::from_slice).collect::<std::result::Result<_, _>>().unwrap();
                    assert!(format::parse(tool, &parsed_rows).unwrap() == messages, "Native round-trip changed message or image bytes");
                }
                eprintln!(
                    "{} {}: {} messages, {} images, {} MiB",
                    scope
                        .workspace(&source.cwd)
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_string_lossy(),
                    source.tool.key(),
                    messages.len(),
                    images,
                    bytes / 1024 / 1024
                );
                json!({"messages":messages.len(),"images":images,"portable_bytes":bytes,"both_native_formats_verified":!messages.is_empty(),"waiting_for_first_completed_turn":messages.is_empty()})
            }
            Err(error) => {
                eprintln!(
                    "{} {}: {}",
                    scope
                        .workspace(&source.cwd)
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_string_lossy(),
                    source.tool.key(),
                    error
                );
                json!({"error":error.to_string()})
            }
        };
        report.push(json!({"source":source,"result":result}));
    }
    if let Some(path) = std::env::var_os("KILN_SYNC_INSPECT_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}

/// Explicit operator-only verification; never invoked by CI or ordinary tests.
#[test]
#[ignore]
fn synchronize_explicitly_authorized_workspaces() {
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            std::env::var_os("KILN_SYNC_AUTHORIZED_MANIFEST")
                .expect("explicit authorized workspace manifest"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["authorization"], "user-requested-workspace-sync");
    let config = PathBuf::from(manifest["config"].as_str().unwrap());
    let persisted: super::super::state::Persist =
        serde_json::from_slice(&fs::read(config.join("state.json")).unwrap()).unwrap();
    let workspaces: Vec<PathBuf> = serde_json::from_value(manifest["workspaces"].clone()).unwrap();
    for root in &workspaces {
        assert!(
            persisted
                .workspaces
                .iter()
                .any(|w| w.conversation_sync
                    && w.root.canonicalize().ok() == root.canonicalize().ok()),
            "workspace must already be opted in"
        );
    }
    let pair = persisted.settings.conversation_sync_pair.unwrap();
    let scope = discovery::Scope::new(&pair, workspaces).unwrap();
    let gate = Arc::new(AtomicBool::new(true));
    let mut cache = Cache::new();
    let mut registry = codex_index::Registry::default();
    let progress = std::cell::Cell::new(0);
    let result = scan_with_metadata(
        &config,
        &pair,
        &gate,
        &mut cache,
        Some(&scope),
        |source, request| registry.apply(source, &gate, request),
        |status| {
            if status.checked > progress.get() {
                progress.set(status.checked);
                eprintln!("Checked {}, published {}", status.checked, status.published);
            }
        },
    )
    .unwrap();
    let state: State = serde_json::from_slice(
        &fs::read(config.join("conversation-sync/workspace-state.json")).unwrap(),
    )
    .unwrap();
    let mut report = Vec::new();
    for (key, row) in &result.rows {
        let entry = &state.entries[key];
        let mut verified = 0;
        for copy in &row.copies {
            let target = state
                .entries
                .values()
                .find(|e| e.source.id == copy.id && e.source.root == copy.root)
                .unwrap();
            let messages = read_messages(&target.source).unwrap();
            assert_eq!(
                digest(&serde_json::to_vec(&messages).unwrap()),
                entry.seen,
                "copy must retain all original messages and images"
            );
            registry.ensure(&target.source, &gate).unwrap();
            verified += 1;
        }
        report.push(json!({"source":row.source,"workspace":row.workspace,"phase":format!("{:?}",row.phase),"error":row.detail,"verified_copies":verified,"copies":row.copies}));
        eprintln!(
            "Verified conversation {}, copies {}",
            report.len(),
            verified
        );
    }
    fs::write(
        manifest["report"].as_str().unwrap(),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    assert!(
        result.errors.is_empty(),
        "{} conversations failed",
        result.errors.len()
    );
    assert!(result.rows.values().all(|r| r.phase == Phase::Synced));
}

#[test]
#[ignore]
fn inspect_authorized_copy_integrity_read_only() {
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(std::env::var_os("KILN_SYNC_AUTHORIZED_MANIFEST").unwrap()).unwrap(),
    )
    .unwrap();
    let config = PathBuf::from(manifest["config"].as_str().unwrap());
    let state: State = serde_json::from_slice(
        &fs::read(config.join("conversation-sync/workspace-state.json")).unwrap(),
    )
    .unwrap();
    for entry in state
        .entries
        .values()
        .filter(|e| !e.imported && !e.copies.is_empty())
    {
        let original = read_messages(&entry.source).unwrap();
        if digest(&serde_json::to_vec(&original).unwrap()) != entry.seen {
            continue;
        }
        let copy = &entry.copies[0];
        let target = state
            .entries
            .values()
            .find(|e| e.source.id == copy.id && e.source.root == copy.root)
            .unwrap();
        let copied = read_messages(&target.source).unwrap();
        if original == copied {
            continue;
        }
        eprintln!(
            "Integrity mismatch {}: {} original, {} copied",
            &entry.source.id[..8],
            original.len(),
            copied.len()
        );
        for (index, (a, b)) in original.iter().zip(&copied).enumerate() {
            if a == b {
                continue;
            }
            eprintln!(
                "First difference {}: roles {} / {}; text lengths {} / {}; images {} / {}; local command {}",
                index,
                a.role,
                b.role,
                a.text.len(),
                b.text.len(),
                a.images.len(),
                b.images.len(),
                a.text.starts_with("<local-command-") || a.text.starts_with("<command-name>")
            );
            break;
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
    assert!(h.query_by_label("워크스페이스 대화 찾는 중").is_none());
    assert!(h.query_by_label("완료된 대화 모두 동기화됨").is_some());
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
    }))
    .unwrap();
    assert!(old.settings.conversation_sync_enabled);
    assert!(old.workspaces.iter().all(|w| !w.conversation_sync));
    let mut selected = old;
    selected.workspaces[0].conversation_sync = true;
    let restored: super::super::state::Persist =
        serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap();
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
    manager
        .tx
        .send(Event::Progress(old_gate.clone(), f.scan()))
        .unwrap();
    manager
        .tx
        .send(Event::Scanned(old_gate, Err("stale cancelled scan".into())))
        .unwrap();
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
    manager.workspaces = vec![
        "/example/a".into(),
        "/example/b".into(),
        "/example/a/child".into(),
    ];
    manager.status.rows.clear();
    for (path, phase) in [
        ("/example/a", Phase::Synced),
        ("/example/b", Phase::Failed),
        ("/example/a/child", Phase::Copying),
    ] {
        let mut row = original.clone();
        row.source.cwd = path.into();
        row.phase = phase;
        manager.status.rows.insert(path.into(), row);
    }
    assert_eq!(
        manager.workspace_status(Path::new("/example/a")).rows.len(),
        1
    );
    assert_eq!(
        manager.badge(Path::new("/example/a")).0,
        kiln_common::icons::Icon::Check
    );
    assert_eq!(
        manager.badge(Path::new("/example/b")).0,
        kiln_common::icons::Icon::Warning
    );
    assert_eq!(
        manager.badge(Path::new("/example/a/child")).0,
        kiln_common::icons::Icon::Refresh
    );
}

#[test]
#[cfg(unix)]
fn workspace_status_uses_canonical_assignment_for_aliased_working_directories() {
    let f = Fixture::new(Tool::Claude);
    let root = PathBuf::from(&f.source.cwd);
    let alias = root.join("workspace-alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let scope = discovery::Scope {
        workspaces: vec![root.clone()],
        roots: vec![],
    };
    let mut source = f.source.clone();
    source.cwd = alias.to_string_lossy().into_owned();
    let row = Conversation::from_entry(
        &Entry {
            source,
            seen: String::new(),
            imported: false,
            title: "Aliased working directory".into(),
            copied_to: None,
            copies: vec![],
        },
        Some(&scope),
    );
    assert_eq!(row.workspace.as_ref(), Some(&root));
    let mut manager = Manager::default();
    manager.workspaces = vec![root.clone()];
    manager.status.rows.insert("fixture".into(), row);
    assert_eq!(manager.workspace_status(&root).rows.len(), 1);
}

#[test]
fn visible_answer_with_hidden_citations_is_not_duplicated_by_text_difference() {
    let f = Fixture::new(Tool::Codex);
    let mut data = rows(&fs::read(&f.source.path).unwrap());
    let answer =
        "Exactly one final answer.\n<oai-mem-citation>\nlocal-reference\n</oai-mem-citation>";
    for row in &mut data {
        if row["type"] == "response_item" && row["payload"]["role"] == "assistant" {
            row["payload"]["content"][0]["text"] = json!(answer);
        }
        if row["type"] == "event_msg" && row["payload"]["item"]["type"] == "AgentMessage" {
            row["payload"]["item"]["content"][0]["text"] = json!("Exactly one final answer.");
        }
    }
    let parsed = format::parse(Tool::Codex, &data).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[1].text, answer);
}

#[test]
fn readable_tool_records_retain_raw_bytes_and_respect_edits_in_both_clis() {
    let raw = "[Tool result: call_fixture]\nScript completed\nOutput:\n{\"exit_code\":0,\"output\":\"첫째 줄\\n둘째 줄 <tag> ``` 코드\"}{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"결과\\\":\\\"성공\\\"}\"}]}";
    let mut input = messages();
    input.insert(
        1,
        Message {
            role: "assistant".into(),
            text: raw.into(),
            images: vec![],
        },
    );
    for tool in Tool::ALL {
        let bytes = format::render_titled(
            tool,
            &stable_id("portable-fixture"),
            "/tmp/fixture",
            "2026-10-14T00:00:00Z",
            &input,
            "Stable conversation title",
        )
        .unwrap();
        let mut data = rows(&bytes);
        assert_eq!(format::parse(tool, &data).unwrap(), input);
        let row = data
            .iter_mut()
            .find(|r| r["kiln_sync"]["original_text"] == raw)
            .unwrap();
        let content = if tool == Tool::Claude {
            &mut row["message"]["content"][0]["text"]
        } else {
            &mut row["payload"]["content"][0]["text"]
        };
        let shown = content.as_str().unwrap();
        assert!(shown.contains("첫째 줄\n둘째 줄 <tag> ``` 코드"));
        assert!(shown.contains("결과: 성공"));
        assert!(!shown.contains("\\n"));
        assert!(shown.contains("````text"));
        *content = json!("Explicitly edited historical record");
        let changed = format::parse(tool, &data).unwrap();
        assert_eq!(changed[1].text, "Explicitly edited historical record");
    }
}

#[test]
fn native_titles_survive_new_turns_and_metadata_only_renames_trigger_new_copies() {
    let f = Fixture::new(Tool::Codex);
    let scope = workspace_scope(&f);
    let path = f.source.root.join("session_index.jsonl");
    let set_name = |name: &str| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(
                format!(
                    "{}\n",
                    json!({"id":f.source.id,"thread_name":name,"updated_at":"2026-10-14T00:00:00Z"})
                )
                .as_bytes(),
            )
            .unwrap();
    };
    set_name("Original named conversation");
    let source_bytes = fs::read(&f.source.path).unwrap();
    let mut cache = Cache::new();
    let run = |cache: &mut Cache| {
        scan_scoped(&f.config, &f.pair, &f.gate, cache, Some(&scope), |_| {}).unwrap()
    };
    let first = run(&mut cache);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.published, 1);
    assert_eq!(
        first.rows.values().next().unwrap().title,
        "Original named conversation"
    );
    let copy = workspace_state(&f)
        .entries
        .into_values()
        .find(|e| e.imported)
        .unwrap();
    assert_eq!(
        titles::projection(&copy.source).unwrap().unwrap(),
        "Original named conversation"
    );
    assert_eq!(run(&mut cache).published, 0);
    set_name("Renamed conversation");
    let renamed = run(&mut cache);
    assert!(renamed.errors.is_empty(), "{:?}", renamed.errors);
    assert_eq!(renamed.published, 1);
    assert_eq!(
        renamed.rows.values().next().unwrap().title,
        "Renamed conversation"
    );
    assert_eq!(run(&mut cache).published, 0);
    assert_eq!(fs::read(&f.source.path).unwrap(), source_bytes);
}

#[test]
fn claude_user_title_overrides_sidecar_and_ai_title_without_changing_transcript() {
    let f = Fixture::new(Tool::Claude);
    let original = fs::read(&f.source.path).unwrap();
    titles::pin_claude(&f.source, "Pinned name").unwrap();
    let mut cache = titles::Cache::default();
    assert_eq!(
        cache.name(&f.source).unwrap().as_deref(),
        Some("Pinned name")
    );
    let mut data = rows(&original);
    data.push(json!({"type":"summary","aiTitle":"Generated name","sessionId":f.source.id}));
    assert_eq!(
        titles::from_rows(&f.source, &data, "Pinned name"),
        messages()[0].text.lines().next().unwrap()
    );
    data.push(json!({"type":"custom-title","customTitle":"User renamed","sessionId":f.source.id}));
    assert_eq!(
        titles::from_rows(&f.source, &data, "Pinned name"),
        "User renamed"
    );
    assert_eq!(fs::read(&f.source.path).unwrap(), original);
}

#[test]
fn native_import_without_private_metadata_does_not_echo_readable_tool_history() {
    let f = Fixture::new(Tool::Codex);
    let mut ms = messages();
    ms.push(Message {
        role: "assistant".into(),
        images: vec![],
        text: "[Tool result: example]\n{\"output\":\"line one\\nline two\"}".into(),
    });
    fs::write(
        &f.source.path,
        format::render(
            Tool::Codex,
            &f.source.id,
            &f.source.cwd,
            "2026-10-13T00:00:00Z",
            &ms,
        )
        .unwrap(),
    )
    .unwrap();
    let scope = workspace_scope(&f);
    assert_eq!(workspace_scan(&f, &scope).published, 1);
    let state = workspace_state(&f);
    let copied = &state.entries.values().find(|e| e.imported).unwrap().source;
    let external = destination(
        &f.pair,
        copied,
        &stable_id("readable-native-import"),
        chrono::Utc::now(),
    )
    .unwrap();
    let mut imported = rows(
        &format::render(
            Tool::Codex,
            &external.id,
            &external.cwd,
            "2026-10-13T00:00:00Z",
            &ms,
        )
        .unwrap(),
    );
    for row in &mut imported {
        row.as_object_mut().unwrap().remove("kiln_sync");
    }
    publish(
        &external.path,
        imported
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>()
            .as_bytes(),
    )
    .unwrap();
    fs::write(
        f.pair
            .codex
            .root
            .join("external_agent_session_imports.json"),
        serde_json::to_vec(
            &json!({"records":[{"source_path":copied.path,"imported_thread_id":external.id}]}),
        )
        .unwrap(),
    )
    .unwrap();
    let again = workspace_scan(&f, &scope);
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    assert_eq!(again.published, 0);
    assert_eq!(again.rows.len(), 1);
}

#[test]
fn database_only_native_names_override_text_index_and_renames_do_not_need_new_turns() {
    let f = Fixture::new(Tool::Codex);
    let mut cache = Cache::new();
    let mut run = |name: &str| {
        scan_with_metadata(
            &f.config,
            &f.pair,
            &f.gate,
            &mut cache,
            None,
            |source, request| {
                let title = request.title();
                if source.path == f.source.path && title.is_none() {
                    Ok(Some(name.into()))
                } else {
                    Ok(None)
                }
            },
            |_| {},
        )
        .unwrap()
    };
    let first = run("Native database title");
    assert!(first.errors.is_empty());
    assert_eq!(first.published, 1);
    assert_eq!(
        first.rows.values().next().unwrap().title,
        "Native database title"
    );
    assert_eq!(run("Native database title").published, 0);
    let renamed = run("Renamed in native database");
    assert!(renamed.errors.is_empty());
    assert_eq!(renamed.published, 1);
    assert_eq!(
        renamed.rows.values().next().unwrap().title,
        "Renamed in native database"
    );
    assert_eq!(run("Renamed in native database").published, 0);
}

#[test]
fn resumed_import_keeps_native_title_even_before_it_has_a_status_row() {
    let f = Fixture::new(Tool::Claude);
    assert_eq!(f.scan().published, 1);
    let copy = f.state().entries.values().find(|e| e.imported).unwrap().source.clone();
    let mut continued = messages();
    continued.push(Message { role: "user".into(), text: "Continue here".into(), images: vec![] });
    continued.push(Message { role: "assistant".into(), text: "A new answer".into(), images: vec![] });
    fs::write(&copy.path, format::render(copy.tool, &copy.id, &copy.cwd, "2026-10-14T00:00:00Z", &continued).unwrap()).unwrap();
    let status = scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), None,
        |source, request| Ok((source.path == copy.path && request.title().is_none()).then(|| "Shared native name".into())), |_| {}).unwrap();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(status.rows[&copy.path.to_string_lossy().into_owned()].title, "Shared native name");
    let state = f.state();
    let target = &state.entries[&copy.path.to_string_lossy().into_owned()].copied_to.as_ref().unwrap();
    let target = state.entries.values().find(|e| e.source.id == target.id).unwrap();
    assert_eq!(titles::projection(&target.source).unwrap().as_deref(), Some("Shared native name"));
}

#[test]
fn unnamed_image_only_history_gets_a_nonempty_shared_resume_title() {
    let f = Fixture::new(Tool::Codex);
    let ms = vec![Message { role: "user".into(), text: String::new(), images: vec!["data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC".into()] }, Message { role: "assistant".into(), text: "Image received.".into(), images: vec![] }];
    let mut data = rows(
        &format::render(
            Tool::Codex,
            &f.source.id,
            &f.source.cwd,
            "2026-10-14T00:00:00Z",
            &ms,
        )
        .unwrap(),
    );
    for row in &mut data {
        row.as_object_mut().unwrap().remove("kiln_title");
    }
    fs::write(
        &f.source.path,
        data.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty());
    assert_eq!(status.published, 1);
    assert_eq!(
        status.rows.values().next().unwrap().title,
        "Linked conversation"
    );
    let state = f.state();
    let copy = state.entries.values().find(|e| e.imported).unwrap();
    assert_eq!(
        titles::projection(&copy.source).unwrap().as_deref(),
        Some("Linked conversation")
    );
    assert_eq!(read_messages(&copy.source).unwrap(), ms);
    assert_eq!(f.scan().published, 0);
}

#[test]
fn unnamed_history_normalizes_title_whitespace_without_changing_messages() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let mut expected = messages();
        expected[0].text = "  공백이 있는 제목? \t\n본문은 그대로 보존합니다.  ".into();
        let mut data = rows(
            &format::render(
                tool,
                &f.source.id,
                &f.source.cwd,
                "2026-10-14T00:00:00Z",
                &expected,
            )
            .unwrap(),
        );
        for row in &mut data {
            row.as_object_mut().unwrap().remove("kiln_title");
            row.as_object_mut().unwrap().remove("customTitle");
        }
        let bytes = data.iter().map(|r| format!("{r}\n")).collect::<String>();
        fs::write(&f.source.path, &bytes).unwrap();
        let mut title = String::new();
        assert_eq!(
            read_messages_titled(&f.source, &mut title).unwrap(),
            expected
        );
        assert_eq!(title, "공백이 있는 제목?");
        let status = scan_with_metadata(
            &f.config,
            &f.pair,
            &f.gate,
            &mut Cache::new(),
            None,
            |_, request| {
                if let Some(title) = request.title() {
                    assert_eq!(title, title.trim());
                }
                Ok(None)
            },
            |_| {},
        )
        .unwrap();
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert_eq!(status.published, 1);
        assert_eq!(fs::read_to_string(&f.source.path).unwrap(), bytes);
        assert_eq!(
            read_messages(
                &f.state()
                    .entries
                    .values()
                    .find(|e| e.imported)
                    .unwrap()
                    .source
            )
            .unwrap(),
            expected
        );
        let mut state = f.state();
        for entry in state.entries.values_mut().filter(|e| e.imported) {
            entry.title.push(' ');
        }
        replace(
            &f.config.join("conversation-sync").join(format!(
                "{}.json",
                digest(&serde_json::to_vec(&f.pair).unwrap())
            )),
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert_eq!(f.scan().published, 0);
    }
}

#[test]
fn streamed_comparison_matches_plain_and_compressed_transcripts_exactly() {
    let f = Fixture::new(Tool::Codex);
    let bytes = fs::read(&f.source.path).unwrap();
    assert!(storage::equals(&f.source, &bytes).unwrap());
    assert!(!storage::equals(&f.source, &bytes[..bytes.len() - 1]).unwrap());
    assert!(!storage::equals(&f.source, &[bytes.as_slice(), b"\n"].concat()).unwrap());
    let mut changed = bytes.clone();
    changed[0] ^= 1;
    assert!(!storage::equals(&f.source, &changed).unwrap());
    let packed = f.source.path.with_extension("jsonl.zst");
    fs::write(&packed, zstd::encode_all(bytes.as_slice(), 3).unwrap()).unwrap();
    fs::remove_file(&f.source.path).unwrap();
    assert!(storage::equals(&f.source, &bytes).unwrap());
    assert!(!storage::equals(&f.source, &changed).unwrap());
}

#[cfg(unix)]
struct WorkerFixture {
    gui: std::os::unix::net::UnixStream,
    replies: mpsc::Receiver<String>,
    done: std::thread::JoinHandle<Result<()>>,
    exiting: bool,
}
#[cfg(unix)]
impl WorkerFixture {
    fn start(f: &Fixture, recycle_peak: u64) -> Self {
        struct Lines(mpsc::Sender<String>, Vec<u8>);
        impl Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.1.extend_from_slice(bytes);
                while let Some(end) = self.1.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = self.1.drain(..=end).collect();
                    let _ = self.0.send(String::from_utf8_lossy(&line).trim_end().to_owned());
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (gui, worker_end) = std::os::unix::net::UnixStream::pair().unwrap();
        let (tx, replies) = mpsc::channel();
        let (config, roots) = (f.config.clone(), workspace_scope(f).roots);
        let done = std::thread::spawn(move || {
            worker::serve_with(std::io::BufReader::new(worker_end), Lines(tx, Vec::new()), &config, recycle_peak, move |_, workspaces| {
                Ok(discovery::Scope { workspaces, roots: roots.clone() })
            })
        });
        Self { gui, replies, done, exiting: false }
    }
    fn send(&mut self, request: &worker::Request) {
        let mut line = serde_json::to_vec(request).unwrap();
        line.push(b'\n');
        self.gui.write_all(&line).unwrap();
    }
    fn scan(&mut self, f: &Fixture, id: u64) -> Status {
        self.send(&worker::Request::Scan { id, pair: f.pair.clone(), workspaces: vec![PathBuf::from(&f.source.cwd)], quiet_ms: 0, expedite: false, reset: false });
        loop {
            let line = self.replies.recv_timeout(Duration::from_secs(30)).expect("worker reply");
            if let worker::Reply::Scanned { id: done, result, exiting } = serde_json::from_str(&line).unwrap() {
                assert_eq!(done, id);
                self.exiting = exiting;
                return result.unwrap();
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn sync_worker_keeps_file_stamped_results_across_scans_and_restarts() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let mut worker = WorkerFixture::start(&f, u64::MAX);
    assert_eq!(worker.scan(&f, 1).published, 1);
    assert_eq!(worker.scan(&f, 2).published, 0, "unchanged histories are not copied again");
    drop(worker.gui);
    worker.done.join().unwrap().unwrap();
    assert!(!worker::cache_path(&f.config).exists());

    let mut extended = messages();
    extended.push(Message { role: "user".into(), text: "A new turn".into(), images: vec![] });
    extended.push(Message { role: "assistant".into(), text: "New answer".into(), images: vec![] });
    let mut recycling = WorkerFixture::start(&f, 0);
    fs::write(&f.source.path, format::render(f.source.tool, &f.source.id, &f.source.cwd, "2026-10-14T00:00:00Z", &extended).unwrap()).unwrap();
    assert_eq!(recycling.scan(&f, 3).published, 1);
    assert!(recycling.exiting, "the reply announces the exit");
    assert!(worker::cache_path(&f.config).exists(), "results are saved before the reply");
    recycling.done.join().unwrap().unwrap();
    let mut next = WorkerFixture::start(&f, u64::MAX);
    assert_eq!(next.scan(&f, 4).published, 0, "the next worker reuses the saved results");
    assert!(!worker::cache_path(&f.config).exists());
    drop(next.gui);
    next.done.join().unwrap().unwrap();
}

#[cfg(unix)]
#[test]
fn sync_worker_reports_a_cancelled_scan_without_publishing() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let mut worker = WorkerFixture::start(&f, u64::MAX);
    worker.send(&worker::Request::Cancel { id: 1 });
    assert_eq!(worker.scan(&f, 1).published, 0, "a scan cancelled before it starts publishes nothing");
    assert_eq!(worker.scan(&f, 2).published, 1);
    drop(worker.gui);
    worker.done.join().unwrap().unwrap();
}

#[test]
fn saved_cache_keeps_retry_delays_for_a_restarted_worker() {
    let f = Fixture::new(Tool::Claude);
    let original = fs::read(&f.source.path).unwrap();
    let mut data = rows(&original);
    data[0]["message"]["content"] = json!([{"type":"document"}]);
    fs::write(&f.source.path, data.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    let mut cache = Cache::new();
    scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    let attempted = cache.attempts[&f.source.path];
    let mut restored: Cache = serde_json::from_slice(&serde_json::to_vec(&cache).unwrap()).unwrap();
    assert_eq!(restored.failures, cache.failures);
    let second = scan(&f.config, &f.pair, &f.gate, &mut restored, |_| {}).unwrap();
    assert_eq!(restored.attempts[&f.source.path], attempted, "the failing history is not read again");
    assert_eq!(second.rows.values().next().unwrap().phase, Phase::Failed);
}

#[test]
fn a_row_left_waiting_is_marked_synced_when_its_unchanged_file_is_cached_as_synced() {
    let f = Fixture::new(Tool::Claude);
    let mut cache = Cache::new();
    assert_eq!(scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap().published, 1);
    for row in cache.rows.values_mut() {
        row.phase = Phase::Waiting;
        row.detail = "Transcript is being written; retrying".into();
    }
    let status = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert!(status.rows.values().all(|r| r.phase == Phase::Synced && r.detail.is_empty()));
    assert_eq!(status.published, 0);
}

#[test]
fn a_synchronized_conversation_whose_original_was_deleted_stays_synced() {
    let f = Fixture::new(Tool::Claude);
    let mut cache = Cache::new();
    assert_eq!(scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap().published, 1);
    fs::remove_file(&f.source.path).unwrap();
    let status = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    let row = &status.rows[&f.source.path.to_string_lossy().into_owned()];
    assert_eq!(row.phase, Phase::Synced, "its copies remain available");
    assert!(row.target.is_some());
}
