//! 저장소 조회/조작 함수. 모두 블로킹이며 백그라운드 스레드에서 호출한다.

use std::path::{Path, PathBuf};

use crate::cmd::{GitError, GitResult, Mode, git, git_bytes, git_combined, git_raw, git_stdin};
use crate::diff::{FileDiff, parse_diff, synth_added_file};
use crate::status::{Status, parse_status_v2};

/// `git status` 를 실행해 파싱한다.
pub fn status(root: &Path) -> GitResult<Status> {
    let raw = git_bytes(
        root,
        Mode::Read,
        &["status", "--porcelain=v2", "--branch", "--show-stash", "-z", "--ignore-submodules=dirty"],
    )?;
    Ok(parse_status_v2(&raw))
}

/// 작업 디렉터리의 저장소 최상위 경로.
pub fn toplevel(root: &Path) -> GitResult<PathBuf> {
    let s = git(root, Mode::Read, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(s.trim()))
}

/// 진행 중인 저장소 작업.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoOp {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl RepoOp {
    pub fn label(self) -> &'static str {
        match self {
            RepoOp::Merge => kiln_common::i18n::tr("병합"),
            RepoOp::Rebase => kiln_common::i18n::tr("리베이스"),
            RepoOp::CherryPick => kiln_common::i18n::tr("체리픽"),
            RepoOp::Revert => kiln_common::i18n::tr("되돌리기"),
        }
    }

    /// 중단 명령 인자.
    pub fn abort_args(self) -> &'static [&'static str] {
        match self {
            RepoOp::Merge => &["merge", "--abort"],
            RepoOp::Rebase => &["rebase", "--abort"],
            RepoOp::CherryPick => &["cherry-pick", "--abort"],
            RepoOp::Revert => &["revert", "--abort"],
        }
    }
}

/// `.git` 디렉터리의 표식 파일로 진행 중인 작업을 알아낸다.
pub fn in_progress_op(root: &Path) -> Option<RepoOp> {
    let out = git(
        root,
        Mode::Read,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "MERGE_HEAD",
            "--git-path",
            "rebase-merge",
            "--git-path",
            "rebase-apply",
            "--git-path",
            "CHERRY_PICK_HEAD",
            "--git-path",
            "REVERT_HEAD",
        ],
    )
    .ok()?;
    let p: Vec<PathBuf> = out.lines().map(PathBuf::from).collect();
    if p.len() < 5 {
        return None;
    }
    if p[0].exists() {
        Some(RepoOp::Merge)
    } else if p[1].exists() || p[2].exists() {
        Some(RepoOp::Rebase)
    } else if p[3].exists() {
        Some(RepoOp::CherryPick)
    } else if p[4].exists() {
        Some(RepoOp::Revert)
    } else {
        None
    }
}

pub fn abort_op(root: &Path, op: RepoOp) -> GitResult<String> {
    git_combined(root, op.abort_args())
}

fn with_paths<'a>(mut base: Vec<&'a str>, paths: &'a [String]) -> Vec<&'a str> {
    base.push("--");
    base.extend(paths.iter().map(String::as_str));
    base
}

/// 파일을 스테이지한다(삭제 포함).
pub fn stage(root: &Path, paths: &[String]) -> GitResult<()> {
    git(root, Mode::Write, &with_paths(vec!["add", "-A"], paths)).map(|_| ())
}

pub fn stage_all(root: &Path) -> GitResult<()> {
    git(root, Mode::Write, &["add", "-A"]).map(|_| ())
}

