//! 커밋 이력(Log) 백엔드: 페이지 단위 로그 스트리밍, 그래프 레인, 커밋 상세, 이력 재작성.
//!
//! 모든 함수는 블로킹이며 백그라운드 스레드에서 호출한다. 재작성은 `git rebase -i` 를
//! `GIT_SEQUENCE_EDITOR` 로 미리 만든 todo 파일을 복사하게 해 비대화형으로 실행한다.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

use crate::cmd::{GitError, GitResult, Mode, git, git_combined, git_stdin};
use crate::repo::{RepoOp, in_progress_op};

/// 빈 트리 개체 해시. 루트 커밋 비교의 기준으로 쓴다.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// 이력 화면이 앱에 요청하는 동작.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    /// 커밋 diff 탭 열기.
    OpenCommit(String),
    /// 두 리비전 비교. `to` 가 `None` 이면 작업 트리와 비교.
    OpenDiff { from: String, to: Option<String> },
    /// 에디터로 파일 열기(절대 경로). 충돌 파일 목록에서 쓴다.
    OpenFile(PathBuf),
    /// 터미널에서 명령 실행.
    RunInTerminal(String),
    /// 앱 토스트로 보여줄 알림 문구.
    Toast(String),
}

// ---------------------------------------------------------------- 참조

/// 참조 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// 분리된 HEAD.
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

/// 커밋에 붙은 참조 하나.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefLabel {
    pub kind: RefKind,
    /// 짧은 이름(`main`, `origin/main`, `v1.0`, `HEAD`).
    pub name: String,
    /// HEAD 가 가리키는 현재 브랜치인지.
    pub current: bool,
}

/// `--decorate=full` 의 `%D` 문자열을 참조 목록으로 바꾼다. 현재 브랜치, 로컬, 태그, 원격 순으로 정렬한다.
pub fn parse_decorations(d: &str) -> Vec<RefLabel> {
    let mut v = Vec::new();
    for part in d.split(", ").map(str::trim).filter(|s| !s.is_empty()) {
        let (current, name) = match part.strip_prefix("HEAD -> ") {
            Some(n) => (true, n),
            None => (false, part),
        };
        if name == "HEAD" {
            v.push(RefLabel { kind: RefKind::Head, name: "HEAD".into(), current: false });
        } else if let Some(n) = name.strip_prefix("refs/heads/") {
            v.push(RefLabel { kind: RefKind::LocalBranch, name: n.into(), current });
        } else if let Some(n) = name.strip_prefix("tag: refs/tags/") {
            v.push(RefLabel { kind: RefKind::Tag, name: n.into(), current: false });
        } else if let Some(n) = name.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                continue;
            }
            v.push(RefLabel { kind: RefKind::RemoteBranch, name: n.into(), current: false });
        }
    }
    v.sort_by_key(|r| match (r.current, r.kind) {
        (true, _) => 0,
        (_, RefKind::Head) => 1,
        (_, RefKind::LocalBranch) => 2,
        (_, RefKind::Tag) => 3,
        (_, RefKind::RemoteBranch) => 4,
    });
    v
}

// ---------------------------------------------------------------- 로그

/// 로그 한 행.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// 작성 시각(유닉스 초).
    pub date: i64,
    pub refs: Vec<RefLabel>,
    pub subject: String,
}

impl LogCommit {
    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(8)]
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// 브랜치 필터.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BranchFilter {
    /// 모든 로컬·원격 브랜치, 태그, HEAD.
    #[default]
    All,
    /// HEAD 에서 닿는 커밋.
    Current,
    /// 지정 리비전(브랜치 이름 등)에서 닿는 커밋.
    Named(String),
}

/// 로그 조회 조건.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogQuery {
    pub branch: BranchFilter,
    /// 작성자 이름/메일 부분 일치(대소문자 무시).
    pub author: String,
    /// 경로 제한(저장소 최상위 기준).
    pub path: String,
    /// 제목 부분 일치 또는 해시 접두사(대소문자 무시).
    pub text: String,
    pub first_parent: bool,
}

impl LogQuery {
    /// 그래프가 끊기는 필터(작성자·경로·검색)가 켜져 있는지.
    pub fn is_filtered(&self) -> bool {
        !self.author.trim().is_empty() || !self.path.trim().is_empty() || !self.text.trim().is_empty()
    }

    fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "log",
            "--topo-order",
            "--decorate=full",
            "-z",
            "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%D%x00%s",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if self.first_parent {
            a.push("--first-parent".into());
        }
        let author = self.author.trim();
        if !author.is_empty() {
            a.push("-i".into());
            a.push("-F".into());
            a.push(format!("--author={author}"));
        }
        match &self.branch {
            BranchFilter::All => {
                a.extend(["--branches", "--remotes", "--tags", "HEAD"].iter().map(|s| s.to_string()));
            }
            BranchFilter::Current => a.push("HEAD".into()),
            BranchFilter::Named(n) => a.push(n.clone()),
        }
        a.push("--".into());
        let path = self.path.trim();
        if !path.is_empty() {
            a.push(format!(":(top){path}"));
        }
        a
    }
}

const LOG_FIELDS: usize = 7;

/// 7개 필드로 커밋 한 건을 만든다.
fn commit_from_fields(f: &[Vec<u8>]) -> Option<LogCommit> {
    let s = |i: usize| String::from_utf8_lossy(&f[i]).into_owned();
    let sha = s(0).trim_start_matches('\n').to_string();
    if sha.len() < 7 {
        return None;
    }
    Some(LogCommit {
        sha,
        parents: s(1).split_whitespace().map(str::to_string).collect(),
        author: s(2),
        email: s(3),
        date: s(4).trim().parse().unwrap_or(0),
        refs: parse_decorations(&s(5)),
        subject: s(6),
    })
}

/// `-z` 로그 전체 출력을 파싱한다.
pub fn parse_log_z(out: &[u8]) -> Vec<LogCommit> {
    let parts: Vec<Vec<u8>> = out.split(|b| *b == 0).map(<[u8]>::to_vec).collect();
    parts.chunks(LOG_FIELDS).filter(|c| c.len() == LOG_FIELDS).filter_map(commit_from_fields).collect()
}

fn text_matches(c: &LogCommit, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    c.sha.starts_with(needle) || c.subject.to_lowercase().contains(needle)
}

