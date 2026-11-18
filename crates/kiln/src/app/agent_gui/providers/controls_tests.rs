use super::*;
fn configured(tool: Tool) -> Adapter {
    let mut a = Adapter::new(tool, "/tmp".into(), None);
    match tool {
        Tool::Claude => {
            a.ingest(json!({"type":"control_response","response":{"request_id":"initialize","subtype":"success","response":{"models":[{"value":"opus","displayName":"Opus","supportedEffortLevels":["low","high"],"supportsEffort":true,"supportsFastMode":true}],"commands":[{"name":"skill-a","description":"A"}],"available_output_styles":["default"],"current_permission_mode":"default"}}})).unwrap();
            a.ingest(json!({"type":"system","model":"opus","session_id":"s"}))
                .unwrap();
        }
        Tool::Codex => {
            a.ingest(json!({"id":"thread","result":{"thread":{"id":"s"},"model":"m","effort":"low","collaborationMode":{"mode":"default","settings":{"model":"m","reasoning_effort":"low","developer_instructions":"SECRET"}}}})).unwrap();
            a.ingest(json!({"id":"models","result":{"data":[{"id":"m","model":"m","displayName":"M","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"ultra"}],"defaultReasoningEffort":"low","serviceTiers":[{"id":"priority","name":"Fast"}]}]}})).unwrap();
            a.ingest(json!({"id":"collaboration","result":{"data":[{"name":"Default","mode":"default"},{"name":"Plan","mode":"plan","reasoning_effort":"low"}]}})).unwrap();
        }
        Tool::Antigravity => {
            a.ingest(json!({"id":"session","result":{"sessionId":"s","configOptions":[{"id":"thought_level","name":"Thought","category":"thought_level","type":"select","currentValue":"low","options":[{"group":"g","name":"Group","options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]}]}]}})).unwrap();
        }
    }
    a
}
fn configure(a: &mut Adapter, key: &str, value: ControlValue) -> Effects {
    a.command(
        &Command::Configure {
            control: key.into(),
            value,
        },
        "runtime",
    )
    .unwrap()
}
fn submit(a: &mut Adapter) -> Result<Effects> {
    a.command(
        &Command::Submit {
            text: "test".into(),
            attachments: vec![],
            model: None,
        },
        "runtime",
    )
}
fn claude_reply(id: &Value, payload: Value) -> Value {
    json!({"type":"control_response","response":{"request_id":id,"subtype":"success","response":payload}})
}
#[test]
fn acp_full_empty_snapshot_and_unknown_type_are_not_lost() {
    let mut a = configured(Tool::Antigravity);
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"custom","name":"Custom","type":"future","category":"unknown","currentValue":true}]}}})).unwrap();
    assert!(e.events.iter().any(
        |e| matches!(e,Event::Controls(ControlEvent::Options(v))if v.len()==1&&!v[0].available)
    ));
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"config_option_update","configOptions":[]}}})).unwrap();
    assert!(
        e.events
            .iter()
            .any(|e| matches!(e,Event::Controls(ControlEvent::Options(v))if v.is_empty()))
    );
}
#[test]
fn acp_ack_snapshot_not_intent_is_effective_and_blocks_submit() {
    let mut a = configured(Tool::Antigravity);
    assert!(
        a.command(
            &Command::Configure {
                control: "thought_level".into(),
                value: ControlValue::Text("ultra".into())
            },
            "runtime"
        )
        .is_err()
    );
    let e = configure(&mut a, "thought_level", ControlValue::Text("high".into()));
    assert!(submit(&mut a).is_err());
    let e = a
        .ingest(json!({"id":e.writes[0]["id"],"result":{"configOptions":[]}}))
        .unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Unconfirmed)));
}
#[test]
fn codex_empty_ack_needs_effective_notification_and_no_secret_projection() {
    for before_ack in [false, true] {
        let mut a = configured(Tool::Codex);
        let e = configure(&mut a, "effort", ControlValue::Text("ultra".into()));
        let ack = json!({"id":e.writes[0]["id"],"result":{}});
        let notification = json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"ultra","collaborationMode":{"mode":"default","settings":{"developer_instructions":"SECRET-CANARY","model":"m","reasoning_effort":"ultra"}}}}});
        let first = if before_ack {
            notification.clone()
        } else {
            ack.clone()
        };
        let second = if before_ack { ack } else { notification };
        let e = a.ingest(first).unwrap();
        assert!(!e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Applied)));
        assert!(submit(&mut a).is_err());
        let e = a.ingest(second).unwrap();
        assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Applied)));
        assert!(
            !serde_json::to_string(&e.events)
                .unwrap()
                .contains("SECRET-CANARY")
        );
        assert!(submit(&mut a).is_ok());
    }
}
#[test]
fn codex_invalid_effort_is_rejected_before_wire() {
    let mut a = configured(Tool::Codex);
    assert!(
        a.command(
            &Command::Configure {
                control: "effort".into(),
                value: ControlValue::Text("made-up".into())
            },
            "runtime"
        )
        .is_err()
    );
    let e = configure(&mut a, "serviceTier", ControlValue::Text("priority".into()));
    assert_eq!(e.writes[0]["params"]["serviceTier"], "priority");
}
#[test]
fn codex_optional_failure_is_local() {
    let mut a = configured(Tool::Codex);
    let q = a
        .command(&Command::Discover(Discovery::Mcp), "runtime")
        .unwrap();
    let e = a
        .ingest(json!({"id":q.writes[0]["id"],"error":{"code":-32600,"message":"unknown"}}))
        .unwrap();
    assert!(
        !e.events
            .iter()
            .any(|e| matches!(e, Event::Lifecycle(Lifecycle::Failed)))
    );
    assert!(submit(&mut a).is_ok());
}
#[test]
fn claude_effort_preserves_ultracode_and_readback_caps() {
    let mut a = configured(Tool::Claude);
    let q = a
        .command(&Command::Discover(Discovery::Settings), "runtime")
        .unwrap();
    a.ingest(claude_reply(&q.writes[0]["request_id"],json!({"effective":{"skillOverrides":{"sibling":"off"},"env":{"SECRET":"secret-canary"}},"applied":{"model":"opus","effort":"low","ultracode":true,"ultracodeRequested":true,"ultracodeAvailable":true}}))).unwrap();
    let q = configure(&mut a, "effortLevel", ControlValue::Text("high".into()));
    assert_eq!(q.writes[0]["request"]["settings"]["ultracode"], true);
    assert!(submit(&mut a).is_err());
    let e = a
        .ingest(claude_reply(&q.writes[0]["request_id"], json!({})))
        .unwrap();
    assert_eq!(e.writes[0]["request"]["subtype"], "get_settings");
    let e=a.ingest(claude_reply(&e.writes[0]["request_id"],json!({"effective":{},"applied":{"model":"opus","effort":"low","ultracode":true,"ultracodeRequested":true}}))).unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.effective==Some(ControlValue::Text("low".into()))&&v.message.is_some())));
    assert!(
        !serde_json::to_string(&e.events)
            .unwrap()
            .contains("secret-canary")
    );
}
#[test]
fn claude_skill_override_patch_preserves_siblings() {
    let mut a = configured(Tool::Claude);
    let q = a
        .command(&Command::Discover(Discovery::Settings), "runtime")
        .unwrap();
    a.ingest(claude_reply(
        &q.writes[0]["request_id"],
        json!({"effective":{"skillOverrides":{"sibling":"off"}},"applied":{"model":"opus"}}),
    ))
    .unwrap();
    let q = configure(
        &mut a,
        "skill:skill-a",
        ControlValue::Text("name-only".into()),
    );
    assert_eq!(
        q.writes[0]["request"]["settings"]["skillOverrides"],
        json!({"sibling":"off","skill-a":"name-only"})
    );
}
#[test]
fn native_cancel_retains_settlement_barrier_without_prompt() {
    let mut a = configured(Tool::Codex);
    let q = configure(&mut a, "effort", ControlValue::Text("ultra".into()));
    a.command(&Command::Interrupt, "runtime").unwrap();
    assert!(submit(&mut a).is_err());
    a.ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    let e=a.ingest(json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"ultra"}}})).unwrap();
    assert!(e.writes.is_empty());
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Cancelled)));
}
#[test]
fn codex_unrelated_effective_update_cannot_confirm_requested_setting() {
    let mut a = configured(Tool::Codex);
    let q = configure(&mut a, "effort", ControlValue::Text("ultra".into()));
    a.ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    let e=a.ingest(json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"low"}}})).unwrap();
    assert!(!e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Applied)));
    assert!(submit(&mut a).is_err());
}
#[test]
fn codex_tier_reset_accepts_native_default_normalization() {
    let mut a = configured(Tool::Codex);
    let q = configure(&mut a, "serviceTier", ControlValue::Default);
    a.ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    let e=a.ingest(json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"low","serviceTier":"default"}}})).unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Applied&&v.message.is_none())));
}
#[test]
fn native_timeout_is_unconfirmed_and_late_readback_still_settles() {
    let mut flight = NativeFlight::default();
    let mut e = Effects::default();
    let id = flight
        .begin("effort", ControlValue::Text("high".into()), &mut e)
        .unwrap();
    let e = flight.tick_at(std::time::Instant::now() + std::time::Duration::from_secs(16));
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Unconfirmed&&v.operation==id)));
    assert!(flight.idle().is_err());
    let mut e = Effects::default();
    flight.finish(
        ApplicationState::Applied,
        Some(ControlValue::Text("high".into())),
        None,
        &mut e,
    );
    assert!(flight.idle().is_ok());
}
#[test]
fn optional_discovery_timeout_does_not_occupy_write_slot() {
    let mut flight = NativeFlight::default();
    flight.read(Discovery::Mcp);
    let e = flight.tick_at(std::time::Instant::now() + std::time::Duration::from_secs(16));
    assert!(flight.reads.is_empty());
    assert!(flight.idle().is_ok());
    assert!(
        e.events.iter().any(
            |e| matches!(e,Event::Controls(ControlEvent::Status(v))if v.unavailable.is_some())
        )
    );
}
#[test]
fn claude_resolved_model_retains_advertised_effort() {
    let mut a = Adapter::new(Tool::Claude, "/tmp".into(), None);
    a.ingest(claude_reply(&json!("initialize"),json!({"models":[{"value":"opus","resolvedModel":"claude-opus-canonical","displayName":"Opus","supportedEffortLevels":["low","high"]}]}))).unwrap();
    let e = a
        .ingest(json!({"type":"system","model":"claude-opus-canonical","session_id":"s"}))
        .unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Options(v))if v.iter().any(|o|o.id=="effortLevel"&&o.choices.len()==2))));
}
#[test]
fn native_skills_send_requires_advertised_exact_path() {
    let mut a = configured(Tool::Codex);
    let q = a
        .command(&Command::Discover(Discovery::Skills), "runtime")
        .unwrap();
    a.ingest(json!({"id":q.writes[0]["id"],"result":{"data":[{"cwd":"/tmp","skills":[{"name":"s","path":"/tmp/s/SKILL.md","enabled":true,"description":"Skill"}],"errors":[]}]}})).unwrap();
    let c = Command::SubmitWithSkills {
        text: "use skill".into(),
        attachments: vec![],
        model: None,
        skills: vec![NativeSkillInput {
            name: "s".into(),
            path: "/wrong".into(),
        }],
    };
    assert!(a.command(&c, "runtime").is_err());
    let mut c = c;
    if let Command::SubmitWithSkills { skills, .. } = &mut c {
        skills[0].path = "/tmp/s/SKILL.md".into();
    }
    let e = a.command(&c, "runtime").unwrap();
    assert_eq!(
        e.writes[0]["params"]["input"][1],
        json!({"type":"skill","name":"s","path":"/tmp/s/SKILL.md"})
    );
}
#[test]
fn explicit_goal_and_queue_actions_never_start_a_turn() {
    let mut a = configured(Tool::Codex);
    let q = a
        .command(
            &Command::NativeAction(NativeAction::Codex(CodexAction::SetGoal {
                objective: Some("goal".into()),
                status: Some(GoalStatus::Active),
                token_budget: None,
            })),
            "runtime",
        )
        .unwrap();
    assert_eq!(q.writes[0]["method"], "thread/goal/set");
    assert_eq!(q.writes[0]["params"]["origin"], "user");
    let e=a.ingest(json!({"id":q.writes[0]["id"],"result":{"goal":{"objective":"goal","status":"active"}}})).unwrap();
    assert!(e.writes.is_empty());
    let q = a
        .command(
            &Command::NativeAction(NativeAction::Codex(CodexAction::AddQueuedTask {
                text: "queued".into(),
            })),
            "runtime",
        )
        .unwrap();
    assert_eq!(q.writes[0]["method"], "thread/queue/add");
    assert!(!q.writes.iter().any(|v| v["method"] == "turn/start"));
}
#[test]
fn permission_subset_retains_deny_entries_and_rejects_expansion() {
    let mut a = configured(Tool::Codex);
    let request = json!({"id":31,"method":"item/permissions/requestApproval","params":{"threadId":"s","turnId":"t","permissions":{"fileSystem":{"read":["/allowed"],"entries":[{"access":"deny","path":{"type":"path","path":"/secret"}}]},"network":{"enabled":false}}}});
    a.ingest(request).unwrap();
    let grant = PermissionGrant {
        entry_indices: vec![],
        read_indices: vec![0],
        write_indices: vec![],
        network: false,
        scope: PermissionScope::Turn,
        strict_review: true,
    };
    let e = a
        .command(
            &Command::Respond {
                request_id: "31".into(),
                decision: Decision::Grant(grant),
                answers: BTreeMap::new(),
            },
            "runtime",
        )
        .unwrap();
    assert_eq!(
        e.writes[0]["result"]["permissions"]["fileSystem"]["entries"][0]["access"],
        "deny"
    );
    assert!(
        e.writes[0]["result"]["permissions"]
            .get("network")
            .is_none()
    );
}
#[test]
fn acp_deferred_submit_requires_matching_full_config_ack() {
    let mut a = configured(Tool::Antigravity);
    a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"old","options":[{"value":"old","name":"Old"},{"value":"new","name":"New"}]}]}}})).unwrap();
    let q = a
        .command(
            &Command::Submit {
                text: "must not send".into(),
                attachments: vec![],
                model: Some("new".into()),
            },
            "runtime",
        )
        .unwrap();
    let e = a
        .ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    assert!(!e.writes.iter().any(|v| v["method"] == "session/prompt"));
    assert!(e.cancelled);
}
#[test]
fn acp_load_keeps_commands_and_usage_without_replaying_history() {
    let mut a = Adapter::new(Tool::Antigravity, "/tmp".into(), Some("s".into()));
    a.ingest(json!({"id":"initialize","result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}})).unwrap();
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"plan","description":"Plan"}]}}})).unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Catalog{kind:CatalogKind::Commands,entries})if entries.len()==1)));
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"text":"already journaled"}}}})).unwrap();
    assert!(e.events.is_empty());
}
#[test]
fn codex_model_change_reconciles_effort_tier_and_nested_mode() {
    let mut a = configured(Tool::Codex);
    a.ingest(json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"ultra","serviceTier":"priority","collaborationMode":{"mode":"plan","settings":{"model":"m","reasoning_effort":"ultra"}}}}})).unwrap();
    a.ingest(json!({"id":"models","result":{"data":[{"id":"new-alias","model":"new-native","displayName":"New","supportedReasoningEfforts":[{"reasoningEffort":"low"}],"defaultReasoningEffort":"low","serviceTiers":[]}]}})).unwrap();
    let q = configure(&mut a, "model", ControlValue::Text("new-alias".into()));
    assert_eq!(
        q.writes[0]["params"]["collaborationMode"]["settings"]["model"],
        "new-native"
    );
    assert_eq!(
        q.writes[0]["params"]["collaborationMode"]["settings"]["reasoning_effort"],
        "low"
    );
    assert_eq!(q.writes[0]["params"]["collaborationMode"]["mode"], "plan");
    assert_eq!(q.writes[0]["params"]["serviceTier"], Value::Null);
}
#[test]
fn native_start_options_are_not_replayed_on_resume() {
    let options = LaunchOptions::Codex(CodexLaunch {
        model: Some("m".into()),
        developer_instructions: Some("private-canary".into()),
        ..Default::default()
    });
    let mut a = Adapter::new(Tool::Codex, "/tmp".into(), Some("s".into()));
    a.launch_options(options);
    let e = a.ingest(json!({"id":"initialize","result":{}})).unwrap();
    let resume = e
        .writes
        .iter()
        .find(|v| v["method"] == "thread/resume")
        .unwrap();
    assert!(resume["params"].get("developerInstructions").is_none());
    assert!(!e.writes.iter().any(|v| v["method"] == "thread/start"));
}
#[test]
fn claude_complete_inventory_contains_182_keys_and_masks_values() {
    let rows: Value = serde_json::from_str(include_str!("claude_settings_catalog.json")).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 182);
    let mut a = configured(Tool::Claude);
    let q = a
        .command(&Command::Discover(Discovery::Settings), "runtime")
        .unwrap();
    let e=a.ingest(claude_reply(&q.writes[0]["request_id"],json!({"effective":{"apiKeyHelper":"canary-secret","env":{"SECRET":"canary-secret"},"hooks":{"body":"canary-secret"},"mcpServers":{"env":"canary-secret"}},"sources":{"userSettings":{"apiKeyHelper":"canary-secret"}},"applied":{"model":"opus"}}))).unwrap();
    assert!(
        !serde_json::to_string(&e.events)
            .unwrap()
            .contains("canary-secret")
    );
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Options(options))if options.len()>=182&&options.iter().any(|o|o.id=="apiKeyHelper"&&!o.available&&o.current.is_none()))));
}
#[test]
fn codex_native_decisions_must_be_offered_and_session_choice_stays_distinct() {
    let mut a = configured(Tool::Codex);
    a.ingest(json!({"id":"approval","method":"item/commandExecution/requestApproval","params":{"threadId":"s","availableDecisions":["acceptForSession","decline"]}})).unwrap();
    let c = Command::Respond {
        request_id: "\"approval\"".into(),
        decision: Decision::Allow,
        answers: BTreeMap::new(),
    };
    assert!(a.command(&c, "runtime").is_err());
    let e = a
        .command(
            &Command::Respond {
                request_id: "\"approval\"".into(),
                decision: Decision::Option("native-decision-0".into()),
                answers: BTreeMap::new(),
            },
            "runtime",
        )
        .unwrap();
    assert_eq!(e.writes[0]["result"]["decision"], "acceptForSession");
}
#[test]
fn codex_active_turn_action_rejects_stale_turn_and_has_separate_state() {
    let mut a = configured(Tool::Codex);
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"t"}}}))
        .unwrap();
    let action = |turn: &str| {
        Command::NativeAction(NativeAction::Codex(CodexAction::UpdateActiveTurn {
            turn_id: turn.into(),
            control: "effort".into(),
            value: ControlValue::Text("ultra".into()),
        }))
    };
    assert!(a.command(&action("old"), "runtime").is_err());
    let q = a.command(&action("t"), "runtime").unwrap();
    assert_eq!(q.writes[0]["method"], "turn/settings/update");
    let e = a
        .ingest(json!({"id":q.writes[0]["id"],"result":{"status":"targetUnavailable"}}))
        .unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Outcome(v))if v.state==ApplicationState::Rejected)));
}
#[test]
fn claude_old_status_read_cannot_replace_new_effective_readback() {
    let mut a = configured(Tool::Claude);
    let old = a
        .command(&Command::Discover(Discovery::Status), "runtime")
        .unwrap();
    let q = configure(&mut a, "ultracode", ControlValue::Bool(true));
    let next = a
        .ingest(claude_reply(&q.writes[0]["request_id"], json!({})))
        .unwrap();
    a.ingest(claude_reply(&next.writes[0]["request_id"],json!({"effective":{},"applied":{"model":"opus","ultracode":true,"ultracodeRequested":true}}))).unwrap();
    let stale=a.ingest(claude_reply(&old.writes[0]["request_id"],json!({"effective":{},"applied":{"model":"opus","ultracode":false,"ultracodeRequested":false}}))).unwrap();
    assert!(stale.events.is_empty());
}
#[test]
fn bounded_catalogs_publish_explicit_partial_coverage() {
    let mut a = configured(Tool::Antigravity);
    let commands: Vec<Value> = (0..513)
        .map(|i| json!({"name":format!("command-{i}"),"description":"x"}))
        .collect();
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"available_commands_update","availableCommands":commands}}})).unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Coverage(v))if v.received==513&&v.retained==512&&v.partial&&v.reason.is_some())));
}
#[test]
fn compact_ack_is_not_completion_and_blocks_send() {
    let mut a = configured(Tool::Codex);
    let q = a
        .command(&Command::NativeAction(NativeAction::Compact), "runtime")
        .unwrap();
    let ack = a
        .ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    assert!(
        !ack.events
            .iter()
            .any(|e| matches!(e, Event::Lifecycle(Lifecycle::Ready)))
    );
    assert!(submit(&mut a).is_err());
    a.ingest(json!({"method":"item/completed","params":{"threadId":"s","turnId":"compact","item":{"id":"compact","type":"contextCompaction"}}})).unwrap();
    assert!(submit(&mut a).is_ok());
}
#[test]
fn codex_active_turn_model_carries_new_model_defaults() {
    let mut a = configured(Tool::Codex);
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"t"}}}))
        .unwrap();
    let q = a
        .command(
            &Command::NativeAction(NativeAction::Codex(CodexAction::UpdateActiveTurn {
                turn_id: "t".into(),
                control: "model".into(),
                value: ControlValue::Text("m".into()),
            })),
            "runtime",
        )
        .unwrap();
    assert_eq!(q.writes[0]["params"]["effort"], "low");
    assert_eq!(q.writes[0]["params"]["serviceTier"], Value::Null);
}

