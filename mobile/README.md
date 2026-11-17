# Kiln Remote

Optional manually started companion for installed Android/iPhone clients. The
mobile host is a separate **0.1.0** Cargo workspace; desktop Kiln keeps its own version.
No listener starts with the desktop app. Native conversations, provider login and
inference lifecycle remain owned by Kiln's existing CLI/daemon.

See [delivery, repeatable builds and actual-UI evidence](../docs/mobile-remote.md)
for Android/iOS artifacts, CI coverage, acceptance steps and owner boundaries.

## Build and run the host

```sh
cargo build --locked --manifest-path mobile/host/Cargo.toml
mkdir -m 700 "$HOME/kiln-mobile-pairing"
mobile/host/target/debug/kiln-mobile-host \
  --workspace /absolute/project \
  --kiln-cli /absolute/Kiln.app/Contents/MacOS/kiln \
  --listen 127.0.0.1:0 \
  --pairing-file "$HOME/kiln-mobile-pairing/connection.json"
```

Repeat `--workspace` to enable more folders. The actual CLI must report
the version of the Kiln source linked into the host; its native daemon must already
be running under the same selected configuration/socket. The companion never
upgrades or shuts down that daemon.
`CARGO_TARGET_DIR=/absolute/owned/target` may reuse a build cache. Root desktop
manifests/lock/version are unchanged; this nested workspace retains both reviewed
local IME vendor patches and seeds its lock from the desktop lock.

Read pairing information only into the app's transient pairing form. Treat that
file as a bearer capability; do not log/share it or persist tokens in mobile
storage by default. Host shutdown revokes authorization and removes the matching
pairing file while leaving reusable native workers alive. Ctrl-C/SIGTERM is the
normal shutdown path. A hard kill may leave a stale private pairing file; remove
it before a fresh launch. Do not reuse the old token.

The exact [API contract](protocol/v1.md), [TypeScript types](protocol/v1.ts), and
[inert fixtures](protocol/fixtures/) are the app integration source of truth.

## Emulator connection

Use the selected port from the private pairing file. iOS Simulator reaches
`http://127.0.0.1:PORT` or `http://localhost:PORT`; Android's standard emulator
reaches `http://10.0.2.2:PORT`. Replace only the pairing URL host for Android;
retain that launch's token. Development cleartext exceptions in the app must be
limited to these emulator hosts; other HTTP URLs are rejected. Native Capacitor
HTTP is preferred for native transport. Browser development may supply one exact
`--dev-origin http://localhost:PORT` or `http://127.0.0.1:PORT`.

## Owner-provisioned physical device connection

A phone cannot use the Mac's loopback address directly. The owner later provisions
one trusted access path; this implementation does not change networking, firewall,
TLS trust, router configuration or native account settings.

For an existing trusted HTTPS reverse proxy, terminate TLS with a certificate the
phone already validates. Forward only the chosen companion to `127.0.0.1:PORT`,
preserve Authorization/Origin, disable caching, set body limits to at most 28 MiB
plus 4 KiB, and rewrite upstream Host to `127.0.0.1:PORT`. A concrete nginx location
inside the owner's existing authenticated TLS server is:

```nginx
location /v1/ {
    client_max_body_size 29364224;
    proxy_pass http://127.0.0.1:HOST_PORT;
    proxy_set_header Host 127.0.0.1:HOST_PORT;
    proxy_set_header Authorization $http_authorization;
    proxy_set_header Origin $http_origin;
    proxy_buffering off;
}
```

Replace `HOST_PORT` with this launch's selected port. Pair the app with the trusted
`https://owner-host` URL plus the transient bearer token. Keep normal certificate
validation; do not add a trust bypass. Alternatively an owner-managed SSH client
on the phone can forward its own localhost port to the Mac's loopback endpoint:
`ssh -N -L 127.0.0.1:LOCAL_PORT:127.0.0.1:HOST_PORT owner@mac`. Its forwarding and
local endpoint must remain loopback-only. This command alone exposes plaintext HTTP
and is not a pairable production endpoint: the production app requires trusted
HTTPS even on localhost. If SSH carries the connection, the owner must still
provide a trusted HTTPS endpoint in front of that forwarding path, with normal
certificate validation. Physical-device signing/network and acceptance belong to
the owner, and are separate from simulator evidence. These instructions provision
no network or deployment changes.

## Verification

```sh
CARGO_TARGET_DIR="$PWD/target" cargo test --locked \
  --manifest-path mobile/host/Cargo.toml --test http_boundary
KILN_MOBILE_TEST_CLI="$PWD/target/release/Kiln.app/Contents/MacOS/kiln" \
  CARGO_TARGET_DIR="$PWD/target" cargo test --locked \
  --manifest-path mobile/host/Cargo.toml --test native_roundtrip -- --nocapture
```

The native suite requires an explicitly selected verified owned CLI matching the
host's Kiln source, starts fresh private HOME/config/account/socket/project fixtures,
uses an inert structured
provider fixture with real journal/daemon/PTY/native command/file effects, and owns
its separate cleanup. It does not establish a real provider inference call or a
real installed-CLI MCP call. Those are separate app acceptance gates. No test may
use or shut down the user's normal daemon.

The companion's native storage path is explicitly hardened with retained file and
directory identities. Listing returns bounded real titles/lifecycles and explicit
partial/unavailable summaries without consuming permanent snapshot readers.
Potentially blocking native preparation/daemon calls execute in exact owned local
helper processes, with 30-second/eight-second deadlines and shutdown cancellation.
HTTP timeouts retain actual-work capacity until helper cleanup. Lost mutation
acknowledgements remain unknown under their original request ID. Only helper
process groups are cancelled; native workers/daemon remain reusable. See the
[storage, ownership and final spawn boundary](protocol/v1.md#retained-native-storage-and-work-ownership-review-fix-1).
