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
cargo test -p kiln-remote --lib --test transfers -- --test-threads=1
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

## Final UI review candidate

The current UI harness generates 16 file-list captures and 48 connection-form
captures: four languages, light/dark themes, 420/980-point viewports, 130% scale,
and all three connection types for forms. Run the default command above
serially because these harnesses change the process-global theme and language.
The synthetic captures are written only under `/tmp/kiln-remote-captures`.
The final 22-test library/UI run regenerated all 64 captures after the last UI
changes. Designer and developer-workflow source/render review passed. The six
protocol checks were run before these UI-only changes; production accounts and
manual installed-native interaction remain separate boundaries.

Review `files-ko-kiln-dark-420.png`, `files-ja-kiln-light-420.png`,
`form-ko-kiln-dark-420-0.png`, `form-zh-CN-kiln-light-420-1.png`, and a 980-point
form alongside their compact counterparts. Inspect text/size separation,
scrolling form bodies with retained footer controls, advanced S3 disclosure,
and explicit credential clearing. Then inspect the actual installed app with
isolated configuration and synthetic storage. Native-installed remote UI and
a real cloud account remain separate verification boundaries.
