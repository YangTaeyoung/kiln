//! A distinct Mach-O entry point for dev.kiln.statusbar (Apple TN3178).
//! Never run the GUI, create a daemon, or own a PTY from this executable.
fn main() -> anyhow::Result<()> {
    #[cfg(feature = "updater-test")]
    kiln::isolate_updater_fixture()?;
    if std::env::args().any(|arg| arg == "--version") {
        println!(
            "kiln-status {}{}",
            env!("CARGO_PKG_VERSION"),
            if cfg!(feature = "updater-test") {
                "-updater-test"
            } else {
                ""
            }
        );
        return Ok(());
    }
    #[cfg(feature = "updater-test")]
    if std::env::args().any(|arg| arg == "updater-test-root") {
        println!(
            "{}",
            std::path::Path::new(option_env!("KILN_UPDATER_TEST_ROOT").unwrap())
                .canonicalize()?
                .display()
        );
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        kiln::status_bar::run()
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("The menu-bar companion requires macOS")
    }
}
