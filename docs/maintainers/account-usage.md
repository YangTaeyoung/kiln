# Automatic account usage

The GUI account manager starts a single weak-reference worker from `load()`.
It requests all saved accounts at startup and every five minutes, outside the
render thread. Per-tool in-flight guards prevent overlap. Closing the GUI stops
new checks; it never sends input to, restarts or stops agent panels. Providers
are queried independently; profile mutations use the existing credential lock,
and results from deleted or changed credentials, or from a Codex sign-in that is not
applied yet, are discarded.

A one-second scheduler tick dispatches a tool only when a saved profile (for
Codex, one whose sign-in is applied) reaches its own next deadline and provider backoff has elapsed. This
avoids skipping a later account when earlier requests finish faster than the
previous batch. The five-minute periodic pass still checks changed credentials
and newly saved accounts. Missing credentials postpone that profile's next
deadline by five minutes instead of causing repeated per-second checks.

Latest per-profile runtime credentials take precedence over imported snapshots.
Claude selects the current operating-system user’s Keychain item before a
service-only legacy lookup and private-file fallback. Credential JSON rendered
as hexadecimal by `security -w` is decoded before use; arbitrary passwords are
not decoded. A namespaced legacy `.config.json` is authoritative when it exists;
`.claude.json` remains the default. A locked or inaccessible Keychain is an
unknown local state, not proof that the account logged out.
Backoff is held per profile and credential fingerprint in memory. Successful
values and check timestamps are persisted without secrets. Failure preserves the last successful sample for internal account decisions,
but settings hides failed or expired samples. Opening settings triggers a fresh
request for both providers. While a request is running, each account shows a
spinner instead of cached bars. Repeated frames do not trigger more requests. The refresh action cannot
bypass provider backoff. The scheduler exists only in the GUI, not the daemon.
Test environments never start real provider polling automatically.

Codex local presence checks do not require an email claim and do not mistake
unsupported authentication methods for logout. Quota attribution still requires
the saved account’s matching identity. Legacy direct Keyring namespaces are read
using the official canonical-path key and retained during new launches/resume;
Kiln does not duplicate their refresh tokens into a second file. Ephemeral and
encrypted secrets backends require CLI status verification and are not inferred
from a missing `auth.json`. Managed file-backed namespaces retain their file pin.

## Observed provider contracts

- Codex: a bounded, private `codex --no-daemon app-server` reader uses
  `account/read` and `account/rateLimits/read`. The official CLI owns credential
  decryption and refresh, including encrypted `auth.json` formats. Account email
  is checked before and after reading quotas. The reader does not proactively rotate refresh tokens on each poll. An active
  profile whose runtime login is unavailable can use the default CLI home only
  after the native reader verifies the same saved identity. Hex-rendered JSON
  files are repaired only when their account matches the selected profile.
  No model turn is started and no
  authentication tokens cross the subprocess boundary. The response uses
  `usedPercent`, `windowDurationMins`, and `resetsAt`, as defined by the
  [official account protocol](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/app-server-protocol/src/protocol/v2/account.rs).
  The reader has a 20-second deadline and a one-megabyte response-line bound.
  Failed requests wait five minutes before retrying, including manual refresh.
