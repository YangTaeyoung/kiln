//! Kiln 앱 라이브러리(GUI). 실행 파일은 `main.rs`.

pub mod app;
pub mod native_actions;

#[cfg(target_os = "macos")]
pub mod status_bar;

#[cfg(target_os = "macos")]
pub mod local_network;

/// Native update fixture evidence; absent from all production builds.
#[cfg(feature = "updater-test")]
pub fn updater_fixture_event(event: &str) {
    use std::io::Write;
    let Some(root) = option_env!("KILN_UPDATER_TEST_ROOT") else { return; };
    let file = std::path::Path::new(root).join("lifecycle.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let _ = writeln!(file, "{now} pid={} {event}", std::process::id());
    }
}

/// Sparkle relaunches through Launch Services without our launch environment.
/// Compile the fixture root into test builds so every child/relaunch stays isolated.
#[cfg(feature = "updater-test")]
pub fn isolate_updater_fixture() -> anyhow::Result<()> {
    use std::path::Path;
    let root = option_env!("KILN_UPDATER_TEST_ROOT")
        .ok_or_else(|| anyhow::anyhow!("updater-test requires KILN_UPDATER_TEST_ROOT at build time"))?;
    let root = Path::new(root).canonicalize()?;
    anyhow::ensure!(root.starts_with("/private/tmp")
        && root.file_name().is_some_and(|n| n.to_string_lossy().starts_with("kiln-updater-")),
        "updater-test root must be a dedicated /tmp/kiln-updater-* directory");
    std::fs::create_dir_all(root.join("config"))?;
    // First action in main, before AppKit, logging, or any worker thread starts.
    unsafe {
        std::env::set_var("KILN_CONFIG_DIR", root.join("config"));
        std::env::set_var("KILN_SOCKET", root.join("daemon.sock"));
        std::env::set_var("SHELL", "/bin/sh");
    }
    Ok(())
}