/// 한 `git log` 프로세스의 출력을 페이지 단위로 넘겨주는 스트림.
///
/// 읽기 스레드는 페이지 하나를 미리 만들어 두고, 소비되기 전까지 멈춘다(파이프가 차면 git 도 멈춘다).
pub struct LogPager {
    rx: Receiver<GitResult<Vec<LogCommit>>>,
    child: Arc<parking_lot::Mutex<Option<Child>>>,
    done: bool,
}

impl LogPager {
    /// 조회를 시작한다. `page` 는 한 번에 넘길 커밋 수.
    pub fn start(root: &Path, query: &LogQuery, page: usize) -> Self {
        let (tx, rx) = sync_channel::<GitResult<Vec<LogCommit>>>(1);
        let child_slot: Arc<parking_lot::Mutex<Option<Child>>> = Arc::new(parking_lot::Mutex::new(None));
        let mut c = git_command(root, Mode::Read);
        c.args(query.args());
        let needle = query.text.trim().to_lowercase();
        let page = page.max(1);
        let slot = child_slot.clone();
        std::thread::Builder::new()
            .name("kiln-history-log".into())
            .spawn(move || {
                let mut child = match c.spawn() {
                    Ok(ch) => ch,
                    Err(e) => {
                        let _ = tx.send(Err(spawn_error(e)));
                        return;
                    }
                };
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                *slot.lock() = Some(child);
                let err_thread = std::thread::spawn(move || {
                    let mut s = String::new();
                    if let Some(mut e) = stderr {
                        let _ = e.read_to_string(&mut s);
                    }
                    s
                });
                let Some(stdout) = stdout else { return };
                let mut reader = BufReader::with_capacity(1 << 16, stdout);
                let mut fields: Vec<Vec<u8>> = Vec::with_capacity(LOG_FIELDS);
                let mut batch: Vec<LogCommit> = Vec::with_capacity(page);
                let mut buf = Vec::with_capacity(256);
                loop {
                    buf.clear();
                    match reader.read_until(0, &mut buf) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                    if buf.last() == Some(&0) {
                        buf.pop();
                    }
                    fields.push(std::mem::take(&mut buf));
                    if fields.len() == LOG_FIELDS {
                        if let Some(commit) = commit_from_fields(&fields)
                            && text_matches(&commit, &needle)
                        {
                            batch.push(commit);
                        }
                        fields.clear();
                        if batch.len() >= page && tx.send(Ok(std::mem::replace(&mut batch, Vec::with_capacity(page)))).is_err() {
                            if let Some(mut ch) = slot.lock().take() {
                                let _ = ch.kill();
                                let _ = ch.wait();
                            }
                            return;
                        }
                    }
                }
                let status = slot.lock().take().map(|mut ch| ch.wait());
                let err = err_thread.join().unwrap_or_default();
                let ok = status.is_none_or(|s| s.is_ok_and(|s| s.success()));
                if !ok && !is_empty_repo_error(&err) && batch.is_empty() {
                    let e = if err.contains("not a git repository") {
                        GitError::NotARepo
                    } else {
                        GitError::Failed(err.trim().to_string())
                    };
                    let _ = tx.send(Err(e));
                    return;
                }
                if !batch.is_empty() {
                    let _ = tx.send(Ok(batch));
                }
            })
            .expect("spawn history log thread");
        Self { rx, child: child_slot, done: false }
    }

    /// 준비된 다음 페이지를 기다리지 않고 가져온다.
    pub fn try_next(&mut self) -> Option<GitResult<Vec<LogCommit>>> {
        if self.done {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                if r.is_err() {
                    self.done = true;
                }
                Some(r)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.done = true;
                None
            }
        }
    }

    /// 다음 페이지를 기다려 가져온다. 끝이면 `None`.
    pub fn next_blocking(&mut self) -> Option<GitResult<Vec<LogCommit>>> {
        if self.done {
            return None;
        }
        match self.rx.recv() {
            Ok(r) => {
                if r.is_err() {
                    self.done = true;
                }
                Some(r)
            }
            Err(_) => {
                self.done = true;
                None
            }
        }
    }

    /// 모든 페이지를 다 넘겼는지.
    pub fn is_done(&self) -> bool {
        self.done
    }
}

impl Drop for LogPager {
    fn drop(&mut self) {
        if let Some(mut ch) = self.child.lock().take() {
            let _ = ch.kill();
            let _ = ch.wait();
        }
    }
}

fn is_empty_repo_error(err: &str) -> bool {
    err.contains("does not have any commits")
        || err.contains("bad default revision")
        || err.contains("bad revision 'HEAD'")
        || err.contains("unknown revision")
}

// ---------------------------------------------------------------- 그래프

/// 그래프 선분. 레인 번호와 색 번호를 담는다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: u16,
}

/// 한 행의 그래프.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneRow {
    /// 커밋 점이 놓인 레인.
    pub col: usize,
    /// 커밋 점 색.
    pub color: u16,
    /// 행 위쪽 레인 `from` 에서 커밋 점으로 들어오는 선(`to` = `col`).
    pub up: Vec<Edge>,
    /// 커밋 점에서 행 아래쪽 레인 `to` 로 나가는 선(`from` = `col`).
    pub down: Vec<Edge>,
    /// 커밋을 거치지 않고 위 레인 `from` 에서 아래 레인 `to` 로 지나가는 선.
    pub pass: Vec<Edge>,
    /// 이 행에서 쓰이는 레인 수.
    pub width: usize,
}

