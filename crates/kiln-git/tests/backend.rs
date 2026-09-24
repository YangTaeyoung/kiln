//! git 백엔드 통합 테스트(임시 저장소 사용).

mod common;

use common::{Repo, numbered};
use kiln_git::diff::{FileChange, LineKind, parse_diff};
use kiln_git::graph::compute_graph;
use kiln_git::repo::{self, HunkAction, RepoOp};
use kiln_git::status::{EntryKind, decoration_char};
use kiln_git::{GitError, repo_summary};

fn base_repo() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "alpha\n");
    r.write("b.txt", "bravo\n");
    r.write("src/lib.rs", &numbered(30));
    r.commit_all("initial");
    r
}

#[test]
fn status_reports_modified_added_deleted_and_untracked() {
    let r = base_repo();
    r.write("a.txt", "alpha changed\n");
    std::fs::remove_file(r.path.join("b.txt")).unwrap();
    r.write("new.txt", "new\n");
    r.git(&["add", "new.txt"]);
    r.write("scratch.md", "tmp\n");
    r.write("newdir/inner.txt", "x\n");

    let st = repo::status(&r.path).unwrap();
    assert_eq!(st.branch.head.as_deref(), Some("main"));
    assert!(st.branch.oid.is_some());
    let get = |p: &str| st.entries.iter().find(|e| e.path == p).unwrap_or_else(|| panic!("{p} missing: {:?}", st.entries));
    let a = get("a.txt");
    assert_eq!((a.index, a.worktree), ('.', 'M'));
    assert!(a.is_unstaged() && !a.is_staged());
    let b = get("b.txt");
    assert_eq!(b.worktree, 'D');
    assert_eq!(decoration_char(b), 'D');
    let n = get("new.txt");
    assert_eq!(n.index, 'A');
    assert!(n.is_staged());
    assert_eq!(decoration_char(n), 'A');
    assert!(get("scratch.md").is_untracked());
    assert_eq!(decoration_char(get("scratch.md")), '?');
    assert!(get("newdir/").is_untracked());
    assert_eq!(st.changed_count(), 5);
    assert_eq!(st.conflicted_count(), 0);
}

#[test]
fn status_parses_staged_rename_with_original_path() {
    let r = base_repo();
    r.git(&["mv", "src/lib.rs", "src/main.rs"]);
    let st = repo::status(&r.path).unwrap();
    let e = st.entries.iter().find(|e| e.kind == EntryKind::Renamed).expect("rename entry");
    assert_eq!(e.path, "src/main.rs");
    assert_eq!(e.orig_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(e.index, 'R');
    assert_eq!(decoration_char(e), 'R');
}

#[test]
fn status_keeps_unicode_and_space_filenames_verbatim() {
    let r = base_repo();
    // 따옴표·탭이 든 파일명은 유닉스에서만 만든다.
    let odd = if cfg!(unix) { "café \"quoted\".txt" } else { "café quoted.txt" };
    r.write("dir with space/파일 이름.txt", "안녕\n");
    r.write(odd, "x\n");
    r.git(&["add", "-A"]);
    if cfg!(unix) {
        r.write("tab\tname.txt", "y\n");
    }
    let st = repo::status(&r.path).unwrap();
    let paths: Vec<&str> = st.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"dir with space/파일 이름.txt"), "{paths:?}");
    assert!(paths.contains(&odd), "{paths:?}");
    if cfg!(unix) {
        assert!(paths.contains(&"tab\tname.txt"), "{paths:?}");
    }

    // 스테이지/해제도 같은 경로로 동작한다.
    repo::unstage(&r.path, &["dir with space/파일 이름.txt".into()]).unwrap();
    let st = repo::status(&r.path).unwrap();
    assert!(st.untracked().any(|e| e.path.starts_with("dir with space/")));

    let d = repo::file_diff(&r.path, odd, true).unwrap().expect("diff");
    assert_eq!(d.path(), odd);
    assert_eq!(d.change, FileChange::Added);
}

