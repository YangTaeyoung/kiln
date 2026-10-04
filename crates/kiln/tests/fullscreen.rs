//! OS fullscreen is independent of pane zoom and remains escapable inside dialogs.
use egui_kittest::{Harness,kittest::Queryable};
use kiln::app::KilnApp;

#[test]
fn fullscreen_shortcut_is_symmetric_inside_settings_and_available_in_palette() {
    let dir=tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("KILN_CONFIG_DIR",dir.path().join("cfg"));
        std::env::set_var("KILN_SOCKET",dir.path().join("daemon.sock"));
        std::env::set_var("KILN_ACCOUNTS_SANDBOX",dir.path().join("accounts"));
        std::env::set_var("KILN_DB_NO_KEYCHAIN","1");
        std::env::set_var("KILN_NO_AUTO_UPGRADE","1");
        std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));
    }
    let mut h=Harness::builder().with_size([1000.0,700.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,None));
    h.state_mut().debug_open_settings(0);h.run_steps(3);
    let (modifiers,key)=if cfg!(target_os="macos") {
        (egui::Modifiers::CTRL|egui::Modifiers::MAC_CMD|egui::Modifiers::COMMAND,egui::Key::F)
    }else{(egui::Modifiers::NONE,egui::Key::F11)};
    for active in [false,true] {
        h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().fullscreen=Some(active);
        h.input_mut().events.push(egui::Event::Key{key,physical_key:None,pressed:true,repeat:false,modifiers});h.step();
        assert!(h.output().viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Fullscreen(!active)),"fullscreen must toggle from {active} while settings remains open");
    }
    h.key_press(egui::Key::Escape);h.run_steps(2);
    h.state_mut().debug_open_palette();h.run_steps(2);
    h.event(egui::Event::Text("앱 전체 화면".into()));h.run_steps(3);
    assert!(h.query_by_label_contains("앱 전체 화면 켜기 / 끄기").is_some());
    if let Ok(c)=kiln_daemon::client::Client::connect(&dir.path().join("daemon.sock").to_string_lossy(),None){c.send(kiln_proto::ClientMsg::Shutdown);}
}