/// 페이지를 이어 붙이며 레인을 계산하는 상태.
#[derive(Clone, Debug, Default)]
pub struct GraphBuilder {
    lanes: Vec<Option<(String, u16)>>,
    next_color: u16,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn free_lane(&mut self, except: usize) -> usize {
        match self.lanes.iter().enumerate().position(|(i, l)| l.is_none() && i != except) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    fn new_color(&mut self) -> u16 {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    /// 다음 커밋(자식 → 부모 순)의 행을 계산한다.
    pub fn push(&mut self, sha: &str, parents: &[String]) -> LaneRow {
        let before: Vec<Option<u16>> = self
            .lanes
            .iter()
            .map(|l| l.as_ref().filter(|(s, _)| s != sha).map(|(_, c)| *c))
            .collect();
        let waiting: Vec<(usize, u16)> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().filter(|(s, _)| s == sha).map(|(_, c)| (i, *c)))
            .collect();
        let (col, color) = match waiting.first() {
            Some(&(i, c)) => (i, c),
            None => {
                let i = self.free_lane(usize::MAX);
                (i, self.new_color())
            }
        };
        for &(i, _) in &waiting {
            self.lanes[i] = None;
        }
        let up = waiting.iter().map(|&(i, c)| Edge { from: i, to: col, color: c }).collect();
        let mut down = Vec::new();
        if let Some(p0) = parents.first() {
            self.lanes[col] = Some((p0.clone(), color));
            down.push(Edge { from: col, to: col, color });
        }
        for p in parents.iter().skip(1) {
            if let Some(i) = self.lanes.iter().position(|l| l.as_ref().is_some_and(|(s, _)| s == p)) {
                let c = self.lanes[i].as_ref().map(|(_, c)| *c).unwrap_or(color);
                down.push(Edge { from: col, to: i, color: c });
            } else {
                let i = self.free_lane(col);
                let c = self.new_color();
                self.lanes[i] = Some((p.clone(), c));
                down.push(Edge { from: col, to: i, color: c });
            }
        }
        let pass = before
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.map(|c| Edge { from: i, to: i, color: c }))
            .collect();
        let width = before.len().max(self.lanes.len()).max(col + 1);
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        LaneRow { col, color, up, down, pass, width }
    }
}

// ---------------------------------------------------------------- 커밋 상세

/// 커밋에서 바뀐 파일 하나.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// 이름 변경·복사의 원래 경로.
    pub old_path: Option<String>,
    /// `M`/`A`/`D`/`R`/`C`/`T`.
    pub status: char,
    /// 추가 줄 수. 바이너리면 `None`.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// 커밋 상세.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub author_date: i64,
    /// 작성 시각(커밋의 시간대, `YYYY-MM-DD HH:MM`).
    pub author_date_text: String,
    pub committer: String,
    pub committer_email: String,
    pub commit_date: i64,
    pub commit_date_text: String,
    pub refs: Vec<RefLabel>,
    /// 전체 메시지(제목 포함).
    pub message: String,
    /// 변경 파일(병합 커밋은 첫 부모 기준).
    pub files: Vec<ChangedFile>,
}

impl CommitInfo {
    /// 첫 부모(루트 커밋이면 빈 트리) 리비전.
    pub fn base_rev(&self) -> String {
        self.parents.first().cloned().unwrap_or_else(|| EMPTY_TREE.to_string())
    }
}

/// 커밋 헤더와 변경 파일 목록.
pub fn commit_info(root: &Path, sha: &str) -> GitResult<CommitInfo> {
    let head = git(
        root,
        Mode::Read,
        &[
            "show",
            "-s",
            "--decorate=full",
            "--date=format:%Y-%m-%d %H:%M",
            "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ad%x00%cn%x00%ce%x00%ct%x00%cd%x00%D%x00%B",
            sha,
        ],
    )?;
    let f: Vec<&str> = head.splitn(12, '\0').collect();
    if f.len() < 12 {
        return Err(GitError::Parse("commit header".into()));
    }
    let parents: Vec<String> = f[1].split_whitespace().map(str::to_string).collect();
    let base = parents.first().cloned().unwrap_or_else(|| EMPTY_TREE.to_string());
    let status = git(root, Mode::Read, &["diff-tree", "-r", "-M", "--no-commit-id", "--name-status", "-z", &base, sha])?;
    let numstat = git(root, Mode::Read, &["diff-tree", "-r", "-M", "--no-commit-id", "--numstat", "-z", &base, sha])?;
    let mut files = parse_name_status_z(&status);
    apply_numstat_z(&mut files, &numstat);
    Ok(CommitInfo {
        sha: f[0].to_string(),
        parents,
        author: f[2].to_string(),
        email: f[3].to_string(),
        author_date: f[4].parse().unwrap_or(0),
        author_date_text: f[5].to_string(),
        committer: f[6].to_string(),
        committer_email: f[7].to_string(),
        commit_date: f[8].parse().unwrap_or(0),
        commit_date_text: f[9].to_string(),
        refs: parse_decorations(f[10]),
        message: f[11].trim_end().to_string(),
        files,
    })
}

/// `--name-status -z` 출력을 파싱한다.
pub fn parse_name_status_z(out: &str) -> Vec<ChangedFile> {
    let toks: Vec<&str> = out.split('\0').collect();
    let mut v = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let st = toks[i].trim();
        if st.is_empty() {
            i += 1;
            continue;
        }
        let ch = st.chars().next().unwrap_or('M');
        if (ch == 'R' || ch == 'C') && i + 2 < toks.len() {
            v.push(ChangedFile { path: toks[i + 2].into(), old_path: Some(toks[i + 1].into()), status: ch, added: None, removed: None });
            i += 3;
        } else if i + 1 < toks.len() {
            v.push(ChangedFile { path: toks[i + 1].into(), old_path: None, status: ch, added: None, removed: None });
            i += 2;
        } else {
            break;
        }
    }
    v
}

fn apply_numstat_z(files: &mut [ChangedFile], out: &str) {
    let toks: Vec<&str> = out.split('\0').collect();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.trim().is_empty() {
            i += 1;
            continue;
        }
        let mut it = t.splitn(3, '\t');
        let a = it.next().unwrap_or("-");
        let r = it.next().unwrap_or("-");
        let p = it.next().unwrap_or("");
        let path = if p.is_empty() {
            i += 2;
            toks.get(i).copied().unwrap_or("")
        } else {
            p
        };
        i += 1;
        if let Some(f) = files.iter_mut().find(|f| f.path == path) {
            f.added = a.parse().ok();
            f.removed = r.parse().ok();
        }
    }
}

// ---------------------------------------------------------------- 저장소 상태

