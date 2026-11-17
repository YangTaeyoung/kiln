---
title: Run Claude Code, Codex and Antigravity
description: Run Claude Code, Codex and Antigravity in native conversations or terminals, with provider settings, commands, permissions and durable drafts.
---

Kiln runs Claude Code, Codex and Google's Antigravity CLI (`agy`) inside workspace panels. You write a request, pick an agent, and follow every task's state from the sidebar without hunting through terminals.

## Before you start {#before-you-start}

The agents are external tools. Install the official CLI for each agent you use and sign in to it. Kiln does not include a subscription to any of them.

- **Claude Code** and **Codex**: install the CLI, then add your accounts in **Settings → Accounts**. See [Accounts](accounts.html).
- **Antigravity**: install `agy` with Google's installer. On macOS, sign in from **Settings → Accounts → Antigravity**. On Windows, sign in by running `agy` in a terminal. Kiln looks for `agy` in your `PATH`, `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`.

On Windows, Claude Code and Codex run as native Windows programs with their homes in `%USERPROFILE%\.codex` and `%USERPROFILE%\.claude`.

## Start a task {#start-a-task}

Choose **New Task**, enter a request, pick an agent and start. The agent selector (Claude Code, Codex or Antigravity) stays directly below the request input. Repository context scrolls separately, so a long summary never pushes the start action out of reach.

For Antigravity you can also use:

| Action | What it does |
| --- | --- |
| **New task → Antigravity** | Starts `agy` in the current workspace. |
| **New task → Open in folder → Antigravity** | Asks for a folder first, then starts `agy` there. |
| **New Antigravity task** (command palette, `⌘K`) | Starts an Antigravity task. |

When you choose Antigravity below a request, Kiln passes the request and the repository summary to `agy` as its initial prompt. The `antigravity` command opens the Antigravity editor, so Kiln does not treat it as an agent.

Unsent requests are saved with the workspace and restored when Kiln reopens. Closing a workspace asks before discarding them.

The right and down split buttons let you choose a terminal or an agent for the new panel. Keyboard split shortcuts still open a terminal directly.

## Native conversation or terminal {#conversation-gui}

In **New Task** and the right/down split menu, choose **Conversation GUI** or **Terminal** before choosing an agent. New agent launches default to Conversation GUI. Save your preference in **Settings → Behavior → New agent presentation**. Ordinary terminals, SSH and existing terminal tasks keep their current presentation.

The conversation shows streamed Markdown, expandable tool input/output and file diffs. Answer inline permission or question cards; both allow and deny work. The header pins the starting account and shows the confirmed effective model/mode. Composer choices contain only models and modes advertised by the provider. Existing native MCP and permission settings are preserved.

Press **Enter** or **Send** to submit; **Shift+Enter** adds a line. Korean IME preedit and commit do not submit. Use **Attach image** to select several files at once, paste an image while the message editor has focus, or drop files onto the intended conversation pane. Each draft accepts up to eight images (PNG, JPEG, GIF, WebP; 20 MiB each). Text paste still inserts text. Check the thumbnails and use each image’s **×** action to remove it before sending. Remove missing or invalid restored attachments before sending. **Stop** interrupts the current turn.

Closing Kiln saves drafts while the daemon keeps the worker alive. Closing a pane ends its session and asks before discarding drafts. Locally queued sends wait for journal confirmation: reopening never resends them. An unconfirmed send offers **Recover as draft**; check history before intentionally sending again. Failed or disconnected sessions retain history and offer **Restart same conversation**. Earlier/latest message controls navigate a bounded display window without deleting history.

Approval cards show the command, folder, affected paths, reason and requested access with localized titles. Transport IDs and schemas stay out of the decision. After full history replay, a restored send absent from the journal and mailbox offers explicit recovery even if the worker is Ready; it is never resent automatically.

Claude approvals include the actual MCP/tool arguments, including domain identifiers; long previews are bounded and labeled partial. Codex transport parameters and schemas stay filtered. Recovery also waits for a queue observation that follows any final publication by the old client.

