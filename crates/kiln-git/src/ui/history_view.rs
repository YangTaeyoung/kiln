//! 커밋 이력(Log) 화면: 가상화된 커밋 표와 그래프 레인, 필터 도구 막대, 상세 패널,
//! 다중 선택과 컨텍스트 메뉴, 끌어다 놓기로 순서 이동·픽스업, 대화형 리베이스.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::epaint::CubicBezierShape;
use egui::text::LayoutJob;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Galley, Id, Key, Layout, Margin, Painter, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextFormat, Ui, UiBuilder, Vec2, pos2, vec2,
};
use kiln_common::icons::{self as kicons, Icon};
use kiln_common::widgets::{ButtonKind, button_with, hue_color, lerp_color, tint, toggle};
use kiln_common::{Task, Theme, fonts};

use crate::cmd::GitResult;
use crate::history::{
    self, BranchFilter, CommitInfo, DropPlace, GraphBuilder, HistoryEvent, LaneRow, LogCommit, LogPager, LogQuery,
    RebasePlan, RefKind, RefLabel, RepoState, ResetMode, Rewrite,
};
use crate::repo::RepoOp;
use crate::util::{now_unix, relative_time};

#[path = "history_dialogs.rs"]
mod dialogs;

use dialogs::{ConfirmDialog, Dialog, DialogOutcome, MessageDialog, MessageKind, NameDialog, NameKind, RebaseDialog, ResetDialog};

const PAGE: usize = 400;
const ROW_H: f32 = 26.0;
const HEADER_H: f32 = 28.0;
const LANE_W: f32 = 13.0;
const GRAPH_PAD: f32 = 12.0;
const MAX_LANES: usize = 12;
const POLL: Duration = Duration::from_millis(2500);
const FILTER_DEBOUNCE: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------- 백그라운드 작업

/// 저장소를 바꾸는 작업.
#[derive(Clone, Debug)]
pub(crate) enum Op {
    Checkout(String),
    Branch { name: String, sha: String, switch: bool },
    Tag { name: String, sha: String, message: String },
    CherryPick(Vec<String>),
    Revert(String),
    Reset(String, ResetMode),
    Reword { sha: String, message: String },
    Drop(Vec<String>),
    Squash { shas: Vec<String>, message: String },
    Move { moving: Vec<String>, target: String, place: DropPlace },
    Fixup { moving: Vec<String>, target: String },
    Rebase(RebasePlan),
    Continue,
    Skip,
    Abort,
    Undo(String),
    ForcePush,
}

impl Op {
    fn label(&self) -> &'static str {
        match self {
            Op::Checkout(_) => "체크아웃",
            Op::Branch { .. } => "브랜치 만들기",
            Op::Tag { .. } => "태그 만들기",
            Op::CherryPick(_) => "체리픽",
            Op::Revert(_) => "커밋 되돌리기(revert)",
            Op::Reset(..) => "리셋",
            Op::Reword { .. } => "메시지 수정",
            Op::Drop(_) => "커밋 삭제",
            Op::Squash { .. } => "스쿼시",
            Op::Move { .. } => "순서 이동",
            Op::Fixup { .. } => "픽스업",
            Op::Rebase(_) => "대화형 리베이스",
            Op::Continue => "계속",
            Op::Skip => "건너뛰기",
            Op::Abort => "중단",
            Op::Undo(_) => "되돌리기",
            Op::ForcePush => "강제 푸시",
        }
    }

    /// 실행 전 HEAD 를 기록해 되돌리기를 제안할 작업인지.
    fn offers_undo(&self) -> bool {
        !matches!(self, Op::Checkout(_) | Op::Branch { .. } | Op::Tag { .. } | Op::Abort | Op::Undo(_) | Op::ForcePush)
    }

    fn run(self, root: &Path, autostash: bool) -> GitResult<OpResult> {
        let rw = |r: GitResult<Rewrite>| r.map(OpResult::Rewrite);
        let tx = |r: GitResult<String>| r.map(OpResult::Text);
        match self {
            Op::Checkout(rev) => tx(history::checkout(root, &rev)),
            Op::Branch { name, sha, switch } => tx(history::create_branch_at(root, &name, &sha, switch)),
            Op::Tag { name, sha, message } => tx(history::create_tag(root, &name, &sha, &message)),
            Op::CherryPick(shas) => rw(history::cherry_pick(root, &shas)),
            Op::Revert(sha) => rw(history::revert(root, &sha)),
            Op::Reset(sha, mode) => rw(history::reset(root, &sha, mode)),
            Op::Reword { sha, message } => rw(history::reword_commit(root, &sha, &message, autostash)),
            Op::Drop(shas) => rw(history::drop_commits(root, &shas, autostash)),
            Op::Squash { shas, message } => rw(history::squash_commits(root, &shas, &message, autostash)),
            Op::Move { moving, target, place } => rw(history::move_commits(root, &moving, &target, place, autostash)),
            Op::Fixup { moving, target } => rw(history::fixup_into(root, &moving, &target, autostash)),
            Op::Rebase(plan) => rw(history::run_rebase(root, &plan, autostash)),
            Op::Continue => rw(history::continue_op(root)),
            Op::Skip => rw(history::skip_op(root)),
            Op::Abort => tx(history::abort_op(root)),
            Op::Undo(old) => tx(history::undo(root, &old)),
            Op::ForcePush => tx(history::force_push(root)),
        }
    }
}

pub(crate) enum OpResult {
    Text(String),
    Rewrite(Rewrite),
}

struct Job {
    op: Op,
    autostash: bool,
    pushed: bool,
}

struct Done {
    op: Op,
    pushed: bool,
    result: GitResult<OpResult>,
}

/// 작업을 한 스레드에서 차례로 실행한다.
struct Runner {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    pending: usize,
    running: Arc<parking_lot::Mutex<Option<&'static str>>>,
}

impl Runner {
    fn new(ctx: &egui::Context, root: PathBuf) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel::<Done>();
        let running = Arc::new(parking_lot::Mutex::new(None));
        let run = running.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("kiln-history-worker".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    *run.lock() = Some(job.op.label());
                    ctx.request_repaint();
                    let result = job.op.clone().run(&root, job.autostash);
                    *run.lock() = None;
                    let _ = done_tx.send(Done { op: job.op, pushed: job.pushed, result });
                    ctx.request_repaint();
                }
            })
            .expect("spawn history worker");
        Self { tx, rx, pending: 0, running }
    }
}

// ---------------------------------------------------------------- 로그 버퍼

#[derive(Default)]
struct LogBuf {
    pager: Option<LogPager>,
    commits: Vec<LogCommit>,
    lanes: Vec<LaneRow>,
    graph: GraphBuilder,
    index: HashMap<String, usize>,
    exhausted: bool,
    error: Option<String>,
}

impl LogBuf {
    fn start(root: &Path, q: &LogQuery) -> Self {
        Self { pager: Some(LogPager::start(root, q, PAGE)), ..Default::default() }
    }

    /// 준비된 페이지를 최대 `max_pages` 개 붙인다. 새 행이 생겼으면 true.
    fn pull(&mut self, max_pages: usize) -> bool {
        let mut grew = false;
        for _ in 0..max_pages {
            let Some(p) = self.pager.as_mut() else { break };
            match p.try_next() {
                Some(Ok(batch)) => {
                    for c in batch {
                        let row = self.graph.push(&c.sha, &c.parents);
                        self.index.insert(c.sha.clone(), self.commits.len());
                        self.lanes.push(row);
                        self.commits.push(c);
                    }
                    grew = true;
                }
                Some(Err(e)) => {
                    self.error = Some(e.to_string());
                    self.exhausted = true;
                    self.pager = None;
                    break;
                }
                None => {
                    if p.is_done() {
                        self.exhausted = true;
                        self.pager = None;
                    }
                    break;
                }
            }
        }
        grew
    }
}

// ---------------------------------------------------------------- 상태

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragZone {
    Above,
    Below,
}

/// 로그 표에서 끌고 있는 커밋.
#[derive(Clone, Debug)]
struct DragState {
    moving: Vec<String>,
    target: Option<(String, DragZone)>,
    valid: bool,
}

/// 놓은 뒤 고를 동작 메뉴.
#[derive(Clone, Debug)]
struct DropMenu {
    moving: Vec<String>,
    target: String,
    place: DropPlace,
    pos: Pos2,
}

/// 작업 완료 알림(되돌리기·강제 푸시 제안 포함).
#[derive(Clone, Debug)]
struct Notice {
    text: String,
    detail: Option<String>,
    error: bool,
    /// 작업이 멈춰 사용자 조치가 필요한 알림.
    warn: bool,
    undo: Option<String>,
    force_push: bool,
}

/// 표시용 메뉴 동작.
#[derive(Clone, Debug)]
enum MenuAction {
    Run(Op),
    CheckoutDetached(String),
    NewBranch(String),
    NewTag(String),
    Reset(String),
    Reword(String),
    Squash(Vec<String>),
    DropCommits(Vec<String>),
    InteractiveFrom(String),
    CompareWorktree(String),
    CopyHashes(Vec<String>),
    OpenCommit(String),
}

/// JetBrains 스타일 Git 로그 화면.
pub struct HistoryView {
    root: PathBuf,
    now_override: Option<i64>,
    query: LogQuery,
    search: String,
    author: String,
    path: String,
    filter_changed: Option<Instant>,
    log: LogBuf,
    next: Option<(LogBuf, usize)>,
    want_more: bool,
    state: Option<RepoState>,
    state_task: Option<Task<GitResult<RepoState>>>,
    state_at: Option<Instant>,
    state_error: Option<String>,
    reload_log: bool,
    linear: HashMap<String, usize>,
    selected: Vec<String>,
    anchor: Option<String>,
    detail: Option<(String, Task<GitResult<CommitInfo>>)>,
    detail_info: Option<CommitInfo>,
    detail_error: Option<String>,
    runner: Option<Runner>,
    notice: Option<Notice>,
    dialog: Option<Dialog>,
    drag: Option<DragState>,
    drop_menu: Option<DropMenu>,
    scroll_to: Option<usize>,
    detail_w: f32,
    events: Vec<HistoryEvent>,
    menu_action: Option<MenuAction>,
    table_rect: Rect,
    auto_scroll: Option<f32>,
    /// 작업 뒤 상세(참조 등)를 다시 읽어야 하는지. 기존 내용은 새 결과가 올 때까지 유지한다.
    detail_stale: bool,
}

impl HistoryView {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            now_override: None,
            query: LogQuery::default(),
            search: String::new(),
            author: String::new(),
            path: String::new(),
            filter_changed: None,
            log: LogBuf::default(),
            next: None,
            want_more: false,
            state: None,
            state_task: None,
            state_at: None,
            state_error: None,
            reload_log: true,
            linear: HashMap::new(),
            selected: Vec::new(),
            anchor: None,
            detail: None,
            detail_info: None,
            detail_error: None,
            runner: None,
            notice: None,
            dialog: None,
            drag: None,
            drop_menu: None,
            scroll_to: None,
            detail_w: 360.0,
            events: Vec::new(),
            menu_action: None,
            table_rect: Rect::NOTHING,
            auto_scroll: None,
            detail_stale: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 로그와 저장소 상태를 다시 읽는다. 선택은 해시로 유지한다.
    pub fn refresh(&mut self) {
        self.reload_log = true;
        self.state_at = None;
    }

