//! The installed, official CLI owns OAuth. Its callback server runs without a
//! terminal panel, under a disposable configuration and credential namespace.
//! URLs and fallback codes are kept only in memory, never in logs or settings.

use crate::{Env, Tool, claude, codex};
use crate::store::read_secret;
use anyhow::{Result, bail};
use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoginStatus {
    pub browser_url: Option<String>,
    pub accepts_code: bool,
}

#[derive(Default)]
pub(crate) struct LoginControl {
    pub cancel: AtomicBool,
    pub status: Mutex<LoginStatus>,
    input: Mutex<Option<ChildStdin>>,
    stop_readers: AtomicBool,
    child: Mutex<Option<Arc<Mutex<Child>>>>,
    finished: Mutex<bool>,
    completion: Condvar,
}

impl LoginControl {
    pub(crate) fn shutdown(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.stop_readers.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.lock().as_ref() {
            // Kill only the disposable OAuth CLI owned by this login job.
            // The worker reaps it and removes its isolated files/keychain entry.
            let _ = child.lock().kill();
        }
        self.input.lock().take();
    }

    fn register_child(&self, child: Arc<Mutex<Child>>) {
        let mut slot = self.child.lock();
        if self.cancel.load(Ordering::Relaxed) {
            let _ = child.lock().kill();
        }
        *slot = Some(child);
    }

    pub(crate) fn wait_for_cleanup(&self, deadline: Instant) -> bool {
        let mut finished = self.finished.lock();
        while !*finished {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return false; }
            self.completion.wait_for(&mut finished, remaining);
        }
        true
    }

    pub(crate) fn send_code(&self, code: &str) -> Result<()> {
        let code = code.trim();
        if code.is_empty() || code.len() > 4096 || code.contains(['\n', '\r', '\0']) {
            bail!(kiln_common::i18n::tr("브라우저에서 받은 로그인 코드를 입력하세요"));
        }
        if !self.status.lock().accepts_code {
            bail!(kiln_common::i18n::tr("현재 로그인은 코드 입력을 기다리지 않습니다"));
        }
        let mut input = self.input.lock();
        let Some(input) = input.as_mut() else {
            bail!(kiln_common::i18n::tr("로그인 대기가 종료되었습니다. 다시 시도하세요"));
        };
        writeln!(input, "{code}").map_err(|_| anyhow::anyhow!(kiln_common::i18n::tr("로그인 코드를 전달하지 못했습니다. 다시 시도하세요")))?;
        input.flush()?;
        Ok(())
    }
}

pub(crate) struct LoginCredentials {
    pub secret: String,
    pub oauth: Option<serde_json::Value>,
    pub email: Option<String>,
}

/// Matches the installed Claude CLI's VN(Hhe): a custom config directory uses
/// `Claude Code-credentials-<sha256(config)[0..8]>`, keeping live login untouched.
pub(crate) fn claude_service(config: Option<&Path>) -> String {
    match config {
        Some(path) => {
            let hash = format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()));
            format!("{}-{}", claude::CLAUDE_SERVICE, &hash[..8])
        }
        None => claude::CLAUDE_SERVICE.to_string(),
    }
}

struct IsolatedLogin {
    dir: tempfile::TempDir,
    env: Env,
    system_home: PathBuf,
}

