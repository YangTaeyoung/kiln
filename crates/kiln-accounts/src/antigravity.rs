//! Antigravity's official CLI owns OAuth and token refresh. On macOS each managed
//! account gets a private HOME/keychain; the user's default keychain is never changed.
use crate::{
    Env,
    store::{read_secret, write_secret_atomic},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const SERVICE: &str = "gemini";
pub const ACCOUNT: &str = "antigravity";
const PASSWORDS: &str = "kiln-antigravity-keychains";
pub fn data_dir(home: &Path) -> PathBuf {
    home.join(".gemini/antigravity-cli")
}
fn keychain(home: &Path) -> PathBuf {
    home.join("Library/Keychains/kiln.keychain-db")
}
fn namespace(home: &Path) -> String {
    format!("{:x}", Sha256::digest(home.to_string_lossy().as_bytes()))
}

pub fn decode(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let text = if let Some(encoded) = raw.strip_prefix("go-keyring-base64:") {
        String::from_utf8(
            STANDARD
                .decode(encoded)
                .context("invalid Antigravity credential encoding")?,
        )
        .context("invalid Antigravity credential encoding")?
    } else {
        raw.to_owned()
    };
    access_token(&text)?;
    Ok(text)
}
pub fn access_token(secret: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(secret)
        .map_err(|_| anyhow::anyhow!("invalid Antigravity credentials"))?;
    let token = value["token"]["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Antigravity sign-in required")?;
    crate::usage_http::header_value(token)?;
    ensure!(
        value["token"]["refresh_token"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "Antigravity sign-in required"
    );
    Ok(token.to_string())
}
/// Read-only Google OpenID Connect UserInfo, never a model request or token refresh.
pub fn identity(secret: &str) -> Result<String> {
    let token = access_token(secret)?;
    let value = crate::usage_http::get(
        "https://openidconnect.googleapis.com/v1/userinfo",
        &format!("Authorization: Bearer {token}\n"),
    )?;
    ensure!(
        value["email_verified"].as_bool() == Some(true),
        "Google account email is not verified"
    );
    Ok(value["email"]
        .as_str()
        .filter(|s| s.contains('@'))
        .context("Google account email unavailable")?
        .to_owned())
}

fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
/// Values contain only paths, never credentials. Both data overrides prevent inherited
/// environment variables from redirecting this account into another account's storage.
pub fn environment(home: &Path) -> Vec<(String, String)> {
    vec![
        ("HOME".into(), home.to_string_lossy().into()),
        ("USERPROFILE".into(), home.to_string_lossy().into()),
        (
            "ANTIGRAVITY_APP_DATA_DIR".into(),
            data_dir(home).to_string_lossy().into(),
        ),
        (
            "ANTIGRAVITY_GEMINI_DIR".into(),
            home.join(".gemini").to_string_lossy().into(),
        ),
        ("AGY_CLI_DISABLE_AUTO_UPDATE".into(), "true".into()),
        ("GEMINI_API_KEY".into(), String::new()),
        ("JETSKI_OAUTH_TOKEN".into(), String::new()),
        ("JETSKI_TEST_GAIA_TOKEN".into(), String::new()),
        ("AGY_ADC_AUTH".into(), String::new()),
        ("GOOGLE_APPLICATION_CREDENTIALS".into(), String::new()),
    ]
}

/// security(1) reads only this private HOME. Explicit paths are used for all mutations.
/// Credentials and unlock passwords go over stdin, never process arguments.
fn security(home: &Path, args: &[&str], input: Option<&str>) -> Result<Option<String>> {
    ensure!(
        cfg!(target_os = "macos"),
        "Antigravity account isolation currently requires macOS"
    );
    ensure!(
        !cfg!(test) || std::env::var("KILN_AGY_KEYCHAIN_TEST").as_deref() == Ok("1"),
        "real keychain disabled in tests"
    );
    ensure!(
        input.is_none_or(|text| text.len() < 4096),
        "Antigravity credential exceeds the keychain command limit"
    );
    let mut command = Command::new("/usr/bin/security");
    command
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home);
    command.stdin(Stdio::piped());
    // The shared runner bounds output and execution; stdin contains one short command.
    let output = crate::store::bounded_output_with_input(
        &mut command,
        std::time::Duration::from_secs(5),
        input.map(str::as_bytes),
    )?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ));
    }
    if output.status.code() == Some(44) && matches!(args.first(), Some(&"find-generic-password")) {
        return Ok(None);
    }
    bail!(
        "Antigravity private keychain operation failed ({})",
        output.status.code().unwrap_or(-1)
    )
}
fn quoted(text: &str) -> Result<String> {
    ensure!(
        !text.contains(['\n', '\r', '\0']),
        "invalid keychain argument"
    );
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}
pub fn prepare(env: &Env, home: &Path) -> Result<()> {
    private_dir(home)?;
    private_dir(&data_dir(home))?;
    if !env.antigravity_keychains {
        return Ok(());
    }
    private_dir(&home.join("Library/Keychains"))?;
    private_dir(&home.join("Library/Preferences"))?;
    let id = namespace(home);
    let path = keychain(home);
    let password = match env.store.get(PASSWORDS, &id)? {
        Some(password) => password,
        None => {
            ensure!(
                !path.exists(),
                "Antigravity keychain password missing; sign in again"
            );
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|_| anyhow::anyhow!("could not create private keychain password"))?;
            let password = STANDARD.encode(bytes);
            env.store.set(PASSWORDS, &id, &password)?;
            password
        }
    };
    let path_text = path.to_string_lossy();
    let quoted_path = quoted(&path_text)?;
    // Avoid the special login.keychain name: macOS can provision that separately
    // with another password. This keychain belongs only to this private HOME.
    if !path.exists() {
        security(
            home,
            &["-i"],
            Some(&format!(
                "create-keychain -p {} {quoted_path}\n",
                quoted(&password)?
            )),
        )?;
    }
    security(
        home,
        &["-i"],
        Some(&format!(
            "unlock-keychain -p {} {quoted_path}\n",
            quoted(&password)?
        )),
    )?;
    security(home, &["set-keychain-settings", &path_text], None)?;
    security(
        home,
        &["default-keychain", "-d", "user", "-s", &path_text],
        None,
    )?;
    security(
        home,
        &["list-keychains", "-d", "user", "-s", &path_text],
        None,
    )?;
    let selected = security(home, &["default-keychain"], None)?.unwrap_or_default();
    ensure!(
        Path::new(selected.trim_matches('"')).canonicalize()? == path.canonicalize()?,
        "Antigravity private keychain is not selected"
    );
    Ok(())
}
pub fn read(env: &Env, home: &Path) -> Result<Option<String>> {
    if !env.antigravity_keychains {
        return Ok(env
            .store
            .get(&format!("kiln-agy-runtime-{}", namespace(home)), ACCOUNT)?
            .or(read_secret(
                &data_dir(home).join("antigravity-oauth-token"),
            )?));
    }
    if !keychain(home).exists() {
        return Ok(None);
    }
    prepare(env, home)?;
    let fallback = data_dir(home).join("antigravity-oauth-token");
    let marker = data_dir(home).join("cache/antigravity-keyring-unavailable");
    if marker
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < 3600)
    {
        return read_secret(&fallback)?.map(|raw| decode(&raw)).transpose();
    }
    if let Some(raw) = security(
        home,
        &[
            "find-generic-password",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-w",
            &keychain(home).to_string_lossy(),
        ],
        None,
    )? {
        return Ok(Some(decode(&raw)?));
    }
    // The official CLI can fall back to its private file after a keyring failure.
    read_secret(&data_dir(home).join("antigravity-oauth-token"))?
        .map(|s| decode(&s))
        .transpose()
}
pub fn write(env: &Env, home: &Path, secret: Option<&str>) -> Result<()> {
    if let Some(secret) = secret {
        access_token(secret)?;
    }
    prepare(env, home)?;
    if !env.antigravity_keychains {
        let service = format!("kiln-agy-runtime-{}", namespace(home));
        if let Some(secret) = secret {
            env.store.set(&service, ACCOUNT, secret)?;
        } else {
            env.store.delete(&service, ACCOUNT)?;
        }
        return Ok(());
    }
    let path = quoted(&keychain(home).to_string_lossy())?;
    if let Some(secret) = secret {
        let encoded = format!("go-keyring-base64:{}", STANDARD.encode(secret));
        security(
            home,
            &["-i"],
            Some(&format!(
                "add-generic-password -U -s gemini -a antigravity -w {} {path}\n",
                quoted(&encoded)?
            )),
        )?;
    } else {
        // Missing items are harmless, but other failures remain visible.
        let exists = security(
            home,
            &[
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                ACCOUNT,
                &keychain(home).to_string_lossy(),
            ],
            None,
        )?
        .is_some();
        if exists {
            security(
                home,
                &["-i"],
                Some(&format!(
                    "delete-generic-password -s gemini -a antigravity {path}\n"
                )),
            )?;
        }
    }
    if let Some(secret) = secret {
        let actual = security(
            home,
            &[
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                ACCOUNT,
                "-w",
                &keychain(home).to_string_lossy(),
            ],
            None,
        )?
        .context("private credential was not saved")?;
        ensure!(
            decode(&actual)? == secret,
            "private credential verification failed"
        );
    }
    let fallback = data_dir(home).join("antigravity-oauth-token");
    if fallback.exists() {
        if let Some(secret) = secret {
            write_secret_atomic(&fallback, secret.as_bytes())?;
        } else {
            std::fs::remove_file(fallback)?;
        }
    }
    Ok(())
}
pub fn cleanup(env: &Env, home: &Path) -> Result<()> {
    if env.antigravity_keychains && keychain(home).exists() {
        security(
            home,
            &["delete-keychain", &keychain(home).to_string_lossy()],
            None,
        )?;
    }
    env.store.delete(PASSWORDS, &namespace(home))?;
    env.store
        .delete(&format!("kiln-agy-runtime-{}", namespace(home)), ACCOUNT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn fixture(token: &str) -> String {
        serde_json::json!({"token":{"access_token":token,"refresh_token":"fixture-refresh","expiry":"2099-01-01T00:00:00Z"},"auth_method":"oauth"}).to_string()
    }
    #[test]
    fn credentials_and_environment_are_isolated() {
        let root = tempfile::tempdir().unwrap();
        let (env, _) = Env::sandbox(root.path(), false);
        let a = root.path().join("a");
        let b = root.path().join("b");
        write(&env, &a, Some(&fixture("first"))).unwrap();
        write(&env, &b, Some(&fixture("second"))).unwrap();
        assert_eq!(
            access_token(&read(&env, &a).unwrap().unwrap()).unwrap(),
            "first"
        );
        assert_eq!(
            access_token(&read(&env, &b).unwrap().unwrap()).unwrap(),
            "second"
        );
        write(&env, &a, None).unwrap();
        assert!(read(&env, &a).unwrap().is_none());
        assert!(read(&env, &b).unwrap().is_some());
        assert!(decode("{\"token\":{\"access_token\":\"key\"}}").is_err());
        assert_eq!(
            decode(&format!(
                "go-keyring-base64:{}",
                STANDARD.encode(fixture("x"))
            ))
            .unwrap(),
            fixture("x")
        );
        for (k, v) in environment(&b) {
            assert!(!v.contains("second"));
            if k == "HOME" {
                assert_eq!(v, b.to_string_lossy());
            }
        }
    }
}

/// Enumerate only registered account homes, without opening any credentials/keychains.
pub fn saved_roots(settings: &Path) -> Vec<PathBuf> {
    let Some(parent) = settings.parent() else {
        return Vec::new();
    };
    let value = std::fs::read(settings)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    value
        .as_ref()
        .and_then(|v| v["antigravity"]["profiles"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str())
        .filter(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        })
        .map(|id| data_dir(&parent.join("agent-accounts/antigravity").join(id)))
        .collect()
}

#[cfg(all(test, target_os = "macos"))]
mod native_tests {
    use super::*;
    /// Opt-in: creates only temporary keychains and uses synthetic credentials.
    #[test]
    #[ignore = "requires KILN_AGY_KEYCHAIN_TEST=1; creates temporary private keychains"]
    fn native_keychains_do_not_change_default_or_cross_accounts() {
        let before = Command::new("/usr/bin/security")
            .arg("default-keychain")
            .output()
            .unwrap()
            .stdout;
        let search_before = Command::new("/usr/bin/security")
            .arg("list-keychains")
            .output()
            .unwrap()
            .stdout;
        let root = tempfile::tempdir().unwrap();
        let (mut env, _) = Env::sandbox(root.path(), false);
        env.antigravity_keychains = true;
        let a = root.path().join("a");
        let b = root.path().join("b");
        let first = super::tests::fixture("first");
        let second = super::tests::fixture("second");
        let result = (|| -> Result<()> {
            write(&env, &a, Some(&first))?;
            write(&env, &b, Some(&second))?;
            assert_eq!(read(&env, &a)?, Some(first));
            assert_eq!(read(&env, &b)?, Some(second.clone()));
            // This is the same implicit lookup used by agy's go-keyring dependency.
            assert_eq!(
                decode(
                    &security(
                        &a,
                        &["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"],
                        None
                    )?
                    .unwrap()
                )?,
                super::tests::fixture("first")
            );
            write(&env, &a, None)?;
            assert!(read(&env, &a)?.is_none());
            assert_eq!(read(&env, &b)?, Some(second));
            Ok(())
        })();
        let clean_a = cleanup(&env, &a);
        let clean_b = cleanup(&env, &b);
        assert_eq!(
            Command::new("/usr/bin/security")
                .arg("default-keychain")
                .output()
                .unwrap()
                .stdout,
            before
        );
        assert_eq!(
            Command::new("/usr/bin/security")
                .arg("list-keychains")
                .output()
                .unwrap()
                .stdout,
            search_before
        );
        clean_a.unwrap();
        clean_b.unwrap();
        result.unwrap();
    }
}