    /// 백그라운드 로드나 작업이 진행 중인지.
    pub fn is_busy(&mut self) -> bool {
        self.reload_log
            || self.next.is_some()
            || self.filter_changed.is_some()
            || self.state_task.as_mut().is_some_and(|t| t.is_pending())
            || self.detail.as_mut().is_some_and(|(_, t)| t.is_pending())
            || self.runner.as_ref().is_some_and(|r| r.pending > 0)
            || self.dialog.as_mut().is_some_and(Dialog::is_loading)
    }

    /// 상대 시간 기준 시각을 고정한다.
    #[doc(hidden)]
    pub fn set_now(&mut self, ts: i64) {
        self.now_override = Some(ts);
    }

    /// 불러온 커밋(표 순서).
    pub fn commits(&self) -> &[LogCommit] {
        &self.log.commits
    }

    /// 선택한 커밋 해시(선택한 순서).
    pub fn selection(&self) -> &[String] {
        &self.selected
    }

    /// 커밋 하나를 선택하고 표에서 보이게 한다.
    pub fn select(&mut self, sha: &str) {
        if let Some(&i) = self.log.index.get(sha) {
            self.selected = vec![sha.to_string()];
            self.anchor = Some(sha.to_string());
            self.scroll_to = Some(i);
        }
    }

    /// 현재 조회 조건.
    pub fn query(&self) -> &LogQuery {
        &self.query
    }

    /// 조회 조건을 바꾸고 다시 읽는다.
    pub fn set_query(&mut self, q: LogQuery) {
        self.search = q.text.clone();
        self.author = q.author.clone();
        self.path = q.path.clone();
        self.query = q;
        self.reload_log = true;
    }

    /// 마지막으로 읽은 저장소 상태.
    pub fn repo_state(&self) -> Option<&RepoState> {
        self.state.as_ref()
    }

    /// 끌기 중 놓을 위치 표시(대상 해시, 위/아래).
    #[doc(hidden)]
    pub fn drag_hint(&self) -> Option<(String, DropPlace)> {
        let d = self.drag.as_ref()?;
        let (sha, z) = d.target.clone()?;
        Some((sha, if z == DragZone::Above { DropPlace::Above } else { DropPlace::Below }))
    }

    /// 열린 대화상자의 메시지 편집 값.
    #[doc(hidden)]
    pub fn dialog_message_mut(&mut self) -> Option<&mut String> {
        match self.dialog.as_mut()? {
            Dialog::Message(m) => Some(&mut m.text),
            _ => None,
        }
    }

