//! Stop both in-memory readers, settle native history, then reopen only the winner.
use kiln_common::paths::Canonical;
use super::*;
use conversation_sync::handover;
use std::fs::File;
mod reader;

#[derive(Default)]
pub(super) struct Transfers {
    next: Option<Instant>,
    reader: Option<reader::Reader>,
    leases: HashMap<PathBuf, WriterLease>,
    jobs: HashMap<String, Job>,
}
struct WriterLease {
    _file: File,
    pid: Option<u32>,
}
fn record_matches(path: &Path, record: &agent_runtime::Record, generation: &str, owners: &handover::Owners) -> bool {
    record.generation == generation || !record.confirmed && owners.values().any(|owner| {
        !owner.ready && owner.record == path && owner.generation == generation
            && record.conversation.as_deref() == Some(&owner.canonical_id)
    })
}
fn writer_may_be_running(exited: Option<bool>, pid: Option<u32>) -> bool {
    if let Some(exited) = exited {
        return !exited;
    }
    // Closing a pane removes its connection metadata before the daemon has
    // finished stopping the CLI. Keep the lease until that writer is gone.
    // A zero signal only checks existence; EPERM and unknown identity stay locked.
    #[cfg(unix)]
    {
        let Some(pid) = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 1)
        else {
            return true;
        };
        return unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
    }
    // Only an unknown process ID or a recorded exit code proves exit; access errors stay locked.
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let Some(pid) = pid.filter(|pid| *pid > 4) else {
            return true;
        };
        // SAFETY: the handle is checked before use and closed once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let queried = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            !queried || code == STILL_ACTIVE as u32
        }
    }
}

struct Job {
    sequence: u128,
    started: Instant,
    tries: u8,
    scan: Option<u64>,
    warned: bool,
}