fn has_head(root: &Path) -> bool {
    git(root, Mode::Read, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
}

/// 스테이지를 해제한다. HEAD 가 없으면 인덱스에서 제거한다.
pub fn unstage(root: &Path, paths: &[String]) -> GitResult<()> {
    if has_head(root) {
        git(root, Mode::Write, &with_paths(vec!["restore", "--staged"], paths)).map(|_| ())
    } else {
        git(root, Mode::Write, &with_paths(vec!["rm", "--cached", "-r", "-q"], paths)).map(|_| ())
    }
}

pub fn unstage_all(root: &Path) -> GitResult<()> {
    if has_head(root) {
        git(root, Mode::Write, &["reset", "-q"]).map(|_| ())
    } else {
        git(root, Mode::Write, &["rm", "--cached", "-r", "-q", "."]).map(|_| ())
    }
}

/// 작업트리 변경을 버린다(인덱스 상태로 되돌림).
pub fn discard(root: &Path, paths: &[String]) -> GitResult<()> {
    git(root, Mode::Write, &with_paths(vec!["restore", "--worktree"], paths)).map(|_| ())
}

/// 미추적 파일/디렉터리를 지운다.
pub fn clean_untracked(root: &Path, paths: &[String]) -> GitResult<()> {
    git(root, Mode::Write, &with_paths(vec!["clean", "-f", "-d", "-q"], paths)).map(|_| ())
}

/// 충돌 파일을 한쪽 버전으로 해결하고 스테이지한다.
pub fn resolve_conflict(root: &Path, path: &str, ours: bool) -> GitResult<()> {
    let side = if ours { "--ours" } else { "--theirs" };
    git(root, Mode::Write, &["checkout", side, "--", path])?;
    git(root, Mode::Write, &["add", "--", path]).map(|_| ())
}

/// 커밋한다. `amend` 이고 메시지가 비면 기존 메시지를 유지한다.
pub fn commit(root: &Path, message: &str, amend: bool) -> GitResult<String> {
    if amend && message.trim().is_empty() {
        return git_combined(root, &["commit", "--amend", "--no-edit"]);
    }
    if message.trim().is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("커밋 메시지가 비어 있습니다").into()));
    }
    let mut args = vec!["commit", "--cleanup=strip", "-F", "-"];
    if amend {
        args.push("--amend");
    }
    git_stdin(root, &args, message.as_bytes()).map(|s| s.trim().to_string())
}

/// 마지막 커밋의 전체 메시지.
pub fn last_commit_message(root: &Path) -> GitResult<String> {
    git(root, Mode::Read, &["log", "-1", "--format=%B"]).map(|s| s.trim_end().to_string())
}

// ---------------------------------------------------------------- 브랜치

/// 브랜치 한 건.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// 짧은 이름(`main`, `origin/main`).
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    /// `[ahead 1, behind 2]` 형식의 추적 상태.
    pub track: String,
    pub sha: String,
    pub subject: String,
    pub date: i64,
}

/// 로컬+원격 브랜치 목록(최근 커밋 순).
pub fn branches(root: &Path) -> GitResult<Vec<Branch>> {
    let out = git(
        root,
        Mode::Read,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%00%(refname:short)%00%(objectname:short)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:unix)%00%(HEAD)%00%(contents:subject)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse_branches(&out))
}

pub fn parse_branches(out: &str) -> Vec<Branch> {
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 8 {
            continue;
        }
        let remote = f[0].starts_with("refs/remotes/");
        if remote && f[0].ends_with("/HEAD") {
            continue;
        }
        v.push(Branch {
            name: f[1].to_string(),
            remote,
            current: f[6] == "*",
            upstream: (!f[3].is_empty()).then(|| f[3].to_string()),
            track: f[4].to_string(),
            sha: f[2].to_string(),
            date: f[5].parse().unwrap_or(0),
            subject: f[7].to_string(),
        });
    }
    v
}

/// 브랜치로 전환한다. 원격 브랜치면 같은 이름의 로컬 추적 브랜치를 만들거나 전환한다.
pub fn checkout(root: &Path, branch: &Branch) -> GitResult<String> {
    if branch.remote {
        let local = branch.name.split_once('/').map(|(_, b)| b).unwrap_or(&branch.name);
        let exists = git(root, Mode::Read, &["rev-parse", "--verify", "-q", &format!("refs/heads/{local}")]).is_ok();
        if exists {
            return git_combined(root, &["switch", local]);
        }
        return git_combined(root, &["switch", "--track", &branch.name]);
    }
    git_combined(root, &["switch", &branch.name])
}

pub fn checkout_name(root: &Path, name: &str) -> GitResult<String> {
    git_combined(root, &["switch", name])
}

/// 현재 HEAD 에서 새 브랜치를 만들고 전환한다.
pub fn create_branch(root: &Path, name: &str) -> GitResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GitError::Failed(kiln_common::i18n::tr("브랜치 이름이 비어 있습니다").into()));
    }
    git(root, Mode::Read, &["check-ref-format", "--branch", name])
        .map_err(|_| GitError::Failed(kiln_common::trf!("'{name}'은(는) 올바른 브랜치 이름이 아닙니다")))?;
    git_combined(root, &["switch", "-c", name])
}

