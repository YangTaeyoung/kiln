# Native updater validation

Run this in addition to the application and Git integration suites before changing
the updater. Never use the user's real workspace to test update failure or data loss.
Follow [release signing](signing.md) first; the fixtures require the existing signing
identity and Sparkle key, without exporting either private key.

## Build an isolated fixture

Start from a successfully signed `target/release/Kiln.app`. The test feature is off
by default. It embeds a dedicated temporary root so Launch Services relaunches and
daemon/PTY children keep using the same isolated configuration and socket, even
without inherited environment variables. It also marks `--version` as `updater-test`;
the production bundle script rejects that executable.

```sh
fixture_root=$(mktemp -d /tmp/kiln-updater-XXXXXXXX)
KILN_UPDATER_TEST_ROOT="$fixture_root" cargo build --locked --release -p kiln --bins --features updater-test
python3 scripts/prepare-updater-test.py "$fixture_root" \
  'Developer ID Application: Taeyoung Yang (G54PSSU8W5)'
python3 -m http.server "$(cat "$fixture_root/port")" --bind 127.0.0.1 \
  --directory "$fixture_root/download"
```

The last command serves the fixture and stays running. In another terminal, open
`installed/Kiln Updater Test.app` with the empty test workspace as its path argument.
The script makes versions 0.0.1 and 0.0.2, with a separate `dev.kiln.updater-test`
bundle ID and a loopback feed. HTTP permission is set only in the fixture's plist.
These are signed local fixtures, **not notarized public distribution artifacts**.

The fixture build temporarily occupies `target/release/kiln`. Always rebuild with
`scripts/release-macos.sh` before producing an official release. Never publish a
fixture, its appcast, or its temporary configuration.

## Exercise the real flow

1. Start a shell in the fixture and record session IDs and PIDs with its `kiln ls --json`.
2. Create a visible unsaved agent request or editor draft. Do not launch a real agent.
3. Choose the native **Check for Updates** menu and install the offered update.
4. When Kiln asks about unsaved work, cancel. Confirm the same GUI is alive and the
   draft is unchanged. If automation fails, do not count the intended click as success.
5. Retry and preserve the draft on exit. Confirm the newer bundle version, a new GUI
   process, restored draft and unchanged shell PIDs. Exercise terminal input again.
6. For the menu companion, launch only the fixture's nested helper. Record its PID
   and executable inode before/after replacement; verify it re-execs the current
   executable without killing the daemon or PTYs.
7. Repeat with a clean saved workspace and with a controlled save failure.

Close the fixture GUI, shut down **only the fixture daemon** with its own CLI, and
stop its helper and local HTTP server. Remove only the recorded temporary root.
Keep live-user screenshots and logs out of public source.

Record actual observations and distinguish local fixture success from notarized,
downloaded production-build verification. See [Sparkle's testing guidance](https://sparkle-project.org/documentation/#6-test-sparkle-out).
