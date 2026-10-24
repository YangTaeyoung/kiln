//! The installed, official CLI owns OAuth. Its callback server runs without a
//! terminal panel, under a disposable configuration and credential namespace.
//! URLs and fallback codes are kept only in memory, never in logs or settings.

use crate::{Env, Tool, antigravity, claude, codex};
use crate::store::read_secret;
use anyhow::{Context, Result, bail};
use parking_lot::{Condvar, Mutex};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    pub profile_id: Option<String>,
    pub status: Mutex<LoginStatus>,
    input: Mutex<Option<Box<dyn Write + Send>>>,
    terminal_input: AtomicBool,
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
        if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
            bail!(kiln_common::i18n::tr("브라우저에서 받은 로그인 코드를 입력하세요"));
        }
        if !self.status.lock().accepts_code {
            bail!(kiln_common::i18n::tr("현재 로그인은 코드 입력을 기다리지 않습니다"));
        }
        let mut input = self.input.lock();
        let Some(input) = input.as_mut() else {
            bail!(kiln_common::i18n::tr("로그인 대기가 종료되었습니다. 다시 시도하세요"));
        };
        // Agy's raw terminal treats CR as Enter; the other CLIs read lines from pipes.
        let enter = if self.terminal_input.load(Ordering::Relaxed) { b"\r" } else { b"\n" };
        input.write_all(code.as_bytes()).and_then(|_| input.write_all(enter))
            .map_err(|_| anyhow::anyhow!(kiln_common::i18n::tr("로그인 코드를 전달하지 못했습니다. 다시 시도하세요")))?;
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
    antigravity_attempt: bool,
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
        Ok(Self { dir, env, system_home: source.home.clone(), antigravity_attempt: false })
    }

    fn command(&self, tool: Tool, binary: &Path) -> Command {
        let mut cmd = Command::new(binary);
        match tool {
            Tool::Codex => { cmd.args(["login", "-c", "cli_auth_credentials_store=\"file\""]); }
            Tool::Claude => { cmd.args(["auth", "login", "--claudeai"]); }
            Tool::Antigravity => {}
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
                let secret = self.env.claude_secret(self.env.claude_config_dir.as_deref())?.map(|x| x.0)
                    .ok_or_else(|| anyhow::anyhow!(kiln_common::i18n::tr("로그인은 완료됐지만 계정 정보를 읽지 못했습니다. 다시 시도하세요")))?;
                claude::access_token(&secret)?;
                let config = self.env.claude_config_dir.as_ref().unwrap();
                let oauth = [config.join(".config.json"), config.join(".claude.json"), self.env.home.join(".claude.json")]
                    .iter().find_map(|path| claude::read_oauth_account(path).ok().flatten());
                let email = oauth.as_ref().and_then(claude::oauth_email);
                (secret, oauth, email)
            }
            Tool::Antigravity => {
                let secret = antigravity::read(&self.env, &self.env.home)?.context("Antigravity sign-in required")?;
                let email = (self.env.antigravity_identity)(&secret)?;
                (secret, None, Some(email))
            },
        };
        Ok(LoginCredentials { secret, oauth, email })
    }
}

