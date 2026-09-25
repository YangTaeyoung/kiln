//! 계정 설정 섹션 스냅샷(다크·라이트·빈 상태). 임시 홈과 메모리 저장소만 쓴다.
//! 테마는 프로세스 전역이므로 테스트 하나에서 순서대로 그린다.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_accounts::{AccountManager, AccountsEvent, CredentialStore, Env, Tool, Usage, accounts_settings_ui, claude};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn save_png<S>(h: &mut Harness<'_, S>, name: &str) {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&d).unwrap();
    match h.render() {
        Ok(img) => {
            let p = d.join(format!("{name}.png"));
            img.save(&p).unwrap();
            eprintln!("snapshot: {}", p.display());
        }
        Err(e) => eprintln!("render unavailable ({e}); skipping snapshot {name}"),
    }
}

/// 테마와 글꼴을 적용한다. 글꼴을 방금 설치한 프레임이면 false.
fn themed(ctx: &egui::Context) -> bool {
    kiln_common::Theme::current().apply(ctx);
    let flag = egui::Id::new("kiln-accounts-test-fonts");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    ctx.set_fonts(kiln_common::fonts::definitions(false));
    false
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn fake_jwt(email: &str) -> String {
    use base64::Engine;
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!("{}.{}.sig", e.encode(br#"{"alg":"none"}"#), e.encode(format!(r#"{{"email":"{email}"}}"#)))
}

fn login_claude(env: &Env, store: &dyn CredentialStore, email: &str, marker: &str) {
    let cred = format!(r#"{{"claudeAiOauth":{{"accessToken":"at-{marker}","refreshToken":"rt-{marker}"}}}}"#);
    store.set(claude::CLAUDE_SERVICE, "claude-code-user", &cred).unwrap();
    std::fs::create_dir_all(&env.home).unwrap();
    std::fs::write(claude::claude_json_path(&env.home), format!(r#"{{"oauthAccount":{{"emailAddress":"{email}"}}}}"#)).unwrap();
}

fn login_codex(env: &Env, email: &str) {
    std::fs::create_dir_all(&env.codex_home).unwrap();
    let auth = format!(r#"{{"auth_mode":"chatgpt","tokens":{{"id_token":"{}","access_token":"a","refresh_token":"r","account_id":"x"}}}}"#, fake_jwt(email));
    std::fs::write(env.codex_home.join("auth.json"), auth).unwrap();
}

fn populated(root: &Path) -> AccountManager {
    let (mut env, store) = Env::sandbox(root, true);
    let t = now() + 30;
    env.probe = Arc::new(move |tok: &str| match tok {
        "at-work" => Ok(Usage { five_hour: Some((0.42, Some(t + 2 * 3600 + 13 * 60))), seven_day: Some((0.18, Some(t + 4 * 86400 + 5 * 3600))), status: "allowed".into() }),
        "at-personal" => Ok(Usage { five_hour: Some((1.0, Some(t + 47 * 60))), seven_day: Some((0.64, Some(t + 2 * 86400))), status: "rejected".into() }),
        _ => Err(anyhow::anyhow!("authentication failed (HTTP 401) — token may be expired")),
    });
    let m = AccountManager::with_env(env.clone());
    login_claude(&env, store.as_ref(), "side@example.com", "side");
    m.save_current(Tool::Claude, "사이드").unwrap();
    login_claude(&env, store.as_ref(), "me@personal.dev", "personal");
    m.save_current(Tool::Claude, "개인").unwrap();
    login_claude(&env, store.as_ref(), "dev@company.io", "work");
    m.save_current(Tool::Claude, "회사").unwrap();
    m.set_auto_rotate(Tool::Claude, true);
    m.refresh_usage_blocking(Tool::Claude);

    login_codex(&env, "me@personal.dev");
    let p = m.save_current(Tool::Codex, "개인").unwrap();
    let day = env.codex_home.join("sessions/2099/01/01");
    std::fs::create_dir_all(&day).unwrap();
    let line = format!(
        r#"{{"timestamp":"2099-01-01T00:00:00Z","type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"primary":{{"used_percent":76.0,"window_minutes":300,"resets_at":{}}},"secondary":{{"used_percent":31.0,"window_minutes":10080,"resets_at":{}}},"plan_type":"pro"}}}}}}"#,
        t + 3 * 3600,
        t + 6 * 86400
    );
    std::fs::write(day.join("rollout-1.jsonl"), line + "\n").unwrap();
    login_codex(&env, "team@company.io");
    m.refresh_usage_blocking(Tool::Codex);
    assert_eq!(m.active(Tool::Codex).as_deref(), Some(p.id.as_str()));
    m
}

fn harness(m: AccountManager, height: f32) -> Harness<'static, (AccountManager, Vec<AccountsEvent>)> {
    Harness::builder()
        .with_size(vec2(720.0, height))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, (m, events): &mut (AccountManager, Vec<AccountsEvent>)| {
                if !themed(ui.ctx()) {
                    return;
                }
                let t = kiln_common::Theme::current();
                egui::Frame::new().fill(t.bg_panel).inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    events.extend(accounts_settings_ui(ui, m));
                });
            },
            (m, Vec::new()),
        )
}

#[test]
fn accounts_settings_snapshots() {
    let d = tempfile::tempdir().unwrap();
    let m = populated(d.path());

    kiln_common::Theme::set_current("kiln-dark");
    let mut h = harness(m.clone(), 900.0);
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.query_all_by_label("전환").count(), 2);
    save_png(&mut h, "accounts_dark");

    kiln_common::Theme::set_current("kiln-light");
    let mut h = harness(m.clone(), 900.0);
    for _ in 0..4 {
        h.step();
    }
    save_png(&mut h, "accounts_light");

    // 새 계정 추가는 백그라운드 sync-back 뒤 RunLogin 이벤트를 낸다.
    h.get_all_by_label("새 계정 추가").next().unwrap().click();
    let start = std::time::Instant::now();
    while !h.state().1.contains(&AccountsEvent::RunLogin(Tool::Claude)) {
        assert!(start.elapsed().as_secs() < 5, "RunLogin event not emitted");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    kiln_common::Theme::set_current("kiln-dark");
    let e = tempfile::tempdir().unwrap();
    let (env, _store) = Env::sandbox(e.path(), true);
    let mut h = harness(AccountManager::with_env(env), 620.0);
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.query_all_by_label("저장된 계정이 없습니다").count(), 2);
    save_png(&mut h, "accounts_empty_dark");
}
