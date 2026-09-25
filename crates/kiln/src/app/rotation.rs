//! 에이전트 CLI(claude, codex) 사용량 한도 감지와 계정 전환 후 세션 이어가기.

use super::conn::Conn;
use kiln_accounts::{AccountManager, Tool, detect_limit, resume_command};
use kiln_proto::SessionId;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// 앱에 알릴 일.
pub enum RotationEvent {
    /// 한도에 걸렸고 자동 전환이 꺼져 있다(사용자가 전환할지 고른다).
    LimitReached { session: SessionId, tool: Tool, reset_hint: Option<String> },
    Switched { session: SessionId, tool: Tool, label: String },
    NoAccount { tool: Tool },
    Failed { tool: Tool, error: String },
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
    tx: Sender<(SessionId, Tool, Result<Option<String>, String>)>,
    rx: Receiver<(SessionId, Tool, Result<Option<String>, String>)>,
}

pub fn tool_for(process: &str) -> Option<Tool> {
    match process {
        "claude" => Some(Tool::Claude),
        "codex" => Some(Tool::Codex),
        _ => None,
    }
}

fn is_shell(p: &str) -> bool {
    super::shells().contains(&p)
}

impl Rotator {
    pub fn new(mgr: AccountManager) -> Self {
        let (tx, rx) = channel();
        Rotator { mgr, last_poll: Instant::now(), handled: HashMap::new(), jobs: Vec::new(), tx, rx }
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
        if self.mgr.auto_rotate(tool) {
            self.start(session, tool);
        } else {
            out.push(RotationEvent::LimitReached { session, tool, reset_hint: hit.reset_hint });
        }
    }

    /// 다음 계정으로 바꾸고 세션을 이어가는 작업을 시작한다.
    pub fn start(&mut self, session: SessionId, tool: Tool) {
        if self.jobs.iter().any(|j| j.session == session) {
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
                out.push(RotationEvent::Failed { tool: job.tool, error: "에이전트를 종료하지 못했습니다. 직접 종료한 뒤 이어서 실행하세요.".into() });
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
    fn only_agent_processes_are_watched() {
        assert!(tool_for("claude").is_some());
        assert!(tool_for("codex").is_some());
        assert!(tool_for("zsh").is_none());
        assert!(tool_for("node").is_none());
    }
}
