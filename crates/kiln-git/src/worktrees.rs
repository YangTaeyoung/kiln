//! Non-destructive worktree management. All functions block; call from a worker.
//! https://git-scm.com/docs/git-worktree
use crate::cmd::{self, GitError, GitResult, Mode};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub locked: bool,
    pub bare: bool,
}
/// Git-reported identity of an exact workspace root, never an ancestor repo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeIdentity {
    pub main_root: PathBuf,
    pub root: PathBuf,
    pub branch: Option<String>,
    pub linked: bool,
}

pub fn identity(root: &Path) -> GitResult<Option<WorktreeIdentity>> {
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };
    let trees = match list(&root) {
        Ok(trees) => trees,
        Err(GitError::NotARepo) => return Ok(None),
        Err(error) => return Err(error),
    };
    // Git documents the main worktree as the first entry, including bare roots.
    let Some(main) = trees.first() else { return Ok(None); };
    let main_root = main.path.canonicalize().unwrap_or_else(|_| main.path.clone());
    let Some(tree) = trees.iter().find(|tree| {
        tree.path.canonicalize().unwrap_or_else(|_| tree.path.clone()) == root
    }) else { return Ok(None); };
    if tree.bare { return Ok(None); }
    Ok(Some(WorktreeIdentity {
        linked: root != main_root,
        main_root,
        root,
        branch: tree.branch.clone(),
    }))
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub root: PathBuf,
    pub trees: Vec<Worktree>,
    pub refs: Vec<String>,
    pub local_refs: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Create {
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub existing_branch: bool,
}

