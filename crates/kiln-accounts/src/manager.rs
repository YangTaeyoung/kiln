//! 계정 저장·전환·사용량 조회. 상태는 `Arc` 안에 두고, 느린 작업은 백그라운드 스레드에서 돈다.

use crate::store::{CredentialStore, FileStore, SecurityCli, read_secret, write_secret_atomic};
use crate::ui::{AccountsEvent, UiState};
use crate::{Profile, Tool, Usage, claude, codex, now_unix};
use anyhow::{Context, Result, anyhow, bail};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 사용량 프로브. accessToken 을 받아 사용량을 돌려준다.
pub type UsageProbe = Arc<dyn Fn(&str) -> Result<Usage> + Send + Sync>;

/// 파일 경로·비밀값 저장소·프로브 묶음. 테스트는 [`Env::sandbox`] 로 임시 디렉토리와 메모리 저장소를 쓴다.
#[derive(Clone)]
pub struct Env {
    pub home: PathBuf,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: PathBuf,
    /// true 면 Claude Code 의 현재 자격증명을 `store` 의 `Claude Code-credentials` 항목으로 다룬다(macOS).
    pub claude_live_in_store: bool,
    pub store: Arc<dyn CredentialStore>,
    pub settings_path: PathBuf,
    pub probe: UsageProbe,
    /// 키체인 account 속성을 모를 때 쓰는 사용자 이름.
    pub username: String,
}

impl Env {
    /// 실제 홈 디렉토리·키체인(macOS) 또는 0600 파일(그 밖)·`kiln_common` 설정 경로.
    pub fn real() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
        let codex_env = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
        let codex_home = codex::codex_home(&home, codex_env.as_deref());
        let macos = cfg!(target_os = "macos");
        let store: Arc<dyn CredentialStore> = if macos {
            Arc::new(SecurityCli)
        } else {
            Arc::new(FileStore { base_dir: kiln_common::paths::config_file("accounts-credentials") })
        };
        Env {
            home,
            claude_config_dir,
            codex_home,
            claude_live_in_store: macos,
            store,
            settings_path: kiln_common::paths::config_file("accounts.json"),
            probe: Arc::new(|token: &str| claude::probe_usage(token)),
            username: std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default(),
        }
    }

    /// `root` 아래 가짜 홈과 메모리 저장소를 쓰는 환경. 프로브는 항상 실패한다.
    pub fn sandbox(root: &Path, claude_live_in_store: bool) -> (Self, Arc<crate::MemoryStore>) {
        let store = Arc::new(crate::MemoryStore::new());
        let home = root.join("home");
        let env = Env {
            codex_home: home.join(".codex"),
            home,
            claude_config_dir: None,
            claude_live_in_store,
            store: store.clone(),
            settings_path: root.join("config/accounts.json"),
            probe: Arc::new(|_: &str| Err(anyhow!("probe disabled"))),
            username: "tester".into(),
        };
        (env, store)
    }

    fn claude_credentials_path(&self) -> PathBuf {
        claude::credentials_path(&self.home, self.claude_config_dir.as_deref())
    }

    fn claude_json_path(&self) -> PathBuf {
        let config_dir = self.claude_config_dir.clone().unwrap_or_else(|| self.home.join(".claude"));
        let legacy = config_dir.join(".config.json");
        if legacy.exists() { legacy } else {
            self.claude_config_dir.as_ref().map(|dir| dir.join(".claude.json")).unwrap_or_else(|| claude::claude_json_path(&self.home))
        }
    }
}