- Claude: the installed official 2.1.296 CLI's embedded JavaScript reads
  `GET https://api.anthropic.com/api/oauth/usage`, Bearer and
  `anthropic-beta: oauth-2025-04-20` headers. `five_hour` and `seven_day` contain
  nullable `utilization` percentages and ISO `resets_at` strings. Observed code
  offsets: 191129259 (request), 189477254 (headers), 191146000–191147966
  (schema/cache), 190814504 (Retry-After). This is an internal CLI contract,
  not a public stable API. Recheck installed official code before changing it.
  The [official usage guide](https://code.claude.com/docs/en/costs#when-the-usage-request-fails)
  confirms last-known bars and their age after failed requests.

The Claude transport uses HTTPS GET without redirects or request bodies. Tokens are
passed to curl through stdin, never process arguments. Curl configuration files
are disabled; response bodies are bounded while streaming, including older curl
versions and chunked responses. Private temporary headers are removed on return.
Provider response bodies and curl diagnostics are never displayed or logged.
HTTP 429/403 observe bounded Retry-After; 401 pauses one hour. Safe HTTP status
labels distinguish authorization, access and rate-limit errors without showing
provider bodies. Settings never invents a fresh 0% reading from a passed reset time, and hides
failed samples and samples older than five minutes; a failure note says how long ago usage
was last read. Keychain subprocesses have a three-second bound and owned children are reaped.

## Claude login renewal

Claude Code renews its OAuth access token only while a CLI process uses it, so the token of
an idle account expires (about eight hours after its last renewal in observed use). Before a
due usage read, Kiln renews a Claude token that expires within five minutes when it is kept in
a Claude Code credential store, a profile's runtime folder or the live CLI configuration, the
way the official CLI 2.1.304 renews it:

- It takes the CLI's refresh locks: the `.oauth_refresh.lock` folder inside the configuration
  folder and the legacy `<real path>.lock` folder beside it, both made with `mkdir`; a lock
  untouched for 60 seconds is abandoned (the CLI's `stale: 60000`). A held lock postpones the
  read by a minute and is left alone.
- Under the locks it reads the credentials again; a token another process already renewed is
  used as it is.
- It posts the refresh grant (`grant_type`, the stored refresh token, the stored or default
  client ID and the stored scopes) as JSON on curl's stdin to
  `https://platform.claude.com/v1/oauth/token` with a `Kiln/<version>` user agent (the endpoint
  answers curl's default user agent with HTTP 429), without redirects, with a bounded response
  and without showing the response body.
- It saves the new access token, refresh token, expiry and returned scopes only while the
  stored refresh token is still the one it sent, keeping every other field (the CLI's
  compare-and-swap save).
- `invalid_grant` marks the account for sign-in again and stops its requests until its
  credentials change.

Kiln also renews, the same way, a Claude login that only Kiln's store holds: a staged sign-in,
or the saved login of an inactive profile without a runtime folder. The re-read and the
compare-and-swap save use Kiln's credential lock and Kiln's store; no CLI store or runtime folder
is written. The active profile's saved login shares its refresh token with the live CLI and is
not renewed; an expired one is reported without a usage request.

The active profile's runtime login is replaced by Claude Code's own login of the same account
(the live login) when the runtime login is expiring and the live one is not: the CLI in use
renews the live login, and nothing renews a runtime copy that no session uses. Neither login
is renewed by Kiln in that case, and the profile asks for sign-in only when no usable login
remains.

A completed browser sign-in of a Claude profile is read with its staged credentials before
**Apply**; the active profile, runtime folders and the live CLI login stay unchanged until
then. The sign-in clears the profile's previous failure, makes it due with the next scheduler
tick and starts a usage refresh of its tool. A Codex sign-in is read once applied, because
the Codex reader uses the CLI of the Codex home holding the applied login.

Codex renewal stays in its own app-server reader.

## Credits, reset credits and usage history

- Claude samples keep the usage endpoint's `extra_usage` (`is_enabled`, `used_credits`,
  `monthly_limit`, `currency`); amounts are minor units of the currency, as the CLI 2.1.304
  describes them. The account row shows the spend against the monthly limit, or "off".
- Codex samples keep `rateLimits.credits` (`hasCredits`, `unlimited`, `balance`) and
  `rateLimitResetCredits` (`availableCount`, and the nearest `expiresAt` of an available
  credit) from `account/rateLimits/read` in Codex 0.163.0. Background polls send
  `excludeResetCreditDetails: true`, which keeps the count; manual reads (settings opened,
  refresh pressed) read the details. Older saved samples without these fields still load.
- The account row rounds amounts and Codex credit balances to whole units and separates
  thousands with commas ("$12 / $50", "크레딧 28,655").
- A reset credit is redeemed only on a click: the Codex account menu (after an inline
  confirmation), or the limit toast when a Codex limit leaves no other account with room and
  the active account's latest sample lists an available credit. Kiln runs
  `account/rateLimitResetCredit/consume` with a new idempotency key per attempt through the
  private app-server of the account's Codex home, between the identity checks of the usage
  reader, reports `reset`, `nothingToReset`, `noCredit` or `alreadyRedeemed`, and reads the
  account's usage again with details.
- The usage history dialog reads `account/usage/read` when it opens and shows the lifetime
  and peak daily tokens, the current and longest streaks and the last 14 daily buckets.

## Verification boundaries

Use `cargo test -p kiln-accounts --lib`. Isolated fake credentials/probes exercise
Claude login renewal (CLI locks, re-read, grant, compare-and-swap save, rejected and busy
logins, saved logins of inactive and active profiles, staged sign-ins read before Apply), both tools, active and inactive profiles, repeated cycles without settings UI,
shutdown, backoff, stale bars and deletion during a slow response. Real loopback
HTTP fixtures check GET/no body, redirects, response bounds, Retry-After and
private-response redaction. Four-language GPU captures cover the account card
at 420px and 130% scale. Real account checks require explicit owner authorization and the ignored
`live_saved_account_usage_without_persisting_settings` test with
`KILN_ACCOUNTS_REAL_KEYCHAIN=1`, `KILN_VERIFY_TOOL` and
`KILN_VERIFY_PROFILE_INDEX`. It queries quotas without persisting settings,
copying credentials, or starting model turns. The ignored
`live_claude_login_renewal_without_persisting_settings` test renews the selected Claude
profile's login the way the usage worker does and reads its quotas, under the same
authorization and variables.
For Codex, the live usage check also reads the usage history and the reset-credit details.
Redeeming a reset credit spends a real credit; it is verified only by the owner.

Before publishing a usage fix, verify the authorized local accounts with the
actual implementation. Ask the owner to reauthenticate only accounts for which
the correct CLI credential source still fails authentication. Record live
verification separately from fixture results; never record credential values.

The 0.1.22 regression includes two simultaneous Claude runtime accounts with
fresh Keychain tokens and stale files, legacy metadata precedence, Codex optional
identity/Keyring presence, account-anchored reauthentication at 130% scale in all
four languages, and version results before a deliberately held catalog lookup.
