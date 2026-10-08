# Getting started

## Install on macOS

Download the Apple Silicon archive from [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest),
extract it, and move **Kiln.app** into **Applications** before opening it.
The macOS release requires macOS 11 or later and an Apple Silicon Mac.
Intel, Windows and Linux release artifacts are not currently provided.

The official build checks for updates automatically. Use **Kiln → Check for
Updates** (shown as **업데이트 확인…** in the current Korean interface), or the
update control in **Settings → About**, to check manually. Installation uses
Sparkle's update dialog. Automatic checks can be disabled in Settings.

The app UI supports English, Korean, Japanese and Simplified Chinese. Documentation is in English.

## Open a workspace

Choose a folder with the **+** button in the workspace sidebar. A workspace can
be one repository or a parent directory containing multiple repositories.
Open your development folder once; your agent can work across its repositories.

Use a terminal directly, or choose **New task**, enter a request, pick Codex or
Claude, and start. The corresponding CLI must already be installed and signed in.
Kiln does not include a subscription to either agent.

## Open a tool

The titlebar **Tools** menu lists files, search, source control, GitHub, databases,
remote connections and problems together. Choose a tool to open it alongside your
current work. In compact windows, **Back to work** returns to the terminal.
See [remote files and SSH](remote-files.md) and [databases](databases.md).

## Keep your place

Use the sidebar to switch workspaces and tasks. Reported running, waiting,
completed and failed states appear beside each task. Unknown state is shown as
an open session rather than guessed from terminal output.

Closing the window leaves terminal sessions in the local daemon. Reopen Kiln to
return to them. A system reboot or explicitly stopping the daemon is different
from closing the GUI and can terminate sessions.

Next: [workspaces and agents](workspaces.md) · [Git workflows](git.md).

## Terminal installation

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | sh
```

The installer downloads the latest stable Apple Silicon app, checks the release
checksum, Developer ID signature, bundle identity and macOS distribution approval,
and installs it in `/Applications`. It leaves an existing app in place; use
**Kiln → Check for Updates** for upgrades, keeping running sessions intact.
To choose another installation folder:

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | KILN_INSTALL_DIR="$HOME/Applications" sh
```
