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
expected_version=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c \
  'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "kiln"))')
[[ $("$KILN_MOBILE_TEST_CLI" --version) == "kiln $expected_version" ]] || {
  echo "Expected the current source-built Kiln $expected_version CLI" >&2; exit 1;
}
# Keep legacy shared Client deadlines/upgrade behavior covered alongside opt-in calls.
cargo test --locked -p kiln-daemon --lib client::
cargo test --locked --manifest-path mobile/host/Cargo.toml \
  --test http_boundary --test native_roundtrip -- --test-threads=1
