# Kiln Remote 0.1.0

Kiln Remote is an optional installed Android/iPhone companion. Start its separate
host manually, allow specific workspaces, then pair with that launch's URL and
token. The desktop application and its published **v0.3.0** release stay separate;
there is no App Store/Play Store release in this delivery.

## Trust and interaction

The [host guide](../mobile/README.md) contains exact startup and owner-provisioned
HTTPS/SSH forwarding instructions. Use an already running Kiln 0.3.0 daemon and an
explicit `--kiln-cli` path. The host accepts loopback listeners only and port `0`
chooses an ephemeral port. Repeat `--workspace` for every permitted directory.
The per-launch bearer lasts at most eight hours. Keep its pairing directory 0700
and file 0600; never paste tokens into logs, screenshots, Git or support reports.
Host shutdown revokes pairing without stopping reusable native workers.

The app keeps credentials and drafts in memory. New task creates a native
conversation and retains the prompt as an editable draft; **Send** is a separate
explicit action. Commands insert drafts. Approvals require complete native details;
unsupported or inherited controls remain visible with their scope. A queued request
is not completion. Requested, pending, Applied, Acknowledged and Unconfirmed states
must be read separately. Connection loss never automatically retries a mutation or
replays a prompt. Reconnect within the same app process preserves the unsent draft;
explicit Disconnect clears the connection. Closing the process loses memory-only data.
Terminal input targets an existing allowlisted native session and creates no shell.

The [protocol](../mobile/protocol/v1.md) records request, image, snapshot, timeout,
reader and helper limits. Advanced native actions, sandbox editing and granular live
editors are outside this version's supported controls. Retained path identities and
helper guards harden native access; the final daemon path-to-OS-spawn identity race
remains uneliminated. This is not a claim of race-free execution.

## Reproducible packages