#[test]
fn status_reports_real_merge_conflict_and_abort_restores_state() {
    let r = base_repo();
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("a.txt", "alpha from feature\n");
    r.commit_all("feature change");
    r.git(&["switch", "-q", "main"]);
    r.write("a.txt", "alpha from main\n");
    r.commit_all("main change");
    assert!(!r.git_status(&["merge", "feature"]), "merge should conflict");

    let st = repo::status(&r.path).unwrap();
    let c: Vec<_> = st.conflicted().collect();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].path, "a.txt");
    assert_eq!((c[0].index, c[0].worktree), ('U', 'U'));
    assert_eq!(c[0].conflict_label(), "both modified");
    assert_eq!(decoration_char(c[0]), 'C');
    assert_eq!(st.conflicted_count(), 1);
    assert_eq!(repo::in_progress_op(&r.path), Some(RepoOp::Merge));

    let summary = repo_summary(&r.path).unwrap();
    assert_eq!(summary.conflicted, 1);

    repo::abort_op(&r.path, RepoOp::Merge).unwrap();
    assert_eq!(repo::in_progress_op(&r.path), None);
    assert_eq!(repo::status(&r.path).unwrap().entries.len(), 0);
}

#[test]
fn resolving_conflict_with_theirs_stages_file_and_commit_completes_merge() {
    let r = base_repo();
    r.git(&["switch", "-q", "-c", "feature"]);
    r.write("a.txt", "theirs\n");
    r.commit_all("feature");
    r.git(&["switch", "-q", "main"]);
    r.write("a.txt", "ours\n");
    r.commit_all("main");
    assert!(!r.git_status(&["merge", "feature"]));
    repo::resolve_conflict(&r.path, "a.txt", false).unwrap();
    assert_eq!(r.read("a.txt"), "theirs\n");
    let st = repo::status(&r.path).unwrap();
    assert_eq!(st.conflicted_count(), 0);
    repo::commit(&r.path, "Merge feature", false).unwrap();
    assert_eq!(repo::in_progress_op(&r.path), None);
    let log = repo::log(&r.path, 0, 1).unwrap();
    assert_eq!(log[0].parents.len(), 2);
}

#[test]
fn status_reports_detached_head() {
    let r = base_repo();
    r.write("a.txt", "second\n");
    let second = r.commit_all("second");
    r.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    let st = repo::status(&r.path).unwrap();
    assert!(st.branch.is_detached());
    assert!(st.branch.display_name().starts_with('('));
    assert_ne!(st.branch.oid.as_deref(), Some(second.as_str()));
    let s = repo_summary(&r.path).unwrap();
    assert!(s.branch.starts_with('('));
    assert!(s.pr.is_none());
}

#[test]
fn status_on_fresh_repository_without_commits() {
    let r = Repo::new();
    r.write("first.txt", "1\n");
    let st = repo::status(&r.path).unwrap();
    assert_eq!(st.branch.head.as_deref(), Some("main"));
    assert_eq!(st.branch.oid, None);
    assert_eq!(repo::log(&r.path, 0, 10).unwrap(), vec![]);
    // HEAD 없이 스테이지/해제
    repo::stage(&r.path, &["first.txt".into()]).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().staged().count(), 1);
    repo::unstage(&r.path, &["first.txt".into()]).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().untracked().count(), 1);
    repo::stage_all(&r.path).unwrap();
    repo::unstage_all(&r.path).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().untracked().count(), 1);
}

#[test]
fn not_a_repository_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(repo::status(dir.path()), Err(GitError::NotARepo));
    assert!(repo_summary(dir.path()).is_none());
    assert!(!repo::is_repo(dir.path()));
    assert!(repo::git_available());
}

#[test]
fn stage_unstage_and_discard_round_trip() {
    let r = base_repo();
    r.write("a.txt", "changed\n");
    r.write("b.txt", "changed too\n");
    repo::stage(&r.path, &["a.txt".into()]).unwrap();
    let st = repo::status(&r.path).unwrap();
    assert_eq!(st.staged().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);
    repo::unstage(&r.path, &["a.txt".into()]).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().staged().count(), 0);

    repo::stage_all(&r.path).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().staged().count(), 2);
    repo::unstage_all(&r.path).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().staged().count(), 0);

    repo::discard(&r.path, &["a.txt".into()]).unwrap();
    assert_eq!(r.read("a.txt"), "alpha\n");
    assert_eq!(r.read("b.txt"), "changed too\n");

    // 삭제된 파일 스테이지
    std::fs::remove_file(r.path.join("b.txt")).unwrap();
    repo::stage(&r.path, &["b.txt".into()]).unwrap();
    let st = repo::status(&r.path).unwrap();
    assert_eq!(st.staged().next().unwrap().index, 'D');

    r.write("junk/x.tmp", "x");
    repo::clean_untracked(&r.path, &["junk/".into()]).unwrap();
    assert!(!r.path.join("junk").exists());
}

