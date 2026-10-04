# Working on Kiln

Kiln is a Rust/egui terminal workspace. Keep the GUI, session daemon, PTY hosts,
and macOS menu-bar companion distinct. Closing/updating the GUI must not kill
the user's shells, agents, builds, or servers.

## Start here

- [Documentation index](docs/README.md)
- [Architecture and code map](docs/architecture.md)
- [Development and meaningful verification](docs/development.md)
- [Localization and native menus](docs/localization.md) — catalog, font caches, and isolated session-control tests.
- **[Release runbook](docs/maintainers/releases.md)** — versioning, signing,
  notarization, Sparkle updates, GitHub Release publication, and recovery.
- [Signing setup](docs/maintainers/signing.md) — public identifiers and Keychain
  setup, never passwords or private keys.
- [Native updater validation](docs/maintainers/updater-validation.md) — isolated
  old/new fixtures, cancellation, relaunch and session survival checks.
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
