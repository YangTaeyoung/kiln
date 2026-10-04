//! Bounded local workspace discovery. No remotes are contacted and symlinks are not followed.
use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub roots: Vec<PathBuf>,
    pub limited: bool,
    pub unreadable: usize,
}
pub fn discover(root: &Path) -> Inventory {
    let mut out = Inventory::default();
    let mut roots = BTreeSet::new();
    // A selected subdirectory still belongs to its containing working tree,
    // including when it also contains independent nested repositories.
    if let Ok(top) = crate::repo::toplevel(root) {
        roots.insert(top);
    }
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut visited = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        visited += 1;
        if visited > 20000 || roots.len() >= 128 {
            out.limited = true;
            break;
        }
        if dir.join(".git").exists() {
            if let Ok(top) = crate::repo::toplevel(&dir) {
                roots.insert(top);
            }
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                out.unreadable += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() || kind.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "node_modules"
                        | "target"
                        | "vendor"
                        | "dist"
                        | "build"
                        | "coverage"
                        | "__pycache__"
                )
            {
                continue;
            }
            if depth >= 5 {
                out.limited = true;
                continue;
            }
            queue.push_back((entry.path(), depth + 1));
        }
    }
    out.roots = roots.into_iter().collect();
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn git(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null"
                ])
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    #[test]
    fn subdirectory_keeps_containing_tree_alongside_nested_repositories() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let selected = dir.path().join("services");
        let nested = selected.join("external-api");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        let found = discover(&selected);
        assert_eq!(found.roots.len(), 2);
        assert!(found.roots.contains(&dir.path().canonicalize().unwrap()));
        assert!(found.roots.contains(&nested.canonicalize().unwrap()));
    }
    #[test]
    fn parent_discovers_siblings_nested_and_linked_worktrees_without_cache_or_symlink_recursion() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("front");
        let b = d.path().join("services/back");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        git(&a, &["init", "-q"]);
        git(&b, &["init", "-q"]);
        git(
            &a,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        );
        let w = d.path().join("front-review");
        git(
            &a,
            &["worktree", "add", "-q", "-b", "review", w.to_str().unwrap()],
        );
        let ignored = d.path().join("node_modules/pkg");
        std::fs::create_dir_all(&ignored).unwrap();
        git(&ignored, &["init", "-q"]);
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path(), d.path().join("loop")).unwrap();
        let found = discover(d.path());
        assert_eq!(found.roots.len(), 3);
        assert!(found.roots.contains(&a.canonicalize().unwrap()));
        assert!(found.roots.contains(&b.canonicalize().unwrap()));
        assert!(found.roots.contains(&w.canonicalize().unwrap()));
    }
}
