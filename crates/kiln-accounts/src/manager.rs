//! 계정 저장·전환·사용량 조회. 상태는 `Arc` 안에 두고, 느린 작업은 백그라운드 스레드에서 돈다.

use crate::store::{CredentialStore, FileStore, SecurityCli, read_secret, write_secret_atomic};
use crate::ui::{AccountsEvent, UiState};
use crate::{Profile, Tool, Usage, claude, claude_refresh, codex, now_unix};
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
    /// None keeps the offline rollout reader available in isolated fixtures.
    pub codex_probe: Option<UsageProbe>,
    pub codex_native_probe: Option<Arc<dyn Fn(&Path, &str) -> Result<Usage> + Send + Sync>>,
    /// Sends a Claude refresh grant to the CLI's token endpoint.
    pub(crate) claude_renew: Arc<dyn Fn(&claude_refresh::Grant) -> Result<claude_refresh::Renewed> + Send + Sync>,
    /// 키체인 account 속성을 모를 때 쓰는 사용자 이름.
    pub username: String,
}

/// The credential store outside macOS: protected files and the Credential Manager on
/// Windows, owner-only files elsewhere.
fn platform_store(files: FileStore) -> Arc<dyn CredentialStore> {
    #[cfg(windows)]
    return Arc::new(crate::store::WindowsStore { files });
    #[cfg(not(windows))]
    Arc::new(files)
}

impl Env {
    /// 실제 홈 디렉토리·키체인(macOS) 또는 0600 파일(그 밖)·`kiln_common` 설정 경로.
    pub fn real() -> Self {
        let home = kiln_common::paths::home().unwrap_or_else(std::env::temp_dir);
        let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
        let codex_env = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
        let codex_home = codex::codex_home(&home, codex_env.as_deref());
        let macos = cfg!(target_os = "macos");
        let files = FileStore { base_dir: kiln_common::paths::config_file("accounts-credentials") };
        let store: Arc<dyn CredentialStore> = if macos { Arc::new(SecurityCli) } else { platform_store(files) };
        Env {
            home,
            claude_config_dir,
            codex_home,
            claude_live_in_store: macos,
            store,
            settings_path: kiln_common::paths::config_file("accounts.json"),
            probe: Arc::new(|token: &str| claude::probe_usage(token)),
            codex_probe: Some(Arc::new(codex::probe_usage)),
            codex_native_probe: Some(Arc::new(codex::probe_native)),
            claude_renew: Arc::new(claude_refresh::request),
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
            codex_probe: None,
            codex_native_probe: None,
            claude_renew: Arc::new(|_| Err(anyhow!("renewal disabled"))),
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

    /// Match the CLI's Keychain primary / private-file fallback. An inaccessible
    /// primary is not evidence that a historical file token is current.
    pub(crate) fn claude_secret(&self, config: Option<&Path>) -> Result<Option<(String, Option<String>)>> {
        if self.claude_live_in_store {
            let service = crate::login::claude_service(config);
            if let Some(secret) = self.store.get(&service, &self.username)? {
                return Ok(Some((secret, Some(self.username.clone()))));
            }
            if let Some((secret, account)) = self.store.get_by_service(&service)? {
                return Ok(Some((secret, Some(account))));
            }
        }
        Ok(read_secret(&claude::credentials_path(&self.home, config))?.map(|secret| (secret, None)))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_usage_at: Option<i64>,
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
    usage_poll: HashMap<(Tool, String), UsagePoll>,
    settings_visible: bool,
    settings: Settings,
    usage: HashMap<(Tool, String), Usage>,
    limited_at: HashMap<(Tool, String), i64>,
    login_required: std::collections::HashSet<(Tool, String)>,
    /// Logins whose refresh token was rejected, by the digest of their access token.
    renewal_rejected: HashMap<(Tool, String), Vec<u8>>,
    login_checked_at: Option<std::time::Instant>,
    pub(crate) refreshing: [bool; 2],
    pub(crate) busy: [Option<String>; 2],
    pub(crate) error: [Option<String>; 2],
    pub(crate) notice: [Option<String>; 2],
    /// 현재 로그인된 계정 이메일(백그라운드에서 읽는다). 바깥 `None` 은 아직 모름.
    pub(crate) live_email: [Option<Option<String>>; 2],
    pub(crate) events: Vec<AccountsEvent>,
}

/// Where the credentials of a usage read are kept.
#[derive(Clone, Debug, PartialEq)]
enum UsageSource {
    /// A profile's runtime folder, which Claude Code panels of that profile renew.
    Runtime(PathBuf),
    /// The live CLI configuration of the active profile.
    Live,
    /// Kiln's saved copy, which no CLI renews.
    Snapshot,
}

/// Attempts to take the CLI's refresh locks before a renewal waits for the next round.
const RENEWAL_LOCK_ATTEMPTS: u32 = if cfg!(test) { 1 } else { 5 };

#[derive(Clone, Default)]
struct UsagePoll {
    fingerprint: Vec<u8>,
    next_at: i64,
    blocked_until: i64,
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
    usage_worker_started: std::sync::atomic::AtomicBool,
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
        m.start_usage_updates();
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
                usage_worker_started: std::sync::atomic::AtomicBool::new(false),
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
        self.login_fixture_for_profile(tool, status, None)
    }

    #[cfg(test)]
    pub(crate) fn login_fixture_for_profile(&self, tool: Tool, status: crate::LoginStatus, profile_id: Option<String>) -> Arc<crate::login::LoginControl> {
        self.ui_state().feedback_profile[idx(tool)] = profile_id.clone();
        let mut control = crate::login::LoginControl::default();
        control.profile_id = profile_id;
        let control = Arc::new(control);
        *control.status.lock() = status;
        self.inner.login.lock()[idx(tool)] = Some(control.clone());
        self.with_state(|s| s.busy[idx(tool)] = Some(kiln_common::i18n::tr("브라우저에서 로그인을 완료하세요").into()));
        control
    }

    pub fn login_profile(&self, tool: Tool) -> Option<Profile> {
        let id = self.inner.login.lock()[idx(tool)].as_ref()?.profile_id.clone()?;
        self.profiles(tool).into_iter().find(|p| p.id == id)
    }

    pub fn login_required(&self, tool: Tool, id: &str) -> bool {
        self.with_state(|s| s.login_required.contains(&(tool, id.to_string())))
    }

    /// Local credential presence, not a remote login guarantee. Never run a
    /// provider request or Keychain operation on the render thread.
    pub fn refresh_login_status(&self) {
        let started = self.with_state(|s| {
            if s.login_checked_at.is_some_and(|at| at.elapsed().as_secs() < 15) { return false; }
            s.login_checked_at = Some(std::time::Instant::now());
            true
        });
        if !started { return; }
        self.repaint();
        let m = self.clone();
        std::thread::spawn(move || { m.check_profile_credentials(); m.repaint(); });
    }

    fn check_profile_credentials(&self) {
        let _op = self.inner.op.lock();
        for tool in Tool::ALL {
            for p in self.profiles(tool) {
                // An unapplied successful login is ready for Apply, not logged out.
                let secret = if self.pending_login(tool, &p.id) {
                    self.inner.env.store.get(&profile_service(tool), &p.id)
                } else if self.runtime_dir(tool, &p.id).join("initialized").exists() {
                    self.runtime_secret(tool, &p.id)
                } else { self.inner.env.store.get(&profile_service(tool), &p.id) };
                if let Ok(secret) = secret {
                    let missing = secret.as_deref().is_none_or(|secret| match tool {
                        Tool::Claude => claude::access_token(secret).is_err(),
                        // Missing identity and unsupported auth methods are not logout.
                        Tool::Codex => codex::credentials_missing(secret),
                    });
                    let digest = secret.as_deref().and_then(|secret| claude::access_token(secret).ok()).map(|token| {
                        use sha2::{Digest, Sha256};
                        Sha256::digest(token.as_bytes()).to_vec()
                    });
                    self.with_state(|s| {
                        let key = (tool, p.id.clone());
                        let rejected = s.renewal_rejected.get(&key).is_some_and(|rejected| Some(rejected) == digest.as_ref());
                        if !rejected { s.renewal_rejected.remove(&key); }
                        if missing || rejected { s.login_required.insert(key); } else { s.login_required.remove(&key); }
                    });
                }
                // An inaccessible Keychain or network error is not proof of logout.
            }
        }
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
        self.start_login_job(tool, label, None, binary, timeout);
    }

    pub fn start_relogin(&self, tool: Tool, id: &str) {
        if self.login_status(tool).is_some() || self.with_state(|s| s.busy[idx(tool)].is_some()) { return; }
        let Some(profile) = self.profiles(tool).into_iter().find(|p| p.id == id) else { return; };
        self.ui_state().feedback_profile[idx(tool)] = Some(id.to_string());
        let Some(binary) = crate::login::find_cli(&self.inner.env.home, tool) else {
            self.with_state(|s| s.error[idx(tool)] = Some(kiln_common::trf!("{}가 설치되어 있지 않습니다. 설치 후 다시 시도하세요", tool.display_name())));
            return;
        };
        self.start_login_job(tool, profile.label, Some(id.to_string()), binary, std::time::Duration::from_secs(300));
    }

    fn save_relogin_locked(&self, tool: Tool, id: &str, credentials: crate::login::LoginCredentials) -> Result<Profile> {
        let profile = self.profiles(tool).into_iter().find(|p| p.id == id)
            .ok_or_else(|| anyhow!(kiln_common::i18n::tr("다시 로그인할 프로필을 찾을 수 없습니다")))?;
        let matches = profile.email.as_ref().zip(credentials.email.as_ref())
            .is_some_and(|(old, new)| old.eq_ignore_ascii_case(new));
        if !matches { bail!("{}", kiln_common::i18n::tr("선택한 프로필과 다른 계정입니다. 기존 계정으로 다시 로그인하세요")); }
        let matching_ids = self.profiles(tool).into_iter()
            .filter(|p| p.email.as_ref().zip(credentials.email.as_ref()).is_some_and(|(old, new)| old.eq_ignore_ascii_case(new)))
            .map(|p| p.id).collect::<Vec<_>>();
        if matching_ids != [id.to_string()] { bail!("{}", kiln_common::i18n::tr("다시 로그인할 프로필을 찾을 수 없습니다")); }
        // Pin the original profile before mutating credentials. Same-account
        // browser sign-in is staged until the user's explicit Apply action.
        let saved = self.save_credentials_locked(tool, &profile.label, credentials.secret, None, credentials.email, credentials.oauth, false)?;
        self.with_state(|s| s.login_required.remove(&(tool, id.to_string())));
        Ok(saved)
    }

    fn start_login_job(&self, tool: Tool, label: String, profile_id: Option<String>, binary: PathBuf, timeout: std::time::Duration) {
        let mut control = crate::login::LoginControl::default();
        control.profile_id = profile_id.clone();
        let control = Arc::new(control);
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
            self.ui_state().feedback_profile[idx(tool)] = profile_id.clone();
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
                    if let Some(id) = profile_id {
                        let profile = m.save_relogin_locked(tool, &id, credentials)?;
                        Ok(kiln_common::trf!("‘{}’ 로그인 완료. ‘적용’을 누르면 갱신됩니다", profile.label))
                    } else {
                        let profile = m.save_credentials_locked(tool, &label, credentials.secret, None, credentials.email, credentials.oauth, false)?;
                        Ok(kiln_common::trf!("‘{}’ 계정을 저장했습니다", profile.label))
                    }
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
            Tool::Claude => env.claude_secret(env.claude_config_dir.as_deref()),
            Tool::Codex => Ok(codex::read_credentials(&env.codex_home, env.store.as_ref())?.map(|v| (v, None))),
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
            m.check_profile_credentials();
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
                ts.profiles.push(StoredProfile { profile: p.clone(), oauth_account: oauth.clone(), last_usage: None, last_usage_at: None, pending_login: !activate });
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
        // Explicit sign-in is the only operation allowed to replace an existing
        // runtime credential. Ordinary launches must preserve CLI refreshes.
        if activate && self.runtime_dir(tool, &profile.id).is_dir() {
            if let Err(error)=self.write_runtime(tool, &profile.id, &secret, oauth.as_ref()) {
                self.with_state(|s| {s.settings=previous_settings;s.live_email[idx(tool)]=previous_live;});
                if let Some(previous)=previous_secret {self.inner.env.store.set(&service,&id,&previous)?;} else {self.inner.env.store.delete(&service,&id)?;}
                self.persist()?;
                return Err(error);
            }
        }
        self.with_state(|s| s.events.push(AccountsEvent::Saved { tool, id: profile.id.clone() }));
        Ok(profile)
    }

    /// Persistent CLI-owned credentials: bootstrap once, then let the CLI refresh them.
    pub fn launch_environment(&self, tool: Tool) -> Result<Vec<(String, String)>> {
        let _op = self.inner.op.lock();
        let Some(id) = self.active(tool) else { return Ok(Vec::new()); };
        let dir = self.runtime_dir(tool, &id);
        let env = &self.inner.env;
        let credential = self.runtime_secret(tool, &id)?;
        if credential.is_none() {
            if self.pending_login(tool, &id) { bail!("{}", kiln_common::i18n::tr("로그인 갱신을 적용한 후 새 세션을 여세요")); }
            // An initialized directory with missing credentials means logout, not a
            // request to restore an old refresh token from a historical snapshot.
            if dir.join("initialized").exists() { bail!("{}", kiln_common::i18n::tr("이 계정은 다시 로그인해야 합니다.")); }
            let secret = env.store.get(&profile_service(tool), &id)?.context("saved account credentials missing")?;
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?; }
            let source = match tool { Tool::Claude => env.claude_config_dir.clone().unwrap_or_else(||env.home.join(".claude")), Tool::Codex => env.codex_home.clone() };
            for file in match tool { Tool::Claude => &["settings.json", "CLAUDE.md"][..], Tool::Codex => &["config.toml", "AGENTS.md"][..] } {
                if !dir.join(file).exists() && let Ok(bytes) = std::fs::read(source.join(file)) { write_secret_atomic(&dir.join(file), &bytes)?; }
            }
            if tool == Tool::Claude {
                let oauth = self.with_state(|s|s.settings.claude.profiles.iter().find(|p|p.profile.id==id).and_then(|p|p.oauth_account.clone()));
                claude::write_oauth_account(&claude::config_path(&dir), oauth.as_ref())?;
                if env.claude_live_in_store { env.store.set(&crate::login::claude_service(Some(&dir)), &env.username, &secret)?; }
                else { write_secret_atomic(&dir.join(".credentials.json"), secret.as_bytes())?; }
            } else { write_secret_atomic(&dir.join("auth.json"), secret.as_bytes())?; }
            write_secret_atomic(&dir.join("initialized"), b"1")?;
        }
        let mut variables = vec![(match tool { Tool::Claude=>"CLAUDE_CONFIG_DIR",Tool::Codex=>"CODEX_HOME" }.into(), dir.to_string_lossy().into_owned()), ("KILN_AGENT_PROFILE".into(), id.clone())];
        if tool == Tool::Codex && !dir.join("auth.json").exists() {
            // A legacy/manual CLI may have migrated this namespace to Keyring.
            // Preserve its current store; never seed a second refresh-token copy.
            self.usage_secret(tool, &id)?.context("account identity unavailable")?;
            variables.push(("KILN_CODEX_AUTH_STORE".into(), "keyring".into()));
        }
        Ok(variables)
    }

    /// Validate the original saved account before interrupting a running panel.
    pub fn validate_launch_environment(&self,tool:Tool,variables:&[(String,String)])->Result<()> {
        let id=variables.iter().find(|(key,_)|key=="KILN_AGENT_PROFILE").map(|(_,value)|value).context("original account not recorded")?;
        if !self.profiles(tool).iter().any(|p|p.id==*id) {bail!("original account was removed");}
        let key=if tool==Tool::Claude {"CLAUDE_CONFIG_DIR"}else{"CODEX_HOME"};
        let config=variables.iter().find(|(k,_)|k==key).map(|(_,v)|PathBuf::from(v)).context("original account configuration missing")?;
        if config!=self.runtime_dir(tool,id) {bail!("account configuration changed");}
        Ok(())
    }

    fn runtime_dir(&self, tool: Tool, id: &str) -> PathBuf {
        // IDs are generated by Kiln, never accepted as a filesystem path.
        self.inner.env.settings_path.parent().unwrap().join("agent-accounts").join(tool.key()).join(id)
    }

    fn runtime_oauth(&self,tool:Tool,id:&str)->Result<Option<Value>> {
        if tool!=Tool::Claude {return Ok(None);}
        let path=claude::config_path(&self.runtime_dir(tool,id));
        match std::fs::metadata(&path) {
            Ok(_)=>claude::read_oauth_account(&path),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),
            Err(e)=>Err(e.into()),
        }
    }