    /// 마지막 알림 문구.
    #[doc(hidden)]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|n| n.text.as_str())
    }

    fn now(&self) -> i64 {
        self.now_override.unwrap_or_else(now_unix)
    }


    fn commit(&self, sha: &str) -> Option<&LogCommit> {
        self.log.index.get(sha).and_then(|&i| self.log.commits.get(i))
    }

    // ------------------------------------------------------------ 로드

    fn pump(&mut self, ctx: &egui::Context) {
        if self.runner.is_none() {
            self.runner = Some(Runner::new(ctx, self.root.clone()));
        }
        self.pump_jobs();

        if let Some(t) = &mut self.state_task
            && let Some(r) = t.take()
        {
            self.state_task = None;
            self.state_at = Some(Instant::now());
            match r {
                Ok(s) => {
                    let changed = self.state.as_ref().is_none_or(|o| o.fingerprint != s.fingerprint);
                    self.state = Some(s);
                    self.state_error = None;
                    if changed {
                        self.reload_log = true;
                    }
                    self.rebuild_linear();
                }
                Err(e) => {
                    self.state_error = Some(e.to_string());
                    self.state = None;
                }
            }
        }
        let busy = self.runner.as_ref().is_some_and(|r| r.pending > 0);
        if self.state_task.is_none() && !busy && self.state_at.is_none_or(|t| t.elapsed() >= POLL) {
            let root = self.root.clone();
            self.state_task = Some(Task::spawn(ctx, move || history::repo_state(&root)));
        }
        ctx.request_repaint_after(POLL);

        if let Some(t) = self.filter_changed
            && t.elapsed() >= FILTER_DEBOUNCE
        {
            self.filter_changed = None;
            let q = LogQuery { text: self.search.clone(), author: self.author.clone(), path: self.path.clone(), ..self.query.clone() };
            if q != self.query {
                self.query = q;
                self.reload_log = true;
            }
        } else if self.filter_changed.is_some() {
            ctx.request_repaint_after(FILTER_DEBOUNCE);
        }

        if self.reload_log && !busy {
            self.reload_log = false;
            let target = self.log.commits.len().clamp(PAGE, PAGE * 20);
            self.next = Some((LogBuf::start(&self.root, &self.query), target));
        }
        let mut swapped = false;
        if let Some((buf, target)) = &mut self.next {
            buf.pull(8);
            if buf.commits.len() >= *target || buf.exhausted {
                let (buf, _) = self.next.take().expect("next");
                self.log = buf;
                swapped = true;
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        if self.want_more && !self.log.exhausted {
            if self.log.pull(2) {
                self.rebuild_linear();
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        if swapped {
            self.selected.retain(|s| self.log.index.contains_key(s));
            if self.selected.is_empty()
                && let Some(c) = self.log.commits.first()
            {
                self.selected.push(c.sha.clone());
                self.anchor = Some(c.sha.clone());
            }
            self.rebuild_linear();
        }
        self.pump_detail(ctx);
    }

    /// HEAD 에서 첫 부모만 따라가며 병합 전까지의 커밋 깊이를 계산한다.
    fn rebuild_linear(&mut self) {
        self.linear.clear();
        let Some(mut cur) = self.state.as_ref().and_then(|s| s.head.clone()) else { return };
        let mut depth = 0;
        while let Some(&i) = self.log.index.get(&cur) {
            let c = &self.log.commits[i];
            if c.is_merge() {
                break;
            }
            self.linear.insert(cur.clone(), depth);
            depth += 1;
            match c.parents.first() {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
    }

    /// 이력 재작성이 가능한 커밋인지. 필터로 그래프가 끊긴 경우엔 백엔드 검증에 맡긴다.
    fn rewritable(&self, sha: &str) -> bool {
        self.linear.contains_key(sha) || (self.query.is_filtered() && !self.commit(sha).is_some_and(LogCommit::is_merge))
    }

    fn pump_detail(&mut self, ctx: &egui::Context) {
        let want = (self.selected.len() == 1).then(|| self.selected[0].clone());
        match (&want, &mut self.detail) {
            (Some(sha), Some((cur, _))) if cur == sha => {}
            (Some(sha), _) => {
                if self.detail_stale || self.detail_info.as_ref().is_none_or(|d| &d.sha != sha) {
                    self.detail_stale = false;
                    let root = self.root.clone();
                    let s = sha.clone();
                    self.detail = Some((sha.clone(), Task::spawn(ctx, move || history::commit_info(&root, &s))));
                }
            }
            (None, _) => {
                self.detail = None;
            }
        }
        if let Some((sha, t)) = &mut self.detail
            && let Some(r) = t.take()
        {
            let sha = sha.clone();
            self.detail = None;
            match r {
                Ok(info) => {
                    self.detail_info = Some(info);
                    self.detail_error = None;
                }
                Err(e) => {
                    self.detail_info = None;
                    self.detail_error = Some(format!("{}: {e}", &sha[..sha.len().min(8)]));
                }
            }
        }
    }

    fn pump_jobs(&mut self) {
        let Some(r) = &mut self.runner else { return };
        let done: Vec<Done> = r.rx.try_iter().collect();
        r.pending = r.pending.saturating_sub(done.len());
        for d in done {
            self.reload_log = true;
            self.state_at = None;
            self.detail_stale = true;
            let label = d.op.label();
            match d.result {
                Ok(OpResult::Rewrite(rw)) => match &rw.outcome {
                    history::Outcome::Done => {
                        let undo = (d.op.offers_undo() && rw.old_head != rw.new_head && !rw.old_head.is_empty())
                            .then(|| rw.old_head.clone());
                        let text = format!("{label} 완료");
                        self.events.push(HistoryEvent::Toast(text.clone()));
                        self.notice = Some(Notice {
                            text,
                            detail: Some(format!("HEAD {} → {}", short(&rw.old_head), short(&rw.new_head))),
                            error: false, warn: false,
                            undo,
                            force_push: d.pushed,
                        });
                    }
                    history::Outcome::Stopped(st) => {
                        let what = if st.conflicts.is_empty() {
                            format!("{}이(가) 편집을 위해 멈췄습니다", st.op.label())
                        } else {
                            format!("{} 중 충돌 {}개", st.op.label(), st.conflicts.len())
                        };
                        self.notice = Some(Notice {
                            text: what,
                            detail: None,
                            error: false, warn: true,
                            undo: (!rw.old_head.is_empty() && d.op.offers_undo()).then(|| rw.old_head.clone()),
                            force_push: false,
                        });
                    }
                },
                Ok(OpResult::Text(out)) => {
                    let text = format!("{label} 완료");
                    self.events.push(HistoryEvent::Toast(text.clone()));
                    let force_push = d.pushed && matches!(d.op, Op::Continue);
                    let last = out.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string);
                    self.notice = Some(Notice { text, detail: last, error: false, warn: false, undo: None, force_push });
                    if let Op::Branch { name, switch: false, .. } = &d.op {
                        self.notice.as_mut().expect("notice").detail = Some(format!("'{name}' 브랜치를 만들었습니다"));
                    }
                }
                Err(e) => {
                    self.notice =
                        Some(Notice { text: format!("{label} 실패"), detail: Some(e.to_string()), error: true, warn: false, undo: None, force_push: false });
                }
            }
        }
    }

    fn submit(&mut self, op: Op, autostash: bool, pushed: bool) {
        self.notice = None;
        if let Some(r) = &mut self.runner {
            r.pending += 1;
            if r.tx.send(Job { op, autostash, pushed }).is_err() {
                r.pending -= 1;
            }
        }
    }

    fn busy_label(&self) -> Option<&'static str> {
        self.runner.as_ref().and_then(|r| *r.running.lock())
    }

    /// 재작성 구간의 가장 오래된 커밋이 원격에 있는지.
    fn rewrites_pushed(&self, shas: &[String]) -> bool {
        let Some(st) = &self.state else { return false };
        let oldest = shas.iter().max_by_key(|s| self.linear.get(*s).copied().unwrap_or(usize::MAX));
        oldest.is_some_and(|s| st.is_pushed(s))
    }

    fn dirty(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.dirty)
    }

    /// 재작성 작업을 시작한다. 원격 커밋이나 커밋되지 않은 변경이 있으면 먼저 확인한다.
    fn start_rewrite(&mut self, op: Op, shas: Vec<String>) {
        let pushed = self.rewrites_pushed(&shas);
        let dirty = self.dirty();
        if pushed || dirty {
            self.dialog = Some(Dialog::Confirm(ConfirmDialog::rewrite(op, pushed, dirty)));
        } else {
            self.submit(op, false, false);
        }
    }

    // ------------------------------------------------------------ 선택

    fn click_row(&mut self, idx: usize, mods: egui::Modifiers) {
        let Some(sha) = self.log.commits.get(idx).map(|c| c.sha.clone()) else { return };
        if mods.shift
            && let Some(a) = self.anchor.as_ref().and_then(|a| self.log.index.get(a).copied())
        {
            let (lo, hi) = if a <= idx { (a, idx) } else { (idx, a) };
            let range: Vec<String> = self.log.commits[lo..=hi].iter().map(|c| c.sha.clone()).collect();
            if mods.command {
                for s in range {
                    if !self.selected.contains(&s) {
                        self.selected.push(s);
                    }
                }
            } else {
                self.selected = range;
            }
            return;
        }
        if mods.command {
            if let Some(p) = self.selected.iter().position(|s| *s == sha) {
                self.selected.remove(p);
            } else {
                self.selected.push(sha.clone());
            }
            self.anchor = Some(sha);
            return;
        }
        self.selected = vec![sha.clone()];
        self.anchor = Some(sha);
    }

    /// 선택을 표 순서(새 커밋 먼저)로.
    fn selection_sorted(&self) -> Vec<String> {
        let mut v: Vec<(usize, String)> =
            self.selected.iter().filter_map(|s| self.log.index.get(s).map(|&i| (i, s.clone()))).collect();
        v.sort_by_key(|(i, _)| *i);
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// 선택이 현재 브랜치 첫 부모 줄의 연속 구간인지(병합 제외).
    fn contiguous_linear(&self, shas: &[String]) -> bool {
        let mut d: Vec<usize> = match shas.iter().map(|s| self.linear.get(s).copied()).collect::<Option<Vec<_>>>() {
            Some(d) => d,
            None => return false,
        };
        d.sort_unstable();
        d.windows(2).all(|w| w[1] == w[0] + 1)
    }

    fn move_selection(&mut self, delta: isize, extend: bool) {
        if self.log.commits.is_empty() {
            return;
        }
        let cur = self
            .selected
            .last()
            .and_then(|s| self.log.index.get(s).copied())
            .map(|i| i as isize)
            .unwrap_or(-1);
        let n = (cur + delta).clamp(0, self.log.commits.len() as isize - 1) as usize;
        let sha = self.log.commits[n].sha.clone();
        if extend {
            if !self.selected.contains(&sha) {
                self.selected.push(sha);
            }
        } else {
            self.selected = vec![sha.clone()];
            self.anchor = Some(sha);
        }
        self.scroll_to = Some(n);
    }

    // ------------------------------------------------------------ 그리기

    /// 화면을 그리고 앱에 요청할 동작을 돌려준다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<HistoryEvent> {
        self.pump(ui.ctx());
        let t = Theme::current();
        let full = ui.available_rect_before_wrap();
        ui.painter().rect_filled(full, 0.0, t.bg);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(full).layout(Layout::top_down(Align::Min)));
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);

        self.ui_toolbar(&mut ui);
        self.ui_op_banner(&mut ui);

        let body = ui.available_rect_before_wrap();
        let detail_w = self.detail_w.clamp(260.0, (body.width() * 0.55).max(260.0));
        let show_detail = body.width() > 640.0;
        let table_rect = if show_detail { Rect::from_min_max(body.min, pos2(body.right() - detail_w, body.bottom())) } else { body };
        self.table_rect = table_rect;
        {
            let mut tui = ui.new_child(UiBuilder::new().max_rect(table_rect).layout(Layout::top_down(Align::Min)));
            tui.set_clip_rect(table_rect.intersect(ui.clip_rect()));
            self.ui_table(&mut tui);
        }
        if show_detail {
            let split = Rect::from_min_max(pos2(table_rect.right(), body.top()), pos2(table_rect.right() + 1.0, body.bottom()));
            ui.painter().rect_filled(split, 0.0, t.border);
            let handle = ui.interact(split.expand2(vec2(3.0, 0.0)), ui.id().with("hist-split"), Sense::drag());
            if handle.hovered() || handle.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if handle.dragged() {
                self.detail_w = (self.detail_w - handle.drag_delta().x).clamp(260.0, body.width() * 0.6);
            }
            let drect = Rect::from_min_max(pos2(split.right(), body.top()), body.max);
            ui.painter().rect_filled(drect, 0.0, t.bg_panel);
            let mut dui = ui.new_child(UiBuilder::new().max_rect(drect).layout(Layout::top_down(Align::Min)));
            dui.set_clip_rect(drect.intersect(ui.clip_rect()));
            self.ui_detail(&mut dui);
        }
        ui.advance_cursor_after_rect(full);
        self.ui_notice(&ui, table_rect);
        self.ui_drop_menu(ui.ctx());
        self.handle_keys(ui.ctx());
        if let Some(a) = self.menu_action.take() {
            self.apply_menu(ui.ctx(), a);
        }
        self.ui_dialog(ui.ctx());
        std::mem::take(&mut self.events)
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || ctx.memory(|m| m.focused().is_some()) || self.drop_menu.is_some() {
            return;
        }
        let (up, down, enter, esc, shift, copy) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                i.modifiers.shift,
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            )
        });
        if up {
            self.move_selection(-1, shift);
        }
        if down {
            self.move_selection(1, shift);
        }
        if enter && let [s] = self.selected.as_slice() {
            self.events.push(HistoryEvent::OpenCommit(s.clone()));
        }
        if esc {
            self.drag = None;
        }
        if copy && !self.selected.is_empty() {
            ctx.copy_text(self.selection_sorted().join("\n"));
        }
    }

    fn ui_toolbar(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, t.bg_panel);
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, t.border));
        let inner = rect.shrink2(vec2(12.0, 9.0));
        let mut bar = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
        bar.spacing_mut().item_spacing.x = 8.0;
        let narrow = w < 900.0;

        let before = (self.search.clone(), self.author.clone(), self.path.clone());
        search_field(&mut bar, "hist-search", &mut self.search, Icon::Search, "메시지 또는 해시 검색", if narrow { 170.0 } else { 240.0 });
        self.branch_menu(&mut bar);
        self.author_menu(&mut bar);
        search_field(&mut bar, "hist-path", &mut self.path, Icon::File, "경로", if narrow { 110.0 } else { 150.0 });
        if before != (self.search.clone(), self.author.clone(), self.path.clone()) {
            self.filter_changed = Some(Instant::now());
        }
        let mut fp = self.query.first_parent;
        let resp = labeled_toggle(&mut bar, &mut fp, "첫 부모만");
        if resp.changed() {
            self.query.first_parent = fp;
            self.reload_log = true;
        }
        bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if kiln_common::widgets::icon_button(ui, Icon::Refresh, 28.0, false, "새로 고침").clicked() {
                self.refresh();
            }
            if let Some(l) = self.busy_label() {
                ui.label(RichText::new(format!("{l} 중…")).font(fonts::medium(12.0)).color(t.text_dim));
                ui.add(egui::Spinner::new().size(13.0).color(t.text_dim));
            } else if self.next.is_some() {
                ui.add(egui::Spinner::new().size(13.0).color(t.text_faint));
            } else if !self.log.commits.is_empty() {
                let n = self.log.commits.len();
                let s = if self.log.exhausted { format!("커밋 {}", group_digits(n)) } else { format!("커밋 {}+", group_digits(n)) };
                ui.label(RichText::new(s).font(fonts::medium(11.5)).color(t.text_faint));
            }
        });
    }

    fn branch_menu(&mut self, ui: &mut Ui) {
        let label = match &self.query.branch {
            BranchFilter::All => "모든 브랜치".to_string(),
            BranchFilter::Current => "현재 브랜치".to_string(),
            BranchFilter::Named(n) => n.clone(),
        };
        let active = self.query.branch != BranchFilter::All;
        let resp = dropdown_button(ui, Icon::Branch, &label, active, "브랜치 필터");
        let mut pick: Option<BranchFilter> = None;
        egui::Popup::menu(&resp).width(240.0).show(|ui| {
            ui.set_min_width(240.0);
            if menu_item(ui, "모든 브랜치", None, self.query.branch == BranchFilter::All, true).clicked() {
                pick = Some(BranchFilter::All);
            }
            let cur = self.state.as_ref().and_then(|s| s.branch.clone()).unwrap_or_else(|| "HEAD".into());
            if menu_item(ui, "현재 브랜치", Some(&cur), self.query.branch == BranchFilter::Current, true).clicked() {
                pick = Some(BranchFilter::Current);
            }
            let (locals, remotes) = self
                .state
                .as_ref()
                .map(|s| (s.local_branches.clone(), s.remote_branches.clone()))
                .unwrap_or_default();
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                if !locals.is_empty() {
                    menu_heading(ui, "로컬");
                    for b in &locals {
                        let sel = self.query.branch == BranchFilter::Named(b.clone());
                        if menu_item(ui, b, None, sel, true).clicked() {
                            pick = Some(BranchFilter::Named(b.clone()));
                        }
                    }
                }
                if !remotes.is_empty() {
                    menu_heading(ui, "원격");
                    for b in &remotes {
                        let sel = self.query.branch == BranchFilter::Named(b.clone());
                        if menu_item(ui, b, None, sel, true).clicked() {
                            pick = Some(BranchFilter::Named(b.clone()));
                        }
                    }
                }
            });
        });
        if let Some(b) = pick {
            if b != self.query.branch {
                self.query.branch = b;
                self.reload_log = true;
            }
            egui::Popup::close_all(ui.ctx());
        }
    }

    fn author_menu(&mut self, ui: &mut Ui) {
        let label = if self.author.trim().is_empty() { "모든 작성자".to_string() } else { self.author.clone() };
        let resp = dropdown_button(ui, Icon::Filter, &label, !self.author.trim().is_empty(), "작성자 필터");
        let mut pick: Option<String> = None;
        egui::Popup::menu(&resp).width(240.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(240.0);
            let mut s = self.author.clone();
            search_field(ui, "hist-author-input", &mut s, Icon::Search, "이름 또는 이메일", 228.0);
            if s != self.author {
                pick = Some(s);
            }
            ui.add_space(4.0);
            if menu_item(ui, "모든 작성자", None, self.author.trim().is_empty(), true).clicked() {
                pick = Some(String::new());
                ui.close();
            }
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for c in self.log.commits.iter().take(5000) {
                *counts.entry(c.author.as_str()).or_default() += 1;
            }
            let mut authors: Vec<(&str, usize)> = counts.into_iter().collect();
            authors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (a, n) in authors.iter().take(30) {
                    let cnt = n.to_string();
                    if menu_item(ui, a, Some(&cnt), self.author == *a, true).clicked() {
                        pick = Some(a.to_string());
                        ui.close();
                    }
                }
            });
        });
        if let Some(a) = pick {
            self.author = a;
            self.filter_changed = Some(Instant::now());
        }
    }

    fn ui_op_banner(&mut self, ui: &mut Ui) {
        let Some(st) = self.state.clone() else {
            if let Some(e) = &self.state_error {
                let e = e.clone();
                egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.label(RichText::new(e).color(Theme::current().red).font(fonts::medium(12.5)));
                });
            }
            return;
        };
        let Some(op) = st.op else { return };
        let t = Theme::current();
        let c = if st.conflicts.is_empty() { t.blue } else { t.orange };
        let busy = self.runner.as_ref().is_some_and(|r| r.pending > 0);
        egui::Frame::new()
            .fill(tint(c, if t.dark { 0.10 } else { 0.07 }))
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    kicons::paint(ui.painter(), r, Icon::Warning, c);
                    let title = if st.conflicts.is_empty() {
                        format!("{} 진행 중 · 편집을 위해 멈춤", op.label())
                    } else {
                        format!("{} 진행 중 · 충돌 {}개", op.label(), st.conflicts.len())
                    };
                    ui.label(RichText::new(title).font(fonts::semibold(13.0)).color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            if button_with(ui, None, "중단", ButtonKind::Secondary, true).clicked() {
                                self.submit(Op::Abort, false, false);
                            }
                            if button_with(ui, Some(Icon::Terminal), "터미널", ButtonKind::Ghost, true)
                                .on_hover_text("터미널에서 git status 실행")
                                .clicked()
                            {
                                self.events.push(HistoryEvent::RunInTerminal("git status".into()));
                            }
                            if op != RepoOp::Merge && button_with(ui, None, "건너뛰기", ButtonKind::Secondary, true).clicked() {
                                self.submit(Op::Skip, false, false);
                            }
                            if button_with(ui, Some(Icon::Play), "계속", ButtonKind::Primary, true).clicked() {
                                self.submit(Op::Continue, false, false);
                            }
                        });
                    });
                });
                if !st.conflicts.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        for f in &st.conflicts {
                            let r = file_chip(ui, f, t.orange);
                            if r.clicked() {
                                self.events.push(HistoryEvent::OpenFile(st.abs_path(&self.root, f)));
                            }
                            r.on_hover_text("충돌 파일 열기");
                        }
                    });
                }
            });
    }

    // ------------------------------------------------------------ 표

    fn ui_table(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        let rect = ui.max_rect();
        let w = rect.width();
        let mut cols = Columns::new(rect.left(), rect.right(), w);
        // 머리글
        let (hr, _) = ui.allocate_exact_size(vec2(w, HEADER_H), Sense::hover());
        let p = ui.painter();
        p.hline(hr.x_range(), hr.bottom() - 0.5, Stroke::new(1.0, t.border));
        let hf = fonts::semibold(11.5);
        p.text(pos2(hr.left() + GRAPH_PAD, hr.center().y), Align2::LEFT_CENTER, "커밋", hf.clone(), t.text_faint);
        if cols.show_author {
            p.text(pos2(cols.author.min, hr.center().y), Align2::LEFT_CENTER, "작성자", hf.clone(), t.text_faint);
        }
        if cols.show_date {
            p.text(pos2(cols.date.min, hr.center().y), Align2::LEFT_CENTER, "날짜", hf.clone(), t.text_faint);
        }
        p.text(pos2(cols.hash.min, hr.center().y), Align2::LEFT_CENTER, "해시", hf, t.text_faint);

        if self.log.commits.is_empty() {
            let loading = self.next.is_some() || self.reload_log;
            ui.add_space(40.0);
            if let Some(e) = self.log.error.clone() {
                kiln_common::widgets::empty_state(ui, Icon::Warning, &e, None);
            } else if loading {
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(18.0).color(t.text_faint));
                    ui.add_space(8.0);
                    ui.label(RichText::new("커밋을 불러오는 중…").color(t.text_dim).font(fonts::medium(13.0)));
                });
            } else if self.query.is_filtered() || self.query.branch != BranchFilter::All || self.query.first_parent {
                if kiln_common::widgets::empty_state(ui, Icon::Filter, "조건에 맞는 커밋이 없습니다", Some("필터 초기화")) {
                    self.set_query(LogQuery::default());
                }
            } else {
                kiln_common::widgets::empty_state(ui, Icon::History, "아직 커밋이 없습니다", None);
            }
            return;
        }

        let n = self.log.commits.len();
        let total = n + usize::from(!self.log.exhausted);
        let mut sa = egui::ScrollArea::vertical().id_salt("hist-table").auto_shrink([false, false]);
        if let Some(i) = self.scroll_to.take() {
            let view_h = ui.available_height();
            let y = i as f32 * ROW_H;
            let cur = ui.ctx().data(|d| d.get_temp::<f32>(Id::new(("hist-scroll", &self.root)))).unwrap_or(0.0);
            if y < cur || y + ROW_H > cur + view_h {
                sa = sa.vertical_scroll_offset((y - view_h / 2.0).max(0.0));
            }
        } else if let Some(d) = self.auto_scroll.take() {
            let cur = ui.ctx().data(|d| d.get_temp::<f32>(Id::new(("hist-scroll", &self.root)))).unwrap_or(0.0);
            sa = sa.vertical_scroll_offset((cur + d).max(0.0));
        }
        let filtered = self.query.is_filtered();
        let head = self.state.as_ref().and_then(|s| s.head.clone());
        let now = self.now();
        let selected: HashSet<String> = self.selected.iter().cloned().collect();
        let mut clicks: Vec<(usize, egui::Modifiers)> = Vec::new();
        let mut double: Option<usize> = None;
        let mut secondary: Option<usize> = None;
        let mut drag_start: Option<usize> = None;
        let mut row_rects: Vec<(usize, Rect)> = Vec::new();
        let mut menu: Option<MenuAction> = None;
        let drag_hint = self.drag.as_ref().and_then(|d| d.target.clone());
        let dragging: HashSet<String> = self.drag.as_ref().map(|d| d.moving.iter().cloned().collect()).unwrap_or_default();
        let out = sa.show_rows(ui, ROW_H, total, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            self.want_more = range.end + 120 >= n;
            cols.graph_lanes = range
                .clone()
                .filter_map(|i| self.log.lanes.get(i))
                .map(|l| l.width.min(MAX_LANES + 1))
                .max()
                .unwrap_or(1)
                .max(1);
            if filtered {
                cols.graph_lanes = 1;
            }
            for i in range {
                let (r, _) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
                if i >= n {
                    ui.painter().text(
                        pos2(r.left() + GRAPH_PAD, r.center().y),
                        Align2::LEFT_CENTER,
                        "더 불러오는 중…",
                        fonts::regular(12.0),
                        t.text_faint,
                    );
                    continue;
                }
                let c = &self.log.commits[i];
                let id = Id::new(("hist-row", &c.sha));
                let resp = ui.interact(r, id, Sense::click_and_drag());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected.contains(&c.sha), &c.subject));
                row_rects.push((i, r));
                let is_sel = selected.contains(&c.sha);
                let hovered = resp.hovered() && self.drag.is_none();
                paint_row_bg(ui.painter(), r, is_sel, hovered, dragging.contains(&c.sha));
                let lanes = if filtered { None } else { self.log.lanes.get(i) };
                paint_commit_row(ui.painter(), r, &cols, c, lanes, head.as_deref() == Some(c.sha.as_str()), is_sel, now);
                if let Some((ts, z)) = &drag_hint
                    && ts == &c.sha
                {
                    paint_drop_hint(ui.painter(), r, *z);
                }
                if resp.clicked() {
                    clicks.push((i, ui.input(|inp| inp.modifiers)));
                }
                if resp.double_clicked() {
                    double = Some(i);
                }
                if resp.secondary_clicked() {
                    secondary = Some(i);
                }
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    drag_start = Some(i);
                }
                resp.context_menu(|ui| {
                    if let Some(a) = self.context_menu(ui, &c.sha) {
                        menu = Some(a);
                        ui.close();
                    }
                });
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(("hist-scroll", &self.root)), out.state.offset.y));
        if menu.is_some() {
            self.menu_action = menu;
        }
        for (i, m) in clicks {
            self.click_row(i, m);
        }
        if let Some(i) = secondary {
            let sha = self.log.commits[i].sha.clone();
            if !self.selected.contains(&sha) {
                self.selected = vec![sha.clone()];
                self.anchor = Some(sha);
            }
        }
        if let Some(i) = double {
            self.events.push(HistoryEvent::OpenCommit(self.log.commits[i].sha.clone()));
        }
        if let Some(i) = drag_start {
            let sha = self.log.commits[i].sha.clone();
            if !self.selected.contains(&sha) {
                self.selected = vec![sha.clone()];
                self.anchor = Some(sha);
            }
            let moving = self.selection_sorted();
            let valid = moving.iter().all(|s| self.rewritable(s));
            self.drag = Some(DragState { moving, target: None, valid });
        }
        self.update_drag(ui, &row_rects, out.inner_rect);
    }

    fn update_drag(&mut self, ui: &Ui, rows: &[(usize, Rect)], viewport: Rect) {
        let Some(d) = &mut self.drag else { return };
        let (pos, down, released) = ui.input(|i| (i.pointer.hover_pos().or(i.pointer.interact_pos()), i.pointer.primary_down(), i.pointer.primary_released()));
        let mut target = None;
        let mut auto_scroll = None;
        if let Some(p) = pos {
            for &(i, r) in rows {
                if r.contains(p) || (p.y >= r.top() && p.y < r.bottom() && viewport.x_range().contains(p.x)) {
                    let sha = &self.log.commits[i].sha;
                    if !d.moving.contains(sha) {
                        let z = if p.y < r.center().y { DragZone::Above } else { DragZone::Below };
                        target = Some((sha.clone(), z));
                    }
                    break;
                }
            }
            // 가장자리 근처면 자동 스크롤
            let edge = 28.0;
            if down && viewport.x_range().contains(p.x) {
                if p.y < viewport.top() + edge {
                    auto_scroll = Some(-8.0);
                } else if p.y > viewport.bottom() - edge {
                    auto_scroll = Some(8.0);
                }
            }
            paint_drag_ghost(ui.ctx(), p, d.moving.len(), d.valid && target.as_ref().is_some_and(|(s, _)| self.linear.contains_key(s) || self.query.is_filtered()));
        }
        let target_ok = target.as_ref().is_some_and(|(s, _)| self.linear.contains_key(s) || (self.query.is_filtered()));
        d.target = if d.valid && target_ok { target } else { None };
        self.auto_scroll = auto_scroll;
        if released || !down {
            let d = self.drag.take().expect("drag");
            if let (Some((sha, z)), Some(p)) = (d.target, pos) {
                self.drop_menu = Some(DropMenu {
                    moving: d.moving,
                    target: sha,
                    place: if z == DragZone::Above { DropPlace::Above } else { DropPlace::Below },
                    pos: p,
                });
            }
        } else {
            ui.ctx().request_repaint();
        }
    }

    fn ui_drop_menu(&mut self, ctx: &egui::Context) {
        let Some(m) = self.drop_menu.clone() else { return };
        let t = Theme::current();
        let target_subject = self.commit(&m.target).map(|c| c.subject.clone()).unwrap_or_default();
        let mut choice: Option<Op> = None;
        let mut close = false;
        let area = egui::Area::new(Id::new("hist-drop-menu")).order(egui::Order::Foreground).fixed_pos(m.pos + vec2(8.0, 8.0)).show(ctx, |ui| {
            popup_frame().show(ui, |ui| {
                ui.set_width(280.0);
                let what = if m.moving.len() == 1 { "커밋 1개".to_string() } else { format!("커밋 {}개", m.moving.len()) };
                ui.label(RichText::new(format!("{what}를 놓을 동작")).font(fonts::semibold(12.0)).color(t.text_faint));
                ui.add_space(4.0);
                let where_ = if m.place == DropPlace::Above { "위" } else { "아래" };
                if ctx_item(ui, "여기로 순서 이동", Some(where_), true).clicked() {
                    choice = Some(Op::Move { moving: m.moving.clone(), target: m.target.clone(), place: m.place });
                }
                let label = format!("이 커밋에 스쿼시(fixup) · {}", elide_str(&target_subject, 24));
                if ctx_item(ui, &label, None, true).clicked() {
                    choice = Some(Op::Fixup { moving: m.moving.clone(), target: m.target.clone() });
                }
                ui.add_space(2.0);
                if ctx_item(ui, "취소", Some("Esc"), true).clicked() {
                    close = true;
                }
            });
        });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() && !area.response.hovered();
        if ctx.input(|i| i.key_pressed(Key::Escape)) || clicked_outside {
            close = true;
        }
        if let Some(op) = choice {
            self.drop_menu = None;
            let mut shas = m.moving.clone();
            shas.push(m.target.clone());
            self.start_rewrite(op, shas);
        } else if close {
            self.drop_menu = None;
        }
    }

    fn context_menu(&self, ui: &mut Ui, clicked: &str) -> Option<MenuAction> {
        let t = Theme::current();
        ui.set_width(270.0);
        let sel = if self.selected.iter().any(|s| s == clicked) { self.selection_sorted() } else { vec![clicked.to_string()] };
        let head = self.state.as_ref().and_then(|s| s.head.clone());
        let mut act = None;
        if sel.len() == 1 {
            let sha = sel[0].clone();
            let c = self.commit(&sha)?;
            let rewritable = self.rewritable(&sha);
            let is_head = head.as_deref() == Some(sha.as_str());
            let why = "현재 브랜치의 첫 부모 줄(병합 이후)에 있는 커밋만 다시 쓸 수 있습니다";
            menu_caption(ui, &format!("{} · {}", c.short(), elide_str(&c.subject, 34)));
            let branches: Vec<&RefLabel> = c.refs.iter().filter(|r| r.kind == RefKind::LocalBranch && !r.current).collect();
            for b in branches.iter().take(3) {
                if ctx_item(ui, &format!("'{}' 체크아웃", b.name), None, true).clicked() {
                    act = Some(MenuAction::Run(Op::Checkout(b.name.clone())));
                }
            }
            if ctx_item(ui, "체크아웃", Some("분리된 HEAD"), !is_head).clicked() {
                act = Some(MenuAction::CheckoutDetached(sha.clone()));
            }
            if ctx_item(ui, "여기서 브랜치 만들기…", None, true).clicked() {
                act = Some(MenuAction::NewBranch(sha.clone()));
            }
            if ctx_item(ui, "태그 만들기…", None, true).clicked() {
                act = Some(MenuAction::NewTag(sha.clone()));
            }
            menu_sep(ui);
            if ctx_item(ui, "체리픽", None, !self.linear.contains_key(&sha)).clicked() {
                act = Some(MenuAction::Run(Op::CherryPick(vec![sha.clone()])));
            }
            if ctx_item(ui, "되돌리기(revert)", None, true).clicked() {
                act = Some(MenuAction::Run(Op::Revert(sha.clone())));
            }
            if ctx_item(ui, "현재 브랜치를 여기로 리셋…", None, !is_head).clicked() {
                act = Some(MenuAction::Reset(sha.clone()));
            }
            menu_sep(ui);
            let r = ctx_item(ui, "메시지 수정…", Some(if is_head { "amend" } else { "rebase" }), rewritable);
            if r.clicked() {
                act = Some(MenuAction::Reword(sha.clone()));
            }
            if !rewritable {
                r.on_disabled_hover_text(why);
            }
            let r = menu_item_danger(ui, "커밋 삭제", rewritable);
            if r.clicked() {
                act = Some(MenuAction::DropCommits(vec![sha.clone()]));
            }
            let r = ctx_item(ui, "이 커밋부터 대화형 리베이스…", None, rewritable);
            if r.clicked() {
                act = Some(MenuAction::InteractiveFrom(sha.clone()));
            }
            menu_sep(ui);
            if ctx_item(ui, "커밋 diff 열기", Some("↵"), true).clicked() {
                act = Some(MenuAction::OpenCommit(sha.clone()));
            }
            if ctx_item(ui, "작업 트리와 비교", None, true).clicked() {
                act = Some(MenuAction::CompareWorktree(sha.clone()));
            }
            if ctx_item(ui, "해시 복사", Some(copy_shortcut(ui.ctx())), true).clicked() {
                act = Some(MenuAction::CopyHashes(vec![sha.clone()]));
            }
        } else {
            let contiguous = self.contiguous_linear(&sel);
            let all_rewritable = sel.iter().all(|s| self.rewritable(s));
            menu_caption(ui, &format!("커밋 {}개 선택됨", sel.len()));
            let r = ctx_item(ui, "하나로 스쿼시…", None, contiguous || (self.query.is_filtered() && all_rewritable));
            if r.clicked() {
                act = Some(MenuAction::Squash(sel.clone()));
            }
            if !contiguous {
                r.on_disabled_hover_text("현재 브랜치의 연속된 커밋(병합 제외)만 합칠 수 있습니다");
            }
            let r = menu_item_danger(ui, "선택 커밋 삭제", all_rewritable);
            if r.clicked() {
                act = Some(MenuAction::DropCommits(sel.clone()));
            }
            let mut oldest_first = sel.clone();
            oldest_first.reverse();
            if ctx_item(ui, "체리픽(순서대로)", Some("오래된 순"), true).clicked() {
                act = Some(MenuAction::Run(Op::CherryPick(oldest_first)));
            }
            menu_sep(ui);
            if ctx_item(ui, "해시 복사", Some(copy_shortcut(ui.ctx())), true).clicked() {
                act = Some(MenuAction::CopyHashes(sel.clone()));
            }
        }
        let _ = t;
        act
    }

    fn apply_menu(&mut self, ctx: &egui::Context, a: MenuAction) {
        match a {
            MenuAction::Run(op) => {
                let pushed = false;
                self.submit(op, false, pushed);
            }
            MenuAction::CheckoutDetached(sha) => {
                let c = self.commit(&sha).cloned();
                let subject = c.map(|c| c.subject).unwrap_or_default();
                let mut d = ConfirmDialog::plain(
                    Op::Checkout(sha.clone()),
                    "커밋을 체크아웃할까요?",
                    format!(
                        "{} \"{}\"을(를) 분리된 HEAD 상태로 체크아웃합니다. 이 상태에서 만든 커밋은 브랜치를 만들지 않으면 잃을 수 있습니다.",
                        short(&sha),
                        subject
                    ),
                    "체크아웃",
                );
                d.danger = false;
                self.dialog = Some(Dialog::Confirm(d));
            }
            MenuAction::NewBranch(sha) => self.dialog = Some(Dialog::Name(NameDialog::new(NameKind::Branch, sha))),
            MenuAction::NewTag(sha) => self.dialog = Some(Dialog::Name(NameDialog::new(NameKind::Tag, sha))),
            MenuAction::Reset(sha) => {
                let c = self.commit(&sha).cloned();
                let head_pushed = self.state.as_ref().and_then(|s| s.head.as_ref().map(|h| s.is_pushed(h))).unwrap_or(false);
                self.dialog = Some(Dialog::Reset(ResetDialog {
                    sha,
                    subject: c.map(|c| c.subject).unwrap_or_default(),
                    branch: self.state.as_ref().and_then(|s| s.branch.clone()),
                    mode: ResetMode::Mixed,
                    dirty: self.dirty(),
                    pushed: head_pushed,
                }));
            }
            MenuAction::Reword(sha) => {
                let pushed = self.rewrites_pushed(std::slice::from_ref(&sha));
                let root = self.root.clone();
                let s = sha.clone();
                let task = Task::spawn(ctx, move || history::commit_message(&root, &s));
                self.dialog = Some(Dialog::Message(MessageDialog::new(MessageKind::Reword(sha), task, pushed, self.dirty())));
            }
            MenuAction::Squash(shas) => {
                let pushed = self.rewrites_pushed(&shas);
                let root = self.root.clone();
                let mut oldest_first = shas.clone();
                oldest_first.reverse();
                let task = Task::spawn(ctx, move || history::combined_message(&root, &oldest_first));
                self.dialog = Some(Dialog::Message(MessageDialog::new(MessageKind::Squash(shas), task, pushed, self.dirty())));
            }
            MenuAction::DropCommits(shas) => {
                let pushed = self.rewrites_pushed(&shas);
                let body = if shas.len() == 1 {
                    let s = self.commit(&shas[0]).map(|c| c.subject.clone()).unwrap_or_default();
                    format!("{} \"{}\"을(를) 이력에서 지우고 이후 커밋을 다시 적용합니다. 이 커밋의 변경 내용은 사라집니다.", short(&shas[0]), s)
                } else {
                    format!("선택한 커밋 {}개를 이력에서 지우고 이후 커밋을 다시 적용합니다. 이 커밋들의 변경 내용은 사라집니다.", shas.len())
                };
                let mut d = ConfirmDialog::plain(Op::Drop(shas), if pushed { "푸시된 커밋을 삭제할까요?" } else { "커밋을 삭제할까요?" }, body, "삭제");
                d.pushed = pushed;
                d.dirty = self.dirty();
                d.autostash = d.dirty;
                self.dialog = Some(Dialog::Confirm(d));
            }
            MenuAction::InteractiveFrom(sha) => {
                let pushed = self.rewrites_pushed(std::slice::from_ref(&sha));
                let root = self.root.clone();
                let s = sha.clone();
                let task = Task::spawn(ctx, move || history::rebase_plan(&root, &s));
                self.dialog = Some(Dialog::Rebase(Box::new(RebaseDialog::new(task, pushed, self.dirty()))));
            }
            MenuAction::CompareWorktree(sha) => self.events.push(HistoryEvent::OpenDiff { from: sha, to: None }),
            MenuAction::CopyHashes(shas) => {
                ctx.copy_text(shas.join("\n"));
                self.events.push(HistoryEvent::Toast(if shas.len() == 1 { "해시를 복사했습니다".into() } else { format!("해시 {}개를 복사했습니다", shas.len()) }));
            }
            MenuAction::OpenCommit(sha) => self.events.push(HistoryEvent::OpenCommit(sha)),
        }
    }

    fn ui_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else { return };
        let out = d.show(ctx, &self.root);
        match out {
            DialogOutcome::Open => {}
            DialogOutcome::Cancel => self.dialog = None,
            DialogOutcome::Run { op, autostash, pushed } => {
                self.dialog = None;
                self.submit(op, autostash, pushed);
            }
        }
    }

    fn ui_notice(&mut self, ui: &Ui, table: Rect) {
        let Some(n) = self.notice.clone() else { return };
        let t = Theme::current();
        let w = (table.width() - 32.0).min(520.0);
        let pos = pos2(table.center().x - w / 2.0, table.bottom() - 16.0);
        let mut close = false;
        egui::Area::new(Id::new(("hist-notice", &self.root)))
            .order(egui::Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, if n.error { tint(t.red, 0.5) } else { t.border_strong }))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(t.shadow())
                    .inner_margin(Margin { left: 12, right: 8, top: 8, bottom: 8 })
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                            let (ic, col) = if n.error {
                                (Icon::Warning, t.red)
                            } else if n.warn {
                                (Icon::Warning, t.orange)
                            } else {
                                (Icon::Check, t.green)
                            };
                            ui.painter().circle_filled(r.center(), 9.0, tint(col, 0.18));
                            kicons::paint(ui.painter(), r.shrink(3.0), ic, col);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(&n.text).font(fonts::semibold(13.0)).color(t.text));
                                if let Some(d) = &n.detail {
                                    let d: String = d.lines().take(4).collect::<Vec<_>>().join("\n");
                                    ui.add(egui::Label::new(RichText::new(d).font(fonts::regular(12.0)).color(t.text_dim)).wrap());
                                }
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if kiln_common::widgets::icon_button(ui, Icon::Close, 24.0, false, "닫기").clicked() {
                                    close = true;
                                }
                                if let Some(old) = &n.undo
                                    && button_with(ui, Some(Icon::Undo), "되돌리기", ButtonKind::Secondary, true)
                                        .on_hover_text(format!("git reset --keep {}", short(old)))
                                        .clicked()
                                {
                                    self.submit(Op::Undo(old.clone()), false, false);
                                    close = true;
                                }
                                if n.force_push
                                    && button_with(ui, Some(Icon::Upload), "강제 푸시(--force-with-lease)", ButtonKind::Primary, true)
                                        .clicked()
                                {
                                    self.submit(Op::ForcePush, false, false);
                                    close = true;
                                }
                            });
                        });
                    });
            });
        if close {
            self.notice = None;
        }
    }

    // ------------------------------------------------------------ 상세 패널

    fn ui_detail(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        if self.selected.len() > 1 {
            self.ui_multi_detail(ui);
            return;
        }
        let Some(sha) = self.selected.first().cloned() else {
            ui.add_space(60.0);
            kiln_common::widgets::empty_state(ui, Icon::History, "커밋을 선택하세요", None);
            return;
        };
        let info = self.detail_info.clone().filter(|d| d.sha == sha);
        let Some(info) = info else {
            ui.add_space(60.0);
            if let Some(e) = &self.detail_error {
                kiln_common::widgets::empty_state(ui, Icon::Warning, e, None);
            } else {
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(16.0).color(t.text_faint));
                });
            }
            return;
        };
        let now = self.now();
        egui::ScrollArea::vertical().id_salt("hist-detail").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 18, right: 16, top: 16, bottom: 20 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let subject = info.message.lines().next().unwrap_or("").to_string();
                let body = info.message.split_once('\n').map(|(_, b)| b.trim().to_string()).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(&subject).font(fonts::semibold(15.5)).color(t.text)).wrap().selectable(true));
                if !body.is_empty() {
                    ui.add_space(8.0);
                    ui.add(egui::Label::new(RichText::new(&body).font(fonts::regular(13.0)).color(t.text_dim)).wrap().selectable(true));
                }
                if !info.refs.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                        for r in &info.refs {
                            let (size, _) = ref_pill_size(ui.painter(), r);
                            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                            paint_ref_pill(ui.painter(), rect, r);
                        }
                    });
                }
                ui.add_space(14.0);
                // 작성자 카드
                egui::Frame::new()
                    .fill(t.bg_elevated)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(9))
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 6.0;
                        person_row(ui, &info.author, &info.email, &format!("{} · {}", info.author_date_text, relative_time(info.author_date, now)));
                        if info.committer != info.author || info.committer_email != info.email {
                            meta_row(ui, "커미터", &format!("{} · {}", info.committer, info.commit_date_text));
                        }
                        let hash_resp = meta_link_row(ui, "해시", &info.sha, true);
                        if hash_resp.clicked() {
                            ui.ctx().copy_text(info.sha.clone());
                            self.events.push(HistoryEvent::Toast("해시를 복사했습니다".into()));
                        }
                        hash_resp.on_hover_text("클릭해 복사");
                        if info.parents.is_empty() {
                            meta_row(ui, "부모", "없음(루트 커밋)");
                        }
                        for (k, p) in info.parents.iter().enumerate() {
                            let label = if k == 0 { "부모" } else { "" };
                            let subject = self.commit(p).map(|c| c.subject.clone()).unwrap_or_default();
                            let r = meta_link_row(ui, label, &format!("{}  {}", short(p), subject), false);
                            if r.clicked() {
                                if self.log.index.contains_key(p) {
                                    self.select(p);
                                } else {
                                    self.events.push(HistoryEvent::OpenCommit(p.clone()));
                                }
                            }
                        }
                    });
                ui.add_space(16.0);
                let (adds, dels) = info.files.iter().fold((0u32, 0u32), |(a, d), f| (a + f.added.unwrap_or(0), d + f.removed.unwrap_or(0)));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new("변경된 파일").font(fonts::semibold(12.5)).color(t.text_dim));
                    count_badge(ui, info.files.len());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("−{dels}")).font(fonts::mono(11.5)).color(t.red));
                        ui.label(RichText::new(format!("+{adds}")).font(fonts::mono(11.5)).color(t.green));
                    });
                });
                ui.add_space(6.0);
                let base = info.base_rev();
                for f in &info.files {
                    let r = file_row(ui, f);
                    if r.clicked() {
                        self.events.push(HistoryEvent::OpenDiff { from: base.clone(), to: Some(info.sha.clone()) });
                    }
                    if r.double_clicked() {
                        self.events.push(HistoryEvent::OpenCommit(info.sha.clone()));
                    }
                }
                if info.files.is_empty() {
                    ui.label(RichText::new("변경된 파일이 없습니다").font(fonts::regular(12.5)).color(t.text_faint));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if button_with(ui, Some(Icon::Eye), "커밋 diff 열기", ButtonKind::Secondary, true).clicked() {
                        self.events.push(HistoryEvent::OpenCommit(info.sha.clone()));
                    }
                    if button_with(ui, Some(Icon::Columns), "작업 트리와 비교", ButtonKind::Ghost, true).clicked() {
                        self.events.push(HistoryEvent::OpenDiff { from: info.sha.clone(), to: None });
                    }
                });
            });
        });
    }

    fn ui_multi_detail(&mut self, ui: &mut Ui) {
        let t = Theme::current();
        let sel = self.selection_sorted();
        let contiguous = self.contiguous_linear(&sel);
        let all_rw = sel.iter().all(|s| self.rewritable(s));
        egui::Frame::new().inner_margin(Margin { left: 18, right: 16, top: 16, bottom: 16 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("커밋 {}개 선택됨", sel.len())).font(fonts::semibold(15.5)).color(t.text));
            ui.add_space(4.0);
            let hint = if contiguous { "현재 브랜치의 연속 구간입니다" } else { "연속 구간이 아니거나 현재 브랜치 밖의 커밋이 있습니다" };
            ui.label(RichText::new(hint).font(fonts::regular(12.5)).color(t.text_faint));
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.add_enabled_ui(contiguous, |ui| {
                    if button_with(ui, None, "스쿼시…", ButtonKind::Primary, true).clicked() {
                        self.menu_action = Some(MenuAction::Squash(sel.clone()));
                    }
                });
                ui.add_enabled_ui(all_rw, |ui| {
                    if button_with(ui, Some(Icon::Trash), "선택 삭제", ButtonKind::Secondary, true).clicked() {
                        self.menu_action = Some(MenuAction::DropCommits(sel.clone()));
                    }
                });
                if button_with(ui, None, "순서대로 체리픽", ButtonKind::Secondary, true).clicked() {
                    let mut v = sel.clone();
                    v.reverse();
                    self.menu_action = Some(MenuAction::Run(Op::CherryPick(v)));
                }
            });
            ui.add_space(14.0);
            egui::ScrollArea::vertical().id_salt("hist-multi").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for s in &sel {
                    let Some(c) = self.commit(s) else { continue };
                    let w = ui.available_width();
                    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
                    let p = ui.painter();
                    p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, c.short(), fonts::mono(11.5), t.text_faint);
                    let g = elide(p, &c.subject, fonts::regular(13.0), t.text, r.width() - 74.0);
                    p.galley(pos2(r.left() + 70.0, r.center().y - g.size().y / 2.0), g, t.text);
                }
            });
        });
    }
}

