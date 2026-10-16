//! In-app updates on Windows. A signed appcast lists one archive per architecture; the
//! archive is checked against the Sparkle Ed25519 key before it replaces `Kiln.exe` and
//! `bin\kiln.exe` of the installed layout.
#![cfg_attr(not(windows), allow(dead_code))]
use anyhow::{Context, Result, anyhow, ensure};
use base64::Engine;

const SPARKLE: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";
const KILN: &str = "https://github.com/YangTaeyoung/kiln/appcast";

/// One archive offered by the appcast.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Release {
    pub version: String,
    pub url: String,
    pub length: u64,
    pub signature: Vec<u8>,
}

/// The archive architecture this build installs: `x64` or `arm64`.
pub(super) fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

/// The newest release in `xml` for Windows on `arch`.
pub(super) fn parse_appcast(xml: &str, arch: &str) -> Result<Release> {
    let doc = roxmltree::Document::parse(xml).context("Invalid update feed")?;
    let item = doc.descendants().find(|n| n.has_tag_name("item")).context("The update feed lists no release")?;
    let version = item
        .children()
        .find(|n| n.has_tag_name((SPARKLE, "version")))
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .context("The update feed lists no version")?;
    let enclosure = item
        .children()
        .filter(|n| n.has_tag_name("enclosure"))
        .find(|n| n.attribute((SPARKLE, "os")) == Some("windows") && n.attribute((KILN, "arch")) == Some(arch))
        .with_context(|| format!("The update feed has no Windows {arch} archive"))?;
    let attribute = |name: &str| enclosure.attribute(name).with_context(|| format!("The Windows {arch} archive has no {name}"));
    let signature = enclosure.attribute((SPARKLE, "edSignature")).with_context(|| format!("The Windows {arch} archive has no signature"))?;
    Ok(Release {
        version: version.to_owned(),
        url: attribute("url")?.to_owned(),
        length: attribute("length")?.parse().context("Invalid archive length")?,
        signature: base64::engine::general_purpose::STANDARD.decode(signature).context("Invalid archive signature")?,
    })
}

/// Whether `candidate` is a later version than `current` (numeric components compared).
pub(super) fn newer(candidate: &str, current: &str) -> bool {
    let numbers = |v: &str| v.split(['.', '-', '+']).map_while(|part| part.parse::<u64>().ok()).collect::<Vec<_>>();
    numbers(candidate) > numbers(current)
}

/// Checks the archive's length, then its Ed25519 signature with `key`.
pub(super) fn verify(archive: &[u8], release: &Release, key: &[u8; 32]) -> Result<()> {
    ensure!(archive.len() as u64 == release.length, "The downloaded archive's length does not match the update feed");
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(archive, &release.signature)
        .map_err(|_| anyhow!("The downloaded archive's signature does not match Kiln's update key"))
}

#[cfg(windows)]
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
fn system_tool(name: &str) -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join(name)
}

/// Replaces the installed files in `dir` with those in `archive` after `check` reports the
/// staged `bin\kiln.exe` as `version`. Each current file is renamed to `<name>.old` first,
/// which Windows allows for running executables; a failed move restores the previous files.
#[cfg(windows)]
pub(super) fn install(archive: &Path, dir: &Path, version: &str, check: impl Fn(&Path) -> Result<String>) -> Result<()> {
    let staging = dir.join(format!(".update-{version}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = (|| {
        let mut tar = Command::new(system_tool("tar.exe"));
        kiln_common::process::hide_window(&mut tar).arg("-xf").arg(archive).arg("-C").arg(&staging);
        let out = tar.output().context("The update archive could not be extracted")?;
        ensure!(out.status.success(), "The update archive could not be extracted: {}", String::from_utf8_lossy(&out.stderr).trim());
        for required in [Path::new("Kiln.exe").to_path_buf(), Path::new("bin").join("kiln.exe")] {
            ensure!(staging.join(&required).is_file(), "The update archive lacks {}", required.display());
        }
        let reported = check(&staging.join("bin").join("kiln.exe"))?;
        ensure!(reported.split_whitespace().any(|word| word == version), "The update archive holds {}, not Kiln {version}", reported.trim());
        let mut files = Vec::new();
        staged_files(&staging, Path::new(""), &mut files)?;
        replace(dir, &staging, &files)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(windows)]
fn staged_files(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            staged_files(root, &path, out)?;
        } else if kind.is_file() {
            out.push(path);
        } else {
            anyhow::bail!("The update archive holds a link: {}", path.display());
        }
    }
    Ok(())
}

