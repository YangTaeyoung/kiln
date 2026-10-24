---
title: Accounts
description: Save several Claude Code, Codex and Antigravity accounts, sign in through your browser, switch between them and keep usage up to date.
---

Kiln keeps a profile for each agent account you use, so you can move between personal and work subscriptions without logging out. Open **Settings → Accounts** to manage Claude Code and Codex subscription accounts, and Antigravity Google accounts on macOS.

Install the official CLI for each agent first. Kiln uses that CLI to sign in and never implements its own OAuth provider or bypasses a provider's account policies.

## Add an account {#add-an-account}

1. Choose **Add Account** on the agent's card.
2. Give the profile a name, such as **Personal** or **Work**.
3. Choose **Sign in with browser** and complete authentication in your browser.

Kiln waits in the background and saves the profile as soon as the CLI reports a successful sign-in. You do not need to open a terminal or run a login command. While the account settings are open you can follow progress, reopen the sign-in page or cancel. An attempt expires after five minutes.

If the browser callback is unavailable, Claude Code may show a sign-in code instead. Kiln then shows a masked field where you paste that code.

> **Warning:** Paste only the sign-in code into that field. Never enter your password or an API key there.

Already signed in with the official CLI? Choose **Save Current Login** to import that login as a profile.

## Choose which account to use {#choose-which-account-to-use}

Signing in saves a profile without touching the account your existing CLI sessions use.

- Choose **Apply** beside a newly signed-in profile to use it for new sessions.
- Choose **Switch** on an existing profile to make it the active one.
- Rename a profile from its row's menu.

Every saved account keeps its own configuration for the panels Kiln opens, so all of them stay signed in at once. Running agent sessions keep the account they started with. After switching, open a new agent panel to use the new account. A CLI you typed into an ordinary shell keeps using that shell's configuration, and Kiln never restarts it or replaces its credentials.

Kiln does not rotate a running panel to another account when it reaches a usage limit. Choose another account here and open a new panel instead.

## Sign in again {#sign-in-again}

Use this when a profile's login needs repairing. You keep the profile instead of deleting and recreating it.

1. Open the account row's menu and choose **Sign in again**.
2. Check the account name and email shown, then choose **Sign in with browser**.
3. Sign in with the same account and choose **Apply** when it completes.

Kiln keeps the profile's name, its place in the list and its conversation history, and does not restart a running terminal. If you pick a different account in the browser, the attempt is rejected. Cancelled and failed attempts also leave the profile intact.

New credentials wait until you choose **Apply**, so a browser sign-in never silently replaces the login your sessions use. A running CLI that cached its previous login may need to be resumed after you apply the new one.

### Sign-in required {#sign-in-required}

When the local credential store confirms that a profile's credentials are gone, its row shows **Login Required** with a **Sign in again** button. Kiln checks this in the background while account settings are open and when you refresh.

This is a local check, not proof that the account is valid online. Network failures and an expired access token are not treated as a logout, because the CLI refreshes tokens itself. If a provider rejects a login while local credentials still exist, use **Sign in again** from the row menu.

## Usage updates {#usage-updates}

Kiln checks every saved Claude Code and Codex account in the background when the app starts and then every five minutes, including inactive profiles. Keep Kiln open. The account settings do not need to stay visible.

- Each card shows the time of the last successful check and keeps the last known bars when a check fails.
- The refresh button requests another check, subject to the provider's rate limits.
- Codex usage windows are labeled only when their reported length matches five hours or seven days.

These checks are read-only. Kiln does not send a model prompt or use reset credits.

| Situation | What Kiln does |
| --- | --- |
| Provider rate limiting | Waits as long as the provider asks (between one minute and one day), or five minutes when it does not say. This also delays manual checks. |
| Rejected credential (401) | Pauses checks for that credential for an hour. A newly refreshed token can be checked without waiting. |
| Other failures | Tries again on the next cycle. You can also retry manually. |
| Unrecognized response | Shows usage as unavailable instead of inventing values. |

An expired or revoked token can block usage checks while a running CLI still works. If the failure persists, choose **Sign in again** in that profile's menu.

## Antigravity accounts {#antigravity-accounts}

> **Note:** Antigravity accounts are new in Kiln 0.2.12 and available on macOS only. On Windows, sign in by running `agy` in a terminal.

Save several Google accounts for the Antigravity CLI (`agy`) and choose which one new tasks use.

1. Open **Settings → Accounts → Antigravity** and choose **Add Account**. Kiln starts the installed CLI's Google sign-in in a temporary private terminal.
2. Complete the login in your browser. If it displays a code, paste it into **Sign-in code** and choose **Confirm** or press Enter. Whitespace introduced by copied line wrapping is removed. Kiln shows **Verifying the sign-in code** and prevents duplicate submissions while checking it. A rejected or expired code ends the attempt with an error; restart sign-in to obtain a new code. After authentication succeeds, choose **Apply**.
3. Repeat for another account, then choose **Switch** to use it for new tasks.

Running tasks keep their original account. The row menu offers rename, delete and sign in again with the same Google account. **Save Current Login** imports the ordinary CLI's Google login.

Each account has its own private home and macOS keychain, which also keep its Antigravity preferences, settings and conversations separate. Kiln never replaces the ordinary CLI's login or changes your default keychain. The account home does not include files from your usual home, such as shell, Git or SSH configuration. Your workspace files remain available.

Applied accounts show used percentages and reset times for each model group reported by the official CLI. Usage is checked on startup and every five minutes; you can also refresh manually. These read-only checks require agy 1.1.11 or later and do not start an agent turn or spend model quota. Stale or failed checks hide the percentages. Automatic rotation is not included. When `agy` reaches its quota, Kiln shows the reset time in a notification. Sign-in uses Google OAuth; Google Cloud project setup is not automated.

New Kiln zsh terminals also use the selected account when you type `agy`, preserving the arguments and current folder. Open a new terminal after updating. With a custom shell, choose **New Task → Antigravity**. Kiln keeps its own shell integration active when Kiro is installed, so its selected-account routing also works in those new terminals.

Conversations from these accounts take part in conversation sync, and restarting in the same conversation keeps the original account. See [Conversations and updates](conversations.html).

## Where credentials are stored {#where-credentials-are-stored}

On macOS, Kiln stores saved credentials in the Keychain and never resets or replaces it. On Windows, saved credentials are private files protected with your Windows user key. The account list contains only profile names and metadata, never access or refresh tokens. Sign-in links and codes are not written to settings or logs.

## Troubleshooting {#troubleshooting}

- **Sign-in fails:** check your browser and network connection, then try again.
- **CLI not found:** a missing CLI is reported on the account card. Install the official CLI for that agent.

To update the CLIs themselves, see [Conversations and updates](conversations.html#cli-updates).
