//! 이력(Log) 백엔드: 실제 임시 저장소로 재작성·체리픽·리셋·페이징을 검증한다.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

use common::Repo;
use kiln_git::history::{
    self, BranchFilter, DropPlace, GraphBuilder, LogPager, LogQuery, Outcome, RebaseAction, RefKind, ResetMode,
};
use kiln_git::repo::RepoOp;

/// 파일 하나씩을 더하는 선형 커밋 `names` 를 만든다. 커밋 해시를 순서대로 돌려준다.
fn linear(r: &Repo, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|n| {
            r.write(&format!("{n}.txt"), &format!("{n}\n"));
            r.commit_all(&format!("Add {n}"))
        })
        .collect()
}

fn subjects(r: &Repo) -> Vec<String> {
    r.git(&["log", "--format=%s"]).lines().map(str::to_string).collect()
}

fn head(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD"]).trim().to_string()
}

fn exists(r: &Repo, rel: &str) -> bool {
    r.path.join(rel).exists()
}

#[test]
fn squash_middle_range_keeps_changes_and_uses_message() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d", "e"]);
    let rw = history::squash_commits(&r.path, &[c[1].clone(), c[2].clone(), c[3].clone()], "Add b, c and d\n\nSquashed.", false).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert_eq!(rw.old_head, c[4]);
    assert_eq!(subjects(&r), ["Add e", "Add b, c and d", "Add a"]);
    assert_eq!(r.git(&["log", "-1", "--skip=1", "--format=%B"]).trim(), "Add b, c and d\n\nSquashed.");
    for f in ["a", "b", "c", "d", "e"] {
        assert!(exists(&r, &format!("{f}.txt")), "{f}");
    }
    // 루트 커밋은 그대로다.
    assert_eq!(r.git(&["rev-list", "--max-parents=0", "HEAD"]).trim(), c[0]);
}

#[test]
fn squash_rejects_non_contiguous_selection() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d"]);
    let e = history::squash_commits(&r.path, &[c[1].clone(), c[3].clone()], "x", false).unwrap_err();
    assert!(e.to_string().contains("연속"), "{e}");
    assert_eq!(head(&r), c[3]);
}

#[test]
fn drop_commit_removes_its_changes() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d"]);
    let rw = history::drop_commits(&r.path, &[c[2].clone()], false).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert_eq!(subjects(&r), ["Add d", "Add b", "Add a"]);
    assert!(!exists(&r, "c.txt"));
    assert!(exists(&r, "d.txt"));
}

#[test]
fn drop_root_commit_uses_root_rebase() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c"]);
    history::drop_commits(&r.path, &[c[0].clone()], false).unwrap();
    assert_eq!(subjects(&r), ["Add c", "Add b"]);
    assert!(!exists(&r, "a.txt"));
}

#[test]
fn reorder_moves_commit_below_target() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d", "e"]);
    // e 를 b 아래(더 오래된 쪽)로.
    history::move_commits(&r.path, &[c[4].clone()], &c[1], DropPlace::Below, false).unwrap();
    assert_eq!(subjects(&r), ["Add d", "Add c", "Add b", "Add e", "Add a"]);
    // 여러 개를 대상 위로.
    let now: Vec<String> = r.git(&["log", "--format=%H"]).lines().map(str::to_string).collect();
    // now: d c b e a → e 를 c 위로
    history::move_commits(&r.path, &[now[3].clone()], &now[1], DropPlace::Above, false).unwrap();
    assert_eq!(subjects(&r), ["Add d", "Add e", "Add c", "Add b", "Add a"]);
    // 루트를 포함한 여러 커밋을 한꺼번에 옮긴다: a, b 를 d 위로.
    let now: Vec<String> = r.git(&["log", "--format=%H"]).lines().map(str::to_string).collect();
    history::move_commits(&r.path, &[now[3].clone(), now[4].clone()], &now[0], DropPlace::Above, false).unwrap();
    assert_eq!(subjects(&r), ["Add b", "Add a", "Add d", "Add e", "Add c"]);
}

#[test]
fn reword_old_commit_and_head() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c"]);
    let rw = history::reword_commit(&r.path, &c[1], "Better b\n\nWith body", false).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert_eq!(subjects(&r), ["Add c", "Better b", "Add a"]);
    assert_eq!(r.git(&["log", "-1", "--skip=1", "--format=%b"]).trim(), "With body");

    // HEAD 는 amend 로. 스테이지된 변경은 커밋에 들어가지 않는다.
    r.write("staged.txt", "s\n");
    r.git(&["add", "staged.txt"]);
    history::reword_commit(&r.path, "HEAD", "Newest", false).unwrap();
    assert_eq!(subjects(&r)[0], "Newest");
    assert!(r.git(&["show", "--name-only", "--format=", "HEAD"]).lines().all(|l| l != "staged.txt"));
    assert!(r.git(&["diff", "--cached", "--name-only"]).contains("staged.txt"));
}

