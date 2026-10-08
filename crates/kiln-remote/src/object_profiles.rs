//! Read only profile metadata. Discovery does not run a CLI or open token/key files.
use crate::{ObjectAuthentication, ObjectProvider, safe_text};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct ProfileFiles {
    /// OCI INI file, gcloud config directory, or Wrangler global directory.
    pub config: PathBuf,
    /// Optional ADC file: only its existence is inspected.
    pub credentials: Option<PathBuf>,
    /// Installed CLI. Tests can supply a private fixture executable.
    pub executable: Option<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectProfile {
    pub name: String,
    pub region: Option<String>,
    pub project: Option<String>,
    pub account_id: Option<String>,
    pub config_path: Option<PathBuf>,
    pub available: bool,
    pub reason: Option<String>,
    pub authentication: ObjectAuthentication,
}
// Preserve the selected metadata source when provider commands use an isolated cwd.
fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}
fn oracle_path(path: &Path, home: &Path, cwd: &Path) -> PathBuf {
    let path = if path == Path::new("~") {
        home.to_owned()
    } else if let Ok(rest) = path.strip_prefix("~") {
        home.join(rest)
    } else {
        path.to_owned()
    };
    absolute(&path, cwd)
}
// Mirrors oci.config._get_config_path_with_fallback. An explicit non-default
// source never silently falls back to another account, even when it is missing.
fn resolve_oracle(home: &Path, cwd: &Path, cli: Option<&Path>, sdk: Option<&Path>) -> PathBuf {
    let literal_default = Path::new("~/.oci/config");
    let selected = cli.unwrap_or(literal_default);
    let path = oracle_path(selected, home, cwd);
    if selected != literal_default || path.is_file() {
        return path;
    }
    if let Some(sdk) = sdk.filter(|p| !p.as_os_str().is_empty()) {
        return oracle_path(sdk, home, cwd);
    }
    let legacy = absolute(&home.join(".oraclebmc/config"), cwd);
    if legacy.is_file() { legacy } else { path }
}
pub(crate) fn oracle_config_path(selected: Option<&Path>) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let cli = std::env::var_os("OCI_CLI_CONFIG_FILE").map(PathBuf::from);
    let sdk = std::env::var_os("OCI_CONFIG_FILE").map(PathBuf::from);
    resolve_oracle(&home, &cwd, selected.or(cli.as_deref()), sdk.as_deref())
}
impl ProfileFiles {
    pub fn current(provider: ObjectProvider) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let cwd = std::env::current_dir().unwrap_or_default();
        let config = match provider {
            ObjectProvider::Oracle => oracle_config_path(None),
            ObjectProvider::Google => absolute(
                &std::env::var_os("CLOUDSDK_CONFIG")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config/gcloud")),
                &cwd,
            ),
            ObjectProvider::Cloudflare => wrangler_root(
                &home,
                &cwd,
                std::env::var_os("XDG_CONFIG_HOME")
                    .as_deref()
                    .map(Path::new),
            ),
        };
        let credentials = (provider == ObjectProvider::Google).then(|| {
            absolute(
                &std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| config.join("application_default_credentials.json")),
                &cwd,
            )
        });
        Self {
            config,
            credentials,
            executable: crate::cloud_cli::find(provider.cli()),
        }
    }
}
fn wrangler_root(home: &Path, cwd: &Path, xdg: Option<&Path>) -> PathBuf {
    let legacy = absolute(&home.join(".wrangler"), cwd);
    if legacy.is_dir() {
        return legacy;
    }
    let base = xdg
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_owned)
        .unwrap_or_else(|| {
            #[cfg(target_os = "macos")]
            {
                home.join("Library/Preferences")
            }
            #[cfg(not(target_os = "macos"))]
            {
                home.join(".config")
            }
        });
    absolute(&base.join(".wrangler"), cwd)
}
pub fn validate_profile_name(name: &str) -> Result<()> {
    safe_text(name)?;
    if name.is_empty()
        || name.len() > 128
        || name.starts_with('-')
        || name == "."
        || name == ".."
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("Invalid CLI profile name");
    }
    Ok(())
}
pub fn validate_provider_profile(provider: ObjectProvider, name: &str) -> Result<()> {
    if provider != ObjectProvider::Oracle {
        return validate_profile_name(name);
    }
    safe_text(name)?;
    // OCI uses a ConfigParser section, not a credential filename. Spaces and
    // non-ASCII names are valid and remain one literal --profile= argument.
    if name.trim().is_empty() || name.len() > 4096 {
        bail!("Invalid OCI profile name");
    }
    Ok(())
}
fn metadata_ini(path: &Path) -> Result<ini::Ini> {
    let n = std::fs::metadata(path)?.len();
    if n > 1024 * 1024 {
        bail!("CLI configuration is too large");
    }
    // Never return parser errors: they can contain a credential-bearing line.
    let text = std::fs::read_to_string(path).context("Cannot read CLI profile metadata")?;
    ini::Ini::load_from_str(&text).map_err(|_| anyhow::anyhow!("Invalid CLI profile configuration"))
}
fn property<'a>(section: &'a ini::Properties, name: &str) -> Option<&'a str> {
    section
        .iter()
        .find_map(|(k, v)| k.eq_ignore_ascii_case(name).then_some(v))
}
/// OCI session profiles require an explicit auth mode. Inspect the field's
/// presence, never the token file itself, and retain DEFAULT inheritance.
pub(crate) fn oracle_auth_mode(name: &str, path: Option<&Path>) -> Result<&'static str> {
    let resolved = oracle_config_path(path);
    let path = resolved.as_path();
    if !path.is_file() {
        return Ok("api_key");
    }
    let ini = metadata_ini(path)?;
    let token = ini
        .section(Some(name))
        .and_then(|s| property(s, "security_token_file"))
        .or_else(|| {
            ini.section(Some("DEFAULT"))
                .and_then(|s| property(s, "security_token_file"))
        });
    Ok(if token.is_some_and(|v| !v.trim().is_empty()) {
        "security_token"
    } else {
        "api_key"
    })
}
pub fn discover(provider: ObjectProvider, files: &ProfileFiles) -> Result<Vec<ObjectProfile>> {
    let ready = files.executable.as_ref().is_some_and(|p| p.is_file());
    let mut rows = BTreeMap::new();
    let make = |name: String, region: Option<String>, project: Option<String>| ObjectProfile {
        authentication: ObjectAuthentication::Cli {
            profile: name.clone(),
            config_path: Some(files.config.clone()),
        },
        name,
        region,
        project,
        account_id: None,
        config_path: Some(files.config.clone()),
        available: ready,
        reason: (!ready).then(|| format!("Install {} to use this profile", provider.cli())),
    };
    match provider {
        ObjectProvider::Oracle if files.config.is_file() => {
            let ini = metadata_ini(&files.config)?;
            let default = ini
                .section(Some("DEFAULT"))
                .and_then(|s| property(s, "region"))
                .map(str::to_owned);
            for (name, section) in &ini {
                let Some(name) = name else {
                    continue;
                };
                if validate_provider_profile(provider, name).is_err() {
                    continue;
                }
                let region = section
                    .iter()
                    .find_map(|(k, v)| k.eq_ignore_ascii_case("region").then_some(v))
                    .map(str::to_owned)
                    .or_else(|| default.clone());
                rows.insert(name.to_owned(), make(name.to_owned(), region, None));
            }
        }
        ObjectProvider::Google => {
            let dir = files.config.join("configurations");
            if dir.is_dir() {
                for entry in std::fs::read_dir(&dir)? {
                    let entry = entry?;
                    if !entry.path().is_file() {
                        continue;
                    }
                    let filename = entry.file_name();
                    let Some(name) = filename.to_str().and_then(|n| n.strip_prefix("config_"))
                    else {
                        continue;
                    };
                    if validate_profile_name(name).is_err() {
                        continue;
                    }
                    let ini = metadata_ini(&entry.path())?;
                    let project = ini
                        .section(Some("core"))
                        .and_then(|s| s.get("project"))
                        .map(str::to_owned);
                    rows.insert(name.to_owned(), make(name.to_owned(), None, project));
                }
            }
            if let Some(path) = &files.credentials {
                if path.is_file() {
                    let name = "Application Default Credentials".to_string();
                    let mut row = make(name.clone(), None, None);
                    row.authentication = ObjectAuthentication::GoogleAdc {
                        credentials_path: Some(path.clone()),
                    };
                    rows.insert(name, row);
                }
            }
        }
        ObjectProvider::Cloudflare => {
            let dir = files.config.join("config");
            if dir.is_dir() {
                for entry in std::fs::read_dir(dir)? {
                    let entry = entry?;
                    if !entry.path().is_file() {
                        continue;
                    }
                    let filename = entry.file_name();
                    let Some(filename) = filename.to_str() else {
                        continue;
                    };
                    let Some(name) = filename
                        .strip_suffix(".toml")
                        .or_else(|| filename.strip_suffix(".enc"))
                    else {
                        continue;
                    };
                    if validate_profile_name(name).is_ok() {
                        rows.insert(name.to_owned(), make(name.to_owned(), None, None));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(rows.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oracle_path_precedence_and_relative_sources_are_exact() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let cwd = d.path().join("cwd");
        std::fs::create_dir_all(home.join(".oraclebmc")).unwrap();
        std::fs::create_dir_all(home.join(".oci")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let legacy = home.join(".oraclebmc/config");
        std::fs::write(&legacy, "[LEGACY]\nregion=legacy-region\n").unwrap();
        assert_eq!(resolve_oracle(&home, &cwd, None, None), legacy);
        let relative = Path::new("profiles/oci");
        assert_eq!(
            resolve_oracle(&home, &cwd, None, Some(relative)),
            cwd.join(relative)
        );
        assert_eq!(
            resolve_oracle(&home, &cwd, Some(Path::new("~/custom")), Some(relative)),
            home.join("custom")
        );
        assert_eq!(
            resolve_oracle(&home, &cwd, Some(Path::new("missing")), None),
            cwd.join("missing")
        );
        let default = home.join(".oci/config");
        std::fs::write(&default, "[PRIMARY]\nregion=primary-region\n").unwrap();
        assert_eq!(resolve_oracle(&home, &cwd, None, Some(relative)), default);
        assert_eq!(
            resolve_oracle(
                &home,
                &cwd,
                Some(Path::new("~/.oci/config")),
                Some(relative)
            ),
            default
        );
        std::fs::remove_file(&default).unwrap();
        let files = ProfileFiles {
            config: resolve_oracle(&home, &cwd, None, Some(Path::new(""))),
            credentials: None,
            executable: None,
        };
        assert_eq!(
            discover(ObjectProvider::Oracle, &files).unwrap()[0].name,
            "LEGACY"
        );
        assert_eq!(
            oracle_path(Path::new("~literal/config"), &home, &cwd),
            cwd.join("~literal/config")
        );
    }
    #[test]
    fn wrangler_relative_xdg_and_legacy_sources_preserve_identity() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let cwd = d.path().join("cwd");
        assert_eq!(
            wrangler_root(&home, &cwd, Some(Path::new("relative"))),
            cwd.join("relative/.wrangler")
        );
        assert_eq!(
            wrangler_root(&home, &cwd, Some(Path::new("~/literal"))),
            cwd.join("~/literal/.wrangler")
        );
        assert_eq!(
            wrangler_root(&home, &cwd, Some(Path::new(""))),
            wrangler_root(&home, &cwd, None)
        );
        std::fs::create_dir_all(home.join(".wrangler")).unwrap();
        assert_eq!(
            wrangler_root(&home, &cwd, Some(Path::new("relative"))),
            home.join(".wrangler")
        );
    }
    #[test]
    fn isolated_native_profiles_do_not_expose_credentials_or_read_adc_and_wrangler_contents() {
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join("cli");
        std::fs::write(&cli, "").unwrap();
        let p = d.path().join("oci");
        std::fs::write(&p,"[DEFAULT]\nregion=us-ashburn-1\nkey_file=/private/key\n[TEAM]\nuser=secret-user\nfingerprint=private\n").unwrap();
        let mut files = ProfileFiles {
            config: p,
            credentials: None,
            executable: Some(cli),
        };
        let rows = discover(ObjectProvider::Oracle, &files).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].region.as_deref(), Some("us-ashburn-1"));
        assert!(!format!("{rows:?}").contains("secret-user"));
        files.config = d.path().join("gcloud");
        std::fs::create_dir_all(files.config.join("configurations")).unwrap();
        std::fs::write(
            files.config.join("configurations/config_team"),
            "[core]\nproject=fixture-project\naccount=private-email\n",
        )
        .unwrap();
        let adc = d.path().join("adc");
        std::fs::write(&adc, [0xff, 0xfe]).unwrap();
        files.credentials = Some(adc);
        let rows = discover(ObjectProvider::Google, &files).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(!format!("{rows:?}").contains("private-email"));
        assert!(
            rows.iter()
                .any(|r| matches!(r.authentication, ObjectAuthentication::GoogleAdc { .. }))
        );
        files.config = d.path().join("wrangler");
        std::fs::create_dir_all(files.config.join("config")).unwrap();
        for n in ["default.toml", "team.enc", "team.toml"] {
            std::fs::write(files.config.join("config").join(n), [0xff]).unwrap();
        }
        let rows = discover(ObjectProvider::Cloudflare, &files).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.available));
        files.executable = None;
        assert!(
            discover(ObjectProvider::Cloudflare, &files)
                .unwrap()
                .iter()
                .all(|r| !r.available && r.reason.is_some())
        );
    }
    #[test]
    fn malformed_metadata_does_not_echo_sensitive_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(&p, "[broken-secret-line\n").unwrap();
        let files = ProfileFiles {
            config: p,
            credentials: None,
            executable: None,
        };
        let err = discover(ObjectProvider::Oracle, &files)
            .unwrap_err()
            .to_string();
        assert!(!err.contains("secret-line"));
        for n in ["../profile", "-p", "a\n--flag", ".."] {
            assert!(validate_profile_name(n).is_err());
        }
    }
    #[test]
    fn oracle_session_mode_uses_metadata_without_reading_token_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        let token = d.path().join("private-token");
        std::fs::write(&token, [0xff]).unwrap();
        std::fs::write(&p,format!("[DEFAULT]\nREGION=us-ashburn-1\n[SESSION]\nSECURITY_TOKEN_FILE={}\n[API]\nuser=fixture\n",token.display())).unwrap();
        assert_eq!(
            oracle_auth_mode("SESSION", Some(&p)).unwrap(),
            "security_token"
        );
        assert_eq!(oracle_auth_mode("API", Some(&p)).unwrap(), "api_key");
    }
    #[test]
    fn oracle_profile_names_preserve_spaces_and_unicode_as_sections() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config");
        std::fs::write(
            &p,
            "[팀 설정]\nregion=ap-seoul-1\n[My Profile]\nregion=us-ashburn-1\n",
        )
        .unwrap();
        let files = ProfileFiles {
            config: p,
            credentials: None,
            executable: None,
        };
        let rows = discover(ObjectProvider::Oracle, &files).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.name == "팀 설정"));
        assert!(rows.iter().any(|r| r.name == "My Profile"));
        assert!(validate_provider_profile(ObjectProvider::Oracle, "--literal alias").is_ok());
        assert!(validate_provider_profile(ObjectProvider::Google, "팀 설정").is_err());
        assert!(validate_provider_profile(ObjectProvider::Oracle, "bad\n--flag").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinked_cli_profiles_are_discovered_without_reading_wrangler_tokens() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let google = d.path().join("gcloud");
        std::fs::create_dir_all(google.join("configurations")).unwrap();
        let metadata = d.path().join("metadata");
        std::fs::write(
            &metadata,
            "[core]\nproject=symlink-project\naccount=private-email\n",
        )
        .unwrap();
        symlink(&metadata, google.join("configurations/config_team")).unwrap();
        symlink(
            d.path().join("missing"),
            google.join("configurations/config_broken"),
        )
        .unwrap();
        std::fs::create_dir(google.join("configurations/config_directory")).unwrap();
        let files = ProfileFiles {
            config: google,
            credentials: None,
            executable: None,
        };
        let rows = discover(ObjectProvider::Google, &files).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "team");
        assert_eq!(rows[0].project.as_deref(), Some("symlink-project"));
        assert!(!format!("{rows:?}").contains("private-email"));
        let wrangler = d.path().join("wrangler");
        std::fs::create_dir_all(wrangler.join("config")).unwrap();
        let private = d.path().join("private");
        std::fs::write(&private, [0xff]).unwrap();
        symlink(&private, wrangler.join("config/team.toml")).unwrap();
        symlink(d.path().join("missing"), wrangler.join("config/broken.enc")).unwrap();
        let files = ProfileFiles {
            config: wrangler,
            credentials: None,
            executable: None,
        };
        let rows = discover(ObjectProvider::Cloudflare, &files).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "team");
    }
}
