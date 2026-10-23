use kiln_common::paths::Canonical;
use super::*;
use format::Message;
use serde_json::json;

/// Creates a symbolic link to a file on Unix and Windows.
pub(super) fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
}

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
    for tool in PROJECTED {
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
    data[1]["cwd"] = json!(std::env::temp_dir().join("changed-directory"));
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
    for tool in PROJECTED {
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
    for tool in PROJECTED {
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
        .save(std::env::temp_dir().join("kiln-0125-sync-dialog.png"))
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
        let root = temp.path().canonical().unwrap();
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
    for tool in PROJECTED {
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
    for tool in PROJECTED {
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
    for tool in PROJECTED {
        let f = Fixture::new(tool);
        let original = fs::read(&f.source.path).unwrap();
        let mut messages = read_messages(&f.source).unwrap();
        messages.push(format::Message { role:"assistant".into(), text:"An additional recorded result".into(), images:vec![] });
        canonical::update(&f.source, &messages, "Shared title", "2026-10-14T06:00:00Z", &Default::default()).unwrap();
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
    canonical::update(&f.source,&ms,"Shared title","2026-10-14T07:00:00Z", &Default::default()).unwrap();
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
    canonical::update(&f.source, &continued, "Compressed native title", "2026-10-14T07:00:00Z", &Default::default()).unwrap();
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
    let dir = dir.canonical().unwrap();
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
    for tool in PROJECTED {
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
            assert_eq!(saved.root, pair.endpoint(tool).unwrap().root);
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
                canonical::update(&source, &continued, &fixture_title, &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true), &Default::default()).unwrap();
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
        .save(std::env::temp_dir().join("kiln-sync-progress-complete.png"))
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
        .save(std::env::temp_dir().join("kiln-sync-progress-failed.png"))
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
        .save(std::env::temp_dir().join("kiln-sync-progress-diverged.png"))
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
    let sources: Vec<Source> = PROJECTED
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
    discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(),
        workspaces: vec![PathBuf::from(&f.source.cwd)],
        roots: PROJECTED
            .into_iter()
            .map(|t| (t, f.pair.endpoint(t).unwrap().root.clone()))
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
    for tool in PROJECTED {
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
    native_turn(&external.path);
    let changed = workspace_scan(&f, &scope);
    assert_eq!(changed.published, 1);
    assert!(changed.errors.is_empty(), "{:?}", changed.errors);
    assert_eq!(changed.rows.len(), 1, "the continued import is the same conversation");
    assert_eq!(transcripts(&f.pair.codex.root), vec![external.path.clone()], "the continued import is the Codex projection");
    assert_eq!(read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(), continued);
}

/// Appends the turn context record the Codex CLI writes for each model turn.
fn native_turn(path: &Path) {
    let row = json!({"timestamp":"2026-10-14T00:00:00Z","type":"turn_context","payload":{"cwd":"/tmp","model":"gpt-test"}});
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{row}").unwrap();
}
/// Publishes a Codex Desktop import of the Claude projection `copy` holding `history`, with its ledger record.
fn desktop_import(f: &Fixture, copy: &Source, name: &str, history: &[Message]) -> Source {
    let import = destination(&f.pair, copy, &stable_id(name), chrono::Utc::now()).unwrap();
    publish(&import.path, &format::render(Tool::Codex, &import.id, &import.cwd, "2026-10-14T00:00:00Z", history).unwrap()).unwrap();
    fs::write(
        f.pair.codex.root.join("external_agent_session_imports.json"),
        serde_json::to_vec(&json!({"records":[{"source_path":copy.path,"imported_thread_id":import.id}]})).unwrap(),
    )
    .unwrap();
    import
}
fn turn(history: &mut Vec<Message>, question: &str, answer: &str) {
    history.push(Message { role: "user".into(), text: question.into(), images: vec![] });
    history.push(Message { role: "assistant".into(), text: answer.into(), images: vec![] });
}

#[test]
fn a_desktop_import_without_native_turns_leaves_without_displacing_the_original() {
    let f = Fixture::new(Tool::Codex);
    native_turn(&f.source.path);
    let scope = workspace_scope(&f);
    assert_eq!(workspace_scan(&f, &scope).published, 1);
    let copy = f.projection_in(&f.pair.claude.root);
    let mut flattened = messages();
    flattened[1].text = "[external_agent_tool_call: Bash]\ncommand: true".into();
    let import = desktop_import(&f, &copy, "flattened-desktop-import", &flattened);
    let joined = workspace_scan(&f, &scope);
    assert!(joined.errors.is_empty(), "{:?}", joined.errors);
    assert_eq!(f.projection_in(&f.pair.codex.root).id, f.source.id, "the original stays the Codex projection");
    assert!(!import.path.exists(), "the import leaves the resume list");
    let record = f.hub().conversations.into_values().next().unwrap();
    assert!(record.branches.is_empty(), "a copy without native turns is not a branch");
    assert_eq!(hub::Hub::open(&f.config).unwrap().events(&hub::conversation_id(&hub::NativeRef::of(&f.source))).unwrap(), messages());
    let mut continued = messages();
    turn(&mut continued, "Original turn", "Original answer");
    write_history(&f.source, &continued, SystemTime::now());
    native_turn(&f.source.path);
    let changed = workspace_scan(&f, &scope);
    assert!(changed.errors.is_empty(), "{:?}", changed.errors);
    assert_eq!(read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(), continued);
}

#[test]
fn a_displaced_original_continued_after_the_canonical_history_takes_its_namespace_back() {
    let f = Fixture::new(Tool::Codex);
    native_turn(&f.source.path);
    set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(1200));
    let scope = workspace_scope(&f);
    let original = f.source.clone();
    // The original has a descendant history, so its native archive is refused.
    let scan = || {
        scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), Some(&scope), |source, request| {
            if matches!(request, Metadata::Archive) {
                ensure!(source.id != original.id, "Linked snapshot has descendant histories; preserved for review");
                return archiving_into(source);
            }
            Ok(None)
        }, |_| {})
        .unwrap()
    };
    assert_eq!(scan().published, 1);
    let copy = f.projection_in(&f.pair.claude.root);
    set_modified(&copy.path, SystemTime::now() - Duration::from_secs(1100));
    let mut imported = messages();
    imported[1].text = "[external_agent_tool_call: Bash]\ncommand: true".into();
    turn(&mut imported, "Import turn", "Import answer");
    let import = desktop_import(&f, &copy, "continued-desktop-import", &imported);
    native_turn(&import.path);
    set_modified(&import.path, SystemTime::now() - Duration::from_secs(600));
    scan();
    assert_eq!(f.projection_in(&f.pair.codex.root).id, import.id, "a continued import becomes the Codex projection");
    assert!(f.hub().conversations.values().any(|r| r.pending.iter().any(|s| s.id == f.source.id)));
    let mut continued = messages();
    turn(&mut continued, "Original turn", "Original answer");
    write_history(&f.source, &continued, SystemTime::now());
    native_turn(&f.source.path);
    for _ in 0..3 {
        scan();
    }
    assert_eq!(f.projection_in(&f.pair.codex.root).id, f.source.id, "the newer original is the Codex projection again");
    assert!(f.source.path.exists());
    assert_eq!(read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(), continued);
    assert!(!import.path.exists(), "the older import leaves the resume list");
    let record = f.hub().conversations.into_values().next().unwrap();
    assert!(record.pending.is_empty(), "{:?}", record.pending);
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
    for tool in PROJECTED {
        private_dir(&default_pair.endpoint(tool).unwrap().root).unwrap();
        scope
            .roots
            .push((tool, default_pair.endpoint(tool).unwrap().root.clone()));
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
    scope.workspaces = vec![project.canonical().unwrap()];
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
fn ongoing_sync_is_latest_twenty_per_workspace_including_native_resumes() {
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
        fs::OpenOptions::new()
            .write(true)
            .open(&source.path)
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
    fs::OpenOptions::new()
        .write(true)
        .open(&sources[0].path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now()))
        .unwrap();
    let changed = workspace_scan(&f, &scope);
    assert_eq!(changed.published, 1);
    assert_eq!(changed.rows.len(), 20);
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
    let scope = discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(),
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
                for tool in PROJECTED.into_iter().filter(|_| !messages.is_empty()) {
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
                    && w.root.canonical().ok() == root.canonical().ok()),
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
    symlink_file(&f.source.path, &linked).unwrap();
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
        .canonical()
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
    let scope = discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(),
        workspaces: vec![dir.clone()],
        roots: PROJECTED
            .into_iter()
            .map(|t| (t, pair.endpoint(t).unwrap().root.clone()))
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
        .save(std::env::temp_dir().join("kiln-workspace-sync-complete.png"))
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
        .save(std::env::temp_dir().join("kiln-workspace-sync-empty.png"))
        .unwrap();
}

#[test]
#[ignore]
fn verify_discovered_workspace_cli_continuations() {
    let dir = PathBuf::from(
        std::env::var_os("KILN_SYNC_FIXTURE_DIR").expect("explicit fixture directory"),
    )
    .canonical()
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
    let scope = discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(),
        workspaces: vec![dir.clone()],
        roots: PROJECTED
            .into_iter()
            .map(|t| (t, pair.endpoint(t).unwrap().root.clone()))
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
    let scope = discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(),
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
    for tool in PROJECTED {
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
        assert!(shown.contains("첫째 줄\n   둘째 줄 <tag> ``` 코드"));
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
    for tool in PROJECTED {
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
                Ok(discovery::Scope { paused: Vec::new(), limits: BTreeMap::new(), workspaces, roots: roots.clone() })
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
        self.send(&worker::Request::Scan { id, pair: f.pair.clone(), workspaces: vec![PathBuf::from(&f.source.cwd)], limits: BTreeMap::new(), paused: Vec::new(), quiet_ms: 0, expedite: false, reset: false });
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
        synced: 0, stamp: None, origin: true, removed: false, title: String::new(), checkpoint: None, database: None, rendered: 0,
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
    for tool in PROJECTED {
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
    for tool in PROJECTED {
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
    h.render().unwrap().save(std::env::temp_dir().join("kiln-sync-attention.png")).unwrap();
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
    for tool in PROJECTED {
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
            canonical::update(&f.source, &expected, "Fixture title", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap();
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
    for tool in PROJECTED {
        let f = Fixture::new(tool);
        let (_, cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
        let mut cp = cp.unwrap();
        let mut expected = messages();
        for round in 0..6 {
            let added = turns(&[&format!("appended question {round}"), &format!("appended answer {round}")]);
            expected.extend(added.clone());
            let before = fs::read(&f.source.path).unwrap();
            cp = canonical::append_since(&f.source, &cp, &added, "Fixture title", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap().expect("a checkpointed append");
            assert!(fs::read(&f.source.path).unwrap().starts_with(&before), "existing bytes are kept");
            let (full, full_cp) = checkpoint::read(&f.source, &mut String::new()).unwrap();
            assert_eq!(full, expected, "{tool:?} round {round}");
            assert_eq!(Some(&cp), full_cp.as_ref());
        }
    }
}

#[test]
fn prefix_change_falls_back_to_a_full_parse() {
    for tool in PROJECTED {
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

/// Hub round trip with installed CLIs: `export` writes one original per CLI, each with
/// tool call turns, into `claude/` and `codex/` under `KILN_HUB_FIXTURE_DIR` and projects
/// them; `sync` reads the
/// CLIs' continuations back and prints the last messages of every projection.
#[test]
#[ignore]
fn hub_cli_round_trip() {
    let dir = PathBuf::from(std::env::var_os("KILN_HUB_FIXTURE_DIR").expect("explicit fixture directory"))
        .canonical()
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
        for tool in PROJECTED {
            let id = stable_id(&format!("hub-fixture-{}", tool.key()));
            let path = native_path(tool, &pair.endpoint(tool).unwrap().root, &work.to_string_lossy(), &id, chrono::Utc::now()).unwrap();
            let source = Source { tool, root: pair.endpoint(tool).unwrap().root.clone(), path, cwd: work.to_string_lossy().into_owned(), id };
            let mut history = turns(&[&format!("Remember the token started-in-{}.", tool.key()), "Remembered."]);
            match tool {
                Tool::Claude => {
                    history.extend(shell_turn(CLAUDE_CALL, CLAUDE_RESULT));
                    history.extend(shell_turn(READ_CALL, READ_RESULT));
                    history.extend(shell_turn("[Tool call: mcp__x__y]\n{\"a\":1}", "[Tool result: toolu_01McpResultId0000000000]\nok"));
                }
                Tool::Codex => {
                    history.extend(shell_turn(&codex_call_in(&source.cwd), CODEX_RESULT));
                    history.extend(shell_turn(JS_CALL, JS_RESULT));
                    history.extend(shell_turn(CODEX_TWO_CALL, CODEX_TWO_RESULT));
                }
                Tool::Antigravity => {}
            }
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
    canonical::update(&f.source, &older, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap();
    canonical::update(&peer, &newer, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap();
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
    for tool in PROJECTED {
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
        assert!(canonical::update(&peer, &more, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap());
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
    assert!(canonical::update(&peer, &continued, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap());
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
fn hub_state_stays_small_with_large_codex_session_instructions() {
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
    assert!(canonical::update(&f.source, &extended, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap());
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

/// `CODEX_CALL` run in `dir`.
fn codex_call_in(dir: &str) -> String {
    CODEX_CALL.replace("workdir:\"/w\"", &format!("workdir:{}", json!(dir)))
}

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
        assert_eq!(commands[0]["cwd"], "file:///w", "the working directory is a file URL");
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
    for tool in PROJECTED {
        let data = rows(&format::render_titled(tool, &stable_id("ends"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap());
        assert!(data.iter().all(|r| r["message"]["content"][0]["type"] != "tool_use" && r["payload"]["item"]["type"] != "CommandExecution"), "{tool:?}");
    }
}

#[test]
fn shell_turns_reach_the_other_cli_as_native_records_without_echo() {
    for tool in PROJECTED {
        let f = Fixture::new(tool);
        assert!(f.scan().errors.is_empty());
        let mut extended = messages();
        extended.extend(match tool {
            Tool::Claude => shell_turn(CLAUDE_CALL, CLAUDE_RESULT),
            Tool::Codex => shell_turn(&codex_call_in(&f.source.cwd), CODEX_RESULT),
            Tool::Antigravity => unreachable!(),
        });
        assert!(canonical::update(&f.source, &extended, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap());
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

#[test]
fn parallel_tool_calls_stay_text() {
    let read_call = "[Tool call: Read]\n{\"file_path\":\"/w/a\"}";
    let read_result = "[Tool result: toolu_01ReadResultId000000000]\nAAA";
    let history = vec![
        text("user", "Do two things."),
        text("assistant", read_call),
        text("assistant", CLAUDE_CALL),
        text("assistant", read_result),
        text("assistant", CLAUDE_RESULT),
        text("assistant", "Done."),
    ];
    for tool in PROJECTED {
        let data = rows(&format::render_titled(tool, &stable_id("parallel"), "/w", "2026-10-15T00:00:00Z", &history, "Parallel").unwrap());
        assert!(
            data.iter().all(|r| r["message"]["content"][0]["type"] != "tool_use" && r["payload"]["item"]["type"] != "CommandExecution"),
            "{tool:?}: a call next to another call is not paired with the next result"
        );
        assert_eq!(format::parse(tool, &data).unwrap(), history);
    }
}

#[test]
fn a_codex_command_run_in_another_directory_changes_directory_in_claude() {
    let sub = if cfg!(windows) { "cd '/w\\sub' && ls -la" } else { "cd /w/sub && ls -la" };
    for (dir, shown) in [("/elsewhere", "cd /elsewhere && ls -la"), ("/a b/it's", "cd '/a b/it'\\''s' && ls -la"), ("sub", sub), (".", "ls -la")] {
        let call = CODEX_CALL.replace("workdir:\"/w\"", &format!("workdir:{}", json!(dir)));
        let history = shell_turn(&call, CODEX_RESULT);
        let data = rows(&format::render_titled(Tool::Claude, &stable_id("workdir"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap());
        let call = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_use").expect("a Bash record");
        assert_eq!(call["message"]["content"][0]["input"]["command"], shown);
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
        let bash = format!("[Tool call: Bash]\n{}", json!({"command": shown}));
        assert!(hub::same(&text("assistant", &bash), &history[1]), "{shown}: the same command in either form");
    }
    let call = CODEX_CALL.replace("workdir:\"/w\"", "workdir:\"/elsewhere\"");
    let history = shell_turn(&call, CODEX_RESULT);
    let data = rows(&format::render_titled(Tool::Codex, &stable_id("workdir"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap());
    let command = data.iter().find(|r| r["payload"]["item"]["type"] == "CommandExecution").expect("command cell");
    assert_eq!(command["payload"]["item"]["cwd"], "file:///elsewhere");
}

#[test]
fn a_native_shell_record_read_without_its_annotation_keeps_the_conversation_together() {
    let f = Fixture::new(Tool::Codex);
    let mut history = messages();
    history.extend(shell_turn(&codex_call_in(&f.source.cwd), CODEX_RESULT));
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let claude = f.peer();
    rewrite_rows(&claude.path, |row| {
        row.as_object_mut().unwrap().remove("kiln_sync");
    });
    let shown = read_messages(&claude).unwrap();
    assert_ne!(shown, history, "the Claude records read as Claude's own shell records");
    assert_eq!(hub::shared(&history, &shown), history.len());
    append_native_claude_turn(&claude, "Continue in Claude.", "Continued in Claude.");
    set_modified(&claude.path, SystemTime::now() - Duration::from_secs(60));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert!(f.source.path.exists(), "the Codex original stays in its resume list");
    let state = f.hub();
    let record = state.conversations.values().next().unwrap();
    assert!(record.branches.is_empty());
    let mut expected = history.clone();
    expected.extend(turns(&["Continue in Claude.", "Continued in Claude."]));
    assert_eq!(read_messages(&f.source).unwrap(), expected);
    // A later Codex turn reaches the Claude projection through a full update.
    let mut later = expected.clone();
    later.extend(turns(&["Back in Codex.", "Back."]));
    assert!(canonical::update(&f.source, &later, "", "2026-10-15T00:00:00.000Z", &Default::default()).unwrap());
    set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(30));
    let mut store = hub::Hub::open(&f.config).unwrap();
    for p in store.state.conversations.values_mut().flat_map(|r| r.projections.iter_mut()) {
        if p.source.path == claude.path {
            p.checkpoint = None;
        }
    }
    store.save().unwrap();
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(read_messages(&claude).unwrap().last().unwrap().text, "Back.");
}

#[test]
fn tool_records_read_like_the_cli_tool_display() {
    use portable::display;
    assert_eq!(display("[Tool call: Read]\n{\"file_path\":\"/w/a.rs\"}"), "Read(/w/a.rs)");
    assert_eq!(display(CLAUDE_CALL), "Bash(ls -la)\n```text\nList files\n```");
    assert_eq!(
        display("[Tool call: js]\n{\"code\":\"await tab.click();\",\"title\":\"Open the heading\"}"),
        "js(Open the heading)\n```text\nawait tab.click();\n```"
    );
    assert_eq!(
        display("[Tool call: Grep]\n{\"output_mode\":\"content\",\"path\":\"/w\",\"pattern\":\"fn main\"}"),
        "Grep(fn main)\n```text\noutput_mode: content\npath: /w\n```"
    );
    assert_eq!(
        display("[Tool call: exec]\ntext(await tools.write_stdin({session_id:1,chars:\"y\"}));\n"),
        "exec\n```text\ntext(await tools.write_stdin({session_id:1,chars:\"y\"}));\n```"
    );
    let long = format!("[Tool call: Agent]\n{}", json!({"description": "x".repeat(90), "prompt": "Check it"}));
    assert_eq!(display(&long), format!("Agent({}…)\n```text\ndescription: {}\nprompt: Check it\n```", "x".repeat(80), "x".repeat(90)));
    assert_eq!(
        display("[Tool result: call_b74e]\nWall time: 2.9 seconds\nOutput: heading clicked"),
        "```text\n⎿  Wall time: 2.9 seconds\n   Output: heading clicked\n```"
    );
    assert_eq!(display(CLAUDE_RESULT), "```text\n⎿  first\n   second\n```");
    assert_eq!(display(CODEX_RESULT), "```text\n⎿  first\n   second\n```");
    let failed = CODEX_RESULT.replace("\"exit_code\":0", "\"exit_code\":2");
    assert_eq!(display(&failed), "```text\n⎿  Exit code 2\n   first\n   second\n```");
    assert_eq!(display("[Tool result: toolu_x]\n"), "```text\n⎿  (No output)\n```");
    assert_eq!(display("[Tool result: toolu_x]\nhas ``` fence"), "````text\n⎿  has ``` fence\n````");
}

const READ_CALL: &str = "[Tool call: Read]\n{\"file_path\":\"/w/notes.md\"}";
const READ_RESULT: &str = "[Tool result: toolu_01ReadResultId000000000]\n     1\t# Notes\n     2\tfirst line\n";
const JS_CALL: &str = "[Tool call: js]\n{\"code\":\"await tab.click();\",\"title\":\"Open the heading\"}";
const JS_RESULT: &str = "[Tool result: call_JsResultId0000000000000000000]\nWall time: 2.9 seconds\nOutput: heading clicked";

#[test]
fn claude_built_in_tools_are_native_in_another_claude_namespace() {
    let history = shell_turn(READ_CALL, READ_RESULT);
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("builtin"), "/w", "2026-10-15T00:00:00Z", &history, "Read").unwrap());
    let call = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_use").expect("a Read tool use");
    assert_eq!(call["message"]["content"][0]["name"], "Read");
    assert_eq!(call["message"]["content"][0]["input"], json!({"file_path": "/w/notes.md"}));
    assert_eq!(call["message"]["content"][0]["id"], "toolu_01ReadResultId000000000");
    let result = data.iter().find(|r| r["message"]["content"][0]["type"] == "tool_result").unwrap();
    assert_eq!(result["message"]["content"][0]["content"], "     1\t# Notes\n     2\tfirst line\n");
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    for (call, result) in [(JS_CALL, JS_RESULT), ("[Tool call: mcp__x__y]\n{\"a\":1}", "[Tool result: toolu_01McpResultId0000000000]\nok")] {
        let history = shell_turn(call, result);
        let data = rows(&format::render_titled(Tool::Claude, &stable_id("builtin"), "/w", "2026-10-15T00:00:00Z", &history, "Other").unwrap());
        assert!(data.iter().all(|r| r["message"]["content"][0]["type"] != "tool_use"), "{call}: a tool Claude Code does not define stays text");
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    }
}

#[test]
fn other_tools_are_codex_tool_call_cells() {
    for (call, result, server, tool, arguments, output) in [
        (READ_CALL, READ_RESULT, "claude_code", "Read", json!({"file_path": "/w/notes.md"}), "     1\t# Notes\n     2\tfirst line\n"),
        (JS_CALL, JS_RESULT, "codex", "js", json!({"code": "await tab.click();", "title": "Open the heading"}), "Wall time: 2.9 seconds\nOutput: heading clicked"),
        ("[Tool call: apply_patch]\n*** Begin Patch\n*** End Patch", "[Tool result: call_PatchResultId00000000000000000]\nDone", "codex", "apply_patch", json!({"input": "*** Begin Patch\n*** End Patch"}), "Done"),
    ] {
        let history = shell_turn(call, result);
        let data = rows(&format::render_titled(Tool::Codex, &stable_id("cells"), "/w", "2026-10-15T00:00:00Z", &history, "Cells").unwrap());
        let items: Vec<&serde_json::Value> = data.iter().filter(|r| r["payload"]["type"] == "item_completed").map(|r| &r["payload"]["item"]).collect();
        let cell = items.iter().find(|i| i["type"] == "McpToolCall").unwrap_or_else(|| panic!("{call}: a tool call cell"));
        assert_eq!((cell["server"].as_str(), cell["tool"].as_str()), (Some(server), Some(tool)));
        assert_eq!(cell["arguments"], arguments);
        assert_eq!(cell["result"]["content"][0]["text"], output);
        assert_eq!(items.iter().filter(|i| i["type"] == "AgentMessage").count(), 1, "{call}: only the final answer is an agent message");
        assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history, "{call}");
    }
}

fn mark_rendered(f: &Fixture, path: &Path, rendered: u32) {
    let mut store = hub::Hub::open(&f.config).unwrap();
    for p in store.state.conversations.values_mut().flat_map(|r| r.projections.iter_mut()) {
        if p.source.path == path {
            p.rendered = rendered;
        }
    }
    store.save().unwrap();
}

/// Marks a projection as written by an earlier renderer and unchanged for twenty minutes.
fn age_projection(f: &Fixture, path: &Path) {
    set_modified(path, SystemTime::now() - Duration::from_secs(20 * 60));
    let meta = fs::metadata(path).unwrap();
    let mut store = hub::Hub::open(&f.config).unwrap();
    for p in store.state.conversations.values_mut().flat_map(|r| r.projections.iter_mut()) {
        if p.source.path == path {
            p.rendered = 0;
            p.stamp = Some((meta.len(), meta.modified().unwrap()));
        }
    }
    store.save().unwrap();
}

/// A history with tool records that exceeds Claude Code's context budget.
fn long_history(cwd: &str) -> Vec<Message> {
    let mut history = messages();
    history.extend(shell_turn(&codex_call_in(cwd), CODEX_RESULT));
    for k in 0..12 {
        history.extend(turns(&[&format!("Long request {k}"), &format!("{k} {}", "long answer line\n".repeat(2_000))]));
    }
    history
}

#[test]
fn oversized_projections_written_by_an_earlier_renderer_are_written_again() {
    let f = Fixture::new(Tool::Codex);
    let history = long_history(&f.source.cwd);
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let old = f.peer();
    age_projection(&f, &old.path);
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert!(!old.path.exists(), "the earlier projection leaves the resume list");
    let new = f.peer();
    assert_ne!(new.id, old.id);
    assert_eq!(read_messages(&new).unwrap(), history);
    assert!(rows(&fs::read(&new.path).unwrap()).iter().any(|r| r["subtype"] == "compact_boundary"));
    assert!(f.source.path.exists(), "the original is never written again");
    assert!(f.hub().conversations.values().next().unwrap().aliases.iter().any(|a| a.id == old.id));
    assert_eq!(f.scan().published, 0);
}

#[test]
fn claude_projections_with_tool_records_are_written_again() {
    let f = Fixture::new(Tool::Codex);
    let mut history = messages();
    history.extend(shell_turn(&codex_call_in(&f.source.cwd), CODEX_RESULT));
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let old = f.peer();
    age_projection(&f, &old.path);
    assert!(f.scan().errors.is_empty());
    assert!(!old.path.exists());
    let new = f.peer();
    assert_ne!(new.id, old.id);
    assert_eq!(read_messages(&new).unwrap(), history);
}

#[test]
fn projections_within_the_context_budget_without_tools_are_kept_by_a_new_renderer() {
    let f = Fixture::new(Tool::Codex);
    let history = messages();
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let old = f.peer();
    age_projection(&f, &old.path);
    assert!(f.scan().errors.is_empty());
    assert!(old.path.exists());
    assert_eq!(f.peer().id, old.id);
    assert!(f.live().iter().filter(|p| !p.origin).all(|p| p.rendered > 0));
}

const CODEX_TWO_CALL: &str = "[Tool call: exec]\ntext(await tools.exec_command({cmd:\"pwd\",\"max_output_tokens\":1000}));\ntext(await tools.exec_command({cmd:\"ls (all)\",\"max_output_tokens\":1000}));\n";
const CODEX_TWO_RESULT: &str = "[Tool result: call_TwoResultId00000000000000000000]\nScript completed\nWall time 0.2 seconds\nOutput:\n{\"chunk_id\":\"1\",\"wall_time_seconds\":0.1,\"exit_code\":0,\"original_token_count\":1,\"output\":\"/w\"}{\"chunk_id\":\"2\",\"wall_time_seconds\":0.1,\"exit_code\":1,\"original_token_count\":1,\"output\":\"a\\nb\"}";

#[test]
fn a_codex_program_with_several_commands_is_parallel_bash_in_claude() {
    let history = shell_turn(CODEX_TWO_CALL, CODEX_TWO_RESULT);
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("several"), "/w", "2026-10-15T00:00:00Z", &history, "Several").unwrap());
    assert_eq!(claude_commands(&data), [("pwd".to_owned(), "/w".to_owned(), false), ("ls (all)".to_owned(), "Exit code 1\na\nb".to_owned(), true)]);
    let uses: Vec<&serde_json::Value> = data.iter().filter(|r| r["message"]["content"][0]["type"] == "tool_use").collect();
    assert_eq!(uses.len(), 2, "one record per tool use");
    assert_eq!(uses[0]["message"]["id"], uses[1]["message"]["id"], "parallel uses share one message");
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    let data = rows(&format::render_titled(Tool::Codex, &stable_id("several"), "/w", "2026-10-15T00:00:00Z", &history, "Several").unwrap());
    let cells: Vec<&serde_json::Value> = data.iter().filter(|r| r["payload"]["item"]["type"] == "CommandExecution").map(|r| &r["payload"]["item"]).collect();
    assert_eq!(cells.iter().map(|c| c["command"][2].as_str().unwrap()).collect::<Vec<_>>(), ["pwd", "ls (all)"]);
    assert_eq!(data.iter().find(|r| r["payload"]["type"] == "custom_tool_call_output").unwrap()["payload"]["output"].as_array().unwrap().len(), 2);
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    let short = CODEX_TWO_RESULT.split("{\"chunk_id\":\"2\"").next().unwrap();
    let history = shell_turn(CODEX_TWO_CALL, short);
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("several"), "/w", "2026-10-15T00:00:00Z", &history, "Several").unwrap());
    assert!(data.iter().all(|r| r["message"]["content"][0]["type"] != "tool_use"), "a result for fewer commands stays text");
}

#[test]
fn a_recently_written_projection_is_not_written_again() {
    let f = Fixture::new(Tool::Codex);
    let history = long_history(&f.source.cwd);
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let projection = f.peer();
    mark_rendered(&f, &projection.path, 0);
    assert!(f.scan().errors.is_empty());
    assert!(projection.path.exists(), "a copy written within ten minutes is kept, also on a check without delays");
}

#[test]
fn a_projection_continued_in_its_cli_is_not_written_again() {
    let f = Fixture::new(Tool::Codex);
    let history = long_history(&f.source.cwd);
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let claude = f.peer();
    append_native_claude_turn(&claude, "Continue in Claude.", "Continued in Claude.");
    rewrite_rows(&claude.path, |row| {
        if row["type"] == "assistant" && row["message"]["content"][0]["text"] == "Continued in Claude." {
            row["message"]["model"] = json!("claude-opus-5-5");
        }
    });
    set_modified(&claude.path, SystemTime::now() - Duration::from_secs(60));
    assert!(f.scan().errors.is_empty());
    age_projection(&f, &claude.path);
    assert!(f.scan().errors.is_empty());
    assert!(claude.path.exists(), "a copy holding the CLI's own records is kept");
    assert_eq!(f.peer().id, claude.id);
    assert!(f.live().iter().filter(|p| !p.origin).all(|p| p.rendered > 0));
}

#[test]
fn a_codex_projection_is_kept_by_a_new_renderer() {
    let f = Fixture::new(Tool::Claude);
    let mut history = messages();
    history.extend(shell_turn(CLAUDE_CALL, CLAUDE_RESULT));
    for k in 0..12 {
        history.extend(turns(&[&format!("Long request {k}"), &format!("{k} {}", "long answer line\n".repeat(2_000))]));
    }
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    assert!(f.scan().errors.is_empty());
    let codex = f.peer();
    age_projection(&f, &codex.path);
    let metadata = |_: &Source, request: Metadata<'_>| -> Result<Option<String>> {
        ensure!(!matches!(request, Metadata::Archive), "Linked snapshot has descendant histories; preserved for review");
        Ok(None)
    };
    for _ in 0..2 {
        let status = scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), None, &metadata, |_| {}).unwrap();
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert!(status.settled.len() == 1, "the conversation stays settled");
    }
    assert!(codex.path.exists());
    assert!(f.live().iter().filter(|p| !p.origin).all(|p| p.rendered > 0));
}

#[test]
fn readable_tool_records_keep_every_input_field() {
    use portable::display;
    let path = format!("/w/{}/notes.md", "deep/".repeat(20));
    let shown = display(&format!("[Tool call: Read]\n{}", json!({"file_path": path})));
    assert!(shown.contains(&path), "{shown}");
    let shown = display("[Tool call: Bash]\n{\"command\":\"cat <<EOF > a\\nline two\\nEOF\",\"description\":\"Write a\",\"run_in_background\":true}");
    assert!(shown.contains("line two") && shown.contains("Write a") && shown.contains("run_in_background: true"), "{shown}");
    let a = text("assistant", "[Tool call: Bash]\n{\"command\":\"cat <<EOF\\none\\nEOF\"}");
    let b = text("assistant", "[Tool call: Bash]\n{\"command\":\"cat <<EOF\\ntwo\\nEOF\"}");
    assert!(!hub::same(&a, &b), "different commands are different messages");
}

#[test]
fn a_command_result_followed_by_whitespace_is_still_a_shell_record() {
    let history = shell_turn(CODEX_CALL, &format!("{CODEX_RESULT}\n"));
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("trailing"), "/w", "2026-10-15T00:00:00Z", &history, "Shell").unwrap());
    assert!(data.iter().any(|r| r["message"]["content"][0]["type"] == "tool_use"));
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
}

#[test]
fn hook_transcript_paths_match_canonical_roots() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let root = real.join("Claude Home");
    let work = dir.path().join("work");
    fs::create_dir_all(root.join("projects").join("work")).unwrap();
    fs::create_dir_all(&work).unwrap();
    let id = "0199a0a0-0000-7000-8000-000000000001";
    let canonical = root.canonical().unwrap().join("projects").join("work").join(format!("{id}.jsonl"));
    let mut forms = Vec::new();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, dir.path().join("alias")).unwrap();
        forms.push(dir.path().join("alias/Claude Home/projects/work").join(format!("{id}.jsonl")).to_string_lossy().into_owned());
    }
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy().into_owned();
        forms.extend([text.to_lowercase(), text.replace('\\', "/"), format!(r"\\?\{text}")]);
    }
    let record = super::super::agent_runtime::Record {
        tool: Tool::Claude,
        spec: kiln_proto::SpawnSpec {
            cwd: Some(work.to_string_lossy().into_owned()),
            env: vec![(env_key(Tool::Claude).into(), root.to_string_lossy().into_owned())],
            ..Default::default()
        },
        version: "fixture".into(),
        conversation: Some(id.into()),
        generation: "fixture".into(),
        confirmed: true,
        request_started: false,
    };
    for (index, raw) in forms.iter().enumerate() {
        let config = dir.path().join(format!("config-{index}"));
        track(&config, &record, raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
        let tracked: Vec<_> = fs::read_dir(config.join("conversation-sync/tracked")).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(tracked.len(), 1);
        let source: Source = serde_json::from_slice(&fs::read(&tracked[0]).unwrap()).unwrap();
        assert_eq!(source.path, canonical, "{raw}");
    }
}

#[test]
fn one_protected_copy_does_not_block_retiring_other_copies() {
    let f = Fixture::new(Tool::Claude);
    assert!(f.scan().errors.is_empty());
    let older = legacy_copy(&f, "protected-copy", &messages(), SystemTime::now() - Duration::from_secs(600));
    let other = legacy_copy(&f, "retirable-copy", &messages(), SystemTime::now() - Duration::from_secs(600));
    let mut store = hub::Hub::open(&f.config).unwrap();
    store.state.conversations.values_mut().next().unwrap().pending.extend([older.clone(), other.clone()]);
    store.save().unwrap();
    let metadata = |source: &Source, request: Metadata<'_>| -> Result<Option<String>> {
        if matches!(request, Metadata::Archive) {
            ensure!(source.id != older.id, "Linked snapshot has descendant histories; preserved for review");
            return archiving_into(source);
        }
        Ok(None)
    };
    let status = scan_with_metadata(&f.config, &f.pair, &f.gate, &mut Cache::new(), None, metadata, |_| {}).unwrap();
    assert!(!status.errors.is_empty());
    assert!(older.path.exists());
    assert!(!other.path.exists());
    let state = f.hub();
    let record = state.conversations.values().next().unwrap();
    assert!(record.pending.iter().any(|s| s.id == older.id));
    assert!(!record.pending.iter().any(|s| s.id == other.id));
}

#[test]
fn a_claude_error_notice_ends_its_turn_without_an_answer() {
    let mut data = rows(&format::render(Tool::Claude, &stable_id("notice"), "/w", "2026-10-15T00:00:00Z", &messages()).unwrap());
    let leaf = data.iter().rev().find(|r| r["type"] == "assistant").unwrap()["uuid"].clone();
    data.push(json!({"type": "user", "uuid": "prompt-1", "parentUuid": leaf, "isSidechain": false,
        "message": {"role": "user", "content": "Is the conversion done?"}}));
    data.push(json!({"type": "assistant", "uuid": "notice-1", "parentUuid": "prompt-1", "isSidechain": false,
        "isApiErrorMessage": true, "apiError": "autocompact_thrashing", "error": "invalid_request",
        "message": {"id": "notice", "type": "message", "role": "assistant", "model": "<synthetic>",
            "content": [{"type": "text", "text": "Autocompact is thrashing: the context refilled to the limit."}],
            "stop_reason": "stop_sequence", "stop_sequence": ""}}));
    let mut expected = messages();
    expected.push(text("user", "Is the conversion done?"));
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), expected);
}

#[test]
fn a_long_history_starts_claude_code_context_at_a_boundary() {
    let history = long_history("/w");
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("long"), "/w", "2026-10-15T00:00:00Z", &history, "Long").unwrap());
    let boundary = data.iter().position(|r| r["subtype"] == "compact_boundary").expect("a compaction boundary");
    assert_eq!(data.iter().filter(|r| r["subtype"] == "compact_boundary").count(), 1);
    assert_eq!(data[boundary]["parentUuid"], serde_json::Value::Null);
    assert_eq!(data[boundary]["logicalParentUuid"], data[boundary - 1]["uuid"]);
    let summary = &data[boundary + 1];
    assert_eq!((summary["isCompactSummary"].as_bool(), summary["parentUuid"].clone()), (Some(true), data[boundary]["uuid"].clone()));
    let listed = summary["message"]["content"].as_str().unwrap();
    assert!(listed.contains("Remember the exact token kiln-orchid-731") && listed.contains("Long request 0"), "{listed}");
    assert!(data[boundary + 2]["message"]["content"][0]["type"] != "tool_result", "a tool result stays with its call");
    let kept = context::after_boundary(&data);
    assert!(kept < context::LIMIT && kept > context::BUDGET / 2, "{kept}");
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    let short = rows(&format::render_titled(Tool::Claude, &stable_id("short"), "/w", "2026-10-15T00:00:00Z", &messages(), "Short").unwrap());
    assert!(short.iter().all(|r| r["subtype"] != "compact_boundary"), "a history within the budget has no boundary");
}

#[test]
fn appends_beyond_the_context_limit_start_a_new_boundary() {
    let f = Fixture::new(Tool::Codex);
    assert!(f.scan().errors.is_empty());
    let claude = f.peer();
    let mut history = messages();
    for k in 0..24 {
        history.extend(turns(&[&format!("Request {k}"), &format!("{k} {}", "appended answer\n".repeat(2_000))]));
    }
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(120));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(f.peer().id, claude.id, "the projection is appended to");
    let data = rows(&fs::read(&claude.path).unwrap());
    assert_eq!(data.iter().filter(|r| r["subtype"] == "compact_boundary").count(), 1);
    assert!(context::after_boundary(&data) < context::LIMIT);
    assert_eq!(read_messages(&claude).unwrap(), history);
    let more = turns(&["One more request", "One more answer"]);
    history.extend(more);
    write_history(&f.source, &history, SystemTime::now() - Duration::from_secs(60));
    assert!(f.scan().errors.is_empty());
    let data = rows(&fs::read(&claude.path).unwrap());
    assert_eq!(data.iter().filter(|r| r["subtype"] == "compact_boundary").count(), 1, "an append within the limit adds no boundary");
    assert_eq!(read_messages(&claude).unwrap(), history);
}

/// Claude Code tool uses and results of a rendered Claude history: (command, content, error).
fn claude_commands(data: &[serde_json::Value]) -> Vec<(String, String, bool)> {
    let blocks: Vec<&serde_json::Value> = data.iter().flat_map(|r| r["message"]["content"].as_array().into_iter().flatten()).collect();
    let uses: Vec<&&serde_json::Value> = blocks.iter().filter(|b| b["type"] == "tool_use").collect();
    uses.iter()
        .map(|u| {
            let result = blocks.iter().find(|b| b["type"] == "tool_result" && b["tool_use_id"] == u["id"]).expect("a result for each use");
            (u["input"]["command"].as_str().unwrap().to_owned(), result["content"].as_str().unwrap().to_owned(), result["is_error"] == true)
        })
        .collect()
}

fn exec_result(output: &str, code: i64) -> serde_json::Value {
    json!({"chunk_id": "c", "wall_time_seconds": 0.06, "exit_code": code, "original_token_count": 3, "output": output})
}

#[test]
fn codex_programs_of_every_printing_shape_are_bash_in_claude() {
    let parallel = "const r = await Promise.allSettled([\n  tools.exec_command({cmd:\"git status --short\",workdir:\"/repo\",max_output_tokens:3000}),\n  tools.exec_command({cmd:\"nl -ba README.md\",workdir:\"/w\",login:true,shell:\"/bin/zsh\"})\n]); r.forEach((x,i)=>text(JSON.stringify({i,result:x})));";
    let numbered = |value: serde_json::Value, i: usize| json!({"i": i, "result": {"status": "fulfilled", "value": value}}).to_string();
    let cases: Vec<(String, String, Vec<(&str, &str, bool)>)> = vec![
        (parallel.into(), numbered(exec_result(" M a.go\n", 0), 0) + &numbered(exec_result("     1\t# Kiln\n", 0), 1),
            vec![("cd /repo && git status --short", " M a.go\n", false), ("nl -ba README.md", "     1\t# Kiln\n", false)]),
        ("const out=await Promise.allSettled([tools.exec_command({cmd:\"pwd\"}),tools.exec_command({cmd:\"false\"})]);out.forEach(text);".into(),
            json!({"status": "fulfilled", "value": exec_result("/w\n", 0)}).to_string() + &json!({"status": "fulfilled", "value": exec_result("", 1)}).to_string(),
            vec![("pwd", "/w\n", false), ("false", "Exit code 1\n", true)]),
        ("const r=await Promise.allSettled([tools.exec_command({cmd:\"pwd\"}),tools.exec_command({cmd:\"date\"})]);r.forEach((x,i)=>text(JSON.stringify({i,status:x.status,result:x.status===\"fulfilled\"?x.value.output:String(x.reason)})))".into(),
            json!({"i": 0, "status": "fulfilled", "result": "/w\n"}).to_string() + &json!({"i": 1, "status": "fulfilled", "result": "Tue\n"}).to_string(),
            vec![("pwd", "/w\n", false), ("date", "Tue\n", false)]),
        ("text((await tools.exec_command({cmd:\"sed -n '1,3p' a.md\",workdir:\"/w\"})).output);".into(), "line one\nline two\n".into(),
            vec![("sed -n '1,3p' a.md", "line one\nline two\n", false)]),
        ("const r=await tools.exec_command({cmd:\"go test ./...\",yield_time_ms:10000});text(r)".into(),
            json!({"chunk_id": "c", "wall_time_seconds": 10.0, "session_id": 94533, "original_token_count": 3, "output": "ok  pkg/a\n"}).to_string(),
            vec![("go test ./...", "ok  pkg/a\nProcess running with session ID 94533", false)]),
    ];
    for (program, printed, expected) in cases {
        let call = format!("[Tool call: exec]\n{program}");
        let result = format!("[Tool result: call_ShapesResultId000000000000000]\nScript completed\nWall time 0.4 seconds\nOutput:\n{printed}");
        let history = shell_turn(&call, &result);
        let data = rows(&format::render_titled(Tool::Claude, &stable_id("shapes"), "/w", "2026-10-15T00:00:00Z", &history, "Shapes").unwrap());
        let shown = claude_commands(&data);
        let expected: Vec<(String, String, bool)> = expected.into_iter().map(|(c, o, e)| (c.to_owned(), o.to_owned(), e)).collect();
        assert_eq!(shown, expected, "{program}");
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history, "{program}");
        let data = rows(&format::render_titled(Tool::Codex, &stable_id("shapes"), "/w", "2026-10-15T00:00:00Z", &history, "Shapes").unwrap());
        assert_eq!(data.iter().filter(|r| r["payload"]["item"]["type"] == "CommandExecution").count(), shown.len());
        assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history, "{program}");
    }
}

#[test]
fn codex_programs_whose_results_cannot_be_matched_stay_text() {
    let reversed = "const [a,b]=await Promise.all([tools.exec_command({cmd:\"pwd\"}),tools.exec_command({cmd:\"date\"})]);text(b);text(a);";
    let mixed = "text(await tools.write_stdin({session_id:1,chars:\"\"}));text(await tools.exec_command({cmd:\"pwd\"}));";
    let counted = "const r=await tools.exec_command({cmd:\"git log\"});text(r.output.split(\"\\n\").length)";
    for (program, printed) in [
        (reversed, exec_result("Tue\n", 0).to_string() + &exec_result("/w\n", 0).to_string()),
        (mixed, json!({"chunk_id": "a", "wall_time_seconds": 1.0, "exit_code": 0, "original_token_count": 1, "output": "done\n"}).to_string() + &exec_result("/w\n", 0).to_string()),
        (counted, "42".into()),
    ] {
        let history = shell_turn(&format!("[Tool call: exec]\n{program}"), &format!("[Tool result: call_TextResultId00000000000000000]\nScript completed\nWall time 0.1 seconds\nOutput:\n{printed}"));
        let data = rows(&format::render_titled(Tool::Claude, &stable_id("text"), "/w", "2026-10-15T00:00:00Z", &history, "Text").unwrap());
        assert!(claude_commands(&data).is_empty(), "{program}");
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    }
    let shown = portable::display(&format!("[Tool result: call_x]\nScript completed\nWall time 0.1 seconds\nOutput:\n{}", json!({"i": 0, "result": {"status": "fulfilled", "value": exec_result("     21\t- line\n", 0)}})));
    assert!(shown.contains("21\t- line") && !shown.contains("wall_time_seconds") && !shown.contains("chunk_id"), "{shown}");
}

#[test]
fn codex_exec_command_function_calls_are_bash_in_claude() {
    let call = "[Tool call: exec_command]\n{\"cmd\":\"sed -n '1,2p' owner.go\",\"workdir\":\"/repo\",\"max_output_tokens\":10000}";
    let done = "[Tool result: call_FunctionResultId0000000000000]\nChunk ID: 62bf47\nWall time: 0.0389 seconds\nProcess exited with code 0\nOriginal token count: 4\nOutput:\npackage toolhost\n";
    let running = "[Tool result: call_FunctionResultId0000000000000]\nChunk ID: 62bf48\nWall time: 10.0 seconds\nProcess running with session ID 77869\nOriginal token count: 0\nOutput:\n";
    for (result, content) in [(done, "package toolhost\n"), (running, "Process running with session ID 77869")] {
        let history = shell_turn(call, result);
        let data = rows(&format::render_titled(Tool::Claude, &stable_id("function"), "/w", "2026-10-15T00:00:00Z", &history, "Function").unwrap());
        assert_eq!(claude_commands(&data), [("cd /repo && sed -n '1,2p' owner.go".to_owned(), content.to_owned(), false)]);
        assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
        let data = rows(&format::render_titled(Tool::Codex, &stable_id("function"), "/w", "2026-10-15T00:00:00Z", &history, "Function").unwrap());
        let item = data.iter().find(|r| r["payload"]["type"] == "function_call").expect("the original function call");
        assert_eq!(item["payload"]["name"], "exec_command");
        assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    }
}

const PATCH_CALL: &str = "[Tool call: apply_patch]\n*** Begin Patch\n*** Update File: /w/app.go\n@@\n type usecase struct {\n \tservice X\n+\tagents Y\n@@\n-func New(s X) U {\n+func New(s X, a Y) U {\n*** Add File: /w/new.go\n+package w\n*** End Patch";
const PATCH_RESULT: &str = "[Tool result: call_PatchCallId0000000000000000]\nExit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM /w/app.go\nA /w/new.go\n";

fn patch_change(id: &str) -> serde_json::Value {
    json!({"type": "FileChange", "id": id, "status": "completed", "stdout": "Success.", "stderr": "", "changes": {
        "/w/new.go": {"type": "add", "content": "package w\n"},
        "/w/app.go": {"type": "update", "unified_diff": "@@ -40,2 +40,3 @@\n type usecase struct {\n \tservice X\n+\tagents Y\n@@ -88,2 +89,2 @@\n-func New(s X) U {\n+func New(s X, a Y) U {\n \treturn nil\n"}}})
}

/// Claude Code records of a rendered history: (tool, file path or command, structured result).
fn claude_records(data: &[serde_json::Value]) -> Vec<(String, String, serde_json::Value)> {
    data.iter()
        .filter(|r| r["message"]["content"][0]["type"] == "tool_use")
        .map(|r| {
            let used = &r["message"]["content"][0];
            let result = data.iter().find(|q| q["message"]["content"][0]["tool_use_id"] == used["id"]).expect("a result record");
            let target = used["input"]["file_path"].as_str().or(used["input"]["command"].as_str()).unwrap().to_owned();
            (used["name"].as_str().unwrap().to_owned(), target, result["toolUseResult"].clone())
        })
        .collect()
}

#[test]
fn codex_patches_are_claude_code_edits_and_writes() {
    let history = shell_turn(PATCH_CALL, PATCH_RESULT);
    let changes = tools::FileChanges::from([("call_PatchCallId0000000000000000".to_owned(), vec![patch_change("call_PatchCallId0000000000000000")])]);
    let data = rows(&format::render_changed(Tool::Claude, &stable_id("patch"), "/w", "2026-10-15T00:00:00Z", &history, "Patch", &changes).unwrap());
    let records = claude_records(&data);
    assert_eq!(records.iter().map(|(t, p, _)| (t.as_str(), p.as_str())).collect::<Vec<_>>(), [("Edit", "/w/app.go"), ("Edit", "/w/app.go"), ("Write", "/w/new.go")]);
    assert_eq!(records[0].2["structuredPatch"], json!([{"oldStart": 40, "oldLines": 2, "newStart": 40, "newLines": 3, "lines": [" type usecase struct {", " \tservice X", "+\tagents Y"]}]));
    assert_eq!(records[1].2["structuredPatch"][0]["newStart"], 89);
    let edit = data.iter().find(|r| r["message"]["content"][0]["name"] == "Edit").unwrap();
    assert_eq!(edit["message"]["content"][0]["input"], json!({"file_path": "/w/app.go", "old_string": "type usecase struct {\n\tservice X", "new_string": "type usecase struct {\n\tservice X\n\tagents Y", "replace_all": false}));
    assert_eq!((records[2].2["type"].as_str(), records[2].2["content"].as_str()), (Some("create"), Some("package w\n")));
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    let data = rows(&format::render_changed(Tool::Codex, &stable_id("patch"), "/w", "2026-10-15T00:00:00Z", &history, "Patch", &changes).unwrap());
    let cell = data.iter().find(|r| r["payload"]["item"]["type"] == "FileChange").expect("a file change cell");
    assert_eq!(cell["payload"]["item"]["id"], "call_PatchCallId0000000000000000#0");
    assert!(data.iter().any(|r| r["payload"]["type"] == "custom_tool_call" && r["payload"]["name"] == "apply_patch"));
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
    let bytes = format::render_changed(Tool::Codex, &stable_id("patch"), "/w", "2026-10-15T00:00:00Z", &history, "Patch", &changes).unwrap();
    assert_eq!(tools::file_changes(&bytes[..]).unwrap().get("call_PatchCallId0000000000000000").map(Vec::len), Some(1), "a Kiln copy keeps its file changes findable");
}

#[test]
fn a_codex_program_that_patches_and_runs_commands_is_edits_and_bash() {
    let patch = PATCH_CALL.strip_prefix("[Tool call: apply_patch]\n").unwrap();
    let call = format!("[Tool call: exec]\ntext(await tools.apply_patch({}));\ntext(await tools.exec_command({{cmd:\"gofmt -l .\",workdir:\"/repo\"}}));", json!(patch));
    let result = format!("[Tool result: call_ProgramPatchId00000000000000]\nScript completed\nWall time 0.3 seconds\nOutput:\n{{}}{}", exec_result("", 0));
    let history = shell_turn(&call, &result);
    let changes = tools::FileChanges::from([("call_ProgramPatchId00000000000000".to_owned(), vec![patch_change("exec-1")])]);
    let data = rows(&format::render_changed(Tool::Claude, &stable_id("program"), "/w", "2026-10-15T00:00:00Z", &history, "Program", &changes).unwrap());
    let records = claude_records(&data);
    assert_eq!(records.iter().map(|(t, p, _)| (t.as_str(), p.as_str())).collect::<Vec<_>>(), [("Edit", "/w/app.go"), ("Edit", "/w/app.go"), ("Write", "/w/new.go"), ("Bash", "cd /repo && gofmt -l .")]);
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
    let data = rows(&format::render_changed(Tool::Codex, &stable_id("program"), "/w", "2026-10-15T00:00:00Z", &history, "Program", &changes).unwrap());
    let kinds: Vec<&str> = data.iter().filter_map(|r| r["payload"]["item"]["type"].as_str()).filter(|t| matches!(*t, "FileChange" | "CommandExecution")).collect();
    assert_eq!(kinds, ["FileChange", "CommandExecution"]);
    assert_eq!(format::parse(Tool::Codex, &data).unwrap(), history);
}

#[test]
fn patches_without_recorded_changes_or_with_deletions_stay_text() {
    let history = shell_turn(PATCH_CALL, PATCH_RESULT);
    let data = rows(&format::render_titled(Tool::Claude, &stable_id("unknown"), "/w", "2026-10-15T00:00:00Z", &history, "Patch").unwrap());
    assert!(claude_records(&data).is_empty(), "without Codex's file changes there are no line numbers");
    let mut deleted = patch_change("call_PatchCallId0000000000000000");
    deleted["changes"]["/w/new.go"] = json!({"type": "delete", "content": "package w\n"});
    let changes = tools::FileChanges::from([("call_PatchCallId0000000000000000".to_owned(), vec![deleted])]);
    let data = rows(&format::render_changed(Tool::Claude, &stable_id("deleted"), "/w", "2026-10-15T00:00:00Z", &history, "Patch", &changes).unwrap());
    assert!(claude_records(&data).is_empty(), "Claude Code has no record of a deletion");
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
}

#[test]
fn file_changes_belong_to_the_call_codex_was_running() {
    let item = |payload: serde_json::Value| json!({"timestamp": "t", "type": "response_item", "payload": payload}).to_string();
    let event = |item: serde_json::Value| json!({"timestamp": "t", "type": "event_msg", "payload": {"type": "item_completed", "item": item}}).to_string();
    let history = [
        item(json!({"type": "custom_tool_call", "call_id": "call_A", "name": "exec", "input": "text(await tools.apply_patch(\"p\"))"})),
        event(patch_change("exec-0b1")),
        item(json!({"type": "custom_tool_call_output", "call_id": "call_A", "output": []})),
        item(json!({"type": "custom_tool_call", "call_id": "call_B", "name": "apply_patch", "input": "p"})),
        item(json!({"type": "custom_tool_call", "call_id": "call_C", "name": "apply_patch", "input": "q"})),
        event(patch_change("call_B")),
        event(patch_change("exec-orphan")),
    ]
    .join("\n");
    let changes = tools::file_changes(history.as_bytes()).unwrap();
    assert_eq!(changes.get("call_A").map(Vec::len), Some(1));
    assert_eq!(changes.get("call_B").map(Vec::len), Some(1));
    assert!(!changes.contains_key("call_C"), "an item made while two editing calls run belongs to neither");
}

#[test]
fn a_claude_copy_shows_the_edits_of_its_codex_original() {
    let f = Fixture::new(Tool::Codex);
    let mut history = messages();
    history.extend(shell_turn(PATCH_CALL, PATCH_RESULT));
    let changes = tools::FileChanges::from([("call_PatchCallId0000000000000000".to_owned(), vec![patch_change("call_PatchCallId0000000000000000")])]);
    fs::write(&f.source.path, format::render_changed(Tool::Codex, &f.source.id, &f.source.cwd, "2026-10-14T00:00:00Z", &history, "Patch", &changes).unwrap()).unwrap();
    set_modified(&f.source.path, SystemTime::now() - Duration::from_secs(120));
    let status = f.scan();
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    let claude = f.peer();
    let data = rows(&fs::read(&claude.path).unwrap());
    assert_eq!(claude_records(&data).iter().filter(|(t, _, _)| t == "Edit").count(), 2);
    assert_eq!(read_messages(&claude).unwrap(), history);
}



#[test]
fn relative_patch_paths_map_to_the_file_each_names() {
    let call = "[Tool call: apply_patch]\n*** Begin Patch\n*** Update File: crates/x/src/lib.rs\n@@\n-a\n+b\n*** Update File: src/lib.rs\n@@\n-c\n+d\n*** End Patch";
    let result = "[Tool result: call_RelativePatchId000000000000000]\nExit code: 0\nWall time: 0 seconds\nOutput:\nSuccess.\n";
    let item = json!({"type": "FileChange", "id": "call_RelativePatchId000000000000000", "status": "completed", "changes": {
        "/w/crates/x/src/lib.rs": {"type": "update", "unified_diff": "@@ -1 +1 @@\n-a\n+b\n"},
        "/w/src/lib.rs": {"type": "update", "unified_diff": "@@ -1 +1 @@\n-c\n+d\n"}}});
    let changes = tools::FileChanges::from([("call_RelativePatchId000000000000000".to_owned(), vec![item])]);
    let history = shell_turn(call, result);
    let data = rows(&format::render_changed(Tool::Claude, &stable_id("relative"), "/w", "2026-10-15T00:00:00Z", &history, "Relative", &changes).unwrap());
    let edits: Vec<(String, String)> = data
        .iter()
        .filter(|r| r["message"]["content"][0]["name"] == "Edit")
        .map(|r| {
            let input = &r["message"]["content"][0]["input"];
            (input["file_path"].as_str().unwrap().to_owned(), input["old_string"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(edits, [("/w/crates/x/src/lib.rs".to_owned(), "a".to_owned()), ("/w/src/lib.rs".to_owned(), "c".to_owned())]);
    assert_eq!(format::parse(Tool::Claude, &data).unwrap(), history);
}

#[test]
fn a_call_left_without_output_does_not_take_later_file_changes() {
    let item = |payload: serde_json::Value| json!({"timestamp": "t", "type": "response_item", "payload": payload}).to_string();
    let history = [
        item(json!({"type": "function_call", "call_id": "call_stale", "name": "web__run", "arguments": "{}"})),
        json!({"timestamp": "t", "type": "compacted", "payload": {"replacement_history": [{"type": "function_call", "call_id": "call_old"}]}}).to_string(),
        item(json!({"type": "custom_tool_call", "call_id": "call_edit", "name": "exec", "input": "text(await tools.apply_patch(\"p\"))"})),
        json!({"timestamp": "t", "type": "event_msg", "payload": {"type": "item_completed", "item": patch_change("exec-1234")}}).to_string(),
        item(json!({"type": "custom_tool_call_output", "call_id": "call_edit", "output": []})),
        item(json!({"type": "function_call", "call_id": "call_waiting", "name": "web__run", "arguments": "{}"})),
        item(json!({"type": "custom_tool_call", "call_id": "call_next", "name": "apply_patch", "input": "p"})),
        json!({"timestamp": "t", "type": "event_msg", "payload": {"type": "item_completed", "item": patch_change("exec-5678")}}).to_string(),
    ]
    .join("\n");
    let changes = tools::file_changes(history.as_bytes()).unwrap();
    assert_eq!(changes.get("call_edit").map(Vec::len), Some(1));
    assert_eq!(changes.get("call_next").map(Vec::len), Some(1), "only calls that edit files take an unnamed change");
    assert!(!changes.contains_key("call_stale") && !changes.contains_key("call_waiting"));
}


/// The summary table of agy 1.4.5's `conversation_summaries.db`.
const AGY_SUMMARIES: &str = "CREATE TABLE IF NOT EXISTS `conversation_summaries` (`conversation_id` text,`title` text NOT NULL DEFAULT \"\",`preview` text NOT NULL DEFAULT \"\",`step_count` integer NOT NULL DEFAULT 0,`last_modified_time` datetime NOT NULL,`workspace_uris` text NOT NULL,`status` text NOT NULL DEFAULT \"\",`source` text NOT NULL DEFAULT \"\",`project_id` text NOT NULL DEFAULT \"\",`agent_name` text NOT NULL DEFAULT \"\",`parent_conversation_id` text NOT NULL DEFAULT \"\",`nesting_depth` integer NOT NULL DEFAULT 0,`battle_id` text NOT NULL DEFAULT \"\",`winning_conversation_id` text NOT NULL DEFAULT \"\",`not_fully_idle` numeric NOT NULL DEFAULT false,`killed` numeric NOT NULL DEFAULT false,`last_user_input_time` datetime NOT NULL,`last_user_input_step_index` integer NOT NULL DEFAULT -1,`app_data_dir` text NOT NULL DEFAULT \"\",`raw_summary` blob,`group_id` text NOT NULL DEFAULT \"\",PRIMARY KEY (`conversation_id`));";

fn agy_user(step: u64, text: &str) -> serde_json::Value {
    json!({"step_index": step, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "created_at": "2026-10-22T10:57:56Z",
        "content": format!("<USER_REQUEST>\n{text}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-10-22T19:57:56+09:00.\n</ADDITIONAL_METADATA>")})
}
fn agy_answer(step: u64, text: &str) -> serde_json::Value {
    json!({"step_index": step, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-10-22T10:58:00Z",
        "input_tokens": 9351, "cache_read_tokens": 0, "output_tokens": 193, "thinking": "Private reasoning.", "content": text})
}
fn agy_calls(step: u64, calls: &[(&str, serde_json::Value)]) -> serde_json::Value {
    let calls: Vec<_> = calls.iter().map(|(name, args)| {
        let args: serde_json::Map<_, _> = args.as_object().unwrap().iter().map(|(k, v)| (k.clone(), json!(v.to_string()))).collect();
        json!({"name": name, "args": args})
    }).collect();
    json!({"step_index": step, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-10-22T10:57:58Z",
        "input_tokens": 9351, "cache_read_tokens": 0, "output_tokens": 193, "thinking": "Private reasoning.", "tool_calls": calls})
}
fn agy_result(step: u64, output: &str) -> serde_json::Value {
    json!({"step_index": step, "source": "MODEL", "type": "GENERIC", "status": "DONE", "created_at": "2026-10-22T10:58:03Z",
        "content": format!("Created At: 2026-10-22T19:58:03+09:00\nCompleted At: 2026-10-22T19:58:03+09:00\n{output}")})
}
fn agy_system(step: u64) -> serde_json::Value {
    json!({"step_index": step, "source": "SYSTEM", "type": "SYSTEM_MESSAGE", "status": "DONE", "created_at": "2026-10-22T11:08:28Z",
        "content": "The following is a <SYSTEM_MESSAGE> not actually sent by the user."})
}
fn agy_turn() -> Vec<serde_json::Value> {
    vec![
        agy_user(0, "Add agy support"),
        agy_calls(1, &[("run_command", json!({"CommandLine": "ls -la", "WaitMsBeforeAsync": 5000})), ("read_file", json!({"Path": "/w/AGENTS.md"}))]),
        agy_result(2, "total 8"),
        agy_result(3, "1: # Working on Kiln"),
        agy_system(4),
        agy_answer(5, "Added support."),
    ]
}
fn agy_messages() -> Vec<Message> {
    vec![
        text("user", "Add agy support"),
        text("assistant", "[Tool call: run_command]\n{\"CommandLine\":\"ls -la\",\"WaitMsBeforeAsync\":5000}"),
        text("assistant", "[Tool call: read_file]\n{\"Path\":\"/w/AGENTS.md\"}"),
        text("assistant", "[Tool result: agy-1-0]\ntotal 8"),
        text("assistant", "[Tool result: agy-1-1]\n1: # Working on Kiln"),
        text("assistant", "Added support."),
    ]
}

/// One agy conversation in an agy data folder, written as agy 1.4.5 writes its files.
struct Agy {
    root: PathBuf,
    id: String,
    transcript: PathBuf,
}
impl Agy {
    fn new(root: &Path, key: &str, cwd: &str, parent: Option<&str>, records: &[serde_json::Value]) -> Self {
        let id = stable_id(key);
        for dir in ["conversations", "annotations", "presence"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("conversations").join(format!("{id}.db")), b"").unwrap();
        fs::write(root.join("annotations").join(format!("{id}.pbtxt")), format!("title:\"Agy {key} \\\"quoted\\\"\"")).unwrap();
        let db = rusqlite::Connection::open(root.join("conversation_summaries.db")).unwrap();
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
        db.execute_batch(AGY_SUMMARIES).unwrap();
        let uris = json!([url::Url::from_file_path(cwd).unwrap().to_string()]).to_string();
        db.execute(
            "INSERT INTO conversation_summaries (conversation_id, workspace_uris, parent_conversation_id, last_modified_time, last_user_input_time) VALUES (?1, ?2, ?3, '2026-10-22 12:00:00+00:00', '2026-10-22 12:00:00+00:00')",
            (&id, &uris, parent.unwrap_or("")),
        ).unwrap();
        let logs = root.join("brain").join(&id).join(".system_generated").join("logs");
        fs::create_dir_all(&logs).unwrap();
        let agy = Self { root: root.into(), id, transcript: logs.join("transcript_full.jsonl") };
        fs::write(&agy.transcript, b"").unwrap();
        agy.append(records);
        agy
    }
    fn append(&self, records: &[serde_json::Value]) {
        let mut file = fs::OpenOptions::new().append(true).open(&self.transcript).unwrap();
        for record in records {
            writeln!(file, "{record}").unwrap();
        }
        drop(file);
        fs::copy(&self.transcript, self.transcript.with_file_name("transcript.jsonl")).unwrap();
    }
    fn source(&self, cwd: &str) -> Source {
        Source { tool: Tool::Antigravity, root: self.root.canonical().unwrap(), path: self.transcript.canonical().unwrap(), cwd: cwd.into(), id: self.id.clone() }
    }
}
/// Every file below `root` with its bytes and modification time.
fn agy_snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let meta = fs::metadata(&path).unwrap();
                files.insert(path.clone(), (fs::read(&path).unwrap(), meta.modified().unwrap()));
            }
        }
    }
    files
}
/// A workspace with only agy conversations: the fixture's own transcript is removed.
fn agy_workspace(f: &Fixture) -> (PathBuf, discovery::Scope) {
    fs::remove_file(&f.source.path).unwrap();
    fs::remove_dir_all(f.config.join("conversation-sync/tracked")).unwrap();
    let root = f.config.parent().unwrap().join("agy");
    fs::create_dir_all(&root).unwrap();
    let mut scope = workspace_scope(f);
    scope.roots.push((Tool::Antigravity, root.canonical().unwrap()));
    (root, scope)
}

#[test]
fn antigravity_records_become_portable_messages_without_reasoning_or_system_steps() {
    let f = Fixture::new(Tool::Claude);
    let (root, _) = agy_workspace(&f);
    let agy = Agy::new(&root, "records", &f.source.cwd, None, &agy_turn());
    let source = agy.source(&f.source.cwd);
    let mut title = String::new();
    let (rows, _, messages) = read_rows_offsets_titled(&source, &mut title).unwrap();
    assert_eq!(messages, agy_messages());
    assert_eq!(title, "Agy records \"quoted\"");
    assert!(rows.iter().all(|r| r.get("thinking").is_none() && r["type"] != "SYSTEM_MESSAGE"));
    // A turn still running is left until it is answered.
    agy.append(&[agy_user(6, "Release it"), agy_calls(7, &[("run_command", json!({"CommandLine": "cargo test"}))])]);
    assert_eq!(read_messages(&source).unwrap(), agy_messages());
    // A transcript left unchanged ends its unanswered turn.
    set_modified(&agy.transcript, SystemTime::now() - Duration::from_secs(6 * 60));
    let mut settled = agy_messages();
    settled.extend([
        text("user", "Release it"),
        text("assistant", "[Tool call: run_command]\n{\"CommandLine\":\"cargo test\"}"),
        text("assistant", "[Session event: unanswered]\nAntigravity stopped before answering this request."),
    ]);
    assert_eq!(read_messages(&source).unwrap(), settled);
    let (_, checkpoint) = checkpoint::read(&source, &mut String::new()).unwrap();
    let checkpoint = checkpoint.unwrap();
    assert_eq!(checkpoint.offset, fs::metadata(&agy.transcript).unwrap().len());
    // The rest of a settled turn is read on its own.
    agy.append(&[agy_result(8, "ok"), agy_answer(9, "Released.")]);
    let (appended, _, _) = checkpoint::since_titled(&source, &checkpoint).unwrap().unwrap();
    assert_eq!(appended, vec![text("assistant", "[Tool result: agy-8]\nok"), text("assistant", "Released.")]);
}

#[test]
fn antigravity_conversations_reach_both_clis_without_writing_to_agy() {
    let f = Fixture::new(Tool::Claude);
    let (root, scope) = agy_workspace(&f);
    let agy = Agy::new(&root, "main", &f.source.cwd, None, &agy_turn());
    Agy::new(&root, "subagent", &f.source.cwd, Some(&agy.id), &agy_turn());
    let orphan = Agy::new(&root, "orphan", &f.source.cwd, None, &agy_turn());
    fs::remove_file(root.join("conversations").join(format!("{}.db", orphan.id))).unwrap();
    let before = agy_snapshot(&root);
    let first = workspace_scan(&f, &scope);
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.published, 2);
    assert_eq!(agy_snapshot(&root), before, "the agy folder is only read");
    let store = hub::Hub::open(&f.config).unwrap();
    assert_eq!(store.state.conversations.len(), 1);
    let record = store.state.conversations.values().next().unwrap();
    assert_eq!(record.title, "Agy main \"quoted\"");
    let origin = record.projections.iter().find(|p| p.origin).unwrap();
    assert_eq!((origin.source.tool, origin.source.id.as_str()), (Tool::Antigravity, agy.id.as_str()));
    let copies: Vec<_> = record.projections.iter().filter(|p| !p.origin).map(|p| p.source.clone()).collect();
    assert_eq!(copies.iter().map(|s| s.tool).collect::<Vec<_>>(), vec![Tool::Claude, Tool::Codex]);
    for copy in &copies {
        assert_eq!(read_messages(copy).unwrap(), agy_messages(), "{:?}", copy.tool);
    }
    let links = fs::read_to_string(f.config.join("conversation-sync/native-links.json")).unwrap();
    assert!(!links.contains("antigravity"), "agy has no resume hook: {links}");
    assert_eq!(workspace_scan(&f, &scope).published, 0);
    // A turn continued in agy reaches both copies.
    agy.append(&[agy_user(6, "Also update docs"), agy_answer(7, "Docs updated.")]);
    let before = agy_snapshot(&root);
    let next = workspace_scan(&f, &scope);
    assert!(next.errors.is_empty(), "{:?}", next.errors);
    assert_eq!(next.published, 2);
    assert_eq!(agy_snapshot(&root), before);
    let mut expected = agy_messages();
    expected.extend(turns(&["Also update docs", "Docs updated."]));
    for copy in &copies {
        assert_eq!(read_messages(copy).unwrap(), expected, "{:?}", copy.tool);
    }
}

#[test]
fn a_diverged_antigravity_original_is_detached_never_archived() {
    let f = Fixture::new(Tool::Claude);
    let (root, scope) = agy_workspace(&f);
    let agy = Agy::new(&root, "diverged", &f.source.cwd, None, &agy_turn());
    assert!(workspace_scan(&f, &scope).errors.is_empty());
    let claude = hub::Hub::open(&f.config).unwrap().state.conversations.values().next().unwrap()
        .projections.iter().find(|p| p.source.tool == Tool::Claude).unwrap().source.clone();
    let mut continued = agy_messages();
    continued.extend(turns(&["claude side", "claude reply"]));
    assert!(canonical::update(&claude, &continued, "", "2026-10-22T13:00:00.000Z", &Default::default()).unwrap());
    agy.append(&[agy_user(6, "agy side"), agy_answer(7, "agy reply")]);
    set_modified(&agy.transcript, SystemTime::now() - Duration::from_secs(120));
    let before = agy_snapshot(&root);
    let status = workspace_scan(&f, &scope);
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(agy_snapshot(&root), before, "a detached original stays in place");
    let store = hub::Hub::open(&f.config).unwrap();
    let record = store.state.conversations.values().next().unwrap();
    assert!(!record.projections.iter().any(|p| p.source.tool == Tool::Antigravity && !p.removed));
    assert!(record.aliases.iter().any(|a| a.tool == Tool::Antigravity && a.id == agy.id));
    assert!(!record.branches.is_empty(), "the agy continuation is kept as a branch");
    assert_eq!(read_messages(&claude).unwrap(), continued);
    agy.append(&[agy_user(8, "later agy turn"), agy_answer(9, "later reply")]);
    let again = workspace_scan(&f, &scope);
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    assert_eq!(hub::Hub::open(&f.config).unwrap().state.conversations.len(), 1, "a detached original is not adopted again");
    assert_eq!(read_messages(&claude).unwrap(), continued);
}

#[test]
fn an_unanswered_agy_turn_reaches_the_copies_once_its_transcript_settles() {
    let f = Fixture::new(Tool::Claude);
    let (root, scope) = agy_workspace(&f);
    let agy = Agy::new(&root, "settles", &f.source.cwd, None, &agy_turn());
    assert!(workspace_scan(&f, &scope).errors.is_empty());
    agy.append(&[agy_user(6, "Release it")]);
    let waiting = workspace_scan(&f, &scope);
    assert!(waiting.errors.is_empty(), "{:?}", waiting.errors);
    assert_eq!(waiting.published, 0, "a turn agy may still answer is not copied");
    set_modified(&agy.transcript, SystemTime::now() - Duration::from_secs(6 * 60));
    let settled = workspace_scan(&f, &scope);
    assert!(settled.errors.is_empty(), "{:?}", settled.errors);
    assert_eq!(settled.published, 2);
    let mut expected = agy_messages();
    expected.extend([text("user", "Release it"), text("assistant", "[Session event: unanswered]\nAntigravity stopped before answering this request.")]);
    for copy in hub::Hub::open(&f.config).unwrap().state.conversations.values().next().unwrap().projections.iter().filter(|p| !p.origin) {
        assert_eq!(read_messages(&copy.source).unwrap(), expected, "{:?}", copy.source.tool);
    }
    assert_eq!(workspace_scan(&f, &scope).published, 0);
}

#[test]
fn an_unreadable_agy_folder_leaves_the_other_clis_synchronizing() {
    let f = Fixture::new(Tool::Claude);
    let root = f.config.parent().unwrap().join("agy");
    let agy = Agy::new(&root, "broken", &f.source.cwd, None, &agy_turn());
    fs::write(root.join("conversation_summaries.db"), b"not a database").unwrap();
    let mut scope = workspace_scope(&f);
    scope.roots.push((Tool::Antigravity, root.canonical().unwrap()));
    let status = workspace_scan(&f, &scope);
    assert!(status.errors.is_empty(), "{:?}", status.errors);
    assert_eq!(status.published, 1, "the Claude conversation still reaches Codex");
    let store = hub::Hub::open(&f.config).unwrap();
    assert!(store.state.conversations.values().all(|r| r.projections.iter().all(|p| p.source.id != agy.id)));
}

#[test]
fn older_agy_tool_steps_and_rewritten_steps_read_once() {
    let f = Fixture::new(Tool::Claude);
    let (root, _) = agy_workspace(&f);
    let mut records = vec![agy_user(0, "Inspect"), agy_calls(1, &[("list_dir", json!({"DirectoryPath": "/w"})), ("view_file", json!({"AbsolutePath": "/w/a"}))])];
    let mut running = agy_result(2, "listing");
    running["status"] = json!("RUNNING");
    records.push(running.clone());
    records.push(running);
    records.push(json!({"step_index": 3, "source": "MODEL", "type": "VIEW_FILE", "status": "DONE", "content": "file a"}));
    records.push(agy_answer(4, "Inspected."));
    let agy = Agy::new(&root, "older", &f.source.cwd, None, &records);
    assert_eq!(read_messages(&agy.source(&f.source.cwd)).unwrap(), vec![
        text("user", "Inspect"),
        text("assistant", "[Tool call: list_dir]\n{\"DirectoryPath\":\"/w\"}"),
        text("assistant", "[Tool call: view_file]\n{\"AbsolutePath\":\"/w/a\"}"),
        text("assistant", "[Tool result: agy-1-0]\nlisting"),
        text("assistant", "[Tool result: agy-1-1]\nfile a"),
        text("assistant", "Inspected."),
    ]);
}


#[test]
fn bounded_sync_rolling_limit_applies_to_existing_hub_and_keeps_files() {
    let f = Fixture::new(Tool::Claude);
    let mut scope=workspace_scope(&f);
    scope.limits.insert(PathBuf::from(&f.source.cwd),1);
    let first=workspace_scan(&f,&scope);
    assert_eq!(first.watched,1);
    let original_peer=f.peer();
    let mut newer=f.source.clone();
    newer.id=stable_id("bounded-newer");
    newer.path=newer.path.with_file_name(format!("{}.jsonl",newer.id));
    write_history(&newer,&turns(&["new question","new answer"]),SystemTime::now()+Duration::from_secs(2));
    let next=workspace_scan(&f,&scope);
    assert_eq!(next.watched,1,"existing hub must obey the cap too");
    assert_eq!(next.rows.len(),1,"copies count as one logical conversation");
    assert!(next.rows.values().any(|r|r.source.id==newer.id));
    assert!(f.source.path.exists() && original_peer.path.exists(),"excluded histories must remain");
    assert_eq!(workspace_scan(&f,&scope).published,0,"self-written copies must not promote old conversations");
    append_native_claude_turn(&f.source,"resumed old question","resumed old answer");
    set_modified(&f.source.path,SystemTime::now()+Duration::from_secs(4));
    let resumed=workspace_scan(&f,&scope);
    assert_eq!(resumed.watched,1);
    assert!(resumed.rows.values().any(|r|r.source.id==f.source.id),"native activity promotes an old conversation");
    scope.limits.insert(PathBuf::from(&f.source.cwd),2);
    assert_eq!(workspace_scan(&f,&scope).watched,2,"raising the limit restores eligibility");
    scope.limits.insert(PathBuf::from(&f.source.cwd),1);
    assert_eq!(workspace_scan(&f,&scope).watched,1,"lowering the limit prunes existing hub work");
}

#[test]
fn bounded_sync_alias_failures_respect_retry_delay() {
    let f=Fixture::new(Tool::Claude);
    let mut cache=Cache::new();
    scan(&f.config,&f.pair,&f.gate,&mut cache,|_|{}).unwrap();
    let mut alias=f.source.clone();
    alias.id=stable_id("bounded-bad-alias");
    alias.path=alias.path.with_file_name(format!("{}.jsonl",alias.id));
    let mut data=rows(&fs::read(&f.source.path).unwrap());
    for row in &mut data {row["sessionId"]=json!(alias.id);if row["type"]=="assistant"{row["parentUuid"]=json!("missing-parent");}}
    fs::write(&alias.path,data.iter().map(|r|format!("{r}\n")).collect::<String>()).unwrap();
    let mut store=hub::Hub::open(&f.config).unwrap();
    store.state.conversations.values_mut().next().unwrap().aliases.push(hub::NativeRef::of(&alias));
    store.save().unwrap();
    publish(&f.config.join("conversation-sync/tracked/alias.json"),&serde_json::to_vec(&alias).unwrap()).unwrap();
    let key=alias.path.to_string_lossy().into_owned();
    scan(&f.config,&f.pair,&f.gate,&mut cache,|_|{}).unwrap();
    let failed=cache.hub[&key].1;
    scan(&f.config,&f.pair,&f.gate,&mut cache,|_|{}).unwrap();
    assert_eq!(cache.hub[&key].1,failed,"unchanged alias failure must not parse/fail every scan");
}

#[test]
#[ignore = "synthetic parser timing probe"]
fn bounded_sync_compaction_parser_scaling() {
    for n in [1000usize,2000,4000,8000] {
        let mut data=Vec::new();
        for i in 0..n {
            data.push(json!({"type":if i%2==0{"user"}else{"assistant"},"uuid":format!("id-{i}"),"parentUuid":if i==0{None}else{Some(format!("id-{}",i-1))},"message":{"content":"synthetic text","stop_reason":"end_turn"}}));
        }
        let plain=std::time::Instant::now();
        assert_eq!(format::parse(Tool::Claude,&data).unwrap().len(),n);
        let plain=plain.elapsed();
        data.push(json!({"type":"system","subtype":"compact_boundary","uuid":"boundary","parentUuid":null,"logicalParentUuid":format!("id-{}",n-1)}));
        data.push(json!({"type":"user","uuid":"summary","parentUuid":"boundary","isCompactSummary":true,"message":{"content":"summary"}}));
        data.push(json!({"type":"assistant","uuid":"after","parentUuid":"summary","message":{"content":"done","stop_reason":"end_turn"}}));
        let start=std::time::Instant::now();
        assert_eq!(format::parse(Tool::Claude,&data).unwrap().len(),n+1);
        println!("AUDIT parser messages={n} plain_ms={:.3} compacted_ms={:.3}",plain.as_secs_f64()*1000.0,start.elapsed().as_secs_f64()*1000.0);
    }
}

#[test]
fn bounded_sync_stale_completion_cannot_clear_a_new_scan() {
    let mut manager=Manager::default();
    let stale=Arc::new(AtomicBool::new(false));
    manager.gate=Arc::new(AtomicBool::new(true));
    manager.busy=true;
    manager.tx.send(Event::Scanned(stale,Err("stale".into()))).unwrap();
    let f=Fixture::new(Tool::Claude);
    let mut settings=super::super::state::Settings::default();
    settings.conversation_sync_enabled=true;
    settings.conversation_sync_pair=Some(f.pair.clone());
    manager.configured=Some(f.pair);
    manager.workspaces=vec![PathBuf::from(f.source.cwd)];
    manager.tick(&mut settings,manager.workspaces.clone(),&egui::Context::default());
    assert!(manager.busy,"late cancelled completion must not end the current scan");
}

#[test]
fn bounded_sync_limit_setting_roundtrips_and_dialog_fits() {
    use egui_kittest::{Harness,kittest::Queryable};
    for theme in ["kiln-dark","kiln-light"] {
        kiln_common::Theme::set_current(theme);
        let f=Fixture::new(Tool::Claude);
        let root=PathBuf::from(&f.source.cwd);
        let accounts=AccountManager::with_env(kiln_accounts::Env::sandbox(f._temp.path(),false).0);
        let settings: super::super::state::Settings=serde_json::from_str("{}").unwrap();
        assert!(settings.conversation_sync_limits.is_empty());
        let mut installed=false;
        let mut h=Harness::builder().with_size([720.0,600.0]).build_ui_state(|ui,state:&mut(Manager,super::super::state::Settings)| {
            if !installed {kiln_common::fonts::install(ui.ctx());kiln_common::Theme::current().apply(ui.ctx());installed=true;return;}
            state.0.dialog(ui.ctx(),"Workspace fixture",&root,true,&mut state.1,&accounts);
        },(Manager::default(),settings));
        h.run_steps(4);
        let input=h.get_by_label("최근 대화 수");
        assert!(h.ctx.content_rect().contains_rect(input.rect()));
        h.render().unwrap().save(std::env::temp_dir().join(format!("kiln-bounded-{theme}.png"))).unwrap();
        h.state_mut().1.conversation_sync_limits.insert(root.clone(),7);
        let restored:super::super::state::Settings=serde_json::from_slice(&serde_json::to_vec(&h.state().1).unwrap()).unwrap();
        assert_eq!(restored.conversation_sync_limits[&root],7);
    }
}

#[test]
fn bounded_sync_full_discovery_is_cached_until_refresh() {
    let f=Fixture::new(Tool::Claude);
    let mut scope=workspace_scope(&f);
    scope.limits.insert(PathBuf::from(&f.source.cwd),1);
    set_modified(&f.source.path,SystemTime::now()-Duration::from_secs(120));
    let mut cache=Cache::new();cache.quiet=Duration::from_secs(10);
    let scan_now=|cache:&mut Cache|scan_with_metadata(&f.config,&f.pair,&f.gate,cache,Some(&scope),|_,_|Ok(None),|_|{}).unwrap();
    assert_eq!(scan_now(&mut cache).watched,1);
    let mut newer=f.source.clone();newer.id=stable_id("catalog-newer");newer.path=newer.path.with_file_name(format!("{}.jsonl",newer.id));
    write_history(&newer,&turns(&["new","answer"]),SystemTime::now()-Duration::from_secs(60));
    assert!(scan_now(&mut cache).rows.values().all(|r|r.source.id!=newer.id));
    cache.catalog.as_mut().unwrap().0=Instant::now()-Duration::from_secs(31);
    assert!(scan_now(&mut cache).rows.values().any(|r|r.source.id==newer.id));
}

#[test]
fn bounded_sync_review_imports_merge_before_limit() {
    let f = Fixture::new(Tool::Codex);
    let mut scope = workspace_scope(&f);
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
    scope.limits.insert(PathBuf::from(&f.source.cwd), 1);
    let again = workspace_scan(&f, &scope);
    assert_eq!(again.published, 0);
    assert_eq!(again.rows.len(), 1);
    let archived = f
        .pair
        .codex
        .root
        .join("archived_sessions")
        .join(external.path.file_name().unwrap());
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
    native_turn(&external.path);
    let changed = workspace_scan(&f, &scope);
    assert_eq!(changed.published, 1);
    assert!(changed.errors.is_empty(), "{:?}", changed.errors);
    assert_eq!(
        changed.rows.len(),
        1,
        "the continued import is the same conversation"
    );
    assert_eq!(
        transcripts(&f.pair.codex.root),
        vec![external.path.clone()],
        "the continued import is the Codex projection"
    );
    assert_eq!(
        read_messages(&f.projection_in(&f.pair.claude.root)).unwrap(),
        continued
    );
}

#[test]
fn bounded_sync_review_duplicate_native_paths_do_not_panic() {
    let f = Fixture::new(Tool::Claude);
    let scope = workspace_scope(&f);
    let duplicate = f
        .pair
        .claude
        .root
        .join("projects/another-project")
        .join(format!("{}.jsonl", f.source.id));
    private_dir(duplicate.parent().unwrap()).unwrap();
    fs::copy(&f.source.path, &duplicate).unwrap();
    let status = workspace_scan(&f, &scope);
    assert_eq!(status.watched, 1);
    assert!(status.errors.is_empty(), "{:?}", status.errors);
}

#[test]
fn bounded_sync_review_pending_copy_does_not_promote_old_history() {
    let f = Fixture::new(Tool::Claude);
    let mut scope = workspace_scope(&f);
    write_history(
        &f.source,
        &messages(),
        SystemTime::now() - Duration::from_secs(120),
    );
    workspace_scan(&f, &scope);
    let mut latest = f.source.clone();
    latest.id = stable_id("review-latest");
    latest.path = latest.path.with_file_name(format!("{}.jsonl", latest.id));
    write_history(
        &latest,
        &turns(&["latest question", "latest answer"]),
        SystemTime::now() - Duration::from_secs(60),
    );
    workspace_scan(&f, &scope);
    let mut pending = f.source.clone();
    pending.id = stable_id("review-pending");
    pending.path = pending.path.with_file_name(format!("{}.jsonl", pending.id));
    write_history(
        &pending,
        &messages(),
        SystemTime::now() + Duration::from_secs(10),
    );
    let mut store = hub::Hub::open(&f.config).unwrap();
    let old = store
        .state
        .conversations
        .values_mut()
        .find(|r| r.projections.iter().any(|p| p.source.id == f.source.id))
        .unwrap();
    old.pending.push(pending.clone());
    store.save().unwrap();
    scope.limits.insert(PathBuf::from(&f.source.cwd), 1);
    let status = workspace_scan(&f, &scope);
    assert_eq!(status.watched, 1);
    assert!(
        status.rows.values().any(|r| r.source.id == latest.id),
        "a synthetic pending copy is not native activity"
    );
    assert!(
        pending.path.exists(),
        "out-of-range pending copy remains untouched"
    );
}

#[test]
fn bounded_sync_paused_workspace_does_not_parse_or_publish() {
    let f = Fixture::new(Tool::Claude);
    let mut scope = workspace_scope(&f);
    workspace_scan(&f, &scope);
    let before = fs::read(f.projection_in(&f.pair.codex.root).path).unwrap();
    fs::write(&f.source.path, b"not json\n").unwrap();
    scope.paused.push(PathBuf::from(&f.source.cwd));
    let paused = workspace_scan(&f, &scope);
    assert_eq!(paused.checked, 0);
    assert_eq!(paused.published, 0);
    assert!(paused.errors.is_empty());
    assert_eq!(fs::read(f.projection_in(&f.pair.codex.root).path).unwrap(), before);
    scope.paused.clear();
    assert!(!workspace_scan(&f, &scope).errors.is_empty(), "the invalid body is read only after resume");
}

#[test]
fn bounded_sync_terminal_exit_schedules_once_and_cancels_running_scan() {
    let mut manager = Manager::default();
    manager.gate.store(true, Ordering::Release);
    manager.busy = true;
    let old = manager.gate.clone();
    let root = PathBuf::from("/workspace");
    let active = BTreeMap::from([(1, (root.clone(), true)), (2, (root.clone(), true))]);
    manager.observe_terminals(active.clone());
    assert!(!old.load(Ordering::Acquire));
    assert!(!manager.busy);
    assert_eq!(manager.paused, vec![root.clone()]);
    manager.observe_terminals(BTreeMap::from([(1, (root.clone(), false)), (2, (root.clone(), true))]));
    assert!(manager.exit_sync);
    assert_eq!(manager.paused, vec![root.clone()], "another running CLI keeps the workspace paused");
    manager.observe_terminals(BTreeMap::from([(1, (root.clone(), false))]));
    assert!(manager.paused.is_empty());
    let deadline = manager.next;
    manager.observe_terminals(BTreeMap::from([(1, (root.clone(), false))]));
    assert_eq!(manager.next, deadline, "idle snapshots do not repeatedly schedule sync");
    assert_eq!(BACKGROUND_INTERVAL, Duration::from_secs(60));
}

#[test]
fn bounded_sync_enabling_requests_first_scan_without_overriding_active_pause() {
    let f = Fixture::new(Tool::Claude);
    let accounts = AccountManager::with_env(kiln_accounts::Env::sandbox(f._temp.path(), false).0);
    let mut settings = super::super::state::Settings::default();
    settings.conversation_sync_pair = Some(f.pair.clone());
    let mut manager = Manager::default();
    manager.paused = vec![PathBuf::from(&f.source.cwd)];
    manager.enable(&mut settings, &accounts);
    assert!(manager.exit_sync, "initial explicit linking must not wait a full polling interval");
    assert!(!manager.urgent, "initial linking still respects the active CLI pause");
    assert_eq!(manager.paused.len(), 1);
}
