//! The Kiln GUI; on Windows it has no console window. The `kiln` executable is the CLI.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    kiln::cli::gui_main()
}
