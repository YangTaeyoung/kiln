#!/bin/sh
# Install the official, notarized Apple Silicon release. Existing apps update in Kiln.
set -eu

fail() { printf 'Kiln: %s\n' "$*" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || fail 'The prebuilt app requires macOS.'
[ "$(uname -m)" = arm64 ] || fail 'The current release supports Apple Silicon.'
install_dir=${KILN_INSTALL_DIR:-/Applications}
app="$install_dir/Kiln.app"
if [ -e "$app" ]; then
    printf 'Kiln is already installed. Open Kiln → Check for Updates to update safely.\n'
    exit 0
fi
mkdir -p "$install_dir"
[ -w "$install_dir" ] || fail "Cannot write to $install_dir. Set KILN_INSTALL_DIR to a writable folder."
work=$(mktemp -d "${TMPDIR:-/tmp}/kiln-install.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
repo=https://github.com/YangTaeyoung/kiln
fetch() { /usr/bin/curl -fLsS --retry 3 --connect-timeout 10 --max-time 180 "$@"; }
latest=$(fetch -o /dev/null -w '%{url_effective}' "$repo/releases/latest")
version=${latest##*/v}
printf '%s\n' "$version" | /usr/bin/grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || fail 'Cannot resolve the latest stable release.'
archive="Kiln-$version-macos-arm64.zip"
base="$repo/releases/download/v$version"
printf 'Downloading Kiln %s…\n' "$version"
fetch -o "$work/$archive" "$base/$archive"
fetch -o "$work/SHA256SUMS.txt" "$base/SHA256SUMS.txt"
expected=$(/usr/bin/awk -v name="$archive" 'NF == 2 && $2 == name {print $1}' "$work/SHA256SUMS.txt")
printf '%s\n' "$expected" | /usr/bin/grep -Eq '^[0-9a-f]{64}$' || fail 'Invalid release checksum.'
actual=$(/usr/bin/shasum -a 256 "$work/$archive" | /usr/bin/awk '{print $1}')
[ "$expected" = "$actual" ] || fail 'Release checksum mismatch.'
# Refuse paths outside the app before extracting the official archive.
/usr/bin/unzip -Z1 "$work/$archive" > "$work/entries"
/usr/bin/awk 'index($0,"Kiln.app/") != 1 || $0 ~ /(^|\/)\.\.(\/|$)/ {bad=1} END {exit bad}' "$work/entries" || fail 'Unexpected archive contents.'
/usr/bin/ditto -x -k "$work/$archive" "$work/extracted"
staged="$work/extracted/Kiln.app"
/usr/bin/codesign --verify --deep --strict "$staged" || fail 'App signature verification failed.'
/usr/bin/codesign --display --verbose=4 "$staged" 2> "$work/signature"
/usr/bin/grep -qx 'TeamIdentifier=G54PSSU8W5' "$work/signature" || fail 'Unexpected app signer.'
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$staged/Contents/Info.plist")" = dev.kiln.app ] || fail 'Unexpected app identity.'
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$staged/Contents/Info.plist")" = "$version" ] || fail 'Release version mismatch.'
/usr/sbin/spctl --assess --type execute "$staged" || fail 'macOS did not accept the notarized app.'
# Never replace an installed app or disrupt its running terminals.
[ ! -e "$app" ] || fail 'Kiln was installed during this download. Use its updater.'
/bin/mv -n "$staged" "$install_dir/"
[ ! -e "$staged" ] || fail 'Installation destination changed; nothing was replaced.'
printf 'Installed Kiln %s. Open %s to start.\n' "$version" "$app"