#[test]
fn fixup_via_drag_backend_merges_into_target() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d"]);
    history::fixup_into(&r.path, &[c[3].clone()], &c[1], false).unwrap();
    assert_eq!(subjects(&r), ["Add c", "Add b", "Add a"]);
    let files = r.git(&["show", "--name-only", "--format=", "HEAD~1"]);
    assert!(files.contains("b.txt") && files.contains("d.txt"), "{files}");
}

#[test]
fn revert_and_cherry_pick_in_order() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b"]);
    let rw = history::revert(&r.path, &c[1]).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert!(!exists(&r, "b.txt"));
    assert!(subjects(&r)[0].starts_with("Revert \"Add b\""));

    r.git(&["switch", "-q", "-c", "side", &c[0]]);
    let side = linear(&r, &["x", "y", "z"]);
    r.git(&["switch", "-q", "main"]);
    let rw = history::cherry_pick(&r.path, &[side[0].clone(), side[2].clone()]).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert_eq!(&subjects(&r)[..2], ["Add z", "Add x"]);
    assert!(exists(&r, "x.txt") && exists(&r, "z.txt") && !exists(&r, "y.txt"));
}

#[test]
fn reset_soft_mixed_hard() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c"]);
    let rw = history::reset(&r.path, &c[1], ResetMode::Soft).unwrap();
    assert_eq!(rw.old_head, c[2]);
    assert_eq!(head(&r), c[1]);
    assert!(r.git(&["diff", "--cached", "--name-only"]).contains("c.txt"));

    // 스테이지 내용이 되돌릴 커밋과 같으면 --keep 으로 되돌릴 수 있다.
    history::undo(&r.path, &c[2]).unwrap();
    assert_eq!(head(&r), c[2]);
    assert!(r.git(&["status", "--porcelain"]).trim().is_empty());

    history::reset(&r.path, &c[1], ResetMode::Mixed).unwrap();
    assert!(r.git(&["diff", "--cached", "--name-only"]).trim().is_empty());
    assert!(r.git(&["status", "--porcelain"]).contains("?? c.txt"));
    std::fs::remove_file(r.path.join("c.txt")).unwrap();

    r.git(&["reset", "-q", "--hard", &c[2]]);
    r.write("a.txt", "dirty\n");
    history::reset(&r.path, &c[0], ResetMode::Hard).unwrap();
    assert_eq!(head(&r), c[0]);
    assert_eq!(r.read("a.txt"), "a\n");
    assert!(!exists(&r, "b.txt"));

    // 되돌리기: 이전 HEAD 로.
    history::undo(&r.path, &c[2]).unwrap();
    assert_eq!(head(&r), c[2]);
}

/// 같은 줄을 차례로 고치는 커밋들.
fn conflicting(r: &Repo) -> Vec<String> {
    r.write("f.txt", "base\n");
    let a = r.commit_all("base");
    r.write("f.txt", "one\n");
    let b = r.commit_all("one");
    r.write("f.txt", "two\n");
    let c = r.commit_all("two");
    vec![a, b, c]
}

#[test]
fn conflict_during_reorder_then_abort_restores_original() {
    let r = Repo::new();
    let c = conflicting(&r);
    let rw = history::move_commits(&r.path, &[c[2].clone()], &c[1], DropPlace::Below, false).unwrap();
    let Outcome::Stopped(st) = &rw.outcome else { panic!("expected conflict: {rw:?}") };
    assert_eq!(st.op, RepoOp::Rebase);
    assert_eq!(st.conflicts, ["f.txt"]);
    let state = history::repo_state(&r.path).unwrap();
    assert_eq!(state.op, Some(RepoOp::Rebase));
    assert_eq!(state.conflicts, ["f.txt"]);
    // 해결하지 않고 계속하면 거부한다.
    assert!(history::continue_op(&r.path).is_err());

    history::abort_op(&r.path).unwrap();
    assert_eq!(head(&r), c[2]);
    assert_eq!(r.read("f.txt"), "two\n");
    assert_eq!(history::repo_state(&r.path).unwrap().op, None);
}

