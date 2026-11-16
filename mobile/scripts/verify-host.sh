#!/usr/bin/env bash
# Synthetic provider protocol + actual source-built Kiln CLI/daemon/PTY effects.
set -euo pipefail
if [[ $# != 0 || $(uname -s) != Darwin ]]; then
  echo 'Usage: verify-host.sh (macOS; isolated fixture suite)' >&2
  exit 2
fi
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$repo/target"}
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR=$(cd "$CARGO_TARGET_DIR" && pwd)
cargo build --locked -p kiln --bin kiln
export KILN_MOBILE_TEST_CLI="$CARGO_TARGET_DIR/debug/kiln"
[[ $("$KILN_MOBILE_TEST_CLI" --version) == 'kiln 0.3.0' ]] || {
  echo 'Expected the current source-built Kiln 0.3.0 CLI' >&2; exit 1;
}
# Keep legacy shared Client deadlines/upgrade behavior covered alongside opt-in calls.
cargo test --locked -p kiln-daemon --lib client::
cargo test --locked --manifest-path mobile/host/Cargo.toml \
  --test http_boundary --test native_roundtrip -- --test-threads=1
