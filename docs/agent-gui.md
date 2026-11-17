# Native agent conversations

Choose **New Task → Conversation GUI → Claude Code, Codex or Antigravity**. New
agent launches use GUI mode by default. The same GUI/Terminal choice is available
in the right/down split menu and **Settings → Behavior → New agent presentation**.
An ordinary terminal, SSH connection, existing saved terminal, and keyboard split
remain terminals. Existing native conversations can use the guarded presentation
handoff described below; an arbitrary running terminal is not converted in place.

The native view uses the installed official CLI and the account selected when the
pane starts. Changing the active account affects future panes. Existing CLI MCP
configuration and native permission defaults remain in effect. Model, reasoning
effort and mode choices follow the provider's advertised metadata. The header shows the confirmed
effective model/mode; a composer choice remains requested until native confirmation.

Enter a task and press **Send** or **Enter**. **Shift+Enter** inserts a line break.
IME composition and its commit frame do not send. Attach or drop up to eight PNG,
JPEG, GIF or WebP images, each at most 20 MiB. Imports and restored attachment
validation run in the background; missing or modified attachments remain visible
and must be removed or replaced before sending.

Assistant text renders as Markdown. Expand tool cards to inspect inputs, outputs
and file diffs. Permission cards show localized action summaries (command, folder,
paths, reason and requested access), keeping transport IDs and schemas out of the decision.
Claude approval cards include the actual tool input, including arbitrary MCP
arguments and domain identifiers. Antigravity’s explicitly provided tool input
follows the same rule. Long input previews are bounded and marked as
partial. Codex request envelopes keep transport fields and schemas out of the UI.
Responses use the generation actually displayed, even while newer events wait in
the bounded view backlog. Stop remains usable during that backlog. Question cards
support text, single choice and multiple choices as advertised. **Stop** interrupts the
current turn without closing its reusable worker. Failed/disconnected sessions
retain history and offer an explicit **Restart same conversation** action.

For Codex, Stop also requests native termination of running command processes
identified in the current thread and turn. It preserves commands from earlier
turns and unrelated background/MCP work. This uses the installed app-server's
experimental `thread/backgroundTerminals/terminate` API. Kiln waits for native
termination or item completion before marking a command stopped and enabling a
new turn. An unsupported, rejected or unanswered termination stays unconfirmed
with an explicit failure after a ten-second confirmation timeout; it never becomes
an automatic retry. Native completion can settle that failure, or a new conversation can be
opened explicitly. Turn interruption alone does not establish process exit.

Closing the GUI saves drafts and disconnects its client. The daemon still owns the
worker. Explicitly closing a pane ends that session and asks before discarding an
unsent or unconfirmed draft. Running/waiting work participates in the existing
close confirmation; an idle GUI host is not treated as a busy task.

A locally queued message is not provider acceptance. Kiln saves its command ID and
payload until the journal confirms acceptance or a matching user message. Reopening
never sends a draft automatically. After complete replay, a restored local send
missing from both journal and mailbox becomes explicitly unknown and recoverable even when its worker remains Ready.
A shared writer revision invalidates an older queue observation after any final
local publication, even when the old client has closed. Only published command
files count as executable mailbox entries. Recovery waits for still-active local
enqueue workers and does not classify missing
work as rejected. Unknown/rejected delivery remains recoverable through **Recover as draft**; check history before intentionally sending again.
A dispatched message is never silently shown as a fresh unsent message.

The latest 100 entries form a bounded display window. **Earlier messages** and
**Latest messages** navigate history without deleting it. Very long individual
messages and tool output are limited for display; the runtime journal remains the
source of truth. Automatic bottom following stops when the user scrolls away.

## Settings, commands and explicit actions

Open **Session settings** to search controls by name and scope. The conversation
composer has a two-line editor and one compact toolbar: attachment, model, mode,
**Reasoning settings**, and a round Send/Stop control. Narrow panes show the model
and **Conversation options**, with the remaining controls inside that popover.
Reasoning settings contains the advertised effort levels as a step slider and
Claude's independent Ultracode switch. Enabled Ultracode uses a purple indicator.
Readable model names and localized mode/effort labels preserve exact provider values.
The header continues to show the confirmed effective model and mode.

Available choices depend on native model/account metadata. Changing a model
reconciles its dependent choices. **Advanced settings** contains the remaining
typed live controls; the reference inventory labels new-conversation, inherited
and managed features. Terminal-only entries are hidden from GUI controls, and
**Change history** starts collapsed. An inventory entry is not permission to invoke
an arbitrary native method or edit native global configuration.

Requested and native values remain separate. Send and conflicting settings wait
for native settlement; transport acceptance alone is insufficient. A rejected or
unconfirmed result stays visible and is not retried automatically. Stop withdraws
deferred prompts but does not roll back a native setting. For a Codex mismatch,
explicit acceptance of the visible native state releases that operation without
claiming the earlier request succeeded. Missing or mismatched ACP snapshots keep
the barrier until the native ACK and authoritative matching state arrive.

Use **Commands and skills** to search native names, display names, descriptions and
aliases. The search has an icon and clear action. A bounded list scrolls internally;
left-aligned rows expand to show complete descriptions and native details.
Terminal-only entries and Codex skills without a native path stay out of the GUI
catalog. Selection inserts an editable draft and preserves existing text and
images; only **Send** executes it. Codex skill selection also retains the exact
native skill input with the draft.