#[test]
fn commit_and_amend() {
    let r = base_repo();
    r.write("a.txt", "one\n");
    repo::stage_all(&r.path).unwrap();
    assert!(repo::commit(&r.path, "   ", false).is_err());
    repo::commit(&r.path, "Add one\n\nLonger body text.", false).unwrap();
    assert_eq!(repo::last_commit_message(&r.path).unwrap(), "Add one\n\nLonger body text.");

    r.write("a.txt", "two\n");
    repo::stage_all(&r.path).unwrap();
    // 빈 메시지 amend 는 기존 메시지를 유지한다.
    repo::commit(&r.path, "", true).unwrap();
    assert_eq!(repo::last_commit_message(&r.path).unwrap(), "Add one\n\nLonger body text.");
    let log = repo::log(&r.path, 0, 10).unwrap();
    assert_eq!(log.len(), 2);

    repo::commit(&r.path, "Reworded", true).unwrap();
    assert_eq!(repo::last_commit_message(&r.path).unwrap(), "Reworded");
    assert_eq!(repo::log(&r.path, 0, 10).unwrap().len(), 2);
    assert_eq!(repo::status(&r.path).unwrap().entries.len(), 0);
}

#[test]
fn branch_create_checkout_and_delete() {
    let r = base_repo();
    assert!(repo::create_branch(&r.path, "bad..name").is_err());
    repo::create_branch(&r.path, "feature/x").unwrap();
    assert_eq!(repo::status(&r.path).unwrap().branch.head.as_deref(), Some("feature/x"));
    r.write("f.txt", "feature\n");
    r.commit_all("feature work");

    let branches = repo::branches(&r.path).unwrap();
    let cur = branches.iter().find(|b| b.current).unwrap();
    assert_eq!(cur.name, "feature/x");
    assert_eq!(cur.subject, "feature work");
    assert!(branches.iter().any(|b| b.name == "main" && !b.remote));

    repo::checkout_name(&r.path, "main").unwrap();
    // 병합되지 않은 브랜치는 -d 로 지워지지 않는다.
    let err = repo::delete_branch(&r.path, "feature/x", false).unwrap_err();
    assert!(err.to_string().contains("not fully merged"), "{err}");
    repo::delete_branch(&r.path, "feature/x", true).unwrap();
    assert!(!repo::branches(&r.path).unwrap().iter().any(|b| b.name == "feature/x"));
}

#[test]
fn remote_branch_checkout_tracks_and_push_publishes_upstream() {
    let origin = Repo::new();
    origin.write("readme.md", "hi\n");
    origin.commit_all("root");
    origin.git(&["switch", "-q", "-c", "topic"]);
    origin.write("t.txt", "topic\n");
    origin.commit_all("topic");
    origin.git(&["switch", "-q", "main"]);

    let clone_path = origin.tempdir().join("clone");
    Repo::git_in(
        origin.tempdir(),
        &["clone", "-q", origin.path.to_str().unwrap(), clone_path.to_str().unwrap()],
        common::BASE_TS,
    );
    let c = Repo::at(clone_path);
    let branches = repo::branches(&c.path).unwrap();
    let remote_topic = branches.iter().find(|b| b.name == "origin/topic").expect("remote branch").clone();
    assert!(remote_topic.remote);
    assert!(!branches.iter().any(|b| b.name.ends_with("/HEAD")));

    repo::checkout(&c.path, &remote_topic).unwrap();
    let st = repo::status(&c.path).unwrap();
    assert_eq!(st.branch.head.as_deref(), Some("topic"));
    assert_eq!(st.branch.upstream.as_deref(), Some("origin/topic"));

    // ahead 계산
    c.write("t.txt", "topic 2\n");
    c.commit_all("topic 2");
    let st = repo::status(&c.path).unwrap();
    assert_eq!((st.branch.ahead, st.branch.behind), (1, 0));

    // upstream 없는 브랜치 푸시 → -u 로 게시
    repo::create_branch(&c.path, "published").unwrap();
    assert!(repo::status(&c.path).unwrap().branch.upstream.is_none());
    repo::push(&c.path).unwrap();
    assert_eq!(repo::status(&c.path).unwrap().branch.upstream.as_deref(), Some("origin/published"));

    // behind 계산
    origin.git(&["switch", "-q", "published"]);
    origin.write("o.txt", "upstream\n");
    origin.commit_all("upstream change");
    repo::fetch(&c.path).unwrap();
    let st = repo::status(&c.path).unwrap();
    assert_eq!(st.branch.behind, 1);
    let s = repo_summary(&c.path).unwrap();
    assert_eq!((s.branch.as_str(), s.behind), ("published", 1));
    repo::pull(&c.path).unwrap();
    assert_eq!(repo::status(&c.path).unwrap().branch.behind, 0);
}

