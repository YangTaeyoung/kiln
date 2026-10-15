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
    let key = f.source.path.to_string_lossy().into_owned();
    let at = cache.hub[&key].1;
    assert_eq!(first.rows.values().next().unwrap().phase, Phase::Failed);
    let second = scan(&f.config, &f.pair, &f.gate, &mut cache, |s| {
        assert_eq!(s.rows.values().next().unwrap().phase, Phase::Failed);
    })
    .unwrap();
    assert_eq!(first.errors, second.errors);
    assert_eq!(cache.hub[&key].1, at);
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
        hub_view(&self.config)
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
    assert!(cache.hub.is_empty(), "a transcript being written is not a failed attempt");
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
    f.gate.store(false, Ordering::Release);
    assert_eq!(f.scan().published, 0);
    f.gate.store(true, Ordering::Release);
    let accepted = f.scan();
    assert!(accepted.errors.is_empty(), "an unknown CLI version with known content synchronizes: {:?}", accepted.errors);
    assert_eq!(accepted.published, 1);
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
            .contains_rect(h.get_by_label("/resume으로 이어가기 · Claude Code (기본 CLI), Codex (기본 CLI)").rect())
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
    row.detail = "Connected account directory is missing or symlinked".into();
    h.run_steps(2);
    assert!(h.query_by_label("동기화 실패").is_some());
    assert!(h.query_by_label("확인이 필요한 대화가 있습니다").is_some());
    h.render()
        .unwrap()
        .save("/tmp/kiln-sync-progress-failed.png")
        .unwrap();
    let row = h.state_mut().0.status.rows.values_mut().next().unwrap();
    row.detail = DIVERGED.into();
    h.run_steps(2);
    assert!(h.query_by_label("따로 이어짐").is_some());
    assert!(h.query_by_label("동기화 실패").is_none());
    assert!(h.query_by_label("확인이 필요한 대화가 있습니다").is_none());
    assert!(h.query_by_label("보류된 대화 1개 · 원본은 그대로 보관됨").is_some());
    h.render()
        .unwrap()
        .save("/tmp/kiln-sync-progress-diverged.png")
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
        let cid = hub::conversation_id(&hub::NativeRef::of(&f.source));
        let id = engine::projection_id(&cid, &f.pair.claude.root, 0);
        let date = chrono::DateTime::parse_from_rfc3339("2026-10-12T23:59:59.000Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let target = destination(&f.pair, &f.source, &id, date).unwrap();
        let timestamp = date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
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
            assert!(status.errors.is_empty(), "{:?}", status.errors);
            f.scan();
            assert_eq!(read_messages(&f.source).unwrap(), ms, "the continued projection reaches the original");
            assert_eq!(f.scan().published, 0);
            assert!(fs::read(&target.path).unwrap().starts_with(&bytes), "existing native bytes survive");
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
    hub_view(&f.config)
}
/// Hub projections as source entries: originals list their copies, copies are imported.
fn hub_view(config: &Path) -> State {
    let store = hub::Hub::open(config).unwrap();
    let mut state = State::default();
    for record in store.state.conversations.values() {
        let live: Vec<_> = record.projections.iter().filter(|p| !p.removed).collect();
        let key = |p: &hub::Projection| p.source.path.to_string_lossy().into_owned();
        let origin = live.iter().find(|p| p.origin).map(|p| key(p));
        for p in &live {
            let others: Vec<Continuation> = live
                .iter()
                .filter(|q| q.source.path != p.source.path)
                .map(|q| Continuation { tool: q.source.tool, id: q.source.id.clone(), cwd: q.source.cwd.clone(), root: q.source.root.clone(), revision: PROJECTION_REVISION })
                .collect();
            state.entries.insert(key(p), Entry {
                source: p.source.clone(),
                seen: String::new(),
                imported: !p.origin,
                title: record.title.clone(),
                copied_to: if p.origin { others.first().cloned() } else { None },
                copies: if p.origin { others } else { Vec::new() },
            });
            if !p.origin && let Some(origin) = &origin {
                state.lineage.insert(key(p), origin.clone());
            }
        }
    }
    state
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
    let archived = f.pair.codex.root.join("archived_sessions").join(external.path.file_name().unwrap());
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
    assert!(changed.errors.is_empty(), "{:?}", changed.errors);
    assert_eq!(changed.rows.len(), 1, "the continued import is the same conversation");
    assert_eq!(transcripts(&f.pair.codex.root), vec![external.path.clone()], "the continued import is the Codex projection");
    assert_eq!(read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(), continued);
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
    assert_eq!(first.published, 3, "every other namespace, including the same CLI's other account");
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.rows.values().next().unwrap().copies.len(), 3);
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
    assert_eq!(workspace_scan(&f, &scope).published, 3);
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    let state = workspace_state(&f);
    for root in [&f.pair.claude.root, &default_pair.claude.root] {
        assert!(state.entries.values().any(|e| &e.source.root == root
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
fn workspace_discovery_skips_symlinks_and_subagents_and_accepts_unknown_versions() {
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
    let accepted = workspace_scan(&f, &scope);
    assert_eq!(accepted.rows.len(), 1);
    assert_eq!(accepted.rows.values().next().unwrap().phase, Phase::Synced, "{:?}", accepted.errors);
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
    let written = transcripts(&f.pair.codex.root);
    assert_eq!(written.len(), 1, "the unregistered copy keeps its native ID for the retry");
    let target_path = written[0].clone();
    let original = fs::read(&target_path).unwrap();
    let complete = scan_with_index(
        &f.config,
        &f.pair,
        &f.gate,
        &mut Cache::new(),
        Some(&scope),
        |source| {
            assert_eq!(source.path, target_path);
            assert_eq!(fs::read(&source.path).unwrap(), original);
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(complete.published, 1);
    assert_eq!(complete.rows.values().next().unwrap().phase, Phase::Synced);
    assert_eq!(transcripts(&f.pair.codex.root), vec![target_path]);
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
        if phase == Phase::Failed {
            row.detail = "Connected account directory is missing or symlinked".into();
        }
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
        fs::write(f.pair.codex.root.join("state_5.sqlite"), name).unwrap();
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
    assert_eq!(status.rows.values().next().unwrap().title, "Shared native name");
    assert_eq!(read_messages(&f.source).unwrap(), continued);
    assert_eq!(titles::projection(&f.source).unwrap().as_deref(), Some("Shared native name"));
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

/// Worker tests register no native metadata, like `scan_with_index`.
#[derive(Default)]
struct NoNativeMetadata;
impl worker::NativeMetadata for NoNativeMetadata {
    fn begin_scan(&mut self) {}
    fn apply(&mut self, _: &Source, _: &Gate, _: Metadata<'_>) -> Result<Option<String>> {
        Ok(None)
    }
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
            worker::serve_with::<NoNativeMetadata>(std::io::BufReader::new(worker_end), Lines(tx, Vec::new()), &config, recycle_peak, move |_, workspaces| {
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
    assert!(worker::cache_path(&f.config).exists(), "a closed worker leaves its results for the next launch");
    let mut relaunched = WorkerFixture::start(&f, u64::MAX);
    assert_eq!(relaunched.scan(&f, 3).published, 0);
    assert!(!worker::cache_path(&f.config).exists());
    drop(relaunched.gui);
    relaunched.done.join().unwrap().unwrap();

    let mut extended = messages();
    extended.push(Message { role: "user".into(), text: "A new turn".into(), images: vec![] });
    extended.push(Message { role: "assistant".into(), text: "New answer".into(), images: vec![] });
    let mut recycling = WorkerFixture::start(&f, 0);
    fs::write(&f.source.path, format::render(f.source.tool, &f.source.id, &f.source.cwd, "2026-10-14T00:00:00Z", &extended).unwrap()).unwrap();
    assert_eq!(recycling.scan(&f, 30).published, 1);
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
    let key = f.source.path.to_string_lossy().into_owned();
    let attempted = cache.hub[&key];
    let mut restored: Cache = serde_json::from_slice(&serde_json::to_vec(&cache).unwrap()).unwrap();
    assert_eq!(restored.hub, cache.hub);
    let second = scan(&f.config, &f.pair, &f.gate, &mut restored, |_| {}).unwrap();
    assert_eq!(restored.hub[&key], attempted, "the failing history is not read again");
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
    let row = status.rows.values().next().unwrap();
    assert_eq!(row.phase, Phase::Synced, "its copies remain available");
    assert!(f.live().iter().any(|p| p.source.root != f.source.root && storage::exists(&p.source)));
}

/// Reference search over publication edges, compared with `canonical::Links`.
fn searched_members(state: &State, key: &str) -> std::collections::BTreeSet<String> {
    let ids: HashMap<_, _> = state
        .entries
        .iter()
        .map(|(key, entry)| {
            (
                (entry.source.tool, &entry.source.root, &entry.source.id),
                key,
            )
        })
        .collect();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for (source, target) in &state.lineage {
        if state.entries.contains_key(source) && state.entries.contains_key(target) {
            edges.entry(source).or_default().push(target);
            edges.entry(target).or_default().push(source);
        }
    }
    for (source, entry) in &state.entries {
        for copy in entry.copies.iter().chain(entry.copied_to.iter()) {
            if let Some(target) = ids.get(&(copy.tool, &copy.root, &copy.id)) {
                edges.entry(source).or_default().push(target);
                edges.entry(target).or_default().push(source);
            }
        }
    }
    let mut members = std::collections::BTreeSet::new();
    let mut pending = vec![key];
    while let Some(key) = pending.pop() {
        if members.insert(key.to_owned()) {
            pending.extend(edges.get(key).into_iter().flatten().copied());
        }
    }
    members
}

#[test]
fn computed_groups_match_a_search_over_publication_edges() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let roots = [PathBuf::from("/a"), PathBuf::from("/b")];
    for _ in 0..20 {
        let mut state = State::default();
        let count = 2 + next(40);
        for i in 0..count {
            let tool = if next(2) == 0 { Tool::Claude } else { Tool::Codex };
            let root = roots[next(2) as usize].clone();
            // Repeated ids exercise entries that share one native identity.
            let id = format!("id-{}", next(count));
            let source = Source { tool, root: root.clone(), path: root.join(format!("{i}.jsonl")), cwd: "/w".into(), id };
            state.entries.insert(format!("k{i}"), Entry { source, seen: String::new(), imported: next(2) == 0, title: String::new(), copied_to: None, copies: Vec::new() });
        }
        let keys: Vec<String> = state.entries.keys().cloned().collect();
        for _ in 0..next(count) {
            let (a, b) = (&keys[next(count) as usize], &keys[next(count) as usize]);
            state.lineage.insert(a.clone(), if next(5) == 0 { "missing".into() } else { b.clone() });
        }
        for key in &keys {
            for _ in 0..next(3) {
                let target = &state.entries[&keys[next(count) as usize]].source;
                let copy = Continuation { tool: target.tool, id: if next(6) == 0 { "absent".into() } else { target.id.clone() }, cwd: "/w".into(), root: target.root.clone(), revision: PROJECTION_REVISION };
                let entry = state.entries.get_mut(key).unwrap();
                if next(3) == 0 { entry.copied_to = Some(copy) } else { entry.copies.push(copy) }
            }
        }
        let links = canonical::Links::new(&state);
        for key in keys.iter().map(String::as_str).chain(["unknown"]) {
            assert_eq!(links.members(key), searched_members(&state, key), "{key}");
        }
    }
}


fn text(role: &str, text: &str) -> Message {
    Message { role: role.into(), text: text.into(), images: Vec::new() }
}

#[test]
fn hub_events_append_rewrite_and_branch_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("kiln");
    let first = hub::NativeRef { tool: Tool::Claude, root: PathBuf::from("/a"), id: "one".into() };
    let cid = hub::conversation_id(&first);
    let mut store = hub::Hub::open(&config).unwrap();
    store.state.conversations.insert(cid.clone(), hub::Record::new("Title", "/w"));
    let old = vec![text("user", "q1"), text("assistant", "a1"), text("user", "q2")];
    store.append(&cid, &old).unwrap();
    assert_eq!(store.events(&cid).unwrap(), old);
    let new = vec![text("user", "other q"), text("assistant", "other a")];
    let replaced = store.rewrite(&cid, 1, &new).unwrap();
    assert_eq!(replaced, old[1..].to_vec());
    let mut expected = old[..1].to_vec();
    expected.extend(new.clone());
    assert_eq!(store.events(&cid).unwrap(), expected);
    let branch = store.save_branch(&cid, 1, &replaced, Some(first.clone()), None).unwrap();
    assert_eq!(store.branch_events(&branch).unwrap(), replaced);
    store.save().unwrap();
    let reopened = hub::Hub::open(&config).unwrap();
    let record = &reopened.state.conversations[&cid];
    assert_eq!(record.events, 3);
    assert_eq!(record.branches.len(), 1);
    assert_eq!(record.branches[0].base, 1);
    assert_eq!(reopened.events(&cid).unwrap(), expected);
}

#[test]
fn hub_identity_is_stable_and_aliases_resolve() {
    let temp = tempfile::tempdir().unwrap();
    let a = hub::NativeRef { tool: Tool::Codex, root: PathBuf::from("/r"), id: "id-a".into() };
    let alias = hub::NativeRef { tool: Tool::Codex, root: PathBuf::from("/r"), id: "id-old".into() };
    assert_eq!(hub::conversation_id(&a), hub::conversation_id(&a.clone()));
    assert_ne!(hub::conversation_id(&a), hub::conversation_id(&alias));
    let mut store = hub::Hub::open(&temp.path().join("kiln")).unwrap();
    let cid = hub::conversation_id(&a);
    let mut record = hub::Record::new("T", "/w");
    record.projections.push(hub::Projection {
        source: Source { tool: Tool::Codex, root: PathBuf::from("/r"), path: PathBuf::from("/r/x.jsonl"), cwd: "/w".into(), id: "id-a".into() },
        synced: 0, stamp: None, origin: true, removed: false, title: String::new(), checkpoint: None, database: None,
    });
    record.aliases.push(alias.clone());
    store.state.conversations.insert(cid.clone(), record);
    assert_eq!(store.find(&a), Some(cid.as_str()));
    assert_eq!(store.find(&alias), Some(cid.as_str()));
    assert_eq!(store.find(&hub::NativeRef { id: "missing".into(), ..a }), None);
}

fn turns(names: &[&str]) -> Vec<Message> {
    names.iter().enumerate().map(|(i, n)| text(if i % 2 == 0 { "user" } else { "assistant" }, n)).collect()
}

#[test]
fn reconcile_classifies_unchanged_append_behind_and_divergence() {
    use hub::{Outcome, reconcile};
    let e = turns(&["a", "b", "c", "d"]);
    assert_eq!(reconcile(&e, 4, &e, true), Outcome::Unchanged);
    assert_eq!(reconcile(&e[..2], 2, &e, false), Outcome::Append(4));
    assert_eq!(reconcile(&e, 2, &e[..2], true), Outcome::Behind, "a projection missing later events only needs an append");
    let rewound = turns(&["a", "b", "x"]);
    assert_eq!(reconcile(&e, 4, &rewound, true), Outcome::NativeWins { base: 2 });
    let both = turns(&["a", "b", "x", "y"]);
    assert_eq!(reconcile(&e, 2, &both, false), Outcome::CanonicalWins { base: 2 }, "ties keep the canonical history");
    assert_eq!(reconcile(&e, 4, &e[..2], true), Outcome::NativeWins { base: 2 }, "a newer native rollback removes events");
    assert_eq!(reconcile(&e, 4, &e[..2], false), Outcome::Behind);
}

#[test]
fn reconciled_history_is_always_the_winning_side() {
    use hub::{Outcome, reconcile};
    let mut seed = 0x51ed_270b_9a3f_c6e1u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for _ in 0..500 {
        let shared = next(5) as usize;
        let make = |prefix: &str, n: usize| -> Vec<Message> { (0..n).map(|i| text(if i % 2 == 0 { "user" } else { "assistant" }, &format!("{prefix}{i}"))).collect() };
        let base = make("s", shared);
        let mut e = base.clone();
        e.extend(make("c", next(4) as usize).into_iter().skip(0));
        let mut m = base.clone();
        let native_tail = make(if next(2) == 0 { "c" } else { "n" }, next(4) as usize);
        m.extend(native_tail);
        let synced = next(e.len() as u64 + 1) as usize;
        let newer = next(2) == 0;
        let outcome = reconcile(&e, synced, &m, newer);
        let (canonical, branch): (Vec<Message>, Vec<Message>) = match outcome {
            Outcome::Unchanged | Outcome::Behind => (e.clone(), vec![]),
            Outcome::Append(n) => {
                assert_eq!(n, m.len());
                (m.clone(), vec![])
            }
            Outcome::NativeWins { base } => (m.clone(), e[base..].to_vec()),
            Outcome::CanonicalWins { base } => (e.clone(), m[base..].to_vec()),
        };
        let p = e.iter().zip(&m).take_while(|(a, b)| a == b).count();
        if !(m.starts_with(&e) || e.starts_with(&m)) {
            let winner = if newer { &m } else { &e };
            assert_eq!(&canonical, winner, "{outcome:?}");
            let loser = if newer { &e } else { &m };
            assert_eq!(branch, loser[p..].to_vec());
        } else if m.starts_with(&e) {
            assert_eq!(canonical, m, "{outcome:?}");
        } else if newer && synced > m.len() {
            assert_eq!((canonical, branch), (m.clone(), e[m.len()..].to_vec()), "a newer rollback wins");
        } else {
            assert_eq!(canonical, e, "{outcome:?}");
        }
    }
}

impl Fixture {
    fn hub(&self) -> hub::HubState {
        hub::Hub::open(&self.config).unwrap().state
    }
    fn live(&self) -> Vec<hub::Projection> {
        self.hub().conversations.values().flat_map(|r| r.projections.clone()).filter(|p| !p.removed).collect()
    }
    fn peer(&self) -> Source {
        self.live().into_iter().find(|p| p.source.root != self.source.root).expect("projection in the other namespace").source
    }
    fn projection_in(&self, root: &Path) -> Source {
        self.live().into_iter().find(|p| p.source.root == root).expect("projection in namespace").source
    }
}
fn transcripts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.join("projects"), root.join("sessions")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
    }
    out
}
fn write_history(source: &Source, history: &[Message], modified: SystemTime) {
    fs::write(&source.path, format::render(source.tool, &source.id, &source.cwd, "2026-10-14T00:00:00Z", history).unwrap()).unwrap();
    fs::File::options().write(true).open(&source.path).unwrap().set_modified(modified).unwrap();
}

#[test]
fn hub_continuations_reach_the_original_without_echo() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let first = f.scan();
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(first.published, 1);
        assert_eq!(f.scan().published, 0);
        assert_eq!(f.hub().conversations.len(), 1);
        let peer = f.peer();
        assert_eq!(peer.tool, opposite(tool));
        assert_eq!(read_messages(&peer).unwrap(), messages());
        let mut extended = messages();
        extended.extend(turns(&["Continue from the other CLI.", "The exact token is kiln-orchid-731."]));
        write_history(&peer, &extended, SystemTime::now() + Duration::from_secs(30));
        let second = f.scan();
        assert!(second.errors.is_empty(), "{:?}", second.errors);
        assert_eq!(second.published, 1);
        assert_eq!(f.scan().published, 0);
        assert_eq!(read_messages(&f.source).unwrap(), extended, "the original native ID receives the continuation");
        assert_eq!(f.live().len(), 2);
    }
}

#[test]
fn diverged_histories_keep_the_newest_branch_and_archive_the_other() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.peer();
    let now = SystemTime::now();
    let mut older = messages();
    older.extend(turns(&["claude side", "claude reply"]));
    let mut newer = messages();
    newer.extend(turns(&["codex side", "codex reply"]));
    write_history(&f.source, &older, now + Duration::from_secs(40));
    write_history(&peer, &newer, now + Duration::from_secs(50));
    let original = fs::read(&f.source.path).unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert!(!f.source.path.exists(), "the losing native history leaves the resume list");
    let claude = f.projection_in(&f.pair.claude.root);
    assert_ne!(claude.id, f.source.id, "the namespace receives a new projection");
    assert_eq!(read_messages(&claude).unwrap(), newer);
    assert_eq!(read_messages(&peer).unwrap(), newer);
    let state = f.hub();
    let record = state.conversations.values().next().unwrap();
    assert_eq!(record.branches.len(), 1);
    let branch = &record.branches[0];
    assert_eq!(fs::read(branch.archived.as_ref().unwrap()).unwrap(), original, "the archived file keeps its bytes");
    assert_eq!(hub::Hub::open(&f.config).unwrap().branch_events(&branch.id).unwrap(), turns(&["claude side", "claude reply"]));
    assert!(record.aliases.iter().any(|a| a.id == f.source.id));
    assert_eq!(f.scan().published, 0);
}

#[test]
fn a_namespace_never_holds_two_live_projections() {
    let f = Fixture::new(Tool::Codex);
    f.scan();
    let mut seed = 0x2b99_2ddf_a232_49d6u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut clock = SystemTime::now() + Duration::from_secs(60);
    for round in 0..16 {
        let state = f.hub();
        let cid = state.conversations.keys().next().unwrap().clone();
        let events = hub::Hub::open(&f.config).unwrap().events(&cid).unwrap();
        let live = f.live();
        let writers: Vec<_> = match next(3) {
            0 => vec![live[next(2) as usize].source.clone()],
            1 => live.iter().map(|p| p.source.clone()).collect(),
            _ => vec![],
        };
        for (i, writer) in writers.iter().enumerate() {
            let mut history = events.clone();
            history.extend(turns(&[&format!("round {round} writer {i}"), "reply"]));
            clock += Duration::from_secs(1 + next(5));
            write_history(writer, &history, clock);
        }
        let status = f.scan();
        assert!(status.errors.is_empty(), "round {round}: {:?}", status.errors);
        for root in [&f.pair.claude.root, &f.pair.codex.root] {
            assert_eq!(transcripts(root).len(), 1, "round {round}: one live transcript in {}", root.display());
        }
        let store = hub::Hub::open(&f.config).unwrap();
        let canonical = store.events(&cid).unwrap();
        for p in f.live() {
            assert_eq!(read_messages(&p.source).unwrap(), canonical, "round {round}");
        }
    }
}

#[test]
fn projection_modified_within_the_quiet_window_is_not_written() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.peer();
    let mut cache = Cache::new();
    cache.quiet = Duration::from_secs(10);
    let mut continued = messages();
    continued.extend(turns(&["next question", "next answer"]));
    write_history(&f.source, &continued, SystemTime::now());
    let waiting = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(waiting.published, 0, "a transcript being written is not read");
    let peer_before = fs::read(&peer.path).unwrap();
    fs::File::options().write(true).open(&f.source.path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
    fs::File::options().write(true).open(&peer.path).unwrap().set_modified(SystemTime::now()).unwrap();
    let deferred = scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(deferred.published, 0);
    assert_eq!(fs::read(&peer.path).unwrap(), peer_before, "a projection being written is not appended to");
    fs::File::options().write(true).open(&peer.path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
    assert_eq!(scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap().published, 1);
    assert_eq!(read_messages(&peer).unwrap(), continued);
}

fn legacy_entry(source: &Source, imported: bool, copies: Vec<Continuation>) -> Entry {
    Entry { source: source.clone(), seen: String::new(), imported, title: String::new(), copied_to: copies.first().cloned(), copies }
}
fn legacy_copy(f: &Fixture, name: &str, history: &[Message], modified: SystemTime) -> Source {
    let copy = destination(&f.pair, &f.source, &stable_id(name), chrono::Utc::now()).unwrap();
    publish(&copy.path, &format::render(copy.tool, &copy.id, &copy.cwd, "2026-10-13T00:00:00Z", history).unwrap()).unwrap();
    fs::File::options().write(true).open(&copy.path).unwrap().set_modified(modified).unwrap();
    copy
}
fn write_legacy(f: &Fixture, state: &State) {
    let path = f.config.join("conversation-sync").join(format!("{}.json", digest(&serde_json::to_vec(&f.pair).unwrap())));
    fs::write(path, serde_json::to_vec(state).unwrap()).unwrap();
}
fn key_of(source: &Source) -> String {
    source.path.to_string_lossy().into_owned()
}
fn as_continuation(source: &Source) -> Continuation {
    Continuation { tool: source.tool, id: source.id.clone(), cwd: source.cwd.clone(), root: source.root.clone(), revision: PROJECTION_REVISION }
}

#[test]
fn migration_keeps_one_projection_per_namespace_and_archives_extra_copies() {
    let f = Fixture::new(Tool::Claude);
    let now = SystemTime::now();
    let older = legacy_copy(&f, "legacy-a", &messages(), now - Duration::from_secs(600));
    let mut longer = messages();
    longer.extend(turns(&["continued in codex", "codex reply"]));
    let newer = legacy_copy(&f, "legacy-b", &longer, now - Duration::from_secs(60));
    fs::File::options().write(true).open(&f.source.path).unwrap().set_modified(now - Duration::from_secs(1200)).unwrap();
    let mut state = State::default();
    state.entries.insert(key_of(&f.source), legacy_entry(&f.source, false, vec![as_continuation(&older), as_continuation(&newer)]));
    state.entries.insert(key_of(&older), legacy_entry(&older, true, vec![]));
    state.entries.insert(key_of(&newer), legacy_entry(&newer, true, vec![]));
    state.lineage.insert(key_of(&older), key_of(&f.source));
    state.lineage.insert(key_of(&newer), key_of(&f.source));
    write_legacy(&f, &state);
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(f.hub().conversations.len(), 1);
    assert_eq!(transcripts(&f.pair.codex.root), vec![newer.path.clone()], "only the most complete copy stays in the Codex list");
    assert!(f.pair.codex.root.join("archived_sessions").join(older.path.file_name().unwrap()).exists());
    assert_eq!(read_messages(&f.source).unwrap(), longer, "the original receives the newest continuation");
    assert!(f.hub().conversations.values().next().unwrap().aliases.iter().any(|a| a.id == older.id));
}

#[test]
fn migration_keeps_unrelated_conversations_with_equal_titles() {
    let f = Fixture::new(Tool::Claude);
    let mut other_history = turns(&["A different question", "A different answer"]);
    other_history[0].text = messages()[0].text.clone() + " (separate)";
    let other = Source { id: stable_id("separate-original"), ..f.source.clone() };
    let other = Source { path: f.source.path.with_file_name(format!("{}.jsonl", other.id)), ..other };
    publish(&other.path, &format::render(other.tool, &other.id, &other.cwd, "2026-10-13T00:00:00Z", &other_history).unwrap()).unwrap();
    let mut state = State::default();
    let mut first = legacy_entry(&f.source, false, vec![]);
    first.title = "Same title".into();
    let mut second = legacy_entry(&other, false, vec![]);
    second.title = "Same title".into();
    state.entries.insert(key_of(&f.source), first);
    state.entries.insert(key_of(&other), second);
    write_legacy(&f, &state);
    f.scan();
    assert_eq!(f.hub().conversations.len(), 2);
    assert!(f.source.path.exists() && other.path.exists());
}

#[test]
fn migration_resumes_after_interruption_without_new_projections() {
    let f = Fixture::new(Tool::Claude);
    let copy = legacy_copy(&f, "legacy-only", &messages(), SystemTime::now() - Duration::from_secs(60));
    let mut state = State::default();
    state.entries.insert(key_of(&f.source), legacy_entry(&f.source, false, vec![as_continuation(&copy)]));
    state.entries.insert(key_of(&copy), legacy_entry(&copy, true, vec![]));
    state.lineage.insert(key_of(&copy), key_of(&f.source));
    write_legacy(&f, &state);
    f.scan();
    let before: Vec<String> = f.live().iter().map(|p| p.source.id.clone()).collect();
    let mut store = hub::Hub::open(&f.config).unwrap();
    store.state.migrated.clear();
    store.save().unwrap();
    f.scan();
    let after: Vec<String> = f.live().iter().map(|p| p.source.id.clone()).collect();
    assert_eq!(before, after);
    assert_eq!(transcripts(&f.pair.codex.root), vec![copy.path.clone()], "the migrated copy is the projection");
}

#[test]
fn unparseable_legacy_transcripts_are_left_untouched() {
    let f = Fixture::new(Tool::Claude);
    let broken = legacy_copy(&f, "legacy-broken", &messages(), SystemTime::now() - Duration::from_secs(60));
    fs::write(&broken.path, b"not a transcript\n").unwrap();
    let mut state = State::default();
    state.entries.insert(key_of(&f.source), legacy_entry(&f.source, false, vec![as_continuation(&broken)]));
    state.entries.insert(key_of(&broken), legacy_entry(&broken, true, vec![]));
    state.lineage.insert(key_of(&broken), key_of(&f.source));
    write_legacy(&f, &state);
    f.scan();
    assert_eq!(fs::read(&broken.path).unwrap(), b"not a transcript\n");
    assert_eq!(transcripts(&f.pair.codex.root), vec![broken.path.clone()], "no second Codex history is created");
}

fn rewrite_rows(path: &Path, change: impl Fn(&mut serde_json::Value)) {
    let mut data = rows(&fs::read(path).unwrap());
    for row in &mut data {
        change(row);
    }
    fs::write(path, data.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
}

#[test]
fn unknown_cli_versions_with_known_content_synchronize() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        rewrite_rows(&f.source.path, |row| {
            if row["version"].is_string() {
                row["version"] = json!("9.9.9");
            }
            if row["type"] == "session_meta" {
                row["payload"]["cli_version"] = json!("9.9.9");
            }
        });
        let status = f.scan();
        assert!(status.errors.is_empty(), "{tool:?}: {:?}", status.errors);
        assert_eq!(status.published, 1);
        assert_eq!(read_messages(&f.peer()).unwrap(), messages());
    }
}

#[test]
fn unknown_content_pauses_only_that_conversation() {
    let f = Fixture::new(Tool::Claude);
    let other = Source { id: stable_id("unknown-content"), ..f.source.clone() };
    let other = Source { path: f.source.path.with_file_name(format!("{}.jsonl", other.id)), ..other };
    publish(&other.path, &format::render(other.tool, &other.id, &other.cwd, "2026-10-13T00:00:00Z", &turns(&["Another question", "Another answer"])).unwrap()).unwrap();
    rewrite_rows(&other.path, |row| {
        if row["type"] == "user" {
            row["message"]["content"] = json!([{"type":"hologram","data":"x"}]);
        }
    });
    publish(&f.config.join("conversation-sync/tracked/other.json"), &serde_json::to_vec(&other).unwrap()).unwrap();
    let status = f.scan();
    assert_eq!(status.published, 1, "the readable conversation still synchronizes");
    let failed: Vec<_> = status.rows.values().filter(|r| r.phase == Phase::Failed).collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].detail.contains("Unsupported content"), "{}", failed[0].detail);
    assert_eq!(status.rows.values().filter(|r| r.phase == Phase::Synced).count(), 1);
}

