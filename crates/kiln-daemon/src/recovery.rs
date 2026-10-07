//! Transport liveness is independent of a silent or thinking foreground job.
use kiln_proto::{TerminalHealth, TerminalState};
use std::time::{Duration, Instant};

pub(crate) struct Recovery {
    pub health: TerminalHealth,
    pub reader_seen: Instant,
    working: bool,
    next: Instant,
    healthy_since: Option<Instant>,
}
impl Default for Recovery {
    fn default() -> Self { let now=Instant::now(); Self {health:Default::default(),reader_seen:now,working:false,next:now,healthy_since:None} }
}
impl Recovery {
    pub fn pulse(&mut self, now:Instant) {
        self.reader_seen=now;
        if !self.working && self.healthy_since.is_some_and(|start|now.saturating_duration_since(start)>=Duration::from_secs(2)) {self.alive(now);}
    }
    pub fn disconnected(&mut self, now:Instant) {
        if self.healthy_since.take().is_some() && self.health.attempts>0 {self.failed(now);}
        if self.health.state==TerminalState::Healthy {
            self.health.state=TerminalState::Recovering; self.next=now;
        }
    }
    pub fn inspect(&mut self, now:Instant) -> TerminalHealth {
        if now.saturating_duration_since(self.reader_seen)>Duration::from_secs(5) {self.disconnected(now);}
        self.health
    }
    pub fn begin(&mut self, now:Instant, manual:bool) -> bool {
        if self.working {return false;}
        if manual {self.health=TerminalHealth {state:TerminalState::Recovering,attempts:0};self.next=now;self.healthy_since=None;}
        self.inspect(now);
        if self.healthy_since.is_some() {return false;}
        if !manual && self.health.attempts>=3 {self.health.state=TerminalState::Stalled;return false;}
        if self.health.state!=TerminalState::Recovering || now<self.next {return false;}
        self.health.attempts+=1; self.working=true; true
    }
    pub fn failed(&mut self, now:Instant) {
        self.working=false;
        self.healthy_since=None;
        self.next=now+Duration::from_secs(u64::from(self.health.attempts));
        self.health.state=if self.health.attempts>=3 {TerminalState::Stalled}else{TerminalState::Recovering};
    }
    pub fn connected(&mut self, now:Instant) {self.working=false;self.reader_seen=now;self.healthy_since=Some(now);}
    pub fn alive(&mut self, now:Instant) {self.working=false;self.healthy_since=None; self.health=Default::default(); self.reader_seen=now;}
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn silence_with_a_live_reader_is_not_an_agent_failure() {
        let mut r=Recovery::default(); let start=Instant::now();
        for n in 0..100 { let now=start+Duration::from_secs(n); r.pulse(now); assert_eq!(r.inspect(now).state,TerminalState::Healthy); assert!(!r.begin(now,false)); }
    }
    #[test] fn lost_transport_retries_three_times_then_requires_manual_repair() {
        let mut r=Recovery::default(); let now=Instant::now(); r.disconnected(now);
        for n in 0..3 { let time=now+Duration::from_secs(n*3); assert!(r.begin(time,false)); assert!(!r.begin(time,true),"no duplicate in-flight repair"); r.failed(time); }
        assert_eq!(r.inspect(now+Duration::from_secs(20)).state,TerminalState::Stalled);
        assert!(!r.begin(now+Duration::from_secs(20),false));
        assert!(r.begin(now+Duration::from_secs(20),true));
        assert_eq!(r.health.attempts,1);
        r.alive(now+Duration::from_secs(21)); assert_eq!(r.health,TerminalHealth::default());
    }
    #[test] fn stalled_reader_is_detected_without_terminal_output_heuristics() {
        let mut r=Recovery::default(); let now=r.reader_seen+Duration::from_secs(6);
        assert_eq!(r.inspect(now).state,TerminalState::Recovering);
        assert!(r.begin(now,false)); r.failed(now);
        r.connected(now);r.pulse(now+Duration::from_secs(2));
        assert_eq!(r.inspect(now+Duration::from_secs(2)).state,TerminalState::Healthy);
    }
    #[test] fn a_successful_hello_followed_by_eof_does_not_reset_the_retry_budget() {
        let mut r=Recovery::default(); let now=Instant::now();r.disconnected(now);
        for n in 0..3 {let time=now+Duration::from_secs(n*3);assert!(r.begin(time,false));r.connected(time);r.disconnected(time+Duration::from_millis(10));}
        assert_eq!(r.health.state,TerminalState::Stalled);assert_eq!(r.health.attempts,3);
    }
}