/// 이력 화면이 쓰는 저장소 상태.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoState {
    /// 저장소 최상위 경로.
    pub top: PathBuf,
    /// 조회한 경로의 최상위 기준 접두사(`sub/dir/`, 최상위면 빈 문자열).
    pub prefix: String,
    pub head: Option<String>,
    /// 현재 브랜치(분리된 HEAD 면 `None`).
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub local_branches: Vec<String>,
    pub remote_branches: Vec<String>,
    /// 원격 추적 브랜치가 하나라도 있는지.
    pub has_remote_refs: bool,
    /// HEAD 에서 닿지만 어떤 원격 추적 브랜치에서도 닿지 않는 커밋.
    pub unpushed: HashSet<String>,
    pub op: Option<RepoOp>,
    /// 충돌 파일(최상위 기준 상대 경로).
    pub conflicts: Vec<String>,
    /// 추적 파일에 커밋되지 않은 변경이 있는지.
    pub dirty: bool,
    /// 참조가 바뀌었는지 비교하기 위한 값.
    pub fingerprint: u64,
}

impl RepoState {
    /// 최상위 기준 상대 경로를 조회 경로(`root`) 기준 절대 경로로 바꾼다.
    pub fn abs_path(&self, root: &Path, rel: &str) -> PathBuf {
        match rel.strip_prefix(self.prefix.as_str()) {
            Some(r) => root.join(r),
            None => self.top.join(rel),
        }
    }

    /// 커밋이 원격에 올라가 있는지(원격 추적 브랜치가 없으면 항상 false).
    pub fn is_pushed(&self, sha: &str) -> bool {
        self.has_remote_refs && !self.unpushed.contains(sha)
    }
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// 저장소 상태를 읽는다.
pub fn repo_state(root: &Path) -> GitResult<RepoState> {
    let top = PathBuf::from(git(root, Mode::Read, &["rev-parse", "--show-toplevel"])?.trim());
    let prefix = git(root, Mode::Read, &["rev-parse", "--show-prefix"])?.trim().to_string();
    let head = git(root, Mode::Read, &["rev-parse", "--verify", "-q", "HEAD"]).ok().map(|s| s.trim().to_string());
    let branch = git(root, Mode::Read, &["symbolic-ref", "-q", "--short", "HEAD"]).ok().map(|s| s.trim().to_string());
    let upstream = git(root, Mode::Read, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let refs = git(root, Mode::Read, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads", "refs/remotes", "refs/tags"])?;
    let mut local_branches = Vec::new();
    let mut remote_branches = Vec::new();
    for l in refs.lines() {
        let Some((_, name)) = l.split_once(' ') else { continue };
        if let Some(n) = name.strip_prefix("refs/heads/") {
            local_branches.push(n.to_string());
        } else if let Some(n) = name.strip_prefix("refs/remotes/")
            && !n.ends_with("/HEAD")
        {
            remote_branches.push(n.to_string());
        }
    }
    let has_remote_refs = !remote_branches.is_empty();
    let mut unpushed = HashSet::new();
    if has_remote_refs && head.is_some() {
        let out = git(root, Mode::Read, &["rev-list", "--max-count=5000", "HEAD", "--not", "--remotes"])?;
        unpushed.extend(out.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    }
    let op = in_progress_op(root);
    let conflicts = conflicted_files(root).unwrap_or_default();
    let dirty = !git(root, Mode::Read, &["status", "--porcelain", "-uno", "--ignore-submodules=dirty"])?.trim().is_empty();
    let mut fp_src = refs.into_bytes();
    fp_src.extend(head.as_deref().unwrap_or("").bytes());
    fp_src.extend(branch.as_deref().unwrap_or("").bytes());
    Ok(RepoState {
        top,
        prefix,
        head,
        branch,
        upstream,
        local_branches,
        remote_branches,
        has_remote_refs,
        unpushed,
        op,
        conflicts,
        dirty,
        fingerprint: fnv(&fp_src),
    })
}

/// 충돌 중인 파일(최상위 기준 상대 경로).
pub fn conflicted_files(root: &Path) -> GitResult<Vec<String>> {
    let out = git(root, Mode::Read, &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    let mut v: Vec<String> = out.split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
    v.dedup();
    Ok(v)
}

/// 커밋이 원격에 올라가 있는지. upstream 이 있으면 upstream, 없으면 모든 원격 추적 브랜치 기준.
pub fn is_pushed(root: &Path, sha: &str) -> GitResult<bool> {
    let has_upstream = git(root, Mode::Read, &["rev-parse", "--verify", "-q", "@{u}"]).is_ok();
    if has_upstream {
        return Ok(git(root, Mode::Read, &["merge-base", "--is-ancestor", sha, "@{u}"]).is_ok());
    }
    let out = git(root, Mode::Read, &["for-each-ref", "--count=1", "--contains", sha, "--format=%(refname)", "refs/remotes"])?;
    Ok(!out.trim().is_empty())
}

fn head_sha(root: &Path) -> GitResult<String> {
    Ok(git(root, Mode::Read, &["rev-parse", "HEAD"])?.trim().to_string())
}

fn resolve(root: &Path, rev: &str) -> GitResult<String> {
    Ok(git(root, Mode::Read, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?.trim().to_string())
}

// ---------------------------------------------------------------- 명령 실행

fn git_command(root: &Path, mode: Mode) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .args(["-c", "core.quotePath=false", "-c", "color.ui=false", "-c", "log.showSignature=false"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if mode == Mode::Read {
        c.env("GIT_OPTIONAL_LOCKS", "0");
    }
    hide_console(&mut c);
    c
}

#[cfg(windows)]
fn hide_console(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_c: &mut Command) {}

fn spawn_error(e: std::io::Error) -> GitError {
    if e.kind() == std::io::ErrorKind::NotFound { GitError::GitMissing } else { GitError::Failed(e.to_string()) }
}

/// 편집기를 쓰지 않도록 환경을 맞춰 git 을 실행하고 (성공 여부, stdout+stderr) 를 돌려준다.
fn run_noninteractive(root: &Path, args: &[&str], envs: &[(&str, String)]) -> GitResult<(bool, String)> {
    let mut c = git_command(root, Mode::Write);
    c.env("GIT_EDITOR", "true");
    for (k, v) in envs {
        c.env(k, v);
    }
    c.args(args);
    let out = c.output().map_err(spawn_error)?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&err);
    }
    if err.contains("not a git repository") {
        return Err(GitError::NotARepo);
    }
    Ok((out.status.success(), s.trim_end().to_string()))
}

// ---------------------------------------------------------------- 작업 결과

/// 작업이 멈춘 이유.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stopped {
    pub op: RepoOp,
    /// 충돌 파일. 비어 있으면 `edit` 로 멈춘 것.
    pub conflicts: Vec<String>,
    /// git 출력.
    pub output: String,
}

/// 저장소를 바꾸는 작업의 결과.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Stopped(Stopped),
}

/// HEAD 를 움직이는 작업의 결과. `old_head` 로 `undo` 할 수 있다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    pub old_head: String,
    pub new_head: String,
    pub outcome: Outcome,
}

fn finish(root: &Path, old_head: String, ok: bool, output: String) -> GitResult<Rewrite> {
    if let Some(op) = in_progress_op(root) {
        let conflicts = conflicted_files(root).unwrap_or_default();
        let new_head = head_sha(root).unwrap_or_default();
        return Ok(Rewrite { old_head, new_head, outcome: Outcome::Stopped(Stopped { op, conflicts, output }) });
    }
    if !ok {
        return Err(GitError::Failed(output));
    }
    let new_head = head_sha(root)?;
    Ok(Rewrite { old_head, new_head, outcome: Outcome::Done })
}

fn ensure_idle(root: &Path) -> GitResult<()> {
    match in_progress_op(root) {
        Some(op) => Err(GitError::Failed(kiln_common::trf!("{} 작업이 진행 중입니다. 먼저 계속하거나 중단하세요.", op.label()))),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- 대화형 리베이스

/// 리베이스 todo 동작.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RebaseAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
    Edit,
}

impl RebaseAction {
    pub const ALL: [RebaseAction; 6] =
        [RebaseAction::Pick, RebaseAction::Reword, RebaseAction::Edit, RebaseAction::Squash, RebaseAction::Fixup, RebaseAction::Drop];

    /// todo 명령어.
    pub fn keyword(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
            RebaseAction::Edit => "edit",
        }
    }

    /// 화면 표시 이름.
    pub fn label(self) -> &'static str {
        match self {
            RebaseAction::Pick => kiln_common::i18n::tr("유지"),
            RebaseAction::Reword => kiln_common::i18n::tr("메시지 수정"),
            RebaseAction::Squash => kiln_common::i18n::tr("스쿼시"),
            RebaseAction::Fixup => kiln_common::i18n::tr("픽스업"),
            RebaseAction::Drop => kiln_common::i18n::tr("삭제"),
            RebaseAction::Edit => kiln_common::i18n::tr("멈추고 편집"),
        }
    }

    /// 앞 커밋에 합쳐지는 동작인지.
    pub fn melds(self) -> bool {
        matches!(self, RebaseAction::Squash | RebaseAction::Fixup)
    }
}

/// todo 한 줄.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebaseStep {
    pub action: RebaseAction,
    pub sha: String,
    pub subject: String,
    /// 전체 메시지. `Reword`/`Squash` 는 이 값이 그룹의 최종 메시지가 된다.
    pub message: String,
    pub author: String,
    pub date: i64,
}

