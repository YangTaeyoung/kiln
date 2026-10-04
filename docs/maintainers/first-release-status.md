# First public release verification

Recorded on **2026-10-04**. Recheck live GitHub before making another release.
Follow the [release runbook](releases.md); future publication still needs the owner's authorization.

## Source and distribution

- Public repository: [YangTaeyoung/kiln](https://github.com/YangTaeyoung/kiln), MIT.
- Release tag: `v0.1.0`, source commit `9a328f79258e2a8f268713e11e68b32d22e00c30`.
- The macOS Apple Silicon archive includes the direct Git-history actions,
  readable workspace task names and clearer quit-dialog wording.
- Notarization submission `b53de846-3e9c-4edb-95e9-bcf6f40a40b4`: **Accepted**.
- Final ZIP SHA-256: `8e624f2c22f86f40a12151a08901f93502aaf2dbb9f1ba972b61a6d7d85ee091`.
- Archive EdDSA verification, extracted deep code-signature verification,
  stapling and Gatekeeper assessment passed.
- [Kiln 0.1.0](https://github.com/YangTaeyoung/kiln/releases/tag/v0.1.0) is published
  as the latest stable release. Downloaded bytes match the local archive; EdDSA,
  code signing, stapling and Gatekeeper checks passed again. Anonymous access to
  the ZIP and the stable update feed passed after publication. The feed bytes
  match the signed release metadata exactly.

## Verification performed

- [Remote CI](https://github.com/YangTaeyoung/kiln/actions/runs/37207407794) passed
  on `2e94fd077bda08b4e5ef5ec6f974ea5f20d933c7`: compile, application units, Git
  integrations, GUI/session integration and release-script validation. This commit
  differs from the release tag only in the test fixture and this handoff.
- Application unit tests: 137 passed. GUI integration: 28 passed, one intentionally
  ignored documentation-image generator. Git suites: 135 passed.
- The first remote CI run passed application unit tests but exposed a test fixture
  dependency on the host Git default branch. The fixture now explicitly initializes
  its remote with `main`; all Git suites passed locally with `init.defaultBranch=master`.
  This test-only correction does not change the signed release executable.
- The quit-copy change additionally passed the agent-workflow and recovery-navigation
  integrations and the minimum-window confirmation test at 130% scale.
- Independent design and developer-workflow reviews passed. Git documentation was
  corrected to distinguish reviewed squash/drop/cherry-pick protection from other actions.
- Native Sparkle fixture: 0.0.1 offered 0.0.2; cancellation retained the same GUI,
  visible agent request, installed version and shell PID. A subsequent owner-assisted
  preserve-and-exit completed installation and relaunched 0.0.2. The owner's screenshot
  showed the request restored. The original shell PID survived and accepted input again.
- An earlier native fixture verified clean update/relaunch and menu-companion executable
  replacement. Save-failure handling was verified with the GUI harness; it was **not**
  exercised through the live AppKit/Sparkle termination loop.
- Native UI automation later lost its connection. Do not present the final installed
  app's live update-menu result as verified: the published feed was verified over
  HTTP, but the official app's post-publication menu result remains unobserved.
- Final notarized app installed at `/Applications/Kiln.app` after normal GUI exit.
  Daemon replacement preserved all session IDs and shell PIDs in the before/after snapshot.
  The updater fixtures, duplicate build/extracted apps and previous installed backup
  were removed. Only the latest production app bundle remains installed. A final
  process check confirmed the GUI and menu companion running from that bundle,
  with the original session IDs and shell PIDs still intact.

## Public-source audit

Current source and 413 historical text blobs were scanned for credential patterns
and private paths. Reachable image history was inspected using OCR and visual review
(282 PNGs); suspicious matches were checked against fixture code. The README image
uses synthetic Acme data. No blocking private content was found in that review.
This is evidence of the performed audit, not a guarantee of absence of all secrets.
Unused brand-review exports and local captures are excluded from source.

## Signing continuity

Public signing identifiers, pinned Sparkle version and public update key live in
`scripts/release-config.json`. The existing Keychain signing key and `kiln-notary`
profile are working. Never export or rotate the private key for a routine release.
See [signing setup](signing.md) and [native updater validation](updater-validation.md).