#[test]
fn paused_conversations_retry_after_a_build_change() {
    let f = Fixture::new(Tool::Claude);
    rewrite_rows(&f.source.path, |row| {
        if row["type"] == "user" {
            row["message"]["content"] = json!([{"type":"hologram","data":"x"}]);
        }
    });
    let key = f.source.path.to_string_lossy().into_owned();
    let mut cache = Cache::new();
    scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    let first = cache.hub[&key].1;
    scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert_eq!(cache.hub[&key].1, first, "an unchanged failure waits");
    cache.build = "an older build".into();
    scan(&f.config, &f.pair, &f.gate, &mut cache, |_| {}).unwrap();
    assert!(cache.hub[&key].1 > first, "a new Kiln build reads the conversation again");
}

#[test]
fn fallback_titles_drop_command_tags_and_image_placeholders() {
    let history = vec![
        text("user", "<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>"),
        text("assistant", "Model set"),
        text("user", "<image name=[Image #1] path=\"/tmp/clipboard.png\">\nFix the layout shown here [Image #1]"),
    ];
    assert_eq!(titles::fallback(&history), "Fix the layout shown here");
    assert_eq!(titles::clean("<command-name>/mcp</command-name>"), "");
    assert_eq!(titles::clean("<local-command-stdout>ok</local-command-stdout> Deploy notes"), "Deploy notes");
}