/// 리베이스 계획. `steps` 는 오래된 커밋부터.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebasePlan {
    /// 새 이력이 올라갈 커밋. `None` 이면 루트부터(`--root`).
    pub base: Option<String>,
    pub steps: Vec<RebaseStep>,
}

impl RebasePlan {
    fn position(&self, sha: &str) -> GitResult<usize> {
        self.steps
            .iter()
            .position(|s| s.sha == sha || s.sha.starts_with(sha))
            .ok_or_else(|| GitError::Failed(kiln_common::trf!("{}은(는) 현재 브랜치의 재작성 구간에 없습니다", short(sha))))
    }

    /// 결과 이력 미리보기: (대표 커밋 인덱스, 합쳐지는 커밋 인덱스들, 최종 제목). 오래된 순.
    pub fn preview(&self) -> Vec<(usize, Vec<usize>, String)> {
        let mut out: Vec<(usize, Vec<usize>, String)> = Vec::new();
        for (i, s) in self.steps.iter().enumerate() {
            match s.action {
                RebaseAction::Drop => {}
                a if a.melds() => {
                    if let Some(last) = out.last_mut() {
                        last.1.push(i);
                        if a == RebaseAction::Squash {
                            last.2 = first_line(&s.message).to_string();
                        }
                    } else {
                        out.push((i, Vec::new(), s.subject.clone()));
                    }
                }
                RebaseAction::Reword => out.push((i, Vec::new(), first_line(&s.message).to_string())),
                _ => out.push((i, Vec::new(), s.subject.clone())),
            }
        }
        out
    }

