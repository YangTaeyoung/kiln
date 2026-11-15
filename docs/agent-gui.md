# Native agent conversations

Choose **New Task → Conversation GUI → Claude Code, Codex or Antigravity**. New
agent launches use GUI mode by default. The same GUI/Terminal choice is available
in the right/down split menu and **Settings → Behavior → New agent presentation**.
An ordinary terminal, SSH connection, existing saved terminal, and keyboard split
remain terminals. A running TUI conversation is not converted in place.

The native view uses the installed official CLI and the account selected when the
pane starts. Changing the active account affects future panes. Existing CLI MCP
configuration and native permission defaults remain in effect. Models and modes
are shown only when advertised by that provider. The header shows the confirmed
effective model/mode; a composer model choice is a request for the next send.

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

## Implementation references

`app/agent_gui/view.rs` owns the native presentation and draft reconciliation.
`app/agent_gui/client.rs` handles bounded background journal/command transport.
The `agent_views` map overlays existing Term panes; launch-spec GUI markers select
that view on restore. GUI teardown drops only the client. Markdown parsing,
attachment I/O and canvas decoding run on background tasks.

The view uses egui 0.36.2 [ScrollArea](https://docs.rs/egui/0.36.2/egui/containers/scroll_area/struct.ScrollArea.html)
and its standard TextEdit IME path. The shipped empty navy canvas is the unchanged
selected design asset `clean-0-71f28052a152.png`; interactive text and cards remain
native widgets. It is used in dark mode; light mode follows the current theme.
