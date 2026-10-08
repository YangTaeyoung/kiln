//! Provider CLIs execute without a shell, interactive stdin, or credential logs.
use crate::{Control, ObjectProvider};
use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn find(name: &str) -> Option<PathBuf> {
    let mut dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    let cwd = std::env::current_dir().ok();
    dirs.into_iter()
        .filter_map(|d| {
            if d.is_absolute() {
                Some(d)
            } else {
                cwd.as_ref().map(|p| p.join(d))
            }
        })
        .map(|d| d.join(name))
        .find(|p| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                p.is_file()
            }
        })
}
struct OwnedChild {
    child: Child,
    finished: bool,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.finished {
            // On Unix the leader is deliberately not reaped before stdout EOF.
            // Its zombie PID anchors the owned group if descendants retain a pipe.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn exited(child: &mut Child) -> Result<bool> {
    #[cfg(unix)]
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        if libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(info.si_pid() != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(child.try_wait()?.is_some())
    }
}
pub(crate) fn execute(
    provider: ObjectProvider,
    args: &[String],
    env: &[(String, String)],
    control: &Control,
) -> Result<Vec<u8>> {
    let executable = find(provider.cli()).with_context(|| {
        format!(
            "Install {} to use this authentication source",
            provider.cli()
        )
    })?;
    execute_at(
        provider,
        &executable,
        args,
        env,
        control,
        Duration::from_secs(300),
        16 * 1024 * 1024,
    )
}
fn command(
    provider: ObjectProvider,
    executable: &Path,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> Command {
    let mut cmd = Command::new(executable);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A selected profile must not silently authenticate as a different env account.
    let remove: &[&str] = match provider {
        ObjectProvider::Google => &[
            "CLOUDSDK_ACTIVE_CONFIG_NAME",
            "CLOUDSDK_CORE_ACCOUNT",
            "CLOUDSDK_AUTH_ACCESS_TOKEN",
            "CLOUDSDK_AUTH_ACCESS_TOKEN_FILE",
            "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE",
            "CLOUDSDK_AUTH_IMPERSONATE_SERVICE_ACCOUNT",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ],
        ObjectProvider::Cloudflare => &[
            "CLOUDFLARE_API_TOKEN",
            "CF_API_TOKEN",
            "CLOUDFLARE_API_KEY",
            "CF_API_KEY",
            "CLOUDFLARE_EMAIL",
            "CF_EMAIL",
            "WRANGLER_API_ENVIRONMENT",
            "CLOUDFLARE_API_BASE_URL",
        ],
        ObjectProvider::Oracle => &[
            "OCI_CLI_AUTH",
            "OCI_CLI_PROFILE",
            "OCI_CLI_CONFIG_FILE",
            "OCI_CLI_REGION",
            "OCI_CLI_USER",
            "OCI_CLI_TENANCY",
            "OCI_CLI_FINGERPRINT",
            "OCI_CLI_KEY_FILE",
            "OCI_CLI_KEY_CONTENT",
            "OCI_CLI_PASSPHRASE",
            "OCI_CLI_SECURITY_TOKEN_FILE",
        ],
    };
    for name in remove {
        cmd.env_remove(name);
    }
    // Wrangler's XDG path is literal (no shell tilde expansion). Resolve a
    // relative override before entering the neutral command directory.
    if provider == ObjectProvider::Cloudflare {
        if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
            let base = PathBuf::from(base);
            if base.is_relative() {
                if let Ok(source_cwd) = std::env::current_dir() {
                    cmd.env("XDG_CONFIG_HOME", source_cwd.join(base));
                }
            }
        }
    }
    cmd.env("NO_COLOR", "1").env("CI", "true");
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}
pub(crate) fn execute_at(
    provider: ObjectProvider,
    executable: &Path,
    args: &[String],
    env: &[(String, String)],
    control: &Control,
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>> {
    control.check()?;
    let cwd = tempfile::tempdir().context("Cannot prepare provider command")?;
    let mut child = OwnedChild {
        child: command(provider, executable, args, env, cwd.path())
            .spawn()
            .with_context(|| format!("Cannot start {}", provider.cli()))?,
        finished: false,
    };
    let mut stdout = child
        .child
        .stdout
        .take()
        .context("Missing provider command output")?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let r = stdout
            .by_ref()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(r);
    });
    let started = Instant::now();
    loop {
        control.check()?;
        if started.elapsed() > timeout {
            bail!("{} command timed out", provider.cli());
        }
        if exited(&mut child.child)? {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let bytes = loop {
        control.check()?;
        if started.elapsed() > timeout {
            bail!("{} command output timed out", provider.cli());
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(r) => break r.context("Cannot read provider output")?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("Provider output reader failed"),
        }
    };
    if bytes.len() > limit {
        bail!("Provider command output exceeded the safe limit");
    }
    let status = child.child.wait()?;
    child.finished = true;
    if !status.success() {
        bail!(
            "{} command failed; check the selected profile and its permissions",
            provider.cli()
        );
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn script(d: &tempfile::TempDir, text: &str) -> PathBuf {
        let p = d.path().join("cli");
        std::fs::write(&p, text).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        p
    }
    #[test]
    fn argv_is_literal_and_errors_never_echo_credentials() {
        let d = tempfile::tempdir().unwrap();
        let p = script(&d, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
        let got = execute_at(
            ObjectProvider::Oracle,
            &p,
            &["--name=a; echo leaked".into(), "한글 [file]".into()],
            &[],
            &Control::default(),
            Duration::from_secs(2),
            1024,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "--name=a; echo leaked\n한글 [file]\n"
        );
        let p = script(&d, "#!/bin/sh\necho private-token >&2\nexit 7\n");
        assert!(
            !execute_at(
                ObjectProvider::Google,
                &p,
                &[],
                &[],
                &Control::default(),
                Duration::from_secs(2),
                1024
            )
            .unwrap_err()
            .to_string()
            .contains("private-token")
        );
    }
    #[test]
    fn timeout_cancel_and_output_limit_are_bounded() {
        let d = tempfile::tempdir().unwrap();
        let p = script(&d, "#!/bin/sh\nsleep 30\n");
        let start = Instant::now();
        assert!(
            execute_at(
                ObjectProvider::Google,
                &p,
                &[],
                &[],
                &Control::default(),
                Duration::from_millis(75),
                1024
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let c = std::sync::Arc::new(Control::default());
        let cc = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            cc.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(
            execute_at(
                ObjectProvider::Google,
                &p,
                &[],
                &[],
                &c,
                Duration::from_secs(30),
                1024
            )
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
        let p = script(&d, "#!/bin/sh\nprintf 123456789\n");
        assert!(
            execute_at(
                ObjectProvider::Google,
                &p,
                &[],
                &[],
                &Control::default(),
                Duration::from_secs(2),
                4
            )
            .unwrap_err()
            .to_string()
            .contains("limit")
        );
    }
    #[test]
    fn selected_profile_removes_credential_environment_overrides() {
        let d = tempfile::tempdir().unwrap();
        for (provider, name) in [
            (ObjectProvider::Google, "CLOUDSDK_AUTH_ACCESS_TOKEN"),
            (ObjectProvider::Cloudflare, "CLOUDFLARE_API_TOKEN"),
            (ObjectProvider::Oracle, "OCI_CLI_KEY_CONTENT"),
        ] {
            let cmd = command(provider, Path::new("fixture"), &[], &[], d.path());
            assert!(cmd.get_envs().any(|(k, v)| k == name && v.is_none()));
        }
    }
    #[test]
    fn exited_leader_with_pipe_holding_descendant_is_cleaned_up() {
        let d = tempfile::tempdir().unwrap();
        let pid_file = d.path().join("pid");
        let p = script(
            &d,
            "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > \"$1\"\nexit 0\n",
        );
        let err = execute_at(
            ObjectProvider::Google,
            &p,
            &[pid_file.to_string_lossy().into_owned()],
            &[],
            &Control::default(),
            Duration::from_secs(2),
            1024,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("output timed out"),
            "The leader did not reach the exited-with-open-pipe phase: {err:#}"
        );
        let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let start = Instant::now();
        loop {
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            // Linux's container init may leave a killed orphan as a zombie.
            #[cfg(target_os = "linux")]
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|s| s.split_whitespace().nth(2) == Some("Z"));
            #[cfg(not(target_os = "linux"))]
            let zombie = false;
            if !alive || zombie {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "Owned CLI descendant survived cancellation"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
