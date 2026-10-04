//! A deterministic CLI probe exercises the real daemon/PTY without calling an AI service.
#![cfg(unix)]
use egui_kittest::{Harness,kittest::Queryable};
use kiln::app::{Action,KilnApp};
use std::{path::PathBuf,time::{Duration,Instant},os::unix::fs::PermissionsExt};
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=kiln_daemon::client::Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(kiln_proto::ClientMsg::Shutdown);}}}
fn pump(h:&mut Harness<'_,KilnApp>,mut f:impl FnMut(&Harness<'_,KilnApp>)->bool){let deadline=Instant::now()+Duration::from_secs(20);loop{h.step();if f(h){return;}assert!(Instant::now()<deadline,"agent workflow timeout; terminal={:?}",h.state().debug_focused_text());std::thread::sleep(Duration::from_millis(25));}}
#[test]
fn task_switch_reveals_canvas_acknowledges_attention_and_retains_original_request(){
 let dir=tempfile::tempdir().unwrap();let base=dir.path();let root=base.join("parent");let bin=base.join("bin");
 for path in [&root,&bin]{std::fs::create_dir_all(path).unwrap();}
 let executable=bin.join("codex");std::fs::write(&executable,"#!/bin/sh\nprintf 'REQUEST_STARTED\\n'\n").unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
 let _cleanup=Cleanup(base.to_path_buf());
 unsafe {for (key,path) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts"))]{std::env::set_var(key,path);}std::env::set_var("PATH",format!("{}:{}",bin.display(),std::env::var("PATH").unwrap_or_default()));std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([720.,440.]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(root.clone())));let ctx=h.ctx.clone();
 pump(&mut h,|h|h.state().debug_focused_text().is_some());h.run_steps(3);
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);h.run_steps(3);
 assert!(!h.state_mut().debug_deliver_attention(&ctx,Some(kiln_proto::AgentActivity::Waiting)));
 assert!(h.state().debug_unread_count()>0);
 h.state_mut().debug_apply_action(&ctx,Action::SelectPage(0));h.run_steps(3);
 assert!(!h.state().debug_sheet_open());assert!(h.state().debug_focused_is_visible());assert_eq!(h.state().debug_unread_count(),0);
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);h.run_steps(2);
 h.state_mut().debug_apply_action(&ctx,Action::NewPage);h.run_steps(3);assert!(!h.state().debug_sheet_open());
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);h.run_steps(2);
 h.state_mut().debug_apply_action(&ctx,Action::NextPage(-1));h.run_steps(3);assert!(!h.state().debug_sheet_open());
 let request=format!("{}\n## 사용자 요청\n{}\n", "프론트엔드와백엔드요청을함께검토하는긴작업제목".repeat(3),"API 연동과 오류 처리 및 접근성을 함께 개선합니다.\n".repeat(140));
 h.state_mut().debug_apply_action(&ctx,Action::LaunchAgent{cwd:root,program:"codex".into(),context:"함께 전달한 별도 context".into(),request:request.clone()});
 pump(&mut h,|h|h.state().debug_focused_text().is_some_and(|s|s.contains("REQUEST_STARTED")));
 h.state_mut().debug_checkpoint_restore(&ctx);h.run_steps(3);
 h.state_mut().debug_apply_action(&ctx,Action::ShowAgentRequest(2));h.run_steps(3);
 h.get_by_label("원래 요청");assert!(h.get_all_by_value(request.as_str()).next().is_some());
 let output=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reviews/2026-10-01-workflow");std::fs::create_dir_all(&output).unwrap();
 h.render().unwrap().save(output.join("original-request-720.png")).unwrap();
 h.get_by_label("요청 복사");h.get_by_label("닫기");
 ctx.set_zoom_factor(1.3);h.run_steps(4);h.render().unwrap().save(output.join("original-request-720-130.png")).unwrap();
 h.get_by_label("요청 복사");h.get_by_label("닫기");
 assert!(!h.state_mut().debug_deliver_attention(&ctx,None));h.run_steps(3);assert!(h.state().debug_unread_count()>0,"viewing task instructions must not acknowledge hidden terminal output");
 h.key_press(egui::Key::Escape);h.run_steps(3);assert_eq!(h.state().debug_unread_count(),0);
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);h.run_steps(3);
 h.state_mut().debug_apply_action(&ctx,Action::ShowAgentRequest(2));h.run_steps(3);
 h.get_by_label("원래 요청");h.key_press(egui::Key::Escape);h.run_steps(3);
 assert!(h.query_by_label("원래 요청").is_none());assert!(h.state().debug_sheet_open(),"Escape should close only the foremost request dialog");

}
