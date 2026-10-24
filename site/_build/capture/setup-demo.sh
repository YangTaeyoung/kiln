#!/bin/bash
# Creates an isolated demo home, repositories, database and scene players for site captures.
# Usage: KILN_EXE=/path/to/Kiln.app/Contents/MacOS/kiln setup-demo.sh
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
D=${KILN_DEMO_DIR:-/tmp/kd}
: "${KILN_EXE:?Set KILN_EXE to the Kiln executable to capture}"
rm -rf "$D"; mkdir -p "$D/home/dev/acme" "$D/bin" "$D/config" "$D/shots/final" "$D/scenes"
H=$D/home

cat > "$H/.zshrc" <<'RC'
export PATH=$KILN_DEMO_DIR/bin:$PATH
PROMPT='%F{blue}%1~%f %F{240}❯%f '
export LANG=en_US.UTF-8
RC
cat > "$H/.gitconfig" <<'GC'
[user]
	name = Mina Park
	email = mina.park@acme.test
[commit]
	gpgsign = false
[tag]
	gpgsign = false
[init]
	defaultBranch = main
GC
mkdir -p "$H/.ssh"; printf 'Host staging-api\n  HostName 10.0.4.12\n  User deploy\nHost build-box\n  HostName build.acme.test\n  User ci\n' > "$H/.ssh/config"
printf '# Acme\napps/web (React), services/api (Go), packages/ui (shared components).\n' > "$H/dev/acme/AGENTS.md"

export GIT_CONFIG_GLOBAL=$H/.gitconfig GIT_CONFIG_NOSYSTEM=1
commit() { # dir, age, author, message, file
  ( cd "$1"; mkdir -p "$(dirname "$5")"; [ -e "$5" ] || echo "// $4" >> "$5"; git add -A
    t="$(date -v-"$2" +%s) +0900"; e="$(echo "$3" | tr 'A-Z ' 'a-z.')@acme.test"
    GIT_AUTHOR_DATE="$t" GIT_COMMITTER_DATE="$t" GIT_AUTHOR_NAME="$3" GIT_AUTHOR_EMAIL="$e" GIT_COMMITTER_NAME="$3" GIT_COMMITTER_EMAIL="$e" git commit -qm "$4" )
}
repo() { mkdir -p "$1"; git -C "$1" init -q; }
A=$H/dev/acme; API=$A/services/api
repo "$API"
commit "$API" 6d "Mina Park" "docs: payment flow sequence" docs/payments.md
commit "$API" 5d "Jun Choi" "fix(api): trim idempotency keys" payment/idempotency.go
commit "$API" 4d "Mina Park" "release: 2.4.0" VERSION
git -C "$API" tag v2.4.0
commit "$API" 3d "Jun Choi" "refactor(api): split payment handler" payment/handler.go
commit "$API" 2d "Mina Park" "test(api): cover duplicate submission" payment/handler_test.go
commit "$API" 30H "Jun Choi" "chore: align error codes with apps/web" payment/errors.go
cp "$HERE/demo/validate.go" "$API/payment/validate.go"
commit "$API" 5H "Mina Park" "feat(payment): validate checkout requests" payment/validate.go
echo >> "$API/payment/validate.go"; commit "$API" 4H "Mina Park" "wip: handle card declines" payment/validate.go
echo >> "$API/payment/validate.go"; commit "$API" 3H "Mina Park" "wip: validation draft" payment/validate.go
cp "$HERE/demo/handler.go" "$API/payment/handler.go"
commit "$API" 40M "Jun Choi" "fix(payment): surface timeout as retryable error" payment/handler.go
cp "$HERE/demo/handler.changed.go" "$API/payment/handler.go"
repo "$A/apps/web"
commit "$A/apps/web" 7d "Jun Choi" "feat(web): checkout page shell" src/checkout.tsx
commit "$A/apps/web" 3d "Mina Park" "feat(web): card form validation" src/card-form.tsx
repo "$A/packages/ui"
commit "$A/packages/ui" 9d "Mina Park" "feat(ui): button and input primitives" src/button.tsx
sqlite3 "$A/acme.db" < "$HERE/demo/acme.sql"

cc -O2 -o "$D/bin/claude" "$HERE/player.c"
for n in codex agy; do cp "$D/bin/claude" "$D/bin/$n"; codesign -f -s - "$D/bin/$n" 2>/dev/null; done
ln -sf "$KILN_EXE" "$D/bin/kiln"
cp "$HERE"/scenes/*.txt "$D/scenes/"
swiftc -O "$HERE/winid.swift" -o "$D/winid" 2>/dev/null
swiftc -O "$HERE/rec.swift" -o "$D/rec" 2>/dev/null
printf '"en"' > "$D/config/language.json"
cat > "$D/env.sh" <<ENV
export KILN_DEMO_DIR=$D KILN_EXE=$KILN_EXE
export HOME=$H ZDOTDIR=$H GIT_CONFIG_GLOBAL=$H/.gitconfig GIT_CONFIG_NOSYSTEM=1
export KILN_CONFIG_DIR=$D/config KILN_SOCKET=$D/d.sock KILN_NO_AUTO_UPGRADE=1 KILN_ACCOUNTS_SANDBOX=1 KILN_DB_NO_KEYCHAIN=1 KILN_REMOTE_NO_KEYCHAIN=1
export SHELL=/bin/zsh PATH=$D/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin
unset ZSH P9K_TTY P9K_SSH
ENV
echo "Demo ready in $D"