#[test]
fn conflict_resolved_then_continue_and_undo() {
    let r = Repo::new();
    let c = conflicting(&r);
    let rw = history::move_commits(&r.path, &[c[2].clone()], &c[1], DropPlace::Below, false).unwrap();
    assert!(matches!(rw.outcome, Outcome::Stopped(_)));
    r.write("f.txt", "two\n");
    r.git(&["add", "f.txt"]);
    let mut rw2 = history::continue_op(&r.path).unwrap();
    // 다음 커밋("one")도 충돌한다.
    while let Outcome::Stopped(_) = rw2.outcome {
        r.write("f.txt", "one\n");
        r.git(&["add", "f.txt"]);
        rw2 = history::continue_op(&r.path).unwrap();
    }
    assert_eq!(subjects(&r), ["one", "two", "base"]);
    history::undo(&r.path, &rw.old_head).unwrap();
    assert_eq!(head(&r), c[2]);
}

#[test]
fn undo_during_stopped_rebase_aborts_first() {
    let r = Repo::new();
    let c = conflicting(&r);
    let rw = history::move_commits(&r.path, &[c[2].clone()], &c[1], DropPlace::Below, false).unwrap();
    assert!(matches!(rw.outcome, Outcome::Stopped(_)));
    history::undo(&r.path, &rw.old_head).unwrap();
    assert_eq!(head(&r), c[2]);
    assert_eq!(history::repo_state(&r.path).unwrap().op, None);
}

#[test]
fn interactive_plan_with_edit_stops_and_continues() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c", "d"]);
    let mut plan = history::rebase_plan(&r.path, &c[1]).unwrap();
    assert_eq!(plan.base.as_deref(), Some(c[0].as_str()));
    assert_eq!(plan.steps.iter().map(|s| s.subject.as_str()).collect::<Vec<_>>(), ["Add b", "Add c", "Add d"]);
    plan.steps[0].action = RebaseAction::Edit;
    plan.steps[2].action = RebaseAction::Squash;
    plan.steps[2].message = "Add c and d".into();
    plan.steps.swap(0, 1); // c, b(edit), d(squash → b)
    let prev: Vec<_> = plan.preview().into_iter().map(|p| p.2).collect();
    assert_eq!(prev, ["Add c", "Add c and d"]);

    let rw = history::run_rebase(&r.path, &plan, false).unwrap();
    let Outcome::Stopped(st) = rw.outcome else { panic!("edit should stop") };
    assert!(st.conflicts.is_empty());
    let rw = history::continue_op(&r.path).unwrap();
    assert_eq!(rw.outcome, Outcome::Done);
    assert_eq!(subjects(&r), ["Add c and d", "Add c", "Add a"]);
    let files = r.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("b.txt") && files.contains("d.txt"));
}

#[test]
fn plan_validation_and_merge_rejection() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b"]);
    let mut plan = history::rebase_plan(&r.path, &c[1]).unwrap();
    plan.steps[0].action = RebaseAction::Fixup;
    assert!(plan.validate().is_err());
    assert!(history::run_rebase(&r.path, &plan, false).is_err());

    r.git(&["switch", "-q", "-c", "side"]);
    linear(&r, &["s"]);
    r.git(&["switch", "-q", "main"]);
    linear(&r, &["m"]);
    r.git(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
    let e = history::rebase_plan(&r.path, &c[1]).unwrap_err();
    assert!(e.to_string().contains("병합"), "{e}");
}

#[test]
fn autostash_keeps_dirty_changes() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b", "c"]);
    r.write("a.txt", "local edit\n");
    assert!(history::repo_state(&r.path).unwrap().dirty);
    assert!(history::drop_commits(&r.path, &[c[1].clone()], false).is_err());
    history::drop_commits(&r.path, &[c[1].clone()], true).unwrap();
    assert_eq!(subjects(&r), ["Add c", "Add a"]);
    assert_eq!(r.read("a.txt"), "local edit\n");
}

#[test]
fn pushed_commit_detection() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b"]);
    let remote = r.tempdir().join("remote.git");
    Repo::git_in(r.tempdir(), &["init", "-q", "--bare", "remote.git"], 0);
    r.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    let st = history::repo_state(&r.path).unwrap();
    assert!(!st.has_remote_refs);
    assert!(!history::is_pushed(&r.path, &c[1]).unwrap());

    r.git(&["push", "-q", "-u", "origin", "main"]);
    let d = linear(&r, &["c"]);
    assert!(history::is_pushed(&r.path, &c[1]).unwrap());
    assert!(!history::is_pushed(&r.path, &d[0]).unwrap());
    let st = history::repo_state(&r.path).unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert!(st.is_pushed(&c[0]) && st.is_pushed(&c[1]));
    assert!(!st.is_pushed(&d[0]));

    // 재작성 후 강제 푸시.
    history::reword_commit(&r.path, &c[1], "Reworded b", false).unwrap();
    assert!(!r.git_status(&["push", "-q"]));
    history::force_push(&r.path).unwrap();
    assert_eq!(r.git(&["rev-parse", "origin/main"]).trim(), head(&r));
}

