#!/bin/bash
# Recaptures every screenshot and hero recording the site uses, in all four languages,
# and writes WebP/MP4/WebM files into site/assets. Run setup-demo.sh first.
set -u
HERE=$(cd "$(dirname "$0")" && pwd); SITE=$(cd "$HERE/../.." && pwd)/assets
D=${KILN_DEMO_DIR:-/tmp/kd}; source "$D/env.sh"; cd "$D"
SHOOT="$HERE/shoot.sh"; LANGS="en ko ja zh-CN"
kiln shutdown-daemon >/dev/null 2>&1; rm -f sessions.json; sleep 1
python3 "$HERE/compose.py" 0 - >/dev/null
S=$(python3 -c "import json;print(json.load(open('sessions.json'))['shell'])")
SETTINGS='python3 -c "import time;open(\"'$D'/config/open-settings\",\"w\").write(str(int(time.time()*1000)))"'
NOTIFY="kiln send $S \"clear; kiln notify 'Integration review is ready' 'Codex is waiting for your answer in Acme'; clear\" -e"
GITOUT="kiln send $S 'clear; git --no-pager -C services/api status -sb; git --no-pager -C services/api log --oneline --decorate -6' -e"
for L in $LANGS; do
  "$SHOOT" "$L" 0 - "final/agents-$L"
  "$SHOOT" "$L" 1 - "final/git-$L"
  "$SHOOT" "$L" 2 db "final/db-$L"
  "$SHOOT" "$L" 3 - "final/editor-$L"
  POST_CMD="$GITOUT" POST_WAIT=1.5 "$SHOOT" "$L" 4 explorer "final/files-$L"
  "$SHOOT" "$L" 4 remote "final/remote-$L"
  PRE_CMD="$SETTINGS" "$SHOOT" "$L" 0 - "final/settings-$L"
  POST_CMD="$NOTIFY" POST_WAIT=1.2 "$SHOOT" "$L" 4 - "final/toast-$L"
done
for L in $LANGS; do
  C=$(kiln new --cwd "$D/home/dev/acme" --name "Connect the frontend and API" claude "$D/scenes/rec-claude.txt" | awk '{print $NF}')
  X=$(kiln new --cwd "$D/home/dev/acme" --name "Review the integration tests" codex "$D/scenes/rec-codex.txt" | awk '{print $NF}')
  G=$(kiln new --cwd "$D/home/dev/acme" --name "Draft the release notes" agy "$D/scenes/rec-agy.txt" | awk '{print $NF}')
  python3 -c "import json;s=json.load(open('sessions.json'));s.update(claude=$C,codex=$X,agy=$G);json.dump(s,open('sessions.json','w'))"
  WAIT=8 "$SHOOT" "$L" 0 - "rec-before-$L"
  W=$(./winid "$(cat gui.pid)" | awk '$7=="Kiln"{print $1; exit}')
  ./rec "$W" 18 "$D/shots/hero-$L.mov" & RP=$!
  sleep 1.5; kiln send "$C" "" -e; kiln send "$X" "" -e; kiln send "$G" "" -e; wait $RP
done
kill "$(cat gui.pid)" 2>/dev/null; kiln shutdown-daemon >/dev/null 2>&1
cd shots
for f in final/*.png; do cwebp -quiet -q 84 -resize 2000 0 "$f" -o "$SITE/shots/$(basename "${f%.png}").webp"; done
for L in $LANGS; do
  # Overlays the window controls from the still frame onto the recording.
  ffmpeg -v error -y -i "final/agents-$L.png" -vf "crop=156:66:0:0" "lights-$L.png"
  VF="[0:v][1:v]overlay=0:0,fps=30,scale=1600:-2,tpad=stop_mode=clone:stop_duration=2.5[v]"
  ffmpeg -v error -y -ss 2.3 -i "hero-$L.mov" -i "lights-$L.png" -filter_complex "$VF" -map "[v]" -c:v libx264 -preset slow -crf 24 -pix_fmt yuv420p -movflags +faststart -an "$SITE/hero-loop-$L.mp4"
  ffmpeg -v error -y -ss 2.3 -i "hero-$L.mov" -i "lights-$L.png" -filter_complex "$VF" -map "[v]" -c:v libvpx-vp9 -b:v 0 -crf 36 -row-mt 1 -an "$SITE/hero-loop-$L.webm"
  ffmpeg -v error -y -sseof -0.2 -i "hero-$L.mov" -i "lights-$L.png" -filter_complex "[0:v][1:v]overlay=0:0,scale=2000:-2" -frames:v 1 "poster-$L.png"
  cwebp -quiet -q 84 "poster-$L.png" -o "$SITE/hero-poster-$L.webp"
done
echo "Captured into $SITE"
