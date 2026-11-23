//! Install Chrome through Codex from the marketplace already provisioned by OpenAI.
use super::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub(super) const PLUGIN: &str = "chrome@openai-bundled";
const MARKET: &str = ".tmp/bundled-marketplaces/openai-bundled";
const MARKETPLACE_KEY: &str = "marketplaces.openai-bundled.source";
const ENV_KEY: &str = "KILN_CODEX_CHROME_MARKETPLACE";

pub(crate) fn prepare(
    home: &Path,
    source: &Path,
    profile: &Path,
    report: &mut ExtensionReport,
) -> Option<(String, String)> {
    match prepare_inner(home, source, profile) {
        Ok(Some(path)) => {
            report.push("plugin", PLUGIN, "prepared", "");
            path.into_os_string()
                .into_string()
                .ok()
                .map(|path| (ENV_KEY.into(), path))
        }
        Ok(None) => None,
        Err(reason) => {
            report.push("plugin", PLUGIN, "failed", reason);
            None
        }
    }
}

/// Keep the isolated account and the desktop-owned native host. Codex accepts its
/// reserved marketplace only beneath its active CODEX_HOME.
pub fn launch_override(environment: &[(String, String)]) -> Option<String> {
    let (_, path) = environment.iter().find(|(key, _)| key == ENV_KEY)?;
    let (_, home) = environment.iter().find(|(key, _)| key == "CODEX_HOME")?;
    if Path::new(home).join(MARKET) != Path::new(path) || !Path::new(path).is_absolute() {
        return None;
    }
    Some(format!(
        "{MARKETPLACE_KEY}={}",
        toml::Value::String(path.clone())
    ))
}

fn read_config(root: &Dir) -> R<toml::Value> {
    let (bytes, _) = root.read("config.toml", METADATA_BYTES)?;
    toml::from_str(std::str::from_utf8(&bytes).map_err(|_| "INVALID_METADATA")?)
        .map_err(|_| "INVALID_METADATA")
}

fn prepare_inner(home: &Path, source: &Path, profile: &Path) -> R<Option<PathBuf>> {
    if source == profile || !source.join(MARKET).is_dir() {
        return Ok(None);
    }
    let root = Dir::open(profile)?;
    if root.kind("config.toml")?.is_none() {
        return Ok(None);
    }
    let state = root.dir(STATE, false)?;
    let _lock = state.lock("lock")?;
    let config = read_config(&root)?;
    let source_market = source.join(MARKET);
    let profile_market = profile.join(MARKET);
    if config
        .get("plugins")
        .and_then(|plugins| plugins.get(PLUGIN))
        .and_then(|setting| setting.get("enabled"))
        .and_then(toml::Value::as_bool)
        != Some(true)
    {
        return Ok(None);
    }
    // Respect an independently configured marketplace in the profile.
    let Some(market) = config
        .get("marketplaces")
        .and_then(|markets| markets.get("openai-bundled"))
    else {
        return Ok(None);
    };
    if market.get("source_type").and_then(toml::Value::as_str) != Some("local")
        || market
            .get("source")
            .and_then(toml::Value::as_str)
            .map(Path::new)
            != Some(&source_market)
    {
        return Ok(None);
    }

    let source_dir = Dir::open(&source_market)?;
    let manifest = scan::read_json(&source_dir, "plugins/chrome/.codex-plugin/plugin.json")?;
    if manifest["name"] != "chrome" || manifest["author"]["name"] != "OpenAI" {
        return Err("INVALID_OPENAI_CHROME_PLUGIN");
    }
    let version = manifest["version"]
        .as_str()
        .ok_or("INVALID_OPENAI_CHROME_PLUGIN")?;
    fs::component(version)?;

    let (parent, name) = root.parent(MARKET, true)?;
    // Never replace a marketplace installed or changed independently.
    match parent.kind(&name)? {
        None => {
            parent.revalidate()?;
            if let Err(error) = link_directory(&source_market, &profile_market)
                && error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err("CHROME_MARKETPLACE_LINK_FAILED");
            }
        }
        Some(Kind::Link) => {}
        Some(_) => return Ok(None),
    }
    if std::fs::read_link(&profile_market).ok().as_deref() != Some(source_market.as_path()) {
        return Ok(None);
    }

    let installed =
        format!("plugins/cache/openai-bundled/chrome/{version}/skills/control-chrome/SKILL.md");
    let installed = profile.join(installed);
    if !installed.is_file() {
        let binary = crate::login::find_cli(home, Tool::Codex).ok_or("CODEX_CLI_MISSING")?;
        let path = profile_market.to_str().ok_or("INVALID_ASSET_PATH")?;
        let override_arg = format!("{MARKETPLACE_KEY}={}", toml::Value::String(path.into()));
        let mut child = Command::new(binary)
            .args(["plugin", "add", PLUGIN, "-c", &override_arg])
            .env("CODEX_HOME", profile)
            .current_dir(profile)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "CHROME_PLUGIN_INSTALL_FAILED")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return Err("CHROME_PLUGIN_INSTALL_FAILED"),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("CHROME_PLUGIN_INSTALL_FAILED");
                }
            }
        }
    }
    if !installed.is_file() {
        return Err("CHROME_PLUGIN_INSTALL_FAILED");
    }
    Ok(Some(profile_market))
}

#[cfg(unix)]
fn link_directory(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(windows)]
fn link_directory(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(from, to)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn chrome_preparation_preserves_the_profile_activation_and_existing_marketplace() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let source = home.join("default");
        let profile = home.join("profile");
        let marketplace = source.join(MARKET);
        let manifest = marketplace.join("plugins/chrome/.codex-plugin");
        std::fs::create_dir_all(&manifest).unwrap();
        std::fs::write(
            manifest.join("plugin.json"),
            r#"{"name":"chrome","author":{"name":"OpenAI"},"version":"1.0.0"}"#,
        )
        .unwrap();
        let skills =
            profile.join("plugins/cache/openai-bundled/chrome/1.0.0/skills/control-chrome");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "fixture").unwrap();
        let state = Dir::open(&profile).unwrap().dir(STATE, true).unwrap();
        state.write_new("lock", b"", false).unwrap();
        let configure = |enabled| {
            std::fs::write(profile.join("config.toml"), format!(
                "[plugins.{PLUGIN:?}]\nenabled = {enabled}\n[marketplaces.openai-bundled]\nsource_type = \"local\"\nsource = {}\n",
                toml::Value::String(marketplace.to_str().unwrap().into()),
            )).unwrap();
        };

        configure(false);
        assert_eq!(prepare_inner(&home, &source, &profile), Ok(None));
        assert!(!profile.join(MARKET).exists());

        configure(true);
        let prepared = profile.join(MARKET);
        assert_eq!(
            prepare_inner(&home, &source, &profile),
            Ok(Some(prepared.clone()))
        );
        assert_eq!(std::fs::read_link(&prepared).unwrap(), marketplace);
        assert_eq!(
            prepare_inner(&home, &source, &profile),
            Ok(Some(prepared.clone()))
        );
        let environment = [
            ("CODEX_HOME".into(), profile.to_str().unwrap().into()),
            (ENV_KEY.into(), prepared.to_str().unwrap().into()),
        ];
        assert!(
            launch_override(&environment)
                .unwrap()
                .starts_with(MARKETPLACE_KEY)
        );

        std::fs::remove_file(&prepared).unwrap();
        std::fs::create_dir(&prepared).unwrap();
        assert_eq!(prepare_inner(&home, &source, &profile), Ok(None));
        assert!(!std::fs::symlink_metadata(&prepared).unwrap().is_symlink());
    }
}