Typing `/` opens a floating, internally scrolling completion list with short
descriptions; Codex skills also complete from `$`. Up/Down selects, Tab or Enter
inserts, and Esc dismisses. Completion never sends the draft. Recognized native
command/skill tokens are visually distinct from ordinary text. The list floats
above the editor without increasing composer height, and IME composition/commit
frames do not accept completion. Absent, empty and partial catalogs remain
distinguishable. Unsupported refreshes report locally; Antigravity supplies its
catalog through native session updates rather than a separate manual-discovery
endpoint.

Claude exposes explicit skill, style and safely supported plugin reloads. Codex
has explicit compaction, goals/budgets, queued tasks, inline review, current-turn
settings, memory mode/status and MCP reload actions. Queue editing can continue
during a turn; starting queued work, review and goal actions require idle state.
Opening settings never creates a goal, budget or task. Native policy and endpoint
availability still govern every action.

**New conversation settings → Open new conversation** starts a separate pane using
the currently selected account and explicit startup options. It preserves the
current session/draft and supplies no replayed prompt. **New conversation in
terminal** opens the ordinary terminal workflow for terminal-only features.
The [provider coverage matrix](agent-feature-coverage.md) lists every implemented
control/action family, startup option and inherited boundary with native references.


## Switching the same conversation between GUI and terminal

Use the joined **GUI | TUI** tabs inside each agent pane, at the top right below its close button, for an already confirmed
native conversation. The handoff preserves the original executable, account
environment, working folder and exact native conversation ID. It requires an idle
conversation without active submissions, approval requests, running tools or queued
work. Additional background/helper processes make the host refuse the switch;
those processes are left running rather than stopped to make the handoff possible.

Requesting GUI while the native CLI is open records a return request. Kiln does
not synthesize an exit command, Ctrl-C or a prompt. Exit the native CLI through its
normal exit action. Only after its natural process exit does Kiln import appended
user/assistant text and reconnect the structured backend. The import verifies the
retained native-history prefix; a mismatch preserves existing GUI history and
reports an error. Images from native history become attachment notices. Native
terminal tool cards and image payloads are not reconstructed by this text import.

An older terminal-launched agent can be imported only from an exactly tracked,
confirmed native conversation. Kiln waits for the daemon to confirm the old PTY
process exited before preparing the GUI from the recorded account environment and
working folder. Disconnection or a missing session-list entry does not establish
exit. Unknown terminals are not guessed from the newest native conversation, and
conversion does not replay a prompt.

Pane and automatic page titles use a shortened first confirmed request, so parallel conversations remain distinguishable. A manually named page keeps its chosen title. Empty conversations show the working folder beside New conversation. Draft edits and later tool output do not rename an established conversation.

## Tool code and readable details

Assistant authors show the provider vector beside their name. Expanded tool cards reserve a responsive reading area with internal scrolling and copy actions. Recognized source paths, shell commands, JSON arguments and fenced source receive the shared editor syntax colors. Unknown logs remain plain text. Highlighting is prepared off the UI thread and bounded; full stored output remains available to scrolling and copying.

## Extensions and tool labels

New launches prepare supported personal author assets for the selected private
Claude/Codex profile. Source files remain unchanged, and private edits/deletions
take precedence over later imports. Existing credential/bootstrap behavior and
native project precedence remain in effect. This is bounded asset reconciliation,
not whole-profile synchronization. Unsupported formats, account-linked assets and
nonportable or credential-bearing plugin configuration need native setup.

**Extension prepared** means the asset was prepared; **Existing extension retained**
means a prior cache payload was preserved. Neither establishes native enablement,
loading or execution. Native catalog/status evidence remains separate. See
[extension coverage and limits](agent-feature-coverage.md#personal-extensions-and-saved-accounts).

Tool/question headers follow the app language, including known MCP operations.
Expand a card or hover its header to inspect the original name. Unknown operations
use an external-tool label. Locale changes relabel existing history without
translating or rewriting arguments, outputs, questions, model text or diffs.

## Implementation references

`app/agent_gui/view.rs` owns the native presentation and draft reconciliation.
`settings_ui.rs`, `settings_presentation.rs`, `commands_ui.rs`, `actions_ui.rs` and
`launch_ui.rs` render typed controls; `presentation.rs` owns native terminal
handoff and validated history import; `tool_presentation.rs` projects localized headers without changing wire
data. `controls.rs` defines the safe shared contracts; provider adapters retain
native IDs and validate each explicit request.
`app/agent_gui/client.rs` handles bounded background journal/command transport.
The `agent_views` map overlays existing Term panes; launch-spec GUI markers select
that view on restore. GUI teardown drops only the client. Markdown parsing,
attachment I/O and canvas decoding run on background tasks.

The view uses egui 0.36.2 [ScrollArea](https://docs.rs/egui/0.36.2/egui/containers/scroll_area/struct.ScrollArea.html)
and its standard TextEdit IME path. The shipped empty navy canvas is the selected design asset `clean-0-71f28052a152.png`; interactive text and cards remain
native widgets. It is used in dark mode; light mode follows the current theme.

See [native component notes](agent-gui-design.md) for the implemented conversation, composer, decision, menu and notification geometry.
See [candidate native verification](maintainers/0.3.0-status.md) for observed native
controls/tools, inference provenance and the unresolved physical Korean IME boundary.

## File and folder references

Type `@` in the composer to search the current workspace. Completion runs off the UI thread with bounded results and stale-query rejection. Arrow keys select; Tab browses folders; Enter inserts without submitting; Esc dismisses. Claude preserves its native `@` reference format, Codex follows the official TUI path-as-text contract, and Antigravity receives full paths. Names with spaces and Unicode remain quoted. Native permissions and context limits still apply.

The current settings surface omits noninteractive inventories, unsupported widgets and unavailable controls, including inherited, managed and terminal-only schema entries. Editable settings are grouped by purpose; reset/help is available from the row information menu.
