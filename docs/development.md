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
