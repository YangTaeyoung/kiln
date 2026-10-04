//! Run SQL through the complete application shortcut dispatcher, not just its widget.
use egui_kittest::{Harness,kittest::Queryable};
use kiln::app::KilnApp;
use serde_json::json;

#[test]
fn focused_sql_run_all_survives_application_shortcuts() {
    let dir=tempfile::tempdir().unwrap();let cfg=dir.path().join("cfg");std::fs::create_dir(&cfg).unwrap();
    let db=dir.path().join("fixture.db");std::fs::write(&db,[]).unwrap();
    // This integration binary has one test; all mutable process configuration is isolated.
    unsafe {
        std::env::set_var("KILN_CONFIG_DIR",&cfg);
        std::env::set_var("KILN_SOCKET",dir.path().join("daemon.sock"));
        std::env::set_var("KILN_ACCOUNTS_SANDBOX",dir.path().join("accounts"));
        std::env::set_var("KILN_DB_NO_KEYCHAIN","1");
        std::env::set_var("KILN_NO_AUTO_UPGRADE","1");
        std::env::set_var("KILN_EXE",env!("CARGO_BIN_EXE_kiln"));
    }
    let connection=kiln_db::ConnConfig{id:kiln_db::ConnId(1),name:"SQL keyboard fixture".into(),driver:kiln_db::Driver::Sqlite,file:db.to_string_lossy().into_owned(),..Default::default()};
    std::fs::write(cfg.join("db_connections.json"),serde_json::to_vec(&json!({"connections":[connection]})).unwrap()).unwrap();
    let sql="SELECT 101 AS first_result; SELECT 202 AS second_result;";
    std::fs::write(cfg.join("state.json"),serde_json::to_vec(&json!({"workspaces":[{"name":"SQL fixture","root":dir.path(),"pages":[{"root":{"Leaf":7},"focused":7,"panes":[{"id":7,"tool":{"DbConsoleDocument":{"conn":1,"document":{"path":null,"text":sql,"saved_text":null}}}}]}]}]})).unwrap()).unwrap();
    let mut h=Harness::builder().with_size([1280.0,800.0]).build_eframe(|cc|KilnApp::new(&cc.egui_ctx,None));
    h.run_steps(4);
    h.query_all_by_value(sql).next().expect("SQL editor").click();h.run_steps(2);
    let command=if cfg!(target_os="macos"){egui::Modifiers::MAC_CMD|egui::Modifiers::COMMAND}else{egui::Modifiers::CTRL|egui::Modifiers::COMMAND};
    h.key_press_modifiers(command|egui::Modifiers::ALT,egui::Key::Enter);
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(8);
    while std::time::Instant::now()<deadline && h.query_by_label_contains("결과 2").is_none(){h.step();std::thread::sleep(std::time::Duration::from_millis(20));}
    assert!(h.query_by_label_contains("결과 1").is_some(),"first statement must execute");
    assert!(h.query_by_label_contains("결과 2").is_some(),"run-all chord must execute both statements through app shortcuts");
    if let Ok(c)=kiln_daemon::client::Client::connect(&dir.path().join("daemon.sock").to_string_lossy(),None){c.send(kiln_proto::ClientMsg::Shutdown);}
}
