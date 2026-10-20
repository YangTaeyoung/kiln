# Account credits, Codex reset credits and usage history

Approved by the owner on 2026-10-20: show credit figures in the account settings, let
Kiln redeem Codex reset credits through the official CLI on an explicit click, show
Codex's official daily usage, and offer a reset credit when a Codex account hits its
limit with no other account to switch to.

## Sources (installed CLIs, checked locally)

- Claude Code 2.1.304, `GET /api/oauth/usage` (already read by Kiln): `extra_usage` with
  `is_enabled`, `monthly_limit`, `used_credits`, `utilization`, `currency`,
  `disabled_reason`. Amounts are in minor units of `currency` (the CLI's own description:
  cents for USD). No reset credits exist for Claude.
- Codex 0.163.0 app-server, generated with `codex app-server generate-ts` (stable API, no
  experimental opt-in):
  - `account/rateLimits/read` → `rateLimits.credits` (`hasCredits`, `unlimited`,
    `balance` string or null) and `rateLimitResetCredits` (`availableCount`, and with
    details `credits[]` of `id`, `resetType`, `status`, `grantedAt`, `expiresAt`).
    `excludeResetCreditDetails: true` keeps the count and skips the detail lookup.
  - `account/rateLimitResetCredit/consume` `{idempotencyKey, creditId?}` →
    `outcome`: `reset`, `nothingToReset`, `noCredit` or `alreadyRedeemed`.
  - `account/usage/read` → `summary` (`lifetimeTokens`, `peakDailyTokens`,
    `longestRunningTurnSec`, `currentStreakDays`, `longestStreakDays`) and
    `dailyUsageBuckets[]` (`startDate`, `tokens`).

## Behavior

- `Usage` gains optional `extra` (Claude), `credits` and `resets` (Codex), persisted with
  the sample and absent in older settings.
- Background Codex polls keep `excludeResetCreditDetails: true`; a manual refresh (settings
  opened or refresh pressed) reads details for the nearest expiry.
- The account row shows one caption under the email: Claude "추가 사용 $12 / $50" or
  "추가 사용 꺼짐"; Codex "크레딧 N" or "크레딧 무제한" and "초기화권 N개 · M/D 만료".
- Codex account menu: "초기화권 사용" (enabled with an available credit) asks inline, like
  delete, and redeems through the profile's own Codex home after checking the account's
  identity, with one idempotency key per attempt. The outcome is shown as the account's
  notice and usage is read again. Nothing is redeemed automatically.
- Codex account menu: "사용 내역" opens a dialog with the summary and the last 14 daily
  buckets, read on open.
- When a Codex limit is reached and no other account has headroom (auto-switch found none,
  or auto-switch is off and no account qualifies), and the active account's latest sample
  has an available reset credit, the limit toast offers "초기화권 사용"; the click redeems.

## Verification

Unit tests for parsing, the fake app-server (consume outcomes, identity mismatch, history),
manager flows (busy state, refresh after a reset), the toast choice, and UI captions and
dialogs in four languages at narrow widths. Live reads of credits and history need the
owner's authorization; a live redemption spends a real credit and is left to the owner.
