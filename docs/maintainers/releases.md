# Release runbook

This is the entry point for any maintainer or agent preparing a Kiln release.
Read [signing setup](signing.md) first. Do not confuse local builds with notarized
public distribution. The current release target is **macOS Apple Silicon**.
Use Python 3.11+, stable Rust, Xcode command-line tools and an authenticated `gh`.
Signing keys and notarization credentials must already be available on the build Mac;
the repository's CI validates source but does not hold credentials or publish releases.

## 1. Establish the source and version

Check the active user authorization, GitHub account (`gh api user --jq .login`),
remote, branch, and working tree. Refresh the authoritative remote branch without
resetting or stashing unrelated work. Commit the reviewed release source.

Check the installed `CFBundleVersion` before choosing the public version,
including any unpublished review candidate. The public bundle and appcast
versions must be higher if the owner will update through Sparkle. Do not publish
the same version as an installed review build and expect it to be offered.

Use a new SemVer in the root `Cargo.toml` `[workspace.package]`. All crates inherit
it. Update `Cargo.lock`, add concise English release notes under `docs/releases/`,
and verify the intended version does not already exist as a published tag/release.
Use a stable numeric version such as `0.1.1` for this channel; prerelease delivery
needs a separate feed and must not replace the stable appcast.

Before the first public push, inspect both the files to be committed and existing
Git history for credentials, private paths, real terminal captures and internal
documents. `.gitignore` does not remove anything from existing commits. Inspect
`git diff --cached --stat` and `git diff --cached` after staging reviewed files;
do not blindly stage the working tree or publish local review artifacts.

```sh
cargo check -p kiln
cargo test -p kiln --lib
cargo test -p kiln --test gui
cargo test -p kiln-git
git diff --check
```

For updater changes, also verify draft cancellation, save failure handling, actual
update/relaunch, and unchanged session IDs and shell PIDs in an isolated test setup.
The normal application uses the persistent daemon; never stop it to refresh the GUI.
Use the [native validation guide](updater-validation.md) for isolated Sparkle fixtures.

## 2. Build, notarize, and sign

```sh
export KILN_SIGN_IDENTITY='Developer ID Application: Taeyoung Yang (G54PSSU8W5)'
export KILN_NOTARY_PROFILE='YOUR_EXISTING_KEYCHAIN_PROFILE'
bash scripts/release-macos.sh 0.1.0
```

Replace the example version with the current version. The script checks credentials
and the existing Sparkle public key, builds the app, embeds pinned Sparkle, signs
nested components inside out, submits the app to Apple, staples its ticket, and
assesses Gatekeeper acceptance. It then creates a **fresh final ZIP after stapling**,
signs those bytes with Ed25519, and creates the appcast and SHA-256 checksums.

Outputs are in `target/distribution/VERSION/`:

- `Kiln-VERSION-macos-arm64.zip`
- `appcast.xml`
- `SHA256SUMS.txt`
- `signature.txt` (public signature metadata, not a private key)

The submission ZIP in `target/release/` is not the release archive. Do not upload
an ad-hoc build, an unstapled archive, or any signing credentials.

## 3. Verify the final artifact

Extract the final ZIP into a temporary folder. Run `codesign --verify --deep --strict`,
`xcrun stapler validate`, and `spctl --assess --type execute --verbose=2` on its app.
Verify the archive with Sparkle `sign_update --account dev.kiln.app --verify` using
the public signature recorded in `signature.txt`. Check the appcast URL, length,
version and public key against the exact archive. Smoke-test startup and updater
initialization. Record what was actually exercised.

For example, from the repository root (replace `0.1.0`):

```sh
release_version=0.1.0
release_dir="target/distribution/$release_version"
release_zip="$release_dir/Kiln-$release_version-macos-arm64.zip"
(cd "$release_dir" && shasum -a 256 -c SHA256SUMS.txt)
release_signature=$(python3 -c 'import re,sys; print(re.search(r"edSignature=\"([^\"]+)\"", open(sys.argv[1]).read())[1])' "$release_dir/signature.txt")
target/vendor/sparkle/bin/sign_update --account dev.kiln.app --verify \
  "$release_zip" "$release_signature"
release_check_dir=$(mktemp -d "${TMPDIR:-/tmp}/kiln-release-check.XXXXXX")
ditto -x -k "$release_zip" "$release_check_dir"
codesign --verify --deep --strict "$release_check_dir/Kiln.app"
xcrun stapler validate "$release_check_dir/Kiln.app"
spctl --assess --type execute --verbose=2 "$release_check_dir/Kiln.app"
```

