//! Exclusive native presentation handoff. The daemon PTY and host lock never change.
use super::*;

fn valid_native_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub(super) fn can_handoff(transcript: &Transcript, active_submission: bool) -> bool {
    transcript.lifecycle == Lifecycle::Ready
        && transcript.activity.live().next().is_none()
        && transcript.requests.is_empty()
        && !active_submission
        && !transcript.entries.iter().any(|entry|matches!(entry, Entry::Tool(tool) if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)))
        && transcript.controls.queue.as_ref().is_none_or(|queue|queue.is_empty())
        && transcript.provider_session_id.as_deref().is_some_and(valid_native_id)
}

fn native_args(tool: Tool, id: &str) -> Result<Vec<String>> {
    ensure!(
        valid_native_id(id),
        "A confirmed native conversation UUID is required"
    );
    Ok(match tool {
        Tool::Claude => vec!["--resume".into(), id.into()],
        Tool::Codex => vec!["resume".into(), id.into(), "--no-daemon".into()],
        Tool::Antigravity => vec!["--conversation".into(), id.into()],
    })
}

struct TerminalOwnership;
impl TerminalOwnership {
    fn claim() -> Self {
        TERMINAL_ACTIVE.store(true, Ordering::Relaxed);
        Self
    }
}
impl Drop for TerminalOwnership {
    fn drop(&mut self) {
        TERMINAL_ACTIVE.store(false, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryCheckpoint {
    count: usize,
    digest: String,
}
pub(super) fn has_user_history(transcript: &Transcript) -> bool {
    transcript.entries.iter().any(|entry| matches!(entry, Entry::Message(message) if message.role == Role::User))
}
pub(super) fn optional_history(config: &Config, id: &str) -> Result<Option<Vec<(String,String)>>> {
    super::super::super::conversation_sync::optional_native_display_history(config.descriptor.tool, &config.environment, &config.descriptor.cwd, id)
}
pub(super) fn fresh_claude_id() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes=[0u8;16];ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_|anyhow::anyhow!("Could not allocate native session identity"))?;
    bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;
    let hex:String=bytes.iter().map(|b|format!("{b:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}",&hex[..8],&hex[8..12],&hex[12..16],&hex[16..20],&hex[20..]))
}
fn history_digest(messages: &[(String, String)]) -> Result<String> {
    Ok(super::super::image_digest(&serde_json::to_vec(messages)?))
}
fn import_terminal_history(
    config: &Config,
    id: &str,
    checkpoint: &HistoryCheckpoint,
    journal: &mut Journal,
) -> Result<()> {
    let messages = match optional_history(config, id)? {
        Some(messages) => messages,
        None if checkpoint.count == 0 && !has_user_history(&journal.transcript) => vec![],
        None => anyhow::bail!("KILN_HANDOFF_HISTORY"),
    };
    ensure!(
        messages.len() >= checkpoint.count
            && history_digest(&messages[..checkpoint.count])? == checkpoint.digest,
        "Native history changed before the handoff boundary; retained GUI history was preserved"
    );
    for (index, (role, text)) in messages.into_iter().enumerate().skip(checkpoint.count) {
        journal.push(Event::Message {
            message: Message {
                id: format!("native-terminal-{id}-{index}"),
                role: if role == "user" {
                    Role::User
                } else {
                    Role::Assistant
                },
                text,
                attachments: vec![],
            },
            append: false,
        })?;
    }
    Ok(())
}

/// The child inherits the real PTY. A native extension requests its documented
/// idle exit; the host waits for actual exit before starting another writer.
pub(super) fn serve_terminal(
    config: &Config,
    accounts: &AccountManager,
    dir: &Path,
    group: &OwnedGroup,
    journal: &mut Journal,
) -> Result<Option<i32>> {
    let bootstrap = config.shell.as_ref().filter(|_| !dir.join("initial-intent").exists());
    let mut shell_exit = None;
    let result = (|| -> Result<()> {
        if let Some(shell) = bootstrap {
            ensure!(claim_initial(dir)?, "The initial native launch was already attempted");
            let spec = &shell.initial_spec;
            let (program, lead) = super::super::super::agent_launch::launcher(Path::new(spec.program.as_deref().context("missing shell launcher")?));
            let mut command = Process::new(program);
            command.args(lead).args(&spec.args).envs(spec.env.iter().cloned());
            if let Some(cwd) = &spec.cwd { command.current_dir(cwd); }
            command.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
            let mut return_bridge = super::native_return::Bridge::prepare(config.descriptor.tool, dir, &mut command, journal, super::native_return::Target::Managed)?;
            let _ownership = TerminalOwnership::claim();
            let mut child = command.spawn().context("Could not open the native shell conversation")?;
            let mut observed = Instant::now() - Duration::from_secs(1);
            let mut observation_error = None;
            let mut antigravity = super::super::super::agent_runtime::Observer::default();
            loop {
                if TERMINATE.load(Ordering::Relaxed) { group.terminate(journal)?; return Ok(()); }
                if config.descriptor.tool == Tool::Antigravity && antigravity.due() {
                    antigravity.tick(vec![(shell.record_path.clone(), shell.record_generation.clone(), child.id())]);
                }
                if observed.elapsed() >= Duration::from_millis(150) {
                    if let Err(error) = refresh_shell_identity(config, journal) { observation_error = Some(error.to_string()); }
                    observed = Instant::now();
                }
                if let Err(error) = poll_terminal_commands(dir, journal) { observation_error = Some(error.to_string()); }
                if let Some(bridge) = &mut return_bridge && let Err(error) = bridge.poll(journal) { observation_error = Some(error.to_string()); }
                let status = match child.try_wait() {
                    Ok(status) => status,
                    Err(error) => { observation_error = Some(error.to_string()); std::thread::sleep(Duration::from_millis(100)); continue; }
                };
                if let Some(status) = status {
                    refresh_shell_identity(config, journal)?;
                    if !journal.transcript.return_requested {
                        shell_exit = Some(status.code().unwrap_or(1));
                        journal.push(Event::Lifecycle(Lifecycle::Stopped))?;
                        journal.flush()?;
                        return Ok(());
                    }
                    if let Some(error) = observation_error { journal.push(Event::Error(error))?; }
                    ensure!(group.handoff_children_exited(), "KILN_HANDOFF_HELPERS");
                    let id = resume_target(&journal.transcript)?.context("KILN_HANDOFF_HISTORY")?;
                    let checkpoint = HistoryCheckpoint {count:0,digest:history_digest(&[])?};
                    import_terminal_history(config,&id,&checkpoint,journal)?;
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }
        let id =
            resume_target(&journal.transcript)?.context("Missing native conversation identity")?;
        let mut command = base_command(config, accounts)?;
        let before = match optional_history(config, &id)? {
            Some(messages) => Some(messages),
            None if !has_user_history(&journal.transcript) => None,
            None => anyhow::bail!("KILN_HANDOFF_HISTORY"),
        };
        if config.descriptor.tool == Tool::Claude && before.is_none() {
            command.args(["--session-id", &id]);
        } else { command.args(native_args(config.descriptor.tool, &id)?); }
        let before = before.unwrap_or_default();
        let checkpoint = HistoryCheckpoint {
            count: before.len(),
            digest: history_digest(&before)?,
        };
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let mut return_bridge = super::native_return::Bridge::prepare(config.descriptor.tool, dir, &mut command, journal, super::native_return::Target::Direct)?;
        let _ownership = TerminalOwnership::claim();
        let mut child = command
            .spawn()
            .context("Could not open the native conversation terminal")?;
        let mut transport_error = journal.push(cli_frontend_status(true))
            .err()
            .map(|error| error.to_string());
        loop {
            if TERMINATE.load(Ordering::Relaxed) {
                group.terminate(journal)?;
                return Ok(());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    journal.push(cli_frontend_status(false))?;
                    if !status.success() {
                        journal.push(Event::Error(format!(
                            "Native terminal exited ({status}); resuming the saved conversation"
                        )))?;
                    }
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    // Failed observation is never proof that the native writer
                    // exited. Keep ownership and do not start a second backend.
                    transport_error = Some(error.to_string());
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
            if let Err(error) = poll_terminal_commands(dir, journal) {
                // Storage errors leave the real CLI running. We only reconnect
                // the structured backend after the CLI reports a natural exit.
                transport_error = Some(error.to_string());
            }
            if let Some(bridge) = &mut return_bridge && let Err(error) = bridge.poll(journal) { transport_error = Some(error.to_string()); }
            std::thread::sleep(Duration::from_millis(30));
        }
        if let Some(error) = transport_error {
            journal.push(Event::Error(error))?;
        }
        import_terminal_history(config, &id, &checkpoint, journal)?;
        Ok(())
    })();
    if shell_exit.is_some() { return Ok(shell_exit); }
    // A spawn failure also returns to the structured view, with a visible error.
    if let Err(error) = result {
        journal.push(Event::Error(error.to_string()))?;
    }
    journal.generation = journal::unique();
    journal.push(Event::Generation)?;
    journal.push(Event::Presentation {
        current: Presentation::Gui,
        return_requested: false,
    })?;
    journal.flush()?;
    Ok(None)
}

fn cli_frontend_status(running: bool) -> Event {
    Event::Controls(ControlEvent::Status(NativeStatus {
        subject: "native_cli_frontend".into(),
        fields: [("running".into(), ControlValue::Bool(running))].into(),
        unavailable: None,
    }))
}

fn refresh_shell_identity(config: &Config, journal: &mut Journal) -> Result<()> {
    let shell = config.shell.as_ref().context("missing shell launch metadata")?;
    let Ok(record) = super::super::super::agent_runtime::read(&shell.record_path) else { return Ok(()); };
    ensure!(record.generation == shell.record_generation && record.tool == config.descriptor.tool,
        "Shell conversation identity changed unexpectedly");
    if record.confirmed && let Some(id) = record.conversation.as_deref().filter(|id|valid_native_id(id))
        && journal.transcript.provider_session_id.as_deref() != Some(id) {
        journal.push(Event::Session(id.into()))?;
        if journal.transcript.lifecycle == Lifecycle::Starting { journal.push(Event::Lifecycle(Lifecycle::Ready))?; }
    }
    Ok(())
}

fn poll_terminal_commands(dir: &Path, journal: &mut Journal) -> Result<()> {
    for (path, queued) in mailbox(dir)? {
        if claim_command(journal, &queued)? {
            let accepted = if let Command::SetPresentation(target) = queued.command {
                journal.push(Event::Presentation {
                    current: Presentation::Terminal,
                    return_requested: target == Presentation::Gui,
                })?;
                true
            } else {
                false
            };
            journal.push(Event::CommandState {
                id: queued.id,
                state: if accepted {
                    CommandPhase::Accepted
                } else {
                    CommandPhase::Rejected
                },
            })?;
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    #[test]
    fn only_idle_confirmed_native_conversations_can_handoff() {
        let mut transcript = Transcript::default();
        transcript.lifecycle = Lifecycle::Ready;
        assert!(!can_handoff(&transcript, false));
        transcript.provider_session_id = Some(ID.into());
        assert!(can_handoff(&transcript, false));
        assert!(!can_handoff(&transcript, true));
        for lifecycle in [
            Lifecycle::Running,
            Lifecycle::Waiting,
            Lifecycle::Disconnected,
            Lifecycle::Starting,
        ] {
            transcript.lifecycle = lifecycle;
            assert!(!can_handoff(&transcript, false));
        }
        transcript.lifecycle = Lifecycle::Ready;
        transcript.requests.insert(
            "approval".into(),
            Request {
                id: "approval".into(),
                title: String::new(),
                questions: vec![],
                options: vec![],
                details: Value::Null,
            },
        );
        assert!(!can_handoff(&transcript, false));
        transcript.requests.clear();
        transcript.entries.push(Entry::Tool(ToolCall {
            status: ToolStatus::Running,
            ..Default::default()
        }));
        assert!(!can_handoff(&transcript, false));
    }
    #[test]
    fn exact_resume_has_no_prompt_or_latest_session_fallback() {
        assert_eq!(native_args(Tool::Claude, ID).unwrap(), ["--resume", ID]);
        assert_eq!(
            native_args(Tool::Codex, ID).unwrap(),
            ["resume", ID, "--no-daemon"]
        );
        assert_eq!(
            native_args(Tool::Antigravity, ID).unwrap(),
            ["--conversation", ID]
        );
        for id in ["", "latest", "../other", "native\ncommand"] {
            assert!(native_args(Tool::Claude, id).is_err());
        }
    }
    #[test]
    fn native_handoff_uses_original_account_cwd_and_executable_after_account_selection_changes() {
        use kiln_accounts::connection::{Connection, Preset};
        for tool in [Tool::Claude, Tool::Codex] {
            let root = tempfile::tempdir().unwrap();
            let (environment, _) = kiln_accounts::Env::sandbox(root.path(), false);
            let accounts = AccountManager::with_env(environment);
            let connection = |key: &str| Connection {
                preset: Preset::Gateway,
                base_url: "https://fixture.invalid/v1".into(),
                key: key.into(),
                ..Default::default()
            };
            let original = accounts
                .save_connection(tool, None, "Original", connection("fixture-original"))
                .unwrap();
            let other = accounts
                .save_connection(tool, None, "Other", connection("fixture-other"))
                .unwrap();
            accounts.switch_to(tool, &original.id).unwrap();
            let pinned = accounts.launch_environment(tool).unwrap();
            accounts.switch_to(tool, &other.id).unwrap();
            let config = Config {
                descriptor: Descriptor {
                    id: "fixture".into(),
                    tool,
                    cwd: root.path().into(),
                    account_label: "Original".into(),
                },
                program: "original-executable".into(),
                environment: pinned,
                initial_prompt: None,
                launch_options: LaunchOptions::Inherited,
            shell: None,
            };
            let mut command = base_command(&config, &accounts).unwrap();
            command.args(native_args(tool, ID).unwrap());
            assert_eq!(command.get_current_dir(), Some(root.path()));
            assert_eq!(command.get_program(), "original-executable");
            let key = if tool == Tool::Claude {
                "ANTHROPIC_AUTH_TOKEN"
            } else {
                "KILN_CODEX_API_KEY"
            };
            assert!(command.get_envs().any(|(name, value)| name == key
                && value == Some(std::ffi::OsStr::new("fixture-original"))));
            assert!(
                !command
                    .get_envs()
                    .any(|(_, value)| value == Some(std::ffi::OsStr::new("fixture-other")))
            );
            assert_eq!(accounts.active(tool).as_deref(), Some(other.id.as_str()));
            assert!(command.get_args().any(|argument| argument == ID));
            assert!(!command.get_args().any(|argument| argument == "--last"
                || argument == "-p"
                || argument == "app-server"));
        }
    }
    #[test]
    fn journal_replay_keeps_terminal_and_return_request_across_reconnect() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(root.path(), "owned".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        journal.push(Event::Session(ID.into())).unwrap();
        journal
            .push(Event::Presentation {
                current: Presentation::Terminal,
                return_requested: true,
            })
            .unwrap();
        journal.flush().unwrap();
        drop(journal);
        let journal = Journal::open(root.path(), "owned".into()).unwrap();
        assert_eq!(journal.transcript.presentation, Presentation::Terminal);
        assert!(journal.transcript.return_requested);
        assert_eq!(journal.transcript.provider_session_id.as_deref(), Some(ID));
    }
}