The activity strip above the composer distinguishes preparing a response, running tools, waiting for input and response completion. It stays visible while you read older messages. Claude’s native shell, Monitor and other background tasks show their reported descriptions, states, progress summaries and available durations; expand a task for details. Ambient watchers remain listed without treating them as an unfinished response. Other providers show their reported running tools. When you scroll up or open earlier history, choose **↓ Jump to latest** to return to the latest messages without changing your draft.

Use **View output · Refresh** in a shell or Monitor task to read its latest output without sending an AI prompt. A partial tail, empty output and unavailable output are labeled separately.

<figure class="shot"><img src="../assets/shots/agent-activity-en.webp" alt="Conversation activity and background tasks" width="1280" height="820" loading="lazy" decoding="async"></figure>

### Switch the same conversation {#switch-presentation}

Idle GUI-to-TUI checks resolve declared MCP helper commands using the conversation’s saved environment and working folder, including a helper-specific `PATH` override. A connected MCP service by itself is not a running model task.

Pane titles and automatic page names use a shortened first confirmed request. Manually named pages keep their names; editing a draft does not rename an established conversation.

The **GUI | TUI** joined tabs sit inside each agent pane at the top right, below its close button. Replies show the agent icon beside its name. Expanded tool details provide a readable, scrollable code area with syntax highlighting for recognized file languages and shell commands, plus a copy action. Unknown output remains plain text.

Use the agent pane’s **GUI | TUI** tabs to continue the same confirmed conversation with its original account and working folder. A new Claude Code conversation can also open in the terminal before its first message, without trying to resume a history file that does not exist yet. You can request TUI while work is running. Kiln keeps the work in GUI and switches automatically after the response, approvals and queued work finish and ownership checks pass. Select GUI to cancel a pending switch. This is a deferred native handoff, not an immediate move of a running model request between processes. A native TUI still needs to exit normally before its GUI writer starts; selecting TUI cancels that return request. If a background task remains or Kiln cannot verify process ownership, it refuses the switch and leaves that work running. The terminal starts only after the previous agent has finished exiting; a refusal explains what blocks the switch.

Selecting Conversation GUI while the native CLI is open requests a return; it does not close the CLI or send a command for you. Exit the CLI normally, using its own exit action. Kiln waits for its process to exit, then imports newly appended user/assistant text and reconnects the GUI. Images in imported native history appear as attachment notices; this is not a reconstruction of native tool cards. If earlier native history changed, Kiln preserves the retained GUI history and reports the mismatch.

Interactive `claude`, `codex` and `agy` commands started through Kiln’s registered shell functions can also use the joined **GUI | TUI** tabs. Kiln connects the pane only after verifying the exact live host binding for that terminal. The original PTY, parent shell, command arguments and selected account profile are retained. This applies to the functions supplied by Kiln’s zsh or PowerShell integration; the registered commands depend on the shell.

To move from TUI to GUI, select **GUI**, then exit the native CLI normally, for example with `/exit` or **Ctrl+D** where that CLI supports it. Kiln waits for natural exit and does not terminate current work or inject a guessed quit command. If you exit the initial TUI without requesting GUI, the original shell resumes with the native exit status.

Management, print and remote commands, pipelines, redirected input/output, and user aliases or custom functions that bypass Kiln’s integration keep their native execution path. Bash and fish do not receive automatic agent wrappers. Commands in terminals outside Kiln are not automatically tracked.

For an older terminal-launched agent, GUI import requires an exactly tracked native conversation. Exit that CLI first. Kiln waits for the terminal daemon to confirm that the old terminal process exited, then opens the recorded conversation with its original account environment and working folder. A disconnected terminal or an unknown conversation does not establish a safe import boundary.

## Work across several repositories {#work-across-several-repositories}

Open a parent folder, such as `~/dev/projects`, when your frontend, backend and shared libraries live in separate Git repositories. Kiln keeps that folder as the agent's working root and can include the discovered repository layout with your request, so one request can cover all of them.

Inspecting a repository in the Git or GitHub tools does not change the task's working directory. Use the repository selector in those tools to target one repository for Git operations. See [Workspaces and tasks](workspaces.html) and [Git and GitHub](git.html).

## Follow task state {#follow-task-state}

Each task row in the sidebar shows the state its agent reports.

