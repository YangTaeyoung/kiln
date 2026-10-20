//! A Codex limit with no other account to switch to offers the active account's reset credit.
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln::app::KilnApp;
use kiln_accounts::{AccountManager, Env, ResetCredits, ResetOutcome, Tool, Usage};
use kiln_daemon::client::Client;
use kiln_proto::ClientMsg;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(c) = Client::connect(&self.0.join("d.sock").to_string_lossy(), None) { c.send(ClientMsg::Shutdown); }
    }
}

/// One exhausted Codex account with `resets` reset credits; redemptions are counted in `redeemed`.
fn codex_account(root: &Path, resets: u32, redeemed: Arc<AtomicUsize>) -> AccountManager {
    let (mut env, _) = Env::sandbox(root, false);
    env.codex_probe = Some(Arc::new(move |_| Ok(Usage { five_hour: Some((1.0, Some(4_200_000_000))), status: "allowed".into(),
        resets: Some(ResetCredits { available: resets, next_expiry: None }), ..Default::default() })));
    env.codex_reset = Arc::new(move |_, _| { redeemed.fetch_add(1, Ordering::SeqCst); Ok(ResetOutcome::Reset) });
    std::fs::create_dir_all(&env.codex_home).unwrap();
    std::fs::write(env.codex_home.join("auth.json"), kiln_accounts::test_support::codex_auth("work@example.test", "work")).unwrap();
    let manager = AccountManager::with_env(env);
    manager.save_current(Tool::Codex, "Work").unwrap();
    manager.refresh_usage_blocking(Tool::Codex);
    manager
}

#[test]
fn a_codex_limit_without_another_account_offers_and_redeems_a_reset_credit() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let root = base.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let _cleanup = Cleanup(base.clone());
    // SAFETY: this test binary runs this one test, before any thread reads these variables.
    unsafe {
        for (key, path) in [("KILN_SOCKET", base.join("d.sock")), ("KILN_CONFIG_DIR", base.join("cfg")), ("KILN_ACCOUNTS_SANDBOX", base.join("accounts"))] {
            std::env::set_var(key, path);
        }
        std::env::set_var("KILN_EXE", env!("CARGO_BIN_EXE_kiln"));
        std::env::set_var("KILN_NO_AUTO_UPGRADE", "1");
        std::env::set_var("KILN_DB_NO_KEYCHAIN", "1");
    }
    let mut h = Harness::builder().with_size([1200., 800.]).build_eframe(|cc| KilnApp::new(&cc.egui_ctx, Some(root.clone())));
    let deadline = Instant::now() + Duration::from_secs(12);
    while h.state().debug_focused_session().is_none() {
        h.step();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let session = h.state().debug_focused_session().unwrap();

    let redeemed = Arc::new(AtomicUsize::new(0));
    h.state_mut().debug_set_accounts(codex_account(&base.join("with-credit"), 2, redeemed.clone()));
    h.state_mut().debug_codex_limit(session, false);
    assert_eq!(h.state().debug_toast_buttons(), vec!["초기화권 사용".to_string()]);
    h.run_steps(3);
    assert_eq!(redeemed.load(Ordering::SeqCst), 0, "the offer alone redeems nothing");
    h.get_by_label("초기화권 사용").click();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.state().debug_toast_titles().iter().any(|title| title == "‘Work’ 사용량 한도를 초기화했습니다") {
        assert!(Instant::now() < deadline, "no outcome toast: {:?}", h.state().debug_toast_titles());
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(redeemed.load(Ordering::SeqCst), 1);

    h.state_mut().debug_set_accounts(codex_account(&base.join("without-credit"), 0, Arc::default()));
    h.state_mut().debug_codex_limit(session, false);
    assert_eq!(h.state().debug_toast_buttons().last().map(String::as_str), Some("다음 계정으로 전환"));

    h.state_mut().debug_set_accounts(codex_account(&base.join("switching-on"), 1, Arc::default()));
    h.state_mut().debug_codex_limit(session, true);
    assert_eq!(h.state().debug_toast_buttons().last().map(String::as_str), Some("초기화권 사용"));
}
