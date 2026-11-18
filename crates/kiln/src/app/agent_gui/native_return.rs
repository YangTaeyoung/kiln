//! A per-launch cooperative return channel for the native Claude frontend.
use super::*;

const SUBJECT: &str = "native_terminal_return";
#[derive(Serialize, Deserialize)]
struct State {
    protocol: u8,
    token: String,
    session: String,
    phase: String,
}
#[derive(Serialize)]
struct Request<'a> {
    protocol: u8,
    token: &'a str,
    session: &'a str,
}
pub(super) struct Bridge {
    root: PathBuf,
    plugin: PathBuf,
    token: String,
    requested: bool,
    available: bool,
}
impl Bridge {
    pub(super) fn prepare(
        tool: Tool,
        dir: &Path,
        command: &mut Process,
        journal: &mut Journal,
    ) -> Result<Option<Self>> {
        journal.push(Event::Controls(ControlEvent::Status(NativeStatus {
            subject: SUBJECT.into(),
            fields: [("automatic".into(), ControlValue::Bool(false))].into(),
            unavailable: None,
        })))?;
        if tool != Tool::Claude {
            return Ok(None);
        }
        let token = journal::unique();
        let root = dir.join("native-return").join(&token);
        let plugin = root.join("plugin");
        journal::private_dir(&plugin.join(".claude-plugin"))?;
        journal::private_dir(&plugin.join("hooks"))?;
        journal::atomic(
            &plugin.join(".claude-plugin/plugin.json"),
            &serde_json::to_vec(&serde_json::json!({
                "name": "kiln-native-return", "version": "1.0.0", "author": {"name":"Kiln"},
                "description": "Return an idle Kiln native conversation to its graphical view"
            }))?,
        )?;
        journal::atomic(
            &plugin.join("hooks/hooks.json"),
            br#"{"modules":["./register.js"]}"#,
        )?;
        journal::atomic(
            &plugin.join("hooks/register.js"),
            include_bytes!("native_return.js"),
        )?;
        command
            .arg("--plugin-dir")
            .arg(&plugin)
            .env("KILN_NATIVE_RETURN_TOKEN", &token);
        Ok(Some(Self {
            root,
            plugin,
            token,
            requested: false,
            available: false,
        }))
    }
    pub(super) fn poll(&mut self, journal: &mut Journal) -> Result<()> {
        let state = journal::read::<State>(&self.plugin.join("bridge.state")).ok();
        let state = state.filter(|state| {
            state.protocol == 1
                && state.token == self.token
                && journal.transcript.provider_session_id.as_deref() == Some(state.session.as_str())
                && matches!(
                    state.phase.as_str(),
                    "starting" | "working" | "draft" | "background" | "ready" | "closing"
                )
        });
        let available = state.is_some_and(|state| state.phase != "starting");
        if available != self.available {
            journal.push(Event::Controls(ControlEvent::Status(NativeStatus {
                subject: SUBJECT.into(),
                fields: [("automatic".into(), ControlValue::Bool(available))].into(),
                unavailable: None,
            })))?;
            self.available = available;
        }
        let requested = journal.transcript.return_requested && available;
        if requested != self.requested {
            let path = self.plugin.join("return.request");
            if requested {
                let session = journal
                    .transcript
                    .provider_session_id
                    .as_deref()
                    .context("missing native return session")?;
                journal::atomic(
                    &path,
                    &serde_json::to_vec(&Request {
                        protocol: 1,
                        token: &self.token,
                        session,
                    })?,
                )?;
            } else if path.exists() {
                fs::remove_file(path)?;
            }
            self.requested = requested;
        }
        Ok(())
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    #[test]
    fn native_return_requires_matching_launch_and_session_and_cancels_only_its_request() {
        let root = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(root.path(), "generation".into()).unwrap();
        journal.push(Event::Generation).unwrap();
        journal.push(Event::Session(SESSION.into())).unwrap();
        journal
            .push(Event::Presentation {
                current: Presentation::Terminal,
                return_requested: true,
            })
            .unwrap();
        let mut command = Process::new("owned-fixture");
        let mut bridge = Bridge::prepare(Tool::Claude, root.path(), &mut command, &mut journal)
            .unwrap()
            .unwrap();
        assert!(command.get_args().any(|arg| arg == "--plugin-dir"));
        let state_path = bridge.plugin.join("bridge.state");
        let request_path = bridge.plugin.join("return.request");
        for (token, session) in [
            ("stale-launch".to_owned(), SESSION.to_owned()),
            (bridge.token.clone(), "other-session".to_owned()),
        ] {
            journal::atomic(
                &state_path,
                &serde_json::to_vec(&State {
                    protocol: 1,
                    token,
                    session,
                    phase: "ready".into(),
                })
                .unwrap(),
            )
            .unwrap();
            bridge.poll(&mut journal).unwrap();
            assert!(
                !request_path.exists(),
                "A stale launch or another conversation never receives an exit request"
            );
        }
        journal::atomic(
            &state_path,
            &serde_json::to_vec(&State {
                protocol: 1,
                token: bridge.token.clone(),
                session: SESSION.into(),
                phase: "starting".into(),
            })
            .unwrap(),
        )
        .unwrap();
        bridge.poll(&mut journal).unwrap();
        assert!(
            !request_path.exists(),
            "Startup is not proof of a compatible native idle API"
        );
        journal::atomic(
            &state_path,
            &serde_json::to_vec(&State {
                protocol: 1,
                token: bridge.token.clone(),
                session: SESSION.into(),
                phase: "ready".into(),
            })
            .unwrap(),
        )
        .unwrap();
        bridge.poll(&mut journal).unwrap();
        let request: serde_json::Value = journal::read(&request_path).unwrap();
        assert_eq!(request["session"], SESSION);
        assert_eq!(request["token"], bridge.token);
        assert!(matches!(
            journal.transcript.controls.status[SUBJECT].fields["automatic"],
            ControlValue::Bool(true)
        ));
        journal
            .push(Event::Presentation {
                current: Presentation::Terminal,
                return_requested: false,
            })
            .unwrap();
        bridge.poll(&mut journal).unwrap();
        assert!(
            !request_path.exists(),
            "Selecting TUI cancels a deferred return request"
        );
        let owned = bridge.root.clone();
        let sibling = root.path().join("preserve-user-data");
        fs::write(&sibling, b"unchanged").unwrap();
        drop(bridge);
        assert!(!owned.exists());
        assert_eq!(fs::read(sibling).unwrap(), b"unchanged");
    }
}