impl KilnApp {
    /// Runs every 250 ms while a transfer is in progress and every second otherwise.
    pub(super) fn tick_agent_handover(&mut self, ctx: &egui::Context) {
        if !self.conn.is_connected() { return; }
        let transferring = !self.agent_handover.jobs.is_empty();
        let period = if transferring { Duration::from_millis(250) } else { Duration::from_secs(1) };
        ctx.request_repaint_after(period);
        let config = kiln_common::paths::config_dir();
        let reader = self.agent_handover.reader.get_or_insert_with(|| reader::Reader::new(ctx.clone()));
        let response = reader.poll();
        if self.agent_handover.next.is_none_or(|next| Instant::now() >= next) {
            let mut request = reader::Request { config: config.clone(), enabled: self.settings.conversation_sync_enabled, ..Default::default() };
            for (pane, spec) in &self.launch_specs {
                let Some(info) = self.panes.get(pane).and_then(|p| p.session()).and_then(|session| self.conn.infos.get(&session)) else { continue; };
                let Some(path) = agent_runtime::record_path(spec) else { continue; };
                request.paths.push(path.clone());
                if info.exited.is_none() && !self.agent_handover.leases.contains_key(&path) {
                    request.leases.push((path, (info.pid > 1).then_some(info.pid)));
                }
            }
            if reader.request(request) { self.agent_handover.next = Some(Instant::now() + period); }
        }
        let Some(response) = response else { return; };
        for (path, lease) in response.leases { self.agent_handover.leases.entry(path).or_insert(lease); }
        let records: Vec<_> = self
            .launch_specs
            .iter()
            .filter_map(|(pane, spec)| {
                let session = self.panes.get(pane)?.session()?;
                let path = agent_runtime::record_path(spec)?;
                let record = response.records.get(&path)?.clone();
                let (_, generation) = agent_runtime::launch_of(spec)?;
                if !record_matches(&path, &record, &generation, &response.owners) { return None; }
                let exited = self
                    .conn
                    .infos
                    .get(&session)
                    .is_some_and(|i| i.exited.is_some());
                Some((*pane, session, path, record, exited))
            })
            .collect();
        // Hold until the session daemon confirms exit, including for older CLIs.
        self.agent_handover.leases.retain(|path, lease| {
            let exited = records
                .iter()
                .find(|(_, _, p, _, _)| p == path)
                .map(|(_, _, _, _, exited)| *exited);
            writer_may_be_running(exited, lease.pid)
        });
        for (_, session, path, _, _) in &records {
            if let Some(lease) = self.agent_handover.leases.get_mut(path)
                && let Some(info) = self.conn.infos.get(session)
                && info.pid > 1
            {
                lease.pid = Some(info.pid);
            }
        }
        if !self.settings.conversation_sync_enabled {
            return;
        }
        if !response.valid { return; }
        let (links, owners) = (response.links, response.owners);
        self.agent_handover.jobs.retain(|group, job| {
            owners
                .get(group)
                .is_some_and(|o| !o.ready && o.sequence == job.sequence)
        });
        for (group, owner) in owners.iter().filter(|(_, o)| !o.ready) {
            let Some((winner, winner_session, _, winner_record, _)) = records
                .iter()
                .find(|(_, _, path, record, _)| handover::pending_matches(owner, path, record))
            else {
                continue;
            };
            // Only opted-in, currently open workspace conversations are eligible.
            if !self.workspaces.iter().any(|w| {
                w.conversation_sync
                    && winner_record.spec.cwd.as_ref().is_some_and(|cwd| {
                        Path::new(cwd)
                            .canonical()
                            .is_ok_and(|cwd| cwd.starts_with(w.tools.canonical_root()))
                    })
            }) {
                continue;
            }
            let participants: Vec<_> = records
                .iter()
                .filter(|(_, _, _, record, _)| {
                    handover::link(&links, record).is_some_and(|l| &l.group == group)
                })
                .collect();
            let fresh = !self.agent_handover.jobs.contains_key(group);
            let job = self
                .agent_handover
                .jobs
                .entry(group.clone())
                .or_insert(Job {
                    sequence: owner.sequence,
                    started: Instant::now(),
                    tries: 0,
                    scan: None,
                    warned: false,
                });
            let launchable = self
                .rotator
                .mgr
                .validate_launch_environment(winner_record.tool, &winner_record.spec.env)
                .and_then(|_| agent_runtime::resume(&owner.record).map(|_| ()));
            if let Err(error) = launchable {
                if !job.warned {
                    job.warned = true;
                    self.toast(
                        kiln_common::i18n::tr("대화 전환 확인 필요"),
                        error.to_string(),
                        ToastKind::Error,
                        Some(*winner_session),
                    );
                }
                continue;
            }
            let all_exited = participants.iter().all(|(_, _, _, _, exited)| *exited);
            let mut warn = false;
            if !all_exited {
                if job.tries < 3
                    && (job.tries == 0
                        || job.started.elapsed() >= Duration::from_secs(u64::from(job.tries) * 2))
                {
                    for (pane, session, _, _, exited) in &participants {
                        self.capacity_recovery.cancel(*pane);
                        self.cli_restarts.remove(pane);
                        self.cli_confirmations.remove(pane);
                        if !exited {
                            self.conn.input(*session, vec![3]);
                        }
                    }
                    job.tries += 1;
                }
                if job.started.elapsed() > Duration::from_secs(15) && !job.warned {
                    job.warned = true;
                    warn = true;
                }
            }
            if fresh {
                self.toast(
                    kiln_common::i18n::tr("대화 전환 중"),
                    kiln_common::i18n::tr(
                        "기존 에이전트를 중지하고 마지막 대화를 동기화한 뒤 다시 엽니다.",
                    ),
                    ToastKind::Info,
                    Some(*winner_session),
                );
            }
            if warn {
                self.toast(kiln_common::i18n::tr("대화 전환 대기"), kiln_common::i18n::tr("기존 CLI가 아직 종료되지 않았습니다. 해당 패널을 확인하세요. 새 작업은 대기 중입니다."), ToastKind::Error, Some(*winner_session));
            }
            if !all_exited {
                continue;
            }
            let job = self.agent_handover.jobs.get_mut(group).unwrap();
            if job.scan.is_none() {
                job.scan = self.conversation_sync.refresh_for_handover();
                continue;
            }
            if !self
                .conversation_sync
                .handover_ready(job.scan.unwrap(), group)
            {
                if job.started.elapsed() > Duration::from_secs(30) && !job.warned {
                    job.warned = true;
                    self.toast(kiln_common::i18n::tr("대화 동기화 대기"), kiln_common::i18n::tr("정본 확인이 끝나지 않아 재개를 대기합니다. 워크스페이스의 대화 연동 상태에서 원인을 확인하세요."), ToastKind::Error, Some(*winner_session));
                }
                continue;
            }
            let result = (|| -> anyhow::Result<SpawnSpec> {
                self.rotator
                    .mgr
                    .validate_launch_environment(winner_record.tool, &winner_record.spec.env)?;
                // A failed grant can leave a renewed, unconfirmed record. Resume
                // the same pending transfer instead of stranding that generation.
                agent_runtime::redirect(
                    &owner.record,
                    &winner_record.generation,
                    &owner.canonical_id,
                )?;
                let spec = agent_runtime::renew(&owner.record)?;
                let renewed = agent_runtime::read(&owner.record)?;
                handover::grant(&config, group, owner.sequence, &owner.record, &renewed)?;
                Ok(spec)
            })();
            match result {
                Ok(spec) => {
                    if !self.conn.kill(*winner_session) {
                        continue;
                    }
                    if let Some(pane) = self.panes.get_mut(winner) {
                        pane.kind = PaneKind::Term {
                            session: None,
                            pending: None,
                            view: None,
                        };
                    }
                    self.launch_specs.insert(*winner, spec);
                    self.cli_confirmations.insert(*winner, Instant::now());
                    self.spawn_for_pane(*winner);
                    self.agent_handover.jobs.remove(group);
                    self.toast(kiln_common::i18n::tr("대화 재개 중"), kiln_common::i18n::tr("이전 에이전트를 중지했습니다. 동기화된 정본으로 새 에이전트를 여는 중입니다."), ToastKind::Info, None);
                }
                Err(error) => {
                    let job = self.agent_handover.jobs.get_mut(group).unwrap();
                    if !job.warned {
                        job.warned = true;
                        self.toast(
                            kiln_common::i18n::tr("대화 전환 확인 필요"),
                            error.to_string(),
                            ToastKind::Error,
                            Some(*winner_session),
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_pane_does_not_release_a_live_native_writer() {
        #[cfg(unix)]
        let mut writer = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut writer = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep 30"])
            .spawn()
            .unwrap();
        let pid = writer.id();
        let alive = writer_may_be_running(None, Some(pid));
        writer.kill().unwrap();
        writer.wait().unwrap();
        assert!(
            alive,
            "a removed pane still owns its transcript until its CLI exits"
        );
        assert!(!writer_may_be_running(None, Some(pid)));
        assert!(
            writer_may_be_running(None, None),
            "missing process identity is not proof of exit"
        );
        assert!(!writer_may_be_running(Some(true), None));
    }
    #[test]
    fn failed_grant_can_retry_renewed_record_without_accepting_old_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path();
        let path = config.join("agent-sessions/owner.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let id = "11111111-1234-1234-1234-123456789abc";
        let record = agent_runtime::Record { tool: kiln_accounts::Tool::Codex,
            spec: SpawnSpec::default(), version: String::new(), conversation: Some(id.into()),
            generation: "launch".into(), confirmed: true, request_started: false };
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let owners = handover::Owners::from([("group".into(), handover::Owner {
            record: path.clone(), generation: "launch".into(), id: id.into(), canonical_id: id.into(), ready: false, sequence: 1,
        })]);
        std::fs::create_dir_all(config.join("conversation-sync")).unwrap();
        std::fs::write(config.join("conversation-sync/owners.json"), serde_json::to_vec(&owners).unwrap()).unwrap();
        agent_runtime::renew(&path).unwrap();
        let renewed = agent_runtime::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(config.join("conversation-sync/ownership.lock")).unwrap();
        lock.try_lock().unwrap();
        assert!(handover::grant(config, "group", 1, &path, &renewed).is_err());
        drop(lock);
        assert!(record_matches(&path, &renewed, "launch", &owners), "failed grant must retain the pending renewal retry path");
        assert!(!record_matches(&path, &renewed, "newer-launch", &owners));
        handover::grant(config, "group", 1, &path, &renewed).unwrap();
        assert!(handover::owners(config).unwrap()["group"].ready);
    }

}