#[test]
fn canonical_title_follows_the_latest_native_rename_in_every_projection() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.peer();
    let mut file = fs::OpenOptions::new().append(true).open(&f.source.path).unwrap();
    writeln!(file, "{}", json!({"type":"custom-title","customTitle":"Renamed in Claude","sessionId":f.source.id})).unwrap();
    drop(file);
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(status.rows.values().next().unwrap().title, "Renamed in Claude");
    assert_eq!(titles::projection(&peer).unwrap().as_deref(), Some("Renamed in Claude"));
    assert_eq!(f.scan().published, 0);
}

fn attention_row(f: &Fixture, detail: &str) -> Conversation {
    Conversation {
        workspace: Some(PathBuf::from(&f.source.cwd)),
        source: f.source.clone(),
        title: "Attention fixture".into(),
        phase: Phase::Failed,
        detail: detail.into(),
        target: None,
        copies: vec![],
        branches: vec![],
    }
}

#[test]
fn only_resolvable_conditions_raise_the_badge() {
    let f = Fixture::new(Tool::Claude);
    let root = PathBuf::from(&f.source.cwd);
    let mut manager = Manager::default();
    manager.workspaces = vec![root.clone()];
    manager.status.rows.insert("info".into(), attention_row(&f, "Unsupported content (hologram); this conversation was not synchronized"));
    manager.revision += 1;
    assert!(!matches!(manager.badge_state(&root), Badge::Attention), "a format Kiln cannot read yet is not the user's to fix");
    manager.status.rows.insert("account".into(), attention_row(&f, "Connected account directory is missing or symlinked"));
    manager.revision += 1;
    assert!(matches!(manager.badge_state(&root), Badge::Attention));
    assert_eq!(classify("No space left on device (os error 28)"), Some(Attention::Storage));
    assert_eq!(classify("Install Codex CLI to register /resume conversations"), Some(Attention::CodexCli));
    assert_eq!(classify("Linked histories diverged; both conversations were preserved"), None);
}