#[test]
fn branch_tag_checkout_and_commit_info() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b"]);
    r.write("a.txt", &common::numbered(20));
    let c = [c[0].clone(), r.commit_all("Grow a")];
    r.write("a.txt", &common::numbered(21));
    std::fs::remove_file(r.path.join("b.txt")).unwrap();
    r.git(&["mv", "a.txt", "renamed.txt"]);
    let d = r.commit_all("Rename and delete\n\nBody line");
    history::create_branch_at(&r.path, "feature/x", &c[0], false).unwrap();
    history::create_tag(&r.path, "v1.0", &c[1], "").unwrap();
    history::create_tag(&r.path, "v1.1", &d, "Release 1.1").unwrap();
    assert!(history::create_branch_at(&r.path, "bad..name", &c[0], false).is_err());

    let info = history::commit_info(&r.path, &d).unwrap();
    assert_eq!(info.message, "Rename and delete\n\nBody line");
    assert_eq!(info.parents, [c[1].clone()]);
    let mut files: Vec<(char, &str)> = info.files.iter().map(|f| (f.status, f.path.as_str())).collect();
    files.sort();
    assert_eq!(files, [('D', "b.txt"), ('R', "renamed.txt")]);
    let ren = info.files.iter().find(|f| f.status == 'R').unwrap();
    assert_eq!(ren.old_path.as_deref(), Some("a.txt"));
    assert_eq!((ren.added, ren.removed), (Some(1), Some(0)));
    assert!(info.refs.iter().any(|x| x.kind == RefKind::LocalBranch && x.name == "main" && x.current));
    assert!(info.refs.iter().any(|x| x.kind == RefKind::Tag && x.name == "v1.1"));

    let root_info = history::commit_info(&r.path, &c[0]).unwrap();
    assert_eq!(root_info.base_rev(), history::EMPTY_TREE);
    assert_eq!(root_info.files.len(), 1);

    history::checkout(&r.path, &c[0]).unwrap();
    let st = history::repo_state(&r.path).unwrap();
    assert_eq!(st.branch, None);
    history::checkout(&r.path, "feature/x").unwrap();
    assert_eq!(history::repo_state(&r.path).unwrap().branch.as_deref(), Some("feature/x"));
}

#[test]
fn log_pager_filters_and_refs() {
    let r = Repo::new();
    let c = linear(&r, &["a", "b"]);
    r.git(&["tag", "v1"]);
    r.git(&["switch", "-q", "-c", "topic", &c[0]]);
    r.write("src/t.rs", "t\n");
    let t = r.commit_all("Topic work");
    r.git(&["switch", "-q", "main"]);
    let m = r.commit_all_merge("topic");

    let all = collect(&r, &LogQuery::default(), 2);
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].sha, m);
    let head_refs = &all[0].refs;
    assert!(head_refs[0].current && head_refs[0].name == "main");
    let b = all.iter().find(|x| x.sha == c[1]).unwrap();
    assert!(b.refs.iter().any(|x| x.kind == RefKind::Tag && x.name == "v1"));
    let tp = all.iter().find(|x| x.sha == t).unwrap();
    assert!(tp.refs.iter().any(|x| x.kind == RefKind::LocalBranch && x.name == "topic" && !x.current));

    let fp_head = collect(&r, &LogQuery { first_parent: true, branch: BranchFilter::Current, ..Default::default() }, 100);
    assert_eq!(fp_head.len(), 3);

    let path = collect(&r, &LogQuery { path: "src".into(), ..Default::default() }, 100);
    assert_eq!(path.iter().map(|x| x.sha.as_str()).collect::<Vec<_>>(), [t.as_str()]);
    let text = collect(&r, &LogQuery { text: "ADD B".into(), ..Default::default() }, 100);
    assert_eq!(text.len(), 1);
    let hash = collect(&r, &LogQuery { text: t[..7].to_string(), ..Default::default() }, 100);
    assert_eq!(hash[0].sha, t);
    let author = collect(&r, &LogQuery { author: "nobody".into(), ..Default::default() }, 100);
    assert!(author.is_empty());
    let named = collect(&r, &LogQuery { branch: BranchFilter::Named("topic".into()), ..Default::default() }, 100);
    assert_eq!(named.len(), 2);

    let empty = Repo::new();
    assert!(collect(&empty, &LogQuery::default(), 10).is_empty());
}