// ---------------------------------------------------------------- 표 열

struct Columns {
    subject: egui::Rangef,
    author: egui::Rangef,
    date: egui::Rangef,
    hash: egui::Rangef,
    show_author: bool,
    /// 보이는 행들의 최대 레인 수. 제목 시작 위치를 맞춘다.
    graph_lanes: usize,
    show_date: bool,
}

impl Columns {
    fn new(left: f32, right: f32, w: f32) -> Self {
        let hash_w = 82.0;
        let date_w = if w > 560.0 { 92.0 } else { 0.0 };
        let author_w = if w > 700.0 { (w * 0.18).clamp(120.0, 190.0) } else { 0.0 };
        let hash = egui::Rangef::new(right - hash_w, right - 10.0);
        let date = egui::Rangef::new(hash.min - date_w, hash.min - 8.0);
        let author = egui::Rangef::new(hash.min - date_w - author_w, hash.min - date_w - 12.0);
        let subject = egui::Rangef::new(left, hash.min - date_w - author_w - 12.0);
        Self { subject, author, date, hash, show_author: author_w > 0.0, show_date: date_w > 0.0, graph_lanes: 1 }
    }
}

fn lane_color(t: &Theme, c: u16) -> Color32 {
    let pal = [t.accent, t.green, t.orange, t.purple, t.blue, t.yellow, t.red, t.ansi[6]];
    pal[c as usize % pal.len()]
}