#[test]
fn ignored_warnings_leave_the_badge_until_a_different_condition() {
    let f = Fixture::new(Tool::Claude);
    let root = PathBuf::from(&f.source.cwd);
    let mut manager = Manager::default();
    manager.workspaces = vec![root.clone()];
    manager.status.rows.insert("account".into(), attention_row(&f, "Connected account directory is missing or symlinked"));
    manager.ignore("account", Attention::Account);
    manager.revision += 1;
    assert!(!matches!(manager.badge_state(&root), Badge::Attention));
    manager.status.rows.insert("account".into(), attention_row(&f, "No space left on device (os error 28)"));
    manager.revision += 1;
    assert!(matches!(manager.badge_state(&root), Badge::Attention), "a different problem warns again");
    manager.unignore("account");
    manager.status.rows.insert("account".into(), attention_row(&f, "Connected account directory is missing or symlinked"));
    manager.revision += 1;
    assert!(matches!(manager.badge_state(&root), Badge::Attention));
}

#[test]
fn resolved_conditions_clear_on_the_next_scan() {
    let f = Fixture::new(Tool::Claude);
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let mut scope = workspace_scope(&f);
    let unavailable = PathBuf::from(&f.source.cwd).join("account-not-created");
    scope.roots.push((Tool::Codex, unavailable.clone()));
    let mut cache = Cache::new();
    let first = scan_scoped(&f.config, &f.pair, &f.gate, &mut cache, Some(&scope), |_| {}).unwrap();
    let row = first.rows.values().next().unwrap();
    assert_eq!(classify(&row.detail), Some(Attention::Account), "{}", row.detail);
    private_dir(&unavailable).unwrap();
    for attempt in cache.hub.values_mut() {
        attempt.1 = SystemTime::now() - Duration::from_secs(600);
    }
    let second = scan_scoped(&f.config, &f.pair, &f.gate, &mut cache, Some(&scope), |_| {}).unwrap();
    let row = second.rows.values().next().unwrap();
    assert_eq!(row.phase, Phase::Synced, "{}", row.detail);
}

