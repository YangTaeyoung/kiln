use kiln_git::history_guard::{self as guard, DefaultStatus, ReviewAction};
use std::{path::{Path,PathBuf},process::Command};
fn git(root:&Path,args:&[&str])->String {
    let output=Command::new("git").current_dir(root).args(["-c","commit.gpgsign=false","-c","user.name=Fixture","-c","user.email=fixture@example.invalid"]).args(args).env("GIT_TERMINAL_PROMPT","0").output().unwrap();
    assert!(output.status.success(),"{args:?}: {}",String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().into()
}
struct Fixture { _dir:tempfile::TempDir, repo:PathBuf, bare:PathBuf, shas:Vec<String> }
impl Fixture {
    fn new()->Self {
        let dir=tempfile::tempdir().unwrap();let repo=dir.path().join("work");let bare=dir.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        git(dir.path(),&["init","--bare","--initial-branch=main",bare.to_str().unwrap()]);
        git(&repo,&["init","--initial-branch=main"]);
        git(&repo,&["config","user.name","Fixture"]);git(&repo,&["config","user.email","fixture@example.invalid"]);git(&repo,&["config","commit.gpgsign","false"]);
        git(&repo,&["config","core.hooksPath",".git/hooks"]);
        std::fs::write(repo.join("base"),"base\n").unwrap();git(&repo,&["add","."]);git(&repo,&["commit","-m","base"]);
        git(&repo,&["remote","add","origin",bare.to_str().unwrap()]);git(&repo,&["push","-u","origin","main"]);
        git(&repo,&["switch","-c","feature"]);
        let mut shas=Vec::new();
        for name in ["a","b","c"] {std::fs::write(repo.join(name),name).unwrap();git(&repo,&["add",name]);git(&repo,&["commit","-m",name]);shas.push(git(&repo,&["rev-parse","HEAD"]));}
        git(&repo,&["push","-u","origin","feature"]);
        Self{_dir:dir,repo,bare,shas}
    }
    fn head(&self)->String {git(&self.repo,&["rev-parse","HEAD"])}
    fn remote_head(&self)->String {git(&self.bare,&["rev-parse","refs/heads/feature"])}
}
#[test]
fn squash_updates_exact_remote_and_keeps_recovery_ref() {
    let f=Fixture::new();let old=f.head();
    std::fs::write(f.repo.join(".git/FETCH_HEAD"),"preserve unrelated fetch state\n").unwrap();
    // Explicit refspec must not be affected by unrelated push configuration.
    git(&f.repo,&["config","push.default","matching"]);
    let review=guard::prepare(&f.repo,ReviewAction::Squash,&f.shas[..2]).unwrap();
    assert!(review.pushed);assert_eq!(review.affected_commits.len(),3);assert_eq!(review.default_status,DefaultStatus::NotMerged);
    let result=guard::execute(&f.repo,&review,"combined",true,false).unwrap();
    assert!(result.pushed);assert_ne!(f.head(),old);assert_eq!(f.head(),f.remote_head());
    assert_eq!(git(&f.repo,&["rev-parse",&result.backup_ref]),old);
    assert_eq!(git(&f.repo,&["log","--format=%s","main..HEAD"]),"c\ncombined");
    assert_eq!(std::fs::read_to_string(f.repo.join(".git/FETCH_HEAD")).unwrap(),"preserve unrelated fetch state\n");
    assert!(git(&f.repo,&["for-each-ref","--format=%(refname)","refs/kiln/review/"]).is_empty());
}
#[test]
fn dirty_detached_and_stale_head_or_branch_are_rejected() {
    let f=Fixture::new();
    std::fs::write(f.repo.join("untracked"),"keep").unwrap();
    assert!(guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).is_err());
    std::fs::remove_file(f.repo.join("untracked")).unwrap();
    let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    git(&f.repo,&["switch","-c","same-head-other-branch"]);
    assert!(guard::execute(&f.repo,&review,"",true,true).is_err());
    git(&f.repo,&["switch","feature"]);git(&f.repo,&["commit","--allow-empty","-m","new work"]);
    let changed=f.head();assert!(guard::execute(&f.repo,&review,"",true,true).is_err());assert_eq!(f.head(),changed);
    git(&f.repo,&["checkout","--detach"]);
    assert!(guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).is_err());
}
#[test]
fn changed_remote_lease_is_rejected_before_rewrite() {
    let f=Fixture::new();let old=f.head();let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    git(&f.bare,&["update-ref","refs/heads/feature",&f.shas[1]]);
    assert!(guard::execute(&f.repo,&review,"",true,true).is_err());
    assert_eq!(f.head(),old);assert_eq!(f.remote_head(),f.shas[1]);
}
#[test]
fn merged_and_unknown_default_require_explicit_acknowledgement() {
    let f=Fixture::new();git(&f.repo,&["push","origin","HEAD:refs/heads/main"]);
    let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    assert!(matches!(review.default_status,DefaultStatus::Merged{..}));
    let old=f.head();assert!(guard::execute(&f.repo,&review,"",false,false).is_err());assert_eq!(old,f.head());
    git(&f.bare,&["symbolic-ref","HEAD","refs/heads/missing"]);
    let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    assert!(matches!(review.default_status,DefaultStatus::Unknown(_)));
    assert!(guard::execute(&f.repo,&review,"",false,false).is_err());
    let result=guard::execute(&f.repo,&review,"",false,true).unwrap();assert!(!result.pushed);assert!(!f.repo.join("a").exists());
}
#[test]
fn cherry_pick_source_commits_and_reject_noncontiguous_squash() {
    let f=Fixture::new();
    assert!(guard::prepare(&f.repo,ReviewAction::Squash,&[f.shas[0].clone(),f.shas[2].clone()]).is_err());
    git(&f.repo,&["switch","main"]);
    let review=guard::prepare(&f.repo,ReviewAction::CherryPick,&[f.shas[1].clone(),f.shas[0].clone()]).unwrap();
    assert_eq!(review.commits[0].sha,f.shas[0]);
    let result=guard::execute(&f.repo,&review,"",false,false).unwrap();assert!(!result.pushed);
    assert!(f.repo.join("a").exists() && f.repo.join("b").exists());assert!(!f.repo.join("c").exists());
}
#[test]
fn merge_history_is_rejected_and_cherry_pick_conflict_keeps_recovery() {
    let f=Fixture::new();git(&f.repo,&["switch","-c","side","main"]);
    std::fs::write(f.repo.join("a"),"different").unwrap();git(&f.repo,&["add","a"]);git(&f.repo,&["commit","-m","conflicting"]);
    let review=guard::prepare(&f.repo,ReviewAction::CherryPick,&f.shas[..1]).unwrap();
    let result=guard::execute(&f.repo,&review,"",true,false).unwrap();
    assert!(matches!(result.rewrite.outcome,kiln_git::history::Outcome::Stopped(_)));assert!(!result.pushed);
    assert_eq!(git(&f.repo,&["rev-parse",&result.backup_ref]),review.old_head);
    assert!(guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).is_err());
    git(&f.repo,&["cherry-pick","--abort"]);
    git(&f.repo,&["switch","feature"]);git(&f.repo,&["merge","--no-ff","-s","ours","side","-m","merge"]);
    assert!(guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).is_err());
    let merge=f.head();assert!(guard::prepare(&f.repo,ReviewAction::CherryPick,&[merge]).is_err());
}
#[cfg(unix)]
#[test]
fn remote_race_during_push_preserves_remote_and_reports_local_success() {
    use std::os::unix::fs::PermissionsExt;
    let f=Fixture::new();let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    // Remote changes after execute's second fetch and before its push reaches
    // receive-pack. Explicit expected OID must reject this update.
    let hook=f.repo.join(".git/hooks/pre-push");
    std::fs::write(&hook,format!("#!/bin/sh\ngit --git-dir='{}' update-ref refs/heads/feature '{}'\n",f.bare.display(),f.shas[1])).unwrap();
    std::fs::set_permissions(&hook,std::fs::Permissions::from_mode(0o755)).unwrap();
    let result=guard::execute(&f.repo,&review,"",true,false).unwrap();
    assert!(!result.pushed);assert!(matches!(result.rewrite.outcome,kiln_git::history::Outcome::Done));
    assert!(result.notice.contains("로컬 이력 수정은 완료"));assert_eq!(f.remote_head(),f.shas[1]);assert_ne!(f.head(),review.old_head);
}

