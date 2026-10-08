//! Production discovery and save, with isolated CLI metadata and no cloud operations.
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use kiln_remote::{
    ObjectAuthentication, ObjectProvider, RemoteEndpoint,
    object_profiles::ProfileFiles,
    ui::{RemoteManager, RemotePanel},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[track_caller]
fn pump(
    h: &mut Harness<'_, RemotePanel>,
    provider: ObjectProvider,
    stage: &str,
    ready: impl Fn(&Harness<'_, RemotePanel>) -> bool,
) {
    let caller = std::panic::Location::caller();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        h.step();
        if ready(h) {
            return;
        }
        assert!(
            Instant::now() < until,
            "{provider:?} {stage} timed out at {caller}; popup_open={}; AX={:#?}",
            egui::Popup::is_any_open(&h.ctx),
            h.root()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn native_cli_profiles_discover_select_save_and_reedit_without_cloud_or_home_changes() {
    let dir = tempfile::tempdir().unwrap();
    let aws_config = dir.path().join("aws-config");
    let aws_credentials = dir.path().join("aws-credentials");
    std::fs::write(&aws_config, "").unwrap();
    std::fs::write(&aws_credentials, "").unwrap();
    unsafe {
        std::env::set_var("AWS_CONFIG_FILE", &aws_config);
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &aws_credentials);
    }
    let executable = dir.path().join("never-execute");
    std::fs::write(&executable, "fixture metadata only").unwrap();
    for (provider, label) in [
        (ObjectProvider::Oracle, "OCI Object Storage"),
        (ObjectProvider::Google, "Google Cloud Storage"),
        (ObjectProvider::Cloudflare, "Cloudflare R2"),
    ] {
        let config = dir.path().join(provider.cli());
        match provider {
            ObjectProvider::Oracle => std::fs::write(
                &config,
                "[release]\nregion=us-ashburn-1\n[studio]\nregion=us-phoenix-1\n",
            )
            .unwrap(),
            ObjectProvider::Google => {
                std::fs::create_dir_all(config.join("configurations")).unwrap();
                for name in ["release", "studio"] {
                    std::fs::write(
                        config.join(format!("configurations/config_{name}")),
                        "[core]\nproject=fixture-project\n",
                    )
                    .unwrap();
                }
            }
            ObjectProvider::Cloudflare => {
                std::fs::create_dir_all(config.join("config")).unwrap();
                for name in ["release", "studio"] {
                    std::fs::write(config.join("config").join(format!("{name}.toml")), [0xff])
                        .unwrap();
                }
            }
        }
        let store = Arc::new(kiln_accounts::MemoryStore::new());
        let manager = RemoteManager::with_store(
            dir.path().join(format!("{}.json", provider.cli())),
            store.clone(),
        );
        let panel = RemotePanel::new(manager.clone()).with_object_profile_files(
            provider,
            ProfileFiles {
                config: config.clone(),
                credentials: None,
                executable: Some(executable.clone()),
            },
        );
        let mut h = Harness::builder().with_size([900., 780.]).build_ui_state(
            |ui, panel: &mut RemotePanel| {
                let fonts = egui::Id::new("object-profile-fonts");
                if !ui
                    .ctx()
                    .data(|d| d.get_temp::<bool>(fonts).unwrap_or(false))
                {
                    ui.ctx().data_mut(|d| d.insert_temp(fonts, true));
                    ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                    return;
                }
                assert!(
                    panel.ui(ui).is_empty(),
                    "saving must not start a remote operation"
                );
            },
            panel,
        );
        // Spinner repaint requests are expected during real async discovery.
        // Render bounded frames for interactions; pump actual readiness below.
        h.run_steps(3);
        h.get_all_by_label("연결 추가").next().unwrap().click();
        h.run_steps(3);
        h.get_by_value("Amazon S3").click();
        h.run_steps(3);
        h.get_by_label(label).click();
        h.run_steps(3);
        pump(&mut h, provider, "initial discovery", |h| {
            h.query_by_label("프로필 읽는 중…").is_none()
                && h.query_all_by_value("프로필 선택").any(|node| {
                    node.accesskit_node().role() == Role::ComboBox
                        && !node.accesskit_node().is_disabled()
                })
        });
        // Let the centered modal settle after loading/metadata changes its height.
        h.run_steps(3);
        h.query_all_by_value("프로필 선택")
            .find(|node| {
                node.accesskit_node().role() == Role::ComboBox
                    && !node.accesskit_node().is_disabled()
            })
            .expect("ready profile selector")
            .click();
        // Discovery completes asynchronously. The save button's previous-frame
        // enabled state is not evidence that the provider's profiles have arrived.
        pump(&mut h, provider, "initial dropdown", |h| {
            h.query_all_by_label("studio").any(|node| {
                node.accesskit_node().role() == Role::Button && !node.accesskit_node().is_disabled()
            })
        });
        h.get_by_label("studio").click();
        h.run_steps(3);
        for (index, value) in [(0, "Fixture assets"), (1, "fixture-bucket")] {
            h.get_all_by_role(Role::TextInput)
                .nth(index)
                .unwrap()
                .click();
            h.run_steps(3);
            h.event(egui::Event::Text(value.into()));
            h.run_steps(3);
        }
        if provider == ObjectProvider::Cloudflare {
            h.get_all_by_role(Role::TextInput).nth(2).unwrap().click();
            h.run_steps(3);
            h.event(egui::Event::Text("0123456789abcdef0123456789abcdef".into()));
            h.run_steps(3);
        }
        h.get_by_label("연결 저장").click();
        pump(&mut h, provider, "save persisted", |_| {
            manager.profiles().len() == 1
        });
        pump(&mut h, provider, "save modal closed", |h| {
            h.query_by_label("취소").is_none()
        });
        let saved = manager.profiles()[0].clone();
        assert!(
            matches!(&saved.endpoint,RemoteEndpoint::ObjectStorage{provider:p,authentication:ObjectAuthentication::Cli{profile,config_path:Some(path)},..} if *p==provider && profile=="studio" && path==&config)
        );
        let secrets = kiln_remote::load_secrets(store.as_ref(), &saved.id).unwrap();
        assert!(
            secrets.access_key.is_none()
                && secrets.secret_key.is_none()
                && secrets.password.is_none()
                && secrets.session_token.is_none()
        );
        h.get_all_by_label("…").last().unwrap().click();
        h.run_steps(3);
        h.get_by_label("연결 편집").click();
        h.run_steps(3);
        pump(&mut h, provider, "reedit discovery", |h| {
            h.query_by_label("프로필 읽는 중…").is_none()
                && h.query_all_by_value("studio").any(|node| {
                    node.accesskit_node().role() == Role::ComboBox
                        && !node.accesskit_node().is_disabled()
                })
        });
        // Let the centered modal settle after loading/metadata changes its height.
        h.run_steps(3);
        h.query_all_by_value("studio")
            .find(|node| {
                node.accesskit_node().role() == Role::ComboBox
                    && !node.accesskit_node().is_disabled()
            })
            .expect("ready profile selector")
            .click();
        pump(&mut h, provider, "reedit dropdown", |h| {
            h.query_all_by_label("studio").any(|node| {
                node.accesskit_node().role() == Role::Button && !node.accesskit_node().is_disabled()
            })
        });
        h.get_by_label("studio").click();
        h.run_steps(3);
        h.get_by_value("studio");
        h.get_by_label("취소").click();
        h.run_steps(3);
        assert_eq!(manager.get(&saved.id), Some(saved));
    }
    assert_eq!(
        std::fs::read_to_string(executable).unwrap(),
        "fixture metadata only"
    );
}
