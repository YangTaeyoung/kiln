---
title: AI terminal: sessions, shells and recovery
description: Shell integration, activity reporting, command history, completion, connection recovery and background sessions in Kiln's terminal.
---

Every Kiln task runs in a real terminal session owned by a local daemon. Shell integration adds command tracking, activity reporting and completion on top, and sessions keep running when you close the window.

## Shell integration {#shell-integration}

New default zsh sessions get shell integration automatically. It tracks command boundaries and the working directory, and it does not modify your `.zshrc`.

Automatic integration does not apply to custom launches with arguments, or to bash and fish. Shells that were already open may need a new session before new integration behavior applies. To turn integration off, set `KILN_SHELL_INTEGRATION=0`.

### Attach an existing zsh session {#attach-an-existing-zsh-session}

1. In Settings, choose `/bin/zsh` as the default shell and open a new Kiln terminal. This sets up the integration files.
2. In the existing zsh, wait for any foreground task to finish, then run this at an idle prompt:

```zsh
source ~/.local/share/kiln/shell-integration-v1/integration.zsh
```

Starting `zsh` inside an existing terminal does not run Kiln's new-session setup on its own.

## Activity and notifications {#activity-and-notifications}

Kiln reads terminal notifications (OSC escape sequences) and explicit activity reports. Both are local terminal escape sequences and do not send your prompt to another service.

Report activity from a script or agent hook with `kiln activity`:

```sh
kiln activity running
kiln activity waiting
kiln activity done
kiln activity failed
```

Send a notification with `kiln notify`. Run `kiln activity --help` and `kiln notify --help` for the syntax of your installed version. Shell integration also provides a `kiln-agent-status running|waiting|done|failed|unknown` function. Hook output has to reach the Kiln terminal for Kiln to receive the activity.

Kiln also recognizes the activity markers that Claude Code and Codex put in their terminal titles, so running activity appears without any hooks. Explicit waiting and failure reports take priority. Plain shells and agents that report nothing appear as open sessions instead of being marked complete. See [Agents](agents.html) for the indicators and colors.

The notification center keeps recent notifications and links each one to its session. Reading a notification clears that session's attention state.

## Command history {#command-history}

The daemon keeps your most recent commands in memory so you can review them and their output.

| Limit | Value |
| --- | --- |
| Commands kept | The most recent 40 |
| Output kept per command | The first 128 KiB |
| Survives a daemon restart or upgrade | No |

History is a reconstructed text view of each command's output. It is not a raw terminal log and does not record every intermediate screen of a full-screen TUI. Without command-boundary integration, only the currently visible output is available.

If you edit a command from history, it runs only when you choose its explicit run action, and it runs in a new terminal.

## Shell completion {#shell-completion}

In a newly opened, integrated zsh terminal, suggestions appear automatically beside the cursor and follow it as you type. They include commands, subcommands, options and supported local arguments.

| Key | Action |
| --- | --- |
| `Tab` | Insert the highlighted suggestion |
| `Down` | Select the menu, then use arrow keys to choose a suggestion |
| `Enter` | Insert once the menu is selected; otherwise run the command you typed |
| `Esc` | Close the menu |
| `Ctrl+Space` | Open or reopen and select the menu, or look up installed command names |

Inserting a suggestion edits the command line. Press `Enter` again to run it. Pressing `Enter` before selecting an automatic menu runs your original command. When the menu is closed, zsh's own `Tab` completion works as usual.

Suggestions use the command and its arguments:

- `cd` suggests folders from the terminal's current directory. Choose a folder and continue into the next level.
- File arguments offer local paths. `docker --config` and `docker compose --project-directory` suggest folders; `docker compose --file` suggests files.
- Git commands can suggest local branches from the current repository.
- `npm run`, `pnpm run`, `yarn run` and `bun run` suggest script names from the project's `package.json`.