Keep this temporary extraction separate from `/Applications/Kiln.app`. Delete only
the recorded `release_check_dir` after verification. Check appcast metadata against
the final ZIP and its embedded `Info.plist`; passing a signature check alone does
not prove the app starts, updates or preserves running work.

## 4. Publish GitHub Release

Only after the preceding checks pass, create/push the new tag from the reviewed
commit. Await the release-upload process **until it actually exits successfully**;
a tool's running session ID is not completion. Check the complete draft asset
set (ZIP, appcast and checksums), lengths and SHA-256 digests against local files.
If any check fails, stop before publication. Use `set -euo pipefail` for dependent
shell commands or checked subprocess calls, and invoke publication as a separate
step only after inspecting successful verification output. Create a draft release, upload the three distribution files, review it,
then publish it as the latest stable release. Initial repository publication must
likewise wait until the release prerequisites are complete.

For the **first release only**, if the target repository still does not exist,
create it after those prerequisites pass. If it already exists, inspect its owner,
visibility and remote history instead of creating or overwriting anything.

```sh
gh api user --jq .login  # must be YangTaeyoung for the initial publication
gh repo view YangTaeyoung/kiln
# Run only when absence is confirmed (not on an authentication/network error):
gh repo create YangTaeyoung/kiln --public --source=. --remote=origin \
  --description 'A native macOS terminal workspace for AI agents and multi-repository development.'
git push -u origin main
gh repo edit YangTaeyoung/kiln \
  --add-topic macos --add-topic terminal --add-topic rust \
  --add-topic ai-agents --add-topic developer-tools
```

The MIT license comes from the reviewed root `LICENSE`; do not ask GitHub to
initialize a second README or license commit over the existing source history.

```sh
git tag vVERSION
git push origin HEAD vVERSION
gh release create vVERSION --repo YangTaeyoung/kiln --verify-tag --draft \
  --title 'Kiln VERSION' --notes-file docs/releases/VERSION.md \
  target/distribution/VERSION/Kiln-VERSION-macos-arm64.zip \
  target/distribution/VERSION/appcast.xml \
  target/distribution/VERSION/SHA256SUMS.txt
gh release edit vVERSION --repo YangTaeyoung/kiln --draft=false --latest
```

Use real values instead of `VERSION`. Do not upload `target/` wholesale. The public
update feed resolves to `releases/latest/download/appcast.xml`; every stable release
must include it. Prereleases must not become the stable feed.

## 5. Verify publication and install

Download the release assets back from GitHub and verify their checksums, EdDSA
signature, notarization ticket and Gatekeeper result again. Confirm the stable feed
returns the intended version. Open **Check for Updates** in the installed official
build and verify the latest-version/update result.

When replacing a local app, record `kiln ls --json` before and after. Close the GUI
through its save/quit flow; preserve the daemon and PTYs. Verify the helper is current,
the workspace restores and all original shell PIDs remain alive. Remove only your
temporary fixtures and duplicate build bundles, never the active app or credentials.

## Failure and recovery

- **Missing credentials:** stop before publication; ask for the profile name or
  owner-assisted Keychain setup. Continue safe code/docs work.
- **Notarization rejected:** use `notarytool log SUBMISSION_ID --keychain-profile ...`
  to investigate, correct the app, then submit a new artifact. Keep private logs local.
- **Submission still processing:** query `notarytool info`; do not call it Accepted
  because upload succeeded. Staple only after acceptance.
- **Upload interrupted:** resume the draft and compare asset bytes before publishing.
- **Published defect:** make a new higher-version corrective release. Do not overwrite
  an existing release archive or silently rotate the update signing key.
- **Key loss:** block the release and follow Sparkle's documented key rotation/recovery
  policy with the owner; a fresh key is not a compatible replacement by itself.

References: [Sparkle setup](https://sparkle-project.org/documentation/),
[Sparkle programmatic APIs](https://sparkle-project.org/documentation/programmatic-setup/),
[Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).