| State | Meaning |
| --- | --- |
| Working | The agent is working. |
| Waiting for Input | The agent needs your input. |
| Done | The agent finished. |
| Failed | The agent reported a failure. |
| Session Open | No state was reported. Kiln shows the session as open instead of guessing. |

Running work appears first, followed by requests waiting for input and then older failures. Click a task to jump to its workspace, tab and split, including a pane hidden by zoom.

### How Kiln knows the state {#how-kiln-knows-the-state}

An agent must report its state, or show a running title Kiln recognizes, for Kiln to display it accurately.

- Kiln recognizes the activity markers in Claude Code and Codex terminal titles, so running activity shows without extra setup.
- Claude Code's `✳` marker is not treated as completion, because it also appears while a dialog is open.
- Explicit waiting and failure signals take priority over title markers.
- An idle terminal is never treated as a completed task, and an unfinished launch command does not mark an idle agent as running.

Scripts and other tools can report state with `kiln activity running`, `waiting`, `done` or `failed`. See [Terminal](terminal.html) for activity reporting and notifications.

### Indicator colors {#indicator-colors}

| Agent | Indicator |
| --- | --- |
| Claude Code | Orange |
| Codex | Blue |
| Antigravity | Antigravity mark in the tab, task list and sidebar |
| Unidentified session | Neutral |

Indicators appear in the sidebar, the tab and the panel header. In a split tab, the focused pane wins when states have equal priority. Warnings always take priority.

## Notifications and usage limits {#notifications-and-usage-limits}

The notification center keeps recent notifications and links each one to its session. Reading a notification clears that session's attention state. On Windows, agent notifications appear as Windows toasts.

