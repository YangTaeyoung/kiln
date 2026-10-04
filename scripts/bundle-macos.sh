#!/usr/bin/env bash
# Kiln.app 번들을 만든다. 사용: scripts/bundle-macos.sh [--install]
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --release -p kiln
python3 scripts/fetch-sparkle.py
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
if [ "$(target/release/kiln --version)" != "kiln $VERSION" ]; then
  echo 'Refusing to bundle a test or mismatched executable.' >&2; exit 1
fi
APP=target/release/Kiln.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/kiln "$APP/Contents/MacOS/kiln"
if [ -f assets/Kiln.icns ]; then cp assets/Kiln.icns "$APP/Contents/Resources/Kiln.icns"; fi
cp assets/KilnStatusTemplate.png "$APP/Contents/Resources/KilnStatusTemplate.png"
python3 scripts/collect-licenses.py "$APP/Contents/Resources/THIRD_PARTY_LICENSES.txt"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kiln</string>
  <key>CFBundleDisplayName</key><string>Kiln</string>
  <key>CFBundleIdentifier</key><string>dev.kiln.app</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>kiln</string>
  <key>CFBundleIconFile</key><string>Kiln</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
mkdir -p "$APP/Contents/Frameworks"
ditto target/vendor/sparkle/Sparkle.framework "$APP/Contents/Frameworks/Sparkle.framework"
python3 - "$APP/Contents/Info.plist" <<'PY'
import json, plistlib, sys
from pathlib import Path
config = json.loads(Path('scripts/release-config.json').read_text())
path = Path(sys.argv[1])
info = plistlib.loads(path.read_bytes())
info.update(SUFeedURL=config['feed_url'], SUPublicEDKey=config['sparkle_public_key'],
            SUEnableAutomaticChecks=True, SUAutomaticallyUpdate=False,
            SUEnableSystemProfiling=False, SUFeedURLOverrideAllowed=False)
path.write_bytes(plistlib.dumps(info))
PY
# The menu companion has its own bundle identity so Finder/Dock reopens the GUI,
# not a windowless companion process. It ships inside the one installed app.
HELPER="$APP/Contents/Library/Kiln Status.app"
mkdir -p "$HELPER/Contents/MacOS"
cp target/release/kiln "$HELPER/Contents/MacOS/kiln-status"
cat > "$HELPER/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Kiln Status</string>
<key>CFBundleIdentifier</key><string>dev.kiln.statusbar</string>
<key>CFBundleExecutable</key><string>kiln-status</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>${VERSION}</string>
<key>LSUIElement</key><true/>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
# Local builds use ad-hoc signing. Release builds must provide a Developer ID.
SIGN_IDENTITY="${KILN_SIGN_IDENTITY:--}"
if [ "$SIGN_IDENTITY" = "-" ]; then
  codesign --force --deep --sign - "$APP"
else
  python3 scripts/sign-framework.py "$APP/Contents/Frameworks/Sparkle.framework" "$SIGN_IDENTITY"
  codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$HELPER"
  codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
fi
codesign --verify --deep --strict --verbose=2 "$APP"
if [ "${KILN_NOTARY_PROFILE:-}" != "" ]; then
  if [ "$SIGN_IDENTITY" = "-" ]; then echo "Notarization requires KILN_SIGN_IDENTITY" >&2; exit 1; fi
  ditto -c -k --keepParent "$APP" target/release/Kiln-notarize.zip
  xcrun notarytool submit target/release/Kiln-notarize.zip --keychain-profile "$KILN_NOTARY_PROFILE" --wait --output-format json > target/release/notarization-result.json
  python3 - <<'PY'
import json
from pathlib import Path
result = json.loads(Path('target/release/notarization-result.json').read_text())
if result.get('status') != 'Accepted':
    raise SystemExit('Notarization was not Accepted; inspect the local result and Apple submission log')
print('Notarization Accepted:', result['id'])
PY
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute --verbose=2 "$APP"
fi
echo "built $APP"
if [ "${1:-}" = "--install" ]; then
  # 실행 중인 데몬은 그대로 두고 앱만 교체한다. 새 앱이 켜지면 데몬을 무중단 업그레이드한다.
  rm -rf /Applications/Kiln.app.new
  cp -R "$APP" /Applications/Kiln.app.new
  rm -rf /Applications/Kiln.app
  mv /Applications/Kiln.app.new /Applications/Kiln.app
  mkdir -p "$HOME/.local/bin"
  ln -sf /Applications/Kiln.app/Contents/MacOS/kiln "$HOME/.local/bin/kiln"
  echo "installed /Applications/Kiln.app (CLI: ~/.local/bin/kiln)"
fi
