//! Real-daemon regression for launch cancellation/failure and checkpoint recovery.
use egui_kittest::Harness;
use kiln::app::{Action,KilnApp};
use kiln_daemon::client::Client;
use kiln_proto::{ClientMsg,ServerMsg};
use std::{path::PathBuf,time::{Duration,Instant}};
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(ClientMsg::Shutdown);}}}
fn pump(h:&mut Harness<'_,KilnApp>,mut done:impl FnMut(&Harness<'_,KilnApp>)->bool) {
 let deadline=Instant::now()+Duration::from_secs(12);
 loop {h.step();if done(h){return;}assert!(Instant::now()<deadline,"lifecycle condition timed out");std::thread::sleep(Duration::from_millis(20));}
}
fn sessions(c:&Client)->usize {match c.request(|req|ClientMsg::ListSessions{req},Duration::from_secs(3)).unwrap(){ServerMsg::Sessions{sessions,..}=>sessions.len(),other=>panic!("unexpected {other:?}")}}
#[test]
fn repeated_open_cancel_failed_launch_and_disconnected_recovery() {
 let dir=tempfile::tempdir().unwrap();let base=dir.path().to_path_buf();let root=base.join("workspace");std::fs::create_dir_all(&root).unwrap();let _cleanup=Cleanup(base.clone());
 unsafe {for (key,path) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts"))]{std::env::set_var(key,path);}std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([1000.,700.]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(root.clone())));let ctx=h.ctx.clone();
 pump(&mut h,|h|h.state().debug_focused_session().is_some());
 let c=Client::connect(&base.join("d.sock").to_string_lossy(),None).unwrap();let initial=sessions(&c);
 h.state_mut().debug_apply_action(&ctx,Action::NewWorkspace(Some(root.clone())));
 #[cfg(unix)] {let alias=base.join("alias");std::os::unix::fs::symlink(&root,&alias).unwrap();h.state_mut().debug_apply_action(&ctx,Action::NewWorkspace(Some(alias)));}
 assert_eq!(h.state().debug_workspace_count(),1);assert_eq!(sessions(&c),initial);
 // Close before the GUI receives Created. The daemon may already have spawned it.
 h.state_mut().debug_apply_action(&ctx,Action::RunInTerminalAt{cwd:root.clone(),command:"echo should-not-run".into()});
 let cancelled=h.state().debug_focused_pane_id().unwrap();
 assert!(h.state().debug_pending_launches()>0);
 h.state_mut().debug_apply_action(&ctx,Action::ClosePane(cancelled,true));
 for _ in 0..20 {h.step();std::thread::sleep(Duration::from_millis(20));}
 assert_eq!(sessions(&c),initial,"cancelled creation left an orphan daemon session");
 // A missing executable produces an actual ServerMsg::Error carrying req.
 h.state_mut().debug_set_shell(base.join("missing-shell").to_string_lossy().into());
 h.state_mut().debug_apply_action(&ctx,Action::RunInTerminalAt{cwd:root.clone(),command:"printf recovered".into()});
 pump(&mut h,|h|!h.state().debug_launch_drafts().is_empty());
 assert_eq!(h.state().debug_pending_launches(),0);
 let (failed,command)=h.state().debug_launch_drafts()[0].clone();assert_eq!(command.as_deref(),Some("printf recovered"));
 h.state_mut().debug_set_shell(String::new());h.state_mut().debug_apply_action(&ctx,Action::RetryTerminalLaunch(failed));
 pump(&mut h,|h|h.state().debug_focused_text().is_some_and(|text|text.contains("recovered")));
 assert!(h.state().debug_launch_drafts().is_empty());
 // A disconnected destructive close must preserve the pane and running session.
 let session=h.state().debug_focused_session();let count=h.state().debug_pane_count();
 h.state_mut().debug_disconnect(&ctx);h.state_mut().debug_apply_action(&ctx,Action::ClosePane(failed,true));
 assert_eq!(h.state().debug_pane_count(),count);assert_eq!(h.state().debug_focused_session(),session);
 h.state_mut().debug_apply_action(&ctx,Action::CloseWorkspaceConfirmed(0));assert_eq!(h.state().debug_workspace_count(),1);
 // Offline queued command survives serialization but never runs automatically.
 let marker=base.join("must-not-execute");let command=format!("touch '{}'",marker.display());
 h.state_mut().debug_apply_action(&ctx,Action::RunInTerminalAt{cwd:root,command:command.clone()});
 assert!(h.state().debug_unsaved_items().iter().any(|item|item.contains("터미널 요청")));
 h.state_mut().debug_checkpoint_restore(&ctx);
 assert!(h.state().debug_launch_drafts().iter().any(|(_,draft)|draft.as_deref()==Some(command.as_str())));
 h.run_steps(4);assert!(!marker.exists());assert_eq!(h.state().debug_pending_launches(),0);
}
