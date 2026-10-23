//! 에이전트 CLI(claude, codex) 사용량 한도 감지와 계정 전환 후 세션 이어가기.

use super::conn::Conn;
use kiln_accounts::{AccountManager, ResetOutcome, Tool, detect_limit, resume_command};
use kiln_proto::SessionId;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// 앱에 알릴 일.
pub enum RotationEvent {
    /// 한도에 걸렸고 자동 전환이 꺼져 있다(사용자가 전환할지 고른다).
    LimitReached { session: SessionId, tool: Tool, reset_hint: Option<String> },
    ManagedLimitReached { session: SessionId, tool: Tool },
    Switched { session: SessionId, tool: Tool, label: String },
    NoAccount { tool: Tool },
    Failed { tool: Tool, error: String },
    /// A reset credit of the account labelled `label` was redeemed with `outcome`.
    ResetRedeemed { label: String, outcome: ResetOutcome },
    ResetFailed { error: String },
}

enum Stage {
    /// 백그라운드에서 자격증명을 바꾸는 중.
    Switching,
    /// 에이전트를 끝내는 중(Ctrl+C 를 보냈다).
    Stopping { since: Instant, tries: u8 },
}

struct Job {
    session: SessionId,
    tool: Tool,
    stage: Stage,
    label: String,
    started: Instant,
}

pub struct Rotator {
    pub mgr: AccountManager,
    last_poll: Instant,
    /// 같은 한도 메시지로 반복 처리하지 않게 세션별 마지막 처리 시각.
    handled: HashMap<SessionId, Instant>,
    jobs: Vec<Job>,
    managed: HashSet<SessionId>,
    tx: Sender<(SessionId, Tool, Result<Option<String>, String>)>,
    rx: Receiver<(SessionId, Tool, Result<Option<String>, String>)>,
    redeeming: bool,
    resets: (Sender<RotationEvent>, Receiver<RotationEvent>),
}

pub fn tool_for(process: &str) -> Option<Tool> {
    match process {
        "claude" => Some(Tool::Claude),
        "codex" => Some(Tool::Codex),
        "agy" => Some(Tool::Antigravity),
        _ => None,
    }
}

fn is_shell(p: &str) -> bool {
    super::shells().contains(&p)
}

impl Rotator {
    pub fn new(mgr: AccountManager) -> Self {
        let (tx, rx) = channel();
        Rotator { mgr, last_poll: Instant::now(), handled: HashMap::new(), jobs: Vec::new(), managed: HashSet::new(), tx, rx, redeeming: false, resets: channel() }
    }

    /// The reset credits the active `tool` account can redeem when no other account has room:
    /// Codex only, from its latest usage sample.
    pub fn reset_offer(&self, tool: Tool) -> Option<u32> {
        if tool != Tool::Codex || self.mgr.next_with_headroom(tool).is_some() { return None; }
        let active = self.mgr.active(tool)?;
        let usage = self.mgr.usage(tool, &active).filter(|usage| !usage.is_unavailable())?;
        usage.resets.map(|resets| resets.available).filter(|available| *available > 0)
    }

    /// Redeems one reset credit of the active `tool` account in the background; a redemption
    /// already running is not started again.
    pub fn redeem(&mut self, tool: Tool) {
        if self.redeeming { return; }
        let Some(id) = self.mgr.active(tool) else { return };
        let label = self.mgr.profiles(tool).into_iter().find(|profile| profile.id == id).map(|profile| profile.label).unwrap_or_default();
        self.redeeming = true;
        let mgr = self.mgr.clone();
        let tx = self.resets.0.clone();
        std::thread::spawn(move || {
            let _ = tx.send(match mgr.redeem_reset_credit(&id) {
                Ok(outcome) => RotationEvent::ResetRedeemed { label, outcome },
                Err(error) => RotationEvent::ResetFailed { error: format!("{error:#}") },
            });
        });
    }

    /// Moves finished redemptions to `out`.
    pub fn drain_resets(&mut self, out: &mut Vec<RotationEvent>) {
        while let Ok(event) = self.resets.1.try_recv() {
            self.redeeming = false;
            out.push(event);
        }
    }

