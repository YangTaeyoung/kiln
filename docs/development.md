# Development

Install stable Rust, the macOS command-line developer tools, and Git. GitHub features
also need the GitHub CLI (`gh`) and an authenticated account.
Release scripts require Python 3.11 or newer (`tomllib` is used to read Cargo metadata).

```sh
cargo run -p kiln -- /path/to/workspace
cargo check -p kiln
cargo test -p kiln --lib
cargo test -p kiln --test gui
cargo test -p kiln-git
git diff --check
```

GUI tests use egui_kittest. Daemon tests start real PTYs in isolated configurations.
Use targeted suites for the files you change, then relevant integration tests.
When the daemon protocol changes, verify the real previous published executable,
not only a same-binary restart or synthetic restore format:

```sh
# Supply an owned extraction/copy of the previous release's kiln executable.
KILN_TEST_OLD_EXE=/tmp/previous-release/Kiln.app/Contents/MacOS/kiln \
  cargo test --locked -p kiln --test published_upgrade -- --test-threads=1 --nocapture
```

This opt-in gate creates private daemon sockets/configuration and checks protocol
5 to 6 handover in both native and hosted modes, preserving shell PID, screen and
real input/output. If the variable is absent, the tests explicitly skip verification;
that run does not establish published-version compatibility or a Sparkle GUI update.

SQL completion is tested against real SQLite metadata with keyboard, mouse,
undo/redo, connection replacement, IME event replay, four languages and bounded
light/dark popup renders. Run it serially because UI language/theme are global:

```sh
cargo test --locked -p kiln-db --lib
cargo test --locked -p kiln-db --test sql_completion -- --test-threads=1
# Starts and removes isolated PostgreSQL/MySQL containers:
bash crates/kiln-db/tests/run_docker_tests.sh
```

SELECT-result editing has separate proof/rollback and real-grid execution tests:

```sh
cargo test --locked -p kiln-db --test result_edit
cargo test --locked -p kiln-db --test query_execution -- --test-threads=1
```

The CI `database-servers` job runs `result_edit_servers` against owned PostgreSQL,
MySQL and MariaDB containers. For a local equivalent, supply isolated test URLs in
`KILN_TEST_PG_URL`, `KILN_TEST_MYSQL_URL` and `KILN_TEST_MARIA_URL`; absent URLs
explicitly skip that engine and do not establish server coverage.

Zsh completion's daemon tests use a real owned PTY; application unit tests render
short/narrow suggestion menus and verify native-input and dismissal behavior.

Database server tests require their documented local containers; do not point tests
at a user's real databases or repositories.

```sh
bash scripts/bundle-macos.sh
```

This creates `target/release/Kiln.app`. Without a Developer ID identity it is a
local ad-hoc build, **not** a public notarized release. Sparkle's official pinned
framework is downloaded and checksum-verified during bundling.

Do not replace a running user's session daemon with a test process. Keep local
captures, logs, credentials and build artifacts out of commits. For distribution,
follow the [release runbook](maintainers/releases.md).

## Terminal IME regressions

The small [egui-winit patch](../vendor/egui-winit/KILN-PATCH.md) preserves native
composition cancellation. Keep its provenance, licenses and backend regression
when updating dependencies. The [winit patch](../vendor/winit/KILN-PATCH.md)
repairs macOS post-composition trigger loss and duplicate spaces. Its hidden-window
callback test runs AppKit on the main thread with deterministic IME callbacks;
it does not change the system input source or automate the user's keyboard.
Validate native callbacks, event conversion and terminal delivery:

```sh
cargo test --locked -p egui-winit --lib kiln_ime_tests
cargo test --locked -p kiln --test native_ime
cargo test --locked -p kiln --lib ime_
cargo test --locked -p kiln --test gui ime_commits_reach_only_the_focused_pty_once_and_cancellation_sends_nothing
```

Native checks should cover Korean composition, Backspace-to-empty, Enter,
Escape, Korean/Latin input-source changes, another tab/panel, app blur/return,
search, and the quick-terminal window. Confirm there are no leftover glyphs or
unexpected committed characters. Event replay tests do not establish native IME
latency or candidate-popup behavior.