pub fn inspect(root: &Path) -> GitResult<Snapshot> {
    let top = cmd::git(root, Mode::Read, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(top.strip_suffix('\n').unwrap_or(&top));
    let trees = list(&root)?;
    let refs = cmd::git(
        &root,
        Mode::Read,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?
    .lines()
    .filter(|s| !s.ends_with("/HEAD"))
    .map(str::to_owned)
    .collect();
    let local_refs = cmd::git(
        &root,
        Mode::Read,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    Ok(Snapshot {
        root,
        trees,
        refs,
        local_refs,
    })
}
pub fn list(root: &Path) -> GitResult<Vec<Worktree>> {
    parse(&cmd::git_bytes(
        root,
        Mode::Read,
        &["worktree", "list", "--porcelain", "-z"],
    )?)
}
fn parse(bytes: &[u8]) -> GitResult<Vec<Worktree>> {
    let mut result = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in bytes.split(|b| *b == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            if let Some(tree) = current.take() {
                result.push(tree);
            }
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(
                std::str::from_utf8(path).map_err(|e| GitError::Parse(e.to_string()))?,
            );
            current = Some(Worktree {
                path,
                branch: None,
                locked: false,
                bare: false,
            });
        } else if let Some(tree) = current.as_mut() {
            if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
                tree.branch = Some(String::from_utf8_lossy(branch).into_owned());
            }
            if field == b"bare" {
                tree.bare = true;
            }
            if field == b"locked" || field.starts_with(b"locked ") {
                tree.locked = true;
            }
        }
    }
    if let Some(tree) = current {
        result.push(tree);
    }
    Ok(result)
}
pub fn create(root: &Path, request: &Create) -> GitResult<Worktree> {
    let fail = |msg: &str| GitError::Failed(msg.to_owned());
    if !request.path.is_absolute() {
        return Err(fail("Worktree 폴더는 전체 경로로 입력하세요."));
    }
    if request.path.symlink_metadata().is_ok() {
        return Err(fail("이미 존재하는 폴더입니다. 새 경로를 입력하세요."));
    }
    let path = request
        .path
        .to_str()
        .ok_or_else(|| fail("이 경로는 UTF-8 형식으로 표현할 수 없습니다."))?;
    let branch = request.branch.trim();
    if branch.is_empty() || branch.starts_with('-') || branch.starts_with('@') {
        return Err(fail("새 브랜치 이름을 확인하세요."));
    }
    cmd::git(root, Mode::Read, &["check-ref-format", "--branch", branch])?;
    if request.existing_branch {
        let full = format!("refs/heads/{branch}");
        cmd::git(root, Mode::Read, &["show-ref", "--verify", &full])?;
        cmd::git(root, Mode::Write, &["worktree", "add", "--", path, branch])?;
    } else {
        // Resolve before mutation, preventing option injection and ambiguous refs.
        let base = request.base.trim();
        if base.is_empty() {
            return Err(fail("시작 브랜치 또는 커밋을 선택하세요."));
        }
        let commit = cmd::git(
            root,
            Mode::Read,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ],
        )?;
        cmd::git(
            root,
            Mode::Write,
            &["worktree", "add", "-b", branch, "--", path, commit.trim()],
        )?;
    }
    // Do not remove files or a branch on subsequent errors. The caller can reload.
    Ok(Worktree {
        path: request
            .path
            .canonicalize()
            .unwrap_or_else(|_| request.path.clone()),
        branch: Some(branch.into()),
        locked: false,
        bare: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        ] {
            let out = std::process::Command::new("git")
                .current_dir(d.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        d
    }
    #[test]
    fn identity_distinguishes_main_linked_and_unrelated_folders() {
        let main = repo();
        let parent = tempfile::tempdir().unwrap();
        let linked = parent.path().join("same-looking-name");
        create(main.path(), &Create { path: linked.clone(), branch: "feature/work".into(), base: "main".into(), existing_branch: false }).unwrap();
        let main_root = main.path().canonicalize().unwrap();
        assert_eq!(identity(main.path()).unwrap(), Some(WorktreeIdentity { main_root: main_root.clone(), root: main_root.clone(), branch: Some("main".into()), linked: false }));
        assert_eq!(identity(&linked).unwrap(), Some(WorktreeIdentity { main_root, root: linked.canonicalize().unwrap(), branch: Some("feature/work".into()), linked: true }));
        let child = linked.join("nested");
        std::fs::create_dir(&child).unwrap();
        assert_eq!(identity(&child).unwrap(), None);
        assert_eq!(identity(parent.path()).unwrap(), None);
        let sibling = parent.path().join("sibling");
        std::fs::create_dir(&sibling).unwrap();
        assert_eq!(identity(&sibling).unwrap(), None);
        #[cfg(unix)] {
            let alias = parent.path().join("alias");
            std::os::unix::fs::symlink(&linked, &alias).unwrap();
            assert_eq!(identity(&alias).unwrap(), identity(&linked).unwrap());
        }
    }
    #[test]
    fn porcelain_preserves_spaces_newlines_and_lock_state() {
        let trees = parse(b"worktree /tmp/a\nb c\0HEAD abc\0branch refs/heads/main\0locked reason\0\0worktree /tmp/d\0HEAD abc\0detached\0\0").unwrap();
        assert_eq!(trees[0].path, Path::new("/tmp/a\nb c"));
        assert!(trees[0].locked);
        assert_eq!(trees[1].branch, None);
    }
    #[test]
    fn creates_from_selected_base_and_refuses_overwrite_or_checked_out_branch() {
        let repo = repo();
        let out = tempfile::tempdir().unwrap();
        let req = Create {
            path: out.path().join("task folder"),
            branch: "feature/test".into(),
            base: "main".into(),
            existing_branch: false,
        };
        create(repo.path(), &req).unwrap();
        assert_eq!(list(repo.path()).unwrap().len(), 2);
        std::fs::write(req.path.join("keep.txt"), "keep").unwrap();
        assert!(create(repo.path(), &req).is_err());
        assert_eq!(
            std::fs::read_to_string(req.path.join("keep.txt")).unwrap(),
            "keep"
        );
        let checked = Create {
            path: out.path().join("other"),
            branch: "main".into(),
            existing_branch: true,
            ..req.clone()
        };
        assert!(create(repo.path(), &checked).is_err());
        assert!(!checked.path.exists());
    }
    #[test]
    fn invalid_base_does_not_create_branch_or_directory() {
        let repo = repo();
        let out = tempfile::tempdir().unwrap();
        let req = Create {
            path: out.path().join("bad"),
            branch: "task".into(),
            base: "--help".into(),
            existing_branch: false,
        };
        assert!(create(repo.path(), &req).is_err());
        assert!(!req.path.exists());
        assert_eq!(inspect(repo.path()).unwrap().refs, vec!["main"]);
    }
}
