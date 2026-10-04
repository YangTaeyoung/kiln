//! Attention must remain unread while the model-selected terminal is covered.
use egui_kittest::Harness;
use kiln::app::{Action,KilnApp};
use kiln_daemon::client::Client;
use kiln_proto::{AgentActivity,ClientMsg};
use std::{path::PathBuf,time::{Duration,Instant}};
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){if let Ok(c)=Client::connect(&self.0.join("d.sock").to_string_lossy(),None){c.send(ClientMsg::Shutdown);}}}
#[test]
fn attention_is_acknowledged_only_by_an_observed_terminal() {
 let dir=tempfile::tempdir().unwrap();let base=dir.path().to_path_buf();let root=base.join("workspace");std::fs::create_dir_all(&root).unwrap();let _cleanup=Cleanup(base.clone());
 unsafe {for (key,path) in [("KILN_SOCKET",base.join("d.sock")),("KILN_CONFIG_DIR",base.join("cfg")),("KILN_ACCOUNTS_SANDBOX",base.join("accounts"))]{std::env::set_var(key,path);}std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));std::env::set_var("KILN_NO_AUTO_UPGRADE","1");std::env::set_var("KILN_DB_NO_KEYCHAIN","1");}
 let mut h=Harness::builder().with_size([720.,440.]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,Some(root.clone())));let ctx=h.ctx.clone();
 let deadline=Instant::now()+Duration::from_secs(12);
 while h.state().debug_focused_text().is_none() {h.step();assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(20));}
 h.run_steps(3);
 assert_eq!(h.state_mut().debug_connection_notice_inbox_growth(&ctx),0,"routine daemon notices must remain transient");
 // A normal shell need not enable the terminal focus-report escape protocol.
 assert!(h.state_mut().debug_deliver_attention(&ctx,None),"visible focused terminal should acknowledge OSC attention");
 assert!(h.state_mut().debug_deliver_attention(&ctx,Some(AgentActivity::Done)));
 // Opening an exclusive Inspector must immediately block even a stale terminal claimant.
 h.state_mut().debug_apply_action(&ctx,Action::ToggleInspector);
 assert!(!h.state_mut().debug_deliver_attention(&ctx,None));
 assert!(!h.state_mut().debug_deliver_attention(&ctx,Some(AgentActivity::Waiting)));
 h.state_mut().debug_apply_action(&ctx,Action::CloseSheet);h.run_steps(3);
 assert!(h.state_mut().debug_deliver_attention(&ctx,None));
 h.state_mut().debug_apply_action(&ctx,Action::ToggleNotifications);
 assert!(!h.state_mut().debug_deliver_attention(&ctx,None));
 h.state_mut().debug_apply_action(&ctx,Action::ToggleNotifications);
 let terminal_widget=ctx.memory(|m|m.focused()).unwrap();
 // Text input/search owns keyboard focus while the same session is model-selected.
 ctx.memory_mut(|memory|memory.request_focus(egui::Id::new("other-input")));
 assert!(!h.state_mut().debug_deliver_attention(&ctx,None));
 ctx.memory_mut(|memory|memory.request_focus(terminal_widget));
 h.state_mut().debug_open_settings(0);
 assert!(!h.state_mut().debug_deliver_attention(&ctx,Some(AgentActivity::Failed)));
 h.state_mut().debug_apply_action(&ctx,Action::OpenRecovery);
 assert!(!h.state_mut().debug_deliver_attention(&ctx,None));
}
