//! Helper processes started without a visible window.
use std::process::Command;

/// Starts `cmd` without a console window on Windows (`CREATE_NO_WINDOW`); other systems
/// are unchanged.
pub fn hide_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