/// A free name for the current copy of `target`: `<name>.old`, or `<name>.old-<n>` while an
/// older copy is still in use.
#[cfg(windows)]
fn old_name(target: &Path) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
    (0..100)
        .map(|n| target.with_file_name(if n == 0 { format!("{name}.old") } else { format!("{name}.old-{n}") }))
        .find(|old| !old.exists() || std::fs::remove_file(old).is_ok())
        .unwrap_or_else(|| target.with_file_name(format!("{name}.old-{}", std::process::id())))
}

#[cfg(windows)]
fn replace(dir: &Path, staging: &Path, files: &[PathBuf]) -> Result<()> {
    let mut moved: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = files.iter().try_for_each(|file| {
        let target = dir.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let previous = if target.exists() {
            let old = old_name(&target);
            std::fs::rename(&target, &old).with_context(|| format!("{} could not be set aside", target.display()))?;
            Some(old)
        } else {
            None
        };
        let placed = std::fs::rename(staging.join(file), &target).with_context(|| format!("{} could not be replaced", target.display()));
        moved.push((target, previous));
        placed
    });
    if result.is_err() {
        for (target, previous) in moved.into_iter().rev() {
            if let Some(previous) = previous {
                let _ = std::fs::remove_file(&target);
                let _ = std::fs::rename(&previous, &target);
            }
        }
    }
    result
}

/// Deletes the `.old` copies a previous update left behind; copies still in use stay.
#[cfg(windows)]
pub(super) fn remove_old_copies(dir: &Path) {
    for folder in [dir.to_path_buf(), dir.join("bin")] {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".old") || name.contains(".old-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

const FEED: &str = "https://github.com/YangTaeyoung/kiln/releases/latest/download/appcast-windows.xml";

/// The update feed; `updater-test` builds read `KILN_UPDATE_FEED` first.
#[cfg(windows)]
fn feed_url() -> String {
    #[cfg(feature = "updater-test")]
    if let Ok(url) = std::env::var("KILN_UPDATE_FEED") {
        return url;
    }
    FEED.into()
}

/// Kiln's Sparkle Ed25519 public key; `updater-test` builds read
/// `KILN_UPDATE_PUBLIC_KEY_TEST` first.
#[cfg(windows)]
fn public_key() -> Result<[u8; 32]> {
    #[cfg(feature = "updater-test")]
    let configured = std::env::var("KILN_UPDATE_PUBLIC_KEY_TEST").ok();
    #[cfg(not(feature = "updater-test"))]
    let configured: Option<String> = None;
    let key = match configured {
        Some(key) => key,
        None => {
            let config: serde_json::Value = serde_json::from_str(include_str!("../../../../scripts/release-config.json"))?;
            config["sparkle_public_key"].as_str().context("Missing update key")?.to_owned()
        }
    };
    base64::engine::general_purpose::STANDARD.decode(key)?.try_into().map_err(|_| anyhow!("Invalid update key"))
}

/// `curl.exe` fetching `url` quietly with retries, HTTPS only outside `updater-test` builds.
#[cfg(windows)]
fn curl(url: &str) -> Command {
    let mut cmd = Command::new(system_tool("curl.exe"));
    kiln_common::process::hide_window(&mut cmd).args(["-fsSL", "--retry", "3", "--max-time", "900"]);
    if !cfg!(feature = "updater-test") {
        cmd.args(["--proto", "=https"]);
    }
    cmd.arg(url);
    cmd
}

/// The folder of an installed layout: the running `Kiln.exe` with `bin\kiln.exe` beside it.
#[cfg(windows)]
pub(super) fn installed_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    (exe.file_name()?.eq_ignore_ascii_case("Kiln.exe") && dir.join("bin").join("kiln.exe").is_file()).then(|| dir.to_path_buf())
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(String),
    Installed(String),
    Failed(String),
}

#[cfg(windows)]
static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State::Idle);
#[cfg(windows)]
static ANNOUNCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn set(state: State) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

