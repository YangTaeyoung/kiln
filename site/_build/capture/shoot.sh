#!/bin/bash
# Captures one Kiln window state. Usage: shoot.sh <lang> <page> <inspector|-> <output-name>
# Optional: PRE_CMD runs before launch, POST_CMD after launch, WAIT/POST_WAIT in seconds.
set -u
D=${KILN_DEMO_DIR:-/tmp/kd}; source "$D/env.sh"; cd "$D"
HERE=$(cd "$(dirname "$0")" && pwd)
[ -f gui.pid ] && kill "$(cat gui.pid)" 2>/dev/null; sleep 1.2
printf '"%s"' "$1" > config/language.json
python3 "$HERE/compose.py" "$2" "$3" >/dev/null
keep=$(python3 -c 'import json;print(" ".join(map(str,json.load(open("sessions.json")).values())))')
for id in $(kiln ls | awk 'NR>1{print $1}'); do case " $keep " in *" $id "*) ;; *) kiln kill "$id" >/dev/null 2>&1;; esac; done
[ -n "${PRE_CMD:-}" ] && eval "$PRE_CMD"
nohup "$KILN_EXE" > gui.log 2>&1 &
echo $! > gui.pid; sleep "${WAIT:-7}"
[ -n "${POST_CMD:-}" ] && { eval "$POST_CMD"; sleep "${POST_WAIT:-2}"; }
W=$(./winid "$(cat gui.pid)" | awk '$7=="Kiln"{print $1; exit}')
screencapture -o -l "$W" "shots/$4.png" && echo "shot $4"