impl Drop for IsolatedLogin {
    fn drop(&mut self) {
        if self.antigravity_attempt {
            let _ = antigravity::cleanup(&self.env, &self.env.home);
        }
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
    if tool == Tool::Antigravity { return run_antigravity_login(source, binary, control, timeout, repaint); }
    let isolated = IsolatedLogin::new(source)?;
    if control.cancel.load(Ordering::Relaxed) { bail!(kiln_common::i18n::tr("로그인을 취소했습니다")); }
    let child = OwnedChild(Arc::new(Mutex::new(isolated.command(tool, binary).spawn()
        .map_err(|_| anyhow::anyhow!(kiln_common::trf!("{} 로그인 프로그램을 시작하지 못했습니다. 설치 상태를 확인하세요", tool.display_name())))?)));
    control.register_child(child.0.clone());
    let pipes = {
        let mut child = child.0.lock();
        *control.input.lock() = child.stdin.take().map(|input| Box::new(input) as Box<dyn Write + Send>);
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

/// Agy 1.4.6 requires a controlling terminal for its official sign-in selector.
/// This terminal belongs only to this attempt and never receives a model prompt.
#[cfg(target_os = "macos")]
fn run_antigravity_login(source: &Env, binary: &Path, control: &Arc<LoginControl>, timeout: Duration, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<LoginCredentials> {
    use std::os::fd::{FromRawFd, AsRawFd};
    use std::os::unix::process::CommandExt;
    let mut isolated = IsolatedLogin::new(source)?;
    isolated.antigravity_attempt = true;
    antigravity::prepare(&isolated.env, &isolated.env.home)?;
    let (mut master, mut slave) = (-1, -1);
    // Wide enough that the authorization URL is not wrapped by the CLI renderer.
    let mut size = libc::winsize { ws_row: 40, ws_col: 4096, ws_xpixel: 0, ws_ypixel: 0 };
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) } != 0 { return Err(std::io::Error::last_os_error().into()); }
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = isolated.command(Tool::Antigravity, binary);
    command.envs(antigravity::environment(&isolated.env.home)).env("TERM", "xterm-256color")
        .stdin(slave.try_clone()?).stdout(slave.try_clone()?).stderr(slave);
    unsafe { command.pre_exec(|| {
        if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 { return Err(std::io::Error::last_os_error()); }
        Ok(())
    }); }
    if control.cancel.load(Ordering::Relaxed) { bail!("{}", kiln_common::i18n::tr("로그인을 취소했습니다")); }
    let child = OwnedChild(Arc::new(Mutex::new(command.spawn().context("could not start Antigravity sign-in")?)));
    // All descendants of this attempt share its new process group.
    struct Group(u32);
    impl Drop for Group { fn drop(&mut self) { unsafe { libc::kill(-(self.0 as i32), libc::SIGKILL); } } }
    let _group = Group(child.0.lock().id());
    control.register_child(child.0.clone());
    control.terminal_input.store(true, Ordering::Relaxed);
    *control.input.lock() = Some(Box::new(master.try_clone()?));
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 { return Err(std::io::Error::last_os_error().into()); }
    let mut reader = master;
    let start = Instant::now();
    let mut next_read = Instant::now();
    let mut screen = Vec::new();
    let mut scanner = OutputScanner::default();
    let mut selected = false;
    loop {
        if control.cancel.load(Ordering::Relaxed) { bail!("{}", kiln_common::i18n::tr("로그인을 취소했습니다")); }
        if start.elapsed() >= timeout { bail!("{}", kiln_common::i18n::tr("로그인 시간이 만료되었습니다. 다시 시도하세요")); }
        let mut bytes = [0; 8192];
        match reader.read(&mut bytes) {
            Ok(n) if n > 0 => {
                screen.extend_from_slice(&bytes[..n]);
                if screen.len() > 65536 { screen.drain(..screen.len()-65536); }
                let text = plain_terminal(&screen);
                if !selected && text.contains("Select login method:") && text.contains("1. Google OAuth") {
                    if let Some(input) = control.input.lock().as_mut() { input.write_all(b"\r")?; input.flush()?; }
                    selected = true;
                }
                if scanner.feed(text.as_bytes(), Tool::Antigravity, control) { repaint(); }
            }
            Err(e) if e.kind() != std::io::ErrorKind::WouldBlock && e.raw_os_error() != Some(libc::EIO) => return Err(e.into()),
            _ => {}
        }
        if Instant::now() >= next_read {
            if antigravity::read(&isolated.env, &isolated.env.home)?.is_some() { return isolated.credentials(Tool::Antigravity); }
            next_read = Instant::now() + Duration::from_secs(1);
        }
        if child.0.lock().try_wait()?.is_some() { bail!("{}", kiln_common::i18n::tr("로그인을 완료하지 못했습니다. 브라우저와 네트워크 상태를 확인한 뒤 다시 시도하세요")); }
        std::thread::sleep(Duration::from_millis(40));
    }
}
#[cfg(not(target_os = "macos"))]
fn run_antigravity_login(_: &Env, _: &Path, _: &Arc<LoginControl>, _: Duration, _: Arc<dyn Fn() + Send + Sync>) -> Result<LoginCredentials> {
    bail!("Antigravity account isolation currently requires macOS")
}
/// Strip CSI styling and OSC controls without copying terminal contents to logs.
#[cfg(target_os = "macos")]
fn plain_terminal(bytes: &[u8]) -> String {
    let mut out = Vec::new(); let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 27 && i+1 < bytes.len() {
            match bytes[i+1] {
                b'[' => { i+=2; while i<bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {i+=1;} i+=1; continue; }
                b']' => { i+=2; while i<bytes.len() && bytes[i]!=7 && !(bytes[i]==27 && bytes.get(i+1)==Some(&b'\\')) {i+=1;} i+=if bytes.get(i)==Some(&27) {2}else{1}; continue; }
                _ => {i+=2;continue;}
            }
        }
        out.push(bytes[i]);i+=1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
        let asks_for_code = match tool {
            Tool::Claude => text.contains("Paste code here if prompted"),
            Tool::Antigravity => {
                let lower = text.to_ascii_lowercase();
                lower.contains("paste the authorization code")
                    || lower.contains("enter the authorization code:")
                    || lower.contains("copy the code displayed in the browser and paste it below:")
            }
            Tool::Codex => false,
        };
        if asks_for_code { status.accepts_code = true; }
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
                Tool::Antigravity => token.starts_with("https://accounts.google.com/o/oauth2/auth?") || token.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"),
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
    #[cfg(target_os = "macos")]
    fn antigravity_pty_login_stages_credentials_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        env.antigravity_identity = Arc::new(|_| Ok("work@example.test".into()));
        let binary = root.path().join("fake-agy");
        std::fs::write(&binary, r##"#!/bin/sh
printf 'Select login method:\n > 1. Google OAuth\n'
read choice
printf 'https://accounts.google.com/o/oauth2/v2/auth?client_id=fixture&state=fixture&redirect_uri=fixture\n'
printf '%s' '{"token":{"access_token":"fixture","refresh_token":"fixture"}}' > "$ANTIGRAVITY_APP_DATA_DIR/antigravity-oauth-token"
sleep 30
"##).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let control = Arc::new(LoginControl::default());
        let credentials = run_login(&env, Tool::Antigravity, &binary, &control, Duration::from_secs(5), Arc::new(|| {})).unwrap();
        assert_eq!(credentials.email.as_deref(), Some("work@example.test"));
        assert_eq!(antigravity::access_token(&credentials.secret).unwrap(), "fixture");
        assert!(control.status.lock().browser_url.as_deref().unwrap().starts_with("https://accounts.google.com/"));
        assert!(control.input.lock().is_none());
        assert!(control.wait_for_cleanup(Instant::now()+Duration::from_secs(1)));
        assert!(!env.home.join(".gemini").exists());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn antigravity_browser_code_reaches_raw_pty_and_completes_login() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        env.antigravity_identity = Arc::new(|_| Ok("work@example.test".into()));
        let binary = root.path().join("fake-agy");
        std::fs::write(&binary, r#"#!/usr/bin/python3
import os, sys, tty, json, time
from pathlib import Path
print('Select login method:\n1. Google OAuth', flush=True)
sys.stdin.readline()
tty.setraw(0)
print('https://accounts.google.com/o/oauth2/v2/auth?client_id=fixture&state=fixture&redirect_uri=fixture', flush=True)
print('Or, paste the authorization code here and press Enter:', flush=True)
code = b''
while True:
    ch = os.read(0, 1)
    if ch == b'\r': break
    code += ch
assert code == b'4/fixture-code', 'incorrect terminal input'
Path(os.environ['ANTIGRAVITY_APP_DATA_DIR'], 'antigravity-oauth-token').write_text(json.dumps({'token':{'access_token':'fixture','refresh_token':'fixture'}}))
time.sleep(30)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let control = Arc::new(LoginControl::default());
        let send = control.clone();
        let sender = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !send.status.lock().accepts_code {
                assert!(Instant::now() < deadline, "Agy code input was never offered");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(send.send_code("bad\x1bcode").is_err());
            send.send_code(" 4/fixture-code ").unwrap();
        });
        let result = run_login(&env, Tool::Antigravity, &binary, &control, Duration::from_secs(6), Arc::new(|| {}));
        sender.join().unwrap();
        let credentials = result.unwrap();
        assert_eq!(credentials.email.as_deref(), Some("work@example.test"));
        assert_eq!(antigravity::access_token(&credentials.secret).unwrap(), "fixture");
        assert!(control.input.lock().is_none());
        assert!(control.wait_for_cleanup(Instant::now()+Duration::from_secs(1)));
    }

    /// Observes only the installed CLI's sign-in prompt in a private keychain.
    /// It never submits an authorization code or starts a model request.
    #[test]
    #[ignore = "requires KILN_AGY_KEYCHAIN_TEST=1 and KILN_AGY_PROMPT_BINARY; creates a private keychain"]
    #[cfg(target_os = "macos")]
    fn native_antigravity_offers_manual_browser_code_input() {
        use std::os::unix::fs::PermissionsExt;
        let real_cli = std::env::var("KILN_AGY_PROMPT_BINARY").expect("explicit agy executable required");
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        env.antigravity_keychains = true;
        let wrapper = root.path().join("agy-prompt-probe");
        std::fs::write(&wrapper, format!("#!/bin/sh\nexport SSH_CONNECTION='fixture fixture fixture fixture'\nexport BROWSER=/usr/bin/true\nexec '{}'\n", real_cli.replace('\'', "'\\''"))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let control = Arc::new(LoginControl::default());
        let observer = control.clone();
        let cancel = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if observer.status.lock().accepts_code { break; }
                std::thread::sleep(Duration::from_millis(20));
            }
            observer.shutdown();
        });
        let result = run_login(&env, Tool::Antigravity, &wrapper, &control, Duration::from_secs(22), Arc::new(|| {}));
        cancel.join().unwrap();
        assert!(control.status.lock().browser_url.is_some(), "no official browser sign-in URL observed: {:?}", result.err());
        assert!(control.status.lock().accepts_code, "installed CLI manual-code prompt was not recognized");
        assert!(control.wait_for_cleanup(Instant::now()+Duration::from_secs(1)));
    }

