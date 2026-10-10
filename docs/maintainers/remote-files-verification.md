# Remote-file verification

The default `cargo test -p kiln-remote` never connects to user hosts or buckets.
Protocol integration tests are opt-in and only target dedicated loopback
fixtures with synthetic credentials. Keep these fixtures separate from any
production daemon, SSH configuration, keychain or cloud account.

## Fixture contract

- FTP: pyftpdlib on `127.0.0.1:22121`, username `fixture`, password
  `fixture-only`, an empty temporary root and passive ports 23000–23009.
- SFTP: disposable OpenSSH server on `127.0.0.1:22022`, username `fixture`, a
  generated fixture-only Ed25519 key, writable `/files`. Configuration lives at
  `/tmp/kiln-remote-fixture/ssh/config`; its alias is `kiln-remote-fixture` and its
  `UserKnownHostsFile` must contain the fixture server's actual host key.
- S3: Moto S3 emulator on `127.0.0.1:29000`, bucket `kiln-fixture`, region
  `us-east-1`, access key `fixture-access`, secret key `fixture-secret-only`.
  Seed 501 objects under `pagination/item-0000` through `item-0500`.

Run:

```sh
cargo test -p kiln-remote --lib --test transfers --test aws_profile_ui --test ssh_profile_ui -- --test-threads=1
KILN_REMOTE_FIXTURES=1 cargo test -p kiln-remote \
  --test transfers --test s3_pagination -- --ignored --test-threads=1
```

The round trips cover folder creation, Korean/spaces/literal-bracket filenames,
upload, duplicate protection, listing, complete download replacing a local
fixture, rename, stat and nonempty-folder deletion refusal. S3 tests additionally
cover 501-object pagination and a multipart file larger than one upload chunk.
The unit tests cover secret isolation/redaction, cancellation, cache cleanup
without recursive deletion, configured-root navigation, unsupported S3 folder
rename visibility, session-token replacement, batch injection,
host-key-check settings and static SSH config discovery without `Match exec`.

Moto is an S3 protocol emulator, not an AWS production account. Passing these
checks establishes the exercised client/server paths; it does not prove real
IAM policies, every S3-compatible implementation, every SSH server, or installed
native UI behavior. Never replace this distinction with a blanket compatibility
claim.

After verification, stop only the fixture servers/container and remove their
temporary keys, roots and virtual environment. Do not shut down Kiln's real
terminals or session daemon. Keep useful synthetic logs until the review ends.

## Verification performed on 2026-10-06

- Default tests: **22 library/UI tests and 1 integration cancellation test passed**.
  Six protocol tests remain opt-in in the default test invocation.
- Opt-in protocol tests: **6 passed, 0 failed**, against pyftpdlib FTP, an
  isolated atmoz/OpenSSH SFTP container and a Moto S3 emulator. The round trips
  included confirmed replacement of an existing file, not only new-file upload.
- The cancelled multipart test cancelled after a completed upload chunk; no
  destination or staging object remained. Direct emulator inspection confirmed
  **zero unfinished multipart uploads** after the test.
- Synthetic UI tests exercised Korean, English, Japanese and Chinese, light and
  dark themes, compact/wide viewports and 130% scale. These are rendered test
  fixtures, not observations of an installed native app.
- Real FTP/SFTP/S3 protocol fixtures revealed and fixed Unicode copy-header
  encoding, quoted wildcard handling and OpenSSH listing metadata parsing.
- FTPS certificate-validation and stream behavior were checked in the dependency
  source and compile path; an actual trusted-certificate FTPS server was not
  exercised. No production bucket, SSH host, credential or terminal was accessed.

Local evidence logs: `/tmp/kiln-remote-final.log` and
`/tmp/kiln-remote-protocol-fixtures-final4.log`. These logs are machine-local
verification evidence; they are not release artifacts.

## AWS profile and final UI verification

The current UI harness generates 16 file-list captures, 16 saved-connection
captures and 192 connection-form captures: four languages, light/dark themes, 420/980-point viewports and 130%
scale. S3 states include named profile, default authentication, manual keys,
advanced settings after scrolling, validation error, editing, no saved
profiles, a pending save, temporary token input, and the profile dropdown with
an unavailable entry and its reason. FTP and SFTP forms remain covered.
The captures are written only under `/tmp/kiln-remote-captures`; they are not
release artifacts or evidence of native execution.

The tested UI interactions include choosing a different named profile,
preserving an edited region, saving and reopening that identity, cancelling
without changing saved settings, switching authentication modes, revealing
advanced/token fields while retaining footer actions, rejecting an incomplete
manual key replacement without changing saved credentials, and persisting an
explicit default-authentication choice. Legacy connection metadata without
new AWS fields remains supported.

AWS profile tests use private temporary config/credentials files and a fake
AWS CLI. They verify config/profile-section merging, metadata-only discovery,
explicit-profile failure without another-account fallback, environment-pair
precedence, missing/empty profile errors, role environment-source preservation
in an isolated subprocess, bounded output, cancellation, timeout, expired or
malformed credential responses, and cleanup after failed credential processes.
No real AWS account, SSO session, credential process or network was used.

The earlier all-fields S3 form was rejected by the user and its UI verdict
was withdrawn. The replacement source/rendered-flow review passed after
explicit authentication choices, compact hierarchy, preserved footer actions
and reachable expanded fields were verified. This replaces the earlier form
review; it does not certify native discovery or live AWS authorization.

