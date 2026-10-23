# Agent accounts

Open **Settings → Accounts** to manage Claude Code and Codex subscription
accounts, and Antigravity Google accounts on macOS. Install the official CLI for each
agent you use. See [Antigravity accounts](antigravity.md#accounts-on-macos) for its
account isolation and platform boundaries.

## Add an account

1. Choose **Add account** on the agent's card.
2. Give the profile a name, such as **Personal** or **Work**.
3. Choose **Sign in with browser** and complete authentication in your browser.

Kiln waits in the background and saves the profile automatically when the CLI
reports successful sign-in. You do not need to open a terminal or run a login
command. Keep the account settings open to see progress, reopen the sign-in
page, or cancel. The attempt expires after five minutes.

Claude Code can display a sign-in code when the browser callback is unavailable.
If it requests one, Kiln shows a masked field where you can paste that code.
Do not paste your password or an API key there.

## Choose which account to use

Signing in saves a profile without replacing the account used by existing CLI
sessions. Choose **Apply** beside a newly signed-in profile to use it for new
sessions. Existing profiles offer **Switch**. Running agent sessions keep running.

If you sign in again to the same account, Kiln updates its saved profile and
offers **Apply** again. The previous live login remains intact until you apply
the new one.

**Save current signed-in account** imports an account you already authenticated
with the official CLI. Profile names can be changed through each row's menu.

## Sign in again without deleting a profile

Open the account row's menu and choose **Sign in again**. Check the displayed
account name and email, then choose **Sign in with browser**. Sign in with that
same account and choose **Apply** after completion. Kiln keeps the profile ID,
name, list order and conversation history; it does not create a replacement
profile or restart a running terminal. Choosing another account in the browser
is rejected without changing the original credentials. Cancelling and failed
attempts leave the original profile intact.

When a supported local credential store confirms credentials are absent, the row shows
**Sign-in required** and a direct **Sign in again** button. Local status is
checked in the background while account settings are open and on refresh. This
is a local credential check, not an online account-validity guarantee. Network
failures and an expired access token alone are not treated as logout: the
installed CLI owns automatic token refresh. If a provider rejects a login while
local credentials remain, use the same row menu to reauthenticate.

New credentials are staged until **Apply** so browser sign-in cannot silently
replace the login used by existing sessions. A running CLI that has cached its
previous login may need to be resumed after applying the repaired login; Kiln
does not interrupt it automatically.

## Automatic usage updates

Kiln checks every saved Claude Code and Codex subscription account in the
background when the app starts, then every five minutes. Keep Kiln open; the
account settings do not need to remain visible. This includes inactive profiles.
The account card shows the last successful check time and retains the last known
bars when a request fails. The refresh button requests another check, subject
to provider backoff. New accounts are picked up by the next cycle.

These are read-only usage requests: Kiln does not send a model prompt or consume
reset credits. Each account uses its latest persistent CLI credentials. Kiln renews
an expiring Claude login the way Claude Code does, including the saved login of an
account you switched away from. A completed Claude browser sign-in is read right
away, before **Apply**; a Codex sign-in is read once applied. Provider
rate limiting delays subsequent checks, including manual checks, according to
Retry-After (bounded to one minute through one day), or five minutes when absent.
A 401 pauses that credential for an hour. A newly refreshed CLI token can be
checked again without waiting for the previous token's backoff. Other request
failures retry on the next scheduled cycle and can be retried manually.

An expired or revoked token can prevent a usage check while a running CLI still
works. The CLI owns token refresh. If the failure persists, use **Sign in again**
in that profile's menu. Network failures alone do not establish logout. Usage
endpoints can change independently of Kiln; an unsupported response is shown as
unavailable instead of inventing quota values. Codex windows are labeled only
when their reported duration matches five hours or seven days.

For implementation references and validation boundaries, see
[automatic usage maintenance](maintainers/account-usage.md).

## Storage and troubleshooting

The official CLI owns the browser flow and the local callback. Each attempt uses
its own temporary configuration and credential namespace. Cancelling or failing
an attempt does not change your existing agent login. Temporary login files and
the attempt's Claude Keychain entry are removed afterwards.

Claude's login process keeps the real home directory so macOS can locate the
login Keychain; only `CLAUDE_CONFIG_DIR` is isolated. Credential and account
metadata reads stay inside the disposable configuration. If the official CLI
falls back to its private credential file, Kiln reads that isolated file first.
A missing or inaccessible Keychain is an error when saving a profile, not a
successful save. Kiln does not reset or replace your Keychain.

Kiln stores saved credential snapshots in the macOS Keychain. Other platforms use
private credential files. Account-list settings contain profile names and
metadata, not access or refresh tokens. Authorization URLs and fallback codes
are not written to account settings or application logs.

If sign-in fails, check the browser and network connection, then retry. A missing
CLI is reported on the account card. Kiln does not implement a separate OAuth
provider or bypass the provider's account policies.

## Keep multiple accounts signed in

New agent panels opened through Kiln use a persistent configuration directory for
each saved account. Claude Code receives `CLAUDE_CONFIG_DIR`; Codex receives
`CODEX_HOME`. Managed Codex account panels explicitly use the private configuration's `auth.json` (mode 0600) so a global keyring preference cannot select another account. The real macOS home directory remains unchanged so Keychain access
continues to work. The CLI owns token refresh in that directory. Kiln bootstraps
it once and does not reinstall an old snapshot on every launch or account switch.

Signing in again replaces that account's runtime credentials only when **Apply**
is chosen. If credentials have been removed from an initialized runtime, Kiln
asks for sign-in instead of restoring an old refresh token. Provider revocation,
organization policies and expired sign-in still require authentication. Claude's
keyless Console sign-in is not isolated by `CLAUDE_CONFIG_DIR`, as described in
[its official multi-account guide](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts).

Existing terminal processes retain their original environment. Open a new agent
panel after switching accounts to use the isolated configuration. A CLI typed
into an ordinary existing shell continues to use that shell's configuration;
Kiln does not replace its credentials or restart it silently.

See [CLI versions and exact conversation restart](cli-updates.md) for update
options and panel identity tracking. To continue a text conversation in the other
provider, enable [conversation synchronization](conversation-sync.md) for an
explicitly connected account pair.

## Codex background-server settings

Codex CLI can connect to its own shared background server. If startup reports
**Background server has incompatible feature settings**, the session’s effective
feature flags differ from those saved by that server. This is separate from
Kiln’s terminal session daemon. New managed Codex panels use `--no-daemon` to
keep their lifecycle independent of shared clients; Kiln does not change the
CLI's feature flags.

**Run without daemon this time** starts this session without restarting the shared
Codex server. The equivalent CLI option is `codex --no-daemon` (check your installed
CLI’s `--help`). **Restart with these settings** changes shared settings and can
interrupt other clients’ active or queued work. Review those clients before
choosing a restart; Kiln does not perform it automatically.

See [Codex startup regression tests](https://github.com/openai/codex/blob/rust-v0.161.0/codex-rs/cli/tests/daemon_startup.rs)
and [feature-setting commands](https://learn.chatgpt.com/docs/developer-commands).

Official references:

- [Claude Code authentication and account isolation](https://code.claude.com/docs/en/authentication#credential-management)
- [Claude Code authentication commands](https://code.claude.com/docs/en/cli-reference#cli-commands)
- [Codex authentication](https://developers.openai.com/codex/auth)
- [Codex CLI browser-login implementation](https://github.com/openai/codex/blob/main/codex-rs/cli/src/login.rs)

Sign-in progress, browser controls, fallback codes and results stay directly
below the account being reauthenticated. New-account sign-in stays with the
add-account controls. Missing email metadata or an unsupported credential store
is not reported as a logout; usage cannot be assigned without matching identity.
