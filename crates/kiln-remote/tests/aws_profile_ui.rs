//! The dependency is compiled without cfg(test), exercising the production
//! asynchronous AWS discovery and save lifecycle through public UI events.
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln_remote::{
    RemoteEndpoint, S3Authentication,
    ui::{RemoteManager, RemotePanel},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn pump(h: &mut Harness<'_, RemotePanel>, ready: impl Fn(&Harness<'_, RemotePanel>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "production async UI did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn replace(h: &mut Harness<'_, RemotePanel>, value: &str, next: &str) {
    h.get_by(|node| node.role() == Role::TextInput && node.value().as_deref() == Some(value))
        .click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(next.into()));
    h.run();
}

#[test]
fn production_profile_discovery_selection_save_and_reedit() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("aws-config");
    let credentials = dir.path().join("aws-credentials");
    let config_bytes = "[default]\nregion=us-east-1\n[profile studio]\nregion=ap-northeast-2\n";
    let credential_bytes = "[default]\naws_access_key_id=fixture-default\naws_secret_access_key=fixture-only\n[studio]\naws_access_key_id=fixture-studio\naws_secret_access_key=fixture-only\n";
    std::fs::write(&config, config_bytes).unwrap();
    std::fs::write(&credentials, credential_bytes).unwrap();
    // This integration binary contains one test. No user configuration or real
    // Keychain is used, and no remote connection event is executed.
    unsafe {
        std::env::set_var("AWS_CONFIG_FILE", &config);
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &credentials);
    }
    let store = Arc::new(kiln_accounts::MemoryStore::new());
    let path = dir.path().join("connections.json");
    let manager = RemoteManager::with_store(path.clone(), store.clone());
    let panel = RemotePanel::new(manager.clone());
    let mut h = Harness::builder().with_size([980.0, 800.0]).build_ui_state(
        |ui, panel: &mut RemotePanel| {
            let font_id = egui::Id::new("aws-integration-fonts");
            if !ui
                .ctx()
                .data(|d| d.get_temp::<bool>(font_id))
                .unwrap_or(false)
            {
                ui.ctx().data_mut(|d| d.insert_temp(font_id, true));
                ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                return;
            }
            assert!(
                panel.ui(ui).is_empty(),
                "must never start a remote operation"
            );
        },
        panel,
    );
    h.run();
    h.get_all_by_label("연결 추가").next().unwrap().click();
    pump(&mut h, |h| h.query_by_value("default").is_some());
    let inputs: Vec<_> = h.get_all_by_role(Role::TextInput).collect();
    assert_eq!(
        inputs.len(),
        3,
        "profile mode shows name, bucket, region only"
    );
    inputs[0].click();
    h.run();
    h.event(egui::Event::Text("Fixture assets".into()));
    h.run();
    h.get_all_by_role(Role::TextInput).nth(1).unwrap().click();
    h.run();
    h.event(egui::Event::Text("kiln-fixture".into()));
    h.run();
    h.get_by_value("default").click();
    h.run();
    h.get_by_label("studio").click();
    h.run();
    assert!(h.query_all_by_value("ap-northeast-2").next().is_some());
    replace(&mut h, "ap-northeast-2", "us-west-2");
    h.get_by_value("studio").click();
    h.run();
    h.get_by_label("default").click();
    h.run();
    assert!(
        h.query_all_by_value("us-west-2").next().is_some(),
        "profile changes preserve edited region"
    );
    h.get_by_value("default").click();
    h.run();
    h.get_by_label("studio").click();
    h.run();
    h.get_by_label("연결 저장").click();
    pump(&mut h, |_| manager.profiles().len() == 1);
    pump(&mut h, |h| h.query_by_label("취소").is_none());
    let saved = manager.profiles()[0].clone();
    assert_eq!(saved.name, "Fixture assets");
    assert!(
        matches!(&saved.endpoint, RemoteEndpoint::S3 {aws_profile: Some(p), region, bucket, ..} if p == "studio" && region == "us-west-2" && bucket == "kiln-fixture")
    );
    let secrets = kiln_remote::load_secrets(store.as_ref(), &saved.id).unwrap();
    assert!(
        secrets.access_key.is_none()
            && secrets.secret_key.is_none()
            && secrets.session_token.is_none()
    );
    assert_eq!(
        RemoteManager::with_store(path.clone(), store.clone()).get(&saved.id),
        Some(saved.clone())
    );
    h.get_all_by_label("…").last().unwrap().click();
    h.run();
    h.get_by_label("연결 편집").click();
    pump(&mut h, |h| h.query_by_value("studio").is_some());
    assert!(h.query_all_by_value("us-west-2").next().is_some());
    h.get_by_value("저장된 AWS 프로필").click();
    h.run();
    h.get_by_label("직접 키 입력").click();
    h.run();
    assert!(h.query_by_label("Access Key ID").is_some());
    h.get_by_value("직접 키 입력").click();
    h.run();
    h.get_by_label("기본 AWS 인증").click();
    h.run();
    assert!(h.query_by_label("Access Key ID").is_none());
    h.get_by_label("취소").click();
    h.run();
    assert_eq!(
        manager.get(&saved.id),
        Some(saved.clone()),
        "cancel must preserve the saved profile"
    );
    h.get_all_by_label("…").last().unwrap().click();
    h.run();
    h.get_by_label("연결 편집").click();
    pump(&mut h, |h| h.query_by_value("studio").is_some());
    h.get_by_value("저장된 AWS 프로필").click();
    h.run();
    h.get_by_label("기본 AWS 인증").click();
    h.run();
    h.get_by_label("연결 저장").click();
    pump(&mut h, |_| {
        matches!(
            manager.get(&saved.id).unwrap().endpoint,
            RemoteEndpoint::S3 {
                aws_profile: None,
                aws_auth: Some(S3Authentication::Default),
                ..
            }
        )
    });
    pump(&mut h, |h| h.query_by_label("취소").is_none());
    h.get_all_by_label("…").last().unwrap().click();
    h.run();
    h.get_by_label("연결 편집").click();
    // Save is disabled until the production discovery task is consumed. Waiting
    // for its enabled state prevents an assertion before the late recommendation.
    pump(&mut h, |h| {
        h.query_by_label("연결 저장")
            .is_some_and(|n| !n.accesskit_node().is_disabled())
    });
    assert!(h.query_by_value("기본 AWS 인증").is_some());
    assert!(h.query_by_value("저장된 AWS 프로필").is_none());
    assert!(h.query_by_value("studio").is_none());
    assert!(h.query_all_by_value("us-west-2").next().is_some());
    h.get_by_label("취소").click();
    h.run();
    let stored = std::fs::read_to_string(&path).unwrap();
    assert!(
        !stored.contains("fixture-default")
            && !stored.contains("fixture-studio")
            && !stored.contains("fixture-only")
    );
    assert_eq!(std::fs::read_to_string(config).unwrap(), config_bytes);
    assert_eq!(
        std::fs::read_to_string(credentials).unwrap(),
        credential_bytes
    );
}