#[test]
fn stash_push_list_apply_pop_and_drop() {
    let r = base_repo();
    r.write("a.txt", "wip 1\n");
    r.write("untracked.txt", "u\n");
    repo::stash_push(&r.path, "first wip").unwrap();
    assert_eq!(r.read("a.txt"), "alpha\n");
    assert!(!r.path.join("untracked.txt").exists());
    r.write("b.txt", "wip 2\n");
    repo::stash_push(&r.path, "").unwrap();

    let list = repo::stash_list(&r.path).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].reference, "stash@{0}");
    assert!(list[1].message.ends_with("first wip"), "{:?}", list[1]);
    assert_eq!(repo::status(&r.path).unwrap().stash_count, 2);

    repo::stash_apply(&r.path, 1).unwrap();
    assert_eq!(r.read("a.txt"), "wip 1\n");
    assert_eq!(repo::stash_list(&r.path).unwrap().len(), 2);
    repo::discard(&r.path, &["a.txt".into()]).unwrap();
    repo::clean_untracked(&r.path, &["untracked.txt".into()]).unwrap();

    repo::stash_pop(&r.path, 0).unwrap();
    assert_eq!(r.read("b.txt"), "wip 2\n");
    assert_eq!(repo::stash_list(&r.path).unwrap().len(), 1);
    repo::stash_drop(&r.path, 0).unwrap();
    assert!(repo::stash_list(&r.path).unwrap().is_empty());
}

#[test]
fn log_parses_refs_parents_and_graph_lanes() {
    let r = base_repo();
    r.git(&["tag", "v1.0"]);
    r.git(&["switch", "-q", "-c", "side"]);
    r.write("side.txt", "s\n");
    r.commit_all("side work");
    r.git(&["switch", "-q", "main"]);
    r.write("main.txt", "m\n");
    r.commit_all("main work | with pipes");
    let ts = r.next_ts();
    Repo::git_in(&r.path, &["merge", "-q", "--no-ff", "-m", "Merge side", "side"], ts);

    let log = repo::log(&r.path, 0, 50).unwrap();
    assert_eq!(log.len(), 4);
    assert_eq!(log[0].subject, "Merge side");
    assert_eq!(log[0].parents.len(), 2);
    assert!(log[0].refs.iter().any(|x| x == "HEAD -> main"), "{:?}", log[0].refs);
    assert_eq!(log[0].author, "Kiln Tester");
    assert_eq!(log[0].email, "tester@kiln.dev");
    assert!(log[0].date > common::BASE_TS);
    let root = log.iter().find(|c| c.subject == "initial").unwrap();
    assert!(root.refs.iter().any(|x| x == "tag: v1.0"));
    assert!(log.iter().any(|c| c.subject == "main work | with pipes"));
    assert!(log.iter().any(|c| c.refs.iter().any(|x| x == "side")));

    let g = compute_graph(&log);
    assert_eq!(g.len(), 4);
    assert_eq!(g[0].col, 0);
    assert_eq!(g[0].bottom_from_commit.len(), 2, "merge commit fans out to two lanes");
    assert!(g.iter().any(|row| row.col == 1), "second parent uses a second lane");
    assert_eq!(g[3].col, 0);
    assert!(g[3].top.len() == 2, "both lanes converge on the root commit: {:?}", g[3]);

    // 페이지 넘김
    let page = repo::log(&r.path, 2, 10).unwrap();
    assert_eq!(page.len(), 2);
}

