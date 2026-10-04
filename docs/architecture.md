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

## Foreground agent identity

The daemon samples foreground jobs for display without changing PTY ownership,
input routing or signal targets. On macOS it follows a known Kiro terminal bridge
into its unique inner shell and verifies that shell's controlling tty and foreground
process group. Background jobs do not determine the displayed agent. The process
resolver is in `crates/kiln-daemon/src/procinfo.rs`; all agent surfaces use the
connection's shared identity in `crates/kiln/src/app/conn.rs`.

A confirmed foreground `codex` or `claude` identifies an idle agent immediately,
including after GUI reconnect. Known OSC prefixes remain a fallback only for exact
unresolved Kiro bridge names. A confirmed shell/editor or failed process lookup
clears stale title-based identity and activity; explicit integration state remains
separate from process identity. Never infer task progress merely because a process
is alive.

References: [Apple process metadata](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h),
[Apple libproc](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.h).
