#[path = "../../kiln/src/app/updater_windows.rs"]
mod updater_windows;
use anyhow::{Result, bail, ensure};
use std::time::{Duration, Instant};
use updater_windows::State;

fn wait_until(done: impl Fn(&State) -> bool) -> Result<State> {
    let until = Instant::now() + Duration::from_secs(120);
    while Instant::now() < until {
        let state = updater_windows::state();
        if let State::Failed(error) = &state { bail!("{error}"); }
        if done(&state) { return Ok(state); }
        std::thread::sleep(Duration::from_millis(80));
    }
    bail!("updater timed out: {:?}", updater_windows::state());
}

fn main() -> Result<()> {
    ensure!(env!("CARGO_PKG_VERSION") == "0.2.23");
    let root = updater_windows::installed_dir().expect("installed layout");
    updater_windows::check_daily();
    let State::Available(release) = wait_until(|state| matches!(state, State::Available(_)))? else { unreachable!() };
    ensure!(updater_windows::take_announcement().as_deref() == Some("0.2.24"));
    ensure!(release.version == "0.2.24");
    println!("0.2.23 automatically detected signed candidate {} for {}", release.version, updater_windows::arch());
    updater_windows::install_release(release);
    let state = wait_until(|state| matches!(state, State::Installed(_)))?;
    ensure!(state == State::Installed("0.2.24".into()));
    ensure!(root.join("Kiln.exe.old").is_file(), "running executable was not set aside");
    ensure!(root.join("bin/kiln.exe.old").is_file(), "previous CLI was not set aside");
    println!("0.2.23 updater downloaded, verified and replaced its running executable and CLI");
    Ok(())
}