#[test]
fn file_diff_parses_hunks_line_numbers_and_word_emphasis() {
    let r = base_repo();
    let mut content = numbered(30);
    content = content.replace("line 3\n", "line three\n").replace("line 25\n", "line 25\ninserted\n");
    r.write("src/lib.rs", &content);
    let d = repo::file_diff(&r.path, "src/lib.rs", false).unwrap().expect("diff");
    assert_eq!(d.path(), "src/lib.rs");
    assert_eq!(d.change, FileChange::Modified);
    assert_eq!(d.hunks.len(), 2);
    assert_eq!((d.added(), d.removed()), (2, 1));
    let h0 = &d.hunks[0];
    assert_eq!((h0.old_start, h0.new_start), (1, 1));
    let rem = h0.lines.iter().find(|l| l.kind == LineKind::Remove).unwrap();
    let add = h0.lines.iter().find(|l| l.kind == LineKind::Add).unwrap();
    assert_eq!(rem.old_no, Some(3));
    assert_eq!(add.new_no, Some(3));
    let (s, e) = add.emph.expect("emphasis");
    assert_eq!(&add.text[s..e], "three");
    let (s, e) = rem.emph.expect("emphasis");
    assert_eq!(&rem.text[s..e], "3");
    let ins = d.hunks[1].lines.iter().find(|l| l.kind == LineKind::Add).unwrap();
    assert_eq!(ins.text, "inserted");
    assert_eq!(ins.new_no, Some(26));

    // 스테이지된 diff 는 처음엔 없다.
    assert!(repo::file_diff(&r.path, "src/lib.rs", true).unwrap().is_none());

    // 미추적 파일은 전체가 추가로 보인다.
    r.write("fresh.txt", "a\nb\n");
    let u = repo::file_diff(&r.path, "fresh.txt", false).unwrap().unwrap();
    assert_eq!(u.change, FileChange::Added);
    assert_eq!(u.added(), 2);
    assert_eq!(u.hunks[0].lines[1].new_no, Some(2));
}

#[test]
fn hunk_stage_unstage_and_revert_apply_single_hunks() {
    let r = base_repo();
    let content = numbered(30).replace("line 2\n", "line 2 edited\n").replace("line 28\n", "line 28 edited\n");
    r.write("src/lib.rs", &content);
    let d = repo::file_diff(&r.path, "src/lib.rs", false).unwrap().unwrap();
    assert_eq!(d.hunks.len(), 2);

    // 두 번째 헝크만 스테이지
    repo::apply_hunk(&r.path, &d, 1, HunkAction::Stage).unwrap();
    let staged = repo::file_diff(&r.path, "src/lib.rs", true).unwrap().unwrap();
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.hunks[0].lines.iter().any(|l| l.text == "line 28 edited"));
    let unstaged = repo::file_diff(&r.path, "src/lib.rs", false).unwrap().unwrap();
    assert_eq!(unstaged.hunks.len(), 1);
    assert!(unstaged.hunks[0].lines.iter().any(|l| l.text == "line 2 edited"));

    // 스테이지된 헝크 해제
    repo::apply_hunk(&r.path, &staged, 0, HunkAction::Unstage).unwrap();
    assert!(repo::file_diff(&r.path, "src/lib.rs", true).unwrap().is_none());

    // 첫 헝크 되돌리기(작업트리)
    let d = repo::file_diff(&r.path, "src/lib.rs", false).unwrap().unwrap();
    repo::apply_hunk(&r.path, &d, 0, HunkAction::Revert).unwrap();
    let text = r.read("src/lib.rs");
    assert!(text.contains("line 2\n"));
    assert!(text.contains("line 28 edited\n"));

    // 새 파일(추가) 헝크 스테이지
    r.write("added.txt", "x\ny\n");
    r.git(&["add", "-N", "added.txt"]);
    let d = repo::file_diff(&r.path, "added.txt", false).unwrap().unwrap();
    repo::apply_hunk(&r.path, &d, 0, HunkAction::Stage).unwrap();
    let st = repo::status(&r.path).unwrap();
    let e = st.entries.iter().find(|e| e.path == "added.txt").unwrap();
    assert_eq!(e.index, 'A');
    assert_eq!(e.worktree, '.');
}

#[test]
fn hunk_patch_keeps_no_newline_marker() {
    let r = Repo::new();
    r.write("n.txt", "a\nb");
    r.commit_all("no newline");
    r.write("n.txt", "a\nc");
    let d = repo::file_diff(&r.path, "n.txt", false).unwrap().unwrap();
    assert!(d.hunks[0].lines.iter().any(|l| l.kind == LineKind::NoNewline));
    let patch = d.hunk_patch(0).unwrap();
    assert!(patch.contains("\\ No newline at end of file"));
    repo::apply_hunk(&r.path, &d, 0, HunkAction::Stage).unwrap();
    assert_eq!(repo::status(&r.path).unwrap().staged().count(), 1);
}