    fn restore_runtime(&self, tool: Tool, id: &str, secret: Option<&str>, oauth: Option<&Value>) -> Result<()> {
        let dir=self.runtime_dir(tool,id);
        if tool==Tool::Claude {claude::write_oauth_account(&claude::config_path(&dir),oauth)?;}
        if tool==Tool::Claude && self.inner.env.claude_live_in_store {
            let service=crate::login::claude_service(Some(&dir));
            if let Some(secret)=secret {self.inner.env.store.set(&service,&self.inner.env.username,secret)?;}
            else {self.inner.env.store.delete(&service,&self.inner.env.username)?;}
            // Preserve the CLI's private-file fallback if Keychain refresh used it.
            let fallback=dir.join(".credentials.json");
            if fallback.exists() {
                if let Some(secret)=secret {write_secret_atomic(&fallback,secret.as_bytes())?;}
                else {std::fs::remove_file(fallback)?;}
            }
        } else {
            let path=dir.join(if tool==Tool::Claude {".credentials.json"} else {"auth.json"});
            if let Some(secret)=secret {write_secret_atomic(&path,secret.as_bytes())?;}
            else if path.exists() {std::fs::remove_file(path)?;}
        }
        Ok(())
    }

    fn write_runtime(&self, tool: Tool, id: &str, secret: &str, oauth: Option<&Value>) -> Result<()> {
        let dir=self.runtime_dir(tool,id);
        let old=self.runtime_secret(tool,id)?;
        let old_oauth=self.runtime_oauth(tool,id)?;
        let result=(|| {self.restore_runtime(tool,id,Some(secret),oauth)?;write_secret_atomic(&dir.join("initialized"),b"1")})();
        if let Err(error)=result {
            self.restore_runtime(tool,id,old.as_deref(),old_oauth.as_ref())?;
            return Err(error);
        }
        Ok(())
    }