impl IsolatedLogin {
    fn new(source: &Env) -> Result<Self> {
        let dir = tempfile::Builder::new().prefix("kiln-account-login-").tempdir()?;
        let mut env = source.clone();
        env.home = dir.path().join("home");
        env.codex_home = dir.path().join("codex");
        env.claude_config_dir = Some(dir.path().join("claude"));
        for path in [&env.home, &env.codex_home, env.claude_config_dir.as_ref().unwrap()] {
            std::fs::create_dir_all(path)?;
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(Self { dir, env, system_home: source.home.clone() })
    }

    fn command(&self, tool: Tool, binary: &Path) -> Command {
        let mut cmd = Command::new(binary);
        match tool {
            Tool::Codex => { cmd.args(["login", "-c", "cli_auth_credentials_store=\"file\""]); }
            Tool::Claude => { cmd.args(["auth", "login", "--claudeai"]); }
        }
        // Keychain lookup uses HOME on macOS. Claude's documented config-dir
        // isolation is sufficient; changing HOME hides the user's login keychain.
        // Keep env.home disposable for credential/metadata file reads below.
        let home = if tool == Tool::Claude { &self.system_home } else { &self.env.home };
        cmd.current_dir(self.dir.path())
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CODEX_HOME", &self.env.codex_home)
            .env("CLAUDE_CONFIG_DIR", self.env.claude_config_dir.as_ref().unwrap())
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CODEX_ACCESS_TOKEN")
            .env_remove("CLAUDE_CODE_USE_BEDROCK")
            .env_remove("CLAUDE_CODE_USE_VERTEX")
            .env_remove("CLAUDE_CODE_USE_FOUNDRY")
            .env_remove("CLAUDE_CODE_HOST_CREDS_FILE")
            .env("TERM", "dumb")
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        cmd
    }

    fn credentials(&self, tool: Tool) -> Result<LoginCredentials> {
        let (secret, oauth, email) = match tool {
            Tool::Codex => {
                let secret = read_secret(&codex::auth_path(&self.env.codex_home))?
                    .ok_or_else(|| anyhow::anyhow!(kiln_common::i18n::tr("로그인은 완료됐지만 계정 정보를 읽지 못했습니다. 다시 시도하세요")))?;
                if !codex::has_chatgpt_tokens(&secret) { bail!(kiln_common::i18n::tr("ChatGPT 구독 계정으로 로그인하세요")); }
                let email = codex::auth_email(&secret);
                (secret, None, email)
            }
            Tool::Claude => {
                let service = claude_service(self.env.claude_config_dir.as_deref());
                // Official CLI falls back to a private file when Keychain writes
                // fail. Read only this job's isolated file before asking Keychain.
                let secret = match read_secret(&claude::credentials_path(&self.env.home, self.env.claude_config_dir.as_deref()))? {
                    Some(secret) => Some(secret),
                    None if self.env.claude_live_in_store => self.env.store.get_by_service(&service)?.map(|(secret, _)| secret),
                    None => None,
                }
                    .ok_or_else(|| anyhow::anyhow!(kiln_common::i18n::tr("로그인은 완료됐지만 계정 정보를 읽지 못했습니다. 다시 시도하세요")))?;
                claude::access_token(&secret)?;
                let config = self.env.claude_config_dir.as_ref().unwrap();
                let oauth = [config.join(".config.json"), config.join(".claude.json"), self.env.home.join(".claude.json")]
                    .iter().find_map(|path| claude::read_oauth_account(path).ok().flatten());
                let email = oauth.as_ref().and_then(claude::oauth_email);
                (secret, oauth, email)
            }
        };
        Ok(LoginCredentials { secret, oauth, email })
    }
}

impl Drop for IsolatedLogin {
    fn drop(&mut self) {
        if self.env.claude_live_in_store {
            // Only the exact disposable namespace created by this job is removed.
            let service = claude_service(self.env.claude_config_dir.as_deref());
            if let Ok(Some((_, account))) = self.env.store.get_by_service(&service) {
                let _ = self.env.store.delete(&service, &account);
            }
        }
    }
}

struct OwnedChild(Arc<Mutex<Child>>);
impl Drop for OwnedChild {
    fn drop(&mut self) { let mut child = self.0.lock(); let _ = child.kill(); let _ = child.wait(); }
}

struct LoginCompletion<'a>(&'a LoginControl);
impl Drop for LoginCompletion<'_> {
    fn drop(&mut self) {
        self.0.input.lock().take();
        self.0.child.lock().take();
        *self.0.finished.lock() = true;
        self.0.completion.notify_all();
    }
}