// Review regressions exercise public adapter commands and native wire ordering.
#[test]
fn review_r1_claude_legacy_submit_invalidates_discovery_across_stop() {
    for target in [Discovery::Settings, Discovery::Status] {
        let mut a = configured(Tool::Claude);
        let old = a
            .command(&Command::Discover(target.clone()), "runtime")
            .unwrap();
        let send = |model: &str| Command::Submit {
            text: "explicit".into(),
            attachments: vec![],
            model: Some(model.into()),
        };
        let first = a.command(&send("other"), "runtime").unwrap();
        a.command(&Command::Interrupt, "runtime").unwrap();
        let stale = a
            .ingest(claude_reply(
                &old.writes[0]["request_id"],
                json!({"applied":{"model":"opus"}}),
            ))
            .unwrap();
        assert!(
            stale.events.is_empty(),
            "pre-change discovery must be discarded"
        );
        // Even discovery requested while a retired write is outstanding cannot settle it.
        let during = a.command(&Command::Discover(target), "runtime").unwrap();
        a.ingest(claude_reply(
            &during.writes[0]["request_id"],
            json!({"applied":{"model":"opus"}}),
        ))
        .unwrap();
        let replacement = a.command(&send("opus"), "runtime").unwrap();
        assert!(replacement.writes.is_empty());
        let retired = a
            .ingest(claude_reply(&first.writes[0]["request_id"], json!({})))
            .unwrap();
        assert_eq!(retired.writes.len(), 1);
        assert_eq!(retired.writes[0]["request"]["subtype"], "set_model");
        assert_eq!(retired.writes[0]["request"]["model"], "opus");
        let fresh = a
            .ingest(claude_reply(&retired.writes[0]["request_id"], json!({})))
            .unwrap();
        assert_eq!(fresh.writes.len(), 1);
        assert_eq!(fresh.writes[0]["type"], "user");
    }
}