The AWS-authentication serial run passed 33 library/UI tests and one default integration
test, with three transfer fixture tests ignored: see the machine-local
`/tmp/kiln-remote-aws-profile-tests8.log`. Designer and DX source/harness reviews
are separate from installed-app validation. Native asynchronous profile
discovery/import must be checked with synthetic AWS files before publication;
real AWS authorization and SSO login remain unverified boundaries.

Implementation references:

- [AWS shared configuration and credential files](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html)
- [AWS CLI credential export](https://docs.aws.amazon.com/cli/latest/reference/configure/export-credentials.html)
- [AWS role credential sources](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-role.html)

## Provider-identity verification

The remote protocol selector, saved connections and browser heading use the
same provider identity: the unchanged official bundled Amazon S3 architecture
mark, Kiln's generic FTP server vector and its generic SFTP terminal vector.
[Asset provenance and trademark terms](../../crates/kiln-common/assets/REMOTE-MARKS.md)
record the official package, archive member and SHA-256. AWS artwork is excluded
from Kiln's MIT license; the symbols do not imply certification or endorsement.

After adding the marks, the serial default invocation passed **34 library/UI
checks and 1 integration cancellation check**, with 3 loopback transfer tests
intentionally ignored. Evidence: `/tmp/kiln-remote-provider-icons-tests2.log`.
The 16 saved-connection captures cover four languages, both themes, 420/980-point
viewports and 130% scale. The selector fixture clicks FTP, SFTP and S3 through
the accessible radio controls, verifies the actual selected form and cancels
without creating a connection. The existing 192 form and 16 file-list captures
were regenerated with the bundled marks. This is source/rendered-flow evidence;
the installed central-tab path is verified separately by the release owner.

## SSH Config dropdown and explicit edits

`ssh_profile_ui` uses the production asynchronous form with a private config,
an included host file, synthetic key paths and an in-memory store. It checks
host selection, all prefilled fields, edited-port persistence, re-editing,
direct-address connections and the SSH event payload. It does not launch SSH
or SFTP; a `Match exec` marker must never be created. `direct_terminal_launch`
checks the SSH event’s edited options against a fixture executable and real PTY.
Unit tests separately check SFTP argv, conditional-config inheritance, legacy
connections, source changes during discovery, late results and input validation.
These checks do not establish real-host authentication or native installed UI behavior.

## Object-storage providers and connection UI (2026-10-08)

The provider picker follows the protocol-first connection flow in
[Transmit's official server guide](https://help.panic.com/transmit/transmit5/servers/),
whose Quick Connect screenshot was inspected directly. A compact provider
selector replaces an expanding row of tabs. Only the chosen provider's fields
are shown; saving a connection is separate from opening it.
[Cyberduck and Mountain Duck connection profiles](https://docs.cyberduck.io/protocols/profiles/)
and the [dedicated R2 profile](https://docs.cyberduck.io/protocols/s3/cloudflare/)
informed provider defaults and keeping optional endpoint details under Advanced.
These references informed interaction structure, not copied interface artwork.

CLI identity and S3 interoperability keys are separate authentication sources.
The UI retains the exact discovered authentication record, lists unavailable
profiles with a reason, and allows an explicit manual-key or AWS-profile fallback.
New OCI/GCS/R2 native-profile connections require a deliberate profile choice;
discovery does not guess the active CLI account. Native and AWS discovery errors are independent. Refreshing
profiles preserves identity, and explicitly edited region/account fields remain
unchanged. Provider marks and their licenses are recorded in
[REMOTE-MARKS.md](../../crates/kiln-common/assets/REMOTE-MARKS.md).

Official authentication references:

- [Google Cloud Storage authentication](https://docs.cloud.google.com/storage/docs/authentication)
  distinguishes gcloud credentials from Application Default Credentials.
- [Google CLI configurations](https://docs.cloud.google.com/sdk/docs/configurations)
  describes named configurations.
- [Cloudflare S3 authentication](https://developers.cloudflare.com/r2/get-started/s3/)
  requires S3 Access Key ID and Secret Access Key; Wrangler login is not presented
  as a source of S3 keys.

`object_profile_ui` exercises production asynchronous discovery, explicit choice,
save and re-edit for three providers using private config files, a fixture CLI
path and an in-memory credential store. It executes no CLI or cloud operation,
and does not modify HOME. Cloud form render fixtures cover both themes and
compact/wide views at 130%; fixtures are review artifacts, not proof of installed
macOS or real cloud-account operation. See the test run report for execution
status. Existing AWS and SSH profile suites remain required.

Native R2 uploads have a conservative 300,000,000-byte limit, shown at Upload and
validated before queueing; S3-compatible credentials use the existing multipart
backend. Native R2 rename downloads, uploads, then deletes and cannot guarantee
atomicity against concurrent writers. Metadata-bearing objects are rejected
rather than silently losing metadata. Use S3-compatible credentials when these
native API constraints are unsuitable.

## Inspector/editor routing regression

`cargo test -p kiln --test remote_inspector -- --test-threads=1` starts the real
application with isolated configuration, accounts and daemon paths and an owned
loopback S3 HTTP fixture. It exercises sidebar connection/folder navigation,
file-specific tabs, draft checkpoint/credential restoration, text editing and
check-before-save requests to the exact object. It also verifies hidden-inspector
transfer polling, workspace/quit protection and unrelated-panel closure.
The fixture stores only synthetic data and inspects the write count and target.
Dark/light captures remain in `/tmp/kiln-remote-navigation/`, outside the repository.

Remote library/application tests also cover legacy directory-tab migration,
unsaved file-draft restoration, path/connection tab identity, removed navigation
cleanup and endpoint changes before queued file operations. These checks do not
establish live account permissions or native installation behavior.