#[cfg(windows)]
pub(super) fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Checks the feed in the background. An automatic check stays quiet when it fails and
/// announces a release once through [`take_announcement`].
#[cfg(windows)]
pub(super) fn check(automatic: bool) {
    if matches!(state(), State::Checking | State::Installing(_)) {
        return;
    }
    set(State::Checking);
    std::thread::spawn(move || {
        let result = (|| -> Result<Option<Release>> {
            let out = curl(&feed_url()).args(["--max-filesize", "1048576"]).output().context("The update feed could not be read")?;
            ensure!(out.status.success(), "The update feed could not be read: {}", String::from_utf8_lossy(&out.stderr).trim());
            let release = parse_appcast(&String::from_utf8(out.stdout)?, arch())?;
            Ok(newer(&release.version, env!("CARGO_PKG_VERSION")).then_some(release))
        })();
        set(match result {
            Ok(Some(release)) => {
                if automatic {
                    ANNOUNCE.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                State::Available(release)
            }
            Ok(None) => State::UpToDate,
            Err(_) if automatic => State::Idle,
            Err(error) => State::Failed(error.to_string()),
        });
    });
}

/// The version an automatic check found, once.
#[cfg(windows)]
pub(super) fn take_announcement() -> Option<String> {
    if !ANNOUNCE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    match state() {
        State::Available(release) => Some(release.version),
        _ => None,
    }
}

/// Downloads, verifies and installs `release` in the background.
#[cfg(windows)]
pub(super) fn install_release(release: Release) {
    let Some(dir) = installed_dir() else { return };
    set(State::Installing(release.version.clone()));
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let temp = tempfile::Builder::new().prefix("kiln-update-").suffix(".zip").tempfile()?.into_temp_path();
            let out = curl(&release.url).args(["--max-filesize", "314572800", "-o"]).arg(&*temp).output().context("The update could not be downloaded")?;
            ensure!(out.status.success(), "The update could not be downloaded: {}", String::from_utf8_lossy(&out.stderr).trim());
            verify(&std::fs::read(&temp)?, &release, &public_key()?)?;
            install(&temp, &dir, &release.version, |cli| {
                let mut cmd = Command::new(cli);
                let out = kiln_common::process::hide_window(&mut cmd).arg("--version").output()?;
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            })
        })();
        set(match result {
            Ok(()) => State::Installed(release.version),
            Err(error) => State::Failed(error.to_string()),
        });
    });
}

#[cfg(windows)]
fn preferences_path() -> PathBuf {
    kiln_common::paths::config_file("update-checks.json")
}

/// Whether Kiln checks for updates once a day (on unless turned off).
#[cfg(windows)]
pub(super) fn automatic_checks() -> bool {
    std::fs::read(preferences_path()).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).and_then(|v| v["automatic"].as_bool()).unwrap_or(true)
}

#[cfg(windows)]
fn set_automatic_checks(on: bool) {
    let _ = kiln_common::safe_file::write(&preferences_path(), serde_json::json!({ "automatic": on, "last_check": last_check() }).to_string().as_bytes());
}

#[cfg(windows)]
fn last_check() -> u64 {
    std::fs::read(preferences_path()).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).and_then(|v| v["last_check"].as_u64()).unwrap_or(0)
}