Kiln automatically reads command, subcommand and option information from supported installed CLIs' local help and caches the results. Installed Docker Desktop plugins such as Compose and Buildx are discovered from the same help output. A bundled catalog provides an offline fallback, including Docker Engine, Buildx and Compose. If a command's help is unsupported or unavailable, Kiln uses the local or bundled suggestions available for it.

Help requests run in an isolated temporary environment without your typed argument values. Completion does not use AI, read shell history files or send completion data to a cloud service.

To hide automatic suggestions, turn off **Command suggestions** in **Settings → Terminal**. `Ctrl+Space` keeps working. Set `KILN_COMPLETION_PREVIEW=0` in your shell to also stop producing preview data.

Completion supports zsh only. Open a new terminal to load updated integration; existing terminals keep their shell setup. Bash, fish, SSH sessions, agent chats, full-screen applications and other shells keep their normal input behavior. Integration help is in the terminal's context menu.

## Connection recovery {#connection-recovery}

Kiln keeps the terminal connection separate from the shell or agent running inside it. If a panel loses its connection or stops responding, Kiln repairs the connection, redraws the screen and asks the CLI to redraw. It does not restart the agent, send a prompt or run `/resume` for you.

| Panel shows | Meaning |
| --- | --- |
| **Reconnecting** | Kiln is repairing the connection, up to three attempts. |
| **Connection stalled** | The attempts failed. Choose **Reconnect** to try again. |

**Reconnect** is also in the **…** menu of every running terminal panel, even when the connection looks healthy, and it resets that panel's retry budget. A quiet agent that is thinking or waiting for input is not treated as stalled.

During recovery, the session and its process keep running and the existing text stays on screen. Input is paused, so your typing never queues up behind a broken connection.

> **Note:** A CLI can hang internally while its connection stays healthy. **Reconnect** cannot fix that. Open another panel and use the CLI's own resume flow.

## Sessions that outlive the window {#sessions-that-outlive-the-window}

Your terminals belong to the session daemon, not to the window. Close Kiln and its shells and agents keep running; reopen it to return to them. Updates keep running terminals too. A system reboot or explicitly stopping the daemon can end sessions.

- **macOS:** the menu-bar icon opens a menu with **Settings…** and **Stop Background Sessions…**. Ending background sessions lists the live sessions and asks for confirmation, with **Cancel** as the default. It includes sessions in open workspaces, so unsaved shell or agent work can be lost.
- **Windows:** the notification-area icon shows a session summary and a red dot when something needs attention or a notification is unread. Its menu has **Open Kiln**, **Settings** and **Stop Background Sessions…**.

> **Warning:** `kiln status` shows the session daemon's state. Do not use `shutdown-daemon` to refresh the window: it ends your sessions.

## Local network access on macOS {#local-network-access-on-macos}

macOS 15 and later require permission for local-network connections. This can affect terminal commands such as `kubectl get nodes`, SSH, database connections and remote files when their servers are on a local network.

1. Open **Kiln → Settings → Behavior**.
2. Under **Privacy → Local Network**, choose **Request Access**.
3. If macOS shows a prompt, allow Kiln, then retry the command.

The gear button opens **System Settings → Privacy & Security**, where you can check Kiln under **Local Network**. Kiln cannot read or change the system's decision, and macOS may not show a new prompt if you already decided.

> **Tip:** Panels opened before an update keep their old process setup. If a local-network command fails there, open a new terminal panel and retry it.

## Windows shells {#windows-shells}

Terminals start in PowerShell 7 when it is installed, otherwise in Windows PowerShell. Choose `cmd.exe`, Git Bash or a WSL distribution in **Settings → Terminal**.

- PowerShell and cmd report commands, exit codes and the current location to Kiln. A custom prompt still sees the last command's status.
- PowerShell keeps PSReadLine's completion (`Tab`, `Ctrl+Space`). Kiln's completion menu is zsh-only.
- Closing a terminal ends its console programs. Window applications it started, such as a browser, keep running.
- The pane is named after the newest console program running in the shell. Programs started from Git Bash are not followed.
- Korean IME input works in terminals and the editor.