#[test]
fn restoring_a_branch_creates_a_separate_conversation_everywhere() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.peer();
    let now = SystemTime::now();
    let mut older = messages();
    older.extend(turns(&["claude side", "claude reply"]));
    let mut newer = messages();
    newer.extend(turns(&["codex side", "codex reply"]));
    write_history(&f.source, &older, now + Duration::from_secs(40));
    write_history(&peer, &newer, now + Duration::from_secs(50));
    f.scan();
    let branch = f.hub().conversations.values().next().unwrap().branches[0].id.clone();
    hub::request_restore(&f.config, &branch, "Restored fixture").unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let state = f.hub();
    assert_eq!(state.conversations.len(), 2);
    let restored = state.conversations.values().find(|r| r.title == "Restored fixture").expect("restored conversation");
    let live: Vec<_> = restored.projections.iter().filter(|p| !p.removed).collect();
    assert_eq!(live.len(), 2, "projected into every namespace");
    for p in live {
        assert_eq!(read_messages(&p.source).unwrap(), older);
    }
    assert!(state.conversations.values().any(|r| r.branches.iter().any(|b| b.id == branch && b.restored)));
    assert_eq!(f.scan().published, 0);
}

#[test]
fn progress_ui_offers_actions_ignore_and_restore() {
    use egui_kittest::{Harness, kittest::Queryable};
    let f = Fixture::new(Tool::Claude);
    let mut manager = Manager::default();
    manager.status.rows.insert("account".into(), attention_row(&f, "Connected account directory is missing or symlinked"));
    let mut kept = attention_row(&f, "");
    kept.phase = Phase::Synced;
    kept.branches = vec![BranchRow { id: "branch".into(), events: 2, at: SystemTime::now() }];
    manager.status.rows.insert("kept".into(), kept);
    manager.status.rows.insert("info".into(), attention_row(&f, "Unsupported content (hologram); this conversation was not synchronized"));
    let mut initialized = false;
    let mut h = Harness::builder().with_size([560.0, 700.0]).build_ui_state(
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
    assert!(h.query_by_label("계정 설정 열기").is_some());
    assert!(h.query_by_label("무시").is_some());
    assert!(h.query_by_label("복구").is_some());
    assert!(h.query_by_label("보류됨").is_some());
    h.render().unwrap().save("/tmp/kiln-sync-attention.png").unwrap();
}

#[test]
fn incremental_and_full_parsing_agree_under_random_appends() {
    let mut seed = 0x6c07_8965_3a1f_92d4u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let (full, cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
        let mut expected = messages();
        assert_eq!(full, expected);
        let mut cp = cp.expect("a completed turn has a checkpoint");
        for round in 0..10 {
            let mut added = Vec::new();
            for k in 0..1 + next(3) {
                added.extend(turns(&[&format!("q{round}-{k}"), &format!("a{round}-{k}")]));
            }
            expected.extend(added.clone());
            canonical::update(&f.source, &expected, "Fixture title", "2026-10-15T00:00:00.000Z").unwrap();
            let (appended, after) = checkpoint::since(&f.source, &cp).unwrap().expect("appended records read on their own");
            assert_eq!(appended, added, "{tool:?} round {round}");
            let (full, full_cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
            assert_eq!(full, expected);
            assert_eq!(Some(&after), full_cp.as_ref(), "{tool:?} round {round}");
            cp = after;
        }
    }
}

#[test]
fn checkpointed_append_matches_a_full_read() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let (_, cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
        let mut cp = cp.unwrap();
        let mut expected = messages();
        for round in 0..6 {
            let added = turns(&[&format!("appended question {round}"), &format!("appended answer {round}")]);
            expected.extend(added.clone());
            let before = fs::read(&f.source.path).unwrap();
            cp = canonical::append_since(&f.source, &cp, &added, "Fixture title", "2026-10-15T00:00:00.000Z").unwrap().expect("a checkpointed append");
            assert!(fs::read(&f.source.path).unwrap().starts_with(&before), "existing bytes are kept");
            let (full, full_cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
            assert_eq!(full, expected, "{tool:?} round {round}");
            assert_eq!(Some(&cp), full_cp.as_ref());
        }
    }
}

#[test]
fn prefix_change_falls_back_to_a_full_parse() {
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        let (_, cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
        let cp = cp.unwrap();
        let mut changed = turns(&["a different opening question", "a different reply"]);
        changed.extend(turns(&["and more", "and more replies"]));
        fs::write(&f.source.path, format::render(tool, &f.source.id, &f.source.cwd, "2026-10-14T00:00:00Z", &changed).unwrap()).unwrap();
        assert!(checkpoint::since(&f.source, &cp).unwrap().is_none(), "{tool:?}");
    }
}

#[test]
#[ignore]
fn inspect_live_histories_read_only() {
    let path = std::env::var_os("KILN_SYNC_INSPECT_STATE").expect("explicit state file");
    let state: State = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut total, mut bytes) = (0, 0u64);
    let started = Instant::now();
    for (key, entry) in &state.entries {
        if state.archived.contains_key(key) || !storage::exists(&entry.source) {
            continue;
        }
        total += 1;
        bytes += storage::metadata(&entry.source).map(|m| m.len()).unwrap_or(0);
        let outcome = match checkpoint::read(&entry.source, &mut String::new()) {
            Ok((messages, Some(cp))) => match checkpoint::since(&entry.source, &cp) {
                Ok(Some((appended, _))) if appended.is_empty() => format!("ok ({:?})", entry.source.tool),
                Ok(Some(_)) => "checkpoint reads extra messages".into(),
                Ok(None) => "checkpoint rejected".into(),
                Err(e) => format!("checkpoint error: {e}"),
            }
            .replace("ok", if messages.is_empty() { "ok-empty" } else { "ok" }),
            Ok((_, None)) => format!("ok without checkpoint ({:?})", entry.source.tool),
            Err(e) => {
                let e = e.to_string();
                format!("error: {}", e.split(':').next().unwrap_or(&e))
            }
        };
        *outcomes.entry(outcome).or_default() += 1;
    }
    eprintln!("{total} live histories, {:.1} GB, read in {:?}", bytes as f64 / 1e9, started.elapsed());
    for (outcome, count) in outcomes {
        eprintln!("{count:5}  {outcome}");
    }
}

/// Hub round trip with installed CLIs: `export` writes one original per CLI, each with a
/// shell command turn, into `claude/` and `codex/` under `KILN_HUB_FIXTURE_DIR` and projects
/// them; `sync` reads the
/// CLIs' continuations back and prints the last messages of every projection.
#[test]
#[ignore]
fn hub_cli_round_trip() {
    let dir = PathBuf::from(std::env::var_os("KILN_HUB_FIXTURE_DIR").expect("explicit fixture directory"))
        .canonicalize()
        .unwrap();
    let pair = Pair {
        claude: Endpoint { root: dir.join("claude"), label: "fixture".into() },
        codex: Endpoint { root: dir.join("codex"), label: "fixture".into() },
    };
    let config = dir.join("kiln");
    let work = dir.join("work");
    for path in [&pair.claude.root, &pair.codex.root, &config, &work] {
        private_dir(path).unwrap();
    }
    let gate: Gate = Arc::new(AtomicBool::new(true));
    if std::env::var("KILN_HUB_FIXTURE_STEP").as_deref() == Ok("export") {
        let tracked = config.join("conversation-sync/tracked");
        private_dir(&tracked).unwrap();
        for tool in Tool::ALL {
            let id = stable_id(&format!("hub-fixture-{}", tool.key()));
            let path = native_path(tool, &pair.endpoint(tool).root, &work.to_string_lossy(), &id, chrono::Utc::now()).unwrap();
            let source = Source { tool, root: pair.endpoint(tool).root.clone(), path, cwd: work.to_string_lossy().into_owned(), id };
            let mut history = turns(&[&format!("Remember the token started-in-{}.", tool.key()), "Remembered."]);
            history.extend(match tool {
                Tool::Claude => shell_turn(CLAUDE_CALL, CLAUDE_RESULT),
                Tool::Codex => shell_turn(CODEX_CALL, CODEX_RESULT),
            });
            publish(&source.path, &format::render_titled(tool, &source.id, &source.cwd, &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true), &history, &format!("Hub fixture from {}", tool.display_name())).unwrap()).unwrap();
            publish(&tracked.join(format!("{}.json", tool.key())), &serde_json::to_vec(&source).unwrap()).unwrap();
        }
    }
    let status = scan_with_metadata(&config, &pair, &gate, &mut Cache::new(), None, |_, _| Ok(None), |_| {}).unwrap();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let store = hub::Hub::open(&config).unwrap();
    for (cid, record) in &store.state.conversations {
        println!("conversation {cid} \"{}\" ({} events)", record.title, record.events);
        for p in record.projections.iter().filter(|p| !p.removed) {
            let tail: Vec<String> = read_messages(&p.source).unwrap().iter().rev().take(2).rev().map(|m| format!("{}: {}", m.role, m.text)).collect();
            println!("  {} {} {} {:?}", p.source.tool.key(), p.source.id, if p.origin { "origin" } else { "projection" }, tail);
        }
    }
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
}
/// Appends one user and one assistant record shaped like Claude Code's own, without a title row.
fn append_native_claude_turn(source: &Source, question: &str, answer: &str) {
    let all = rows(&fs::read(&source.path).unwrap());
    let leaf = all.iter().rev().find(|r| r["type"] == "assistant").unwrap()["uuid"].as_str().unwrap().to_owned();
    let mut user = all.iter().rev().find(|r| r["type"] == "user").unwrap().clone();
    let mut assistant = all.iter().rev().find(|r| r["type"] == "assistant").unwrap().clone();
    for row in [&mut user, &mut assistant] {
        let object = row.as_object_mut().unwrap();
        for key in ["kiln_projection", "kiln_original", "kiln_text"] {
            object.remove(key);
        }
    }
    let (uid, aid) = (stable_id(&format!("{question}:u")), stable_id(&format!("{question}:a")));
    user["uuid"] = json!(uid);
    user["parentUuid"] = json!(leaf);
    user["message"] = json!({"role": "user", "content": question});
    assistant["uuid"] = json!(aid);
    assistant["parentUuid"] = json!(uid);
    assistant["message"]["content"] = json!([{"type": "text", "text": answer}]);
    assistant["message"]["id"] = json!(format!("msg_{aid}"));
    let mut file = fs::OpenOptions::new().append(true).open(&source.path).unwrap();
    writeln!(file, "{user}").unwrap();
    writeln!(file, "{assistant}").unwrap();
}