#[test]
fn review_r2_acp_missing_snapshot_retains_barrier_until_authoritative_update() {
    for malformed in [json!({}), json!({"configOptions":{}})] {
        let mut a = configured(Tool::Antigravity);
        let q = configure(&mut a, "thought_level", ControlValue::Text("high".into()));
        let e = a
            .ingest(json!({"id":q.writes[0]["id"],"result":malformed}))
            .unwrap();
        assert!(e.events.iter().any(|e| matches!(e,Event::Controls(ControlEvent::Outcome(v)) if v.state == ApplicationState::Unconfirmed && !v.settled)));
        assert!(submit(&mut a).is_err());
        let e = a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"thought_level","name":"Thought","type":"select","currentValue":"high","options":[{"value":"high","name":"High"}]}]}}})).unwrap();
        assert!(e.writes.is_empty(), "blocked Submit is never replayed");
        assert!(e.events.iter().any(|e| matches!(e,Event::Controls(ControlEvent::Outcome(v)) if v.state == ApplicationState::Applied && v.settled)));
        assert!(submit(&mut a).is_ok());
    }
    let mut a = configured(Tool::Antigravity);
    let q = configure(&mut a, "thought_level", ControlValue::Text("high".into()));
    let e = a
        .ingest(json!({"id":q.writes[0]["id"],"error":{"code":-32602,"message":"invalid"}}))
        .unwrap();
    assert!(e.events.iter().any(|e| matches!(e,Event::Controls(ControlEvent::Outcome(v)) if v.state == ApplicationState::Rejected && v.settled)));
    assert!(submit(&mut a).is_ok());
}

