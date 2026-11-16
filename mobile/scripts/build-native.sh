#!/usr/bin/env bash
# Build in a fresh private copy: never replace another QA run's native assets.
set -euo pipefail
if [[ $# != 3 || ! "$1" =~ ^(android|ios)$ || ! "$2" =~ ^(development|production)$ ]]; then
  echo 'Usage: build-native.sh android|ios development|production NEW_OUTPUT_DIRECTORY' >&2
  exit 2
fi
platform=$1
mode=$2
repo=$(cd "$(dirname "$0")/../.." && pwd)
output=$3
mkdir "$output" # Refuse to overwrite artifacts from an earlier run.
output=$(cd "$output" && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/kiln-remote-build.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
# Include only tracked app sources, with current working-tree edits; never local auth,
# captures, pairing files, node_modules, native build products or SDK properties.
python3 - "$repo" "$scratch" <<'PY'
import pathlib, shutil, subprocess, sys
repo, scratch = map(pathlib.Path, sys.argv[1:])
files = subprocess.check_output(['git', '-C', str(repo), 'ls-files', '-z',
                                 'mobile/app', 'mobile/protocol']).split(b'\0')
for name in filter(None, files):
    relative = pathlib.Path(name.decode())
    source, target = repo / relative, scratch / relative
    if source.is_symlink():
        raise SystemExit('Refusing symlink in build inputs')
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
PY
cd "$scratch/mobile/app"
npm ci --no-audit --no-fund
if [[ $mode == development ]]; then
  npm run native:dev
  configuration=Debug
else
  npm run native:sync
  configuration=Release
fi
if [[ $platform == android ]]; then
  : "${JAVA_HOME:?Select JDK 21 with JAVA_HOME}"
  : "${ANDROID_HOME:?Select Android SDK 36 with ANDROID_HOME}"
  bash android/gradlew -p android "assemble$configuration" --no-daemon
  if [[ $configuration == Debug ]]; then
    apk=android/app/build/outputs/apk/debug/app-debug.apk
  else
    apk=android/app/build/outputs/apk/release/app-release-unsigned.apk
  fi
  cp "$apk" "$output/kiln-remote-0.1.0-$mode.apk"
else
  xcodebuild -project ios/App/App.xcodeproj -scheme App \
    -configuration "$configuration" -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$scratch/DerivedData" CODE_SIGNING_ALLOWED=NO build
  ditto -c -k --sequesterRsrc --keepParent \
    "$scratch/DerivedData/Build/Products/$configuration-iphonesimulator/App.app" \
    "$output/kiln-remote-0.1.0-$mode-simulator.zip"
fi
python3 - "$repo" "$output" "$platform" "$mode" <<'PY'
import hashlib, json, pathlib, subprocess, sys
repo, output = map(pathlib.Path, sys.argv[1:3])
manifest = {'version': '0.1.0', 'platform': sys.argv[3], 'mode': sys.argv[4],
            'source_commit': subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip(),
            'tracked_source_dirty': bool(subprocess.check_output(['git', '-C', str(repo), 'diff', 'HEAD', '--', 'mobile/app', 'mobile/protocol'])),
            'artifacts': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir() if p.is_file()},
            'evidence': 'build only; no installed app, provider, MCP or physical-device acceptance'}
(output / 'build.json').write_text(json.dumps(manifest, indent=2) + '\n')
PY
echo 'Built artifacts and build.json in the selected output directory.'
