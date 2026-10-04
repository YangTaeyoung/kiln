# First public release handoff

Recorded on **2026-10-04**. This is a checkpoint, not proof that publication is complete.
Recheck live GitHub and local state before resuming. Follow the [release runbook](releases.md).

## User-authorized scope

Publish Kiln as `YangTaeyoung/kiln`, public, MIT; provide a notarized macOS build,
automatic updates, a GitHub Release, concise English README and linked documentation.
The owner explicitly approved creating the Sparkle update-signing key in the local
Keychain. The private key must not be uploaded or written into the repository.

## Ready locally

- GitHub CLI authenticated as `YangTaeyoung`; the target repository did not exist
  at the initial check. No public repository or release was created in this checkpoint.
- Developer ID identity `Taeyoung Yang (G54PSSU8W5)` is present.
- Sparkle 2.10.0 is pinned by SHA-256. Its public update key is recorded in
  `scripts/release-config.json`; the private key is in Keychain account `dev.kiln.app`.
- Updater initialization, manual check and automatic-check preference are implemented.
- The bundle/release scripts cover nested signing, notarization, stapling,
  final archive signing, appcast and checksums.
- MIT LICENSE, English README, synthetic product screenshot, docs, CI and maintainer
  entry points are prepared. Local review logs/captures and credential files are ignored.
- Application unit tests (137), GUI integration tests (28), and the Git suites passed.
- Keychain profile `kiln-notary` now authenticates successfully. The temporary plaintext
  credential assignment was removed after secure Keychain storage; no password was logged.
- The latest production candidate was notarized: submission
  `df9a295c-811f-456f-84e6-814f847d7992`, **Accepted**. The refreshed final ZIP passed
  EdDSA, extracted code-signature, staple and Gatekeeper checks. Its SHA-256 is
  `194550a41c23bd9ec9770ff1991ea18f69d0fd4a4a42e5b115d752508f34951d`.
  It includes direct Git-history actions and the readable workspace task labels.
- Native local fixture 0.0.1 to 0.0.2 updated/relaunched, preserving both shell PIDs
  and replacing the menu companion executable. A fresh native fixture then passed update cancellation: the quit dialog was dismissed
  with Escape, retaining the same GUI PID, visible agent draft, version 0.0.1 and
  shell PID. The native automation connection failed before preserve/relaunch;
  that remaining step is awaiting owner-assisted verification. Earlier fixtures
  were removed; the current isolated fixture remains only for this check.
- Workspace task labels now use task names, folder distinctions and local ordinals instead
  of internal pane IDs. Three focused regressions and a 180px rendered review passed,
  including long folder names with a shared prefix. Design and DX reviewers passed.
- Git detail panels now embed the interactive history directly. Right-click actions,
  range selection, branch filtering and operation review share the full history engine;
  there is no separate edit mode. Inline tests exercised reword with draft preservation,
  cherry-pick, reviewed drop and squash cancellation in temporary repositories.
  Design and DX reviews passed. The notarized build is installed and running.
- `/Applications/Kiln.app` is the retained production installation. The GUI was normally
  closed and relaunched; all three live terminal session IDs and PIDs were preserved.
  The old menu companion was explicitly replaced after detecting that it still mapped
  the old executable and icon. GUI, daemon and companion now map the installed bundle.
  Duplicate build apps and orphan GUI-test daemons were removed. The current signed
  distribution ZIP remains in `target/distribution/0.1.0/`; recreate the build app from
  the archive or rebuild before preparing any future updater fixture.
- Relative documentation links and release-script syntax passed validation. A credential
  pattern scan found no matches in 413 historical text blobs. This is a limited scan,
  not a substitute for reviewing private content and the final staged source.
- No release commit or tag has been created. The existing working tree contains the
  accumulated application improvements; preserve them when resuming.

## Required to finish

1. Recheck `notarytool history --keychain-profile kiln-notary` before submitting.
   Credential setup initially returned an agreement-related 403 despite active
   agreements; storing securely without pre-validation followed by a real history
   request succeeded. Do not assume an account error is fixed without that request.
2. Complete native Sparkle update/relaunch verification. The production updater
   intentionally skips processes with `KILN_CONFIG_DIR` or `KILN_SOCKET`; ordinary
   isolated GUI tests therefore do not exercise Sparkle. Use a dedicated macOS test
   account or the [isolated updater harness](updater-validation.md), not the real workspace.
   Verify unsaved-work cancellation/retry, helper replacement and session/PID survival.
3. Build the committed release source with license notices, notarize, staple and
   verify its final archive before publication.
4. Audit the staged files and Git history for private data, commit the reviewed source,
   then create the public repository and publish the release and update feed.
5. Verify downloaded release bytes and the installed updater, then update this handoff
   with the actual release URL, commit/tag and successful verification evidence.

The installed Kiln now matches this notarized candidate; it has not been published.
Do not report automatic updates, notarization, GitHub publication, or downloaded-build
Gatekeeper acceptance as end-to-end verified until those steps are actually complete.