    fn runtime_secret(&self, tool: Tool, id: &str) -> Result<Option<String>> {
        let dir = self.runtime_dir(tool, id);
        if !dir.is_dir() { return Ok(None); }
        match tool {
            Tool::Claude => Ok(self.inner.env.claude_secret(Some(&dir))?.map(|x| x.0)),
            Tool::Codex => {
                // Managed panels pin file storage; prefer that file, then inspect
                // CLI-owned Keyring configuration for legacy/manual sessions.
                if let Some(secret) = read_secret(&dir.join("auth.json"))? { return Ok(Some(secret)); }
                codex::read_credentials(&dir, self.inner.env.store.as_ref())
            },
        }
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
        if let Some(secret) = self.runtime_secret(tool, &active)? {
            self.inner.env.store.set(&profile_service(tool), &active, &secret)?;
            return Ok(());
        }
        if self.runtime_dir(tool, &active).join("initialized").exists() { return Ok(()); }
        let Some((secret, _)) = self.read_live(tool)? else { return Ok(()); };
        let (live_email, oauth) = self.live_identity(tool, Some(&secret));
        if active_email.as_ref().is_none_or(|a| live_email.as_ref().is_none_or(|l|!a.eq_ignore_ascii_case(l))) {
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

    fn restore_live(&self,tool:Tool,previous:Option<&str>)->Result<()> {
        if let Some(secret)=previous {return self.write_live(tool,secret);}
        let env=&self.inner.env;
        if tool==Tool::Claude && env.claude_live_in_store {
            let service=crate::login::claude_service(env.claude_config_dir.as_deref());
            if let Some((_,account))=env.store.get_by_service(&service)? {env.store.delete(&service,&account)?;}
        } else {
            let path=if tool==Tool::Claude {env.claude_credentials_path()} else {codex::auth_path(&env.codex_home)};
            if path.exists() {std::fs::remove_file(path)?;}
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
        self.sync_back_locked(tool)?;
        if self.active(tool).as_deref() == Some(id) && !self.pending_login(tool, id) {
            return Ok(());
        }
        let pending=self.pending_login(tool,id);
        let secret = if pending {self.inner.env.store.get(&profile_service(tool),id)?.context("new sign-in credentials missing")?}
            else if let Some(secret)=self.runtime_secret(tool,id)? {secret}
            else if self.runtime_dir(tool,id).join("initialized").exists() {bail!("{}",kiln_common::i18n::tr("이 계정은 다시 로그인해야 합니다."));}
            else {self.inner.env.store.get(&profile_service(tool),id)?.with_context(||kiln_common::trf!("프로필 {id} 의 저장된 자격증명이 없습니다"))?};
        let previous_runtime=self.runtime_secret(tool,id)?;
        let previous_runtime_oauth=self.runtime_oauth(tool,id)?;
        let previous = self.read_live(tool)?;
        let previous_oauth=if tool==Tool::Claude {claude::read_oauth_account(&self.inner.env.claude_json_path())?} else {None};
        let (previous_settings,previous_email,previous_events)=self.with_state(|s|(s.settings.clone(),s.live_email[idx(tool)].clone(),s.events.len()));
        self.write_live(tool, &secret)?;
        if tool == Tool::Claude && let Some(oa) = &oauth
            && let Err(error) = claude::write_oauth_account(&self.inner.env.claude_json_path(), Some(oa)) {
            self.restore_live(tool,previous.as_ref().map(|(v,_)|v.as_str()))?;
            return Err(error);
        }
        if pending && self.runtime_dir(tool,id).is_dir() {
            if let Err(error)=self.write_runtime(tool,id,&secret,oauth.as_ref()) {
                self.restore_live(tool,previous.as_ref().map(|(v,_)|v.as_str()))?;
                if tool==Tool::Claude {claude::write_oauth_account(&self.inner.env.claude_json_path(),previous_oauth.as_ref())?;}
                return Err(error);
            }
        }
        self.with_state(|s| {
            let ts = s.settings.tool_mut(tool);
            ts.active = Some(id.to_string());
            ts.active_since = now_unix();
            if let Some(profile) = ts.profiles.iter_mut().find(|profile| profile.profile.id == id) { profile.pending_login = false; }
            s.live_email[idx(tool)] = Some(email);
            s.login_required.remove(&(tool, id.to_string()));
            s.events.push(AccountsEvent::Switched { tool, id: id.to_string() });
        });
        if let Err(error)=self.persist() {
            self.with_state(|s| {s.settings=previous_settings;s.live_email[idx(tool)]=previous_email;s.events.truncate(previous_events);});
            if pending && self.runtime_dir(tool,id).is_dir() {self.restore_runtime(tool,id,previous_runtime.as_deref(),previous_runtime_oauth.as_ref())?;}
            self.restore_live(tool,previous.as_ref().map(|(v,_)|v.as_str()))?;
            if tool==Tool::Claude {claude::write_oauth_account(&self.inner.env.claude_json_path(),previous_oauth.as_ref())?;}
            return Err(error);
        }
        Ok(())
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
            s.usage_poll.remove(&(tool, id.to_string()));
            s.limited_at.remove(&(tool, id.to_string()));
            s.login_required.remove(&(tool, id.to_string()));
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
            if !s.settings.tool(tool).profiles.iter().any(|p| p.profile.id == id) { return; }
            let key = (tool, id.to_string());
            if !u.is_unavailable() {
                s.limited_at.remove(&key);
                if let Some(p) = s.settings.tool_mut(tool).profiles.iter_mut().find(|p| p.profile.id == id) {
                    p.last_usage = Some(u.clone());
                    p.last_usage_at = Some(now_unix());
                }
            } else if let Some(prev) = s.usage.get(&key)
                && (prev.five_hour.is_some() || prev.seven_day.is_some())
            {
                let mut kept = prev.clone();
                kept.status = u.status.clone();
                s.usage.insert(key, kept);
                return;
            }
            s.usage.insert(key, u);
        });
    }

    /// Starts one lifecycle-bound background worker. The settings pane need not be open.
    fn start_usage_updates(&self) {
        self.start_usage_updates_with_interval(std::time::Duration::from_secs(300));
    }

    fn start_usage_updates_with_interval(&self, interval: std::time::Duration) {
        use std::sync::atomic::Ordering;
        if self.inner.usage_worker_started.swap(true, Ordering::Relaxed) { return; }
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            let mut next = std::time::Instant::now();
            loop {
                let Some(inner) = weak.upgrade() else { break };
                if inner.shutting_down.load(Ordering::Relaxed) { break; }
                let manager = AccountManager { inner };
                let periodic = std::time::Instant::now() >= next;
                for tool in Tool::ALL {
                    if periodic || manager.usage_due(tool, now_unix()) {
                        manager.refresh_usage_impl(tool, false);
                    }
                }
                if periodic {
                    next = std::time::Instant::now() + interval;
                }
                drop(manager);
                std::thread::sleep(interval.min(std::time::Duration::from_secs(1)));
            }
        });
    }

    /// A later account can become due between periodic credential checks.
    fn usage_due(&self, tool: Tool, now: i64) -> bool {
        self.with_state(|s| !s.refreshing[idx(tool)] && s.settings.tool(tool).profiles.iter().any(|profile| {
            !profile.pending_login && s.usage_poll.get(&(tool, profile.profile.id.clone())).is_some_and(|poll| {
                poll.next_at > 0 && now >= poll.next_at.max(poll.blocked_until)
            })
        }))
    }

    pub fn usage_fetched_at(&self, tool: Tool, id: &str) -> Option<i64> {
        self.with_state(|s| s.settings.tool(tool).profiles.iter().find(|p| p.profile.id == id).and_then(|p| p.last_usage_at))
    }

    /// Opening settings refreshes both providers before cached usage is rendered.
    pub fn settings_visibility(&self, visible: bool) {
        let opened = self.with_state(|s| {
            let opened = visible && !s.settings_visible;
            s.settings_visible = visible;
            opened
        });
        if opened {
            for tool in Tool::ALL { self.refresh_usage(tool); }
        }
    }

    /// Refresh all saved accounts; concurrent requests and provider backoff are respected.
    pub fn refresh_usage(&self, tool: Tool) { self.refresh_usage_impl(tool, true); }

    fn refresh_usage_impl(&self, tool: Tool, manual: bool) {
        use std::sync::atomic::Ordering;
        if self.inner.shutting_down.load(Ordering::Relaxed) || self.profiles(tool).is_empty() { return; }
        let started = self.with_state(|s| !std::mem::replace(&mut s.refreshing[idx(tool)], true));
        if !started { return; }
        self.repaint();
        let m = self.clone();
        std::thread::spawn(move || {
            if tool == Tool::Codex && m.inner.env.codex_native_probe.is_some() {
                m.refresh_native_codex(manual);
            } else {
                m.check_profile_credentials();
                if tool == Tool::Codex && m.inner.env.codex_probe.is_none() {
                    m.refresh_codex_logs();
                } else { m.refresh_profiles(tool, manual); }
            }
            m.with_state(|s| s.refreshing[idx(tool)] = false);
            m.persist_logged();
            m.repaint();
        });
    }

    /// Test-only synchronous boundary; never used by rendering.
    pub fn refresh_usage_blocking(&self, tool: Tool) {
        self.refresh_usage(tool);
        while self.is_refreshing(tool) { std::thread::sleep(std::time::Duration::from_millis(5)); }
    }

    fn refresh_native_codex(&self, manual: bool) {
        for profile in self.profiles(Tool::Codex) {
            if self.inner.shutting_down.load(std::sync::atomic::Ordering::Relaxed) { break; }
            if self.pending_login(Tool::Codex, &profile.id) { continue; }
            let now = now_unix();
            let due = self.with_state(|s| {
                let poll = s.usage_poll.entry((Tool::Codex, profile.id.clone())).or_default();
                now >= poll.blocked_until && (manual || now >= poll.next_at)
            });
            if !due { continue; }
            let result = self.probe_codex_profile(&profile);
            let _op = self.inner.op.lock();
            if self.pending_login(Tool::Codex, &profile.id) || !self.profiles(Tool::Codex).iter().any(|p| p.id == profile.id && p.email == profile.email) { continue; }
            self.with_state(|s| {
                let poll = s.usage_poll.entry((Tool::Codex, profile.id.clone())).or_default();
                poll.next_at = now_unix() + 300;
                poll.blocked_until = if result.is_err() { poll.next_at } else { 0 };
            });
            self.set_usage(Tool::Codex, &profile.id, result.unwrap_or_else(Usage::unavailable));
            self.repaint();
        }
    }

    fn probe_codex_profile(&self, profile: &Profile) -> Result<Usage> {
        let runtime = self.runtime_dir(Tool::Codex, &profile.id);
        let home = if runtime.is_dir() { &runtime } else { &self.inner.env.codex_home };
        profile.email.as_deref().context("account identity unavailable").and_then(|email| {
            let probe = self.inner.env.codex_native_probe.as_ref().unwrap();
            probe(home, email).or_else(|error| {
                if error.to_string() == "authentication required"
                    && home != &self.inner.env.codex_home
                    && self.active(Tool::Codex).as_deref() == Some(&profile.id)
                {
                    // The native reader checks this profile's identity before returning quotas.
                    probe(&self.inner.env.codex_home, email)
                } else { Err(error) }
            })
        })
    }

    fn usage_secret(&self, tool: Tool, id: &str) -> Result<Option<String>> {
        Ok(self.usage_credentials(tool, id)?.map(|(secret, _)| secret))
    }

    /// The credentials a usage read uses for profile `id`, and where they are kept.
    fn usage_credentials(&self, tool: Tool, id: &str) -> Result<Option<(String, UsageSource)>> {
        let stored = self.with_state(|state| state.settings.tool(tool).profiles.iter().find(|profile| profile.profile.id == id).cloned()).context("account profile missing")?;
        let expected = stored.profile.email.as_deref().context("account identity unavailable")?;
        let matches = |email: Option<String>| email.is_some_and(|email| email.eq_ignore_ascii_case(expected));
        if self.runtime_dir(tool, id).is_dir() {
            let Some(secret) = self.runtime_secret(tool, id)? else { return Ok(None); };
            let email = match tool {
                Tool::Claude => self.runtime_oauth(tool, id)?.as_ref().and_then(claude::oauth_email),
                Tool::Codex => codex::auth_email(&secret),
            };
            if !matches(email) { bail!("account credentials do not match saved profile"); }
            return Ok(Some((secret, UsageSource::Runtime(self.runtime_dir(tool, id)))));
        }
        if self.active(tool).as_deref() == Some(id)
            && let Some((secret, _)) = self.read_live(tool)?
            && matches(self.live_identity(tool, Some(&secret)).0)
        { return Ok(Some((secret, UsageSource::Live))); }
        let secret = self.inner.env.store.get(&profile_service(tool), id)?;
        let email = match tool {
            Tool::Claude => stored.oauth_account.as_ref().and_then(claude::oauth_email),
            Tool::Codex => secret.as_deref().and_then(codex::auth_email),
        };
        if secret.is_some() && !matches(email) { bail!("account credentials do not match saved profile"); }
        Ok(secret.map(|secret| (secret, UsageSource::Snapshot)))
    }

    /// Renews Claude `secret` for profile `id` when it is about to expire and kept in a Claude
    /// Code credential store, as the CLI renews it. None while the CLI is renewing it.
    fn renew_claude(&self, id: &str, source: &UsageSource, secret: &str, now_ms: i64) -> Result<Option<String>> {
        let env = &self.inner.env;
        let (config, folder) = match source {
            UsageSource::Runtime(dir) => (Some(dir.clone()), dir.clone()),
            UsageSource::Live => (env.claude_config_dir.clone(), env.claude_config_dir.clone().unwrap_or_else(|| env.home.join(".claude"))),
            UsageSource::Snapshot => bail!("sign-in expired"),
        };
        let observed = claude::access_token(secret)?;
        std::fs::create_dir_all(&folder)?;
        let Some(_lock) = claude_refresh::RefreshLock::acquire(&folder, RENEWAL_LOCK_ATTEMPTS, std::time::Duration::from_millis(1500))? else { return Ok(None) };
        let read = || -> Result<(String, Option<String>)> {
            let _op = self.inner.op.lock();
            if self.usage_credentials(Tool::Claude, id)?.is_none_or(|(_, now)| &now != source) { bail!("account credentials changed"); }
            env.claude_secret(config.as_deref())?.context("account credentials missing")
        };
        let (stored, _) = read()?;
        if claude::access_token(&stored).ok().as_deref() != Some(observed.as_str()) || !claude_refresh::expiring(&stored, now_ms) {
            return Ok(Some(stored));
        }
        let grant = claude_refresh::Grant::from_secret(&stored).ok_or(claude_refresh::SignInRequired)?;
        let renewed = (env.claude_renew)(&grant)?;
        let _op = self.inner.op.lock();
        let (current, account) = env.claude_secret(config.as_deref())?.context("account credentials missing")?;
        let Some(next) = claude_refresh::merged(&current, &grant.refresh_token, &renewed, now_ms)? else { return Ok(Some(current)) };
        match account {
            Some(account) if env.claude_live_in_store => env.store.set(&crate::login::claude_service(config.as_deref()), &account, &next)?,
            _ => write_secret_atomic(&claude::credentials_path(&env.home, config.as_deref()), next.as_bytes())?,
        }
        Ok(Some(next))
    }

    fn refresh_profiles(&self, tool: Tool, manual: bool) {
        self.refresh_profiles_with_clock(tool, manual, &now_unix);
    }

    fn refresh_profiles_with_clock(&self, tool: Tool, manual: bool, clock: &dyn Fn() -> i64) {
        use sha2::{Digest, Sha256};
        for profile in self.profiles(tool) {
            if self.inner.shutting_down.load(std::sync::atomic::Ordering::Relaxed) { break; }
            let credentials = {
                let _op = self.inner.op.lock();
                if self.pending_login(tool, &profile.id) { continue; }
                self.usage_credentials(tool, &profile.id)
            };
            let (mut secret, source) = match credentials {
                Ok(Some(credentials)) => credentials,
                _ => {
                    self.with_state(|s| {
                        if s.settings.tool(tool).profiles.iter().any(|p| p.profile.id == profile.id) {
                            s.usage_poll.entry((tool, profile.id.clone())).or_default().next_at = clock() + 300;
                        }
                    });
                    self.set_usage(tool, &profile.id, Usage::unavailable("no credentials"));
                    continue;
                }
            };
            // The digest stays in memory; a CLI token refresh makes a previously blocked account eligible.
            // Backoff belongs to the bearer/account, not changing CLI bookkeeping fields.
            let identity = match tool {
                Tool::Claude => claude::access_token(&secret).unwrap_or_default(),
                Tool::Codex => serde_json::from_str::<Value>(&secret).ok().map(|doc| format!("{}\0{}",
                    doc.pointer("/tokens/access_token").and_then(Value::as_str).unwrap_or_default(),
                    doc.pointer("/tokens/account_id").and_then(Value::as_str).unwrap_or_default())).unwrap_or_default(),
            };
            let mut fingerprint = Sha256::digest(identity.as_bytes()).to_vec();
            let now = clock();
            let due = self.with_state(|s| {
                let poll = s.usage_poll.entry((tool, profile.id.clone())).or_default();
                if poll.fingerprint != fingerprint { *poll = UsagePoll { fingerprint: fingerprint.clone(), ..Default::default() }; }
                now >= poll.blocked_until && (manual || now >= poll.next_at)
            });
            if !due { continue; }
            if tool == Tool::Claude && claude_refresh::expiring(&secret, now.saturating_mul(1000)) {
                let postpone = |s: &mut State, next_at: i64, blocked_until: i64| {
                    s.usage_poll.insert((tool, profile.id.clone()), UsagePoll { fingerprint: fingerprint.clone(), next_at, blocked_until });
                };
                match self.renew_claude(&profile.id, &source, &secret, now.saturating_mul(1000)) {
                    Ok(Some(renewed)) => {
                        secret = renewed;
                        fingerprint = Sha256::digest(claude::access_token(&secret).unwrap_or_default().as_bytes()).to_vec();
                    }
                    Ok(None) => {
                        self.with_state(|s| postpone(s, now + 60, 0));
                        continue;
                    }
                    Err(error) => {
                        let rejected = error.downcast_ref::<claude_refresh::SignInRequired>().is_some();
                        self.with_state(|s| {
                            postpone(s, now + 300, if rejected { i64::MAX } else { 0 });
                            if rejected {
                                s.renewal_rejected.insert((tool, profile.id.clone()), fingerprint.clone());
                                s.login_required.insert((tool, profile.id.clone()));
                            }
                        });
                        self.set_usage(tool, &profile.id, Usage::unavailable(error));
                        self.repaint();
                        continue;
                    }
                }
            }
            let result = match tool {
                Tool::Claude => claude::access_token(&secret).and_then(|token| (self.inner.env.probe)(&token)),
                Tool::Codex => (self.inner.env.codex_probe.as_ref().expect("online probe"))(&secret),
            };
            let _op = self.inner.op.lock();
            // A response belongs to the exact profile credentials it was requested with.
            if self.pending_login(tool, &profile.id) || self.usage_secret(tool, &profile.id).ok().flatten().as_deref() != Some(&secret) { continue; }
            let exists = self.profiles(tool).iter().any(|p| p.id == profile.id);
            if !exists { continue; }
            let (usage, delay) = match result {
                Ok(usage) => (usage, 0),
                Err(error) => {
                    let delay = error.downcast_ref::<crate::usage_http::RetryLater>().map_or(0, |retry| retry.0);
                    (Usage::unavailable(error), delay)
                }
            };
            self.with_state(|s| {
                s.usage_poll.insert((tool, profile.id.clone()), UsagePoll { fingerprint, next_at: now + 300, blocked_until: now + delay });
            });
            self.set_usage(tool, &profile.id, usage);
            self.repaint();
        }
    }

    fn refresh_codex_logs(&self) {
        let (active, since) = self.with_state(|s| (s.settings.codex.active.clone(), s.settings.codex.active_since));
        let Some(active) = active else { return };
        let runtime = self.runtime_dir(Tool::Codex, &active);
        let home = if runtime.is_dir() { &runtime } else { &self.inner.env.codex_home };
        match codex::latest_observed(home, now_unix()) {
            Some(observed) if observed.at_unix >= since => self.set_usage(Tool::Codex, &active, observed.usage),
            _ if self.usage(Tool::Codex, &active).is_none() => self.set_usage(Tool::Codex, &active, Usage::unavailable(kiln_common::i18n::tr("최근 세션 기록 없음"))),
            _ => {},
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

    #[test]
    #[ignore = "requires explicit owner authorization and real Keychain opt-in"]
    fn live_saved_account_usage_without_persisting_settings() {
        let tool = match std::env::var("KILN_VERIFY_TOOL").as_deref() {
            Ok("claude") => Tool::Claude,
            Ok("codex") => Tool::Codex,
            _ => panic!("explicit tool required"),
        };
        let index: usize = std::env::var("KILN_VERIFY_PROFILE_INDEX").expect("explicit profile ordinal").parse().unwrap();
        let manager = AccountManager::with_env(Env::real());
        let profile = manager.profiles(tool).into_iter().nth(index).expect("saved profile");
        let usage = if tool == Tool::Codex {
            manager.probe_codex_profile(&profile).expect("live Codex quota request")
        } else {
            let secret = manager.usage_secret(tool, &profile.id).unwrap().expect("profile credentials");
            let token = claude::access_token(&secret).expect("subscription authentication");
            (manager.inner.env.probe)(&token).expect("live Claude quota request")
        };
        assert!(!usage.is_unavailable());
        assert!(usage.five_hour.is_some() || usage.seven_day.is_some());
    }

    #[test]
    #[ignore = "requires explicit owner authorization and real Keychain opt-in"]
    fn live_claude_login_renewal_without_persisting_settings() {
        let index: usize = std::env::var("KILN_VERIFY_PROFILE_INDEX").expect("explicit profile ordinal").parse().unwrap();
        let manager = AccountManager::with_env(Env::real());
        let profile = manager.profiles(Tool::Claude).into_iter().nth(index).expect("saved profile");
        let (secret, source) = manager.usage_credentials(Tool::Claude, &profile.id).unwrap().expect("profile credentials");
        let now_ms = now_unix() * 1000;
        let secret = if claude_refresh::expiring(&secret, now_ms) {
            manager.renew_claude(&profile.id, &source, &secret, now_ms).expect("login renewal").expect("CLI refresh lock free")
        } else { secret };
        assert!(!claude_refresh::expiring(&secret, now_ms));
        let usage = (manager.inner.env.probe)(&claude::access_token(&secret).unwrap()).expect("live Claude quota request");
        assert!(!usage.is_unavailable());
        assert!(usage.five_hour.is_some() || usage.seven_day.is_some());
    }

    #[test]
    fn active_codex_uses_matching_live_credentials_when_runtime_auth_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        std::fs::create_dir_all(&env.codex_home).unwrap();
        std::fs::write(env.codex_home.join("auth.json"), codex::tests::fake_auth("fixture@example.test", "fixture")).unwrap();
        let live = env.codex_home.clone();
        env.codex_native_probe = Some(Arc::new(move |home, email| {
            assert_eq!(email, "fixture@example.test");
            if home != live { anyhow::bail!("authentication required"); }
            Ok(Usage { five_hour: Some((0.42, None)), status: "allowed".into(), ..Default::default() })
        }));
        let manager = AccountManager::with_env(env);
        let profile = manager.save_current(Tool::Codex, "Fixture").unwrap();
        std::fs::create_dir_all(manager.runtime_dir(Tool::Codex, &profile.id)).unwrap();
        manager.refresh_usage_blocking(Tool::Codex);
        assert_eq!(manager.usage(Tool::Codex, &profile.id).unwrap().five_hour.unwrap().0, 0.42);
        manager.with_state(|s| s.settings.codex.active = None);
        assert_eq!(manager.probe_codex_profile(&profile).unwrap_err().to_string(), "authentication required");
    }

    #[test]
    fn claude_credentials_prefer_the_current_user_keychain_item() {
        let root = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(root.path(), true);
        store.set(claude::CLAUDE_SERVICE, "another-user", r#"{"mcpOAuth":{}}"#).unwrap();
        store.set(claude::CLAUDE_SERVICE, &env.username, &claude_cred("current-token")).unwrap();
        let (secret, account) = env.claude_secret(None).unwrap().unwrap();
        assert_eq!(secret, claude_cred("current-token"));
        assert_eq!(account.as_deref(), Some(env.username.as_str()));
    }

    #[test]
    fn native_codex_usage_does_not_parse_cli_owned_encrypted_credentials() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        std::fs::create_dir_all(&env.codex_home).unwrap();
        std::fs::write(env.codex_home.join("auth.json"), codex::tests::fake_auth("fixture@example.test", "fixture")).unwrap();
        env.codex_native_probe = Some(Arc::new(|home, email| {
            assert_eq!(email, "fixture@example.test");
            assert_eq!(std::fs::read_to_string(home.join("auth.json")).unwrap(), "encrypted-fixture");
            Ok(Usage { five_hour: Some((0.42, None)), status: "allowed".into(), ..Default::default() })
        }));
        let manager = AccountManager::with_env(env);
        let profile = manager.save_current(Tool::Codex, "Fixture").unwrap();
        let runtime = manager.runtime_dir(Tool::Codex, &profile.id);
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("auth.json"), "encrypted-fixture").unwrap();
        manager.refresh_usage_blocking(Tool::Codex);
        assert_eq!(manager.usage(Tool::Codex, &profile.id).unwrap().five_hour.unwrap().0, 0.42);
    }

    #[test]
    fn settings_open_fetches_once_and_reopening_fetches_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        env.probe = Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Usage { five_hour: Some((0.3, None)), status: "allowed".into(), ..Default::default() })
        });
        login_claude(&env, &store, "fixture@example.test", "fixture");
        let manager = AccountManager::with_env(env);
        manager.save_current(Tool::Claude, "Fixture").unwrap();
        for expected in [1, 2] {
            manager.settings_visibility(true);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while manager.is_refreshing(Tool::Claude) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            manager.settings_visibility(true);
            assert_eq!(calls.load(Ordering::SeqCst), expected);
            manager.settings_visibility(false);
        }
    }

    #[test]
    fn two_claude_accounts_use_fresh_keychain_tokens_and_cli_config_metadata() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.probe = Arc::new(|token| Ok(Usage { five_hour: Some((match token { "at-a-fresh" => 0.2, "at-b-fresh" => 0.7, _ => panic!("stale or wrong-account token selected") }, None)), status: "allowed".into(), ..Default::default() }));
        let m = AccountManager::with_env(env.clone());
        let mut saved = Vec::new();
        for (email, marker) in [("a@example.test", "a"), ("b@example.test", "b")] {
            login_claude(&env, &store, email, marker);
            let p = m.save_current(Tool::Claude, marker).unwrap();
            m.launch_environment(Tool::Claude).unwrap();
            let dir = m.runtime_dir(Tool::Claude, &p.id);
            write_secret_atomic(&dir.join(".credentials.json"), claude_cred("stale-file").as_bytes()).unwrap();
            store.set(&crate::login::claude_service(Some(&dir)), &env.username, &claude_cred(&format!("{marker}-fresh"))).unwrap();
            claude::write_oauth_account(&dir.join(".config.json"), Some(&serde_json::json!({"emailAddress":email}))).unwrap();
            // The newer CLI config is authoritative even when an old file remains.
            claude::write_oauth_account(&dir.join(".claude.json"), Some(&serde_json::json!({"emailAddress":"stale@example.test"}))).unwrap();
            saved.push(p);
        }
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(m.usage(Tool::Claude, &saved[0].id).unwrap().five_hour.unwrap().0, 0.2);
        assert_eq!(m.usage(Tool::Claude, &saved[1].id).unwrap().five_hour.unwrap().0, 0.7);
        m.check_profile_credentials();
        assert!(saved.iter().all(|p| !m.login_required(Tool::Claude, &p.id)));
    }

    #[test]
    fn codex_optional_identity_and_legacy_keyring_do_not_report_logout() {
        let root = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(root.path(), false);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "work@example.test", "old");
        let p = m.save_current(Tool::Codex, "Work").unwrap();
        m.launch_environment(Tool::Codex).unwrap();
        let dir = m.runtime_dir(Tool::Codex, &p.id);
        let cases = [serde_json::json!({"tokens":{"access_token":"fixture", "id_token":"no-email"}}), serde_json::json!({"agent_identity":{"agent_runtime_id":"fixture","agent_private_key":"fixture","email":null}}), serde_json::json!({"auth_mode":"future","new_credential":"fixture"})];
        for doc in cases {
            write_secret_atomic(&dir.join("auth.json"), serde_json::to_string(&doc).unwrap().as_bytes()).unwrap();
            m.check_profile_credentials();
            assert!(!m.login_required(Tool::Codex, &p.id));
            assert!(m.usage_secret(Tool::Codex, &p.id).is_err(), "unknown identity must never attribute quota to the saved account");
        }
        std::fs::remove_file(dir.join("auth.json")).unwrap();
        std::fs::write(dir.join("config.toml"), "cli_auth_credentials_store = 'keyring'\n").unwrap();
        if cfg!(windows) {
            // The CLI keeps keyring secrets in its encrypted store by default on Windows.
            m.check_profile_credentials();
            assert!(!m.login_required(Tool::Codex, &p.id));
            assert!(m.usage_secret(Tool::Codex, &p.id).is_err());
            std::fs::write(dir.join("config.toml"), "cli_auth_credentials_store = 'keyring'\n[features]\nsecret_auth_storage = false\n").unwrap();
        }
        store.set("Codex Auth", &codex::keyring_account(&dir), &fake_auth("work@example.test", "fresh")).unwrap();
        m.check_profile_credentials();
        assert!(!m.login_required(Tool::Codex, &p.id));
        assert!(m.usage_secret(Tool::Codex, &p.id).unwrap().is_some());
        assert!(m.launch_environment(Tool::Codex).unwrap().iter().any(|(key,value)|key=="KILN_CODEX_AUTH_STORE" && value=="keyring"));
        assert!(!dir.join("auth.json").exists(), "launch must not duplicate a Keyring refresh token into a file");
        store.delete("Codex Auth", &codex::keyring_account(&dir)).unwrap();
        m.check_profile_credentials();
        assert!(m.login_required(Tool::Codex, &p.id));
    }

    #[test]
    fn usage_never_attributes_external_or_runtime_login_to_another_profile() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for tool in Tool::ALL {
            for runtime_changed in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let (mut env, store) = Env::sandbox(dir.path(), true);
                let calls = Arc::new(AtomicUsize::new(0));
                let count = calls.clone();
                env.probe = Arc::new(move |token| {
                    count.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(token, "at-a");
                    Ok(Usage {five_hour:Some((0.25,None)),status:"allowed".into(),..Default::default()})
                });
                let count = calls.clone();
                env.codex_probe = Some(Arc::new(move |auth| {
                    count.fetch_add(1, Ordering::Relaxed);
                    assert_eq!(serde_json::from_str::<Value>(auth).unwrap()["tokens"]["account_id"], "a");
                    Ok(Usage {five_hour:Some((0.25,None)),status:"allowed".into(),..Default::default()})
                }));
                let login = |email, marker| match tool {
                    Tool::Claude => login_claude(&env, &store, email, marker),
                    Tool::Codex => login_codex(&env, email, marker),
                };
                login("a@example.test", "a");
                let manager = AccountManager::with_env(env.clone());
                let profile = manager.save_current(tool, "A").unwrap();
                if runtime_changed {
                    manager.launch_environment(tool).unwrap();
                    let other = match tool { Tool::Claude => claude_cred("b"), Tool::Codex => fake_auth("b@example.test", "b") };
                    manager.restore_runtime(tool, &profile.id, Some(&other), Some(&serde_json::json!({"emailAddress":"b@example.test"}))).unwrap();
                } else { login("b@example.test", "b"); }
                manager.refresh_usage_blocking(tool);
                assert_eq!(manager.active(tool).as_deref(), Some(profile.id.as_str()));
                assert_eq!(calls.load(Ordering::Relaxed), if runtime_changed { 0 } else { 1 });
                if runtime_changed { assert!(manager.usage(tool, &profile.id).unwrap().is_unavailable()); }
                else { assert_eq!(manager.usage(tool, &profile.id).unwrap().five_hour.unwrap().0, 0.25); }
            }
        }
    }

    #[test]
    fn slower_previous_batch_does_not_delay_a_later_account_until_next_batch() {
        use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(dir.path(), true);
        let time = Arc::new(AtomicI64::new(1_000));
        let first = Arc::new(AtomicUsize::new(0));
        let later = Arc::new(AtomicUsize::new(0));
        let (probe_time, probe_first, probe_later) = (time.clone(), first.clone(), later.clone());
        env.probe = Arc::new(move |token| {
            if token == "at-a" {
                let call = probe_first.fetch_add(1, Ordering::Relaxed);
                probe_time.fetch_add(if call == 0 { 15 } else { 1 }, Ordering::Relaxed);
            } else { probe_later.fetch_add(1, Ordering::Relaxed); }
            Ok(Usage { five_hour: Some((0.2, None)), status: "allowed".into(), ..Default::default() })
        });
        let manager = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@example.test", "a");
        manager.save_current(Tool::Claude, "a").unwrap();
        login_claude(&env, &store, "b@example.test", "b");
        let b = manager.save_current(Tool::Claude, "b").unwrap();
        let clock = || time.load(Ordering::Relaxed);
        manager.refresh_profiles_with_clock(Tool::Claude, false, &clock);
        time.store(1_300, Ordering::Relaxed);
        manager.refresh_profiles_with_clock(Tool::Claude, false, &clock);
        assert_eq!(clock(), 1_301);
        assert_eq!(later.load(Ordering::Relaxed), 1);
        assert!(!manager.usage_due(Tool::Claude, 1_314));
        assert!(manager.usage_due(Tool::Claude, 1_315), "wake at the later account's deadline, not the next 1,600 batch");
        time.store(1_315, Ordering::Relaxed);
        manager.refresh_profiles_with_clock(Tool::Claude, false, &clock);
        assert_eq!(first.load(Ordering::Relaxed), 2);
        assert_eq!(later.load(Ordering::Relaxed), 2);
        assert!(!manager.usage_due(Tool::Claude, 1_599));
        manager.with_state(|s| s.usage_poll.get_mut(&(Tool::Claude, b.id.clone())).unwrap().blocked_until = 2_000);
        manager.with_state(|s| s.settings.claude.profiles[0].pending_login = true);
        assert!(!manager.usage_due(Tool::Claude, 1_900), "staged logins and provider delays do not cause busy polling");
        manager.with_state(|s| s.settings.claude.profiles[0].pending_login = false);
        manager.refresh_profiles_with_clock(Tool::Claude, true, &clock);
        store.delete(&profile_service(Tool::Claude), &b.id).unwrap();
        manager.with_state(|s| s.usage_poll.get_mut(&(Tool::Claude, b.id.clone())).unwrap().blocked_until = 0);
        time.store(2_000, Ordering::Relaxed);
        // Remove the active global credentials too; no CLI/network request should spin.
        store.delete(claude::CLAUDE_SERVICE, "claude-code-user").unwrap();
        manager.refresh_profiles_with_clock(Tool::Claude, false, &clock);
        assert!(!manager.usage_due(Tool::Claude, 2_001));
    }

    #[test]
    fn background_usage_updates_all_accounts_without_settings_and_stops_on_exit() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(dir.path(), true);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        env.probe = Arc::new(move |token| {
            count.fetch_add(1, Ordering::Relaxed);
            Ok(Usage { five_hour: Some((if token == "at-a" {0.2} else {0.8}, None)), status:"allowed".into(), ..Default::default() })
        });
        let count = calls.clone();
        env.codex_probe = Some(Arc::new(move |auth| {
            count.fetch_add(1, Ordering::Relaxed);
            let value: Value = serde_json::from_str(auth).unwrap();
            Ok(Usage { five_hour: Some((if value["tokens"]["account_id"] == "c" {0.3} else {0.9}, None)), status:"allowed".into(), ..Default::default() })
        }));
        let manager = AccountManager::with_env(env.clone());
        let mut profiles = Vec::new();
        for marker in ["a", "b"] {
            login_claude(&env, &store, &format!("{marker}@example.test"), marker);
            profiles.push(manager.save_current(Tool::Claude, marker).unwrap());
        }
        for marker in ["c", "d"] {
            login_codex(&env, &format!("{marker}@example.test"), marker);
            profiles.push(manager.save_current(Tool::Codex, marker).unwrap());
        }
        let interval = std::time::Duration::from_millis(10);
        manager.start_usage_updates_with_interval(interval);
        manager.start_usage_updates_with_interval(interval); // Idempotent, including the background lifecycle.
        let wait_for = |wanted| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while calls.load(Ordering::Relaxed) < wanted || Tool::ALL.iter().any(|&tool| manager.is_refreshing(tool)) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(interval);
            }
        };
        wait_for(4);
        for (profile, expected) in profiles.iter().zip([0.2,0.8,0.3,0.9]) {
            assert_eq!(manager.usage(profile.tool, &profile.id).unwrap().five_hour.unwrap().0, expected);
            assert!(manager.usage_fetched_at(profile.tool, &profile.id).is_some());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 4);
        manager.with_state(|state| for poll in state.usage_poll.values_mut() { poll.next_at = 0; });
        wait_for(8); // A later background cycle, with no settings UI or manual refresh.
        manager.shutdown_logins();
        wait_for(8);
        let stopped = calls.load(Ordering::Relaxed);
        std::thread::sleep(interval * 3);
        assert_eq!(calls.load(Ordering::Relaxed), stopped);
        let reloaded = AccountManager::with_env(env);
        assert!(reloaded.usage_fetched_at(Tool::Claude, &profiles[0].id).is_some());
    }

    #[test]
    fn refresh_backoff_is_per_account_and_retains_last_good_values_on_repeated_failures() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(dir.path(), true);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        env.probe = Arc::new(move |_| {
            if count.fetch_add(1, Ordering::Relaxed) == 0 { Ok(Usage { five_hour:Some((0.4,None)), status:"allowed".into(), ..Default::default() }) }
            else { Err(crate::usage_http::RetryLater(900).into()) }
        });
        let manager = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@example.test", "a");
        let profile = manager.save_current(Tool::Claude, "a").unwrap();
        manager.refresh_usage_blocking(Tool::Claude);
        let fetched = manager.usage_fetched_at(Tool::Claude, &profile.id);
        manager.refresh_usage_blocking(Tool::Claude);
        manager.refresh_usage_blocking(Tool::Claude);
        assert_eq!(calls.load(Ordering::Relaxed), 2, "manual refresh also honors Retry-After");
        manager.with_state(|state| state.usage_poll.values_mut().for_each(|poll| poll.blocked_until = 0));
        manager.refresh_usage_blocking(Tool::Claude);
        assert_eq!(manager.usage(Tool::Claude, &profile.id).unwrap().five_hour.unwrap().0, 0.4);
        assert_eq!(manager.usage_fetched_at(Tool::Claude, &profile.id), fetched);
        login_claude(&env, &store, "a@example.test", "new-token");
        manager.refresh_usage_blocking(Tool::Claude);
        assert_eq!(calls.load(Ordering::Relaxed), 4, "new CLI credentials clear the old credential backoff");
    }

    #[test]
    fn deleted_profile_cannot_be_resurrected_by_a_slow_usage_response() {
        let dir = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(dir.path(), true);
        let (started, begin) = std::sync::mpsc::channel();
        let (finish, done) = std::sync::mpsc::channel();
        let done = Mutex::new(done);
        env.probe = Arc::new(move |token| {
            if token == "at-a" { started.send(()).unwrap(); done.lock().recv().unwrap(); }
            Ok(Usage {five_hour:Some((0.4,None)),status:"allowed".into(),..Default::default()})
        });
        let manager = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "a@example.test", "a");
        let profile = manager.save_current(Tool::Claude, "a").unwrap();
        login_claude(&env, &store, "b@example.test", "b");
        manager.save_current(Tool::Claude, "b").unwrap();
        manager.refresh_usage(Tool::Claude);
        begin.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        manager.remove(Tool::Claude, &profile.id).unwrap();
        finish.send(()).unwrap();
        while manager.is_refreshing(Tool::Claude) { std::thread::sleep(std::time::Duration::from_millis(5)); }
        assert!(manager.usage(Tool::Claude, &profile.id).is_none());
    }
    use crate::codex::tests::fake_auth;
    use crate::store::CredentialStore;

    fn claude_cred(marker: &str) -> String {
        serde_json::json!({"claudeAiOauth": {"accessToken": format!("at-{marker}"), "refreshToken": format!("rt-{marker}"), "expiresAt": 4_102_444_800_000i64}}).to_string()
    }

    fn expired_claude_cred(marker: &str) -> String {
        serde_json::json!({"claudeAiOauth": {"accessToken": format!("at-{marker}"), "refreshToken": format!("rt-{marker}"), "expiresAt": 1,
            "scopes": ["user:inference", "user:profile"], "subscriptionType": "max"}, "mcpOAuth": {"kept": true}}).to_string()
    }

    fn renewal(marker: &str) -> crate::claude_refresh::Renewed {
        crate::claude_refresh::Renewed { access_token: format!("at-{marker}"), refresh_token: Some(format!("rt-{marker}")), expires_in: 28_800, scope: None, refresh_token_expires_in: None }
    }

    /// Two saved Claude accounts with runtime folders; `b` is idle and holds `b_secret`.
    fn idle_claude_account(env: &Env, store: &crate::MemoryStore, m: &AccountManager, b_secret: &str) -> (Profile, Profile, PathBuf) {
        login_claude(env, store, "b@example.test", "b-old");
        let b = m.save_current(Tool::Claude, "b").unwrap();
        m.launch_environment(Tool::Claude).unwrap();
        let dir = m.runtime_dir(Tool::Claude, &b.id);
        claude::write_oauth_account(&dir.join(".claude.json"), Some(&serde_json::json!({"emailAddress": "b@example.test"}))).unwrap();
        login_claude(env, store, "a@example.test", "a");
        let a = m.save_current(Tool::Claude, "a").unwrap();
        store.set(&crate::login::claude_service(Some(&dir)), &env.username, b_secret).unwrap();
        assert_eq!(m.active(Tool::Claude).as_deref(), Some(a.id.as_str()));
        (a, b, dir)
    }

    fn usage_from(token: &'static str, value: f32) -> UsageProbe {
        Arc::new(move |seen: &str| {
            if seen == token { Ok(Usage { five_hour: Some((value, None)), status: "allowed".into(), ..Default::default() }) }
            else if seen == "at-a" { Ok(Usage { five_hour: Some((0.1, None)), status: "allowed".into(), ..Default::default() }) }
            else { panic!("usage requested with {seen}") }
        })
    }

    #[test]
    fn an_idle_account_login_is_renewed_under_the_cli_locks_before_its_usage_is_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.probe = usage_from("at-b-renewed", 0.4);
        let grants = Arc::new(AtomicUsize::new(0));
        let seen = grants.clone();
        let runtime = root.path().join("config/agent-accounts/claude");
        env.claude_renew = Arc::new(move |grant| {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(grant.refresh_token, "rt-b-old");
            assert_eq!(grant.scope.as_deref(), Some("user:inference user:profile"));
            let locked = std::fs::read_dir(&runtime).unwrap().flatten().any(|entry| entry.path().join(".oauth_refresh.lock").is_dir());
            assert!(locked, "the grant is sent while the CLI lock is held");
            Ok(renewal("b-renewed"))
        });
        let m = AccountManager::with_env(env.clone());
        let (_, b, dir) = idle_claude_account(&env, &store, &m, &expired_claude_cred("b-old"));
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(m.usage(Tool::Claude, &b.id).unwrap().five_hour.unwrap().0, 0.4);
        assert_eq!(grants.load(Ordering::SeqCst), 1);
        let stored: Value = serde_json::from_str(&store.get(&crate::login::claude_service(Some(&dir)), &env.username).unwrap().unwrap()).unwrap();
        assert_eq!(stored["claudeAiOauth"]["accessToken"], "at-b-renewed");
        assert_eq!(stored["claudeAiOauth"]["refreshToken"], "rt-b-renewed");
        assert!(stored["claudeAiOauth"]["expiresAt"].as_i64().unwrap() > now_unix() * 1000);
        assert_eq!(stored["claudeAiOauth"]["subscriptionType"], "max");
        assert_eq!(stored["mcpOAuth"]["kept"], true);
        assert!(!dir.join(".oauth_refresh.lock").exists(), "the locks are released");
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(grants.load(Ordering::SeqCst), 1, "a fresh login is not renewed again");
    }

    #[test]
    fn a_login_renewed_elsewhere_is_used_without_another_grant() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.claude_renew = Arc::new(|_| panic!("the other process already renewed this login"));
        let m = AccountManager::with_env(env.clone());
        let (_, b, dir) = idle_claude_account(&env, &store, &m, &claude_cred("b-fresh"));
        let source = UsageSource::Runtime(dir);
        assert_eq!(m.renew_claude(&b.id, &source, &expired_claude_cred("b-old"), now_unix() * 1000).unwrap(), Some(claude_cred("b-fresh")));
    }

    #[test]
    fn a_rejected_refresh_token_asks_to_sign_in_and_stops_usage_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.probe = usage_from("at-b-new-login", 0.3);
        let grants = Arc::new(AtomicUsize::new(0));
        let seen = grants.clone();
        env.claude_renew = Arc::new(move |_| { seen.fetch_add(1, Ordering::SeqCst); Err(crate::claude_refresh::SignInRequired.into()) });
        let m = AccountManager::with_env(env.clone());
        let (_, b, dir) = idle_claude_account(&env, &store, &m, &expired_claude_cred("b-old"));
        m.refresh_usage_blocking(Tool::Claude);
        assert!(m.usage(Tool::Claude, &b.id).unwrap().status.contains("sign-in required"));
        m.check_profile_credentials();
        assert!(m.login_required(Tool::Claude, &b.id), "the rejected login still asks for sign-in after a presence check");
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(grants.load(Ordering::SeqCst), 1, "a rejected login is not renewed again");
        store.set(&crate::login::claude_service(Some(&dir)), &env.username, &claude_cred("b-new-login")).unwrap();
        m.check_profile_credentials();
        assert!(!m.login_required(Tool::Claude, &b.id));
        m.refresh_usage_blocking(Tool::Claude);
        assert_eq!(m.usage(Tool::Claude, &b.id).unwrap().five_hour.unwrap().0, 0.3);
    }

    #[test]
    fn an_expired_saved_snapshot_is_not_sent_to_the_usage_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.probe = usage_from("unused", 0.0);
        env.claude_renew = Arc::new(|_| panic!("a saved snapshot is not a CLI credential store"));
        let m = AccountManager::with_env(env.clone());
        login_claude(&env, &store, "b@example.test", "b-old");
        let b = m.save_current(Tool::Claude, "b").unwrap();
        store.set(&profile_service(Tool::Claude), &b.id, &expired_claude_cred("b-old")).unwrap();
        login_claude(&env, &store, "a@example.test", "a");
        m.save_current(Tool::Claude, "a").unwrap();
        assert!(!m.runtime_dir(Tool::Claude, &b.id).exists());
        m.refresh_usage_blocking(Tool::Claude);
        assert!(m.usage(Tool::Claude, &b.id).unwrap().status.contains("sign-in expired"));
    }

    #[test]
    fn a_held_cli_lock_postpones_renewal_without_a_usage_request() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, store) = Env::sandbox(root.path(), true);
        env.probe = usage_from("unused", 0.0);
        env.claude_renew = Arc::new(|_| panic!("the CLI is renewing this login"));
        let m = AccountManager::with_env(env.clone());
        let (_, b, dir) = idle_claude_account(&env, &store, &m, &expired_claude_cred("b-old"));
        std::fs::create_dir(dir.join(".oauth_refresh.lock")).unwrap();
        m.refresh_usage_blocking(Tool::Claude);
        assert!(m.usage(Tool::Claude, &b.id).is_none());
        assert!(dir.join(".oauth_refresh.lock").is_dir(), "another process's lock is left alone");
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
    fn browser_relogin_waits_for_apply_before_replacing_runtime() {
        let d=tempfile::tempdir().unwrap();let(env,store)=Env::sandbox(d.path(),true);let m=AccountManager::with_env(env.clone());
        login_claude(&env,&store,"a@x.com","old");let a=m.save_current(Tool::Claude,"").unwrap();m.launch_environment(Tool::Claude).unwrap();
        let oauth=claude::read_oauth_account(&env.claude_json_path()).unwrap();
        {let _op=m.inner.op.lock();m.save_credentials_locked(Tool::Claude,"",claude_cred("new"),None,Some("a@x.com".into()),oauth,false).unwrap();}
        assert!(m.pending_login(Tool::Claude,&a.id));assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("old")));
        m.switch_to(Tool::Claude,&a.id).unwrap();assert!(!m.pending_login(Tool::Claude,&a.id));assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("new")));
    }

    #[test]
    fn persistent_claude_accounts_keep_rotated_credentials_and_logout() {
        let d=tempfile::tempdir().unwrap();let(env,store)=Env::sandbox(d.path(),true);let m=AccountManager::with_env(env.clone());
        login_claude(&env,&store,"a@x.com","a1");let a=m.save_current(Tool::Claude,"").unwrap();
        let aenv=m.launch_environment(Tool::Claude).unwrap();let adir=PathBuf::from(&aenv[0].1);
        store.set(&crate::login::claude_service(Some(&adir)),&env.username,&claude_cred("a-refreshed")).unwrap();
        login_claude(&env,&store,"b@x.com","b1");let b=m.save_current(Tool::Claude,"").unwrap();
        let benv=m.launch_environment(Tool::Claude).unwrap();assert_ne!(aenv[0].1,benv[0].1);
        m.switch_to(Tool::Claude,&a.id).unwrap();assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("a-refreshed")));
        assert_eq!(m.launch_environment(Tool::Claude).unwrap(),aenv);
        store.delete(&crate::login::claude_service(Some(&adir)),&env.username).unwrap();
        assert!(m.launch_environment(Tool::Claude).is_err(),"logout must not resurrect old snapshot");
        login_claude(&env,&store,"a@x.com","new-login");m.save_current(Tool::Claude,"").unwrap();
        assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("new-login")));assert!(m.launch_environment(Tool::Claude).is_ok());
        assert_ne!(a.id,b.id);
    }
    #[test]
    fn claude_private_file_refresh_is_fallback_when_keychain_is_absent() {
        let d=tempfile::tempdir().unwrap();let(env,store)=Env::sandbox(d.path(),true);let m=AccountManager::with_env(env.clone());
        login_claude(&env,&store,"a@x.com","keychain-old");let a=m.save_current(Tool::Claude,"").unwrap();m.launch_environment(Tool::Claude).unwrap();
        store.delete(&crate::login::claude_service(Some(&m.runtime_dir(Tool::Claude,&a.id))), &env.username).unwrap();
        let path=m.runtime_dir(Tool::Claude,&a.id).join(".credentials.json");write_secret_atomic(&path,claude_cred("file-refreshed").as_bytes()).unwrap();
        assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("file-refreshed")));
        m.sync_back(Tool::Claude).unwrap();assert_eq!(store.get(&profile_service(Tool::Claude),&a.id).unwrap(),Some(claude_cred("file-refreshed")));
    }

    #[test]
    fn failed_apply_rolls_back_runtime_and_active_account() {
        let d=tempfile::tempdir().unwrap();let(env,store)=Env::sandbox(d.path(),true);let m=AccountManager::with_env(env.clone());
        login_claude(&env,&store,"a@x.com","old");let a=m.save_current(Tool::Claude,"").unwrap();m.launch_environment(Tool::Claude).unwrap();m.drain_events();
        let oauth=claude::read_oauth_account(&env.claude_json_path()).unwrap();
        {let _op=m.inner.op.lock();m.save_credentials_locked(Tool::Claude,"",claude_cred("new"),None,Some("a@x.com".into()),oauth,false).unwrap();}
        m.drain_events();
        std::fs::remove_file(&env.settings_path).unwrap();std::fs::create_dir(&env.settings_path).unwrap();
        assert!(m.switch_to(Tool::Claude,&a.id).is_err());
        assert!(m.pending_login(Tool::Claude,&a.id));assert_eq!(m.runtime_secret(Tool::Claude,&a.id).unwrap(),Some(claude_cred("old")));assert!(m.drain_events().is_empty());
    }

    #[test]
    fn missing_identity_does_not_overwrite_snapshot() {
        let d=tempfile::tempdir().unwrap();let(env,store)=Env::sandbox(d.path(),true);let m=AccountManager::with_env(env.clone());
        login_claude(&env,&store,"a@x.com","a1");let a=m.save_current(Tool::Claude,"").unwrap();
        claude::write_oauth_account(&env.claude_json_path(),None).unwrap();store.set(claude::CLAUDE_SERVICE,"claude-code-user",&claude_cred("unknown-account")).unwrap();
        m.sync_back(Tool::Claude).unwrap();assert_eq!(store.get(&profile_service(Tool::Claude),&a.id).unwrap(),Some(claude_cred("a1")));
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
            "#!/bin/sh\nfixture_dir='{fixture_dir}'\ncase \"$1\" in auth) fixture_tool=claude;; *) fixture_tool=codex;; esac\nprintf '%s\\n' \"$$\" > \"$fixture_dir/$fixture_tool.pid\"\nprintf '%s\\n' \"$HOME\" > \"$fixture_dir/$fixture_tool.home\"\nprintf '%s\\n' \"$CLAUDE_CONFIG_DIR\" > \"$fixture_dir/$fixture_tool.config\"\nexec /bin/sleep 60\n"
        )).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        for tool in Tool::ALL {
            manager.start_login_with_binary(tool, "Disposable".into(), binary.clone(), std::time::Duration::from_secs(90));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut children = Vec::new();
        for name in ["claude", "codex"] {
            let home_file = d.path().join(format!("{name}.home"));
            while ["home", "config", "pid"].iter().any(|suffix| std::fs::read_to_string(d.path().join(format!("{name}.{suffix}"))).ok().is_none_or(|s| s.trim().is_empty())) {
                assert!(std::time::Instant::now() < deadline, "fixture login did not start");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let home = PathBuf::from(std::fs::read_to_string(home_file).unwrap().trim());
            if name == "claude" { assert_eq!(home, env.home, "Claude must retain the system home for Keychain"); }
            else { assert!(!home.starts_with(&env.home), "Codex home stays disposable"); }
            let config = PathBuf::from(std::fs::read_to_string(d.path().join(format!("{name}.config"))).unwrap().trim());
            let isolated = config.parent().unwrap().to_path_buf();
            assert!(!isolated.starts_with(d.path()), "login configuration is separate from live files");
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
    #[cfg(unix)]
    fn targeted_browser_relogin_keeps_profile_and_requires_apply_for_both_tools() {
        use std::os::unix::fs::PermissionsExt;
        for tool in Tool::ALL {
            let d = tempfile::tempdir().unwrap();
            let (env, store) = Env::sandbox(d.path(), false);
            let m = AccountManager::with_env(env.clone());
            let old = if tool == Tool::Codex { fake_auth("same@example.test", "old") } else { claude_cred("old") };
            m.write_live(tool, &old).unwrap();
            if tool == Tool::Claude { set_claude_identity(&env, "same@example.test"); }
            let original = m.save_current(tool, "Work").unwrap();
            m.launch_environment(tool).unwrap();
            // Keep another profile active while repairing the original.
            let other = m.save_credentials_locked(tool, "Personal", old.clone(), None, Some("other@example.test".into()), None, true).unwrap();
            m.drain_events();
            let fresh = if tool == Tool::Codex { fake_auth("same@example.test", "fresh") } else { claude_cred("fresh") };
            let script = d.path().join("login-fixture");
            let body = if tool == Tool::Codex {
                format!("printf '%s' '{}' > \"$CODEX_HOME/auth.json\"", fresh)
            } else {
                format!("printf '%s' '{}' > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\nprintf '%s' '{{\"oauthAccount\":{{\"emailAddress\":\"same@example.test\"}}}}' > \"$CLAUDE_CONFIG_DIR/.claude.json\"", fresh)
            };
            std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            m.start_login_job(tool, original.label.clone(), Some(original.id.clone()), script, std::time::Duration::from_secs(3));
            let start = std::time::Instant::now();
            while m.with_state(|s| s.busy[idx(tool)].is_some()) {
                assert!(start.elapsed() < std::time::Duration::from_secs(4));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(m.with_state(|s| s.error[idx(tool)].is_none()), "{:?}", m.with_state(|s| s.error[idx(tool)].clone()));
            assert_eq!(m.profiles(tool), vec![original.clone(), other.clone()]);
            assert_eq!(m.active(tool), Some(other.id));
            assert_eq!(m.runtime_secret(tool, &original.id).unwrap(), Some(old));
            assert_eq!(store.get(&profile_service(tool), &original.id).unwrap(), Some(fresh.clone()));
            assert!(m.pending_login(tool, &original.id));
            m.switch_to(tool, &original.id).unwrap();
            assert_eq!(m.runtime_secret(tool, &original.id).unwrap(), Some(fresh));
            assert!(!m.pending_login(tool, &original.id));
            let reloaded = AccountManager::with_env(env);
            assert_eq!(reloaded.profiles(tool), m.profiles(tool));
        }
    }

    #[test]
    #[cfg(unix)]
    fn cancelling_targeted_browser_login_leaves_profile_runtime_and_snapshot_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(d.path(), false);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "same@example.test", "old");
        let original = m.save_current(Tool::Codex, "Work").unwrap();
        m.launch_environment(Tool::Codex).unwrap();
        m.drain_events();
        let before = std::fs::read(&env.settings_path).unwrap();
        let script = d.path().join("cancel-login-fixture");
        std::fs::write(&script, "#!/bin/sh\nexec sleep 20\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        m.start_login_job(Tool::Codex, original.label.clone(), Some(original.id.clone()), script, std::time::Duration::from_secs(3));
        assert_eq!(m.login_profile(Tool::Codex), Some(original.clone()));
        m.cancel_login(Tool::Codex);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while m.with_state(|s| s.busy[idx(Tool::Codex)].is_some()) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(m.login_status(Tool::Codex).is_none());
        assert_eq!(m.profiles(Tool::Codex), vec![original.clone()]);
        assert_eq!(store.get(&profile_service(Tool::Codex), &original.id).unwrap(), Some(fake_auth("same@example.test", "old")));
        assert_eq!(m.runtime_secret(Tool::Codex, &original.id).unwrap(), Some(fake_auth("same@example.test", "old")));
        assert_eq!(std::fs::read(&env.settings_path).unwrap(), before);
        assert!(m.drain_events().is_empty());
    }

    #[test]
    fn staged_relogin_cannot_bootstrap_runtime_before_apply() {
        let d = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(d.path(), false);
        let m = AccountManager::with_env(env.clone());
        login_codex(&env, "same@example.test", "old");
        let original = m.save_current(Tool::Codex, "Work").unwrap();
        m.save_relogin_locked(Tool::Codex, &original.id, crate::login::LoginCredentials {
            secret: fake_auth("same@example.test", "new"), email: original.email.clone(), oauth: None,
        }).unwrap();
        assert!(m.launch_environment(Tool::Codex).is_err());
        assert!(!m.runtime_dir(Tool::Codex, &original.id).exists());
        m.switch_to(Tool::Codex, &original.id).unwrap();
        assert!(m.launch_environment(Tool::Codex).is_ok());
    }

    #[test]
    fn targeted_relogin_rejects_another_account_and_missing_identity_without_mutation() {
        for tool in Tool::ALL {
            let d = tempfile::tempdir().unwrap();
            let (env, store) = Env::sandbox(d.path(), false);
            let m = AccountManager::with_env(env.clone());
            let old = if tool == Tool::Codex { fake_auth("same@example.test", "old") } else { claude_cred("old") };
            m.write_live(tool, &old).unwrap();
            if tool == Tool::Claude { set_claude_identity(&env, "same@example.test"); }
            let original = m.save_current(tool, "Work").unwrap();
            m.launch_environment(tool).unwrap();
            m.drain_events();
            let settings = std::fs::read(&env.settings_path).unwrap();
            for email in [Some("wrong@example.test".to_string()), None] {
                assert!(m.save_relogin_locked(tool, &original.id, crate::login::LoginCredentials { secret: "fresh".into(), email, oauth: None }).is_err());
                assert_eq!(m.profiles(tool), vec![original.clone()]);
                assert_eq!(store.get(&profile_service(tool), &original.id).unwrap(), Some(old.clone()));
                assert_eq!(m.runtime_secret(tool, &original.id).unwrap(), Some(old.clone()));
                assert_eq!(std::fs::read(&env.settings_path).unwrap(), settings);
                assert!(m.drain_events().is_empty());
            }
        }
    }

    #[test]
    fn removed_runtime_credentials_require_signin_but_expiry_and_probe_errors_do_not() {
        for tool in Tool::ALL {
            let d = tempfile::tempdir().unwrap();
            let (env, _) = Env::sandbox(d.path(), false);
            let m = AccountManager::with_env(env.clone());
            let old = if tool == Tool::Codex { fake_auth("same@example.test", "old") } else { claude_cred("expired-but-refreshable") };
            m.write_live(tool, &old).unwrap();
            if tool == Tool::Claude { set_claude_identity(&env, "same@example.test"); }
            let original = m.save_current(tool, "Work").unwrap();
            m.launch_environment(tool).unwrap();
            m.set_usage(tool, &original.id, Usage::unavailable("network timeout"));
            m.check_profile_credentials();
            assert!(!m.login_required(tool, &original.id));
            std::fs::remove_file(m.runtime_dir(tool, &original.id).join(if tool == Tool::Claude { ".credentials.json" } else { "auth.json" })).unwrap();
            m.check_profile_credentials();
            assert!(m.login_required(tool, &original.id));
            m.save_relogin_locked(tool, &original.id, crate::login::LoginCredentials { secret: old, email: original.email.clone(), oauth: None }).unwrap();
            m.check_profile_credentials();
            assert!(!m.login_required(tool, &original.id), "staged credentials must expose Apply rather than ask for another login");
        }
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