    pub fn set_managed(&mut self, sessions: impl IntoIterator<Item=SessionId>) {
        self.managed=sessions.into_iter().collect();
    }

    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// 3초마다 에이전트 세션의 화면을 요청한다.
    pub fn poll(&mut self, conn: &mut Conn) {
        if self.last_poll.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.last_poll = Instant::now();
        let ids: Vec<SessionId> = conn
            .infos
            .values()
            .filter(|i| i.exited.is_none() && i.fg_process.as_deref().and_then(tool_for).is_some())
            .map(|i| i.id)
            .collect();
        for id in ids {
            if !self.jobs.iter().any(|j| j.session == id) {
                conn.read_text(id);
            }
        }
    }

    /// 화면 텍스트가 오면 한도 메시지를 찾는다.
    pub fn on_text(&mut self, conn: &Conn, session: SessionId, text: &str, out: &mut Vec<RotationEvent>) {
        let Some(tool) = conn.infos.get(&session).and_then(|i| i.fg_process.as_deref()).and_then(tool_for) else { return };
        let Some(hit) = detect_limit(tool, text) else { return };
        if self.handled.get(&session).is_some_and(|t| t.elapsed() < Duration::from_secs(600)) {
            return;
        }
        self.handled.insert(session, Instant::now());
        if !tool.supports_rotation() {
            out.push(RotationEvent::LimitReached { session, tool, reset_hint: hit.reset_hint });
        } else if self.managed.contains(&session) {
            out.push(RotationEvent::ManagedLimitReached{session,tool});
        } else if self.mgr.auto_rotate(tool) {
            self.start(session, tool);
        } else {
            out.push(RotationEvent::LimitReached { session, tool, reset_hint: hit.reset_hint });
        }
    }