/// Checks once a day from an installed layout when automatic checks are on; never while
/// a test configuration folder is in use.
#[cfg(windows)]
pub(super) fn check_daily() {
    if !automatic_checks() || installed_dir().is_none() || std::env::var_os("KILN_CONFIG_DIR").is_some() {
        return;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if now.saturating_sub(last_check()) < 24 * 60 * 60 {
        return;
    }
    let _ = kiln_common::safe_file::write(&preferences_path(), serde_json::json!({ "automatic": automatic_checks(), "last_check": now }).to_string().as_bytes());
    check(true);
}

#[cfg(windows)]
pub(super) fn settings(ui: &mut egui::Ui) {
    use kiln_common::i18n::tr;
    use kiln_common::widgets::{self, ButtonKind};
    if installed_dir().is_none() {
        ui.label(tr("자동 업데이트는 설치된 Windows 배포판에서 제공됩니다."));
        return;
    }
    let mut automatic = automatic_checks();
    if ui.checkbox(&mut automatic, tr("자동으로 업데이트 확인")).changed() {
        set_automatic_checks(automatic);
    }
    let state = state();
    ui.horizontal_wrapped(|ui| {
        let busy = matches!(state, State::Checking | State::Installing(_) | State::Installed(_));
        if ui.add_enabled_ui(!busy, |ui| widgets::button(ui, tr("업데이트 확인…"), ButtonKind::Secondary)).inner.clicked() {
            check(false);
        }
        match &state {
            State::Idle => {}
            State::Checking => {
                ui.label(tr("업데이트를 확인하는 중…"));
            }
            State::UpToDate => {
                ui.label(tr("최신 버전입니다"));
            }
            State::Available(release) => {
                ui.label(kiln_common::trf!("Kiln {} 업데이트를 설치할 수 있습니다", release.version));
                if widgets::button(ui, tr("설치하고 다시 시작"), ButtonKind::Primary).clicked() {
                    install_release(release.clone());
                }
            }
            State::Installing(version) => {
                ui.label(kiln_common::trf!("Kiln {version} 설치 중…"));
            }
            State::Installed(version) => {
                ui.label(kiln_common::trf!("Kiln {version} 설치됨 · 다시 시작하는 중…"));
            }
            State::Failed(error) => {
                ui.label(kiln_common::trf!("업데이트 실패: {error}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    #[cfg(windows)]
    fn layout(root: &Path, gui: &[u8], cli: &[u8]) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("Kiln.exe"), gui).unwrap();
        std::fs::write(root.join("bin").join("kiln.exe"), cli).unwrap();
    }

    #[cfg(windows)]
    fn zipped(dir: &Path) -> PathBuf {
        let source = dir.join("source");
        layout(&source, b"new gui", b"new cli");
        let archive = dir.join("update.zip");
        let status = Command::new(system_tool("tar.exe")).args(["-a", "-c", "-f"]).arg(&archive).arg("-C").arg(&source).args(["Kiln.exe", "bin"]).status().unwrap();
        assert!(status.success());
        archive
    }

    #[cfg(windows)]
    #[test]
    fn install_replaces_both_executables_and_keeps_old_copies_until_restart() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Kiln");
        layout(&installed, b"old gui", b"old cli");
        let archive = zipped(dir.path());
        install(&archive, &installed, "0.2.0", |cli| {
            assert_eq!(std::fs::read(cli).unwrap(), b"new cli");
            Ok("kiln 0.2.0".into())
        })
        .unwrap();
        let read = |path: &[&str]| std::fs::read(path.iter().fold(installed.clone(), |p, part| p.join(part))).unwrap();
        assert_eq!((read(&["Kiln.exe"]), read(&["bin", "kiln.exe"])), (b"new gui".to_vec(), b"new cli".to_vec()));
        assert_eq!((read(&["Kiln.exe.old"]), read(&["bin", "kiln.exe.old"])), (b"old gui".to_vec(), b"old cli".to_vec()));
        assert!(!installed.join(".update-0.2.0").exists());
        remove_old_copies(&installed);
        assert!(!installed.join("Kiln.exe.old").exists() && !installed.join("bin").join("kiln.exe.old").exists());
    }

    #[cfg(windows)]
    #[test]
    fn an_archive_of_another_version_leaves_the_installation_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Kiln");
        layout(&installed, b"old gui", b"old cli");
        let archive = zipped(dir.path());
        assert!(install(&archive, &installed, "0.2.0", |_| Ok("kiln 0.1.9".into())).is_err());
        assert_eq!(std::fs::read(installed.join("Kiln.exe")).unwrap(), b"old gui");
        assert!(!installed.join("Kiln.exe.old").exists() && !installed.join(".update-0.2.0").exists());
    }

    const APPCAST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:kiln="https://github.com/YangTaeyoung/kiln/appcast">
  <channel>
    <title>Kiln updates</title>
    <item>
      <title>Kiln 0.2.0</title>
      <sparkle:version>0.2.0</sparkle:version>
      <enclosure url="https://example.test/Kiln-0.2.0-windows-x64.zip" length="11" type="application/octet-stream" sparkle:os="windows" kiln:arch="x64" sparkle:edSignature="AAEC"/>
      <enclosure url="https://example.test/Kiln-0.2.0-windows-arm64.zip" length="12" type="application/octet-stream" sparkle:os="windows" kiln:arch="arm64" sparkle:edSignature="AwQF"/>
    </item>
  </channel>
</rss>"#;

    fn signed(archive: &[u8]) -> (Release, [u8; 32]) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let key: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
        let release = Release { version: "0.2.0".into(), url: "https://example.test/a.zip".into(), length: archive.len() as u64, signature: pair.sign(archive).as_ref().to_vec() };
        (release, key)
    }

    #[test]
    fn appcast_selects_the_matching_architecture() {
        let arm = parse_appcast(APPCAST, "arm64").unwrap();
        assert_eq!(arm, Release { version: "0.2.0".into(), url: "https://example.test/Kiln-0.2.0-windows-arm64.zip".into(), length: 12, signature: vec![3, 4, 5] });
        assert_eq!(parse_appcast(APPCAST, "x64").unwrap().signature, vec![0, 1, 2]);
        assert!(parse_appcast(APPCAST, "riscv").is_err());
        assert!(parse_appcast(&APPCAST.replace("sparkle:os=\"windows\"", "sparkle:os=\"macos\""), "x64").is_err());
    }

    #[test]
    fn later_versions_compare_by_number() {
        assert!(newer("0.2.0", "0.1.35"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.1.35", "0.1.35"));
        assert!(!newer("0.1.9", "0.1.35"));
    }

    #[test]
    fn a_tampered_archive_is_refused() {
        let archive = b"kiln archive bytes".to_vec();
        let (release, key) = signed(&archive);
        verify(&archive, &release, &key).unwrap();
        let mut tampered = archive.clone();
        tampered[3] ^= 1;
        assert!(verify(&tampered, &release, &key).is_err());
    }

    #[test]
    fn a_wrong_length_is_refused_before_the_signature() {
        let archive = b"kiln archive bytes".to_vec();
        let (mut release, key) = signed(&archive);
        release.length += 1;
        assert!(verify(&archive, &release, &key).unwrap_err().to_string().contains("length"));
    }
}