    #[test]
    fn antigravity_code_prompt_handles_chunking_and_does_not_enable_other_providers() {
        for prompt in ["Paste the authorization code below:", "Or, paste the authorization code here and press Enter:", "Enter the authorization code:", "After authenticating, copy the code displayed in the browser and paste it below:"] {
            let job = LoginControl::default();
            let mut scanner = OutputScanner::default();
            for chunk in prompt.as_bytes().chunks(3) { scanner.feed(chunk, Tool::Antigravity, &job); }
            assert!(job.status.lock().accepts_code, "{prompt}");
            for tool in [Tool::Codex, Tool::Claude] {
                let job = LoginControl::default();
                OutputScanner::default().feed(prompt.as_bytes(), tool, &job);
                assert!(!job.status.lock().accepts_code);
            }
        }
        let job = LoginControl::default();
        OutputScanner::default().feed(b"https://accounts.google.com/o/oauth2/auth?client_id=fixture&state=fixture&redirect_uri=fixture\n", Tool::Antigravity, &job);
        assert!(!job.status.lock().accepts_code, "a URL alone is not an input prompt");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn antigravity_pty_cancel_and_timeout_do_not_save_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (env, store) = Env::sandbox(root.path(), false);
        let binary = root.path().join("fake-agy");
        std::fs::write(&binary, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        for cancel in [false, true] {
            let control = Arc::new(LoginControl::default());
            if cancel { control.cancel.store(true, Ordering::Relaxed); }
            assert!(run_login(&env, Tool::Antigravity, &binary, &control, Duration::from_millis(80), Arc::new(|| {})).is_err());
            assert!(control.wait_for_cleanup(Instant::now()+Duration::from_secs(1)));
            assert!(store.is_empty());
        }
    }

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
        for tool in Tool::ACCOUNTS {
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
    fn claude_file_fallback_requires_absent_namespaced_keychain_and_uses_own_metadata() {
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), true);
        env.store = Arc::new(crate::MemoryStore::new());
        claude::write_oauth_account(&env.home.join(".claude.json"), Some(&serde_json::json!({"emailAddress":"existing@example.test"}))).unwrap();
        let mut login = IsolatedLogin::new(&env).unwrap();
        crate::store::write_secret_atomic(&claude::credentials_path(&login.env.home, login.env.claude_config_dir.as_deref()), br#"{"claudeAiOauth":{"accessToken":"isolated-fixture"}}"#).unwrap();
        let credential = login.credentials(Tool::Claude).unwrap();
        assert_eq!(claude::access_token(&credential.secret).unwrap(), "isolated-fixture");
        assert_eq!(credential.email, None);
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
    let extra = [home.join(".local").join("bin"), home.join(".cargo").join("bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")];
    kiln_common::paths::find_executable(tool.program(), &extra)
}