When Antigravity reports that its quota is reached, Kiln shows a usage-limit notification with the reset time. For Claude Code and Codex, a panel keeps the account it was opened with. When an account reaches its usage limit, switch to another account in **Settings → Accounts** and open a new panel. See [Accounts](accounts.html#choose-which-account-to-use).

## Codex capacity recovery {#codex-capacity-recovery}

While Kiln is open, managed Codex panels retry a turn that Codex ended because its server was overloaded. Only that specific Codex error starts a retry. Usage limits, sign-in failures, stream errors, cancelled turns and successful turns are left alone.

- Retries wait 15, 30, 60, 120 and 240 seconds. The panel shows a countdown and a **Cancel auto-retry** button.
- Typing in the panel after the failure cancels the pending retry, including a partly typed prompt.
- A new request you submit yourself starts a fresh set of retries.
- Retries keep the same account and model.

If the Codex CLI is still open, Kiln sends one continuation prompt in the same conversation. If the CLI has exited, Kiln resumes the same conversation with its original account and working directory, then sends the prompt. The prompt asks Codex to check the current state before repeating a completed action. It does not replay your original request, and changes made during the failed turn are not rolled back.

Retries apply to Codex panels opened with a saved account whose conversation Kiln has confirmed. Failures from before Kiln started watching a running CLI are left for you to handle.

## Codex background-server settings {#codex-background-server-settings}

New Codex panels opened by Kiln run with `--no-daemon`, independent of Codex's own shared background server. If a Codex session reports **Background server has incompatible feature settings**, the session's feature flags differ from the ones that server saved. You can choose:

| Option | Effect |
| --- | --- |
| **Run without daemon this time** | Starts this session without restarting the shared Codex server. The CLI equivalent is `codex --no-daemon`. |
| **Restart with these settings** | Changes the shared settings. This can interrupt other clients' active or queued work, so check them first. |

In newly opened Kiln Zsh terminals, plain `codex`, `codex resume …` and `codex fork …` also use `--no-daemon`, so a shared server reporting **Server is draining; retry after reconnecting** does not block those launches. Explicit `--remote` connections, existing aliases/functions, other command forms and other shells keep their normal behavior. Use `command codex …` to bypass the wrapper or add `--no-daemon` yourself for a local session. Reopen existing shells to load this integration. Kiln does not restart the shared server or change its update settings.

## Native session settings and commands {#native-session-settings}

You can send a follow-up with Send or Enter while a response is running; Stop remains a separate button. Claude accepts streaming follow-ups and Codex steers the current turn. Antigravity holds one follow-up until its current prompt finishes. A pending receipt is kept separate from an accepted message and is never automatically resent.

Model, effort and mode controls remain available during a response. Claude applies live changes at the native boundary; Codex composer selections update the next turn, while its explicit active-turn action changes supported settings of the current turn. Settings still wait for native confirmation. Workspace activity rows show the conversation title instead of the provider name, while preserving custom page titles.

In a conversation GUI, use **Session settings** for searchable provider controls and **Commands and skills** to find native commands. A compact two-line message editor sits above one toolbar: attach an image, choose the model and mode, open **Reasoning settings**, or use the round Send/Stop control. Narrow panes keep the model beside **Conversation options**, which contains the remaining controls. Choices show readable model names and localized mode/effort labels while preserving the provider’s actual values. A missing catalog remains different from an empty one.

Open Reasoning settings, or Conversation options in a narrow pane, to choose an offered reasoning level with an explanation of each choice. Claude’s independent Ultracode switch has a purple indicator when enabled. The header shows the confirmed effective model and mode.

Claude Code, Codex and Antigravity share localized setting names and visible explanations for built-in choices. In Korean, approval policies use readable names such as “필요할 때 승인 요청” (ask when needed). The same presentation covers startup options, active-turn settings, sandbox controls and granular approvals. Provider-defined names and native values remain intact.

Model comparison puts the current model first, with cards showing one model name, a concise summary, default reasoning effort and supported inputs. Choose **Use this model** to request a change; the provider confirms its application. Expand details for supported levels and the provider’s original description.

<figure class="shot"><img src="../assets/shots/agent-models-en.webp" alt="Model comparison with the current model first" width="760" height="560" loading="lazy" decoding="async"><figcaption>Model comparison with the current model first</figcaption></figure>

<figure class="shot"><img src="../assets/shots/agent-settings-en.webp" alt="Settings with visible explanations" width="1280" height="820" loading="lazy" decoding="async"><figcaption>Settings with visible explanations</figcaption></figure>

Illustrations rendered from synthetic catalogs with the native UI; not an authenticated model response.

A changed setting shows its requested and native values separately. Send and conflicting settings wait for native confirmation; Stop remains available. A transport acknowledgment alone does not confirm that a setting took effect. Rejected or unconfirmed changes stay visible and are never automatically retried.

Commands and skills has a search icon, a clear action and a bounded scrolling list. Names align to the left; expand a row to read its full description and details. Search covers names, descriptions and aliases. Terminal-only commands and skills that cannot be invoked in the GUI stay out of the list. Selecting an entry only inserts an editable draft, preserving existing text and images; Codex skills also retain their native input until explicit Send.

Typing `/` opens a scrolling completion list with short descriptions; Codex skills also complete from `$`. Use Up/Down to select, Tab or Enter to insert, and Esc to dismiss. Completion does not send the message. Recognized command/skill tokens are visually distinct from ordinary text. Refresh the catalog after native skills change.

Session settings shows controls that can actually be edited in the GUI, grouped by response, working style, context and permissions. Read-only native inventories and unavailable controls are omitted. Change history, reset actions and reference information stay behind details controls. Start a separate terminal from **New task** when you need the CLI interface.

**New conversation settings** opens a separate conversation with the currently selected account and explicit startup options. It preserves the current session and draft, and does not replay a prompt. Native actions such as review, goals and queued tasks run only from their explicit action buttons. Current-turn settings target the reported running turn and do not change work already captured by the provider.

Tool headers use the app language, including questions and known MCP operations. Expand a card or hover its header to inspect the original tool name; arguments, output, questions and diffs keep their original content. Language changes relabel existing history. Unknown operations use an external-tool label. Extension preparation and retained cache versions do not prove that the native provider loaded an extension.

Type `@` in the message to search files and folders in the current workspace. Use the arrow keys to choose, Tab to browse a folder, Enter to insert, and Esc to dismiss. Paths with spaces and non-Latin names are quoted. Selection only edits the draft; sending remains explicit. Claude keeps its native `@` reference syntax, Codex uses the same path-as-text contract as its TUI, and Antigravity receives the selected full path. Provider permissions and native context handling still apply.

## Provider feature coverage {#provider-coverage}

The reference versions inspected for these controls are Claude Code 2.1.325 (SDK 0.3.325), Codex CLI 0.167.2 and Antigravity CLI 1.5.9. These are reference versions, not minimum requirements or a promise that every later release behaves identically. The installed CLI, model, account and organization policy determine availability. A visible control or prepared extension does not establish that an authenticated model task succeeded.

| Provider | Current-conversation controls and actions | Separate new-conversation options |
| --- | --- | --- |
| Claude Code | Model, advertised effort, permission mode and independent Ultracode. Advanced controls cover Fast mode, workflows/size/keyword, output style, main agent/advisor, automatic compaction, response language, fallback models, cache lifetimes, prompt suggestions, skill exposure and meaningful thinking budget/display. Explicit skill/style/plugin reloads; plugin reload needs native safe-reload support. | Turn/cost budgets, compaction window, system/additional instructions, setting sources, tool list and tool allow/deny lists. |
| Codex | Model, advertised effort/tier, collaboration mode, permission profile, approval policy/reviewer, summary and sandbox/granular approval. Explicit compaction, goals/budgets, queue editing/start, review, active-turn settings, memory mode/status and MCP reload. Skills keep their native input until Send. | Model/effort/tier, permission profile, summary, verbosity, web search, context/compaction limits, base/developer instructions, ephemeral conversation and absolute workspace roots. |
| Antigravity | All advertised supported ACP select options, including model, reasoning effort and mode. Ordered groups/descriptions are retained; unknown categories stay visible and unsupported types are omitted from the GUI. Native commands insert drafts. Commands and usage update during load and live use; separate manual refresh/actions can be unavailable. | Sandbox request, main agent and additional absolute directories. |

Only editable GUI controls appear in **Session settings**. Native reference catalogs and method inventories cover more than the GUI exposes; they are not presented as configurable settings. Under-development plugin APIs are not dispatched. This adapter does not report Antigravity MCP connection health; an empty client server list is not a health result.

Permission cards retain native offered decisions. Codex can grant a selected requested filesystem/network subset with turn/session scope and strict-review choice. Kiln does not silently select a bypass policy. Native policy, entitlement and endpoint errors remain visible. Current-turn changes affect the reported active turn's settings, not inference the provider already captured. Source acknowledgment without runtime-effect readback is shown separately from confirmed application.

See the native [Claude model reference](https://code.claude.com/docs/en/model-config.md), [Codex App Server reference](https://learn.chatgpt.com/docs/app-server) and [ACP configuration contract](https://agentclientprotocol.com/protocol/v1/session-config-options). Native catalogs can change between accounts and models; an unavailable or partial result is not a complete list.

## Personal extensions and account scope {#personal-extensions}

New launches prepare supported personal Claude/Codex skills and self-contained plugin assets for the selected private profile. Claude also supports portable legacy commands, agents, output styles and workflow bundles. Native project precedence remains in effect; shared Codex HOME skills and Antigravity assets remain native. Existing account sign-in/bootstrap behavior is preserved. Source author files remain unchanged, and private profile edits/deletions survive later imports. Status polling and resuming a worker do not refresh these imports.

**Extension prepared** means import preparation succeeded. **Existing extension retained** means a prior cache version remains for readers. Neither means enabled, loaded or executed. Native discovery and execution need their own result. Portable supported plugin declarations can retain bundled hooks/MCP/LSP configuration without running it during preparation. Credential-bearing configuration, account-linked connectors, unsupported formats, external paths, routines and scheduler state need native setup or produce a visible diagnostic. Kiln does not copy the entire configuration or account history. See [native plugin loading](https://code.claude.com/docs/en/plugins/loading.md).

## Next steps {#next-steps}

- [Accounts](accounts.html): sign in, switch accounts and watch usage.
- [Conversations and updates](conversations.html): continue a conversation in another CLI and restart after updates.

If a shell-attached agent exits unexpectedly, its unsent draft is kept with that conversation. GUI remains available for the saved draft, and TUI returns to the original shell. The draft is not injected into a different agent started later in the same shell.

Kiln preserves the launch PATH and appends installed CLI runtime directories for GUI and TUI workers. This also lets command hooks find Node installed in Homebrew or the local user bin. Existing running workers keep their original environment; start a new worker after updating. Your hook files are not rewritten.
