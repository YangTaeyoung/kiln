//! Kiln 앱 라이브러리(GUI). 실행 파일은 `main.rs`.

pub mod app;

#[cfg(target_os = "macos")]
pub mod status_bar;

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
