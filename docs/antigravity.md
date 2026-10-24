# Antigravity CLI (`agy`)

Kiln runs Google's Antigravity CLI (`agy`) next to Claude Code and Codex. Install it with
Google's installer. On macOS, sign in from **Settings → Accounts → Antigravity**;
on other platforms, sign in by running `agy` in a terminal. Kiln finds it in your
`PATH`, `~/.local/bin`, `/opt/homebrew/bin` or `/usr/local/bin`.

## Start a task

- **New task → Antigravity** starts `agy` in the current workspace. **New task → Open
  in folder → Antigravity** asks for a folder first. The command palette offers
  **New Antigravity task**.
- In a workspace's agent request, choose **Antigravity** below the request. Kiln passes the
  request and the repository summary as agy's initial prompt (`--prompt-interactive`).
- Panels running `agy` show the Antigravity mark in their tab, the workspace task list and
  the sidebar. A command such as `antigravity` opens the Antigravity editor, not the CLI,
  and is not treated as an agent.


New Kiln zsh terminals route a typed `agy` command through the currently selected account,
preserving its arguments and working directory. Open a new terminal after updating Kiln.
Version/help and CLI-update commands go directly to the CLI. Custom shells can use
**New task → Antigravity**. The Kiln helper pins its config directory before applying the
private account home, so launch records stay readable. Integrated zsh launches set `Q_TERM_DISABLED=1` to prevent Kiro from replacing the shell before Kiln installs its account wrapper. User startup files still load; other terminals keep their existing Kiro setup.

## Conversations and restart

agy has no command-line option for lifecycle hooks, so Kiln does not add hooks or change
agy's configuration. While a conversation is open, agy keeps
`~/.gemini/antigravity-cli/presence/<conversation>.lock` open. Kiln reads which lock the
panel's `agy` process holds and records that conversation for the panel, the way a
SessionStart hook does for the other CLIs. A panel resumed with `--conversation` confirms only
its original conversation.

With a recorded conversation, **… → Restart in the same conversation** and **Restart
sessions in the same conversation immediately after updates** reopen the panel with
`agy --conversation <id>`. agy exits on its normal Ctrl-C sequence, as Claude Code and Codex do.
Windows does not let Kiln read another process's open files, so Antigravity panels on
Windows have no recorded conversation and are not restarted automatically.

## CLI updates

**Settings → Accounts → CLI updates** shows the installed `agy` and its version, or **Not
installed** when Kiln does not find it. Updates use agy's own `agy update`. agy checks its published version itself, so Kiln shows no latest
version for it. **Automatically update Codex, Claude Code, and Antigravity** includes agy in
the six-hour checks.

## Usage limits

Applied Antigravity accounts show the quota groups reported by the official CLI in
**Settings → Accounts**. Each group has its own five-hour and/or weekly used percentage
and reset time. Kiln checks on startup and every five minutes; the refresh button also
requests a check. Failed, expired or unrecognized results do not display cached percentages.

Checks use the official read-only `agy -p /usage --output-format json` command in that
account's private home, require agy 1.1.11 or later, and verify the Google identity before
attributing the result to a profile. They do not start an agent turn or spend model quota.
Automatic account rotation is not included.

When agy reports `Individual quota reached … Resets in …`, Kiln also shows a usage-limit
notification with the reset time.

## Accounts on macOS

**Settings → Accounts → Antigravity → Add account** starts the installed CLI's Google
OAuth sign-in in a temporary private terminal. Complete the browser login. If the browser
shows an authorization code, copy it into **Sign-in code** in Kiln and choose **Confirm**
(or press Enter). Whitespace introduced by copied line wrapping is removed. Kiln shows **Verifying the sign-in code** while the CLI checks it and prevents duplicate submissions. A rejected or expired code ends the attempt with an error; restart sign-in to obtain a new code. Once authentication finishes, choose **Apply**. Repeat for another account and choose **Switch** to use it for new tasks.
Running tasks keep their original account. The row menu offers rename, delete and
same-account sign-in again; a different Google identity cannot replace that profile.
**Save current signed-in account** imports the ordinary CLI's Google login. Importing
requires a successful read-only Google UserInfo lookup to identify the account.

