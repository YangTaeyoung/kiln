# Architecture

Kiln uses Rust and egui. The GUI is a client of the local session daemon; terminal
ownership does not depend on the lifetime of a particular window.

New macOS and Windows panels use one persistent PTY host per terminal; the daemon
relays their output and owns emulation and session metadata. Retained native Unix
panels keep their original daemon parent during updates. Mixed native/hosted
handover replaces that daemon with the same PID and preserves both descriptor
and endpoint state. See [local-network access](local-network.md) for macOS
attribution, existing-panel limitations and isolated verification.

| Component | Responsibility |
| --- | --- |
| `crates/kiln` | GUI, workspaces, task navigation, macOS integration |
| `crates/kiln-daemon` | PTY ownership, terminal emulation, session lifecycle |
| `crates/kiln-proto` | Client/daemon messages |
| `crates/kiln-common` | Theme, typography, widgets, paths |
| `crates/kiln-editor` | Files, editor, search and LSP |
| `crates/kiln-git` | Git/GitHub operations and history review |
| `crates/kiln-db` | Database connections, grids and SQL |
| `crates/kiln-accounts` | Local agent account profiles and background CLI sign-in |
| `crates/kiln-remote` | Remote connections, file browser and isolated transfer workers |

On macOS, a separate menu-bar companion reports background sessions. It does not
own their PTYs. Official app bundles embed Sparkle; only the GUI initializes its updater.
CLI commands, daemons and isolated tests must not initialize it.

The native termination guard routes quit requests through the GUI's draft/save
handling. Keep it intact when changing the updater or application delegate.

See [development](development.md) and the [release runbook](maintainers/releases.md).

## Database schema changes

`kiln-db` owns schema inspection, form drafts and execution. The table surface
provides Data, Columns, Indexes and DDL sections. `meta.rs` supplies table/index
metadata; `tab/table/schema_ui.rs` owns the form, preview jobs and explicit review
state. Forms restore their inputs, not an executable approval. A restored form
that was applying asks the user to check the prior result instead of retrying it.
See [form lifecycle](../crates/kiln-db/src/tab/table/schema_ui.rs:7) and
[database workflows](databases.md).

`schema.rs` separates `SchemaAction` from a prepared `SchemaPlan`. Preparation
captures SQL, the target, connection epoch and schema evidence. Apply rejects
altered plans, changed connections and changed table metadata/DDL before execution.
PostgreSQL revalidates its catalog stamp after acquiring the table lock inside a
transaction. SQLite checks its schema version inside an immediate transaction;
rebuilds also validate foreign keys. MySQL plans contain one DDL statement and
must not be described as supporting rollback of multiple DDL statements.
See [plan application](../crates/kiln-db/src/schema.rs:811) and
[driver execution](../crates/kiln-db/src/schema.rs:866).

Successful changes advance the connection's schema epoch. Table views invalidate
cached data/details and reload; views with pending row edits preserve those edits
and enter a conflict/review state instead. Pending row edits block schema actions
until applied or cancelled. Closing a tab is not a cancellation contract for an
already-running schema change. See
[epoch handling](../crates/kiln-db/src/tab/table/schema_ui.rs:70) and
[apply-state UI](../crates/kiln-db/src/tab/table/schema_ui.rs:489).

## Editor language and completion

`kiln-editor` owns automatic language detection and the per-editor override.
`Editor::refresh_language` uses bounded document content with the file path;
changing the effective language updates highlighting, comment behavior and LSP
attachment. Explicit Plain Text disables language-server attachment. The toolbar
is an editor-local control, not an application-wide language setting. See
[language selection](../crates/kiln-editor/src/editor/mod.rs:245) and
[editor workflows](editor.md).

Completion uses the existing LSP edit pipeline when a document is attached, with
local suggestions when it is not attached or a completion request fails.
`editor/local_completion.rs` supplies bounded keyword and document-word candidates;
it does not supply semantic analysis. `editor/lsp_glue.rs` shares the popup,
filtering, acceptance and edit-history path between sources. Delayed completion
results and acceptance are checked against document version and caret position.
Accepted replacement and additional edits use the editor's undo history.
See [completion sources](../crates/kiln-editor/src/editor/lsp_glue.rs:646),
[acceptance checks](../crates/kiln-editor/src/editor/lsp_glue.rs:772) and the
[LSP completion specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion).

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