#[test]
fn the_more_recently_continued_side_wins_when_both_are_read_incrementally() {
    let f = Fixture::new(Tool::Claude);
    assert!(f.scan().errors.is_empty());
    let peer = f.peer();
    let mut older = messages();
    older.extend(turns(&["claude side", "claude reply"]));
    let mut newer = messages();
    newer.extend(turns(&["codex side", "codex reply"]));
    canonical::update(&f.source, &older, "", "2026-10-15T00:00:00.000Z").unwrap();
    canonical::update(&peer, &newer, "", "2026-10-15T00:00:00.000Z").unwrap();
    let now = SystemTime::now();
    set_modified(&f.source.path, now - Duration::from_secs(30));
    set_modified(&peer.path, now - Duration::from_secs(20));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let cid = f.hub().conversations.keys().next().unwrap().clone();
    assert_eq!(hub::Hub::open(&f.config).unwrap().events(&cid).unwrap(), newer);
    assert!(!f.source.path.exists(), "the earlier continuation is archived");
    assert!(peer.path.exists());
}

#[test]
fn a_rename_in_codex_survives_later_claude_turns() {
    let f = Fixture::new(Tool::Codex);
    assert!(f.scan().errors.is_empty());
    let claude = f.peer();
    fs::write(
        f.pair.codex.root.join("session_index.jsonl"),
        format!("{}\n", json!({"id": f.source.id, "thread_name": "Renamed in Codex"})),
    )
    .unwrap();
    assert!(f.scan().errors.is_empty());
    assert_eq!(f.hub().conversations.values().next().unwrap().title, "Renamed in Codex");
    let sidecar = claude.path.parent().unwrap().join(&claude.id).join("custom-title.json");
    let pinned: serde_json::Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(pinned["customTitle"], "Renamed in Codex", "the Claude sidecar follows the canonical title");
    for (i, (question, answer)) in [("first claude question", "first answer"), ("second claude question", "second answer")].into_iter().enumerate() {
        append_native_claude_turn(&claude, question, answer);
        set_modified(&claude.path, SystemTime::now() - Duration::from_secs(5 - i as u64));
        let status = f.scan();
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        let state = f.hub();
        let record = state.conversations.values().next().unwrap();
        assert_eq!(record.events, 4 + 2 * i, "the continuation was ingested");
        assert_eq!(record.title, "Renamed in Codex");
    }
    assert_eq!(titles::projection(&f.source).unwrap().as_deref(), Some("Renamed in Codex"));
}

#[test]
fn a_restored_branch_keeps_the_history_it_continued_after_a_later_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("kiln");
    let cid = hub::conversation_id(&hub::NativeRef { tool: Tool::Claude, root: PathBuf::from("/a"), id: "one".into() });
    let mut store = hub::Hub::open(&config).unwrap();
    store.state.conversations.insert(cid.clone(), hub::Record::new("T", "/w"));
    store.append(&cid, &turns(&["u0", "a0", "u1", "a1"])).unwrap();
    let branch = store.save_branch(&cid, 4, &turns(&["u2x", "a2x"]), None, None).unwrap();
    store.rewrite(&cid, 2, &turns(&["u1y", "a1y"])).unwrap();
    store.save().unwrap();
    let mut store = hub::Hub::open(&config).unwrap();
    let restored = store.restore(&hub::Restore { branch, title: "R".into() }).unwrap().unwrap();
    let events: Vec<String> = store.events(&restored).unwrap().into_iter().map(|m| m.text).collect();
    assert_eq!(events, ["u0", "a0", "u1", "a1", "u2x", "a2x"]);
}

#[test]
fn a_failing_restore_or_import_record_leaves_synchronization_running() {
    let f = Fixture::new(Tool::Claude);
    f.scan();
    let peer = f.peer();
    let now = SystemTime::now();
    let mut older = messages();
    older.extend(turns(&["claude side", "claude reply"]));
    let mut newer = messages();
    newer.extend(turns(&["codex side", "codex reply"]));
    write_history(&f.source, &older, now + Duration::from_secs(40));
    write_history(&peer, &newer, now + Duration::from_secs(50));
    f.scan();
    let branch = f.hub().conversations.values().next().unwrap().branches[0].id.clone();
    fs::remove_file(f.config.join("conversation-sync/hub/branches").join(format!("{branch}.jsonl"))).unwrap();
    hub::request_restore(&f.config, &branch, "Restored fixture").unwrap();
    fs::write(f.pair.codex.root.join("external_agent_session_imports.json"), b"{not json").unwrap();
    let mut extended = newer.clone();
    extended.extend(turns(&["after the failed restore", "still synchronized"]));
    write_history(&peer, &extended, now + Duration::from_secs(60));
    let status = scan(&f.config, &f.pair, &f.gate, &mut Cache::new(), |_| {}).expect("the scan completes");
    assert_eq!(read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(), extended);
    let failed: Vec<&Conversation> = status.rows.values().filter(|r| r.phase == Phase::Failed).collect();
    assert_eq!(failed.len(), 2, "the restore and the import record each report their failure: {:?}", status.errors);
}

fn archiving_into(source: &Source) -> Result<Option<String>> {
    let physical = storage::physical(source);
    let path = source.root.join("archived_sessions").join(physical.file_name().unwrap());
    private_dir(path.parent().unwrap())?;
    fs::rename(&physical, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[test]
fn copies_left_by_a_failed_migration_archive_leave_once_archiving_works() {
    let f = Fixture::new(Tool::Claude);
    let now = SystemTime::now();
    let older = legacy_copy(&f, "legacy-a", &messages(), now - Duration::from_secs(600));
    let mut longer = messages();
    longer.extend(turns(&["continued in codex", "codex reply"]));
    let newer = legacy_copy(&f, "legacy-b", &longer, now - Duration::from_secs(60));
    set_modified(&f.source.path, now - Duration::from_secs(1200));
    let mut state = State::default();
    state.entries.insert(key_of(&f.source), legacy_entry(&f.source, false, vec![as_continuation(&older), as_continuation(&newer)]));
    state.entries.insert(key_of(&older), legacy_entry(&older, true, vec![]));
    state.entries.insert(key_of(&newer), legacy_entry(&newer, true, vec![]));
    state.lineage.insert(key_of(&older), key_of(&f.source));
    state.lineage.insert(key_of(&newer), key_of(&f.source));
    write_legacy(&f, &state);
    let available = std::cell::Cell::new(false);
    let metadata = |source: &Source, request: Metadata<'_>| -> Result<Option<String>> {
        if matches!(request, Metadata::Archive) {
            ensure!(available.get(), "Install Codex CLI to register /resume conversations");
            return archiving_into(source);
        }
        Ok(None)
    };
    let first = scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), None, &metadata, |_| {}).unwrap();
    assert!(
        first.rows.values().any(|r| r.attention() == Some(Attention::CodexCli)),
        "the copy that could not leave the resume list is reported: {:?}",
        first.errors
    );
    available.set(true);
    let second = scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), None, &metadata, |_| {}).unwrap();
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(transcripts(&f.pair.codex.root), vec![newer.path.clone()]);
    assert!(f.hub().conversations.values().next().unwrap().aliases.iter().any(|a| a.id == older.id));
}

