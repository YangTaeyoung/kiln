# Antigravity CLI (`agy`)

Kiln runs Google's Antigravity CLI (`agy`) next to Claude Code and Codex. Install it with
Google's installer and sign in once by running `agy` in a terminal; Kiln finds it in your
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

**Settings → Accounts → CLI updates** shows the installed `agy` and its version. Updates use
agy's own `agy update`. agy checks its published version itself, so Kiln shows no latest
version for it. **Automatically update Codex, Claude Code, and Antigravity** includes agy in
the six-hour checks.

## Usage limits

When agy reports `Individual quota reached … Resets in …`, Kiln shows a usage-limit
notification with the reset time. Kiln does not save or switch Antigravity accounts: agy
keeps its sign-in in the system credential store and offers no per-launch configuration
folder, so **Settings → Accounts** lists only Claude Code and Codex.

## Conversations in Claude Code and Codex

In a workspace linked for [conversation synchronization](conversation-sync.md), Antigravity
conversations whose workspace is in that folder are copied to Claude Code and Codex, so
`/resume` in either CLI continues them. Kiln reads, and never writes:

- `conversation_summaries.db` for each conversation's workspace folder and parent
  conversation (subagent conversations are skipped),
- `brain/<id>/.system_generated/logs/transcript_full.jsonl` for the conversation, when
  `conversations/<id>.db` exists,
- `annotations/<id>.pbtxt` for the title agy shows in its resume picker.

User requests, answers, tool calls and tool results are copied. agy's reasoning, system and
checkpoint steps stay in agy. A turn appears once agy answers it; a request agy never
answered, for example after its quota ran out, appears once its transcript has been
unchanged for five minutes.

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
