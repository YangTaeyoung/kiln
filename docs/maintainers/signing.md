# Signing setup

Read the [release runbook](releases.md) before building a public artifact.

## Public identifiers

| Setting | Value |
| --- | --- |
| Repository | `YangTaeyoung/kiln` |
| Bundle ID | `dev.kiln.app` |
| Apple team | `G54PSSU8W5` |
| Developer ID | `Developer ID Application: Taeyoung Yang (G54PSSU8W5)` |
| Sparkle Keychain account | `dev.kiln.app` |
| Update configuration | [`scripts/release-config.json`](../../scripts/release-config.json) |

The Sparkle public key is recorded in the version-controlled configuration. The private key was
created in the maintainer's macOS Keychain with explicit authorization. It is not
in this repository or GitHub Actions. Do not generate a replacement when it is missing.

## Developer ID

```sh
security find-identity -v -p codesigning
```

Use the listed **Developer ID Application** identity. An ad-hoc signature or
Apple Development identity is not the distribution identity. On another Mac, the
owner must securely install the certificate and its private key. Do not put a P12
export or password in the repository, issue comments, chat, or build logs.

## Notarization credentials

The certificate and notarization credentials are separate prerequisites. Ask the
maintainer for the existing `notarytool` Keychain **profile name**, not its password.
The profile name is supplied through `KILN_NOTARY_PROFILE`; it is not hardcoded.
The current maintainer Mac uses `kiln-notary`. Recheck authentication before each
release; this profile is local Keychain state, not part of the checkout.

If no profile exists, the owner can run the following locally and follow the secure
interactive prompts. Use an Apple app-specific password or the supported App Store
Connect API-key flow described in Apple's documentation.

```sh
xcrun notarytool store-credentials kiln-notary --team-id G54PSSU8W5
xcrun notarytool history --keychain-profile kiln-notary
```

Never pass a password as a command-line literal recorded in a shared transcript.
Do not create new account access or signing keys without the owner's authorization.

## Check the existing Sparkle key

```sh
python3 scripts/fetch-sparkle.py
target/vendor/sparkle/bin/generate_keys --account dev.kiln.app -p
```

This prints the **public key only**. It must match `sparkle_public_key` in the
configuration. A missing private key blocks new signed releases until the owner
restores it through a secure channel. Do not export it into this checkout.

References: [Apple notarization](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
and [Sparkle signing](https://sparkle-project.org/documentation/).
