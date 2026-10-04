//! `git` / `gh` CLI 실행 래퍼.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// git/gh 실행 오류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// `git` 실행 파일을 찾지 못함.
    GitMissing,
    /// `gh` 실행 파일을 찾지 못함.
    GhMissing,
    /// `gh`가 로그인되지 않음.
    GhAuth(String),
    /// 대상 경로가 git 저장소가 아님.
    NotARepo,
    /// 명령이 0이 아닌 코드로 끝남. stderr(없으면 stdout) 내용을 담는다.
    Failed(String),
    /// 출력 파싱 실패.
    Parse(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::GitMissing => write!(f, "git이 설치되어 있지 않거나 PATH에 없습니다"),
            GitError::GhMissing => write!(f, "GitHub CLI(gh)를 찾을 수 없습니다. 설치 경로와 PATH를 확인하세요 — https://cli.github.com"),
            GitError::GhAuth(m) => write!(f, "GitHub CLI 인증이 필요합니다. `gh auth login`을 실행하세요. {m}"),
            GitError::NotARepo => write!(f, "Git 저장소가 아닙니다"),
            GitError::Failed(m) => write!(f, "{m}"),
            GitError::Parse(m) => write!(f, "출력을 해석할 수 없습니다: {m}"),
        }
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

/// 명령 실행 방식.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// 읽기 전용 조회. `GIT_OPTIONAL_LOCKS=0`을 설정한다.
    Read,
    /// 저장소를 바꾸는 명령.
    Write,
}

fn base_git(root: &Path, mode: Mode) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .arg("-c")
        .arg("core.quotePath=false")
        .arg("-c")
        .arg("color.ui=false")
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
    // CREATE_NO_WINDOW
    c.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_c: &mut Command) {}

fn classify_git_failure(stderr: &str, stdout: &str) -> GitError {
    let s = stderr.trim();
    if s.contains("not a git repository") {
        return GitError::NotARepo;
    }
    let msg = if s.is_empty() { stdout.trim() } else { s };
    GitError::Failed(msg.to_string())
}

/// 출력 원본(바이트)과 종료 코드.
pub(crate) struct RawOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub success: bool,
}

pub(crate) fn git_raw(root: &Path, mode: Mode, args: &[&str], stdin: Option<&[u8]>) -> GitResult<RawOutput> {
    let mut c = base_git(root, mode);
    c.args(args);
    if stdin.is_some() {
        c.stdin(Stdio::piped());
    }
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            if root.exists() { GitError::GitMissing } else { GitError::NotARepo }
        } else {
            GitError::Failed(e.to_string())
        }
    })?;
    if let Some(data) = stdin
        && let Some(mut w) = child.stdin.take()
    {
        let data = data.to_vec();
        // stdin 은 별도 스레드에서 쓴다.
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        });
    }
    let out = child.wait_with_output().map_err(|e| GitError::Failed(e.to_string()))?;
    Ok(RawOutput {
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
    })
}

/// git 을 실행하고 성공 시 stdout 바이트를 돌려준다.
pub(crate) fn git_bytes(root: &Path, mode: Mode, args: &[&str]) -> GitResult<Vec<u8>> {
    let o = git_raw(root, mode, args, None)?;
    if o.success {
        Ok(o.stdout)
    } else {
        Err(classify_git_failure(&o.stderr, &String::from_utf8_lossy(&o.stdout)))
    }
}

/// git 을 실행하고 stdout 을 문자열로 돌려준다.
pub(crate) fn git(root: &Path, mode: Mode, args: &[&str]) -> GitResult<String> {
    git_bytes(root, mode, args).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// stdin 을 넘겨 git 을 실행한다.
pub(crate) fn git_stdin(root: &Path, args: &[&str], input: &[u8]) -> GitResult<String> {
    let o = git_raw(root, Mode::Write, args, Some(input))?;
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    if o.success {
        Ok(stdout)
    } else {
        Err(classify_git_failure(&o.stderr, &stdout))
    }
}

/// 사용자에게 보여줄 목적으로 stdout+stderr 를 합쳐 돌려준다.
pub(crate) fn git_combined(root: &Path, args: &[&str]) -> GitResult<String> {
    let o = git_raw(root, Mode::Write, args, None)?;
    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
    if !o.stderr.trim().is_empty() {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&o.stderr);
    }
    if o.success {
        Ok(s.trim_end().to_string())
    } else {
        Err(classify_git_failure(&o.stderr, &s))
    }
}

/// gh 를 실행하고 stdout 을 돌려준다.
pub(crate) fn gh(root: &Path, args: &[&str]) -> GitResult<String> {
    gh_stdin(root, args, None)
}

pub(crate) fn gh_stdin(root: &Path, args: &[&str], input: Option<&[u8]>) -> GitResult<String> {
    gh_stdin_with_path(root, args, input, std::env::var_os("PATH").as_deref())
}