#[cfg(unix)]
fn output_pipe<T: Read + Send + std::os::fd::AsRawFd + 'static>(pipe: T) -> Result<Box<dyn Read + Send>> {
    let fd = pipe.as_raw_fd();
    // A browser opener or other descendant can inherit stdout. Nonblocking
    // reads let our own readers stop even while that descendant holds it open.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        bail!(kiln_common::i18n::tr("로그인 대기를 시작하지 못했습니다. 다시 시도하세요"));
    }
    Ok(Box::new(pipe))
}

#[cfg(not(unix))]
fn output_pipe<T: Read + Send + 'static>(pipe: T) -> Result<Box<dyn Read + Send>> { Ok(Box::new(pipe)) }

pub(crate) fn run_login(source: &Env, tool: Tool, binary: &Path, control: &Arc<LoginControl>, timeout: Duration, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<LoginCredentials> {
    // Declared first so completion is signalled only after OwnedChild and the
    // isolated credential namespace have both been dropped, including errors.
    let _completion = LoginCompletion(control);
    let isolated = IsolatedLogin::new(source)?;
    if control.cancel.load(Ordering::Relaxed) { bail!(kiln_common::i18n::tr("로그인을 취소했습니다")); }
    let child = OwnedChild(Arc::new(Mutex::new(isolated.command(tool, binary).spawn()
        .map_err(|_| anyhow::anyhow!(kiln_common::trf!("{} 로그인 프로그램을 시작하지 못했습니다. 설치 상태를 확인하세요", tool.display_name())))?)));
    control.register_child(child.0.clone());
    let pipes = {
        let mut child = child.0.lock();
        *control.input.lock() = child.stdin.take();
        [child.stdout.take().map(output_pipe).transpose()?, child.stderr.take().map(output_pipe).transpose()?]
    };
    let mut readers = Vec::new();
    for pipe in pipes {
        if let Some(mut pipe) = pipe {
            let control = control.clone();
            let repaint = repaint.clone();
            readers.push(std::thread::spawn(move || {
                let mut scanner = OutputScanner::default();
                let mut chunk = [0u8; 2048];
                while !control.stop_readers.load(Ordering::Relaxed) {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => { if scanner.feed(&chunk[..n], tool, &control) { repaint(); } }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            }));
        }
    }
    let start = Instant::now();
    let result = loop {
        if control.cancel.load(Ordering::Relaxed) { break Err(anyhow::anyhow!(kiln_common::i18n::tr("로그인을 취소했습니다"))); }
        if start.elapsed() >= timeout { break Err(anyhow::anyhow!(kiln_common::i18n::tr("로그인 시간이 만료되었습니다. 다시 시도하세요"))); }
        let status = child.0.lock().try_wait();
        match status {
            Ok(Some(status)) if status.success() => break isolated.credentials(tool),
            Ok(Some(_)) => break Err(anyhow::anyhow!(kiln_common::i18n::tr("로그인을 완료하지 못했습니다. 브라우저와 네트워크 상태를 확인한 뒤 다시 시도하세요"))),
            Err(_) => break Err(anyhow::anyhow!(kiln_common::i18n::tr("로그인 대기가 종료되었습니다. 다시 시도하세요"))),
            Ok(None) => std::thread::sleep(Duration::from_millis(60)),
        }
    };
    { let mut child = child.0.lock(); let _ = child.kill(); let _ = child.wait(); }
    control.input.lock().take();
    control.stop_readers.store(true, Ordering::Relaxed);
    // Unix readers are nonblocking and stop within one polling interval. A
    // blocked Windows pipe reader must not block login cancellation/UI state.
    #[cfg(unix)] for reader in readers { let _ = reader.join(); }
    #[cfg(not(unix))] drop(readers);
    result
}

#[derive(Default)]
struct OutputScanner { tail: Vec<u8> }
impl OutputScanner {
    fn feed(&mut self, chunk: &[u8], tool: Tool, control: &LoginControl) -> bool {
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > 16384 { self.tail.drain(..self.tail.len() - 16384); }
        let text = String::from_utf8_lossy(&self.tail);
        let mut status = control.status.lock();
        let old = status.clone();
        if tool == Tool::Claude && text.contains("Paste code here if prompted") { status.accepts_code = true; }
        let delimiter = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '\"' | '\'' | '<' | '>');
        // URLs may be split between reads or wrapped in an OSC-8 hyperlink by
        // Claude. Find their actual start and require a terminating separator.
        for (start, _) in text.match_indices("https://") {
            let remaining = &text[start..];
            let Some(end) = remaining.find(delimiter) else { continue; };
            let token = &remaining[..end];
            let valid = match tool {
                Tool::Codex => token.starts_with("https://auth.openai.com/oauth/authorize?"),
                Tool::Claude => token.starts_with("https://claude.ai/oauth/authorize?") || token.starts_with("https://console.anthropic.com/oauth/authorize?") || token.starts_with("https://platform.claude.com/oauth/authorize?"),
            };
            // Require complete OAuth parameters to avoid reopening a split URL.
            if valid && token.contains("state=") && token.contains("client_id=") && token.contains("redirect_uri=") {
                status.browser_url = Some(token.to_string());
            }
        }
        *status != old
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CredentialStore;

    #[test]
    fn scanner_accepts_only_complete_provider_authorization_urls_and_code_prompt() {
        let job = LoginControl::default();
        let mut scanner = OutputScanner::default();
        let url = "https://auth.openai.com/oauth/authorize?client_id=public&redirect_uri=http%3A%2F%2Flocalhost&state=fixture";
        assert!(!scanner.feed(&url.as_bytes()[..60], Tool::Codex, &job));
        assert!(!scanner.feed(&url.as_bytes()[60..], Tool::Codex, &job));
        assert!(scanner.feed(b"\n", Tool::Codex, &job));
        assert_eq!(job.status.lock().browser_url.as_deref(), Some(url));
        let job = LoginControl::default();
        let mut scanner = OutputScanner::default();
        scanner.feed(b"https://auth.openai.com.evil.test/oauth/authorize?client_id=x&redirect_uri=x&state=x\n", Tool::Codex, &job);
        assert!(job.status.lock().browser_url.is_none());
        scanner.feed(b"Paste code here if ", Tool::Claude, &job);
        scanner.feed(b"prompted > ", Tool::Claude, &job);
        assert!(job.status.lock().accepts_code);
        scanner.feed(b"\x1b]8;;https://claude.ai/oauth/authorize?client_id=x&redirect_uri=x&state=x\x07login\x1b]8;;\x07", Tool::Claude, &job);
        assert_eq!(job.status.lock().browser_url.as_deref(), Some("https://claude.ai/oauth/authorize?client_id=x&redirect_uri=x&state=x"));
        assert!(job.send_code("code\ncommand").is_err());
    }

    #[test]
    fn isolated_paths_have_distinct_keychain_names_and_restrictive_permissions() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), true);
        let login = IsolatedLogin::new(&env).unwrap();
        let service = claude_service(login.env.claude_config_dir.as_deref());
        assert_ne!(service, claude::CLAUDE_SERVICE);
        assert_eq!(service.len(), claude::CLAUDE_SERVICE.len() + 9);
        assert!(!login.env.home.starts_with(&env.home));
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(login.env.home.metadata().unwrap().permissions().mode() & 0o777, 0o700); }
    }

    #[test]
    fn claude_login_keeps_system_home_and_isolates_only_cli_files() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), true);
        let login = IsolatedLogin::new(&env).unwrap();
        for tool in Tool::ALL {
            let command = login.command(tool, Path::new("fixture-cli"));
            let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
            let expected = if tool == Tool::Claude { &env.home } else { &login.env.home };
            assert_eq!(vars[std::ffi::OsStr::new("HOME")], Some(expected.as_os_str()));
            assert_eq!(vars[std::ffi::OsStr::new("USERPROFILE")], Some(expected.as_os_str()));
            assert_eq!(vars[std::ffi::OsStr::new("CLAUDE_CONFIG_DIR")], Some(login.env.claude_config_dir.as_ref().unwrap().as_os_str()));
            assert_eq!(vars[std::ffi::OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR")], None);
        }
        assert_ne!(login.env.home, env.home);
        assert_ne!(login.env.claude_config_dir, env.claude_config_dir);
    }

    #[test]
    fn claude_file_fallback_does_not_read_keychain_or_live_account_metadata() {
        struct BrokenStore;
        impl crate::CredentialStore for BrokenStore {
            fn get(&self, _: &str, _: &str) -> Result<Option<String>> { panic!("must use isolated file") }
            fn get_by_service(&self, _: &str) -> Result<Option<(String, String)>> { panic!("must use isolated file") }
            fn set(&self, _: &str, _: &str, _: &str) -> Result<()> { panic!("no writes") }
            fn delete(&self, _: &str, _: &str) -> Result<bool> { panic!("no deletes") }
        }
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), true);
        env.store = Arc::new(BrokenStore);
        claude::write_oauth_account(&env.home.join(".claude.json"), Some(&serde_json::json!({"emailAddress":"existing@example.test"}))).unwrap();
        let mut login = IsolatedLogin::new(&env).unwrap();
        crate::store::write_secret_atomic(&claude::credentials_path(&login.env.home, login.env.claude_config_dir.as_deref()), br#"{"claudeAiOauth":{"accessToken":"isolated-fixture"}}"#).unwrap();
        let credential = login.credentials(Tool::Claude).unwrap();
        assert_eq!(claude::access_token(&credential.secret).unwrap(), "isolated-fixture");
        assert_eq!(credential.email, None);
        // This fixture intentionally cannot access a credential store, including Drop.
        login.env.claude_live_in_store = false;
    }

    #[test]
    fn isolated_claude_keychain_is_cleaned_without_deleting_live_account() {
        let root = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(root.path(), true);
        store.set(claude::CLAUDE_SERVICE, "tester", "live-fixture").unwrap();
        let login = IsolatedLogin::new(&env).unwrap();
        let service = claude_service(login.env.claude_config_dir.as_deref());
        store.set(&service, "tester", "isolated-fixture").unwrap();
        drop(login);
        assert!(store.get_by_service(&service).unwrap().is_none());
        assert_eq!(store.get_by_service(claude::CLAUDE_SERVICE).unwrap().unwrap().0, "live-fixture");
    }

    #[cfg(unix)]
    fn script(root: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake-login");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn subprocess_success_uses_isolated_auth_without_overwriting_live_credentials() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), false);
        std::fs::create_dir_all(&env.codex_home).unwrap();
        let live = codex::auth_path(&env.codex_home);
        std::fs::write(&live, "existing-account-fixture").unwrap();
        let auth = codex::tests::fake_auth("new@example.test", "new");
        let binary = script(root.path(), &format!("test \"$1\" = login || exit 2\ntest \"$4\" = 'cli_auth_credentials_store=\"file\"' || test \"$3\" = 'cli_auth_credentials_store=\"file\"' || exit 3\nprintf '%s' '{}' > \"$CODEX_HOME/auth.json\"", auth));
        let job = Arc::new(LoginControl::default());
        let credential = run_login(&env, Tool::Codex, &binary, &job, Duration::from_secs(2), Arc::new(|| {})).unwrap();
        assert_eq!(credential.email.as_deref(), Some("new@example.test"));
        assert_eq!(std::fs::read_to_string(live).unwrap(), "existing-account-fixture");
    }

    #[test]
    #[cfg(unix)]
    fn failure_cancel_and_timeout_leave_live_credentials_untouched_and_redact_output() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), false);
        std::fs::create_dir_all(&env.codex_home).unwrap();
        let live = codex::auth_path(&env.codex_home);
        std::fs::write(&live, "existing-account-fixture").unwrap();
        let binary = script(root.path(), "printf 'secret-code-do-not-display' >&2\nexit 1");
        let job = Arc::new(LoginControl::default());
        let error = run_login(&env, Tool::Codex, &binary, &job, Duration::from_secs(1), Arc::new(|| {})).err().unwrap();
        assert!(!error.to_string().contains("secret-code"));
        let binary = script(root.path(), "exec /bin/sleep 5");
        let start = Instant::now();
        assert!(run_login(&env, Tool::Codex, &binary, &job, Duration::from_millis(80), Arc::new(|| {})).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        job.cancel.store(true, Ordering::Relaxed);
        assert!(run_login(&env, Tool::Codex, &binary, &job, Duration::from_secs(1), Arc::new(|| {})).is_err());
        assert_eq!(std::fs::read_to_string(live).unwrap(), "existing-account-fixture");
    }

    #[test]
    #[cfg(unix)]
    fn timeout_finishes_when_a_descendant_keeps_the_output_pipe_open() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), false);
        let binary = script(root.path(), "/bin/sleep 1 &\nexec /bin/sleep 5");
        let job = Arc::new(LoginControl::default());
        let start = Instant::now();
        assert!(run_login(&env, Tool::Codex, &binary, &job, Duration::from_millis(80), Arc::new(|| {})).is_err());
        assert!(start.elapsed() < Duration::from_millis(600), "an inherited pipe must not extend login timeout");
    }

    #[test]
    #[cfg(unix)]
    fn claude_fallback_code_reaches_hidden_stdin_and_metadata_is_read_from_isolated_config() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), false);
        let binary = script(root.path(), "printf 'Paste code here if prompted > '\nread code\ntest \"$code\" = 'fixture-code' || exit 2\nprintf '%s' '{\"claudeAiOauth\":{\"accessToken\":\"fixture\",\"refreshToken\":\"fixture-refresh\"}}' > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\nprintf '%s' '{\"oauthAccount\":{\"emailAddress\":\"claude@example.test\"}}' > \"$CLAUDE_CONFIG_DIR/.claude.json\"");
        let job = Arc::new(LoginControl::default());
        let send = job.clone();
        let sender = std::thread::spawn(move || {
            let start = Instant::now();
            while !send.status.lock().accepts_code { assert!(start.elapsed() < Duration::from_secs(2)); std::thread::sleep(Duration::from_millis(10)); }
            send.send_code("fixture-code").unwrap();
        });
        let result = run_login(&env, Tool::Claude, &binary, &job, Duration::from_secs(3), Arc::new(|| {})).unwrap();
        sender.join().unwrap();
        assert_eq!(result.email.as_deref(), Some("claude@example.test"));
        assert!(!env.home.join(".claude.json").exists());
    }
}

pub(crate) fn find_cli(home: &Path, tool: Tool) -> Option<PathBuf> {
    let name = if cfg!(windows) { format!("{}.exe", tool.key()) } else { tool.key().to_string() };
    let mut roots = std::env::var_os("PATH").map(|v| std::env::split_paths(&v).collect::<Vec<_>>()).unwrap_or_default();
    roots.extend([home.join(".local/bin"), home.join(".cargo/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]);
    roots.into_iter().map(|r| r.join(&name)).find(|path| {
        let Ok(metadata) = path.metadata() else { return false; };
        if !metadata.is_file() { return false; }
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; metadata.permissions().mode() & 0o111 != 0 }
        #[cfg(not(unix))] { true }
    })
}