#[test]
fn a_copy_known_only_to_a_second_legacy_state_leaves_the_resume_list() {
    let f = Fixture::new(Tool::Claude);
    let now = SystemTime::now();
    let first_copy = legacy_copy(&f, "pair-copy", &messages(), now - Duration::from_secs(600));
    let second_copy = legacy_copy(&f, "workspace-copy", &messages(), now - Duration::from_secs(300));
    let dir = f.config.join("conversation-sync");
    let legacy = |copy: &Source| {
        let mut state = State::default();
        state.entries.insert(key_of(&f.source), legacy_entry(&f.source, false, vec![as_continuation(copy)]));
        state.entries.insert(key_of(copy), legacy_entry(copy, true, vec![]));
        state.lineage.insert(key_of(copy), key_of(&f.source));
        serde_json::to_vec(&state).unwrap()
    };
    let (workspace, pair) = (dir.join("workspace-state.json"), dir.join("pair-state.json"));
    fs::write(&workspace, legacy(&second_copy)).unwrap();
    fs::write(&pair, legacy(&first_copy)).unwrap();
    let mut store = hub::Hub::open(&f.config).unwrap();
    migrate::run(&mut store, &[workspace, pair], &f.gate).unwrap();
    store.save().unwrap();
    assert!(f.scan().errors.is_empty());
    assert_eq!(transcripts(&f.pair.codex.root).len(), 1, "one Codex session remains for the conversation");
    let state = f.hub();
    let record = state.conversations.values().next().unwrap();
    for copy in [&first_copy, &second_copy] {
        assert!(
            record.projections.iter().any(|p| p.source.id == copy.id) || record.aliases.iter().any(|a| a.id == copy.id),
            "{} belongs to the conversation",
            copy.id
        );
    }
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    files
}

#[test]
fn a_scan_interrupted_before_its_state_save_leaves_a_readable_hub_without_duplicates() {
    let f = Fixture::new(Tool::Claude);
    assert!(f.scan().errors.is_empty());
    let saved = snapshot(&f.config.join("conversation-sync/hub"));
    let rewound = vec![messages()[0].clone(), text("assistant", "a different answer"), text("user", "next"), text("assistant", "next answer")];
    write_history(&f.source, &rewound, SystemTime::now() + Duration::from_secs(40));
    assert!(f.scan().errors.is_empty());
    // Every file written by that scan stays; the state and the files it replaced are as before.
    for (path, bytes) in &saved {
        fs::write(path, bytes).unwrap();
    }
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let state = f.hub();
    let (cid, record) = state.conversations.iter().next().unwrap();
    assert_eq!(hub::Hub::open(&f.config).unwrap().events(cid).unwrap(), rewound);
    assert!(record.projections.iter().all(|p| !p.removed), "no namespace is given up");
    let codex = transcripts(&f.pair.codex.root);
    assert_eq!(codex.len(), 1, "one Codex session: {codex:?}");
    assert!(record.projections.iter().any(|p| p.source.path == codex[0]));
    assert_eq!(read_messages(&f.projection_in(&f.pair.codex.root)).unwrap(), rewound);
    assert_eq!(f.scan().published, 0);
}

#[test]
fn appended_turns_and_later_writes_through_an_open_handle_both_remain() {
    let marker = b"{\"type\":\"kiln-test-marker\"}\n";
    for tool in Tool::ALL {
        let f = Fixture::new(tool);
        assert!(f.scan().errors.is_empty());
        let peer = f.peer();
        let mut writer = fs::OpenOptions::new().append(true).open(&f.source.path).unwrap();
        let mut extended = messages();
        extended.extend(turns(&["Continue from the other CLI.", "Continued."]));
        write_history(&peer, &extended, SystemTime::now() + Duration::from_secs(30));
        let status = f.scan();
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert_eq!(read_messages(&f.source).unwrap(), extended, "{tool:?}: checkpointed append");
        writer.write_all(marker).unwrap();
        assert!(fs::read(&f.source.path).unwrap().ends_with(marker), "{tool:?}: checkpointed append");
        let mut writer = fs::OpenOptions::new().append(true).open(&peer.path).unwrap();
        let mut more = extended.clone();
        more.extend(turns(&["A third question.", "A third answer."]));
        assert!(canonical::update(&peer, &more, "", "2026-10-15T00:00:00.000Z").unwrap());
        assert_eq!(read_messages(&peer).unwrap(), more, "{tool:?}: full update");
        writer.write_all(marker).unwrap();
        assert!(fs::read(&peer.path).unwrap().ends_with(marker), "{tool:?}: full update");
    }
}

#[test]
fn a_continuation_that_kept_only_displayed_tool_text_reaches_a_projection_without_checkpoint() {
    let raw = "[Tool result: call_fixture]\nScript completed\nOutput:\n{\"exit_code\":0,\"output\":\"done\"}";
    let mut history = messages();
    history.insert(1, Message { role: "assistant".into(), text: raw.into(), images: vec![] });
    let f = Fixture::new(Tool::Claude);
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(60));
    assert!(f.scan().errors.is_empty());
    let peer = f.peer();
    let mut data = rows(&fs::read(&peer.path).unwrap());
    for row in &mut data {
        row.as_object_mut().unwrap().remove("kiln_sync");
    }
    fs::write(&peer.path, data.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
    let shown = read_messages(&peer).unwrap();
    assert_ne!(shown, history, "the native rewrite kept only the displayed tool text");
    let mut continued = shown.clone();
    continued.extend(turns(&["Continue after the tool.", "Continued after the tool."]));
    assert!(canonical::update(&peer, &continued, "", "2026-10-15T00:00:00.000Z").unwrap());
    set_modified(&peer.path, SystemTime::now() - Duration::from_secs(30));
    let mut store = hub::Hub::open(&f.config).unwrap();
    for record in store.state.conversations.values_mut() {
        for p in &mut record.projections {
            if p.source.path == f.source.path {
                p.checkpoint = None;
            }
        }
    }
    store.save().unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let mut expected = history.clone();
    expected.extend(turns(&["Continue after the tool.", "Continued after the tool."]));
    assert_eq!(read_messages(&f.source).unwrap(), expected);
}

#[test]
fn a_scan_level_warning_offers_its_action_and_can_be_ignored() {
    use egui_kittest::{Harness, kittest::Queryable};
    let f = Fixture::new(Tool::Claude);
    let root = PathBuf::from(&f.source.cwd);
    let mut manager = Manager::default();
    manager.workspaces = vec![root.clone()];
    manager.error = Some("No space left on device (os error 28)".into());
    manager.revision += 1;
    assert!(matches!(manager.badge_state(&root), Badge::Attention));
    let mut initialized = false;
    let mut h = Harness::builder().with_size([560.0, 500.0]).build_ui_state(
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
    assert!(h.query_by_label("다시 시도").is_some());
    h.get_by_label("무시").click();
    h.run_steps(3);
    assert!(!matches!(h.state().badge_state(&root), Badge::Attention), "the ignored scan warning leaves the badge");
    assert!(h.query_by_label("다시 표시").is_some());
}

#[test]
fn codex_checkpoints_keep_only_the_session_fields_their_parser_reads() {
    let f = Fixture::new(Tool::Codex);
    rewrite_rows(&f.source.path, |row| {
        if row["type"] == "session_meta" {
            row["payload"]["base_instructions"] = json!({"text": "x".repeat(20_000)});
        }
    });
    set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(60));
    assert!(f.scan().errors.is_empty());
    assert!(f.live().iter().all(|p| p.checkpoint.is_some()), "every projection has a checkpoint");
    let state = fs::read(f.config.join("conversation-sync/hub/state.json")).unwrap();
    assert!(state.len() < 10_000, "hub state holds {} bytes", state.len());
    let mut extended = messages();
    extended.extend(turns(&["Continue in Codex.", "Continued in Codex."]));
    assert!(canonical::update(&f.source, &extended, "", "2026-10-15T00:00:00.000Z").unwrap());
    set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(30));
    assert!(f.scan().errors.is_empty());
    assert_eq!(read_messages(&f.peer()).unwrap(), extended);
}

#[test]
fn codex_names_are_read_again_only_after_its_state_database_changes() {
    let f = Fixture::new(Tool::Codex);
    let reads = std::cell::Cell::new(0);
    let name = std::cell::RefCell::new(String::from("Native database title"));
    let run = || {
        scan_with_metadata(
            &f.config,
            &f.pair,
            &f.gate,
            &mut Cache::new(),
            None,
            |source, request| {
                if matches!(request, Metadata::Read) {
                    reads.set(reads.get() + 1);
                    if source.path == f.source.path {
                        return Ok(Some(name.borrow().clone()));
                    }
                }
                Ok(None)
            },
            |_| {},
        )
        .unwrap()
    };
    let database = f.pair.codex.root.join("state_5.sqlite");
    fs::write(&database, b"first").unwrap();
    assert!(run().errors.is_empty());
    reads.set(0);
    assert!(run().errors.is_empty());
    assert_eq!(reads.get(), 0, "an unchanged database is not asked again");
    *name.borrow_mut() = "Renamed in native database".into();
    fs::write(&database, b"renamed").unwrap();
    assert!(run().errors.is_empty());
    assert!(reads.get() > 0);
    assert_eq!(f.hub().conversations.values().next().unwrap().title, "Renamed in native database");
}