fn active_change(turn: &str, control: &str, value: &str) -> Command {
    Command::NativeAction(NativeAction::Codex(CodexAction::UpdateActiveTurn {
        turn_id: turn.into(),
        control: control.into(),
        value: ControlValue::Text(value.into()),
    }))
}
fn disjoint_models() -> Value {
    json!({"data":[
        {"id":"m","model":"m","displayName":"A","supportedReasoningEfforts":[{"reasoningEffort":"low"}],"defaultReasoningEffort":"low","serviceTiers":[{"id":"priority","name":"Fast"}],"defaultServiceTier":"priority"},
        {"id":"b","model":"b-native","displayName":"B","supportedReasoningEfforts":[{"reasoningEffort":"high"}],"defaultReasoningEffort":"high","serviceTiers":[{"id":"flex","name":"Flex"}],"defaultServiceTier":"flex"}
    ]})
}
#[test]
fn review_r3_active_turn_model_drives_sequential_effort_and_tier() {
    let mut a = configured(Tool::Codex);
    a.ingest(json!({"id":"models","result":disjoint_models()}))
        .unwrap();
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"t"}}}))
        .unwrap();
    assert!(
        a.command(&active_change("stale", "model", "b"), "runtime")
            .is_err()
    );
    let q = a
        .command(&active_change("t", "model", "b"), "runtime")
        .unwrap();
    a.ingest(json!({"id":q.writes[0]["id"],"result":{"status":"targetUnavailable"}}))
        .unwrap();
    assert!(
        a.command(&active_change("t", "effort", "high"), "runtime")
            .is_err()
    );
    let q = a
        .command(&active_change("t", "model", "b"), "runtime")
        .unwrap();
    let e = a
        .ingest(json!({"id":q.writes[0]["id"],"result":{"status":"applied"}}))
        .unwrap();
    assert!(
        !e.events
            .iter()
            .any(|e| matches!(e,Event::Models{selected:Some(v),..} if v=="b"||v=="b-native"))
    );
    for (control, new, old) in [
        ("effort", "high", "low"),
        ("serviceTier", "flex", "priority"),
    ] {
        assert!(
            a.command(&active_change("t", control, old), "runtime")
                .is_err()
        );
        let q = a
            .command(&active_change("t", control, new), "runtime")
            .unwrap();
        assert_eq!(q.writes[0]["params"][control], new);
        a.ingest(json!({"id":q.writes[0]["id"],"result":{"status":"applied"}}))
            .unwrap();
    }
    a.ingest(json!({"method":"turn/completed","params":{"threadId":"s","turn":{"id":"t","status":"completed"}}})).unwrap();
    // Future-thread model A has not changed, nor does the next turn inherit B.
    let q = configure(&mut a, "effort", ControlValue::Text("low".into()));
    a.ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    a.ingest(json!({"method":"thread/settings/updated","params":{"threadId":"s","threadSettings":{"model":"m","effort":"low"}}})).unwrap();
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"next"}}}))
        .unwrap();
    assert!(
        a.command(&active_change("t", "effort", "high"), "runtime")
            .is_err()
    );
    assert!(
        a.command(&active_change("next", "effort", "high"), "runtime")
            .is_err()
    );
    assert!(
        a.command(&active_change("next", "effort", "low"), "runtime")
            .is_ok()
    );
}

