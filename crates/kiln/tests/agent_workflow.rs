//! A deterministic CLI probe exercises the real daemon/PTY without calling an AI service.
#![cfg(unix)]
use egui_kittest::{Harness,kittest::Queryable};
use kiln::app::{Action,KilnApp};
use std::{path::PathBuf,process::Command,time::{Duration,Instant},os::unix::fs::PermissionsExt};
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=kiln_daemon::client::Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(kiln_proto::ClientMsg::Shutdown);}}}
fn pump(h:&mut Harness<'_,KilnApp>,mut f:impl FnMut(&Harness<'_,KilnApp>)->bool){let deadline=Instant::now()+Duration::from_secs(20);loop{h.step();if f(h){return;}assert!(Instant::now()<deadline,"agent workflow timeout; terminal={:?}",h.state().debug_focused_text());std::thread::sleep(Duration::from_millis(25));}}
#[test]
fn long_parent_request_launches_a_separate_task_and_preserves_both_repositories(){
 let dir=tempfile::tempdir().unwrap();let base=dir.path();let root=base.join("personal");let bin=base.join("bin");let shell=base.join("shell");
 for p in [&root,&bin,&shell]{std::fs::create_dir_all(p).unwrap();}
 for name in ["frontend","backend"] {let p=root.join(name);std::fs::create_dir_all(&p).unwrap();assert!(Command::new("git").arg("-C").arg(&p).args(["init","-q"]).status().unwrap().success());}
 let executable=bin.join("codex");
 std::fs::write(&executable,"#!/bin/sh\nprintf '%s' \"$1\" > request.txt\npwd > working-directory.txt\nprintf 'probe frontend change\\n' > frontend/changed.txt\nprintf 'probe backend change\\n' > backend/changed.txt\nprintf 'KILN_PROBE_COMPLETE\\n'\n").unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
 std::fs::write(shell.join(".zshrc"),"PROMPT='probe > '\n").unwrap();
 let _cleanup=Cleanup(base.to_path_buf());
 unsafe {for (key,path) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts")),("ZDOTDIR",shell)]{std::env::set_var(key,path);}std::env::set_var("PATH",format!("{}:{}",bin.display(),std::env::var("PATH").unwrap_or_default()));std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([1200.,800.]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(root.clone())));let ctx=h.ctx.clone();
 pump(&mut h,|h|h.state().debug_focused_text().is_some());
 let original=h.state().debug_focused_session();let count=h.state().debug_task_count();
 h.state_mut().debug_apply_action(&ctx,Action::NewAgentTask);h.run_steps(3);
 let request=format!("프론트와 백엔드 로그인 함께 수정\n{}\n$(touch PWNED) ' \" `touch PWNED`\n", "한국어 긴 요청의 본문을 정확히 전달합니다.\n".repeat(250));
 h.event(egui::Event::Text(request.clone()));h.run_steps(3);
 pump(&mut h,|h|h.query_by_label("저장소 구성을 확인하는 중…").is_none());
 h.get_by_label("작업 시작").click();
 pump(&mut h, |_|root.join("backend/changed.txt").exists());h.run_steps(4);
 let delivered=std::fs::read_to_string(root.join("request.txt")).unwrap();
 assert!(delivered.contains(&request));assert!(delivered.contains("frontend"));assert!(delivered.contains("backend"));
 assert_eq!(std::fs::read_to_string(root.join("working-directory.txt")).unwrap().trim(),root.canonicalize().unwrap().to_str().unwrap());
 assert!(!root.join("PWNED").exists());assert_eq!(h.state().debug_task_count(),count+1);assert_ne!(h.state().debug_focused_session(),original);
 assert_eq!(h.state().debug_active_workspace_root(),root.canonicalize().unwrap());
 assert!(!h.state().debug_unsaved_items().iter().any(|s|s.contains("에이전트 요청 초안")));
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);h.run_steps(3);h.get_by_label("변경").click();
 pump(&mut h,|h|h.query_by_label("frontend 변경 펼치기").is_some()&&h.query_by_label("backend 변경 펼치기").is_some());
 h.get_by_label("frontend 변경 펼치기").click();h.run_steps(3);h.get_by_label("backend 변경 펼치기").click();h.run_steps(3);
 pump(&mut h,|h|h.query_all_by_label_contains("changed.txt").count()>=2);
 // Unsupported/missing tools fail before creating a useless task, preserving the draft.
 h.state_mut().debug_apply_action(&ctx,Action::NewAgentTask);h.run_steps(3);h.event(egui::Event::Text("보존할 요청".into()));h.run_steps(3);
 h.state_mut().debug_apply_action(&ctx,Action::LaunchAgent{cwd:root.clone(),program:"missing-agent".into(),context:String::new(),request:"보존할 요청".into()});h.run_steps(3);
 assert_eq!(h.state().debug_task_count(),count+1);assert!(h.get_all_by_value("보존할 요청").next().is_some());
 // Discarding a failed terminal command is not permission to discard the agent request.
 h.state_mut().debug_set_shell(base.join("missing-shell").to_string_lossy().into());
 h.state_mut().debug_apply_action(&ctx,Action::LaunchAgent{cwd:root,program:"codex".into(),context:String::new(),request:"보존할 요청".into()});
 pump(&mut h,|h|!h.state().debug_launch_drafts().is_empty());
 let failed=h.state().debug_launch_drafts()[0].0;
 h.state_mut().debug_set_shell(String::new());h.state_mut().debug_apply_action(&ctx,Action::DiscardTerminalLaunch(failed));
 pump(&mut h,|h|h.state().debug_focused_session().is_some());h.run_steps(3);
 assert!(h.state().debug_unsaved_items().iter().any(|s|s.contains("에이전트 요청 초안")));
 h.state_mut().debug_apply_action(&ctx,Action::NewAgentTask);h.run_steps(3);
 assert!(h.get_all_by_value("보존할 요청").next().is_some());
}