fn paint_row_bg(p: &Painter, r: Rect, selected: bool, hovered: bool, dragging: bool) {
    let t = Theme::current();
    let rr = r.shrink2(vec2(4.0, 1.0));
    if selected {
        p.rect_filled(rr, CornerRadius::same(6), t.accent_soft(if t.dark { 40 } else { 30 }));
    } else if hovered {
        p.rect_filled(rr, CornerRadius::same(6), t.bg_hover);
    }
    if dragging {
        p.rect_stroke(rr, CornerRadius::same(6), Stroke::new(1.0, tint(t.accent, 0.6)), StrokeKind::Inside);
    }
}

fn paint_drop_hint(p: &Painter, r: Rect, z: DragZone) {
    let t = Theme::current();
    let rr = r.shrink2(vec2(4.0, 1.0));
    p.rect_filled(rr, CornerRadius::same(6), t.accent_soft(if t.dark { 26 } else { 18 }));
    p.rect_stroke(rr, CornerRadius::same(6), Stroke::new(1.0, tint(t.accent, 0.55)), StrokeKind::Inside);
    let y = if z == DragZone::Above { r.top() } else { r.bottom() };
    let x0 = r.left() + 6.0;
    p.line_segment([pos2(x0 + 4.0, y), pos2(r.right() - 6.0, y)], Stroke::new(2.0, t.accent));
    p.circle(pos2(x0 + 2.0, y), 3.5, t.bg, Stroke::new(2.0, t.accent));
}