/// 프로필 스냅샷을 담는 키체인 service 이름.
pub fn profile_service(tool: Tool) -> String {
    format!("kiln-accounts-{}", tool.key())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredProfile {
    #[serde(flatten)]
    profile: Profile,
    /// Claude: `~/.claude.json` 의 `oauthAccount` 원문(계정 식별 메타데이터).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth_account: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_usage: Option<Usage>,
    /// An isolated browser login is saved but not installed in the live CLI yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending_login: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ToolSettings {
    #[serde(default)]
    auto_rotate: bool,
    #[serde(default)]
    active: Option<String>,
    /// 현재 계정으로 전환한 시각(unix 초).
    #[serde(default)]
    active_since: i64,
    /// Claude Code 키체인 항목의 account 속성.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keychain_account: Option<String>,
    #[serde(default)]
    profiles: Vec<StoredProfile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    claude: ToolSettings,
    #[serde(default)]
    codex: ToolSettings,
}

impl Settings {
    fn tool(&self, t: Tool) -> &ToolSettings {
        match t {
            Tool::Claude => &self.claude,
            Tool::Codex => &self.codex,
        }
    }

    fn tool_mut(&mut self, t: Tool) -> &mut ToolSettings {
        match t {
            Tool::Claude => &mut self.claude,
            Tool::Codex => &mut self.codex,
        }
    }
}

fn idx(t: Tool) -> usize {
    match t {
        Tool::Claude => 0,
        Tool::Codex => 1,
    }
}

#[derive(Default)]
pub(crate) struct State {
    settings: Settings,
    usage: HashMap<(Tool, String), Usage>,
    limited_at: HashMap<(Tool, String), i64>,
    pub(crate) refreshing: [bool; 2],
    pub(crate) busy: [Option<String>; 2],
    pub(crate) error: [Option<String>; 2],
    pub(crate) notice: [Option<String>; 2],
    /// 현재 로그인된 계정 이메일(백그라운드에서 읽는다). 바깥 `None` 은 아직 모름.
    pub(crate) live_email: [Option<Option<String>>; 2],
    pub(crate) events: Vec<AccountsEvent>,
}

struct Inner {
    env: Env,
    state: Mutex<State>,
    /// 자격증명 파일·키체인을 바꾸는 작업을 한 번에 하나씩 돌린다.
    op: Mutex<()>,
    ctx: Mutex<Option<egui::Context>>,
    pub(crate) ui: Mutex<UiState>,
    login: Mutex<[Option<Arc<crate::login::LoginControl>>; 2]>,
    shutting_down: std::sync::atomic::AtomicBool,
}

/// 계정 관리자. 복제해도 같은 상태를 공유한다.
#[derive(Clone)]
pub struct AccountManager {
    inner: Arc<Inner>,
}

/// 소진된 것으로 표시한 계정을 건너뛰는 시간(초).
const LIMITED_HOLD_SECS: i64 = 5 * 3600;

impl AccountManager {
    /// 실제 환경에서 설정을 읽고, 현재 로그인 계정 확인을 백그라운드로 시작한다.
    pub fn load() -> Self {
        let m = Self::with_env(Env::real());
        m.check_live_identity_async();
        m
    }

    /// 주어진 환경으로 만든다. 설정 파일만 읽고 자격증명에는 손대지 않는다.
    pub fn with_env(env: Env) -> Self {
        let settings: Settings = kiln_common::store::load_json(&env.settings_path);
        let mut usage = HashMap::new();
        for t in Tool::ALL {
            for p in &settings.tool(t).profiles {
                if let Some(u) = &p.last_usage {
                    usage.insert((t, p.profile.id.clone()), u.clone());
                }
            }
        }
        AccountManager {
            inner: Arc::new(Inner {
                env,
                state: Mutex::new(State { settings, usage, ..Default::default() }),
                op: Mutex::new(()),
                ctx: Mutex::new(None),
                ui: Mutex::new(UiState::default()),
                login: Mutex::new([None, None]),
                shutting_down: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    pub fn env(&self) -> &Env {
        &self.inner.env
    }

    /// 백그라운드 작업이 끝나면 다시 그리게 할 컨텍스트.
    pub fn set_repaint_context(&self, ctx: &egui::Context) {
        let mut c = self.inner.ctx.lock();
        if c.is_none() {
            *c = Some(ctx.clone());
        }
    }

    fn repaint(&self) {
        if let Some(c) = self.inner.ctx.lock().as_ref() {
            c.request_repaint();
        }
    }

    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.inner.state.lock())
    }

    pub(crate) fn ui_state(&self) -> parking_lot::MutexGuard<'_, UiState> {
        self.inner.ui.lock()
    }

    pub fn profiles(&self, tool: Tool) -> Vec<Profile> {
        self.with_state(|s| s.settings.tool(tool).profiles.iter().map(|p| p.profile.clone()).collect())
    }

    pub fn active(&self, tool: Tool) -> Option<String> {
        self.with_state(|s| s.settings.tool(tool).active.clone())
    }

    pub fn pending_login(&self, tool: Tool, id: &str) -> bool {
        self.with_state(|s| s.settings.tool(tool).profiles.iter().find(|profile| profile.profile.id == id).is_some_and(|profile| profile.pending_login))
    }

    pub fn usage(&self, tool: Tool, id: &str) -> Option<Usage> {
        self.with_state(|s| s.usage.get(&(tool, id.to_string())).cloned())
    }

    pub fn is_refreshing(&self, tool: Tool) -> bool {
        self.with_state(|s| s.refreshing[idx(tool)])
    }

    pub fn auto_rotate(&self, tool: Tool) -> bool {
        self.with_state(|s| s.settings.tool(tool).auto_rotate)
    }

    pub fn set_auto_rotate(&self, tool: Tool, on: bool) {
        self.with_state(|s| s.settings.tool_mut(tool).auto_rotate = on);
        self.persist_logged();
    }

    /// 현재 로그인된 계정 이메일. 아직 확인 전이면 `None`, 확인했지만 모르면 `Some(None)`.
    pub fn live_email(&self, tool: Tool) -> Option<Option<String>> {
        self.with_state(|s| s.live_email[idx(tool)].clone())
    }

    pub fn login_command(&self, tool: Tool) -> String {
        crate::login_command(tool)
    }

    pub fn login_status(&self, tool: Tool) -> Option<crate::LoginStatus> {
        self.inner.login.lock()[idx(tool)].as_ref().map(|job| job.status.lock().clone())
    }

    #[cfg(test)]
    pub(crate) fn login_fixture(&self, tool: Tool, status: crate::LoginStatus) -> Arc<crate::login::LoginControl> {
        let control = Arc::new(crate::login::LoginControl::default());
        *control.status.lock() = status;
        self.inner.login.lock()[idx(tool)] = Some(control.clone());
        self.with_state(|s| s.busy[idx(tool)] = Some(kiln_common::i18n::tr("브라우저에서 로그인을 완료하세요").into()));
        control
    }

    pub fn cancel_login(&self, tool: Tool) {
        if let Some(job) = &self.inner.login.lock()[idx(tool)] {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.with_state(|s| s.busy[idx(tool)] = Some(kiln_common::i18n::tr("로그인 취소 중…").into()));
        }
        self.repaint();
    }

    /// Stop disposable browser-login CLIs before the GUI process exits.
    /// Existing terminal sessions and installed live CLI credentials are untouched.
    pub fn shutdown_logins(&self) -> bool {
        self.inner.shutting_down.store(true, std::sync::atomic::Ordering::Relaxed);
        let jobs: Vec<_> = self.inner.login.lock().iter().flatten().cloned().collect();
        for job in &jobs { job.shutdown(); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        jobs.iter().all(|job| job.wait_for_cleanup(deadline))
    }

    pub fn login_is_cancelling(&self, tool: Tool) -> bool {
        self.inner.login.lock()[idx(tool)].as_ref().is_some_and(|job| job.cancel.load(std::sync::atomic::Ordering::Relaxed))
    }

    pub fn submit_login_code(&self, tool: Tool, code: &str) -> Result<()> {
        let job = self.inner.login.lock()[idx(tool)].clone()
            .ok_or_else(|| anyhow!(kiln_common::i18n::tr("로그인 대기가 종료되었습니다. 다시 시도하세요")))?;
        job.send_code(code)
    }

    /// The browser authenticates the user; no terminal panel or live credential
    /// mutation is needed while waiting. A successful job saves its own snapshot.
    pub fn start_login(&self, tool: Tool, label: &str) {
        let label = label.trim().to_string();
        if label.is_empty() { return; }
        if self.login_status(tool).is_some() || self.with_state(|s| s.busy[idx(tool)].is_some()) { return; }
        let Some(binary) = crate::login::find_cli(&self.inner.env.home, tool) else {
            self.with_state(|s| s.error[idx(tool)] = Some(kiln_common::trf!("{}가 설치되어 있지 않습니다. 설치 후 다시 시도하세요", tool.display_name())));
            return;
        };
        self.start_login_with_binary(tool, label, binary, std::time::Duration::from_secs(300));
    }

    fn start_login_with_binary(&self, tool: Tool, label: String, binary: PathBuf, timeout: std::time::Duration) {
        let control = Arc::new(crate::login::LoginControl::default());
        {
            let mut jobs = self.inner.login.lock();
            if self.inner.shutting_down.load(std::sync::atomic::Ordering::Relaxed) { return; }
            if jobs[idx(tool)].is_some() { return; }
            let reserved = self.with_state(|s| {
                if s.busy[idx(tool)].is_some() { return false; }
                s.busy[idx(tool)] = Some(kiln_common::i18n::tr("브라우저에서 로그인을 완료하세요").to_string());
                s.error[idx(tool)] = None;
                s.notice[idx(tool)] = None;
                true
            });
            if !reserved { return; }
            jobs[idx(tool)] = Some(control.clone());
        }
        let m = self.clone();
        let redraw = m.clone();
        std::thread::spawn(move || {
            let result = crate::login::run_login(&m.inner.env, tool, &binary, &control, timeout, Arc::new(move || redraw.repaint()))
                .and_then(|credentials| {
                    let _op = m.inner.op.lock();
                    if control.cancel.load(std::sync::atomic::Ordering::Relaxed) { bail!(kiln_common::i18n::tr("로그인을 취소했습니다")); }
                    // Saving a second account must not silently replace the account
                    // used by existing agent sessions or revoke its refresh token.
                    let profile = m.save_credentials_locked(tool, &label, credentials.secret, None, credentials.email, credentials.oauth, false)?;
                    Ok(kiln_common::trf!("‘{}’ 계정을 저장했습니다", profile.label))
                });
            m.inner.login.lock()[idx(tool)] = None;
            m.with_state(|s| {
                s.busy[idx(tool)] = None;
                match result {
                    Ok(notice) => s.notice[idx(tool)] = Some(notice),
                    Err(error) => s.error[idx(tool)] = Some(format!("{error:#}")),
                }
            });
            m.repaint();
        });
        self.repaint();
    }

    /// 목록 순서를 `ids` 순서로 바꾼다. 빠진 id 는 원래 순서대로 뒤에 붙는다.
    pub fn set_order(&self, tool: Tool, ids: &[String]) -> Result<()> {
        self.with_state(|s| {
            let ps = &mut s.settings.tool_mut(tool).profiles;
            ps.sort_by_key(|p| ids.iter().position(|i| *i == p.profile.id).unwrap_or(usize::MAX));
        });
        self.persist()
    }

    fn persist(&self) -> Result<()> {
        let snapshot = self.with_state(|s| s.settings.clone());
        kiln_common::store::save_json(&self.inner.env.settings_path, &snapshot)
    }

    fn persist_logged(&self) {
        if let Err(e) = self.persist() {
            log::warn!("accounts: failed to save settings: {e:#}");
        }
    }

    // ---- 현재 자격증명(live) ----

    /// 현재 로그인된 자격증명과 (Claude 키체인이면) account 속성.
    fn read_live(&self, tool: Tool) -> Result<Option<(String, Option<String>)>> {
        let env = &self.inner.env;
        match tool {
            Tool::Claude if env.claude_live_in_store => Ok(env.store.get_by_service(&crate::login::claude_service(env.claude_config_dir.as_deref()))?.map(|(v, a)| (v, Some(a)))),
            Tool::Claude => Ok(read_secret(&env.claude_credentials_path())?.map(|v| (v, None))),
            Tool::Codex => Ok(read_secret(&codex::auth_path(&env.codex_home))?.map(|v| (v, None))),
        }
    }

    fn write_live(&self, tool: Tool, secret: &str) -> Result<()> {
        let env = &self.inner.env;
        match tool {
            Tool::Claude if env.claude_live_in_store => {
                let acct = self
                    .with_state(|s| s.settings.claude.keychain_account.clone())
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| env.username.clone());
                env.store.set(&crate::login::claude_service(env.claude_config_dir.as_deref()), &acct, secret)
            }
            Tool::Claude => write_secret_atomic(&env.claude_credentials_path(), secret.as_bytes()),
            Tool::Codex => write_secret_atomic(&codex::auth_path(&env.codex_home), secret.as_bytes()),
        }
    }

    /// 현재 로그인 계정의 이메일과 (Claude) oauthAccount.
    fn live_identity(&self, tool: Tool, live_secret: Option<&str>) -> (Option<String>, Option<Value>) {
        match tool {
            Tool::Claude => match claude::read_oauth_account(&self.inner.env.claude_json_path()) {
                Ok(Some(oa)) => (claude::oauth_email(&oa), Some(oa)),
                Ok(None) => (None, None),
                Err(e) => {
                    log::warn!("accounts: failed to read oauthAccount: {e:#}");
                    (None, None)
                }
            },
            Tool::Codex => (live_secret.and_then(codex::auth_email), None),
        }
    }

    fn check_live_identity_async(&self) {
        let m = self.clone();
        std::thread::spawn(move || {
            for t in Tool::ALL {
                m.check_live_identity(t);
            }
            m.repaint();
        });
    }

    fn check_live_identity(&self, tool: Tool) {
        let live = self.read_live(tool).ok().flatten();
        let email = if live.is_some() { self.live_identity(tool, live.as_ref().map(|l| l.0.as_str())).0 } else { None };
        self.with_state(|s| s.live_email[idx(tool)] = Some(email));
    }

    // ---- 저장·전환 ----

    /// 현재 로그인된 계정을 프로필로 저장하고 활성으로 표시한다. 같은 이메일이 있으면 그 프로필을 갱신한다.
    pub fn save_current(&self, tool: Tool, label: &str) -> Result<Profile> {
        let _op = self.inner.op.lock();
        let (secret, acct) = self.read_live(tool)?.ok_or_else(|| match tool {
            Tool::Claude => anyhow!(kiln_common::i18n::tr("저장할 Claude Code 계정이 없습니다. ‘새 계정 추가’에서 로그인하세요")),
            Tool::Codex => anyhow!(kiln_common::i18n::tr("저장할 Codex 계정이 없습니다. ‘새 계정 추가’에서 로그인하세요")),
        })?;
        if tool == Tool::Codex && !codex::has_chatgpt_tokens(&secret) {
            bail!(kiln_common::trf!("Codex 가 ChatGPT 계정으로 로그인되어 있지 않습니다(API 키 로그인은 저장하지 않습니다)"));
        }
        let (email, oauth) = self.live_identity(tool, Some(&secret));
        self.save_credentials_locked(tool, label, secret, acct, email, oauth, true)
    }

    fn save_credentials_locked(&self, tool: Tool, label: &str, secret: String, acct: Option<String>, email: Option<String>, oauth: Option<Value>, activate: bool) -> Result<Profile> {
        let service = profile_service(tool);
        let now = now_unix();
        let (previous_settings, previous_live) = self.with_state(|s| (s.settings.clone(), s.live_email[idx(tool)].clone()));

        let existing = email.as_ref().and_then(|e| {
            self.with_state(|s| {
                s.settings
                    .tool(tool)
                    .profiles
                    .iter()
                    .find(|p| p.profile.email.as_deref().is_some_and(|pe| pe.eq_ignore_ascii_case(e)))
                    .map(|p| p.profile.id.clone())
            })
        });
        let id = existing.clone().unwrap_or_else(|| new_id(tool));
        let previous_secret = self.inner.env.store.get(&service, &id)?;
        self.inner.env.store.set(&service, &id, &secret)?;

        let profile = self.with_state(|s| {
            let ts = s.settings.tool_mut(tool);
            if let Some(a) = acct.filter(|a| !a.is_empty()) {
                ts.keychain_account = Some(a);
            }
            if activate {
                ts.active = Some(id.clone());
                ts.active_since = now;
            }
            let profile = if let Some(p) = ts.profiles.iter_mut().find(|p| p.profile.id == id) {
                p.pending_login = !activate;
                if oauth.is_some() {
                    p.oauth_account = oauth.clone();
                }
                p.profile.clone()
            } else {
                let n = ts.profiles.len() + 1;
                let label = label.trim();
                let label = if !label.is_empty() {
                    label.to_string()
                } else if let Some(e) = &email {
                    e.split('@').next().unwrap_or(e).to_string()
                } else {
                    kiln_common::trf!("계정 {n}")
                };
                let p = Profile { id: id.clone(), tool, label, email: email.clone(), added_unix: now as u64 };
                ts.profiles.push(StoredProfile { profile: p.clone(), oauth_account: oauth.clone(), last_usage: None, pending_login: !activate });
                p
            };
            if activate { s.live_email[idx(tool)] = Some(email.clone()); }
            profile
        });
        if let Err(error) = self.persist() {
            self.with_state(|s| { s.settings = previous_settings; s.live_email[idx(tool)] = previous_live; });
            let rollback = if let Some(secret) = previous_secret {
                self.inner.env.store.set(&service, &id, &secret)
            } else { self.inner.env.store.delete(&service, &id).map(|_| ()) };
            if rollback.is_err() { log::warn!("accounts: profile persistence failed; credential rollback requires retry"); }
            return Err(error);
        }
        self.with_state(|s| s.events.push(AccountsEvent::Saved { tool, id: profile.id.clone() }));
        Ok(profile)
    }

    /// 현재 자격증명을 활성 프로필 스냅샷에 되돌려 쓴다(토큰 회전 대응).
    /// 현재 로그인 이메일과 활성 프로필 이메일이 둘 다 있는데 다르면 쓰지 않는다.
    pub fn sync_back(&self, tool: Tool) -> Result<()> {
        let _op = self.inner.op.lock();
        self.sync_back_locked(tool)
    }

    fn sync_back_locked(&self, tool: Tool) -> Result<()> {
        let Some((active, active_email)) = self.with_state(|s| {
            let ts = s.settings.tool(tool);
            let a = ts.active.clone()?;
            let p = ts.profiles.iter().find(|p| p.profile.id == a)?;
            if p.pending_login { return None; }
            Some((a, p.profile.email.clone()))
        }) else {
            return Ok(());
        };
        let Some((secret, _)) = self.read_live(tool)? else {
            return Ok(());
        };
        let (live_email, oauth) = self.live_identity(tool, Some(&secret));
        if let (Some(a), Some(l)) = (&active_email, &live_email)
            && !a.eq_ignore_ascii_case(l)
        {
            log::warn!("accounts: sync-back skipped for {}: logged-in account differs from active profile", tool.key());
            return Ok(());
        }
        self.inner.env.store.set(&profile_service(tool), &active, &secret)?;
        if let Some(oa) = oauth {
            self.with_state(|s| {
                if let Some(p) = s.settings.tool_mut(tool).profiles.iter_mut().find(|p| p.profile.id == active) {
                    p.oauth_account = Some(oa);
                }
            });
        }
        Ok(())
    }

    /// 활성 프로필로 sync-back 한 뒤 `id` 의 자격증명을 현재 자격증명으로 바꾼다.
    pub fn switch_to(&self, tool: Tool, id: &str) -> Result<()> {
        let _op = self.inner.op.lock();
        let Some((email, oauth)) = self.with_state(|s| {
            s.settings.tool(tool).profiles.iter().find(|p| p.profile.id == id).map(|p| (p.profile.email.clone(), p.oauth_account.clone()))
        }) else {
            bail!(kiln_common::trf!("프로필을 찾을 수 없습니다: {id}"));
        };
        if let Err(e) = self.sync_back_locked(tool) {
            log::warn!("accounts: sync-back failed: {e:#}");
        }
        if self.active(tool).as_deref() == Some(id) && !self.pending_login(tool, id) {
            return Ok(());
        }
        let secret = self
            .inner
            .env
            .store
            .get(&profile_service(tool), id)?
            .with_context(|| kiln_common::trf!("프로필 {id} 의 저장된 자격증명이 없습니다"))?;
        self.write_live(tool, &secret)?;
        if tool == Tool::Claude
            && let Some(oa) = &oauth
            && let Err(e) = claude::write_oauth_account(&self.inner.env.claude_json_path(), Some(oa))
        {
            log::warn!("accounts: failed to swap oauthAccount: {e:#}");
        }
        self.with_state(|s| {
            let ts = s.settings.tool_mut(tool);
            ts.active = Some(id.to_string());
            ts.active_since = now_unix();
            if let Some(profile) = ts.profiles.iter_mut().find(|profile| profile.profile.id == id) { profile.pending_login = false; }
            s.live_email[idx(tool)] = Some(email);
            s.events.push(AccountsEvent::Switched { tool, id: id.to_string() });
        });
        self.persist()
    }

    /// 저장된 프로필을 지운다. 사용 중인 프로필은 지울 수 없다.
    pub fn remove(&self, tool: Tool, id: &str) -> Result<()> {
        let _op = self.inner.op.lock();
        if self.active(tool).as_deref() == Some(id) {
            bail!(kiln_common::trf!("사용 중인 계정은 삭제할 수 없습니다. 다른 계정으로 전환한 뒤 삭제하세요"));
        }
        if !self.profiles(tool).iter().any(|p| p.id == id) {
            bail!(kiln_common::trf!("프로필을 찾을 수 없습니다: {id}"));
        }
        self.inner.env.store.delete(&profile_service(tool), id)?;
        self.with_state(|s| {
            s.settings.tool_mut(tool).profiles.retain(|p| p.profile.id != id);
            s.usage.remove(&(tool, id.to_string()));
            s.limited_at.remove(&(tool, id.to_string()));
        });
        self.persist()
    }

    pub fn rename(&self, tool: Tool, id: &str, label: &str) -> Result<()> {
        let label = label.trim();
        if label.is_empty() {
            bail!(kiln_common::trf!("이름은 비울 수 없습니다"));
        }
        let found = self.with_state(|s| {
            s.settings.tool_mut(tool).profiles.iter_mut().find(|p| p.profile.id == id).map(|p| p.profile.label = label.to_string()).is_some()
        });
        if !found {
            bail!(kiln_common::trf!("프로필을 찾을 수 없습니다: {id}"));
        }
        self.persist()
    }

    // ---- 사용량·순환 ----

    /// 계정이 한도에 걸렸다고 표시한다. 새 사용량이 들어오거나 일정 시간이 지나면 풀린다.
    pub fn mark_limited(&self, tool: Tool, id: &str) {
        self.with_state(|s| {
            s.limited_at.insert((tool, id.to_string()), now_unix());
        });
    }

    fn is_exhausted(s: &State, tool: Tool, id: &str, now: i64) -> bool {
        let key = (tool, id.to_string());
        s.usage.get(&key).is_some_and(|u| u.exhausted_at(now)) || s.limited_at.get(&key).is_some_and(|t| now - t < LIMITED_HOLD_SECS)
    }

    /// 활성 계정 다음부터 순서대로 돌며 한도가 소진되지 않은 첫 계정. 활성 계정은 고르지 않는다.
    pub fn next_with_headroom(&self, tool: Tool) -> Option<Profile> {
        let now = now_unix();
        self.with_state(|s| {
            let ts = s.settings.tool(tool);
            let n = ts.profiles.len();
            let start = ts.active.as_ref().and_then(|a| ts.profiles.iter().position(|p| p.profile.id == *a));
            let first = start.map_or(0, |i| i + 1);
            (0..n)
                .map(|k| (first + k) % n)
                .filter(|i| Some(*i) != start)
                .filter(|i| !ts.profiles[*i].pending_login)
                .map(|i| &ts.profiles[i].profile)
                .find(|p| !Self::is_exhausted(s, tool, &p.id, now))
                .cloned()
        })
    }

    /// 활성 계정을 한도 도달로 표시하고 여유 있는 다음 계정으로 전환한다. 전환한 계정을 돌려준다.
    pub fn rotate(&self, tool: Tool) -> Result<Option<Profile>> {
        if let Some(a) = self.active(tool) {
            self.mark_limited(tool, &a);
        }
        let Some(next) = self.next_with_headroom(tool) else {
            return Ok(None);
        };
        self.switch_to(tool, &next.id)?;
        Ok(Some(next))
    }

    fn set_usage(&self, tool: Tool, id: &str, u: Usage) {
        self.with_state(|s| {
            let key = (tool, id.to_string());
            if !u.is_unavailable() {
                s.limited_at.remove(&key);
                if let Some(p) = s.settings.tool_mut(tool).profiles.iter_mut().find(|p| p.profile.id == id) {
                    p.last_usage = Some(u.clone());
                }
            } else if let Some(prev) = s.usage.get(&key)
                && !prev.is_unavailable()
            {
                let mut kept = prev.clone();
                kept.status = u.status.clone();
                s.usage.insert(key, kept);
                return;
            }
            s.usage.insert(key, u);
        });
    }

    /// 사용량을 백그라운드에서 다시 읽는다. 이미 도는 중이면 아무것도 하지 않는다.
    pub fn refresh_usage(&self, tool: Tool) {
        let started = self.with_state(|s| !std::mem::replace(&mut s.refreshing[idx(tool)], true));
        if !started {
            return;
        }
        let m = self.clone();
        std::thread::spawn(move || {
            m.check_live_identity(tool);
            match tool {
                Tool::Claude => m.refresh_claude(),
                Tool::Codex => m.refresh_codex(),
            }
            m.with_state(|s| s.refreshing[idx(tool)] = false);
            m.persist_logged();
            m.repaint();
        });
    }

    /// 사용량 새로 고침이 끝날 때까지 기다린다(테스트용).
    pub fn refresh_usage_blocking(&self, tool: Tool) {
        self.refresh_usage(tool);
        while self.is_refreshing(tool) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn refresh_claude(&self) {
        let active = self.active(Tool::Claude);
        for p in self.profiles(Tool::Claude) {
            let cred = if active.as_deref() == Some(p.id.as_str()) {
                self.read_live(Tool::Claude).map(|o| o.map(|x| x.0))
            } else {
                self.inner.env.store.get(&profile_service(Tool::Claude), &p.id)
            };
            let u = match cred {
                Ok(Some(c)) => match claude::access_token(&c).and_then(|t| (self.inner.env.probe)(&t)) {
                    Ok(u) => u,
                    Err(e) => Usage::unavailable(e),
                },
                Ok(None) => Usage::unavailable("no credentials"),
                Err(e) => Usage::unavailable(e),
            };
            self.set_usage(Tool::Claude, &p.id, u);
            self.repaint();
        }
    }

    fn refresh_codex(&self) {
        let (active, since) = self.with_state(|s| (s.settings.codex.active.clone(), s.settings.codex.active_since));
        let Some(active) = active else { return };
        let now = now_unix();
        match codex::latest_observed(&self.inner.env.codex_home, now) {
            Some(o) if o.at_unix >= since => self.set_usage(Tool::Codex, &active, o.usage),
            _ => {
                if self.usage(Tool::Codex, &active).is_none() {
                    self.set_usage(Tool::Codex, &active, Usage::unavailable(kiln_common::i18n::tr("최근 세션 기록 없음")));
                }
            }
        }
    }

    /// UI 가 아직 가져가지 않은 이벤트를 꺼낸다.
    pub(crate) fn drain_events(&self) -> Vec<AccountsEvent> {
        self.with_state(|s| std::mem::take(&mut s.events))
    }

    /// 백그라운드에서 `f` 를 돌리고, 끝나면 결과를 알림·오류 칸에 남긴다.
    pub(crate) fn run_async(&self, tool: Tool, busy: &str, f: impl FnOnce(&AccountManager) -> Result<String> + Send + 'static) {
        let already = self.with_state(|s| {
            if s.busy[idx(tool)].is_some() {
                return true;
            }
            s.busy[idx(tool)] = Some(busy.to_string());
            s.error[idx(tool)] = None;
            s.notice[idx(tool)] = None;
            false
        });
        if already {
            return;
        }
        let m = self.clone();
        std::thread::spawn(move || {
            let r = f(&m);
            m.with_state(|s| {
                s.busy[idx(tool)] = None;
                match r {
                    Ok(msg) if !msg.is_empty() => s.notice[idx(tool)] = Some(msg),
                    Ok(_) => {}
                    Err(e) => s.error[idx(tool)] = Some(format!("{e:#}")),
                }
            });
            m.repaint();
        });
    }
}

fn new_id(tool: Tool) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{:x}{:02x}", tool.key(), nanos & 0xffff_ffff_ffff, SEQ.fetch_add(1, Ordering::Relaxed) & 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex::tests::fake_auth;
    use crate::store::CredentialStore;

    fn claude_cred(marker: &str) -> String {
        serde_json::json!({"claudeAiOauth": {"accessToken": format!("at-{marker}"), "refreshToken": format!("rt-{marker}"), "expiresAt": 1}}).to_string()
    }

    fn set_claude_identity(env: &Env, email: &str) {
        let p = env.claude_json_path();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut doc: serde_json::Map<String, Value> = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        doc.insert("oauthAccount".into(), serde_json::json!({"emailAddress": email, "accountUuid": email}));
        doc.insert("numStartups".into(), 7.into());
        std::fs::write(&p, serde_json::to_vec(&doc).unwrap()).unwrap();
    }

    /// 키체인(메모리) 모드에서 Claude 로그인 상태를 흉내 낸다.
    fn login_claude(env: &Env, store: &crate::MemoryStore, email: &str, marker: &str) {
        store.set(claude::CLAUDE_SERVICE, "claude-code-user", &claude_cred(marker)).unwrap();
        set_claude_identity(env, email);
    }

    fn login_codex(env: &Env, email: &str, marker: &str) {
        write_secret_atomic(&codex::auth_path(&env.codex_home), fake_auth(email, marker).as_bytes()).unwrap();
    }

    fn live_claude(store: &crate::MemoryStore) -> String {
        store.get_by_service(claude::CLAUDE_SERVICE).unwrap().unwrap().0
    }

    #[test]
    fn save_current_captures_and_dedupes_by_email() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let m = AccountManager::with_env(env.clone());
        assert!(m.save_current(Tool::Claude, "x").is_err());

        login_claude(&env, &store, "a@x.com", "a1");
        let a = m.save_current(Tool::Claude, "work").unwrap();
        assert_eq!(a.label, "work");
        assert_eq!(a.email.as_deref(), Some("a@x.com"));
        assert_eq!(m.active(Tool::Claude).as_deref(), Some(a.id.as_str()));

        login_claude(&env, &store, "A@x.com", "a2");
        let again = m.save_current(Tool::Claude, "other").unwrap();
        assert_eq!(again.id, a.id);
        assert_eq!(again.label, "work");
        assert_eq!(m.profiles(Tool::Claude).len(), 1);
        assert_eq!(store.get(&profile_service(Tool::Claude), &a.id).unwrap(), Some(claude_cred("a2")));

        let raw = std::fs::read_to_string(&env.settings_path).unwrap();
        assert!(!raw.contains("at-a2") && !raw.contains("rt-a2"), "settings must not contain secrets");
        assert!(raw.contains("claude-code-user"));
    }

    #[test]
    #[cfg(unix)]
    fn gui_shutdown_reaps_both_login_children_and_cleans_only_isolated_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let manager = AccountManager::with_env(env.clone());
        login_codex(&env, "active@example.test", "active");
        login_claude(&env, &store, "live@example.test", "live");
        let codex_before = std::fs::read(codex::auth_path(&env.codex_home)).unwrap();
        let claude_before = store.get_by_service(claude::CLAUDE_SERVICE).unwrap();
        let fixture_dir = d.path().to_string_lossy().replace('\'', "'\\''");
        let binary = d.path().join("login-shutdown-fixture");
        std::fs::write(&binary, format!(
            "#!/bin/sh\nfixture_dir='{fixture_dir}'\ncase \"$1\" in auth) fixture_tool=claude;; *) fixture_tool=codex;; esac\nprintf '%s\\n' \"$$\" > \"$fixture_dir/$fixture_tool.pid\"\nprintf '%s\\n' \"$HOME\" > \"$fixture_dir/$fixture_tool.home\"\nexec /bin/sleep 60\n"
        )).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        for tool in Tool::ALL {
            manager.start_login_with_binary(tool, "Disposable".into(), binary.clone(), std::time::Duration::from_secs(90));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut children = Vec::new();
        for name in ["claude", "codex"] {
            let home_file = d.path().join(format!("{name}.home"));
            while std::fs::read_to_string(&home_file).ok().is_none_or(|s| s.trim().is_empty()) {
                assert!(std::time::Instant::now() < deadline, "fixture login did not start");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let home = PathBuf::from(std::fs::read_to_string(home_file).unwrap().trim());
            let isolated = home.parent().unwrap().to_path_buf();
            let pid: i32 = std::fs::read_to_string(d.path().join(format!("{name}.pid"))).unwrap().trim().parse().unwrap();
            children.push((pid, isolated));
        }
        let isolated_claude = children[0].1.join("claude");
        let service = crate::login::claude_service(Some(&isolated_claude));
        store.set(&service, "fixture", "disposable").unwrap();
        let started = std::time::Instant::now();
        assert!(manager.shutdown_logins(), "GUI exit must wait for login cleanup");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        for (pid, isolated) in children {
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "disposable login process survived GUI shutdown");
            assert!(!isolated.exists(), "disposable credential files survived GUI shutdown");
        }
        assert!(store.get_by_service(&service).unwrap().is_none());
        assert_eq!(store.get_by_service(claude::CLAUDE_SERVICE).unwrap(), claude_before);
        assert_eq!(std::fs::read(codex::auth_path(&env.codex_home)).unwrap(), codex_before);
        let marker = d.path().join("codex.pid");
        std::fs::remove_file(&marker).unwrap();
        manager.start_login_with_binary(Tool::Codex, "After exit".into(), binary, std::time::Duration::from_secs(1));
        assert!(!marker.exists(), "a closing GUI must not admit another login");
    }

    #[test]
    #[cfg(unix)]
    fn shutdown_before_worker_launch_completes_without_creating_a_child() {
        let d = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(d.path(), false);
        let control = Arc::new(crate::login::LoginControl::default());
        control.shutdown();
        let result = crate::login::run_login(&env, Tool::Codex, Path::new("/does-not-exist"), &control, std::time::Duration::from_secs(1), Arc::new(|| {}));
        assert!(result.is_err());
        assert!(control.wait_for_cleanup(std::time::Instant::now() + std::time::Duration::from_millis(100)));
    }

    #[test]
    #[cfg(unix)]
    fn background_login_auto_saves_named_profile_and_keeps_active_account() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), false);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "active@example.test", "old");
        let active = m.save_current(Tool::Codex, "Active").unwrap();
        m.drain_events();
        let live_before = std::fs::read(codex::auth_path(&env.codex_home)).unwrap();
        let script = d.path().join("isolated-login-fixture");
        let auth = fake_auth("new@example.test", "new");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s' '{}' > \"$CODEX_HOME/auth.json\"\n", auth)).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        m.start_login_with_binary(Tool::Codex, "Company".into(), script, std::time::Duration::from_secs(3));
        let start = std::time::Instant::now();
        while m.login_status(Tool::Codex).is_some() {
            assert!(start.elapsed() < std::time::Duration::from_secs(4));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let profiles = m.profiles(Tool::Codex);
        assert_eq!(profiles.len(), 2);
        let added = profiles.iter().find(|profile| profile.label == "Company").unwrap();
        assert_eq!(added.email.as_deref(), Some("new@example.test"));
        assert_eq!(m.active(Tool::Codex).as_deref(), Some(active.id.as_str()));
        assert_eq!(std::fs::read(codex::auth_path(&env.codex_home)).unwrap(), live_before);
        assert_eq!(store.get(&profile_service(Tool::Codex), &added.id).unwrap(), Some(auth));
        assert!(m.next_with_headroom(Tool::Codex).is_none(), "automatic rotation must wait for explicit Apply of a new browser login");
        let events = m.drain_events();
        assert!(events.iter().any(|event| matches!(event, AccountsEvent::Saved { id, .. } if id == &added.id)));
        assert!(!events.iter().any(|event| matches!(event, AccountsEvent::RunLogin(_))));
        let settings = std::fs::read_to_string(&env.settings_path).unwrap();
        assert!(!settings.contains("access-new") && !settings.contains("refresh-new"));
    }

    #[test]
    fn imported_profile_storage_failure_rolls_back_profile_and_secret() {
        let d = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(d.path(), false);
        // A regular file as parent makes atomic settings persistence fail.
        let blocked = d.path().join("not-a-directory");
        std::fs::write(&blocked, b"fixture").unwrap();
        env.settings_path = blocked.join("accounts.json");
        let m = AccountManager::with_env(env);
        let error = m.save_credentials_locked(Tool::Codex, "Company", fake_auth("new@example.test", "new"), None, Some("new@example.test".into()), None, false);
        assert!(error.is_err());
        assert!(m.profiles(Tool::Codex).is_empty());
        assert!(m.active(Tool::Codex).is_none());
        assert!(store.is_empty());
        assert!(m.drain_events().is_empty());
    }

    #[test]
    fn browser_reauthentication_is_kept_until_explicit_apply_even_for_active_profile() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), false);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "same@example.test", "old");
        let profile = m.save_current(Tool::Codex, "Personal").unwrap();
        let fresh = fake_auth("same@example.test", "fresh");
        let imported = m.save_credentials_locked(Tool::Codex, "Personal", fresh.clone(), None, Some("same@example.test".into()), None, false).unwrap();
        assert_eq!(imported.id, profile.id);
        assert!(m.pending_login(Tool::Codex, &profile.id));
        m.sync_back(Tool::Codex).unwrap();
        assert_eq!(store.get(&profile_service(Tool::Codex), &profile.id).unwrap(), Some(fresh.clone()), "live token sync must not erase the new isolated login");
        assert_eq!(std::fs::read_to_string(codex::auth_path(&env.codex_home)).unwrap(), fake_auth("same@example.test", "old"));
        m.switch_to(Tool::Codex, &profile.id).unwrap();
        assert_eq!(std::fs::read_to_string(codex::auth_path(&env.codex_home)).unwrap(), fresh);
        assert!(!m.pending_login(Tool::Codex, &profile.id));
    }

    #[test]
    fn switch_syncs_back_rotated_tokens_then_restores_target() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let m = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@x.com", "a1");
        let a = m.save_current(Tool::Claude, "").unwrap();
        login_claude(&env, &store, "b@x.com", "b1");
        let b = m.save_current(Tool::Claude, "").unwrap();
        assert_eq!(b.label, "b");

        // B 로 쓰는 동안 토큰이 회전했다.
        store.set(claude::CLAUDE_SERVICE, "claude-code-user", &claude_cred("b2")).unwrap();
        m.switch_to(Tool::Claude, &a.id).unwrap();
        assert_eq!(store.get(&profile_service(Tool::Claude), &b.id).unwrap(), Some(claude_cred("b2")));
        assert_eq!(live_claude(&store), claude_cred("a1"));
        assert_eq!(store.get_by_service(claude::CLAUDE_SERVICE).unwrap().unwrap().1, "claude-code-user");
        let oa = claude::read_oauth_account(&env.claude_json_path()).unwrap().unwrap();
        assert_eq!(claude::oauth_email(&oa).as_deref(), Some("a@x.com"));
        let doc: Value = serde_json::from_slice(&std::fs::read(env.claude_json_path()).unwrap()).unwrap();
        assert_eq!(doc["numStartups"], 7);
        assert_eq!(m.active(Tool::Claude).as_deref(), Some(a.id.as_str()));

        // 다시 B 로: A 에서 회전한 토큰을 A 에 되돌려 쓰고 B 의 최신 토큰을 복원한다.
        store.set(claude::CLAUDE_SERVICE, "claude-code-user", &claude_cred("a3")).unwrap();
        m.switch_to(Tool::Claude, &b.id).unwrap();
        assert_eq!(store.get(&profile_service(Tool::Claude), &a.id).unwrap(), Some(claude_cred("a3")));
        assert_eq!(live_claude(&store), claude_cred("b2"));
    }

    #[test]
    fn sync_back_skips_when_logged_in_account_differs() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let m = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@x.com", "a1");
        let a = m.save_current(Tool::Claude, "").unwrap();
        // 저장하지 않은 C 계정으로 새로 로그인했다.
        login_claude(&env, &store, "c@x.com", "c1");
        m.sync_back(Tool::Claude).unwrap();
        assert_eq!(store.get(&profile_service(Tool::Claude), &a.id).unwrap(), Some(claude_cred("a1")));
    }

    #[test]
    fn claude_file_mode_uses_credentials_json() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), false);
        let m = AccountManager::with_env(env.clone());
        let cred_path = env.claude_credentials_path();
        write_secret_atomic(&cred_path, claude_cred("a1").as_bytes()).unwrap();
        set_claude_identity(&env, "a@x.com");
        let a = m.save_current(Tool::Claude, "").unwrap();
        write_secret_atomic(&cred_path, claude_cred("b1").as_bytes()).unwrap();
        set_claude_identity(&env, "b@x.com");
        let b = m.save_current(Tool::Claude, "").unwrap();
        m.switch_to(Tool::Claude, &a.id).unwrap();
        assert_eq!(std::fs::read_to_string(&cred_path).unwrap(), claude_cred("a1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&cred_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(store.get_by_service(claude::CLAUDE_SERVICE).unwrap().is_none());
        assert_eq!(store.get(&profile_service(Tool::Claude), &b.id).unwrap(), Some(claude_cred("b1")));
    }

    #[test]
    fn codex_save_switch_swaps_auth_json() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let m = AccountManager::with_env(env.clone());
        let auth = codex::auth_path(&env.codex_home);
        login_codex(&env, "one@x.com", "o1");
        let one = m.save_current(Tool::Codex, "").unwrap();
        assert_eq!(one.email.as_deref(), Some("one@x.com"));
        login_codex(&env, "two@x.com", "t1");
        let two = m.save_current(Tool::Codex, "개인").unwrap();
        assert_eq!(two.label, "개인");

        login_codex(&env, "two@x.com", "t2");
        m.switch_to(Tool::Codex, &one.id).unwrap();
        assert_eq!(std::fs::read_to_string(&auth).unwrap(), fake_auth("one@x.com", "o1"));
        assert_eq!(store.get(&profile_service(Tool::Codex), &two.id).unwrap(), Some(fake_auth("two@x.com", "t2")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&auth).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&auth, r#"{"OPENAI_API_KEY":"sk-test"}"#).unwrap();
        assert!(m.save_current(Tool::Codex, "").is_err());
    }

    #[test]
    fn remove_and_rename() {
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), true);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "one@x.com", "o1");
        let one = m.save_current(Tool::Codex, "").unwrap();
        login_codex(&env, "two@x.com", "t1");
        let two = m.save_current(Tool::Codex, "").unwrap();
        assert!(m.remove(Tool::Codex, &two.id).is_err());
        m.rename(Tool::Codex, &one.id, "  회사  ").unwrap();
        assert!(m.rename(Tool::Codex, &one.id, " ").is_err());
        assert_eq!(m.profiles(Tool::Codex)[0].label, "회사");
        m.remove(Tool::Codex, &one.id).unwrap();
        assert_eq!(store.get(&profile_service(Tool::Codex), &one.id).unwrap(), None);
        assert_eq!(m.profiles(Tool::Codex).len(), 1);

        let reloaded = AccountManager::with_env(env);
        assert_eq!(reloaded.profiles(Tool::Codex), m.profiles(Tool::Codex));
        assert_eq!(reloaded.active(Tool::Codex).as_deref(), Some(two.id.as_str()));
    }

    fn three_codex(d: &Path) -> (AccountManager, Vec<Profile>) {
        let (env, _store) = Env::sandbox(d, true);
        let m = AccountManager::with_env(env.clone());
        let ps: Vec<Profile> = ["a", "b", "c"]
            .iter()
            .map(|n| {
                login_codex(&env, &format!("{n}@x.com"), n);
                m.save_current(Tool::Codex, n).unwrap()
            })
            .collect();
        (m, ps)
    }

    #[test]
    fn next_with_headroom_round_robin_skips_exhausted() {
        let d = tempfile::tempdir().unwrap();
        let (m, ps) = three_codex(d.path());
        let now = now_unix();
        m.switch_to(Tool::Codex, &ps[0].id).unwrap();
        assert_eq!(m.next_with_headroom(Tool::Codex).unwrap().id, ps[1].id);

        m.set_usage(Tool::Codex, &ps[1].id, Usage { five_hour: Some((1.0, Some(now + 600))), seven_day: None, status: "allowed".into() });
        assert_eq!(m.next_with_headroom(Tool::Codex).unwrap().id, ps[2].id);

        m.switch_to(Tool::Codex, &ps[2].id).unwrap();
        assert_eq!(m.next_with_headroom(Tool::Codex).unwrap().id, ps[0].id);
        m.mark_limited(Tool::Codex, &ps[0].id);
        assert_eq!(m.next_with_headroom(Tool::Codex), None);

        // 창이 초기화된 계정은 다시 후보가 된다.
        m.set_usage(Tool::Codex, &ps[1].id, Usage { five_hour: Some((1.0, Some(now - 1))), seven_day: None, status: "allowed".into() });
        assert_eq!(m.next_with_headroom(Tool::Codex).unwrap().id, ps[1].id);
    }

    #[test]
    fn rotate_marks_active_and_switches() {
        let d = tempfile::tempdir().unwrap();
        let (m, ps) = three_codex(d.path());
        assert_eq!(m.active(Tool::Codex).as_deref(), Some(ps[2].id.as_str()));
        let next = m.rotate(Tool::Codex).unwrap().unwrap();
        assert_eq!(next.id, ps[0].id);
        let next = m.rotate(Tool::Codex).unwrap().unwrap();
        assert_eq!(next.id, ps[1].id);
        assert_eq!(m.rotate(Tool::Codex).unwrap(), None);
        let codex_auth = std::fs::read_to_string(codex::auth_path(&m.env().codex_home)).unwrap();
        assert_eq!(codex::auth_email(&codex_auth).as_deref(), Some("b@x.com"));
    }

    #[test]
    fn refresh_claude_uses_probe_per_profile() {
        let d = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(d.path(), true);
        env.probe = Arc::new(|tok: &str| {
            let util = if tok == "at-a1" { 0.25 } else { 1.0 };
            Ok(Usage { five_hour: Some((util, Some(4_000_000_000))), seven_day: Some((0.5, None)), status: "allowed".into() })
        });
        let m = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@x.com", "a1");
        let a = m.save_current(Tool::Claude, "").unwrap();
        login_claude(&env, &store, "b@x.com", "b1");
        let b = m.save_current(Tool::Claude, "").unwrap();
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(m.usage(Tool::Claude, &a.id).unwrap().five_hour.unwrap().0, 0.25);
        assert_eq!(m.usage(Tool::Claude, &b.id).unwrap().five_hour.unwrap().0, 1.0);
        assert_eq!(m.next_with_headroom(Tool::Claude).unwrap().id, a.id);
        m.switch_to(Tool::Claude, &a.id).unwrap();
        assert_eq!(m.next_with_headroom(Tool::Claude), None);
        assert_eq!(m.live_email(Tool::Claude), Some(Some("a@x.com".into())));
    }

    #[test]
    fn refresh_codex_attributes_rollout_to_active_after_switch() {
        let d = tempfile::tempdir().unwrap();
        let (m, ps) = three_codex(d.path());
        let day = m.env().codex_home.join("sessions/2099/01/01");
        std::fs::create_dir_all(&day).unwrap();
        let line = serde_json::json!({"timestamp": "2099-01-01T00:00:00Z", "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {"primary": {"used_percent": 30.0, "window_minutes": 300, "resets_at": 4_200_000_000i64}, "secondary": {"used_percent": 70.0, "window_minutes": 10080, "resets_at": 4_200_000_000i64}, "plan_type": "plus"}}});
        std::fs::write(day.join("rollout-x.jsonl"), format!("{line}\n")).unwrap();
        m.refresh_usage_blocking(Tool::Codex);
        let u = m.usage(Tool::Codex, &ps[2].id).unwrap();
        assert_eq!(u.five_hour, Some((0.3, Some(4_200_000_000))));
        assert_eq!(u.seven_day.unwrap().0, 0.7);
        assert!(m.usage(Tool::Codex, &ps[0].id).is_none());
    }

    #[test]
    fn settings_persist_auto_rotate_and_order() {
        let d = tempfile::tempdir().unwrap();
        let (m, ps) = three_codex(d.path());
        m.set_auto_rotate(Tool::Codex, true);
        m.set_order(Tool::Codex, &[ps[2].id.clone(), ps[0].id.clone()]).unwrap();
        let r = AccountManager::with_env(m.env().clone());
        assert!(r.auto_rotate(Tool::Codex));
        assert!(!r.auto_rotate(Tool::Claude));
        let ids: Vec<String> = r.profiles(Tool::Codex).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![ps[2].id.clone(), ps[0].id.clone(), ps[1].id.clone()]);
    }
}