#[test]
fn review_r4_untrusted_policy_preserves_requirements_intersection() {
    for (allowed, expected) in [
        (json!(["untrusted"]), vec!["untrusted"]),
        (
            json!(["untrusted", "on-request"]),
            vec!["untrusted", "on-request"],
        ),
        (json!(["on-request"]), vec!["on-request"]),
        (Value::Null, vec!["untrusted", "on-request", "never"]),
    ] {
        let mut a = configured(Tool::Codex);
        let q = a
            .command(&Command::Discover(Discovery::Policy), "runtime")
            .unwrap();
        let request = q
            .writes
            .iter()
            .find(|v| v["method"] == "configRequirements/read")
            .unwrap();
        let e = a.ingest(json!({"id":request["id"],"result":{"requirements":{"allowedApprovalPolicies":allowed}}})).unwrap();
        let option = e
            .events
            .iter()
            .find_map(|e| match e {
                Event::Controls(ControlEvent::Options(v)) => {
                    v.iter().find(|o| o.id == "approvalPolicy")
                }
                _ => None,
            })
            .unwrap();
        assert!(option.available);
        assert_eq!(
            option
                .choices
                .iter()
                .map(|c| c.value.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let update = a.command(
            &Command::Configure {
                control: "approvalPolicy".into(),
                value: ControlValue::Text("untrusted".into()),
            },
            "runtime",
        );
        if expected.contains(&"untrusted") {
            assert_eq!(
                update.unwrap().writes[0]["params"]["approvalPolicy"],
                "untrusted"
            );
        } else {
            assert!(update.is_err());
        }
    }
}

#[test]
fn review_r5_hidden_models_use_consistent_first_and_next_page_filters() {
    for paginated in [false, true] {
        let mut a = Adapter::new(Tool::Codex, "/tmp".into(), None);
        a.launch_options(LaunchOptions::Codex(CodexLaunch {
            model: Some("hidden".into()),
            effort: Some("high".into()),
            service_tier: Some("priority".into()),
            ..Default::default()
        }));
        let init = a.ingest(json!({"id":"initialize","result":{}})).unwrap();
        let first = init
            .writes
            .iter()
            .find(|v| v["method"] == "model/list")
            .unwrap();
        assert_eq!(first["params"]["includeHidden"], true);
        if paginated {
            let page = a
                .ingest(json!({"id":"models","result":{"data":[],"nextCursor":"opaque"}}))
                .unwrap();
            assert_eq!(
                page.writes[0]["params"]["includeHidden"],
                first["params"]["includeHidden"]
            );
            assert_eq!(page.writes[0]["params"]["limit"], first["params"]["limit"]);
        }
        let e = a.ingest(json!({"id":"models","result":{"data":[{"id":"hidden","model":"hidden-native","hidden":true,"displayName":"Hidden","supportedReasoningEfforts":[{"reasoningEffort":"high"}],"defaultReasoningEffort":"high","serviceTiers":[{"id":"priority","name":"Fast"}]}]}})).unwrap();
        assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Models(v)) if v.iter().any(|m|m.id=="hidden"&&m.capabilities.get("hidden")==Some(&true)&&m.efforts.len()==1&&m.tiers.len()==1))));
        let start = e
            .writes
            .iter()
            .find(|v| v["method"] == "thread/start")
            .unwrap();
        assert_eq!(start["params"]["model"], "hidden-native");
        let e = a.ingest(json!({"id":"thread","result":{"thread":{"id":"s"},"model":"hidden-native","effort":"high"}})).unwrap();
        assert!(e.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Options(v)) if v.iter().any(|o|o.id=="effort"&&o.available&&o.choices.iter().any(|c|c.value=="high")))));
    }
}

