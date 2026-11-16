# Kiln Remote Mobile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver real Android/iPhone remote operation of Kiln conversations with emulator/simulator UI proof and preserved original user work.

**Architecture:** An optional authenticated loopback Rust host uses the existing native Kiln client/reducer and daemon. A shared React/TypeScript app ships in Capacitor native wrappers, consumes a versioned safe API, and retains authoritative native operation states.

**Tech Stack:** Rust/Axum 0.8.9/Tokio; existing Kiln 0.3.0 library/daemon APIs; React/TypeScript/Capacitor 8.6.2; Android SDK and Xcode 27.1 Simulator; 12ui candidate C and its expanded prototype.

**Spec:** `docs/superpowers/specs/2026-11-16-mobile-remote-design.md`

## Global Constraints

- Preserve the original installed app, daemon, source checkout, account data and unrelated work.
- Never overwrite the published desktop `v0.3.0` tag or assets; mobile host/package version is 0.1.0 in its own Cargo workspace.
- The companion is manually started, default-off, loopback-only, with repeatable explicit `--workspace PATH`, required `--kiln-cli PATH`, and an ephemeral random port when port 0 is supplied. Reject non-loopback listen addresses.
- Generate 32 random bytes of bearer authorization per host launch, valid at most eight hours; private 0600 pairing file under 0700 directory; no tokens in logs or persistent mobile storage by default.
- Body limit 128 KiB for command JSON, five-second request deadline, bounded concurrent requests and snapshots; text at most 120,000 bytes; at most 20 MiB decoded image on the separate upload route.
- Mutation IDs retain up to 1,024 results for ten minutes; same ID/payload returns original result, changed payload conflicts; no automatic mutation retry/prompt replay. Stale generation never dispatches.
- Long native preparation has one tracked asynchronous operation, at most four concurrent preparations; snapshots at most 2 MiB/200 entries/16 active readers; incomplete approval details are non-respondable in both UI and API.
- Reuse actual native types/client/envelopes and CLI account preparation. No second inference lifecycle, copied authentication, global provider configuration writes or raw config/environment export.
- Provider requested/pending/Applied/Acknowledged/Unconfirmed states remain distinct. Scope, unsupported and partial-discovery states remain visible. Commands insert drafts; actions require explicit intent.
- Integrate the selected 12ui C expanded prototype; actual state replaces invented metrics and generated brand logos. 360–430 CSS-pixel widths, system theme, keyboard safe area and 44-point action targets.
- Real Android emulator and iOS Simulator UI proof is required within available vendor tooling; physical devices are the owner's later task. Synthetic inference is labeled separately from actual native CLI/tool execution.
- Clean owned QA apps/processes/listeners/devices/fixtures/test data; preserve original/native namespaces and needed SDK/toolchains, final artifacts and durable summaries. Retain explicit earlier cleanup boundaries.

## Review Focus

- An authenticated client submits duplicate requests after a lost HTTP response: one effect, visible unknown delivery, no implicit replay. Task 1 integration and Task 2 client tests.
- A path/config file is replaced by a symlink or a conversation points outside enabled folders: no data exposure or dispatch. Task 1 adversarial metadata tests.
- Native configuration settles during reconnect/backlog: actual generation/outcome controls UI, Stop remains available and incompatible Send stays disabled. Tasks 1 and 2 state tests, Task 3 actual run.
- Model/tool output includes HTML, malformed Markdown or long data: safe rendering, bounded response, explicit truncation, no raw execution. Tasks 1 DTO limits and 2 rendering tests.
- A phone loses connection with an approval/unsent draft and keyboard open: no broadened permission, no lost local draft or auto-resend; controls remain reachable at narrow width. Task 2 tests and Task 3 simulator UI.

---

### Task 1: Authenticated native host and exact API contract

**Files:** Create `mobile/host/Cargo.toml`, `Cargo.lock`, focused `src/{main,server,auth,api,runtime,metadata}.rs`, `tests/{http_boundary,native_roundtrip}.rs`, `mobile/protocol/v1.md`, `mobile/protocol/v1.ts`, `mobile/protocol/fixtures/` and `mobile/README.md` host sections. Add generated-output ignores under `mobile/.gitignore`. Do not change desktop code/dependencies/version.

**Consumes:** Published native `prepare_with_options`, `descriptor`, `Client::connect/drain/send_for_generation/replay_current`, `Transcript::apply`, public Command/LaunchOptions/control types, AccountManager::with_env and daemon Create/ListSessions/ReadText/Input.

**Produces:** `kiln-mobile-host --workspace PATH --kiln-cli PATH --listen 127.0.0.1:0 --pairing-file PATH`; spec routes under `/v1`; exact documented DTO and TypeScript contract/representative fixtures for Task 2. Safe metadata projection and actual generation-bound native commands.