    /// 실행할 수 없는 계획이면 이유를 돌려준다.
    pub fn validate(&self) -> Result<(), String> {
        let first_kept = self.steps.iter().find(|s| s.action != RebaseAction::Drop);
        if first_kept.is_some_and(|s| s.action.melds()) {
            return Err(kiln_common::i18n::tr("첫 커밋은 스쿼시·픽스업할 수 없습니다. 합칠 앞 커밋이 없습니다.").into());
        }
        for s in &self.steps {
            if matches!(s.action, RebaseAction::Reword | RebaseAction::Squash) && s.message.trim().is_empty() {
                return Err(kiln_common::trf!("{}의 커밋 메시지가 비어 있습니다", short(&s.sha)));
            }
        }
        Ok(())
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("").trim()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// `oldest` 부터 HEAD 까지(오래된 순) 모두 `pick` 인 계획을 만든다.
/// `oldest` 가 HEAD 조상이 아니거나, 구간에 병합 커밋이 있으면 오류.
pub fn rebase_plan(root: &Path, oldest: &str) -> GitResult<RebasePlan> {
    let oldest = resolve(root, oldest)?;
    if git(root, Mode::Read, &["merge-base", "--is-ancestor", &oldest, "HEAD"]).is_err() {
        return Err(GitError::Failed(kiln_common::trf!("{}은(는) 현재 브랜치에 없는 커밋입니다", short(&oldest))));
    }
    let parents: Vec<String> =
        git(root, Mode::Read, &["rev-list", "--parents", "-n1", &oldest])?.split_whitespace().skip(1).map(str::to_string).collect();
    if parents.len() > 1 {
        return Err(GitError::Failed(kiln_common::i18n::tr("병합 커밋은 다시 쓸 수 없습니다").into()));
    }
    let base = parents.into_iter().next();
    let range = match &base {
        Some(b) => format!("{b}..HEAD"),
        None => "HEAD".to_string(),
    };
    let out = git_bytes_read(root, &["log", "--reverse", "-z", "--format=%H%x00%P%x00%an%x00%at%x00%B", &range])?;
    let toks: Vec<String> = out.split(|b| *b == 0).map(|b| String::from_utf8_lossy(b).into_owned()).collect();
    let mut steps = Vec::new();
    for c in toks.chunks(5).filter(|c| c.len() == 5) {
        let sha = c[0].trim_start_matches('\n').to_string();
        if sha.is_empty() {
            continue;
        }
        if c[1].split_whitespace().count() > 1 {
            return Err(GitError::Failed(kiln_common::trf!(
                "{}부터 HEAD 사이에 병합 커밋({})이 있어 이력을 다시 쓸 수 없습니다",
                short(&oldest),
                short(&sha)
            )));
        }
        let message = c[4].trim_end().to_string();
        steps.push(RebaseStep {
            action: RebaseAction::Pick,
            subject: first_line(&message).to_string(),
            message,
            sha,
            author: c[2].clone(),
            date: c[3].parse().unwrap_or(0),
        });
    }
    if steps.first().is_none_or(|s| s.sha != oldest) {
        return Err(GitError::Failed(kiln_common::trf!("{}부터의 구간을 계산하지 못했습니다", short(&oldest))));
    }
    Ok(RebasePlan { base, steps })
}

fn git_bytes_read(root: &Path, args: &[&str]) -> GitResult<Vec<u8>> {
    crate::cmd::git_bytes(root, Mode::Read, args)
}

/// sh 의 작은따옴표 인용.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// git 이 sh 로 넘겨받을 경로(Windows 역슬래시를 슬래시로).
fn sh_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 계획을 todo 문서로 만든다. 메시지 파일은 `msg_dir` 에 쓴다.
fn render_todo(plan: &RebasePlan, msg_dir: &Path) -> GitResult<String> {
    let mut todo = String::new();
    let mut n = 0;
    let steps = &plan.steps;
    let mut i = 0;
    while i < steps.len() {
        let s = &steps[i];
        if s.action == RebaseAction::Drop {
            todo.push_str(&format!("drop {} {}\n", s.sha, s.subject));
            i += 1;
            continue;
        }
        let leader = s;
        let mut group_msg: Option<String> = (leader.action == RebaseAction::Reword).then(|| leader.message.clone());
        let leader_kw = if leader.action == RebaseAction::Edit { "edit" } else { "pick" };
        todo.push_str(&format!("{leader_kw} {} {}\n", leader.sha, leader.subject));
        let mut j = i + 1;
        let mut after_drops = String::new();
        while j < steps.len() && (steps[j].action.melds() || steps[j].action == RebaseAction::Drop) {
            let m = &steps[j];
            if m.action == RebaseAction::Drop {
                after_drops.push_str(&format!("drop {} {}\n", m.sha, m.subject));
            } else {
                todo.push_str(&format!("fixup {} {}\n", m.sha, m.subject));
                if m.action == RebaseAction::Squash {
                    group_msg = Some(m.message.clone());
                }
            }
            j += 1;
        }
        if let Some(msg) = group_msg {
            n += 1;
            let f = msg_dir.join(format!("msg-{n}.txt"));
            std::fs::write(&f, format!("{}\n", msg.trim_end())).map_err(|e| GitError::Failed(e.to_string()))?;
            todo.push_str(&format!(
                "exec git commit --amend --allow-empty --no-verify --cleanup=whitespace -q -F {}\n",
                sh_quote(&sh_path(&f))
            ));
        }
        todo.push_str(&after_drops);
        i = j;
    }
    Ok(todo)
}

fn work_dir(root: &Path) -> GitResult<PathBuf> {
    let p = git(root, Mode::Read, &["rev-parse", "--path-format=absolute", "--git-path", "kiln-history"])?;
    let dir = PathBuf::from(p.trim());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| GitError::Failed(e.to_string()))?;
    Ok(dir)
}

/// 계획대로 대화형 리베이스를 실행한다. 충돌이나 `edit` 에서 멈추면 `Outcome::Stopped`.
pub fn run_rebase(root: &Path, plan: &RebasePlan, autostash: bool) -> GitResult<Rewrite> {
    plan.validate().map_err(GitError::Failed)?;
    ensure_idle(root)?;
    let old_head = head_sha(root)?;
    let dir = work_dir(root)?;
    let todo = render_todo(plan, &dir)?;
    let todo_path = dir.join("git-rebase-todo");
    std::fs::write(&todo_path, &todo).map_err(|e| GitError::Failed(e.to_string()))?;
    let seq_editor = format!("cp {}", sh_quote(&sh_path(&todo_path)));
    let mut args: Vec<&str> = vec![
        "-c",
        "rebase.updateRefs=false",
        "-c",
        "rebase.autoSquash=false",
        "-c",
        "rebase.missingCommitsCheck=ignore",
        "-c",
        "rebase.abbreviateCommands=false",
        "rebase",
        "-i",
        "--no-rebase-merges",
    ];
    args.push(if autostash { "--autostash" } else { "--no-autostash" });
    let base = plan.base.clone();
    match &base {
        Some(b) => args.push(b),
        None => args.push("--root"),
    }
    let (ok, out) = run_noninteractive(root, &args, &[("GIT_SEQUENCE_EDITOR", seq_editor)])?;
    finish(root, old_head, ok, out)
}

/// 선택한 커밋들(연속 구간)을 가장 오래된 커밋 하나로 합친다.
pub fn squash_commits(root: &Path, shas: &[String], message: &str, autostash: bool) -> GitResult<Rewrite> {
    let mut plan = plan_for(root, shas)?;
    let mut idx: Vec<usize> = shas.iter().map(|s| plan.position(s)).collect::<GitResult<_>>()?;
    idx.sort_unstable();
    idx.dedup();
    if idx.len() < 2 {
        return Err(GitError::Failed(kiln_common::i18n::tr("합칠 커밋을 두 개 이상 선택하세요").into()));
    }
    if idx.windows(2).any(|w| w[1] != w[0] + 1) {
        return Err(GitError::Failed(kiln_common::i18n::tr("연속된 커밋만 하나로 합칠 수 있습니다").into()));
    }
    plan.steps[idx[0]].action = RebaseAction::Reword;
    plan.steps[idx[0]].message = message.to_string();
    for &i in &idx[1..] {
        plan.steps[i].action = RebaseAction::Fixup;
    }
    run_rebase(root, &plan, autostash)
}

/// 커밋들을 이력에서 지운다.
pub fn drop_commits(root: &Path, shas: &[String], autostash: bool) -> GitResult<Rewrite> {
    let mut plan = plan_for(root, shas)?;
    for s in shas {
        let i = plan.position(s)?;
        plan.steps[i].action = RebaseAction::Drop;
    }
    run_rebase(root, &plan, autostash)
}

/// 커밋 메시지를 바꾼다. HEAD 면 `commit --amend --only`, 아니면 리베이스.
pub fn reword_commit(root: &Path, sha: &str, message: &str, autostash: bool) -> GitResult<Rewrite> {
    if message.trim().is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("커밋 메시지가 비어 있습니다").into()));
    }
    ensure_idle(root)?;
    let full = resolve(root, sha)?;
    let old_head = head_sha(root)?;
    if full == old_head {
        git_stdin(
            root,
            &["commit", "--amend", "--only", "--allow-empty", "--no-verify", "--cleanup=whitespace", "-q", "-F", "-"],
            format!("{}\n", message.trim_end()).as_bytes(),
        )?;
        let new_head = head_sha(root)?;
        return Ok(Rewrite { old_head, new_head, outcome: Outcome::Done });
    }
    let mut plan = rebase_plan(root, &full)?;
    let i = plan.position(&full)?;
    plan.steps[i].action = RebaseAction::Reword;
    plan.steps[i].message = message.to_string();
    run_rebase(root, &plan, autostash)
}