fn paint_drag_ghost(ctx: &egui::Context, pos: Pos2, n: usize, ok: bool) {
    let t = Theme::current();
    let text = if n == 1 { "커밋 1개 이동".to_string() } else { format!("커밋 {n}개 이동") };
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("hist-drag-ghost")));
    let g = p.layout_no_wrap(text, fonts::medium(12.0), if ok { t.accent_fg } else { t.text });
    let r = Rect::from_min_size(pos + vec2(14.0, 10.0), g.size() + vec2(20.0, 10.0));
    p.rect_filled(r, CornerRadius::same(7), if ok { t.accent } else { t.bg_elevated });
    if !ok {
        p.rect_stroke(r, CornerRadius::same(7), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
    }
    p.galley(r.min + vec2(10.0, 5.0), g, t.text);
}

#[allow(clippy::too_many_arguments)]
fn paint_commit_row(p: &Painter, r: Rect, cols: &Columns, c: &LogCommit, lanes: Option<&LaneRow>, is_head: bool, selected: bool, now: i64) {
    let t = Theme::current();
    let lx = |lane: usize| r.left() + GRAPH_PAD + lane.min(MAX_LANES) as f32 * LANE_W + LANE_W / 2.0;
    let top = r.top();
    let mid = r.center().y;
    let bot = r.bottom();
    let clip = p.with_clip_rect(p.clip_rect().intersect(Rect::from_min_max(pos2(r.left(), top), pos2(cols.subject.max, bot))));
    let width = cols.graph_lanes;
    let (col, ccolor) = match lanes {
        Some(l) => {
            let sw = 1.7;
            for e in &l.pass {
                let s = Stroke::new(sw, lane_color(&t, e.color));
                if e.from == e.to {
                    clip.line_segment([pos2(lx(e.from), top), pos2(lx(e.to), bot)], s);
                } else {
                    curve(&clip, pos2(lx(e.from), top), pos2(lx(e.to), bot), s);
                }
            }
            for e in &l.up {
                let s = Stroke::new(sw, lane_color(&t, e.color));
                if e.from == e.to {
                    clip.line_segment([pos2(lx(e.from), top), pos2(lx(e.to), mid)], s);
                } else {
                    curve(&clip, pos2(lx(e.from), top), pos2(lx(e.to), mid), s);
                }
            }
            for e in &l.down {
                let s = Stroke::new(sw, lane_color(&t, e.color));
                if e.from == e.to {
                    clip.line_segment([pos2(lx(e.from), mid), pos2(lx(e.to), bot)], s);
                } else {
                    curve(&clip, pos2(lx(e.from), mid), pos2(lx(e.to), bot), s);
                }
            }
            (l.col, lane_color(&t, l.color))
        }
        None => (0, t.text_faint),
    };
    let center = pos2(lx(col), mid);
    let bg = if selected { lerp_color(t.bg, t.accent, if t.dark { 0.16 } else { 0.12 }) } else { t.bg };
    if is_head {
        clip.circle_filled(center, 6.5, tint(ccolor, 0.25));
        clip.circle(center, 4.2, bg, Stroke::new(2.2, ccolor));
        clip.circle_filled(center, 1.8, ccolor);
    } else if c.is_merge() {
        clip.circle(center, 3.8, bg, Stroke::new(1.8, ccolor));
    } else {
        clip.circle(center, 4.0, ccolor, Stroke::new(1.5, bg));
    }

    // 참조 알약 + 제목
    let mut x = r.left() + GRAPH_PAD + width as f32 * LANE_W + 8.0;
    let max_x = cols.subject.max;
    let sp = p.with_clip_rect(p.clip_rect().intersect(Rect::from_min_max(pos2(x, top), pos2(max_x, bot))));
    let mut shown = 0;
    for rf in &c.refs {
        if shown == 3 {
            let rest = c.refs.len() - shown;
            let g = sp.layout_no_wrap(format!("+{rest}"), fonts::medium(11.0), t.text_faint);
            let pr = Rect::from_min_size(pos2(x, mid - 9.0), vec2(g.size().x + 10.0, 18.0));
            sp.rect_stroke(pr, CornerRadius::same(9), Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
            sp.galley(pr.center() - g.size() / 2.0, g, t.text_faint);
            x = pr.right() + 6.0;
            break;
        }
        let (size, _) = ref_pill_size(&sp, rf);
        if x + size.x > max_x - 80.0 {
            break;
        }
        let pr = Rect::from_min_size(pos2(x, mid - size.y / 2.0), size);
        paint_ref_pill(&sp, pr, rf);
        x = pr.right() + 4.0;
        shown += 1;
    }
    if shown > 0 {
        x += 2.0;
    }
    let font = if is_head { fonts::medium(13.0) } else { fonts::regular(13.0) };
    let g = elide(&sp, &c.subject, font, t.text, (max_x - x).max(10.0));
    sp.galley(pos2(x, mid - g.size().y / 2.0), g, t.text);

    // 작성자
    if cols.show_author {
        let ar = Rect::from_center_size(pos2(cols.author.min + 8.0, mid), vec2(16.0, 16.0));
        let hc = hue_color(&c.author);
        p.circle_filled(ar.center(), 8.0, tint(hc, if t.dark { 0.28 } else { 0.22 }));
        let initial: String = c.author.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
        p.text(ar.center(), Align2::CENTER_CENTER, initial, fonts::semibold(9.5), if t.dark { hc } else { lerp_color(hc, Color32::BLACK, 0.35) });
        let g = elide(p, &c.author, fonts::regular(12.5), t.text_dim, cols.author.max - cols.author.min - 22.0);
        p.galley(pos2(cols.author.min + 22.0, mid - g.size().y / 2.0), g, t.text_dim);
    }
    if cols.show_date {
        let g = elide(p, &relative_time(c.date, now), fonts::regular(12.5), t.text_dim, cols.date.max - cols.date.min);
        p.galley(pos2(cols.date.min, mid - g.size().y / 2.0), g, t.text_dim);
    }
    p.text(pos2(cols.hash.min, mid), Align2::LEFT_CENTER, &c.sha[..c.sha.len().min(8)], fonts::mono(11.5), t.text_faint);
}

fn curve(p: &Painter, a: Pos2, b: Pos2, s: Stroke) {
    let dy = (b.y - a.y) * 0.55;
    let shape = CubicBezierShape::from_points_stroke([a, pos2(a.x, a.y + dy), pos2(b.x, b.y - dy), b], false, Color32::TRANSPARENT, s);
    p.add(shape);
}

fn ref_style(r: &RefLabel) -> (Color32, Color32, bool) {
    let t = Theme::current();
    match (r.current, r.kind) {
        (true, _) => (t.accent, t.accent_fg, true),
        (_, RefKind::Head) => (t.orange, if t.dark { Color32::from_rgb(24, 18, 10) } else { Color32::WHITE }, true),
        (_, RefKind::LocalBranch) => (t.green, t.green, false),
        (_, RefKind::RemoteBranch) => (t.purple, t.purple, false),
        (_, RefKind::Tag) => (t.yellow, t.yellow, false),
    }
}

fn ref_pill_size(p: &Painter, r: &RefLabel) -> (Vec2, Arc<Galley>) {
    let (_, fg, _) = ref_style(r);
    let g = p.layout_no_wrap(elide_str(&r.name, 28), fonts::medium(11.0), fg);
    (vec2(g.size().x + 26.0, 18.0), g)
}

fn paint_ref_pill(p: &Painter, rect: Rect, r: &RefLabel) {
    let t = Theme::current();
    let (base, fg, solid) = ref_style(r);
    let (_, g) = ref_pill_size(p, r);
    if solid {
        p.rect_filled(rect, CornerRadius::same(9), base);
    } else {
        p.rect_filled(rect, CornerRadius::same(9), tint(base, if t.dark { 0.15 } else { 0.11 }));
        p.rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, tint(base, 0.30)), StrokeKind::Inside);
    }
    let ic = Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(11.0, 11.0));
    match r.kind {
        RefKind::Tag => paint_tag_glyph(p, ic, fg),
        RefKind::Head => {
            p.circle_stroke(ic.center(), 3.5, Stroke::new(1.4, fg));
        }
        RefKind::RemoteBranch => paint_cloud_glyph(p, ic, fg),
        RefKind::LocalBranch => kicons::paint(p, ic, Icon::Branch, fg),
    }
    p.galley(pos2(rect.left() + 19.0, rect.center().y - g.size().y / 2.0), g, fg);
}