fn collect(r: &Repo, q: &LogQuery, page: usize) -> Vec<history::LogCommit> {
    let mut p = LogPager::start(&r.path, q, page);
    let mut v = Vec::new();
    while let Some(b) = p.next_blocking() {
        let b = b.expect("page");
        assert!(b.len() <= page);
        v.extend(b);
    }
    v
}

trait MergeExt {
    fn commit_all_merge(&self, branch: &str) -> String;
}

impl MergeExt for Repo {
    fn commit_all_merge(&self, branch: &str) -> String {
        let ts = self.next_ts();
        Repo::git_in(&self.path, &["merge", "-q", "--no-ff", "-m", &format!("Merge {branch}"), branch], ts);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }
}

#[test]
fn graph_builder_lanes_for_merge() {
    // m(merge of b, t) → b → a ; t → a
    let s = |x: &str| x.to_string();
    let mut g = GraphBuilder::new();
    let r0 = g.push("m", &[s("b"), s("t")]);
    assert_eq!((r0.col, r0.up.len(), r0.down.len()), (0, 0, 2));
    assert_eq!(r0.down[1].to, 1);
    let r1 = g.push("b", &[s("a")]);
    assert_eq!(r1.col, 0);
    assert_eq!(r1.pass.iter().map(|e| e.from).collect::<Vec<_>>(), [1]);
    let r2 = g.push("t", &[s("a")]);
    assert_eq!(r2.col, 1);
    let r3 = g.push("a", &[]);
    assert_eq!(r3.col, 0);
    assert_eq!(r3.up.len(), 2);
    assert!(r3.down.is_empty());
    // 병합으로 생긴 레인은 다른 색이다.
    assert_ne!(r0.down[0].color, r0.down[1].color);
}

/// fast-import 로 `n` 개 선형 커밋(+ 주기적인 병합)을 만든다.
fn fast_import(r: &Repo, n: usize) {
    let mut s = String::with_capacity(n * 200);
    for i in 1..=n {
        let ts = 1_700_000_000 + i as i64 * 60;
        let msg = format!("Commit number {i}");
        s.push_str(&format!("commit refs/heads/main\nmark :{i}\ncommitter Kiln Tester <tester@kiln.dev> {ts} +0000\n"));
        s.push_str(&format!("data {}\n{msg}\n", msg.len()));
        if i > 1 {
            s.push_str(&format!("from :{}\n", i - 1));
        }
        if i > 10 && i % 500 == 0 {
            s.push_str(&format!("merge :{}\n", i - 7));
        }
        let body = format!("{i}\n");
        s.push_str(&format!("M 644 inline f{}.txt\ndata {}\n{body}\n", i % 50, body.len()));
    }
    s.push_str("done\n");
    let mut child = Command::new("git")
        .current_dir(&r.path)
        .args(["fast-import", "--quiet", "--done"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    r.git(&["reset", "-q", "--hard", "main"]);
}

#[test]
fn paging_50k_commits_is_fast() {
    let r = Repo::new();
    fast_import(&r, 50_000);

    let t0 = Instant::now();
    let mut p = LogPager::start(&r.path, &LogQuery::default(), 500);
    let first = p.next_blocking().unwrap().unwrap();
    let first_page = t0.elapsed();
    assert_eq!(first.len(), 500);
    assert_eq!(first[0].subject, "Commit number 50000");

    let mut g = GraphBuilder::new();
    let mut total = 0;
    let mut max_width = 0;
    for c in &first {
        max_width = max_width.max(g.push(&c.sha, &c.parents).width);
    }
    total += first.len();
    while let Some(b) = p.next_blocking() {
        let b = b.unwrap();
        for c in &b {
            max_width = max_width.max(g.push(&c.sha, &c.parents).width);
        }
        total += b.len();
    }
    let all = t0.elapsed();
    assert_eq!(total, 50_000);
    assert!(max_width <= 3, "lanes: {max_width}");
    eprintln!("50k paging: first page {first_page:?}, all {all:?}");
    assert!(first_page.as_secs_f64() < 2.0, "first page took {first_page:?}");
    assert!(all.as_secs_f64() < 8.0, "full paging took {all:?}");

    // 중간에 버려도 git 프로세스가 정리된다.
    let mut p = LogPager::start(&r.path, &LogQuery::default(), 100);
    assert_eq!(p.next_blocking().unwrap().unwrap().len(), 100);
    drop(p);
}