/// Finder-launched apps do not inherit shell startup files. Keep explicit PATH
/// entries first, then add standard macOS installations for gh and its children.
/// https://docs.brew.sh/FAQ#my-macos-apps-dont-find-homebrew-utilities
fn gh_search_path(inherited: Option<&std::ffi::OsStr>) -> Option<std::ffi::OsString> {
    #[cfg(target_os = "macos")]
    {
        let mut paths: Vec<_> = inherited
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .collect();
        for directory in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ] {
            let path = std::path::PathBuf::from(directory);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        Some(
            std::env::join_paths(paths)
                .expect("existing PATH entries and standard macOS paths are valid"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        inherited.map(std::ffi::OsStr::to_os_string)
    }
}

fn gh_stdin_with_path(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    path: Option<&std::ffi::OsStr>,
) -> GitResult<String> {
    if !root.is_dir() {
        return Err(GitError::NotARepo);
    }
    let mut c = Command::new("gh");
    if let Some(path) = gh_search_path(path) {
        c.env("PATH", path);
    } else {
        c.env_remove("PATH");
    }
    c.current_dir(root)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut c);
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            if root.is_dir() {
                GitError::GhMissing
            } else {
                GitError::NotARepo
            }
        } else {
            GitError::Failed(e.to_string())
        }
    })?;
    if let Some(data) = input
        && let Some(mut w) = child.stdin.take()
    {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        });
    }
    let out = child
        .wait_with_output()
        .map_err(|e| GitError::Failed(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // Authentication is a CLI status, not a word in an arbitrary repository,
    // branch name, SSO policy or network error. Preserve those errors verbatim.
    // https://cli.github.com/manual/gh_help_exit-codes
    if out.status.code() == Some(4) {
        return Err(GitError::GhAuth(String::new()));
    }
    if stderr.contains("not a git repository") {
        return Err(GitError::NotARepo);
    }
    Err(GitError::Failed(if stderr.is_empty() {
        stdout.trim().to_string()
    } else {
        stderr
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_gh(directory: &Path, script: &str) {
        let executable = directory.join("gh");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn gh_preserves_explicit_path_priority_and_stdin() {
        let dir = tempfile::tempdir().unwrap();
        fake_gh(dir.path(), "printf 'custom gh: '; /bin/cat");
        let result = gh_stdin_with_path(
            dir.path(),
            &["api"],
            Some(b"request body"),
            Some(dir.path().as_os_str()),
        )
        .unwrap();
        assert_eq!(result, "custom gh: request body");
    }

    #[test]
    fn gh_missing_working_directory_is_not_missing_cli() {
        let dir = tempfile::tempdir().unwrap();
        fake_gh(dir.path(), "printf success");
        assert_eq!(
            gh_stdin_with_path(
                &dir.path().join("removed-repository"),
                &["--version"],
                None,
                Some(dir.path().as_os_str())
            ),
            Err(GitError::NotARepo)
        );
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert_eq!(
            gh_stdin_with_path(&file, &["--version"], None, None),
            Err(GitError::NotARepo)
        );
    }

    #[test]
    fn gh_auth_error_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        fake_gh(dir.path(), "printf 'Run gh auth login' >&2; exit 4");
        assert_eq!(
            gh_stdin_with_path(dir.path(), &["api"], None, Some(dir.path().as_os_str())),
            Err(GitError::GhAuth(String::new()))
        );
    }

    #[test]
    fn gh_repository_and_permission_errors_are_not_login_requests() {
        let dir = tempfile::tempdir().unwrap();
        for message in [
            "GraphQL: Could not resolve to a Repository with the name owner/authentication-service. (repository)",
            "GraphQL: Resource protected by organization SAML enforcement. You must grant your OAuth token access to this organization using SSO authentication.",
            "HTTP 403: Resource not accessible by integration",
            "error connecting to authentication.example.com",
        ] {
            fake_gh(dir.path(), &format!("printf '%s' '{message}' >&2; exit 1"));
            assert_eq!(
                gh_stdin_with_path(dir.path(), &["repo", "view"], None, Some(dir.path().as_os_str())),
                Err(GitError::Failed(message.into()))
            );
        }
        // The documented status remains authoritative even without English text.
        fake_gh(dir.path(), "exit 4");
        assert_eq!(
            gh_stdin_with_path(dir.path(), &["repo", "view"], None, Some(dir.path().as_os_str())),
            Err(GitError::GhAuth(String::new()))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_fallback_path_reaches_gh_children_without_changing_process_path() {
        let dir = tempfile::tempdir().unwrap();
        let original_path = std::env::var_os("PATH");
        fake_gh(dir.path(), "printf '%s\\n' \"$PATH\"; command -v git");
        let output = gh_stdin_with_path(
            dir.path(),
            &["--version"],
            None,
            Some(dir.path().as_os_str()),
        )
        .unwrap();
        let mut lines = output.lines();
        let paths: Vec<_> = std::env::split_paths(lines.next().unwrap()).collect();
        assert_eq!(paths[0], dir.path());
        assert!(paths.contains(&Path::new("/opt/homebrew/bin").to_path_buf()));
        assert!(paths.contains(&Path::new("/usr/local/bin").to_path_buf()));
        assert!(lines.next().unwrap().ends_with("/git"));
        assert_eq!(std::env::var_os("PATH"), original_path);
        let missing_path = gh_search_path(None).unwrap();
        assert!(std::env::split_paths(&missing_path).any(|p| p == Path::new("/opt/homebrew/bin")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires gh installed at a standard Homebrew prefix; no network or authentication"]
    fn installed_homebrew_gh_works_with_finder_path_and_absent_path() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            Some(std::ffi::OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin")),
            None,
        ] {
            let output = gh_stdin_with_path(dir.path(), &["--version"], None, path).unwrap();
            assert!(output.starts_with("gh version "), "{output}");
        }
    }
}
