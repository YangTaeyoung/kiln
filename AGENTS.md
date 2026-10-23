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
- [Automatic account usage](docs/maintainers/account-usage.md) — background cadence, provider contracts, bounded requests and isolated verification.
- [0.2.5 release verification](docs/maintainers/0.2.5-status.md) — whole-unit credit figures, live 0.2.4 samples, verified archives.
- [0.2.10 release verification](docs/maintainers/0.2.10-status.md) — Antigravity CLI panels, conversation restart and read-only conversation copies, verified archives.
- [0.2.9 release verification](docs/maintainers/0.2.9-status.md) — active account usage with the live login when the runtime login expired, verified archives.
- [0.2.8 release verification](docs/maintainers/0.2.8-status.md) — Codex commands and patches as native Claude Code records, verified archives.
- [0.2.7 release verification](docs/maintainers/0.2.7-status.md) — Claude Code context budget for long synchronized conversations, error notices, verified archives.
- [0.2.6 release verification](docs/maintainers/0.2.6-status.md) — original Codex sessions against Desktop imports, inactive Claude login renewal, verified archives.
- [0.2.4 release verification](docs/maintainers/0.2.4-status.md) — account credits, Codex reset credits and usage history, workspace reordering, verified archives.
- [0.2.3 release verification](docs/maintainers/0.2.3-status.md) — idle Claude login renewal, live CLI credentials, notarized macOS and verified Windows archives.
- [0.2.1 release verification](docs/maintainers/0.2.1-status.md) — automatic usage refresh, independent copy cleanup, notarized macOS and verified Windows archives.
- [0.2.0 release verification](docs/maintainers/0.2.0-status.md) — Windows parity, notarized macOS and signed Windows archives, VM install and update with the release key.
- [0.1.35 release verification](docs/maintainers/0.1.35-status.md) — native records for every tool both CLIs can show, rewritten earlier copies, resume-view verification.
- [0.1.34 release verification](docs/maintainers/0.1.34-status.md) — native shell records across Claude Code and Codex, resume-view and request verification.
- [0.1.33 release verification](docs/maintainers/0.1.33-status.md) — conversation hub, migration, review fixes and incremental synchronization.
- [0.1.32 release verification](docs/maintainers/0.1.32-status.md) — select on press, low-latency frames, 10 fps activity indicator.
- [0.1.31 release verification](docs/maintainers/0.1.31-status.md) — immediate click feedback, link groups computed once per scan, diverged conversations shown as kept.
- [0.1.30 release verification](docs/maintainers/0.1.30-status.md) — failed linked conversations retried only on change, worker results kept across launches.
- [0.1.29 release verification](docs/maintainers/0.1.29-status.md) — conversation-sync worker process, UI-thread I/O and render costs, linked-prompt fixes.
- [0.1.26 release verification](docs/maintainers/0.1.26-status.md) — Codex capacity recovery, native CLI proof and session-preserving update.
- [0.1.25 release verification](docs/maintainers/0.1.25-status.md) — image-preserving conversation sync, stable status UI and native resume validation.
- [0.1.24 release verification](docs/maintainers/0.1.24-status.md) — workspace-based existing-session discovery and initial 20-conversation synchronization.
- [0.1.23 release verification](docs/maintainers/0.1.23-status.md) — native conversation continuity, progress indicators and verified distribution.
- [0.1.22 release verification](docs/maintainers/0.1.22-status.md) — credential selection, account-specific sign-in and responsive CLI updates.
- [0.1.21 release verification](docs/maintainers/0.1.21-status.md) — automatic per-account usage, deadline scheduling and sign-in recovery.
- [Agent accounts](docs/accounts.md) — official CLI browser sign-in and profile isolation.
- [Antigravity CLI](docs/antigravity.md) — agy launch and initial prompt, presence-lock conversation tracking, read-only copies into Claude Code and Codex, no account management.
- [Localization and native menus](docs/localization.md) — catalog, font caches, and isolated session-control tests.
- [Shell completion](docs/shell-completion.md) — Zsh buffer bridge, bounded local suggestions, private metadata and native-input boundaries.
- [Query-result editing](docs/query-results.md) — source proof, exact-row transactions, execution scope and review-only recovery.
- [Object storage](docs/object-storage.md) — reusable authentication profiles, native bucket discovery and permission boundaries.
- [Terminal recovery](docs/terminal-recovery.md) — preserve child processes, bounded repair, upstream comparisons and isolated regression fixtures.
- [Local network access](docs/local-network.md) — explicit permission request, distinct executable UUIDs and native verification boundaries.
- **[Release runbook](docs/maintainers/releases.md)** — versioning, signing,
  notarization, Sparkle updates, GitHub Release publication, and recovery.
- [Signing setup](docs/maintainers/signing.md) — public identifiers and Keychain
  setup, never passwords or private keys.
- [Codex capacity recovery](docs/codex-capacity-recovery.md) — bounded retries, exact-session resume and cancellation.
- [CLI updates and exact conversation restart](docs/cli-updates.md) — persistent account namespaces, hook trust and isolated restart checks.
- [Native updater validation](docs/maintainers/updater-validation.md) — isolated
  old/new fixtures, cancellation, relaunch and session survival checks.
- [0.1.9 release verification](docs/maintainers/0.1.9-status.md) — schema editing, file-language assistance and distribution evidence.
- [0.1.11 release verification](docs/maintainers/0.1.11-status.md) — pane layout, remote files, browser sign-in, table dialog and terminal recovery.
- [0.1.16 release verification](docs/maintainers/0.1.16-status.md) — object-storage profile/bucket catalogs, safe SELECT editing, shell suggestions, remote UI review and public artifact evidence.
- [0.1.19 release verification](docs/maintainers/0.1.19-status.md) — saved-profile sign-in recovery, account isolation and distribution evidence.
- [0.1.18 release verification](docs/maintainers/0.1.18-status.md) — CLI update/restart fixtures, persistent account credentials and distribution evidence.
- [0.1.17 release verification](docs/maintainers/0.1.17-status.md) — inspector navigation, independent file editors, transfer/recovery guards, source CI and exact public artifact evidence.
- [0.1.15 release verification](docs/maintainers/0.1.15-status.md) — named workspaces, unified tools, native object profiles, researched connection UI and public distribution evidence.
- [0.1.14 release verification](docs/maintainers/0.1.14-status.md) — bounded terminal connection repair, redraw/resize regressions and exact public artifact evidence.
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

Windows changes are cross-checked from the Mac with `cargo xwin check -p kiln --bins
--tests --target aarch64-pc-windows-msvc` and run on Windows by the `windows-x64` and
`windows-arm64` CI jobs (each split into libraries, crates and terminal); [Windows support](docs/windows-parity.md) lists the VM
scenarios behind each feature.

Tests must use isolated configuration/socket/account paths. Do not shut down the
real session daemon to make a test pass. Public-facing documentation is English;
keep README short and put detailed guides under `docs/` with relative links.
