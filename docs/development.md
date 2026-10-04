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
when updating dependencies. Validate both event conversion and terminal delivery:

```sh
cargo test --locked -p egui-winit --lib kiln_ime_tests
cargo test --locked -p kiln --lib ime_
cargo test --locked -p kiln --test gui ime_commits_reach_only_the_focused_pty_once_and_cancellation_sends_nothing
```

Native checks should cover Korean composition, Backspace-to-empty, Enter,
Escape, Korean/Latin input-source changes, another tab/panel, app blur/return,
search, and the quick-terminal window. Confirm there are no leftover glyphs or
unexpected committed characters. Event replay tests do not establish native IME
latency or candidate-popup behavior.