#[test]
fn review_r1_claude_inflight_discovery_cannot_replace_later_model_ack() {
    let mut a = configured(Tool::Claude);
    let q = a
        .command(
            &Command::Submit {
                text: "explicit".into(),
                attachments: vec![],
                model: Some("other".into()),
            },
            "runtime",
        )
        .unwrap();
    let during = a
        .command(&Command::Discover(Discovery::Settings), "runtime")
        .unwrap();
    a.ingest(claude_reply(&q.writes[0]["request_id"], json!({})))
        .unwrap();
    let stale = a
        .ingest(claude_reply(
            &during.writes[0]["request_id"],
            json!({"applied":{"model":"opus"}}),
        ))
        .unwrap();
    assert!(stale.events.is_empty());
}

#[test]
fn review_r2_acp_notification_before_ack_and_mismatch_cannot_release_send() {
    let mut a = configured(Tool::Antigravity);
    let q = configure(&mut a, "thought_level", ControlValue::Text("high".into()));
    let snapshot = |value: &str| json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"thought_level","name":"Thought","type":"select","currentValue":value,"options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]}]}}});
    a.ingest(snapshot("high")).unwrap();
    assert!(submit(&mut a).is_err());
    a.ingest(json!({"id":q.writes[0]["id"],"result":{}}))
        .unwrap();
    let e = a.ingest(snapshot("low")).unwrap();
    assert!(e.events.iter().any(|e| matches!(e,Event::Controls(ControlEvent::Outcome(v)) if v.state == ApplicationState::Unconfirmed && !v.settled)));
    assert!(submit(&mut a).is_err());
    a.command(&Command::Interrupt, "runtime").unwrap();
    let e = a.ingest(snapshot("high")).unwrap();
    assert!(e.writes.is_empty());
    assert!(e.events.iter().any(|e| matches!(e,Event::Controls(ControlEvent::Outcome(v)) if v.state == ApplicationState::Cancelled && v.settled)));
    assert!(submit(&mut a).is_ok());
}

#[test]
fn review_r3_late_active_turn_ack_cannot_seed_next_turn() {
    let mut a = configured(Tool::Codex);
    a.ingest(json!({"id":"models","result":disjoint_models()}))
        .unwrap();
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"t"}}}))
        .unwrap();
    let q = a
        .command(&active_change("t", "model", "b"), "runtime")
        .unwrap();
    a.ingest(json!({"method":"turn/completed","params":{"threadId":"s","turn":{"id":"t","status":"completed"}}})).unwrap();
    let late = a
        .ingest(json!({"id":q.writes[0]["id"],"result":{"status":"applied"}}))
        .unwrap();
    assert!(!late.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Status(v)) if v.subject=="active_turn_settings")));
    submit(&mut a).unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"next"}}}))
        .unwrap();
    assert!(
        a.command(&active_change("next", "effort", "high"), "runtime")
            .is_err()
    );
    assert!(
        a.command(&active_change("next", "effort", "low"), "runtime")
            .is_ok()
    );
}

