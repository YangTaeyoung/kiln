# Agent accounts

Open **Settings → Accounts** to manage Claude Code and Codex subscription
accounts. Install the official CLI for each agent you use.

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

## Codex background-server settings

Codex CLI can connect to its own shared background server. If startup reports
**Background server has incompatible feature settings**, the session’s effective
feature flags differ from those saved by that server. This is separate from
Kiln’s terminal session daemon. Kiln does not override these Codex feature flags.

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
