# Automatic account usage

The GUI account manager starts a single weak-reference worker from `load()`.
It requests all saved accounts at startup and every five minutes, outside the
render thread. Per-tool in-flight guards prevent overlap. Closing the GUI stops
new checks; it never sends input to, restarts or stops agent panels. Providers
are queried independently; profile mutations use the existing credential lock,
and results from deleted, staged or changed credentials are discarded.

A one-second scheduler tick dispatches a tool only when a saved, unstaged
profile reaches its own next deadline and provider backoff has elapsed. This
avoids skipping a later account when earlier requests finish faster than the
previous batch. The five-minute periodic pass still checks changed credentials
and newly saved accounts. Missing credentials postpone that profile's next
deadline by five minutes instead of causing repeated per-second checks.

Latest per-profile runtime credentials take precedence over imported snapshots.
Claude follows the official CLI’s Keychain-first selection and private-file
fallback. A namespaced legacy `.config.json` is authoritative when it exists;
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
  is checked before and after reading quotas. No model turn is started and no
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
failed samples and samples older than five minutes. Keychain
subprocesses have a three-second bound and owned children are reaped. Token refresh
remains the responsibility of each official CLI.

## Verification boundaries

Use `cargo test -p kiln-accounts --lib`. Isolated fake credentials/probes exercise
both tools, active and inactive profiles, repeated cycles without settings UI,
shutdown, backoff, stale bars and deletion during a slow response. Real loopback
HTTP fixtures check GET/no body, redirects, response bounds, Retry-After and
private-response redaction. Four-language GPU captures cover the account card
at 420px and 130% scale. Do not query real accounts to make a test pass.

Real provider responses and expired-token refresh must be observed by the owner
without exposing credentials. Fixture success is not live-provider validity.

The 0.1.22 regression includes two simultaneous Claude runtime accounts with
fresh Keychain tokens and stale files, legacy metadata precedence, Codex optional
identity/Keyring presence, account-anchored reauthentication at 130% scale in all
four languages, and version results before a deliberately held catalog lookup.