#[test]
fn review_r3_explicit_submit_model_is_active_turn_baseline() {
    let mut a = configured(Tool::Codex);
    a.ingest(json!({"id":"models","result":disjoint_models()}))
        .unwrap();
    a.command(
        &Command::Submit {
            text: "explicit".into(),
            attachments: vec![],
            model: Some("b".into()),
        },
        "runtime",
    )
    .unwrap();
    a.ingest(json!({"method":"turn/started","params":{"threadId":"s","turn":{"id":"t"}}}))
        .unwrap();
    a.ingest(json!({"id":"turn","result":{"turn":{"id":"t"}}}))
        .unwrap();
    assert!(
        a.command(&active_change("t", "effort", "low"), "runtime")
            .is_err()
    );
    assert!(
        a.command(&active_change("t", "effort", "high"), "runtime")
            .is_ok()
    );
}

#[test]
fn review_r1_discovery_from_retired_write_cannot_settle_after_its_ack() {
    let mut a = configured(Tool::Claude);
    let send = |model: &str| Command::Submit {
        text: "explicit".into(),
        attachments: vec![],
        model: Some(model.into()),
    };
    let q = a.command(&send("other"), "runtime").unwrap();
    a.command(&Command::Interrupt, "runtime").unwrap();
    let during = a
        .command(&Command::Discover(Discovery::Settings), "runtime")
        .unwrap();
    a.ingest(claude_reply(&q.writes[0]["request_id"], json!({})))
        .unwrap();
    let stale = a
        .ingest(claude_reply(
            &during.writes[0]["request_id"],
            json!({"applied":{"model":"opus"}}),
        ))
        .unwrap();
    assert!(stale.events.is_empty());
    let fresh = a.command(&send("opus"), "runtime").unwrap();
    assert_eq!(fresh.writes[0]["request"]["subtype"], "set_model");
    assert!(!fresh.writes.iter().any(|w| w["type"] == "user"));
}

#[test]
fn task6_acp_tool_metadata_preserves_actual_arguments_and_opaque_native_name() {
    let mut a=configured(Tool::Antigravity);
    let input=json!({"path":"fixture.txt","presentation":"domain-data"});
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"call","title":"Reading fixture.txt","name":"read_file","kind":"read","rawInput":input,"status":"in_progress"}}})).unwrap();
    let tool=e.events.iter().find_map(|e|if let Event::Tool(t)=e{Some(t)}else{None}).unwrap();
    assert_eq!(tool.name,"Reading fixture.txt");assert_eq!(tool.input,input);
    let meta=tool.presentation.as_ref().unwrap();assert_eq!(meta.kind.as_deref(),Some("read"));assert_eq!(meta.native_name.as_deref(),Some("read_file"));
    let e=a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"call","status":"completed"}}})).unwrap();
    assert!(e.events.iter().any(|e|matches!(e,Event::Tool(t) if t.presentation==tool.presentation && t.input==input)));
}

#[test]
fn task6_active_turn_identity_is_published_and_cleared_for_native_form() {
    let mut a=configured(Tool::Codex);
    let start=a.ingest(json!({"method":"turn/started","params":{"threadId":"s","turn":{"id":"turn-exact"}}})).unwrap();
    assert!(start.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Status(s)) if s.subject=="active_turn_settings" && s.fields.get("turnId")==Some(&ControlValue::Text("turn-exact".into())))));
    let end=a.ingest(json!({"method":"turn/completed","params":{"threadId":"s","turn":{"id":"turn-exact","status":"completed"}}})).unwrap();
    assert!(end.events.iter().any(|e|matches!(e,Event::Controls(ControlEvent::Status(s)) if s.subject=="active_turn_settings" && !s.fields.contains_key("turnId") && s.unavailable.is_some())));
}

#[test]
fn ux_claude_model_picker_retains_native_name_and_description() {
    let mut a = Adapter::new(Tool::Claude, "/tmp".into(), None);
    let e = a.ingest(claude_reply(&json!("initialize"), json!({"models":[{"value":"claude-fable-5-2[1m]","displayName":"Fable 5.2","description":"Native hardest tasks","resolvedModel":"claude-fable-5-2"}]}))).unwrap();
    let options = e.events.iter().filter_map(|e| if let Event::Controls(ControlEvent::Options(v))=e {Some(v)} else {None}).last().unwrap();
    let model = options.iter().find(|o|o.id=="model").unwrap();
    assert_eq!(model.choices[0].value,"claude-fable-5-2[1m]");
    assert_eq!(model.choices[0].label,"Fable 5.2");
    assert_eq!(model.choices[0].description,"Native hardest tasks");
}

#[test]
fn ux_native_permission_snapshot_populates_visible_current_mode() {
    let mut a=Adapter::new(Tool::Claude,"/tmp".into(),None);
    let e=a.ingest(claude_reply(&json!("initialize"),json!({"current_permission_mode":"auto","models":[]}))).unwrap();
    let options=e.events.iter().filter_map(|e|if let Event::Controls(ControlEvent::Options(v))=e{Some(v)}else{None}).last().unwrap();
    assert_eq!(options.iter().find(|o|o.id=="permissionMode").unwrap().current,Some(ControlValue::Text("auto".into())));
}

#[test]
fn inflight_claude_followup_receipt_is_not_an_earlier_response() {
    let mut a = configured(Tool::Claude);
    submit(&mut a).unwrap();
    a.ingest(json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"first"}}})).unwrap();
    let sent = submit(&mut a).unwrap();
    assert_eq!(sent.writes.len(), 1);
    let id = sent.writes[0]["uuid"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    let old = a.ingest(json!({"type":"assistant","message":{"id":"first","content":[]}})).unwrap();
    assert!(!old.accepted);
    let old_end = a.ingest(json!({"type":"result","queued_turn_count":1})).unwrap();
    assert!(!old_end.accepted);
    assert!(!old_end.events.iter().any(|e|matches!(e,Event::Lifecycle(Lifecycle::Ready))));
    let receipt = a.ingest(json!({"type":"user","uuid":id,"message":{"role":"user","content":"test"}})).unwrap();
    assert!(receipt.accepted);
    let end = a.ingest(json!({"type":"result","user_message_uuids":[id],"queued_turn_count":0})).unwrap();
    assert!(end.events.iter().any(|e|matches!(e,Event::Lifecycle(Lifecycle::Ready))));
}

