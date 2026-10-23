# Antigravity account integration

The macOS account provider uses the installed `agy` CLI for Google OAuth, a disposable
PTY for onboarding, a private HOME/keychain per saved profile, and Google OpenID
Connect UserInfo for identity matching. It does not implement a separate OAuth client.

## Evidence and contracts

- The official [CLI authentication reference](https://github.com/google-antigravity/antigravity-cli#authentication)
  describes system-keyring authentication and browser sign-in.
- The [go-keyring macOS provider](https://github.com/zalando/go-keyring/blob/master/keyring_darwin.go)
  invokes `/usr/bin/security` and wraps secrets with `go-keyring-base64:`.
- The installed agy 1.4.6 binary and a disposable-home launch established the CLI's
  `gemini` / `antigravity` keyring slot, token object, TTY requirement and Google OAuth
  selector. `ANTIGRAVITY_APP_DATA_DIR` and `ANTIGRAVITY_GEMINI_DIR` redirected generated
  files into the disposable home. No model prompt was sent.
- [Google UserInfo](https://developers.google.com/identity/openid-connect/openid-connect#obtaininguserprofileinformation)
  identifies the imported or newly authenticated account; a verified email is required.
- A dedicated `kiln.keychain-db` is selected only within the private HOME. Do not use
  `login.keychain-db`: isolated native tests found that name acquired a different
  password even immediately after `security create-keychain`. The dedicated name
  supports creation, unlock, implicit CLI lookup, explicit reads and deletion.
- Both the ordinary default keychain and its search list are compared before and after
  the opt-in native regression. Its accounts contain only synthetic credentials.

## Verification

```sh
cargo check --locked -p kiln
cargo test --locked -p kiln-accounts --lib -- --test-threads=1
KILN_AGY_KEYCHAIN_TEST=1 cargo test --locked -p kiln-accounts \
  native_keychains_do_not_change_default_or_cross_accounts -- --ignored --test-threads=1
cargo test --locked -p kiln --lib agy_ -- --test-threads=1
cargo test --locked -p kiln --lib exact_resume_preserves_account_and_cwd -- --test-threads=1
cargo test --locked -p kiln --lib antigravity_account_roots -- --test-threads=1
```

Fixtures cover PTY login completion, cancellation, timeout, account persistence, staged
reauthentication, wrong-account rejection, separate token refresh state, logout without
snapshot resurrection, account-specific restart and read-only discovery. UI checks cover
add, switch and reauthentication controls at 420/760-pixel widths, 1.3 scaling and all four
languages. Captures stay in the temporary directory.

A real browser authorization with two Google accounts, a live UserInfo response and
account-specific model execution remain interactive acceptance checks. Synthetic
credentials and green tests do not establish those outcomes. No release or installed-app
replacement is part of this change.

## Boundaries

Linux/Windows account management, Google Cloud onboarding, quota polling and automatic
cross-account conversation rotation are not enabled. Agy's ordinary unregistered mode
continues to work. Private homes isolate global CLI preferences and home-relative tooling
configuration; workspace files remain available. See [user-facing account behavior](../antigravity.md#accounts-on-macos).