const CLAUDE_CALL: &str = "[Tool call: Bash]\n{\"command\":\"ls -la\",\"description\":\"List files\"}";
const CLAUDE_RESULT: &str = "[Tool result: toolu_01FixtureToolUseId0000]\nfirst\nsecond";
const CODEX_CALL: &str = "[Tool call: exec]\ntext(await tools.exec_command({cmd:\"ls -la\",\"max_output_tokens\":6000,workdir:\"/w\"}));\n";
const CODEX_RESULT: &str = "[Tool result: call_FixtureCallId000000000000000000]\nScript completed\nWall time 0.1 seconds\nOutput:\n{\"chunk_id\":\"1a\",\"wall_time_seconds\":0.1,\"exit_code\":0,\"original_token_count\":2,\"output\":\"first\\nsecond\"}";

fn shell_turn(call: &str, result: &str) -> Vec<Message> {
    vec![text("user", "List the files."), text("assistant", call), text("assistant", result), text("assistant", "Listed.")]
}

#[test]
fn shell_records_are_recognised_from_both_clis() {
    let claude = tools::shell(CLAUDE_CALL, CLAUDE_RESULT).expect("Claude Bash");
    assert_eq!((claude.command.as_str(), claude.cwd.as_deref(), claude.exit_code, claude.output.as_str()), ("ls -la", None, 0, "first\nsecond"));
    assert_eq!(claude.id, "toolu_01FixtureToolUseId0000");
    let failed = tools::shell(CLAUDE_CALL, "[Tool result: toolu_01FixtureToolUseId0000]\nExit code 2\nboom").unwrap();
    assert_eq!((failed.exit_code, failed.output.as_str()), (2, "boom"));
    let codex = tools::shell(CODEX_CALL, CODEX_RESULT).expect("Codex exec_command");
    assert_eq!((codex.command.as_str(), codex.cwd.as_deref(), codex.exit_code, codex.output.as_str()), ("ls -la", Some("/w"), 0, "first\nsecond"));
    assert_eq!(codex.id, "call_FixtureCallId000000000000000000");
    let two = "[Tool call: exec]\ntext(await tools.exec_command({cmd:\"ls\"}));text(await tools.exec_command({cmd:\"pwd\"}));";
    assert!(tools::shell(two, CODEX_RESULT).is_none(), "a program with two calls stays text");
    let background = "[Tool call: Bash]\n{\"command\":\"sleep 9\",\"run_in_background\":true}";
    assert!(tools::shell(background, CLAUDE_RESULT).is_none(), "a background shell stays text");
    assert!(tools::shell(CODEX_CALL, "[Tool result: call_x]\nScript completed\nOutput:\nnot json").is_none());
    assert!(tools::shell("[Tool call: Read]\n{\"file_path\":\"/a\"}", CLAUDE_RESULT).is_none());
}

#[test]
fn shell_pairs_render_as_native_claude_tool_records() {
    for (call, result) in [(CODEX_CALL, CODEX_RESULT), (CLAUDE_CALL, CLAUDE_RESULT)] {
        let history = shell_turn(call, result);
        let bytes = format::render_titled(Tool::Claude, &stable_id("native-claude"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap();
        let data = rows(&bytes);
        let uses: Vec<&serde_json::Value> = data.iter().flat_map(|r| r["message"]["content"].as_array().into_iter().flatten()).filter(|b| b["type"] == "tool_use").collect();
        assert_eq!(uses.len(), 1, "{call}");
        assert_eq!((uses[0]["name"].as_str(), uses[0]["input"]["command"].as_str()), (Some("Bash"), Some("ls -la")));
        let results: Vec<&serde_json::Value> = data.iter().flat_map(|r| r["message"]["content"].as_array().into_iter().flatten()).filter(|b| b["type"] == "tool_result").collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool_use_id"], uses[0]["id"]);
        assert_eq!((results[0]["content"].as_str(), results[0]["is_error"].as_bool()), (Some("first\nsecond"), Some(false)));
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history, "{call}");
    }
    let bytes = format::render_titled(Tool::Claude, &stable_id("native-claude"), "/w", "2026-10-15T00:00:00Z", &shell_turn(CLAUDE_CALL, CLAUDE_RESULT), "Shell").unwrap();
    let data = rows(&bytes);
    let call = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_use").unwrap();
    assert_eq!(call["message"]["content"][0]["input"], json!({"command": "ls -la", "description": "List files"}), "a Claude call keeps its input");
    assert_eq!(call["message"]["content"][0]["id"], "toolu_01FixtureToolUseId0000");
    assert!(data.iter().all(|r| r.get("kiln_sync").is_none_or(|m| m["revision"] != 5)), "a Claude call needs no recorded original");
    let failing = shell_turn(CODEX_CALL, &CODEX_RESULT.replace("\"exit_code\":0", "\"exit_code\":2").replace("Script completed", "Script failed"));
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("native-claude"), "/w", "2026-10-15T00:00:00Z", &failing, "Shell").unwrap());
    let result = data.iter().flat_map(|r| r["message"]["content"].as_array().into_iter().flatten()).find(|b| b["type"] == "tool_result").unwrap();
    assert_eq!((result["content"].as_str(), result["is_error"].as_bool()), (Some("Exit code 2\nfirst\nsecond"), Some(true)));
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), failing);
}

#[test]
fn shell_pairs_render_as_native_codex_items() {
    for (call, result) in [(CLAUDE_CALL, CLAUDE_RESULT), (CODEX_CALL, CODEX_RESULT)] {
        let history = shell_turn(call, result);
        let bytes = format::render_titled(Tool::Codex, &stable_id("native-codex"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap();
        let data = rows(&bytes);
        let items: Vec<&serde_json::Value> = data.iter().filter(|r| r["payload"]["type"] == "item_completed").map(|r| &r["payload"]["item"]).collect();
        let commands: Vec<&&serde_json::Value> = items.iter().filter(|i| i["type"] == "CommandExecution").collect();
        assert_eq!(commands.len(), 1, "{call}");
        assert_eq!(commands[0]["command"], json!(["/bin/zsh", "-lc", "ls -la"]));
        assert_eq!(commands[0]["cwd"], "file:///w", "Codex reads the working directory as a file URL");
        assert_eq!((commands[0]["aggregated_output"].as_str(), commands[0]["exit_code"].as_i64(), commands[0]["status"].as_str()), (Some("first\nsecond"), Some(0), Some("completed")));
        assert_eq!(items.iter().filter(|i| i["type"] == "AgentMessage").count(), 1, "only the final answer is an agent message");
        assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history, "{call}");
    }
    let data = rows(&format::render_titled(Tool::Codex, &stable_id("native-codex"), "/w", "2026-10-15T00:00:00Z", &shell_turn(CODEX_CALL, CODEX_RESULT), "Shell").unwrap());
    let call = data.iter().find(|r| r["payload"]["type"] == "custom_tool_call").expect("a Codex call stays a native tool call");
    assert_eq!((call["payload"]["name"].as_str(), call["payload"]["call_id"].as_str()), (Some("exec"), Some("call_FixtureCallId000000000000000000")));
    let output = data.iter().find(|r| r["payload"]["type"] == "custom_tool_call_output").unwrap();
    assert_eq!(output["payload"]["output"].as_array().unwrap().len(), 2);
    let data = rows(&format::render_titled(Tool::Codex, &stable_id("native-codex"), "/w", "2026-10-15T00:00:00Z", &shell_turn(CLAUDE_CALL, CLAUDE_RESULT), "Shell").unwrap());
    assert!(data.iter().all(|r| r["payload"]["type"] != "custom_tool_call"), "a Claude call stays text in Codex's model context");
}

#[test]
fn a_shell_pair_that_ends_its_batch_stays_text() {
    let mut history = shell_turn(CLAUDE_CALL, CLAUDE_RESULT);
    history.pop();
    for tool in Tool::ALL {
        let data = rows(&format::render_titled(tool, &stable_id("ends"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap());
        assert!(data.iter().all(|r| r["message"]["content"][0]["type"] != "tool_use" && r["payload"]["item"]["type"] != "CommandExecution"), "{tool:?}");
    }
}

#[test]
fn shell_turns_reach_the_other_cli_as_native_records_without_echo() {
    for (tool, call, result) in [(Tool::Claude, CLAUDE_CALL, CLAUDE_RESULT), (Tool::Codex, CODEX_CALL, CODEX_RESULT)] {
        let f = Fixture::new(tool);
        assert!(f.scan().errors.is_empty());
        let mut extended = messages();
        extended.extend(shell_turn(call, result));
        assert!(canonical::update(&f.source, &extended, "", "2026-10-15T00:00:00.000Z").unwrap());
        set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(30));
        let status = f.scan();
        assert!(status.errors.is_empty(), "{tool:?}: {:?}", status.errors);
        assert_eq!(status.published, 1);
        let peer = f.peer();
        assert_eq!(read_messages(&peer).unwrap(), extended, "{tool:?}");
        let data = rows(&fs::read(&peer.path).unwrap());
        let native = if peer.tool == Tool::Claude {
            data.iter().any(|r| r["message"]["content"][0]["type"] == "tool_use" && r["message"]["content"][0]["name"] == "Bash")
        } else {
            data.iter().any(|r| r["payload"]["item"]["type"] == "CommandExecution")
        };
        assert!(native, "{tool:?}: the shell turn is a native record in the other CLI");
        assert_eq!(f.scan().published, 0, "{tool:?}: no echo");
        assert_eq!(read_messages(&f.source).unwrap(), extended);
    }
}