#[test]
fn inflight_codex_steer_uses_exact_turn_and_rejection_keeps_worker_alive() {
    for reject in [false, true] {
        let mut a = configured(Tool::Codex);
        submit(&mut a).unwrap();
        a.ingest(json!({"id":"turn","result":{"turn":{"id":"active"}}})).unwrap();
        let sent = submit(&mut a).unwrap();
        assert_eq!(sent.writes.len(), 1);
        let wire = &sent.writes[0];
        assert_eq!(wire["method"], "turn/steer");
        assert_eq!(wire["params"]["expectedTurnId"], "active");
        assert!(wire["params"].get("model").is_none());
        let reply = if reject { json!({"id":wire["id"],"error":{"code":-32600,"message":"turn already ended"}}) }
            else { json!({"id":wire["id"],"result":{"turnId":"active"}}) };
        let receipt = a.ingest(reply).unwrap();
        assert_eq!(receipt.accepted, !reject);
        assert_eq!(receipt.cancelled, reject);
        assert!(!receipt.events.iter().any(|e|matches!(e,Event::Lifecycle(Lifecycle::Failed|Lifecycle::Disconnected|Lifecycle::Ready))));
    }
}

#[test]
fn inflight_acp_followup_waits_for_prompt_reply_without_cancelling_work() {
    let mut a = configured(Tool::Antigravity);
    submit(&mut a).unwrap();
    let queued = submit(&mut a).unwrap();
    assert!(queued.writes.is_empty());
    let old = a.ingest(json!({"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old"}}}})).unwrap();
    assert!(!old.accepted);
    let end = a.ingest(json!({"id":"prompt","result":{"stopReason":"end_turn"}})).unwrap();
    assert!(!end.accepted);
    assert_eq!(end.writes.len(), 1);
    assert_eq!(end.writes[0]["method"], "session/prompt");
    let next = a.ingest(json!({"id":"prompt","result":{"stopReason":"end_turn"}})).unwrap();
    assert!(next.accepted);
}

#[test]
fn inflight_model_effort_and_mode_configuration_does_not_interrupt() {
    for (tool, settings) in [
        (Tool::Claude, vec![("model", "opus"), ("permissionMode", "default")]),
        (Tool::Codex, vec![("model", "m"), ("effort", "ultra"), ("collaborationMode", "plan")]),
        (Tool::Antigravity, vec![("thought_level", "high")]),
    ] {
        for (key, value) in settings {
            let mut a = configured(tool);
            submit(&mut a).unwrap();
            let effects = configure(&mut a, key, ControlValue::Text(value.into()));
            assert_eq!(effects.writes.len(), 1, "{tool:?} {key}");
            assert!(!effects.cancelled);
            assert!(!effects.events.iter().any(|e|matches!(e,Event::Lifecycle(Lifecycle::Ready|Lifecycle::Stopped))));
            assert!(!effects.writes.iter().any(|w|w["method"]=="turn/interrupt" || w["request"]["subtype"]=="interrupt"));
        }
    }
}

#[test]
fn inflight_claude_initial_receipt_unlocks_followup_before_first_response_token() {
    let mut a = configured(Tool::Claude);
    let initial = submit(&mut a).unwrap();
    let id = initial.writes[0]["uuid"].clone();
    let receipt = a.ingest(json!({"type":"user","uuid":id,"message":{"content":"test"}})).unwrap();
    assert!(receipt.accepted);
    let followup = submit(&mut a).unwrap();
    assert_eq!(followup.writes[0]["type"], "user");
    assert_ne!(followup.writes[0]["uuid"], id);
}

#[test]
fn inflight_acp_legacy_mode_ack_keeps_the_response_running() {
    let mut a = Adapter::new(Tool::Antigravity, "/fixture".into(), None);
    a.ingest(json!({"id":"session","result":{"sessionId":"s","modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Ask"},{"id":"code","name":"Code"}]}}})).unwrap();
    submit(&mut a).unwrap();
    let change = a.command(&Command::SetMode{mode:"code".into()}, "runtime").unwrap();
    assert_eq!(change.writes[0]["method"], "session/set_mode");
    let receipt = a.ingest(json!({"id":change.writes[0]["id"],"result":{}})).unwrap();
    assert!(!receipt.events.iter().any(|e|matches!(e, Event::Lifecycle(_))));
    assert!(receipt.events.iter().any(|e|matches!(e, Event::Modes { selected:Some(mode), .. } if mode=="code")));
}

#[test]
fn claude_native_prompt_suggestions_are_display_data_and_clear_on_submission() {
    let mut adapter = configured(Tool::Claude);
    let effects = adapter.ingest(json!({"type":"prompt_suggestion","suggestion":"테스트도 실행해줘","uuid":"suggestion-1","session_id":"s"})).unwrap();
    assert!(effects.writes.is_empty(), "A suggestion never becomes a user prompt");
    assert!(effects.events.iter().any(|event| matches!(event,
        Event::Controls(ControlEvent::Status(status)) if status.subject=="prompt_suggestion"
        && status.fields.get("text")==Some(&ControlValue::Text("테스트도 실행해줘".into())))));
    let effects = submit(&mut adapter).unwrap();
    assert!(effects.events.iter().any(|event| matches!(event,
        Event::Controls(ControlEvent::Status(status)) if status.subject=="prompt_suggestion" && status.fields.is_empty())));
    let stale = adapter.ingest(json!({"type":"prompt_suggestion","suggestion":"오래된 추천","uuid":"stale","session_id":"s"})).unwrap();
    assert!(stale.events.is_empty(), "A late suggestion never appears while a newer turn is running");
}
