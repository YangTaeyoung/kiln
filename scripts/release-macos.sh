#!/usr/bin/env bash
# Build local, notarized release artifacts. Publishing is a separate explicit step.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -m)" != arm64 ]]; then echo 'This release channel currently supports Apple Silicon only.' >&2; exit 1; fi
: "${KILN_SIGN_IDENTITY:?Set KILN_SIGN_IDENTITY to your Developer ID Application identity}"
: "${KILN_NOTARY_PROFILE:?Set KILN_NOTARY_PROFILE to an existing notarytool Keychain profile}"
if [[ "$KILN_SIGN_IDENTITY" != Developer\ ID\ Application:* ]]; then
  echo 'A Developer ID Application identity is required for a release.' >&2; exit 1
fi
VERSION=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')
if [[ "${1:-}" != "$VERSION" ]]; then echo "Usage: $0 $VERSION (must match Cargo.toml)" >&2; exit 1; fi
# Check credentials before the expensive build; never print/export private keys.
xcrun notarytool history --keychain-profile "$KILN_NOTARY_PROFILE" --output-format json > /dev/null
python3 scripts/fetch-sparkle.py
KEY_ACCOUNT=$(python3 -c 'import json; print(json.load(open("scripts/release-config.json"))["sparkle_key_account"])')
PUBLIC_KEY=$(target/vendor/sparkle/bin/generate_keys --account "$KEY_ACCOUNT" -p)
python3 - "$PUBLIC_KEY" <<'PY'
import json,sys
assert sys.argv[1].strip()==json.load(open('scripts/release-config.json'))['sparkle_public_key'], 'Wrong update signing key; do not rotate it accidentally'
PY
bash scripts/bundle-macos.sh
ARCH=$(uname -m)
OUT="target/distribution/$VERSION"
mkdir -p "$OUT"
ARCHIVE="$OUT/Kiln-$VERSION-macos-$ARCH.zip"
# This is a fresh archive AFTER stapling, not the submission archive.
ditto -c -k --keepParent target/release/Kiln.app "$ARCHIVE"
target/vendor/sparkle/bin/sign_update --account "$KEY_ACCOUNT" "$ARCHIVE" > "$OUT/signature.txt"
python3 scripts/make-appcast.py "$VERSION" "$ARCHIVE" "$OUT/signature.txt"
shasum -a 256 "$ARCHIVE" | sed "s|$OUT/||" > "$OUT/SHA256SUMS.txt"
echo "Notarized artifacts ready: $OUT"
echo "Next: follow docs/maintainers/releases.md to publish the reviewed release."
