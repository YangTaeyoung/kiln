//! Explicit AWS profile discovery and resolution. Discovery never authenticates.
use crate::{Control, Secrets};
use anyhow::{Result, bail};
use ini::Ini;
use kiln_common::i18n::tr;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authentication {
    Static,
    External,
    Missing,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AwsProfile {
    pub name: String,
    pub region: Option<String>,
    pub authentication: Authentication,
}
#[derive(Clone, Debug)]
pub struct AwsFiles {
    pub config: PathBuf,
    pub credentials: PathBuf,
}
impl AwsFiles {
    pub fn current() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".aws");
        Self {
            config: std::env::var_os("AWS_CONFIG_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("config")),
            credentials: std::env::var_os("AWS_SHARED_CREDENTIALS_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("credentials")),
        }
    }
}
struct AuthProcess {
    child: std::process::Child,
    armed: bool,
}
impl Drop for AuthProcess {
    fn drop(&mut self) {
        if self.armed {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
type Settings = BTreeMap<String, BTreeMap<String, String>>;
fn settings(files: &AwsFiles) -> Result<Settings> {
    let mut profiles = Settings::new();
    for (path, config) in [(&files.config, true), (&files.credentials, false)] {
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => bail!("{}", tr("AWS 설정 파일을 읽을 수 없습니다")),
        };
        if metadata.len() > 4 * 1024 * 1024 {
            bail!("{}", tr("AWS 설정 파일을 읽을 수 없습니다"));
        }
        // Parser errors may contain credential values. Never propagate them.
        let ini = Ini::load_from_file(path)
            .map_err(|_| anyhow::anyhow!(tr("AWS 설정 파일을 읽을 수 없습니다")))?;
        for (section, values) in &ini {
            let Some(section) = section else { continue };
            let name = if config {
                if section == "default" {
                    section
                } else if let Some(name) = section.strip_prefix("profile ") {
                    name.trim()
                } else {
                    continue;
                }
            } else {
                section
            };
            if name.is_empty() || name.chars().any(char::is_control) {
                continue;
            }
            let row = profiles.entry(name.into()).or_default();
            for (key, value) in values {
                row.insert(key.to_lowercase(), value.to_owned());
            }
        }
    }
    Ok(profiles)
}
fn authentication(row: &BTreeMap<String, String>) -> Authentication {
    if [
        "role_arn",
        "sso_session",
        "sso_start_url",
        "credential_process",
        "credential_source",
        "web_identity_token_file",
    ]
    .iter()
    .any(|key| row.get(*key).is_some_and(|v| !v.trim().is_empty()))
    {
        Authentication::External
    } else if ["aws_access_key_id", "aws_secret_access_key"]
        .iter()
        .all(|key| row.get(*key).is_some_and(|v| !v.trim().is_empty()))
    {
        Authentication::Static
    } else {
        Authentication::Missing
    }
}
pub fn discover(files: &AwsFiles) -> Result<Vec<AwsProfile>> {
    Ok(settings(files)?
        .into_iter()
        .map(|(name, row)| AwsProfile {
            name,
            region: row.get("region").filter(|r| !r.is_empty()).cloned(),
            authentication: authentication(&row),
        })
        .collect())
}
pub fn cli_path() -> Option<PathBuf> {
    let mut paths = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    paths.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    paths
        .into_iter()
        .map(|p| p.join(if cfg!(windows) { "aws.exe" } else { "aws" }))
        .find(|p| {
            let Ok(m) = std::fs::metadata(p) else {
                return false;
            };
            if !m.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
}
struct DefaultEnvironment {
    access: Option<String>,
    secret: Option<String>,
    token: Option<String>,
    profile: Option<String>,
}
pub(crate) fn resolve_default(control: &Control) -> Result<Option<Secrets>> {
    let value = |key| std::env::var(key).ok().filter(|v| !v.is_empty());
    resolve_default_with(
        &AwsFiles::current(),
        DefaultEnvironment {
            access: value("AWS_ACCESS_KEY_ID"),
            secret: value("AWS_SECRET_ACCESS_KEY"),
            token: value("AWS_SESSION_TOKEN"),
            profile: std::env::var("AWS_PROFILE")
                .ok()
                .or_else(|| std::env::var("AWS_DEFAULT_PROFILE").ok()),
        },
        cli_path().as_deref(),
        control,
    )
}
fn resolve_default_with(
    files: &AwsFiles,
    environment: DefaultEnvironment,
    cli: Option<&Path>,
    control: &Control,
) -> Result<Option<Secrets>> {
    if environment.access.is_some() || environment.secret.is_some() {
        if environment.access.is_none() || environment.secret.is_none() {
            bail!(
                "{}",
                tr("Access Key ID 및 Secret Access Key를 함께 입력하세요")
            );
        }
        return Ok(Some(Secrets {
            access_key: environment.access,
            secret_key: environment.secret,
            session_token: environment.token,
            password: None,
        }));
    }
    if let Some(name) = environment.profile {
        if name.is_empty() {
            bail!("{}", tr("선택한 AWS 프로필을 찾을 수 없습니다"));
        }
        return resolve_with(files, &name, cli, control, Duration::from_secs(30)).map(Some);
    }
    if discover(files)?
        .iter()
        .any(|p| p.name == "default" && p.authentication != Authentication::Missing)
    {
        return resolve_with(files, "default", cli, control, Duration::from_secs(30)).map(Some);
    }
    Ok(None)
}
pub(crate) fn resolve(name: &str, control: &Control) -> Result<Secrets> {
    resolve_with(
        &AwsFiles::current(),
        name,
        cli_path().as_deref(),
        control,
        Duration::from_secs(30),
    )
}
fn resolve_with(
    files: &AwsFiles,
    name: &str,
    cli: Option<&Path>,
    control: &Control,
    timeout: Duration,
) -> Result<Secrets> {
    control.check()?;
    let profiles = settings(files)?;
    let row = profiles
        .get(name)
        .ok_or_else(|| anyhow::anyhow!(tr("선택한 AWS 프로필을 찾을 수 없습니다")))?;
    match authentication(row) {
        Authentication::Static => Ok(Secrets {
            access_key: row.get("aws_access_key_id").cloned(),
            secret_key: row.get("aws_secret_access_key").cloned(),
            session_token: row
                .get("aws_session_token")
                .or_else(|| row.get("aws_security_token"))
                .filter(|v| !v.is_empty())
                .cloned(),
            password: None,
        }),
        Authentication::Missing => bail!("{}", tr("이 AWS 프로필에 인증 정보가 없습니다")),
        Authentication::External => {
            let cli =
                cli.ok_or_else(|| anyhow::anyhow!(tr("이 AWS 프로필은 AWS CLI v2가 필요합니다")))?;
            let mut cmd = Command::new(cli);
            cmd.args([
                "configure",
                "export-credentials",
                "--profile",
                name,
                "--format",
                "process",
                "--no-cli-pager",
                "--no-cli-auto-prompt",
            ])
            .env("AWS_CONFIG_FILE", &files.config)
            .env("AWS_SHARED_CREDENTIALS_FILE", &files.credentials)
            .env("AWS_CLI_AUTO_PROMPT", "off")
            .env("AWS_PAGER", "")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
            // Explicit --profile suppresses the CLI's top-level environment
            // provider, while preserving keys required by credential_source=Environment.
            for key in ["AWS_PROFILE", "AWS_DEFAULT_PROFILE"] {
                cmd.env_remove(key);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            let mut child = cmd
                .spawn()
                .map_err(|_| anyhow::anyhow!(tr("이 AWS 프로필은 AWS CLI v2가 필요합니다")))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!(tr("AWS 프로필 인증 응답이 올바르지 않습니다")))?;
            let mut process = AuthProcess { child, armed: true };
            let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let overflow_reader = overflow.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            std::thread::spawn(move || {
                use std::io::Read;
                let mut bytes = Vec::new();
                let result = stdout.take(64 * 1024 + 1).read_to_end(&mut bytes);
                if bytes.len() > 64 * 1024 {
                    overflow_reader.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let _ = tx.send(result.map(|_| bytes));
            });
            let start = Instant::now();
            let status = loop {
                if control.check().is_err()
                    || start.elapsed() > timeout
                    || overflow.load(std::sync::atomic::Ordering::Relaxed)
                {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(process.child.id() as i32), libc::SIGKILL);
                    }
                    let _ = process.child.kill();
                    let _ = process.child.wait();
                    bail!("{}", tr("AWS 프로필 인증을 중단했습니다"));
                }
                if let Some(status) = process.child.try_wait()? {
                    break status;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            if !status.success() {
                bail!(
                    "{}",
                    tr("AWS 프로필 인증에 실패했습니다. AWS CLI 로그인 상태를 확인하세요.")
                );
            }
            let bytes = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Ok(bytes)) if bytes.len() <= 64 * 1024 => bytes,
                _ => {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(process.child.id() as i32), libc::SIGKILL);
                    }
                    bail!("{}", tr("AWS 프로필 인증 응답이 올바르지 않습니다"));
                }
            };
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "PascalCase")]
            struct Export {
                version: u8,
                access_key_id: String,
                secret_access_key: String,
                session_token: Option<String>,
                expiration: Option<String>,
            }
            let value: Export = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!(tr("AWS 프로필 인증 응답이 올바르지 않습니다")))?;
            if value.version != 1
                || value.access_key_id.is_empty()
                || value.secret_access_key.is_empty()
            {
                bail!("{}", tr("AWS 프로필 인증 응답이 올바르지 않습니다"));
            }
            if let Some(expiration) = &value.expiration {
                let expiration = chrono::DateTime::parse_from_rfc3339(expiration)
                    .map_err(|_| anyhow::anyhow!(tr("AWS 프로필 인증 응답이 올바르지 않습니다")))?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as i64;
                if expiration.timestamp() <= now {
                    bail!(
                        "{}",
                        tr("AWS 프로필 인증에 실패했습니다. AWS CLI 로그인 상태를 확인하세요.")
                    );
                }
            }
            process.armed = false;
            Ok(Secrets {
                access_key: Some(value.access_key_id),
                secret_key: Some(value.secret_access_key),
                session_token: value.session_token,
                password: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn files(dir: &Path) -> AwsFiles {
        AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
        }
    }
    #[test]
    fn merges_named_profiles_and_ignores_non_profile_sections_without_secret_metadata() {
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(&f.config, "[default]\nregion=ap-northeast-2\n[profile dev]\nregion=us-west-2\n[profile role]\nrole_arn=fixture-role\nsource_profile=dev\n[profile sso]\nsso_session=fixture\n[sso-session fixture]\nsso_start_url=https://example.invalid\n[services custom]\nservice=value\n").unwrap();
        std::fs::write(&f.credentials,"[dev]\naws_access_key_id=fixture-access\naws_secret_access_key=fixture-secret\naws_session_token=fixture-token\n").unwrap();
        let profiles = discover(&f).unwrap();
        assert_eq!(profiles.len(), 4);
        let dev = profiles.iter().find(|p| p.name == "dev").unwrap();
        assert_eq!(dev.authentication, Authentication::Static);
        assert_eq!(dev.region.as_deref(), Some("us-west-2"));
        assert!(!format!("{profiles:?}").contains("fixture-secret"));
        let secrets =
            resolve_with(&f, "dev", None, &Control::default(), Duration::from_secs(1)).unwrap();
        assert_eq!(secrets.session_token.as_deref(), Some("fixture-token"));
        assert!(
            resolve_with(
                &f,
                "missing",
                None,
                &Control::default(),
                Duration::from_secs(1)
            )
            .is_err()
        );
        assert!(
            resolve_with(
                &f,
                "role",
                None,
                &Control::default(),
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
    #[test]
    fn default_auth_obeys_explicit_profile_and_environment_pair_without_fallback() {
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(&f.credentials,"[default]\naws_access_key_id=default-access\naws_secret_access_key=default-secret\n[production]\naws_access_key_id=production-access\naws_secret_access_key=production-secret\n").unwrap();
        let env = |profile: Option<&str>, access: Option<&str>, secret: Option<&str>| {
            DefaultEnvironment {
                profile: profile.map(String::from),
                access: access.map(String::from),
                secret: secret.map(String::from),
                token: None,
            }
        };
        assert_eq!(
            resolve_default_with(
                &f,
                env(Some("production"), None, None),
                None,
                &Control::default()
            )
            .unwrap()
            .unwrap()
            .access_key
            .as_deref(),
            Some("production-access")
        );
        assert!(
            resolve_default_with(&f, env(Some(""), None, None), None, &Control::default()).is_err()
        );
        assert!(
            resolve_default_with(
                &f,
                env(Some("deleted"), None, None),
                None,
                &Control::default()
            )
            .is_err()
        );
        assert_eq!(
            resolve_default_with(
                &f,
                env(
                    Some("deleted"),
                    Some("environment-access"),
                    Some("environment-secret")
                ),
                None,
                &Control::default()
            )
            .unwrap()
            .unwrap()
            .access_key
            .as_deref(),
            Some("environment-access")
        );
        assert!(
            resolve_default_with(
                &f,
                env(Some("production"), Some("half-access"), None),
                None,
                &Control::default()
            )
            .is_err()
        );
    }
    #[test]
    fn malformed_profile_error_does_not_expose_its_contents() {
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(&f.credentials, "[fixture-secret-invalid\n").unwrap();
        assert!(
            !discover(&f)
                .unwrap_err()
                .to_string()
                .contains("fixture-secret")
        );
    }
    #[cfg(unix)]
    #[test]
    fn external_profile_env_source_fixture() {
        let Some(path) = std::env::var_os("KILN_AWS_ROLE_FIXTURE_DIR") else {
            return;
        };
        let dir = PathBuf::from(path);
        let f = files(&dir);
        let secrets = resolve_with(
            &f,
            "role",
            Some(&dir.join("aws")),
            &Control::default(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(secrets.access_key.as_deref(), Some("role-access"));
    }
    #[cfg(unix)]
    #[test]
    fn role_environment_source_is_preserved_without_global_environment_mutation() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(
            &f.config,
            "[profile role]\nrole_arn=fixture-role\ncredential_source=Environment\n",
        )
        .unwrap();
        let cli = d.path().join("aws");
        std::fs::write(
            &cli,
            r#"#!/bin/sh
[ "$4" = role ] || exit 9
[ "$AWS_ACCESS_KEY_ID" = fixture-source-access ] || exit 8
[ "$AWS_SECRET_ACCESS_KEY" = fixture-source-secret ] || exit 7
printf '%s' '{"Version":1,"AccessKeyId":"role-access","SecretAccessKey":"role-secret"}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "aws_profiles::tests::external_profile_env_source_fixture",
                "--test-threads=1",
            ])
            .env("KILN_AWS_ROLE_FIXTURE_DIR", d.path())
            .env("AWS_ACCESS_KEY_ID", "fixture-source-access")
            .env("AWS_SECRET_ACCESS_KEY", "fixture-source-secret")
            .env("AWS_PROFILE", "wrong-ambient-profile")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(result.success());
    }
    #[cfg(unix)]
    #[test]
    fn external_auth_output_is_bounded_and_all_failure_children_are_cleaned() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(&f.config, "[profile sso]\nsso_session=fixture\n").unwrap();
        let cli = d.path().join("aws");
        let run = |script: &str, control: &Control| {
            std::fs::write(&cli, script).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
            resolve_with(&f, "sso", Some(&cli), control, Duration::from_secs(2))
        };
        let start = Instant::now();
        assert!(
            run(
                "#!/bin/sh\nawk 'BEGIN { for (i=0;i<100000;i++) printf \"x\" }'\nsleep 30\n",
                &Control::default()
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let marker = d.path().join("leaked-child");
        let script = format!(
            "#!/bin/sh\n(sleep 1; touch '{}') &\nexit 9\n",
            marker.display()
        );
        assert!(run(&script, &Control::default()).is_err());
        std::thread::sleep(Duration::from_millis(1200));
        assert!(!marker.exists());
        let expired = r#"#!/bin/sh
printf '%s' '{"Version":1,"AccessKeyId":"fixture-access","SecretAccessKey":"fixture-secret","Expiration":"2000-01-01T00:00:00Z"}'
"#;
        assert!(run(expired, &Control::default()).is_err());
        let malformed = "#!/bin/sh\nprintf '%s' 'fixture-private-malformed'\n";
        let error = run(malformed, &Control::default()).unwrap_err().to_string();
        assert!(!error.contains("fixture-private"));
        let control = std::sync::Arc::new(Control::default());
        let cancel = control.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            cancel
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let start = Instant::now();
        assert!(run("#!/bin/sh\nsleep 30\n", &control).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[cfg(unix)]
    #[test]
    fn external_profile_resolves_only_through_explicit_cli_and_cancels_its_process_group() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = files(d.path());
        std::fs::write(&f.config, "[profile sso]\nsso_session=fixture\n").unwrap();
        let cli = d.path().join("aws");
        std::fs::write(&cli,"#!/bin/sh\n[ \"$4\" = sso ] || exit 9\nprintf '%s' '{\"Version\":1,\"AccessKeyId\":\"fixture-access\",\"SecretAccessKey\":\"fixture-secret\",\"SessionToken\":\"fixture-token\"}'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let secrets = resolve_with(
            &f,
            "sso",
            Some(&cli),
            &Control::default(),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(secrets.access_key.as_deref(), Some("fixture-access"));
        std::fs::write(&cli, "#!/bin/sh\nsleep 30\n").unwrap();
        let start = Instant::now();
        assert!(
            resolve_with(
                &f,
                "sso",
                Some(&cli),
                &Control::default(),
                Duration::from_millis(30)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
