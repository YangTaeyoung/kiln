//! Reviewed history changes. All network targets and leases are captured before
//! confirmation; no implicit push destination or remote-tracking-ref lease.
//! https://git-scm.com/docs/git-push#Documentation/git-push.txt---force-with-leaseltrefnamegtltexpectgt
use std::{collections::HashSet, path::{Path,PathBuf}};
use crate::{GitError, GitResult, cmd::{git, git_raw, git_combined, Mode}, history::{self, Rewrite, Outcome}, repo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAction { Squash, Drop, CherryPick }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewCommit { pub sha: String, pub subject: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseTarget {
    /// The single push URL reviewed by the user, never an implicit remote name.
    pub remote: String,
    /// Fully qualified destination, e.g. refs/heads/feature.
    pub branch: String,
    pub expected_oid: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultStatus { Unknown(String), NotMerged, Merged { branch: String }, DefaultBranch { branch: String } }
#[derive(Clone, Debug)]
pub struct HistoryReview {
    pub action: ReviewAction,
    pub branch: String,
    pub old_head: String,
    pub commits: Vec<ReviewCommit>,
    pub affected_commits: Vec<ReviewCommit>,
    pub message: String,
    pub remote: Option<LeaseTarget>,
    pub default_status: DefaultStatus,
    pub pushed: bool,
}
#[derive(Clone, Debug)]
pub struct HistoryResult { pub rewrite: Rewrite, pub notice: String, pub pushed: bool, pub backup_ref: String }
fn failure(message: impl Into<String>) -> GitError { GitError::Failed(message.into()) }
fn resolve(root: &Path, rev: &str) -> GitResult<String> {
    Ok(git(root, Mode::Read, &["rev-parse", "--verify", "--end-of-options", &format!("{rev}^{{commit}}")])?.trim().into())
}
fn clean_branch(root: &Path) -> GitResult<(String,String)> {
    if let Some(op)=repo::in_progress_op(root) { return Err(failure(format!("{} 작업을 먼저 완료하거나 중단하세요",op.label()))); }
    if !git(root,Mode::Read,&["status","--porcelain=v1","--untracked-files=all","--ignore-submodules=none"])?.trim().is_empty() {
        return Err(failure("변경 파일을 커밋하거나 스태시한 뒤 이력을 수정하세요"));
    }
    let branch=git(root,Mode::Read,&["symbolic-ref","--quiet","--short","HEAD"])
        .map_err(|_|failure("브랜치에 체크아웃한 뒤 이력을 수정하세요 (detached HEAD)"))?.trim().to_string();
    Ok((branch,resolve(root,"HEAD")?))
}
fn ancestor(root:&Path,a:&str,b:&str)->GitResult<bool> {
    let out=git_raw(root,Mode::Read,&["merge-base","--is-ancestor",a,b],None)?;
    if out.success { Ok(true) } else if out.stderr.trim().is_empty() { Ok(false) } else { Err(failure(out.stderr)) }
}
fn commit(root:&Path,sha:String)->GitResult<ReviewCommit> {
    let subject=git(root,Mode::Read,&["show","-s","--format=%s",&sha])?.trim_end().to_string();
    Ok(ReviewCommit{sha,subject})
}
fn push_url(root:&Path,remote:&str)->GitResult<String> {
    if remote=="." || remote.starts_with('-') { return Err(failure("로컬 또는 모호한 원격 설정에서는 자동 푸시할 수 없습니다")); }
    let urls=git(root,Mode::Read,&["remote","get-url","--push","--all",remote])?;
    let urls:Vec<_>=urls.lines().filter(|s|!s.is_empty()).collect();
    if urls.len()!=1 || urls[0].starts_with('-') { return Err(failure("푸시 대상 URL이 하나인 원격 저장소를 설정하세요")); }
    Ok(urls[0].to_string())
}
fn unique_stamp()->String {
    format!("{}-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),std::process::id())
}
fn fetch_tip(root:&Path,url:&str,branch:&str)->GitResult<String> {
    if !branch.starts_with("refs/heads/") { return Err(failure("원격 브랜치 경로를 확인할 수 없습니다")); }
    // An unrelated background fetch can replace FETCH_HEAD. Use a private ref
    // for this snapshot and do not alter users' tracking refs or FETCH_HEAD.
    let snapshot=format!("refs/kiln/review/{}",unique_stamp());
    let refspec=format!("+{branch}:{snapshot}");
    let fetched=git(root,Mode::Write,&["fetch","--no-tags","--no-recurse-submodules","--no-write-fetch-head","--",url,&refspec]);
    let result=fetched.and_then(|_|resolve(root,&snapshot));
    let _=git(root,Mode::Write,&["update-ref","-d",&snapshot]);
    result
}
struct RewriteLock(PathBuf);
impl Drop for RewriteLock { fn drop(&mut self) { let _=std::fs::remove_file(&self.0); } }
fn lock_rewrite(root:&Path)->GitResult<RewriteLock> {
    let path=git(root,Mode::Read,&["rev-parse","--path-format=absolute","--git-path","kiln-history-guard.lock"])?;
    let path=PathBuf::from(path.trim());
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|_|failure("다른 이력 수정 작업이 진행 중입니다. 완료 후 다시 시도하세요"))?;
    Ok(RewriteLock(path))
}
fn default_status(root:&Path,url:&str,branch:&str,affected:&[ReviewCommit])->DefaultStatus {
    let inspect=||->GitResult<DefaultStatus>{
        let output=git(root,Mode::Read,&["ls-remote","--symref","--",url,"HEAD"])?;
        let default=output.lines().find_map(|line|line.strip_prefix("ref: ").and_then(|s|s.split_once('\t')).filter(|(_,r)|*r=="HEAD").map(|(r,_)|r.to_owned()))
            .ok_or_else(||failure("원격 기본 브랜치를 확인할 수 없습니다"))?;
        let tip=fetch_tip(root,url,&default)?;
        let name=default.strip_prefix("refs/heads/").unwrap_or(&default).to_owned();
        if default==format!("refs/heads/{branch}") { return Ok(DefaultStatus::DefaultBranch{branch:name}); }
        for c in affected { if ancestor(root,&c.sha,&tip)? { return Ok(DefaultStatus::Merged{branch:name}); } }
        Ok(DefaultStatus::NotMerged)
    };
    inspect().unwrap_or_else(|e|DefaultStatus::Unknown(e.to_string()))
}

pub fn prepare(root:&Path,action:ReviewAction,shas:&[String])->GitResult<HistoryReview> {
    let (branch,old_head)=clean_branch(root)?;
    if shas.is_empty() { return Err(failure("커밋을 선택하세요")); }
    let mut selected=HashSet::new();
    for sha in shas { selected.insert(resolve(root,sha)?); }
    let (commits,affected_commits)=if action==ReviewAction::CherryPick {
        let mut args=vec!["rev-list","--topo-order","--reverse"];
        let seeds:Vec<_>=selected.iter().map(String::as_str).collect();
        args.extend(seeds);args.push("--");
        let mut commits=Vec::new();
        for sha in git(root,Mode::Read,&args)?.lines().filter(|s|selected.contains(*s)) {
            if git(root,Mode::Read,&["rev-list","--parents","-n1",sha])?.split_whitespace().count()>2 {
                return Err(failure("병합 커밋은 메인라인 선택이 필요하므로 이 작업에서 체리픽할 수 없습니다"));
            }
            commits.push(commit(root,sha.into())?);
        }
        (commits,Vec::new())
    } else {
        let chain=git(root,Mode::Read,&["rev-list","HEAD"])?;
        let oldest=chain.lines().filter(|s|selected.contains(*s)).last().ok_or_else(||failure("현재 브랜치의 커밋을 선택하세요"))?;
        let plan=history::rebase_plan(root,oldest)?;
        let affected:Vec<_>=plan.steps.into_iter().map(|c|ReviewCommit{sha:c.sha,subject:c.subject}).collect();
        let commits:Vec<_>=affected.iter().filter(|c|selected.contains(&c.sha)).cloned().collect();
        if commits.len()!=selected.len() { return Err(failure("선택한 커밋이 현재 브랜치의 선형 이력에 없습니다")); }
        if action==ReviewAction::Squash {
            if commits.len()<2 { return Err(failure("합칠 커밋을 두 개 이상 선택하세요")); }
            if affected.iter().take(commits.len()).any(|c|!selected.contains(&c.sha)) { return Err(failure("연속된 커밋만 합칠 수 있습니다")); }
        }
        if action==ReviewAction::Drop && affected.len()==selected.len() && history::rebase_plan(root,oldest)?.base.is_none() {
            return Err(failure("브랜치의 모든 커밋을 제거할 수 없습니다"));
        }
        (commits,affected)
    };
    let full_branch=format!("refs/heads/{branch}");
    let tracking=git(root,Mode::Read,&["for-each-ref","--format=%(upstream:remotename)%09%(upstream:remoteref)",&full_branch])?;
    let (remote_name,remote_ref)=tracking.trim_end().split_once('\t').unwrap_or(("",""));
    let mut remote=None;
    let mut pushed=false;
    let default_status=if !remote_name.is_empty() && !remote_ref.is_empty() {
        let url=push_url(root,remote_name)?;
        let expected_oid=fetch_tip(root,&url,remote_ref)?;
        if action!=ReviewAction::CherryPick {
            if !ancestor(root,&expected_oid,&old_head)? { return Err(failure("원격 브랜치에 아직 반영하지 않은 변경이 있습니다. 먼저 동기화하세요")); }
            for c in &affected_commits { if ancestor(root,&c.sha,&expected_oid)? { pushed=true;break; } }
        }
        let status=default_status(root,&url,&branch,&affected_commits);
        remote=Some(LeaseTarget{remote:url,branch:remote_ref.into(),expected_oid});
        status
    } else {
        let remotes=git(root,Mode::Read,&["remote"])?;
        let names:Vec<_>=remotes.lines().collect();
        let name=if names.contains(&"origin") {Some("origin")}else if names.len()==1 {Some(names[0])}else{None};
        match name.map(|n|push_url(root,n)) {
            Some(Ok(url))=>{
                // `git push origin feature` need not set an upstream. Detect the
                // exact same-named branch rather than treating published history
                // as local just because tracking configuration is absent.
                let refs=git(root,Mode::Read,&["ls-remote","--heads","--",&url,&full_branch])?;
                if refs.lines().any(|line|line.split_once('\t').is_some_and(|(_,r)|r==full_branch)) {
                    let expected_oid=fetch_tip(root,&url,&full_branch)?;
                    if action!=ReviewAction::CherryPick {
                        if !ancestor(root,&expected_oid,&old_head)? { return Err(failure("원격 브랜치와 먼저 동기화하세요")); }
                        for c in &affected_commits { if ancestor(root,&c.sha,&expected_oid)? {pushed=true;break;} }
                    }
                    remote=Some(LeaseTarget{remote:url.clone(),branch:full_branch.clone(),expected_oid});
                }
                default_status(root,&url,&branch,&affected_commits)
            },
            Some(Err(e))=>DefaultStatus::Unknown(e.to_string()),
            None=>DefaultStatus::Unknown("원격 기본 브랜치를 확인할 연결이 없습니다".into()),
        }
    };
    // A fetch/hook/background operation must not silently change the review base.
    if clean_branch(root)?!=(branch.clone(),old_head.clone()) { return Err(failure("검토 중 HEAD가 변경되었습니다. 다시 검토하세요")); }
    let message=commits.iter().map(|c|c.subject.as_str()).collect::<Vec<_>>().join("\n\n");
    Ok(HistoryReview{action,branch,old_head,commits,affected_commits,message,remote,default_status,pushed})
}

pub fn execute(root:&Path,review:&HistoryReview,message:&str,auto_push:bool,ack_risk:bool)->GitResult<HistoryResult> {
    let _lock=lock_rewrite(root)?;
    if clean_branch(root)?!=(review.branch.clone(),review.old_head.clone()) { return Err(failure("검토한 뒤 브랜치 또는 HEAD가 변경되었습니다. 다시 검토하세요")); }
    let shas:Vec<_>=review.commits.iter().map(|c|c.sha.clone()).collect();
    let fresh=prepare(root,review.action,&shas)?;
    if fresh.old_head!=review.old_head || fresh.branch!=review.branch || fresh.remote!=review.remote || fresh.pushed!=review.pushed || fresh.default_status!=review.default_status || fresh.affected_commits!=review.affected_commits {
        return Err(failure("검토한 원격 또는 커밋 범위가 변경되었습니다. 다시 검토하세요"));
    }
    if review.action!=ReviewAction::CherryPick && !matches!(fresh.default_status,DefaultStatus::NotMerged) && !ack_risk {
        return Err(failure("기본 브랜치 반영 여부와 이력 변경 위험을 확인하세요"));
    }
    if review.action==ReviewAction::Squash && message.trim().is_empty() { return Err(failure("커밋 메시지를 입력하세요")); }
    let backup_ref=format!("refs/kiln/backup/{}",unique_stamp());
    git(root,Mode::Write,&["update-ref",&backup_ref,&review.old_head,""])?;
    // Preserve the original ref on every result, including conflicts and errors.
    if clean_branch(root)?!=(review.branch.clone(),review.old_head.clone()) { return Err(failure(format!("HEAD가 변경되었습니다. 원본 보관: {backup_ref}"))); }
    let result=match review.action {
        ReviewAction::Squash=>history::squash_commits(root,&shas,message,false),
        ReviewAction::Drop=>history::drop_commits(root,&shas,false),
        ReviewAction::CherryPick=>history::cherry_pick(root,&shas),
    };
    let rewrite=result.map_err(|e|failure(format!("{e}\n원본 보관: {backup_ref}")))?;
    let mut notice=format!("원본 보관: {backup_ref}");
    if review.pushed && !auto_push && matches!(rewrite.outcome,Outcome::Done) {
        notice=format!("로컬 이력만 수정했습니다. 원격 브랜치는 변경하지 않았습니다.\n{notice}");
    }
    let mut pushed=false;
    if auto_push && review.pushed && review.action!=ReviewAction::CherryPick && matches!(rewrite.outcome,Outcome::Done) {
        let target=review.remote.as_ref().ok_or_else(||failure("검토한 푸시 대상이 없습니다"))?;
        let lease=format!("--force-with-lease={}:{}",target.branch,target.expected_oid);
        let refspec=format!("{}:{}",rewrite.new_head,target.branch);
        // Push exactly the rewritten object. Background fetches cannot weaken
        // the lease, and a different current HEAD cannot change this payload.
        match git_combined(root,&["-c","push.followTags=false","push","--no-follow-tags",&lease,"--",&target.remote,&refspec]) {
            Ok(_)=>{pushed=true;notice=format!("원격 브랜치 반영 완료\n{notice}");}
            Err(e)=>{notice=format!("로컬 이력 수정은 완료했지만 원격 반영에 실패했습니다. 원격 상태를 다시 검토하세요.\n{e}\n{notice}");}
        }
    }
    Ok(HistoryResult{rewrite,notice,pushed,backup_ref})
}
