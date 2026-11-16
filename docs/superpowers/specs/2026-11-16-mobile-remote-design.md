# Kiln Remote mobile design

## Intent and authorization

Build installed Android and iPhone apps that operate the Mac's Kiln conversations: find work, create an agent task, read streamed messages and real tool results, answer approvals/questions, change advertised provider settings and interrupt work. The owner explicitly authorized autonomous implementation, emulator/simulator UI verification, merge and desktop release without intervention. Physical-device acceptance belongs to the owner later. This is working remote software, not a disconnected visual demo.

Desktop 0.3.0 is already published at `v0.3.0`, merge `561284352e8e2a520a064f7084994df6ec20dc71`, with the same tree as built/reviewed `6e5fa22943bf7325446b0fa94b0a32afb590cf33`. Never overwrite that tag or its assets. Preserve the original installed app, daemon, source checkout, account data and unrelated work.

## Approach: controller's choice

Use one React/TypeScript interface inside Capacitor 8 native iOS/Android projects and an optional Rust host companion using Axum 0.8.9. Separate SwiftUI/Compose apps would duplicate the same conversation and control policy. A browser-only application would not satisfy the installed-app and simulator deliverables. Capacitor core 8.6.2 was observed in the package registry; pin actual compatible package versions and commit lockfiles. The existing Node 26.13.1 and Xcode 27.1 meet the documented tooling floors; successful builds still require actual verification.

The host reuses the published `kiln::app::agent_gui::{Client, Descriptor, Transcript, Command, prepare_with_options, descriptor, import_attachment}` and the existing session daemon. It does not implement another provider adapter, own a second inference lifecycle, copy native authentication, alter global provider settings or scrape CLI screen output to reconstruct GUI events. A separate terminal view may use existing daemon ListSessions/ReadText/Input, scoped to enabled project folders.

Primary references and inspected source:

- [Capacitor environment](https://capacitorjs.com/docs/getting-started/environment-setup), [native HTTP](https://capacitorjs.com/docs/apis/http), [iOS](https://capacitorjs.com/docs/ios), [Android](https://capacitorjs.com/docs/android).
- [Axum](https://docs.rs/axum/0.8.9/axum/); inspect downloaded types/source before implementing middleware and limits.
- `crates/kiln/src/app/agent_gui/mod.rs:37,52,135,170`: prepare, descriptor and immutable image import; `client.rs:192,456,502`: connect, generation-bound send and replay state.
- `model.rs`: actual serialized command/entry/request types; `controls.rs`: advertised options, scope, outcomes, catalogs and capabilities. Transcript and NativeControls require explicit safe serializable projections.
- `crates/kiln-accounts/src/manager.rs:312,319`: use `AccountManager::with_env(Env::real())` to avoid starting unsolicited usage polling. Explicit launch preparation remains native-owned.

## Product and visual design

Use 12ui's inspected candidate C from the current mobile run as the approved design direction: warm light surface, clear sans typography, compact connected-computer summary, readable task rows, restrained status chips and a three-item bottom bar. Expand that actual image into pairing, task list, conversation/approval, provider settings and new-task states; integrate its generated prototype rather than recreating a generic form. Existing Kiln/provider brand assets replace inaccurate generated provider logos. Generated file counts, computer specifications and timing estimates must not become invented live metrics. Show only real state or omit the field.

Support 360–430 CSS-pixel phone widths, portrait safe areas and keyboard resizing. Respect system light/dark appearance, readable contrast, large text, 44-point minimum action targets, reduced motion and labeled controls. The primary flow must work without hover. Korean and English interface copy are required; reuse the existing four-language catalog for shared tool/provider labels where practical and include Japanese/Simplified Chinese for shared controls. Original arguments, model output, questions and diffs remain unchanged and inspectable. Render untrusted model/tool content as escaped text or sanitized Markdown; no raw HTML execution or remote scripts/fonts.

Tasks are real native conversation identities grouped by enabled folder. This is a conversation controller, not a remote desktop/pane-layout mirror. All three providers remain available through the installed native CLI and account. Advertised model, effort, modes, independent Claude Ultracode, native settings scopes, command/skill discovery and pending/Applied/Acknowledged/Unconfirmed states must retain their desktop semantics. The first mobile delivery does not claim every advanced desktop action or terminal-only workflow has a custom mobile form; unsupported mobile actions remain explicit and cannot appear as working buttons.

## Host boundary and authentication

- The companion is manually started, default-off, loopback-only, with repeatable explicit `--workspace PATH`, required `--kiln-cli PATH`, and an ephemeral random port when port 0 is supplied. Reject non-loopback listen addresses. Do not change OS networking, firewall, TLS trust, account access or original app preferences.
- Physical-device access uses an owner-provisioned trusted HTTPS reverse proxy or SSH tunnel to loopback. Provide concrete setup documentation; do not silently expose a plaintext LAN service. Emulators use iOS `localhost` and Android `10.0.2.2`; cleartext exceptions are limited to those development hosts. Reject other HTTP URLs in the app. Preserve certificate validation for HTTPS.
- Generate 32 random bytes of bearer authorization per host launch, valid at most eight hours, using OS randomness. Write connection/pairing information only to an explicitly selected owner-private 0600 file under a 0700 directory; normal logs and API errors must not include the token. No persistent mobile token storage by default. Disconnect clears it. Host exit revokes it.
- Authenticate before reading project/session data or accepting mutation. Validate Host/Origin, permit only the native app origins and explicitly configured development origin, no wildcard credential CORS. Reject mismatched Origin, unexpected content types, oversized/slow bodies, path traversal, symlink-based metadata escapes and folders outside the canonical startup allowlist.
- Body limit 128 KiB for command JSON, five-second request deadline, bounded concurrent requests and bounded snapshots. Text limit 120,000 bytes follows the existing native boundary. Attachment JSON has a separate bounded route for a maximum 20 MiB decoded image; use actual signature/MIME checking and `import_attachment`, never client-supplied filesystem paths. Upload staging is private and removed on all outcomes.
- Mutation has a client-generated request ID. Retain up to 1,024 results for ten minutes per host session. Same ID and same payload returns the original result; a changed payload returns conflict. Cache eviction/expiry is explicit; clients never automatically retry a mutation or resend a prompt after reconnect. Stale generation returns conflict without dispatch. API receipt means queued, never native completion.
- Launch/auth preparation that exceeds a request deadline runs as one bounded owned asynchronous operation. Return an operation receipt and expose authenticated read-only operation status; a timed-out HTTP response must not create an untracked task or launch it again on duplicate ID. Bound concurrent native preparations to four.
- Use actual client replay/generation state and actual transcript envelopes. Keep Stop available during ordinary configuration settlement. Bound replay work and return a visible catching-up state rather than inventing readiness. A reader/writer error preserves readable history and marks delivery unknown.
- Read stored descriptor metadata through a bounded no-follow file read and explicit projection. Never send raw config, provider environment, credentials, headers, auth files or arbitrary history outside the enabled folders. Do not invoke `AccountManager::load()` merely to list work.
- Snapshots are at most 2 MiB with explicit coverage/truncation, at most 200 recent entries and 16 active conversation readers. A request whose complete approval details cannot be safely projected is marked non-respondable on mobile; both UI and API refuse its Allow/Grant rather than asking the owner to approve hidden/truncated input. Older readable history and a computer-review reason remain visible.
- `prepare_with_options` normally derives a companion from current_exe. The host must replace the resulting SpawnSpec program with the explicitly validated Kiln CLI; otherwise it would mistakenly launch itself as agent-gui-host. Validate CLI version at startup and preserve the native generated arguments/environment. No remote parameter selects an executable or arbitrary working directory.

## Versioned API

Implement `mobile/host` as its own Cargo workspace/package `kiln-mobile-host` version 0.1.0, consuming sibling published Kiln source through path dependencies. Do not add it to the desktop crates glob or change the desktop 0.3.0 version. Create `mobile/protocol/v1.md` and TypeScript definitions/representative JSON fixtures as the authoritative interface for the app.

All routes are under `/v1` and authenticated. Define stable JSON errors `{error:{code,message}}` without sensitive native diagnostics.

| Route | Behavior |
| --- | --- |
| `GET /v1/bootstrap` | Host version/name, enabled workspace IDs/display names, installed provider availability, capability coverage. No machine specifications unless measured. |
| `GET /v1/conversations` | Bounded safe descriptor and lifecycle/title summaries within the allowlist; empty is distinct from error. |
| `POST /v1/conversations` | Request ID, workspace ID, provider enum and typed LaunchOptions. Use native prepare/descriptor + actual daemon Create. Exactly one new native conversation for duplicate request IDs. No automatic prompt. |
| `GET /v1/operations/{request_id}` | Authoritative pending/succeeded/failed operation result; authenticated, bounded, no mutation or automatic retry. |
| `GET /v1/conversations/{id}` | Safe snapshot: descriptor, generation, sequence, lifecycle, bounded entries, requests, models/modes/selected values, explicit controls projection, commands/errors, replay readiness and truncation/coverage. |
| `POST /v1/conversations/{id}/commands` | Request ID, generation and existing typed Command. Validate against the selected conversation, advertised controls and current request IDs; generation-bound native send. Return actual queued command ID. |
| `POST /v1/conversations/{id}/attachments` | Request ID, bounded filename and image bytes; return only an actual imported Attachment. No arbitrary path accepted. |
| `GET /v1/terminals` and `GET /v1/terminals/{id}` | Optional native terminal summaries/text for enabled folders, bounded output and explicit exited state. |
| `POST /v1/terminals/{id}/input` | Explicit user text/Enter, request ID, enabled live terminal only; no Create arbitrary program, shutdown or upgrade endpoint. |

The host implementer owns the exact serializable DTO/types/fixtures within these requirements and documents them before app implementation. Subsequent tasks consume those exact interfaces, not guessed field names. Provider controls/actions stay typed: unsupported or unavailable controls are visible with their reason, inherited/managed/terminal scopes read-only. Commands insert a draft and require Send; native skills retain exact name/path metadata. Questions and one-time Allow/Deny do not silently broaden permissions.

## Execution and meaningful verification

1. Host tests exercise real HTTP authentication/origin/body limits, allowlist/no-follow cases, stale-generation rejection, duplicate mutation effects and safe DTO output. An isolated daemon/native-worker integration proves actual journal commands and tool effects; mocked responses alone do not establish remote operation.
2. App tests cover disconnect/reconnect unknown delivery, semantic command payloads, actual pending/native settlement display, read-only scopes, question answers and narrow/safe-area layouts. Avoid implementation-mirroring tests for style values.
3. Build Android APK and iOS Simulator app, install on newly owned emulator/simulator, and inspect actual UI. Each platform connects to the real host with private config/socket/project, creates/opens a native conversation, submits a synthetic task, answers a real MCP approval, sees exact completed result and changes a supported control with actual native settlement. Also verify unauthorized/disconnected state and no automatic replay. Use installed native CLI with an owned loopback inference fixture; label inference scope accurately. No real provider credentials are copied.
4. Native UI/physical Korean IME and real-device/network behavior remain separate evidence; physical devices belong to the owner later. Do not claim all advertised controls/providers were executed because one Claude task passed.
5. Keep source and build artifacts reviewable, run relevant CI, independent scoped reviews and a final mobile review before authorized merge. Preserve the desktop release.
6. Close every owned mobile/desktop QA app and owned listener/worker. Remove owned simulator devices/AVDs, fixtures, upload staging, test projects and downloaded installer/runtime archives after proof consumption. Preserve normal native authentication, original work, SDK/toolchains needed for the owner's later device build, durable verification summaries, final APK/Simulator app and signed desktop release artifacts. Three earlier normal-native Antigravity QA histories retain their documented cleanup boundary; do not bypass native onboarding/storage rules.

## Deliverables and honest boundaries

Deliver merged mobile source, reproducible build/run commands, Android APK, iOS Simulator bundle, local host companion and emulator/simulator proof. No App Store/Play publication, paid cloud devices, physical-device signing or public-network exposure is implied. If a vendor runtime cannot be installed, investigate available safe alternatives and report the exact remaining boundary; compilation alone is not simulator UI acceptance.