fn paint_tag_glyph(p: &Painter, r: Rect, c: Color32) {
    let s = r.width() / 12.0;
    let o = r.min;
    let pt = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let pts = vec![pt(1.0, 1.5), pt(6.5, 1.5), pt(11.0, 6.0), pt(6.5, 10.5), pt(1.0, 10.5)];
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.3, c)));
    p.circle_filled(pt(4.0, 6.0), 1.1 * s, c);
}

fn paint_cloud_glyph(p: &Painter, r: Rect, c: Color32) {
    let s = r.width() / 12.0;
    let o = r.min;
    let pt = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let st = Stroke::new(1.3, c);
    p.circle_stroke(pt(4.3, 6.8), 2.6 * s, st);
    p.circle_stroke(pt(7.4, 5.4), 3.2 * s, st);
    p.line_segment([pt(2.0, 9.4), pt(10.3, 9.4)], st);
}

// ---------------------------------------------------------------- 공용 위젯

pub(crate) fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

pub(crate) fn elide_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut v: String = s.chars().take(max.saturating_sub(1)).collect();
        v.push('…');
        v
    }
}

pub(crate) fn elide(p: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, color, valign: Align::Center, ..Default::default() });
    job.wrap.max_width = max_w.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    p.layout_job(job)
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn copy_shortcut(ctx: &egui::Context) -> &'static str {
    if ctx.os() == egui::os::OperatingSystem::Mac { "⌘C" } else { "Ctrl+C" }
}