Use Node 22+, JDK 21, Android SDK 36 and Xcode 26+. Dependencies are locked to
Capacitor 8.6.2. See [Capacitor setup](https://capacitorjs.com/docs/getting-started/environment-setup),
[Android command-line tools](https://developer.android.com/tools) and
[Xcode command-line builds](https://developer.apple.com/library/archive/technotes/tn2339/_index.html).
The checked-in Android Gradle wrapper and iOS Swift package lock/version select the
native dependencies; set `JAVA_HOME`, `ANDROID_HOME` and, if needed, `DEVELOPER_DIR`
for the toolchains you already installed. The script installs no SDKs and changes no
machine settings.

Run from the repository root; the output parent must exist and each output directory
must be new. Builds use a temporary private copy of tracked app/protocol inputs,
including current tracked edits, then remove that exact copy on exit. Existing native
projects, SDKs, apps and devices are untouched. `build.json` contains source revision,
tracked-dirty status and artifact SHA-256; it is build evidence only.

```sh
bash mobile/scripts/build-native.sh android development ./android-dev-artifacts
bash mobile/scripts/build-native.sh ios development ./ios-dev-artifacts
bash mobile/scripts/build-native.sh android production ./android-prod-artifacts
bash mobile/scripts/build-native.sh ios production ./ios-prod-artifacts
cargo build --locked --release --manifest-path mobile/host/Cargo.toml
```

Development enables HTTP only for emulator loopback hosts. Production uses the
HTTPS-only web bundle plus Release native network policy. The Android production APK
is **unsigned**; the iOS ZIP contains an **unsigned Simulator app**, not an iPhone
archive. Keep artifacts distinct from desktop release assets. No mobile build replaces
the immutable desktop tag, archive, updater feed or signing identity.

For an explicitly selected owned emulator, install the development APK with
`adb -s SERIAL install PATH_TO_APK`, then launch `dev.kiln.remote` from its launcher.
For an explicitly selected owned iOS Simulator, unzip its artifact and run
`xcrun simctl install SIMULATOR_ID PATH_TO_App.app`, then
`xcrun simctl launch SIMULATOR_ID dev.kiln.remote`. Obtain current targets with
`adb devices -l` / `xcrun simctl list devices available`; never select another person's
active device by assumption. These commands are owner actions, not automatic build steps.
Use the host's selected port and transient bearer: iOS Simulator connects to
`http://127.0.0.1:PORT`, Android's standard emulator to `http://10.0.2.2:PORT`.

For a physical Android phone, open the native project in Android Studio, select an
owner signing configuration and build/install a signed HTTPS production package.
For iPhone, run `npm ci && npm run native:sync` in `mobile/app`, open
`ios/App/App.xcodeproj`, select your development team and physical device, then
build/run **Release** with a valid provisioning profile. Simulator ZIPs cannot be
installed on iPhones. The owner must also provision the trusted HTTPS path described
in the host guide; neither build script exposes a public listener or bypasses TLS.
Physical signing, real-device network reachability and acceptance remain pending.

## Repeatable QA and evidence scopes

```sh
cd mobile/app
npm ci
npm test
npm run build
cd ../..
bash mobile/scripts/verify-host.sh
```

The host script runs on macOS, builds the actual current source CLI, verifies its
0.3.0 version, runs legacy shared Client tests and the HTTP/native suites. Those suites
create private HOME/config/account/socket/workspace fixtures and clean their own
processes. Their provider protocol is **synthetic**; native daemon, PTY, journal and
file effects are real. They do not prove installed Claude/Codex/Antigravity inference,
installed-provider MCP calls or native mobile UI. CI retains existing desktop suites,
adds this host gate to macOS, and separately tests the web client and compiles both
development and production Android/iOS packages. Compile artifacts are not UI PASS.

For actual native UI acceptance, inspect the current rendered UI before each action
on a newly owned emulator and Simulator. Record platform, source, vendor tools,
build mode, exact expected/observed output and inference scope, while keeping device
IDs, tokens, raw captures and local paths in ignored private receipts. Verify:

1. Pairing, unauthorized, offline and reconnect states. A loopback redirect must fail
   closed; a second owned destination must receive zero requests and zero tokens.
2. Open/create a native conversation; explicitly Send a task; allow a real MCP call
   through the installed provider CLI and inspect its exact completed output.
   HTML-like output must remain escaped text. Label synthetic model inference separately.
3. Change one supported native control and observe its actual settlement, then
   disconnect/reconnect with an unsent draft and verify no prompt replay.
4. Send harmless terminal input to the exact private shell and verify its real effect.
5. Check 360–430px layouts, system themes, native pickers, keyboard safe area and
   44-point actions. Synthetic browser screenshots alone do not satisfy this gate.
6. Stop only recorded owned hosts/listeners/workers/apps; delete exact owned devices
   and fixtures after fresh ownership checks. Retain needed SDKs/toolchains and final
   artifacts. Preserve original app/daemon/auth and unrelated native history.

### Dated verification: 2026-11-16

The source-only host/app implementation is
`e94f490a0432fe16d77ab9465ac9cea0a7216bb1`.
This immutable input is distinct from subsequent delivery commits. Desktop source
and capture provenance remain recorded independently in
[site source metadata](../site/_build/source.json).

| Scope | Evidence status |
| --- | --- |
| Host/API and synthetic native-provider regression suites | Prior reviewed producer evidence: 17 HTTP + 8 shared tests; final delivery run recorded separately |
| App behavior | Prior reviewed producer evidence: 33 tests; actual device acceptance is separate |
| Installed Android + iOS conversation and MCP | Actual UI Create → explicit Submit → installed Claude 2.1.325 → real stdio MCP → one-time Allow → exact completed echo, with one effect on each platform. Verified at public implementation `b45beac`; later app changes are scoped separately below |
| Model inference in actual-CLI acceptance | Synthetic loopback inference, explicitly separate from installed CLI/native/MCP execution |
| Final iOS native setting and phone width | At public implementation `e94f490`, actual Effort Level high returned Applied, settled=true and effective high after the UI test (37.3s, PASS). Rendered main width 402px matched the 402px frame after the input/select fix; the earlier overflow was 459.5px |
| Final Android native setting | At `e94f490`, actual UI selected Effort Level high and explicitly requested the change; Native confirmation high was rendered. Independent GET settlement verification remains pending |
| Native redirect refusal | Both platforms refused HTTP 302 at `e94f490`: two requests reached the exact owned redirect listener, zero reached its destination, and zero authorized requests reached that destination |
| Scoped native terminal effects | Both platforms sent explicit UI input + Enter to a real private `/bin/sh` and observed `REMOTE_TERMINAL_VERIFIED` plus exactly one marker effect. Android verified at `b45beac`; iOS at `e94f490` |
| Mobile CI packages | Public delivery `21ca116` passed web and both native jobs, including development/production compilation. At `e94f490`, one web phone-layout test exceeded its 5s test deadline while 35 other tests passed; final CI acceptance remains pending. This test deadline is not evidence of a runtime product failure |
| Remaining actual UI matrix | Native picker, reconnect/draft/no-replay, narrow-width and dark-theme matrix still in progress; individual observed cases above do not complete this gate |
| Physical phones, signing and trusted remote network | Owner acceptance pending |
| Cleanup | Controller-owned final exact-allowlist audit pending; three earlier normal Antigravity history entries retain their native cleanup boundary |

No public mobile tag, Store deployment or physical-device acceptance is implied by
source integration. The MCP proof predates the final 16px phone inputs, settings
priority and terminal fallback changes; it is not relabeled as a repeated final-head
MCP run. Update this table only from sanitized actual receipts.
