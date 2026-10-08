//! Production asynchronous discovery and save lifecycle, using only owned
//! configuration files and an in-memory credential store. No SSH is executed.
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln_remote::{
    RemoteEndpoint,
    ui::{RemoteEvent, RemoteManager, RemotePanel},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn pump(h: &mut Harness<'_, RemotePanel>, ready: impl Fn(&Harness<'_, RemotePanel>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        if ready(h) {
            h.run_steps(3);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "production SSH form did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn replace(h: &mut Harness<'_, RemotePanel>, value: &str, text: &str) {
    h.get_by(|n| n.role() == Role::TextInput && n.value().as_deref() == Some(value))
        .click();
    h.run_steps(3);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(text.into()));
    h.run_steps(3);
}
fn ready(h: &Harness<'_, RemotePanel>) -> bool {
    h.query_by_label("SSH Config 호스트").is_some()
        && h.query_by_label("SSH 연결을 읽는 중…").is_none()
        && h.query_by_label("연결 저장")
            .is_some_and(|n| !n.accesskit_node().is_disabled())
}

#[test]
fn config_dropdown_prefills_saves_reedits_and_direct_hosts_need_no_alias() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ssh config");
    let include = dir.path().join("hosts.conf");
    let key = dir.path().join("키 with spaces");
    let marker = dir.path().join("must-not-run");
    let config_text = format!(
        "Include \"{}\"\nMatch exec \"touch {}\"\n User forbidden\n",
        include.display(),
        marker.display()
    );
    let include_text = format!(
        "Host studio\n HostName 192.0.2.10\n User deploy\n Port 2202\n IdentityFile \"{}\"\nHost lab\n HostName 192.0.2.11\n User developer\n",
        key.display()
    );
    std::fs::write(&config, &config_text).unwrap();
    std::fs::write(&include, &include_text).unwrap();
    let manager = RemoteManager::with_store(
        dir.path().join("connections.json"),
        Arc::new(kiln_accounts::MemoryStore::new()),
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    let panel = RemotePanel::new(manager.clone()).with_ssh_config_path(config.clone());
    let mut h = Harness::builder()
        .with_size([780., 760.])
        .wgpu()
        .build_ui_state(
            move |ui, panel: &mut RemotePanel| {
                let id = egui::Id::new("ssh-ui-fixture-fonts");
                if !ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
                    ui.ctx().data_mut(|d| d.insert_temp(id, true));
                    ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                    return;
                }
                kiln_common::Theme::current().apply(ui.ctx());
                recorded.lock().unwrap().extend(panel.ui(ui));
            },
            panel,
        );
    h.run_steps(3);
    h.get_by_label("SSH Config 가져오기").click();
    pump(&mut h, ready);
    h.get_by_value("직접 입력").click();
    h.run_steps(3);
    h.get_by_label("studio · 192.0.2.10").click();
    h.run_steps(3);
    for value in ["192.0.2.10", "deploy", "2202", key.to_str().unwrap()] {
        h.get_by(|n| n.role() == Role::TextInput && n.value().as_deref() == Some(value));
    }
    assert!(
        !marker.exists(),
        "passive discovery must never run Match exec"
    );
    h.set_size(egui::vec2(420., 760.));
    h.run_steps(4);
    for input in h.get_all_by_role(Role::TextInput) {
        let bounds = input.rect();
        assert!(
            bounds.left() >= 0. && bounds.right() <= 420.,
            "input outside compact viewport: {bounds:?}"
        );
    }
    h.render()
        .expect("render")
        .save("/tmp/kiln-015-sftp-prefill.png")
        .unwrap();
    replace(&mut h, "2202", "2300");
    h.get_by_label("연결 저장").click();
    pump(&mut h, |_| manager.profiles().len() == 1);
    pump(&mut h, |h| h.query_by_label("취소").is_none());
    let saved = manager.profiles()[0].clone();
    let RemoteEndpoint::Sftp {
        alias,
        config_path,
        options,
        ..
    } = &saved.endpoint
    else {
        panic!()
    };
    assert_eq!(alias, "studio");
    assert_eq!(config_path.as_ref(), Some(&config));
    assert_eq!(options.port, Some(2300));
    assert!(
        options.hostname.is_none() && options.user.is_none() && options.identity_file.is_none()
    );
    assert!(
        events.lock().unwrap().is_empty(),
        "saving must not launch a connection"
    );
    h.get_all_by_label("…").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("연결 편집").click();
    pump(&mut h, ready);
    h.get_by(|n| n.role() == Role::TextInput && n.value().as_deref() == Some("2300"));
    h.get_by_label("연결 저장").click();
    pump(&mut h, |h| h.query_by_label("취소").is_none());
    assert_eq!(
        manager.get(&saved.id),
        Some(saved.clone()),
        "re-edit keeps identity and explicit options"
    );
    h.get_by_label("SSH 터미널 열기").click();
    h.run_steps(3);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], RemoteEvent::Ssh { alias: a, config_path: c, options: o } if a==alias && c==config_path && o==options)
    );
    drop(events);
    h.get_by_label("SSH Config 가져오기").click();
    pump(&mut h, ready);
    let inputs: Vec<_> = h.get_all_by_role(Role::TextInput).collect();
    inputs[0].click();
    h.run_steps(3);
    h.event(egui::Event::Text("Direct fixture".into()));
    h.run_steps(3);
    h.get_all_by_role(Role::TextInput).nth(1).unwrap().click();
    h.run_steps(3);
    h.event(egui::Event::Text("192.0.2.20".into()));
    h.run_steps(3);
    h.get_all_by_role(Role::TextInput).nth(2).unwrap().click();
    h.run_steps(3);
    h.event(egui::Event::Text("developer".into()));
    h.run_steps(3);
    h.get_by_label("연결 저장").click();
    pump(&mut h, |_| manager.profiles().len() == 2);
    let direct = manager
        .profiles()
        .into_iter()
        .find(|p| p.name == "Direct fixture")
        .unwrap();
    assert!(
        matches!(direct.endpoint, RemoteEndpoint::Sftp {alias,config_path:None,options,..} if alias=="192.0.2.20" && options.user.as_deref()==Some("developer"))
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), config_text);
    assert_eq!(std::fs::read_to_string(&include).unwrap(), include_text);
    assert!(!marker.exists());
}
