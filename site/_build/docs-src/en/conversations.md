---
title: Conversations and updates
description: Continue a conversation between Claude Code and Codex, bring Antigravity conversations into both, and update agent CLIs without losing your place.
---

Kiln can copy a workspace's conversations between Claude Code and Codex, so you can pick up where the other agent left off with `/resume`. It also ties each panel to its conversation, so a CLI update or restart returns you to the same place.

## Link a workspace {#link-a-workspace}

1. Sign in to a Claude Code account and a Codex account in **Settings → Accounts**. See [Accounts](accounts.html).
2. Right-click a workspace in the sidebar and choose **Link conversations**.
3. In the dialog, set **Recent conversations** to the number of conversations to keep in sync. The default is **20**, and you can choose from 1 to 4096.

Sync is opt-in for each workspace and runs while Kiln is open. It includes conversations anywhere inside the workspace folder, including ones you started outside Kiln.

The limit counts the most recently active conversations in the workspace. Resuming an older conversation makes it eligible again. Lowering the limit stops syncing the rest, and their existing copies stay available.

To stop, choose **Unlink conversations** from the same menu. Copies already made stay available in `/resume`. Closing a workspace pauses its sync.

The link stays with the account pair shown in the status dialog, even if you later switch accounts.

## Continue in the other CLI {#continue-in-the-other-cli}

Open the same project in the other CLI and use `/resume` to pick the conversation. Each linked conversation appears once in each CLI's resume list with the same title, in ordinary terminals and in Kiln account panels alike.

The earlier messages show up in the CLI's normal conversation view and become part of the next request to the model. Kiln makes no model requests of its own.

### What carries over {#what-carries-over}

| Content | Result |
| --- | --- |
| Your requests and the agent's answers | Copied with their wording, line breaks and code blocks. |
| Conversation name | Shared by every copy. A rename in either CLI applies everywhere, and the most recent rename wins. |
| Shell commands and file edits | Shown as the other CLI's own tool records where it has a matching tool, otherwise as readable text. |
| Images | Kept. |
| Reasoning, provider instructions, encrypted records | Not copied. |
| Available tool definitions | Each CLI keeps its own configured tools. Codex adding definitions does not block copying messages or recorded calls. |

Tool calls from the history never run again. A conversation with content Kiln cannot copy, such as documents or audio, fails visibly instead of being marked complete with something missing.

### When sync runs {#when-sync-runs}

Automatic sync pauses for a workspace while any agent CLI in it is running, including while it waits for input. About a second after a CLI exits back to its shell, or its terminal closes, Kiln syncs again. Background checks run again 60 seconds after each check finishes.

Only completed responses are copied. A conversation still being written is read once it has been unchanged for 10 seconds.

## Antigravity conversations {#antigravity-conversations}

Antigravity (`agy`) conversations in a linked folder are copied to Claude Code and Codex, so `/resume` in either CLI continues them. Copying goes one way: Kiln only reads Antigravity's files and never writes to them.

- Requests, answers and tool calls are copied. Antigravity's reasoning stays in Antigravity.
- A turn appears once Antigravity answers it. A request Antigravity never answered, for example after its quota ran out, appears after five minutes without changes, followed by a note that Antigravity stopped before answering.
- Turns continued in Claude Code or Codex do not reach Antigravity.
- On macOS, conversations from [Antigravity accounts](accounts.html#antigravity-accounts) are included.

The status dialog names the accounts that receive copies from the central history. These labels do not exclude Antigravity sources.

Recognized Antigravity file reads, single file replacements and shell commands appear as native tools in Claude Code and Codex. Failures remain failures. Unknown tool shapes stay readable, and copying never executes a tool.

## When both sides continue {#when-both-sides-continue}

If the same conversation was continued in two CLIs before Kiln synced it, the side used most recently becomes the main history. The other continuation is kept as an archived branch and removed from that CLI's resume list. Choose **Restore** in the status dialog to turn the branch into a separate conversation in every CLI.

If the copies of an Antigravity conversation win, the original stays unchanged in Antigravity and Kiln stops following it.

### Opening a linked conversation inside Kiln {#opening-a-linked-conversation-inside-kiln}

When you open a linked conversation in a new Kiln panel while another Kiln panel holds it, Kiln stops the previous CLI, waits for it to exit, syncs the final state and opens only the new panel. If a CLI does not exit, the new panel waits with a visible status. Kiln never force-quits it.

> **Note:** External terminals and other apps do not take part in this handover. Close them before continuing the conversation in Kiln.

## Check sync status {#check-sync-status}

Click a workspace's sync icon, or choose **View conversation sync status** from its context menu. The icon and dialog show waiting, working, attention needed, paused, or a green check when every completed conversation is synced.

Each row shows a conversation's title, the CLIs that hold it and where to continue it with `/resume`. **Conversation details** lets you copy each conversation ID and a failed item's diagnostic.

Warnings appear only for problems you can fix:

| Warning | Action |
| --- | --- |
| A connected account folder is missing | **Open account settings** |
| Storage is full or cannot be written | **Retry** |
| The Codex CLI is missing | Install Codex so conversations can be registered for `/resume` |

**Ignore** hides a warning from the badge until a different problem occurs; it stays listed under *Show ignored warnings*. Conditions that need nothing from you appear in grey. **Check again** starts a fresh check right away.

## CLI updates {#cli-updates}

Open **Settings → Accounts → CLI updates** to see each CLI's installed executable and version, and the latest published version where available.

| CLI | How Kiln updates it |
| --- | --- |
| Claude Code | `claude update`, keeping your configured update channel. |
| Codex | The Homebrew or npm installer it was installed with. Custom installations report that their own installer is required. |
| Antigravity | `agy update`. Antigravity checks its own latest version, so Kiln shows none. |

Update a CLI from this page, or turn on **Automatically update Codex, Claude Code, and Antigravity** (off by default) to check every six hours while Kiln is open. Kiln never uses sudo.

## Restart in the same conversation {#restart-in-the-same-conversation}

Choose **… → Restart in the same conversation** on a panel to reopen it in its current conversation. To do this after every update, turn on **Restart sessions in the same conversation immediately after updates** (off by default). It interrupts the panel's current response once the update has installed and the CLI reports its new version. The restart keeps the panel, layout, working directory and account. It does not resend your last request or repeat tool calls.

Kiln stops the CLI with its normal `Ctrl-C` sequence. If the CLI refuses to stop, Kiln leaves it running and shows an error. If Kiln cannot confirm the original conversation after relaunch, it warns you and offers a retry.

### Which panels can restart {#which-panels-can-restart}

Kiln tracks the conversation of each new Claude Code and Codex panel it opens, even when two panels share a folder. Open the panel menu to see its CLI version and conversation ID. Panels that existed before tracking and CLIs typed into an ordinary shell are not tracked, so open a new agent panel.

> **Tip:** Codex asks you to trust new hooks. If tracking stays pending, open `/hooks` in Codex, trust the Kiln hooks and send your next message. If hooks are disabled, automatic restart is unavailable.

Antigravity panels are tracked without hooks. On Windows, Antigravity panels have no recorded conversation and are not restarted automatically.

If a task's terminal disappears, Kiln resumes its recorded conversation instead of sending the original request again.