pub(crate) fn popup_frame() -> egui::Frame {
    let t = Theme::current();
    egui::Frame::new()
        .fill(t.bg_elevated)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(9))
        .shadow(t.shadow())
        .inner_margin(Margin::same(6))
}

/// 아이콘이 붙은 한 줄 입력칸.
pub(crate) fn search_field(ui: &mut Ui, id: &str, value: &mut String, icon: Icon, hint: &str, width: f32) -> egui::Response {
    let t = Theme::current();
    let id = ui.id().with(id);
    let focused = ui.memory(|m| m.has_focus(id));
    let resp = kiln_common::widgets::input_frame(focused, false)
        .inner_margin(Margin { left: 8, right: 6, top: 4, bottom: 4 })
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.spacing_mut().interact_size.y = 20.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::hover());
                kicons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), icon, t.text_faint);
                let clear_w = if value.is_empty() { 0.0 } else { 20.0 };
                let te = egui::TextEdit::singleline(value)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new(hint).color(t.text_faint))
                    .font(fonts::regular(13.0))
                    .desired_width(ui.available_width() - clear_w)
                    .margin(Margin::ZERO);
                let r = ui.add(te);
                if !value.is_empty() {
                    let (cr, cresp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                    let col = if cresp.hovered() { t.text } else { t.text_faint };
                    kicons::paint(ui.painter(), cr.shrink(3.0), Icon::Close, col);
                    if cresp.clicked() {
                        value.clear();
                    }
                }
                r
            })
            .inner
        });
    resp.inner
}

fn labeled_toggle(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = Theme::current();
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let r = toggle(ui, on);
        let l = ui.add(egui::Label::new(RichText::new(label).font(fonts::medium(12.5)).color(if *on { t.text } else { t.text_dim })).sense(Sense::click()));
        if l.clicked() {
            *on = !*on;
            let mut r = r;
            r.mark_changed();
            return r;
        }
        r
    });
    let enabled = ui.is_enabled();
    let on_now = *on;
    inner.inner.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on_now, label));
    inner.inner
}

fn dropdown_button(ui: &mut Ui, icon: Icon, label: &str, active: bool, a11y: &str) -> egui::Response {
    let t = Theme::current();
    let text = elide_str(label, 22);
    let g = ui.painter().layout_no_wrap(text, fonts::medium(12.5), t.text);
    let size = vec2(g.size().x + 50.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, a11y));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let (fill, stroke) = if active {
        (t.accent_soft(if t.dark { 30 } else { 20 }), tint(t.accent, 0.45))
    } else if resp.hovered() || open {
        (t.bg_hover, t.border_strong)
    } else {
        (t.bg_input, t.border_strong)
    };
    ui.painter().rect(rect, CornerRadius::same(7), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let fg = if active { t.accent } else { t.text_dim };
    kicons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), vec2(13.0, 13.0)), icon, fg);
    ui.painter().galley(pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, if active { t.accent } else { t.text });
    kicons::paint(ui.painter(), Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(10.0, 10.0)), Icon::ChevronDown, t.text_faint);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

pub(crate) fn menu_heading(ui: &mut Ui, text: &str) {
    let t = Theme::current();
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(fonts::semibold(11.0)).color(t.text_faint));
    ui.add_space(2.0);
}

fn menu_caption(ui: &mut Ui, text: &str) {
    let t = Theme::current();
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    let g = elide(ui.painter(), text, fonts::medium(11.5), t.text_faint, w - 16.0);
    ui.painter().galley(pos2(r.left() + 8.0, r.center().y - g.size().y / 2.0), g, t.text_faint);
}

fn menu_sep(ui: &mut Ui) {
    let t = Theme::current();
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 9.0), Sense::hover());
    ui.painter().hline(r.shrink2(vec2(6.0, 0.0)).x_range(), r.center().y, Stroke::new(1.0, t.border));
}

/// 메뉴 한 줄. `hint` 는 오른쪽 흐린 글자, `checked` 면 앞에 체크 표시.
pub(crate) fn menu_item(ui: &mut Ui, label: &str, hint: Option<&str>, checked: bool, enabled: bool) -> egui::Response {
    menu_item_colored(ui, label, hint, Some(checked), enabled, None)
}

/// 체크 칸이 없는 메뉴 한 줄(컨텍스트 메뉴용).
fn ctx_item(ui: &mut Ui, label: &str, hint: Option<&str>, enabled: bool) -> egui::Response {
    menu_item_colored(ui, label, hint, None, enabled, None)
}

fn menu_item_danger(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    menu_item_colored(ui, label, None, None, enabled, Some(Theme::current().red))
}

fn menu_item_colored(
    ui: &mut Ui,
    label: &str,
    hint: Option<&str>,
    checked: Option<bool>,
    enabled: bool,
    color: Option<Color32>,
) -> egui::Response {
    let t = Theme::current();
    let w = ui.available_width().max(160.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(6), if color.is_some() { tint(t.red, 0.14) } else { t.bg_hover });
    }
    let fg = if !enabled { t.text_faint } else { color.unwrap_or(t.text) };
    let mut x = rect.left() + 10.0;
    if checked == Some(true) {
        kicons::paint(ui.painter(), Rect::from_center_size(pos2(x + 5.0, rect.center().y), vec2(12.0, 12.0)), Icon::Check, t.accent);
    }
    if checked.is_some() {
        x += 18.0;
    }
    let hint_w = hint.map(|h| ui.painter().layout_no_wrap(h.to_string(), fonts::regular(11.5), t.text_faint).size().x + 12.0).unwrap_or(0.0);
    let g = elide(ui.painter(), label, fonts::regular(13.0), fg, rect.right() - x - hint_w - 8.0);
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, fg);
    if let Some(h) = hint {
        ui.painter().text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, h, fonts::regular(11.5), t.text_faint);
    }
    resp
}

fn file_chip(ui: &mut Ui, path: &str, c: Color32) -> egui::Response {
    let t = Theme::current();
    let g = ui.painter().layout_no_wrap(path.to_string(), fonts::mono(11.5), t.text);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, path));
    let fill = if resp.hovered() { t.bg_hover } else { t.bg_elevated };
    ui.painter().rect(r, CornerRadius::same(6), fill, Stroke::new(1.0, tint(c, 0.4)), StrokeKind::Inside);
    ui.painter().text(pos2(r.left() + 11.0, r.center().y), Align2::CENTER_CENTER, "U", fonts::semibold(10.5), c);
    ui.painter().galley(pos2(r.left() + 22.0, r.center().y - g.size().y / 2.0), g, t.text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn count_badge(ui: &mut Ui, n: usize) {
    let t = Theme::current();
    let g = ui.painter().layout_no_wrap(n.to_string(), fonts::medium(11.0), t.text_dim);
    let (r, _) = ui.allocate_exact_size(vec2((g.size().x + 12.0).max(20.0), 18.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(9), t.bg_hover);
    ui.painter().galley(r.center() - g.size() / 2.0, g, t.text_dim);
}

fn person_row(ui: &mut Ui, name: &str, email: &str, when: &str) {
    let t = Theme::current();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        let hc = hue_color(name);
        ui.painter().circle_filled(r.center(), 15.0, tint(hc, if t.dark { 0.26 } else { 0.2 }));
        let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        ui.painter().text(r.center(), Align2::CENTER_CENTER, initial, fonts::semibold(13.0), if t.dark { hc } else { lerp_color(hc, Color32::BLACK, 0.35) });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(name).font(fonts::semibold(13.0)).color(t.text));
                ui.label(RichText::new(email).font(fonts::regular(12.0)).color(t.text_faint));
            });
            ui.label(RichText::new(when).font(fonts::regular(12.0)).color(t.text_dim));
        });
    });
}

fn meta_row(ui: &mut Ui, key: &str, value: &str) {
    let t = Theme::current();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (r, _) = ui.allocate_exact_size(vec2(52.0, 20.0), Sense::hover());
        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, key, fonts::medium(12.0), t.text_faint);
        ui.add(egui::Label::new(RichText::new(value).font(fonts::regular(12.5)).color(t.text_dim)).truncate());
    });
}

fn meta_link_row(ui: &mut Ui, key: &str, value: &str, mono: bool) -> egui::Response {
    let t = Theme::current();
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, value));
    let p = ui.painter();
    p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, key, fonts::medium(12.0), t.text_faint);
    let font = if mono { fonts::mono(11.5) } else { fonts::regular(12.5) };
    let col = if resp.hovered() { t.accent } else { t.text_dim };
    let g = elide(p, value, font, col, r.width() - 52.0);
    p.galley(pos2(r.left() + 52.0, r.center().y - g.size().y / 2.0), g, col);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn file_row(ui: &mut Ui, f: &history::ChangedFile) -> egui::Response {
    let t = Theme::current();
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &f.path));
    if resp.hovered() {
        ui.painter().rect_filled(r.expand2(vec2(6.0, 0.0)), CornerRadius::same(6), t.bg_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let sc = match f.status {
        'A' => t.green,
        'D' => t.red,
        'R' | 'C' => t.blue,
        'M' => t.yellow,
        _ => t.text_dim,
    };
    let br = Rect::from_center_size(pos2(r.left() + 9.0, r.center().y), vec2(18.0, 18.0));
    p.rect_filled(br, CornerRadius::same(5), tint(sc, if t.dark { 0.16 } else { 0.12 }));
    p.text(br.center(), Align2::CENTER_CENTER, f.status.to_string(), fonts::semibold(10.5), sc);
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_string()),
        None => (String::new(), f.path.clone()),
    };
    let stats = match (f.added, f.removed) {
        (Some(a), Some(d)) => format!("+{a} −{d}"),
        _ => "바이너리".to_string(),
    };
    let sg = p.layout_no_wrap(stats, fonts::mono(11.0), t.text_faint);
    let avail = r.width() - 28.0 - sg.size().x - 10.0;
    let mut job = LayoutJob::default();
    job.append(&name, 0.0, TextFormat { font_id: fonts::medium(12.5), color: t.text, valign: Align::Center, ..Default::default() });
    let tail = match &f.old_path {
        Some(o) => format!("  ← {o}"),
        None => format!("  {dir}"),
    };
    job.append(&tail, 0.0, TextFormat { font_id: fonts::regular(11.5), color: t.text_faint, valign: Align::Center, ..Default::default() });
    job.wrap.max_width = avail.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = p.layout_job(job);
    p.galley(pos2(r.left() + 26.0, r.center().y - g.size().y / 2.0), g, t.text);
    p.galley(pos2(r.right() - sg.size().x, r.center().y - sg.size().y / 2.0), sg, t.text_faint);
    resp
}
