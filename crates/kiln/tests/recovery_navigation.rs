//! Attention must remain unread while the model-selected terminal is covered.
use egui_kittest::Harness;
use kiln::app::{Action,KilnApp};
use kiln_daemon::client::Client;
use kiln_proto::ClientMsg;
use egui_kittest::kittest::Queryable;
use std::{path::PathBuf,time::{Duration,Instant}};
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(ClientMsg::Shutdown);}}}
#[test]
fn recovery_resumes_prompt_and_shell_settings_and_background_limit_keeps_owner() {
 let dir=tempfile::tempdir().unwrap();let base=dir.path().to_path_buf();let root=base.join("parent-workspace");std::fs::create_dir_all(&root).unwrap();let _cleanup=Cleanup(base.clone());
 unsafe {for (key,path) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts"))]{std::env::set_var(key,path);}std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([1200.,800.]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(root.clone())));let ctx=h.ctx.clone();
 let deadline=Instant::now()+Duration::from_secs(12);
 while h.state().debug_focused_text().is_none() {h.step();assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(20));}
 let session=h.state().debug_focused_session().unwrap();
 h.state_mut().debug_recovery_fixture("프론트와 백 API를 함께 변경");h.run_steps(3);
 h.get_by_label("에이전트 요청 이어 쓰기").click();h.run_steps(3);
 assert!(h.get_all_by_value("프론트와 백 API를 함께 변경").next().is_some());
 assert!(h.state().debug_unsaved_items().iter().any(|v|v.contains("에이전트 요청 초안")));
 h.state_mut().debug_apply_action(&ctx,Action::OpenRecovery);h.run_steps(3);
 h.get_by_label("셸 설정").click();h.run_steps(3);
 h.get_by_label("기본 셸");
 h.key_press(egui::Key::Escape);h.run_steps(2);
 let second=base.join("another-workspace");std::fs::create_dir_all(&second).unwrap();
 h.state_mut().debug_apply_action(&ctx,Action::NewWorkspace(Some(second)));
 assert_eq!(h.state_mut().debug_limit_workspace(session),"parent-workspace");
}
