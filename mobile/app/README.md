# Kiln Remote 0.1.0

Native iPhone and Android companion, app ID `dev.kiln.remote`. Requires the separately started Kiln Remote host and its per-launch pairing token. The published desktop app, daemon, accounts and v0.3.0 release are separate.

## Build

For isolated portable artifact builds use [the delivery scripts](../../docs/mobile-remote.md#reproducible-packages).

Node 22+, Xcode 26+, Android SDK 36 and JDK 21 are required by Capacitor 8.6.2. Install dependencies with `npm ci`. Run `npm test` and `npm run build` for client verification. `npm run native:sync` builds the HTTPS-only production bundle and copies it to both native projects. Open `ios/App/App.xcodeproj`, scheme `App`, or build `android` with `JAVA_HOME` pointing to a local JDK 21 and `ANDROID_HOME` to your SDK. No machine-local SDK path is committed.

For emulator/simulator development only, `npm run native:dev` builds a bundle permitting HTTP for `localhost`, `127.0.0.1` and Android's `10.0.2.2`. Android's Debug resource limits cleartext to those names; Release denies cleartext. iOS Debug uses `Info.Debug.plist` for localhost/local networking; Release uses `Info.plist` without ATS exceptions. Certificate validation is never disabled. Before a release, run `npm run native:sync` to replace development assets, then compile Release. Never ship the development bundle.

Build examples:

```sh
npm ci
npm test
npm run native:dev
JAVA_HOME="$(/usr/libexec/java_home -v 21)" ANDROID_HOME="$HOME/Library/Android/sdk" ./android/gradlew -p android assembleDebug --no-daemon
xcodebuild -project ios/App/App.xcodeproj -scheme App -configuration Debug -destination 'generic/platform=iOS Simulator' -derivedDataPath ios/DerivedData CODE_SIGNING_ALLOWED=NO build
```

`npm run icons` regenerates native icon/splash rasters from the existing Kiln SVG. Provider icons preserve `crates/kiln-common/assets/mark-{claude,codex,antigravity}.svg` geometry. Their original repository notices/licenses remain authoritative; no new third-party branding was generated.

## Interaction and evidence

The generated 12ui five-state prototype supplies `p0` Tasks, `p1` Pairing, `p2` Conversation, `p3` Settings and `p4` New task routing, paper/ink tokens, cards and bottom navigation. The original prototype chrome CSS is retained in `src/styles/prototype.css`; responsive composition in `app.css` reflows the source's 1024px canvas to 360–430px and keeps actions at least 44px tall. Functional corrections remove invented hardware, task counts, completed history and provider defaults.

Pairing credentials and drafts are memory-only. Disconnect clears credentials/native state. Every explicit mutation receives one retained request ID and a synchronous duplicate-tap guard. Lost responses, 408 and ambiguous native delivery stay unknown; only GET operations/snapshots are polled. No mutation is automatically retried or prompt replayed. A queued receipt is not completion or Applied. An incomplete approval has no enabled response, including Deny/Cancel. Raw model/tool text is rendered as escaped React text. Redirects are disabled before authenticated browser/native requests.

A new task creates its conversation first; the retained prompt becomes an editable draft that requires a separate Send. Startup options belong only to this new conversation. Commands insert drafts; enabled native skills are explicitly selected for SubmitWithSkills. Unsupported/inherited/managed/terminal controls remain visible and read-only. Existing terminal input is explicit; it creates no process.

`tests/capture-visual.mjs` uses Playwright interception and inert protocol fixtures against the real app. Set `KILN_VISUAL_OUT` and run it while `npm run dev -- --port 5178` is running. Screenshots are synthetic visual proof only, not host/CLI or native-device execution. Actual device/UI acceptance is recorded separately by the controller.