Each account has a private home directory and macOS Keychain. Kiln never replaces
the ordinary CLI's credential, changes the user's default keychain, or logs tokens.
Keychain unlock secrets and profile snapshots use Kiln's credential store; account-list
settings contain only metadata. The CLI owns token refresh inside its account home.
An initialized account with missing credentials requires sign-in again instead of
restoring an old token. Login cancellation removes the temporary home and keychain.

Account homes also isolate Agy preferences, global CLI configuration and conversations.
They do not inherit files such as the ordinary home's shell, Git or SSH configuration.
Workspace files remain available. Account conversations participate in Kiln's existing
read-only Agy-to-Claude/Codex synchronization, and exact-conversation restart preserves
the original account home. Ordinary unregistered Agy sessions keep their existing behavior.

This account integration is macOS-only. Linux and Windows retain ordinary CLI sign-in
until their credential isolation has been implemented and verified. Browser sign-in
currently selects Google OAuth; Google Cloud project onboarding is not automated.

## Conversations in Claude Code and Codex

In a workspace linked for [conversation synchronization](conversation-sync.md), Antigravity
conversations whose workspace is in that folder are copied to Claude Code and Codex, so
`/resume` in either CLI continues them. Kiln reads, and never writes:

- `conversation_summaries.db` for each conversation's workspace folder and parent
  conversation (subagent conversations are skipped); Kiln queries a private copy of it and its
  write-ahead log, so nothing is created next to agy's database,
- `brain/<id>/.system_generated/logs/transcript_full.jsonl` for the conversation, when
  `conversations/<id>.db` exists,
- `annotations/<id>.pbtxt` for the title agy shows in its resume picker.

User requests, answers, tool calls and tool results are copied. agy's reasoning, system and
checkpoint steps stay in agy. A turn appears once agy answers it; a request agy never
answered, for example after its quota ran out, appears once its transcript has been
unchanged for five minutes, followed by a note that Antigravity stopped before answering.
If agy's files cannot be read, Antigravity conversations are skipped and Claude Code and Codex
conversations keep synchronizing.

Turns continued in agy reach the copies. Turns continued in Claude Code or Codex do not reach
agy, because agy keeps its conversations in a private database. When both continue the same
conversation, the most recent side wins as for the other CLIs: if the copies win, the agy
conversation is detached and kept unchanged in agy, its turns are kept as a branch in the
status dialog, and Kiln stops following it.

## Maintainer checks

Use fixtures shaped like agy 1.4.5's files in a temporary folder (`KILN_ANTIGRAVITY_DIR`
points Kiln at it) and a fake `agy` that holds its presence lock. Never send prompts to a
developer's real agy account as part of a test.

References:

- [Antigravity CLI](https://antigravity.google/docs/cli/reference)
- [Antigravity hooks](https://antigravity.google/docs/hooks)

Account integration references:

- [Official CLI authentication](https://github.com/google-antigravity/antigravity-cli#authentication)
- [Google OpenID Connect UserInfo](https://developers.google.com/identity/openid-connect/openid-connect#obtaininguserprofileinformation)
- [go-keyring macOS implementation](https://github.com/zalando/go-keyring/blob/master/keyring_darwin.go)

The local agy 1.4.6 binary and isolated launches were inspected for TTY requirements,
`ANTIGRAVITY_APP_DATA_DIR`, `ANTIGRAVITY_GEMINI_DIR`, private keychain lookup, and the
`antigravity-oauth-token` fallback. These are version-sensitive integration details.
Tests use synthetic credentials; a real browser sign-in and Google UserInfo response
remain a separate interactive acceptance check.