#[test]
fn commit_detail_includes_header_and_rename_and_binary_files() {
    let r = base_repo();
    r.git(&["mv", "a.txt", "renamed.txt"]);
    std::fs::write(r.path.join("img.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
    r.write("src/lib.rs", &numbered(31));
    let sha = r.commit_all("Refactor things\n\nBody line.");
    let c = repo::commit_detail(&r.path, &sha).unwrap();
    assert_eq!(c.sha, sha);
    assert_eq!(c.author, "Kiln Tester");
    assert_eq!(c.message, "Refactor things\n\nBody line.");
    assert_eq!(c.parents.len(), 1);
    assert!(c.refs.iter().any(|x| x.contains("main")));
    let ren = c.files.iter().find(|f| f.change == FileChange::Renamed).expect("rename");
    assert_eq!(ren.old_path.as_deref(), Some("a.txt"));
    assert_eq!(ren.new_path.as_deref(), Some("renamed.txt"));
    let bin = c.files.iter().find(|f| f.path() == "img.bin").unwrap();
    assert!(bin.binary);
    assert_eq!(bin.change, FileChange::Added);
    let m = c.files.iter().find(|f| f.path() == "src/lib.rs").unwrap();
    assert_eq!(m.added(), 1);

    // 첫 커밋(루트)도 읽을 수 있다.
    let root = repo::log(&r.path, 0, 10).unwrap().last().unwrap().sha.clone();
    let rc = repo::commit_detail(&r.path, &root).unwrap();
    assert!(rc.parents.is_empty());
    assert_eq!(rc.files.len(), 3);
}

#[test]
fn diff_parser_handles_quoted_paths_mode_changes_and_deletions() {
    let text = "diff --git \"a/sp\\303\\251cial \\\"x\\\".txt\" \"b/sp\\303\\251cial \\\"x\\\".txt\"\n\
index 1111111..2222222 100644\n\
--- \"a/sp\\303\\251cial \\\"x\\\".txt\"\n\
+++ \"b/sp\\303\\251cial \\\"x\\\".txt\"\n\
@@ -1 +1 @@\n\
-old\n\
+new\n\
diff --git a/with space.txt b/with space.txt\n\
old mode 100644\n\
new mode 100755\n\
diff --git a/gone.txt b/gone.txt\n\
deleted file mode 100644\n\
index 3333333..0000000\n\
--- a/gone.txt\n\
+++ /dev/null\n\
@@ -1,2 +0,0 @@\n\
-one\n\
-\n";
    let files = parse_diff(text);
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].path(), "spécial \"x\".txt");
    assert_eq!(files[0].hunks[0].lines.len(), 2);
    assert_eq!(files[1].path(), "with space.txt");
    assert_eq!(files[1].old_mode.as_deref(), Some("100644"));
    assert_eq!(files[1].new_mode.as_deref(), Some("100755"));
    assert!(files[1].hunks.is_empty());
    assert_eq!(files[2].change, FileChange::Deleted);
    assert_eq!(files[2].path(), "gone.txt");
    assert_eq!(files[2].removed(), 2);
    assert_eq!(files[2].hunks[0].lines[1].text, "");
}

#[test]
fn submodule_changes_are_flagged() {
    let sub = Repo::new();
    sub.write("s.txt", "s\n");
    sub.commit_all("sub root");

    let r = base_repo();
    let ts = r.next_ts();
    Repo::git_in(&r.path, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub.path.to_str().unwrap(), "libs/sub"], ts);
    r.commit_all("add submodule");

    let inner = r.path.join("libs/sub");
    Repo::git_in(&inner, &["config", "commit.gpgsign", "false"], ts);
    std::fs::write(inner.join("s.txt"), "changed\n").unwrap();
    Repo::git_in(&inner, &["commit", "-q", "-am", "bump"], ts);

    let st = repo::status(&r.path).unwrap();
    let e = st.entries.iter().find(|e| e.path == "libs/sub").expect("submodule entry");
    assert!(e.submodule);
    assert_eq!(e.worktree, 'M');
    let d = repo::file_diff(&r.path, "libs/sub", false).unwrap().unwrap();
    assert!(d.hunks[0].lines.iter().any(|l| l.text.starts_with("Subproject commit")));
}

#[test]
fn repo_summary_counts_changes() {
    let r = base_repo();
    r.write("a.txt", "x\n");
    r.write("new.txt", "n\n");
    let s = repo_summary(&r.path).unwrap();
    assert_eq!(s.branch, "main");
    assert_eq!(s.changed, 2);
    assert_eq!(s.conflicted, 0);
    assert_eq!((s.ahead, s.behind), (0, 0));
    assert!(s.pr.is_none(), "no GitHub remote → no PR");
}