/// 로그에서 끌어다 놓을 위치.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPlace {
    /// 대상보다 새 커밋이 되도록(로그에서 대상 위).
    Above,
    /// 대상보다 오래된 커밋이 되도록(로그에서 대상 아래).
    Below,
}

fn plan_for(root: &Path, shas: &[String]) -> GitResult<RebasePlan> {
    if shas.is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("선택한 커밋이 없습니다").into()));
    }
    ensure_idle(root)?;
    // 가장 오래된 커밋은 HEAD 에서 가장 먼 커밋이다.
    let head = head_sha(root)?;
    let mut best: Option<(usize, String)> = None;
    for s in shas {
        let full = resolve(root, s)?;
        let n: usize = git(root, Mode::Read, &["rev-list", "--count", &format!("{full}..{head}")])?.trim().parse().unwrap_or(0);
        if best.as_ref().is_none_or(|(bn, _)| n > *bn) {
            best = Some((n, full));
        }
    }
    let (_, oldest) = best.expect("non-empty");
    rebase_plan(root, &oldest)
}

fn reorder(plan: &mut RebasePlan, moving: &[String], target: &str, place: DropPlace, fixup: bool) -> GitResult<()> {
    let mut idx: Vec<usize> = moving.iter().map(|s| plan.position(s)).collect::<GitResult<_>>()?;
    idx.sort_unstable();
    idx.dedup();
    let target_sha = plan.steps[plan.position(target)?].sha.clone();
    if idx.iter().any(|&i| plan.steps[i].sha == target_sha) {
        return Err(GitError::Failed(kiln_common::i18n::tr("선택한 커밋 위로는 옮길 수 없습니다").into()));
    }
    let mut moved: Vec<RebaseStep> = Vec::new();
    for &i in idx.iter().rev() {
        moved.insert(0, plan.steps.remove(i));
    }
    if fixup {
        for m in &mut moved {
            m.action = RebaseAction::Fixup;
        }
    }
    let t = plan.position(&target_sha)?;
    let at = match (fixup, place) {
        (true, _) | (false, DropPlace::Above) => t + 1,
        (false, DropPlace::Below) => t,
    };
    for (k, m) in moved.into_iter().enumerate() {
        plan.steps.insert(at + k, m);
    }
    Ok(())
}

/// 커밋들을 `target` 의 위나 아래로 옮긴다(순서 유지).
pub fn move_commits(root: &Path, moving: &[String], target: &str, place: DropPlace, autostash: bool) -> GitResult<Rewrite> {
    let mut all: Vec<String> = moving.to_vec();
    all.push(target.to_string());
    let mut plan = plan_for(root, &all)?;
    let before: Vec<String> = plan.steps.iter().map(|s| s.sha.clone()).collect();
    reorder(&mut plan, moving, target, place, false)?;
    if plan.steps.iter().map(|s| &s.sha).eq(before.iter()) {
        return Err(GitError::Failed(kiln_common::i18n::tr("순서가 바뀌지 않습니다").into()));
    }
    run_rebase(root, &plan, autostash)
}

/// 커밋들을 `target` 에 픽스업으로 합친다(대상의 메시지를 유지).
pub fn fixup_into(root: &Path, moving: &[String], target: &str, autostash: bool) -> GitResult<Rewrite> {
    let mut all: Vec<String> = moving.to_vec();
    all.push(target.to_string());
    let mut plan = plan_for(root, &all)?;
    reorder(&mut plan, moving, target, DropPlace::Above, true)?;
    run_rebase(root, &plan, autostash)
}

/// 여러 커밋을 합칠 때 쓸 기본 메시지(오래된 순으로 이어 붙임).
pub fn combined_message(root: &Path, shas_oldest_first: &[String]) -> GitResult<String> {
    let mut parts = Vec::new();
    for s in shas_oldest_first {
        parts.push(git(root, Mode::Read, &["log", "-1", "--format=%B", s])?.trim_end().to_string());
    }
    Ok(parts.join("\n\n"))
}

/// 커밋 전체 메시지.
pub fn commit_message(root: &Path, sha: &str) -> GitResult<String> {
    Ok(git(root, Mode::Read, &["log", "-1", "--format=%B", sha])?.trim_end().to_string())
}

// ---------------------------------------------------------------- 단순 작업