    /// 다음 계정으로 바꾸고 세션을 이어가는 작업을 시작한다.
    pub fn start(&mut self, session: SessionId, tool: Tool) {
        if !tool.supports_rotation() || self.managed.contains(&session) || self.jobs.iter().any(|j| j.session == session) {
            return;
        }
        self.handled.insert(session, Instant::now());
        self.jobs.push(Job { session, tool, stage: Stage::Switching, label: String::new(), started: Instant::now() });
        let mgr = self.mgr.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = mgr.rotate(tool).map(|p| p.map(|p| p.email.map(|e| format!("{} ({e})", p.label)).unwrap_or(p.label))).map_err(|e| e.to_string());
            let _ = tx.send((session, tool, r));
        });
    }

    /// 진행 중인 작업을 한 단계씩 진행한다.
    pub fn tick(&mut self, conn: &mut Conn, out: &mut Vec<RotationEvent>) {
        self.drain_resets(out);
        while let Ok((session, tool, r)) = self.rx.try_recv() {
            let Some(pos) = self.jobs.iter().position(|j| j.session == session) else { continue };
            match r {
                Ok(Some(label)) => {
                    let job = &mut self.jobs[pos];
                    job.label = label;
                    job.stage = Stage::Stopping { since: Instant::now(), tries: 1 };
                    conn.input(session, vec![0x03]);
                }
                Ok(None) => {
                    self.jobs.remove(pos);
                    out.push(RotationEvent::NoAccount { tool });
                }
                Err(e) => {
                    self.jobs.remove(pos);
                    out.push(RotationEvent::Failed { tool, error: e });
                }
            }
        }
        let mut done = Vec::new();
        for (i, job) in self.jobs.iter_mut().enumerate() {
            let Stage::Stopping { since, tries } = &mut job.stage else { continue };
            let fg = conn.infos.get(&job.session).and_then(|x| x.fg_process.clone()).unwrap_or_default();
            if is_shell(&fg) {
                // 셸로 돌아왔으면 같은 카드에서 대화를 이어간다.
                conn.input(job.session, format!("{}\r", resume_command(job.tool)).into_bytes());
                out.push(RotationEvent::Switched { session: job.session, tool: job.tool, label: job.label.clone() });
                done.push(i);
            } else if since.elapsed() > Duration::from_millis(700) && *tries < 6 {
                // Claude Code·Codex 는 Ctrl+C 두 번으로 종료한다.
                conn.input(job.session, vec![0x03]);
                *tries += 1;
                *since = Instant::now();
            } else if job.started.elapsed() > Duration::from_secs(20) {
                out.push(RotationEvent::Failed { tool: job.tool, error: kiln_common::i18n::tr("에이전트를 종료하지 못했습니다. 직접 종료한 뒤 이어서 실행하세요.").into() });
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            self.jobs.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tool_for;

    #[test]
    fn managed_panels_never_start_legacy_rotation() {
        let d=tempfile::tempdir().unwrap();
        let (env,_)=kiln_accounts::Env::sandbox(d.path(),true);
        let mut r=super::Rotator::new(kiln_accounts::AccountManager::with_env(env));
        r.set_managed([42]);r.start(42,kiln_accounts::Tool::Codex);
        assert!(!r.busy());assert!(r.handled.is_empty());
    }
    /// A sandbox with one saved Codex account whose usage reads `exhausted` and `resets`
    /// available reset credits, and a second account with `spare` usage when given.
    fn codex_accounts(root: &std::path::Path, resets: u32, spare: Option<f32>) -> kiln_accounts::AccountManager {
        use kiln_accounts::{Env, ResetCredits, Tool, Usage};
        let (mut env, _) = Env::sandbox(root, false);
        env.codex_probe = Some(std::sync::Arc::new(move |auth: &str| {
            let used = if auth.contains("spare") { spare.unwrap_or(1.0) } else { 1.0 };
            Ok(Usage { five_hour: Some((used, Some(4_200_000_000))), status: "allowed".into(), resets: Some(ResetCredits { available: resets, next_expiry: None }), ..Default::default() })
        }));
        env.codex_reset = std::sync::Arc::new(|_, _| Ok(kiln_accounts::ResetOutcome::Reset));
        std::fs::create_dir_all(&env.codex_home).unwrap();
        let manager = kiln_accounts::AccountManager::with_env(env.clone());
        if spare.is_some() {
            std::fs::write(env.codex_home.join("auth.json"), kiln_accounts::test_support::codex_auth("spare@example.test", "spare")).unwrap();
            manager.save_current(Tool::Codex, "Spare").unwrap();
        }
        std::fs::write(env.codex_home.join("auth.json"), kiln_accounts::test_support::codex_auth("work@example.test", "work")).unwrap();
        manager.save_current(Tool::Codex, "Work").unwrap();
        manager.refresh_usage_blocking(Tool::Codex);
        manager
    }

    #[test]
    fn a_reset_credit_is_offered_only_when_no_other_account_has_room() {
        use kiln_accounts::Tool;
        let root = tempfile::tempdir().unwrap();
        let rotator = super::Rotator::new(codex_accounts(&root.path().join("a"), 2, None));
        assert_eq!(rotator.reset_offer(Tool::Codex), Some(2));
        assert_eq!(rotator.reset_offer(Tool::Claude), None);
        let rotator = super::Rotator::new(codex_accounts(&root.path().join("b"), 0, None));
        assert_eq!(rotator.reset_offer(Tool::Codex), None, "no credit to offer");
        let rotator = super::Rotator::new(codex_accounts(&root.path().join("c"), 2, Some(0.1)));
        assert_eq!(rotator.reset_offer(Tool::Codex), None, "another account can take over");
    }

    #[test]
    fn a_redeemed_reset_credit_is_reported_once() {
        use kiln_accounts::{ResetOutcome, Tool};
        let root = tempfile::tempdir().unwrap();
        let mut rotator = super::Rotator::new(codex_accounts(root.path(), 1, None));
        rotator.redeem(Tool::Codex);
        rotator.redeem(Tool::Codex);
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while events.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            rotator.drain_resets(&mut events);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        rotator.drain_resets(&mut events);
        assert_eq!(events.len(), 1, "a redemption already running is not started again");
        assert!(matches!(&events[0], super::RotationEvent::ResetRedeemed { label, outcome: ResetOutcome::Reset } if label == "Work"));
    }

    #[test]
    fn only_agent_processes_are_watched() {
        assert!(tool_for("claude").is_some());
        assert!(tool_for("codex").is_some());
        assert_eq!(tool_for("agy"), Some(kiln_accounts::Tool::Antigravity));
        assert!(tool_for("antigravity").is_none());
        assert!(tool_for("zsh").is_none());
        assert!(tool_for("node").is_none());
    }
}