/// 로컬 브랜치를 삭제한다.
pub fn delete_branch(root: &Path, name: &str, force: bool) -> GitResult<String> {
    git_combined(root, &["branch", if force { "-D" } else { "-d" }, name])
}

// ---------------------------------------------------------------- 원격 동기화

pub fn fetch(root: &Path) -> GitResult<String> {
    git_combined(root, &["fetch", "--all", "--prune"])
}

pub fn pull(root: &Path) -> GitResult<String> {
    git_combined(root, &["pull", "--no-edit"])
}

/// 푸시한다. upstream 이 없으면 첫 원격으로 `-u` 푸시한다.
pub fn push(root: &Path) -> GitResult<String> {
    let st = status(root)?;
    let Some(head) = st.branch.head.clone() else {
        return Err(GitError::Failed(kiln_common::i18n::tr("분리된 HEAD는 Push할 수 없습니다").into()));
    };
    if st.branch.upstream.is_some() {
        return git_combined(root, &["push"]);
    }
    let remotes = git(root, Mode::Read, &["remote"])?;
    let remote = remotes
        .lines()
        .find(|r| *r == "origin")
        .or_else(|| remotes.lines().next())
        .ok_or_else(|| GitError::Failed(kiln_common::i18n::tr("설정된 원격이 없습니다. `git remote add origin <url>`로 추가하세요.").into()))?
        .to_string();
    git_combined(root, &["push", "-u", &remote, &head])
}

pub fn remotes(root: &Path) -> GitResult<Vec<String>> {
    Ok(git(root, Mode::Read, &["remote"])?.lines().map(str::to_string).collect())
}

// ---------------------------------------------------------------- stash

/// stash 항목.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    pub index: usize,
    /// `stash@{n}`
    pub reference: String,
    pub message: String,
    pub date: i64,
}

pub fn stash_list(root: &Path) -> GitResult<Vec<StashEntry>> {
    let out = git(root, Mode::Read, &["stash", "list", "--format=%gd%x00%gs%x00%ct"])?;
    Ok(out
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let f: Vec<&str> = l.split('\0').collect();
            (f.len() >= 3).then(|| StashEntry {
                index: i,
                reference: f[0].to_string(),
                message: f[1].to_string(),
                date: f[2].parse().unwrap_or(0),
            })
        })
        .collect())
}

/// 미추적 파일을 포함해 stash 한다.
pub fn stash_push(root: &Path, message: &str) -> GitResult<String> {
    let mut args = vec!["stash", "push", "--include-untracked"];
    if !message.trim().is_empty() {
        args.push("-m");
        args.push(message.trim());
    }
    git_combined(root, &args)
}

pub fn stash_pop(root: &Path, index: usize) -> GitResult<String> {
    git_combined(root, &["stash", "pop", &format!("stash@{{{index}}}")])
}

pub fn stash_apply(root: &Path, index: usize) -> GitResult<String> {
    git_combined(root, &["stash", "apply", &format!("stash@{{{index}}}")])
}

pub fn stash_drop(root: &Path, index: usize) -> GitResult<String> {
    git_combined(root, &["stash", "drop", &format!("stash@{{{index}}}")])
}

// ---------------------------------------------------------------- log

/// 커밋 로그 한 건.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: i64,
    /// `%D` 장식(`HEAD -> main, origin/main, tag: v1`).
    pub refs: Vec<String>,
    pub subject: String,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

const LOG_FORMAT: &str = "--format=%x1e%H%x00%P%x00%an%x00%ae%x00%at%x00%D%x00%s";

/// 최근 커밋 로그(`--topo-order`, 전체 브랜치가 아닌 HEAD 기준).
pub fn log(root: &Path, skip: usize, limit: usize) -> GitResult<Vec<Commit>> {
    let skip = format!("--skip={skip}");
    let n = format!("-n{limit}");
    match git(root, Mode::Read, &["log", "--topo-order", "--decorate=short", LOG_FORMAT, &skip, &n]) {
        Ok(out) => Ok(parse_log(&out)),
        Err(GitError::Failed(m)) if m.contains("does not have any commits") || m.contains("bad default revision") => {
            Ok(Vec::new())
        }
        Err(e) => Err(e),
    }
}

pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_matches('\n');
            if rec.is_empty() {
                return None;
            }
            let f: Vec<&str> = rec.splitn(7, '\0').collect();
            if f.len() < 7 {
                return None;
            }
            Some(Commit {
                sha: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                date: f[4].parse().unwrap_or(0),
                refs: f[5].split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect(),
                subject: f[6].to_string(),
            })
        })
        .collect()
}

/// `base..HEAD` 커밋의 (제목, 본문) 목록. PR 폼 미리 채우기용.
pub fn commits_since(root: &Path, base: &str) -> GitResult<Vec<(String, String)>> {
    let range = format!("{base}..HEAD");
    let out = git(root, Mode::Read, &["log", "--reverse", "--format=%x1e%s%x00%b", &range])?;
    Ok(out
        .split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .map(|r| {
            let (s, b) = r.split_once('\0').unwrap_or((r, ""));
            (s.trim().to_string(), b.trim().to_string())
        })
        .collect())
}

// ---------------------------------------------------------------- diff

/// 작업트리(또는 인덱스) 파일 하나의 diff. 미추적 파일은 전체를 추가로 만든다.
pub fn file_diff(root: &Path, path: &str, staged: bool) -> GitResult<Option<FileDiff>> {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M", "--patch"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(path);
    let out = git(root, Mode::Read, &args)?;
    if let Some(f) = parse_diff(&out).into_iter().next() {
        return Ok(Some(f));
    }
    if !staged {
        let tracked = git(root, Mode::Read, &["ls-files", "--error-unmatch", "--", path]).is_ok();
        if !tracked {
            let full = root.join(path);
            if full.is_file() {
                let data = std::fs::read(&full).map_err(|e| GitError::Failed(e.to_string()))?;
                return Ok(Some(synth_added_file(path, &data)));
            }
        }
    }
    Ok(None)
}

/// 커밋 상세.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: i64,
    pub committer: String,
    pub refs: Vec<String>,
    pub message: String,
    pub files: Vec<FileDiff>,
}

/// 커밋 헤더와 diff(병합 커밋은 첫 부모 기준).
pub fn commit_detail(root: &Path, sha: &str) -> GitResult<CommitDetail> {
    let head = git(root, Mode::Read, &["show", "-s", "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%D%x00%B", sha])?;
    let f: Vec<&str> = head.splitn(8, '\0').collect();
    if f.len() < 8 {
        return Err(GitError::Parse("commit header".into()));
    }
    let patch = git(
        root,
        Mode::Read,
        &["show", "--format=", "--no-color", "--no-ext-diff", "-M", "--patch", "--diff-merges=first-parent", sha],
    )?;
    Ok(CommitDetail {
        sha: f[0].to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        author: f[2].to_string(),
        email: f[3].to_string(),
        date: f[4].parse().unwrap_or(0),
        committer: f[5].to_string(),
        refs: f[6].split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect(),
        message: f[7].trim_end().to_string(),
        files: parse_diff(&patch),
    })
}

/// 헝크 적용 동작.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction {
    /// 작업트리 헝크를 인덱스에 반영.
    Stage,
    /// 인덱스 헝크를 되돌려 스테이지 해제.
    Unstage,
    /// 작업트리 헝크를 버림.
    Revert,
}

/// 헝크 패치를 `git apply` 로 적용한다.
pub fn apply_hunk(root: &Path, file: &FileDiff, hunk_index: usize, action: HunkAction) -> GitResult<()> {
    let patch = file
        .hunk_patch(hunk_index)
        .ok_or_else(|| GitError::Failed(kiln_common::i18n::tr("헝크를 찾을 수 없습니다").into()))?;
    let args: &[&str] = match action {
        HunkAction::Stage => &["apply", "--cached", "--whitespace=nowarn", "-"],
        HunkAction::Unstage => &["apply", "--cached", "-R", "--whitespace=nowarn", "-"],
        HunkAction::Revert => &["apply", "-R", "--whitespace=nowarn", "-"],
    };
    git_stdin(root, args, patch.as_bytes()).map(|_| ())
}

/// git 이 설치돼 있는지 확인한다.
pub fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 경로가 저장소 안인지 확인한다.
pub fn is_repo(root: &Path) -> bool {
    git_raw(root, Mode::Read, &["rev-parse", "--is-inside-work-tree"], None)
        .map(|o| o.success)
        .unwrap_or(false)
}
