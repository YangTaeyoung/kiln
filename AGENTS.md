# Working on Kiln

Kiln is a Rust/egui terminal workspace. Keep the GUI, session daemon, PTY hosts,
and macOS menu-bar companion distinct. Closing/updating the GUI must not kill
the user's shells, agents, builds, or servers.

## Start here

- [Documentation index](docs/README.md)
- [Architecture and code map](docs/architecture.md)
- [Development and meaningful verification](docs/development.md)
- [Pane layout](docs/pane-layout.md) — accepted movement, boundary scope and snapping behavior.
- [Remote files and verification](docs/maintainers/remote-files-verification.md) — isolated provider fixtures and explicit coverage boundaries.
- [Agent accounts](docs/accounts.md) — official CLI browser sign-in and profile isolation.
- [Localization and native menus](docs/localization.md) — catalog, font caches, and isolated session-control tests.
- [Terminal recovery](docs/terminal-recovery.md) — preserve child processes, bounded repair, upstream comparisons and isolated regression fixtures.
- [Local network access](docs/local-network.md) — explicit permission request, distinct executable UUIDs and native verification boundaries.
- **[Release runbook](docs/maintainers/releases.md)** — versioning, signing,
  notarization, Sparkle updates, GitHub Release publication, and recovery.
- [Signing setup](docs/maintainers/signing.md) — public identifiers and Keychain
  setup, never passwords or private keys.
- [Native updater validation](docs/maintainers/updater-validation.md) — isolated
  old/new fixtures, cancellation, relaunch and session survival checks.
- [0.1.9 release verification](docs/maintainers/0.1.9-status.md) — schema editing, file-language assistance and distribution evidence.
- [0.1.11 release verification](docs/maintainers/0.1.11-status.md) — pane layout, remote files, browser sign-in, table dialog and terminal recovery.
- [0.1.13 release verification](docs/maintainers/0.1.13-status.md) — macOS hosted terminal ownership, mixed-session migration and failure recovery.
- [0.1.12 release verification](docs/maintainers/0.1.12-status.md) — local-network permission metadata, distinct companion identity and Claude Keychain home.
- [0.1.8 release verification](docs/maintainers/0.1.8-status.md) — SQL completion, connected metadata, editor regressions and distribution evidence.
- [0.1.7 release verification](docs/maintainers/0.1.7-status.md) — database inspector, terminal palette queries, ordinal-free task names and published artifact verification.
- [0.1.6 release verification](docs/maintainers/0.1.6-status.md) — shared branding, saved appearance, portable icon rendering and published artifacts; installed-device update observation is pending.
- [0.1.5 release verification](docs/maintainers/0.1.5-status.md) — published artifacts, installed version/session survival and pending menu-icon recovery observation.
- [0.1.4 release verification](docs/maintainers/0.1.4-status.md) — foreground identity, branded session surfaces, signed artifacts and observed live update.
- [0.1.3 release verification](docs/maintainers/0.1.3-status.md) — menu template, agent identity and native punctuation regression.
- [0.1.2 release verification](docs/maintainers/0.1.2-status.md) — IME cancellation, artifact checks and native observation boundary.
- [0.1.1 release verification](docs/maintainers/0.1.1-status.md) — published artifact evidence and user update boundary.
- [First-release handoff](docs/maintainers/first-release-status.md) — pending work
  and verified preparation; recheck its dated status before continuing.

## Change and release rules

1. Inspect the current checkout and remote before editing. Preserve unrelated
   local changes; never stash/reset someone else's work to obtain a clean tree.
2. Read the release runbook before changing the updater, bundle script, version,
   update feed, signing keys, or GitHub releases. `scripts/release-config.json`
   is the source of truth for the repository, bundle ID and Sparkle public key.
3. Never regenerate or rotate the update key just because a build machine does
   not have it. Never commit/export/upload private keys, auth files, passwords,
   real-user terminal output, or local review captures.
4. Do not create a public release until notarization is **Accepted**, stapling
   and Gatekeeper assessment succeed, and the final archive signature is checked.
   A Developer ID signature alone is not notarization.
5. Preserve `dev.kiln.app` and the existing update public key for upgrade
   continuity. Versions and tags are immutable once published. Do not replace a
   published archive with different bytes under the same version.
6. Verify the real user path in addition to unit tests. For updater work include
   saved/unsaved work, cancellation, session survival, helper replacement and
   relaunch. State any unverified boundary honestly.
7. Follow the active user's authorization for publication. Do not infer permission
   to publish future releases solely from this document.

## Common commands

```sh
cargo check -p kiln
cargo test -p kiln --lib
cargo test -p kiln --test gui
cargo test -p kiln-git
git diff --check
```

Tests must use isolated configuration/socket/account paths. Do not shut down the
real session daemon to make a test pass. Public-facing documentation is English;
keep README short and put detailed guides under `docs/` with relative links.
