# Automatic account usage

The GUI account manager starts a single weak-reference worker from `load()`.
It requests all saved accounts at startup and every five minutes, outside the
render thread. Per-tool in-flight guards prevent overlap. Closing the GUI stops
new checks; it never sends input to, restarts or stops agent panels. Providers
are queried independently; profile mutations use the existing credential lock,
and results from deleted, staged or changed credentials are discarded.

Latest per-profile runtime credentials take precedence over imported snapshots.
Backoff is held per profile and credential fingerprint in memory. Successful
values and check timestamps are persisted without secrets. Failure preserves
previous bars and their original successful timestamp. The refresh action cannot
bypass provider backoff. The scheduler exists only in the GUI, not the daemon.
Test environments never start real provider polling automatically.

## Observed provider contracts

- Codex: read-only `GET https://chatgpt.com/backend-api/wham/usage`, Bearer and
  `ChatGPT-Account-Id` headers, matching the
  [official 0.162.1 backend client](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/backend-client/src/client/rate_limit_resets.rs#L64).
  The [window conversion](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/backend-client/src/client.rs#L741)
  uses `used_percent`, `limit_window_seconds` and `reset_at`. Kiln does not opt
  into Reserve or consume reset credits.
- Claude: the installed official 2.1.296 CLI's embedded JavaScript reads
  `GET https://api.anthropic.com/api/oauth/usage`, Bearer and
  `anthropic-beta: oauth-2025-04-20` headers. `five_hour` and `seven_day` contain
  nullable `utilization` percentages and ISO `resets_at` strings. Observed code
  offsets: 191129259 (request), 189477254 (headers), 191146000–191147966
  (schema/cache), 190814504 (Retry-After). This is an internal CLI contract,
  not a public stable API. Recheck installed official code before changing it.
  The [official usage guide](https://code.claude.com/docs/en/costs#when-the-usage-request-fails)
  confirms last-known bars and their age after failed requests.

The transport uses HTTPS GET without redirects or request bodies. Tokens are
passed to curl through stdin, never process arguments. Curl configuration files
are disabled; response bodies are bounded while streaming, including older curl
versions and chunked responses. Private temporary headers are removed on return.
Provider response bodies and curl diagnostics are never displayed or logged.
HTTP 429/403 observe bounded Retry-After; 401 pauses one hour. Token refresh
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