#[test]
fn published_branch_without_upstream_is_still_reviewed_and_pushed() {
    let f=Fixture::new();git(&f.repo,&["branch","--unset-upstream"]);
    let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    assert!(review.pushed);assert_eq!(review.remote.as_ref().unwrap().branch,"refs/heads/feature");
    let result=guard::execute(&f.repo,&review,"",true,false).unwrap();
    assert!(result.pushed);assert_eq!(f.head(),f.remote_head());
}

#[test]
fn changed_default_branch_requires_new_review_even_after_ack() {
    let f=Fixture::new();git(&f.repo,&["push","origin","HEAD:refs/heads/main"]);
    let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    assert!(matches!(review.default_status,DefaultStatus::Merged{..}));
    git(&f.bare,&["symbolic-ref","HEAD","refs/heads/feature"]);
    let old=f.head();assert!(guard::execute(&f.repo,&review,"",true,true).is_err());assert_eq!(old,f.head());
}
#[test]
fn concurrent_guarded_rewrite_is_rejected_without_changing_head() {
    let f=Fixture::new();let review=guard::prepare(&f.repo,ReviewAction::Drop,&f.shas[..1]).unwrap();
    std::fs::write(f.repo.join(".git/kiln-history-guard.lock"),"").unwrap();
    assert!(guard::execute(&f.repo,&review,"",true,true).is_err());assert_eq!(f.head(),review.old_head);
}
