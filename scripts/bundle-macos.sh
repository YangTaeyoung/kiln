#!/usr/bin/env bash
# Kiln.app 번들을 만든다. 사용: scripts/bundle-macos.sh [--install]
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p kiln
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
APP=target/release/Kiln.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/kiln "$APP/Contents/MacOS/kiln"
if [ -f assets/Kiln.icns ]; then cp assets/Kiln.icns "$APP/Contents/Resources/Kiln.icns"; fi
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
codesign --force --deep -s - "$APP" >/dev/null 2>&1 || true
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
