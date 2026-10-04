# Architecture

Kiln uses Rust and egui. The GUI is a client of the local session daemon; terminal
ownership does not depend on the lifetime of a particular window.

| Component | Responsibility |
| --- | --- |
| `crates/kiln` | GUI, workspaces, task navigation, macOS integration |
| `crates/kiln-daemon` | PTY ownership, terminal emulation, session lifecycle |
| `crates/kiln-proto` | Client/daemon messages |
| `crates/kiln-common` | Theme, typography, widgets, paths |
| `crates/kiln-editor` | Files, editor, search and LSP |
| `crates/kiln-git` | Git/GitHub operations and history review |
| `crates/kiln-db` | Database connections, grids and SQL |
| `crates/kiln-accounts` | Local agent account profiles |

On macOS, a separate menu-bar companion reports background sessions. It does not
own their PTYs. Official app bundles embed Sparkle; only the GUI initializes its updater.
CLI commands, daemons and isolated tests must not initialize it.

The native termination guard routes quit requests through the GUI's draft/save
handling. Keep it intact when changing the updater or application delegate.

See [development](development.md) and the [release runbook](maintainers/releases.md).