/// 커밋들을 순서대로(오래된 것부터) 현재 브랜치에 체리픽한다.
pub fn cherry_pick(root: &Path, shas_oldest_first: &[String]) -> GitResult<Rewrite> {
    ensure_idle(root)?;
    let old_head = head_sha(root)?;
    let mut args: Vec<&str> = vec!["cherry-pick"];
    let merges = shas_oldest_first
        .iter()
        .any(|s| git(root, Mode::Read, &["rev-list", "--parents", "-n1", s]).is_ok_and(|o| o.split_whitespace().count() > 2));
    if merges {
        args.extend(["-m", "1"]);
    }
    args.extend(shas_oldest_first.iter().map(String::as_str));
    let (ok, out) = run_noninteractive(root, &args, &[])?;
    finish(root, old_head, ok, out)
}

/// 커밋을 되돌리는 새 커밋을 만든다.
pub fn revert(root: &Path, sha: &str) -> GitResult<Rewrite> {
    ensure_idle(root)?;
    let old_head = head_sha(root)?;
    let merge = git(root, Mode::Read, &["rev-list", "--parents", "-n1", sha])?.split_whitespace().count() > 2;
    let mut args = vec!["revert", "--no-edit"];
    if merge {
        args.extend(["-m", "1"]);
    }
    args.push(sha);
    let (ok, out) = run_noninteractive(root, &args, &[])?;
    finish(root, old_head, ok, out)
}

/// 리셋 방식.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// 현재 브랜치를 `sha` 로 리셋한다.
pub fn reset(root: &Path, sha: &str, mode: ResetMode) -> GitResult<Rewrite> {
    let old_head = head_sha(root)?;
    let (ok, out) = run_noninteractive(root, &["reset", "-q", mode.flag(), sha], &[])?;
    if !ok {
        return Err(GitError::Failed(out));
    }
    Ok(Rewrite { old_head, new_head: head_sha(root)?, outcome: Outcome::Done })
}

/// 재작성 전 HEAD 로 되돌린다. 진행 중인 작업은 먼저 중단한다. 작업 트리 변경은 `--keep` 으로 보존한다.
pub fn undo(root: &Path, old_head: &str) -> GitResult<String> {
    if let Some(op) = in_progress_op(root) {
        git_combined(root, op.abort_args())?;
    }
    git_combined(root, &["reset", "--keep", old_head])
}

/// 리비전을 체크아웃한다. 로컬 브랜치 이름이면 전환하고, 아니면 분리된 HEAD 로.
pub fn checkout(root: &Path, rev: &str) -> GitResult<String> {
    let is_branch = git(root, Mode::Read, &["rev-parse", "--verify", "-q", &format!("refs/heads/{rev}")]).is_ok();
    if is_branch {
        git_combined(root, &["switch", rev])
    } else {
        git_combined(root, &["switch", "--detach", rev])
    }
}

/// `sha` 에서 브랜치를 만든다. `switch` 면 바로 전환한다.
pub fn create_branch_at(root: &Path, name: &str, sha: &str, switch: bool) -> GitResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("브랜치 이름이 비어 있습니다").into()));
    }
    git(root, Mode::Read, &["check-ref-format", "--branch", name])
        .map_err(|_| GitError::Failed(kiln_common::trf!("'{name}'은(는) 올바른 브랜치 이름이 아닙니다")))?;
    if switch {
        git_combined(root, &["switch", "-c", name, sha])
    } else {
        git_combined(root, &["branch", name, sha])
    }
}

/// `sha` 에 태그를 만든다. 메시지가 있으면 주석 태그.
pub fn create_tag(root: &Path, name: &str, sha: &str, message: &str) -> GitResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("태그 이름이 비어 있습니다").into()));
    }
    git(root, Mode::Read, &["check-ref-format", &format!("refs/tags/{name}")])
        .map_err(|_| GitError::Failed(kiln_common::trf!("'{name}'은(는) 올바른 태그 이름이 아닙니다")))?;
    if message.trim().is_empty() {
        git_combined(root, &["tag", name, sha])
    } else {
        git_stdin(root, &["tag", "-a", name, "-F", "-", sha], message.as_bytes())
    }
}

/// 진행 중인 작업을 계속한다(메시지 편집기 없이).
pub fn continue_op(root: &Path) -> GitResult<Rewrite> {
    let Some(op) = in_progress_op(root) else {
        return Err(GitError::Failed(kiln_common::i18n::tr("진행 중인 작업이 없습니다").into()));
    };
    let old_head = head_sha(root).unwrap_or_default();
    let conflicts = conflicted_files(root)?;
    if !conflicts.is_empty() {
        return Err(GitError::Failed(kiln_common::trf!("충돌이 해결되지 않은 파일이 {}개 있습니다: {}", conflicts.len(), conflicts.join(", "))));
    }
    let args: &[&str] = match op {
        RepoOp::Rebase => &["rebase", "--continue"],
        RepoOp::CherryPick => &["cherry-pick", "--continue"],
        RepoOp::Revert => &["revert", "--continue"],
        RepoOp::Merge => &["commit", "--no-edit"],
    };
    let (ok, out) = run_noninteractive(root, args, &[])?;
    finish(root, old_head, ok, out)
}

/// 현재 커밋을 건너뛰고 계속한다.
pub fn skip_op(root: &Path) -> GitResult<Rewrite> {
    let Some(op) = in_progress_op(root) else {
        return Err(GitError::Failed(kiln_common::i18n::tr("진행 중인 작업이 없습니다").into()));
    };
    let old_head = head_sha(root).unwrap_or_default();
    let args: &[&str] = match op {
        RepoOp::Rebase => &["rebase", "--skip"],
        RepoOp::CherryPick => &["cherry-pick", "--skip"],
        RepoOp::Revert => &["revert", "--skip"],
        RepoOp::Merge => return Err(GitError::Failed(kiln_common::i18n::tr("병합은 건너뛸 수 없습니다").into())),
    };
    let (ok, out) = run_noninteractive(root, args, &[])?;
    finish(root, old_head, ok, out)
}

/// 진행 중인 작업을 중단한다.
pub fn abort_op(root: &Path) -> GitResult<String> {
    match in_progress_op(root) {
        Some(op) => git_combined(root, op.abort_args()),
        None => Ok(String::new()),
    }
}

/// 현재 브랜치를 `--force-with-lease` 로 푸시한다.
pub fn force_push(root: &Path) -> GitResult<String> {
    git_combined(root, &["push", "--force-with-lease"])
}