- [ ] Write failing HTTP boundary tests for missing/wrong/expired token, Origin/Host/content-type/body bounds, non-loopback bind rejection, allowlist/no-follow metadata, duplicate ID same/different payload including slow preparation, stale generation, safe DTO/output truncation and refusal to approve incomplete details. Show meaningful RED before production implementation.
- [ ] Implement the authenticated server, native session discovery/reducer, authoritative command receipt/settlement, validated new conversation launch and immutable image upload. Replace prepared SpawnSpec program with the validated explicit Kiln CLI, preserving native args/environment. Use with_env without unsolicited usage polling. Keep filesystem/auth data outside DTOs/errors.
- [ ] Define exact versioned API/TypeScript contract and inert fixtures for app consumers; document the trusted HTTPS/tunnel physical-device path and loopback emulation path. Terminal routes only expose enabled existing terminals and explicit input.
- [ ] Run focused tests plus real private daemon/native-worker roundtrip demonstrating actual effects and duplicate/stale non-effects. Test graceful host shutdown without killing reusable native workers; fixture teardown separately owns their cleanup. Record source/dependency references, exact commands/results/hashes and owned resources.
- [ ] Commit explicit Task 1 files, self-review and provide the report for independent scoped review. No subagents or independent push/merge.

### Task 2: Mobile interface and native application projects

**Files:** Create `mobile/app/package.json`, lockfile, Vite/TypeScript/Capacitor configuration, focused `src/{App,api,state,locales,components,styles}` files, meaningful tests, generated `ios/` and `android/` native projects and build instructions. Use `.gitignore` for SDK-local paths, node_modules, build outputs, pairing data and native derived data.

**Consumes:** Task 1's exact `mobile/protocol/v1.ts`, fixtures and host routes; root's selected 12ui image, expanded prototype, per-state targets and visual brief.

**Produces:** Buildable native apps `dev.kiln.remote` with pairing, real work list, conversation/tool/approval/question UI, composer and images, advertised model/effort/mode/Ultracode/settings, command/skill discovery, new task, disconnect/settings and scoped terminal access. State updates come from the real host, not embedded demo data.

- [ ] Inspect installed Capacitor/native package source/types and generated platform defaults. Write meaningful failing client/state tests for duplicate/unknown delivery and no retry, generation changes, pending versus native Applied, scopes, question payload and unsafe output. Keep visual styling tests out.
- [ ] Integrate the generated 12ui prototype shell and routing baseline with typed host state, system theme/safe areas, accessible labels and useful empty/loading/error states. Preserve local drafts on reconnect. Never fabricate running/completed/connected state. Render original tool data safely and localize known headers.
- [ ] Implement advertised typed settings and catalogs; modes/effort and independent Ultracode must send the actual commands from Task 1/native types. Distinguish unavailable/unsupported/new-conversation/inherited/managed/terminal scopes; no fake feature buttons. Startup options create a separate actual task.
- [ ] Add Capacitor iOS/Android projects, strictly scoped emulator cleartext development exceptions, native HTTP with HTTPS certificate validation retained, and actual image selection/upload. No camera permission or persistent credential storage is required for the primary flow.
- [ ] Run client tests, TypeScript/build checks and platform compile checks. Close visual alignment against every matching original 12ui target after functionality. Commit explicit files and report exact commands/results/hashes/owned outputs for independent review.

### Task 3: Actual dual-platform operation, delivery and cleanup

**Files:** Add focused real-host integration/QA scripts under `mobile/scripts/`, relevant mobile CI workflow, `docs/mobile-remote.md`, four-language site guide/feature updates, dated verification and reproducible build/run instructions. Private screenshots/logs/device IDs and pairing data remain in this plan's ignored workspace. Do not publish credentials or raw user captures.

**Consumes:** Reviewed host/API and native apps, root-owned official SDK/runtime installs, fresh isolated daemon/config/project and real installed provider CLI with synthetic loopback inference. Desktop 0.3.0 public verification receipts remain independent evidence.

**Produces:** Actual Android APK and iOS Simulator app plus host companion artifacts; real UI verification receipts; reviewed source/CI/authorized merged PR; complete exact-owned QA cleanup and final report with physical-device/network limits.

- [ ] Build/install/launch on newly owned Android emulator and iOS Simulator, inspect actual UI before each action, and verify pairing/unauthorized/disconnected states. Use purpose-built simulator tools/CUA according to their current documentation.
- [ ] On each platform, operate the actual host: open/create a native conversation, submit an explicit task, allow a real MCP call, inspect exact completed output, change a supported native control and observe actual settlement. Verify reconnect preserves drafts and never repeats a prompt, and scoped terminal input reaches the actual private shell. Compare actual rendered states at narrow width/system theme; do not count compilation or a consent screen as execution.
- [ ] Record provider/native/inference/source scopes separately. Run appropriate final checks and one whole mobile review, resolve findings through the bounded fix loop, then merge under the owner's existing authorization. Do not add Store publishing or paid cloud devices.
- [ ] Retain reproducible APK/Simulator/host artifacts and concrete physical-device HTTPS/tunnel/build instructions. Physical signing/network/device acceptance stays explicitly pending for the owner.
- [ ] Export rulings/durable sanitized summaries, stop exact owned listeners/workers/apps, remove owned virtual devices/AVDs and QA files/datasets/archives using fresh ownership/process checks, preserve original work/auth/needed SDKs. Reconcile the earlier desktop allowlists and three pending native-history entries honestly, then provide the final report.

## Controller self-review

The three tasks cover host trust/state, installed application behavior and real platform delivery separately. API and fixture ownership is assigned to Task 1 before Task 2 consumes it. The five review-focus conditions map to explicit tests and actual Task 3 operation. Source/dependency facts and native-ready states are inspected, not assumed. The owner opted out of questions and approval pauses; proceed with these controller choices and record their costs in the ledger. Physical devices and public-network provisioning remain concrete later-owner actions, not simulated proof.
