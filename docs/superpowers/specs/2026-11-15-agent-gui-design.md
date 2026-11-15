# Native agent conversation panels

## Intent and accepted direction

The owner asked for attractive native GUI conversations for Claude Code, Codex and
Antigravity, accepted retaining the terminal panels while adding GUI presentation,
and instructed implementation to start. They additionally require direct native UI
verification: enter a task, observe the actual agent working, tools, approvals and
interruption, not just unit tests or a mock conversation.

## Approaches

1. Render terminal text as chat: cheap but loses tool identity and reliable approval
   responses; rejected because the requested surface is an interactive agent UI.
2. Use provider-supported structured protocols and a shared native view: selected.
   Claude's streaming CLI control protocol, Codex App Server and Antigravity ACP
   retain their installed agent harnesses while Kiln presents their events.
3. Implement a separate model/tool harness: unnecessary replacement of existing
   accounts, MCPs, sessions and agent behavior; outside this change.

## Product behavior

- Existing terminal panes and their launch actions remain usable and unchanged.
- New Task and split menus offer GUI conversation / Terminal presentation. New
  agent panes default to GUI; terminal and SSH choices remain explicit terminals.
- A GUI pane has provider identity, pinned account label, current model, readable
  Markdown conversation, expandable tool cards, file diffs, inline approvals and
  questions, image attachment chips, multiline IME-safe composer and Send/Stop.
- Preserve readable input at 520px pane width and the minimum supported window.
  Respect existing themes, fonts, localization and keyboard focus; no raw protocol
  messages, debug fields or credentials belong in the product UI.
- New GUI conversations are native provider conversations. Retain their IDs and
  history across GUI relaunch, including pending draft text and attachments.
- Restore must never submit a draft, replay a task or approve a request by itself.
- Changing the selected account affects newly opened panels. A running panel keeps
  its original account. Original account/configuration files are not overwritten.
- Preserve the MCP inheritance added in 0.2.26. Do not use Claude bare mode.
- Existing terminal conversation continuation remains available. Converting a
  currently running TUI process in place is not part of this first GUI release.

## Visual direction

Use the selected 12ui direction: existing slate shell, one readable conversation
column, compact bordered expandable tool cards, inline file diffs and approval
card, and a fixed multiline composer. Orange indicates provider/action emphasis;
status also has text/icons so it does not rely on color. Keep the original shell
and real supported model names; synthetic names in the design are placeholders.
At narrow widths, controls wrap rather than creating a second fixed column.

## Runtime ownership

The GUI must not own the agent child. A private `kiln agent-gui-host --id ID`
worker runs inside an ordinary daemon-owned PTY session. It launches the real CLI
with stdin/stdout pipes and translates structured messages; the PTY is a lifecycle
anchor and activity channel, not the conversation renderer. This reuses existing
macOS/Windows persistent session ownership and close/kill semantics without a
second daemon or new public network listener.

The worker and provider remain alive when a window or GUI client closes. Existing PTY kill sends SIGHUP on Unix; that alone is not a process-tree
termination guarantee. The GUI host must explicitly clean its own session process
group on termination, including HUP-resistant children. In production the host
must be the PTY session/group leader before using group signals; never signal a
foreign parent group. Flush terminal state and escalate only that owned group
when needed. Provider children must not escape into another group. Windows uses
the existing Job Object lifecycle. Verify this with child/grandchild fixtures.
Windows launchers must reuse `agent_launch::launcher` and hide stray consoles.

Each session has an owner-only directory below `config/agent-gui/ID`, containing a
credential-free launch configuration, append-only event journal, host lock and
atomic command queue. IDs must be validated before use as paths. Limit reads and
event sizes. The host lock prevents two workers from controlling one conversation.
Background preparation may call the existing account launch environment builder,
including its credential bootstrap. Capture that pinned profile environment once.
The host validates the original profile rather than selecting whichever account is
active later. Resolve connection API-key environment values only in the final launch
worker. Never log or persist those resolved connection values.

Journal commands include unique IDs and a host generation. Reject stale approvals,
duplicates and commands from an older generation. Interrupted or ambiguous sends
must be reported; never silently retry a model prompt. Pending UI approvals are
resolved only by their live matching provider request. EOF or backend exit is an
explicit failure/disconnected state, never an indefinitely animated response.

Restoring an absent/exited daemon session may start the same host ID; validate the
existing native conversation ID and resume it without an extra user turn. A durable
initial-intent marker is written before its first attempted send, so a crash during
initial submission never causes automatic prompt replay. Ambiguous pending commands
from an old worker generation require explicit user retry.

Disk I/O, child I/O, JSON parsing, image reads and journal tailing run off the UI
thread. Deliver bounded batches to the UI; coalesce streaming text and cache
Markdown parsing. Dropping the view disconnects its client, not the worker.

## Provider contracts

- Claude: installed `claude -p --input-format stream-json --output-format
  stream-json --include-partial-messages --verbose --permission-prompt-tool stdio`.
  Implement initialize/control request responses, text and tool events, result,
  interruption, model discovery/selection, AskUserQuestion and real permission
  responses. Preserve CLI settings, working directory and account namespace.
- Codex: installed `codex --no-daemon app-server`; initialize/initialized,
  thread/start or thread/resume, turn/start/interrupt, model/list, streamed item
  events and server-initiated approvals/questions/MCP elicitation. Preserve native
  user policy; do not bypass permissions to make the demo run.
- Antigravity: `agy acp`; initialize, session/new or supported session/load,
  session/prompt/cancel, session/update, model/mode options and request_permission.
  Advertise only filesystem/terminal client capabilities Kiln actually implements;
  leave the agent's own tools in its harness. Reuse the account home/keychain setup.
- Keep protocol-specific IDs and pending requests inside adapters. Expose a shared
  transcript/event model, preserving provider session IDs, tool IDs, errors and
  supported model choices. Unknown messages must not accidentally complete turns.
- Cancellation stops the current turn and resolves pending prompts without killing
  an otherwise reusable agent. A provider that cannot resume must explain that fact
  rather than silently starting another conversation under old history.

## Shared interfaces

Module: `app::agent_gui`. Keep runtime, protocol, journal/client and renderer in
separate focused files. Provider-specific files live under `agent_gui/providers/`.

- `prepare(tool: Tool, cwd: PathBuf, accounts: &AccountManager,
  initial_prompt: Option<String>) -> anyhow::Result<SpawnSpec>` writes the private
  host config and returns the lifecycle spawn spec. It carries `KILN_GUI_AGENT`
  and `KILN_GUI_TOOL` markers, without resolved credentials.
- `descriptor(spec: &SpawnSpec) -> Option<Descriptor>` validates those markers.
  Descriptor contains ID, Tool, cwd and account label for the view.
- `serve(id: &str) -> anyhow::Result<()>` is the hidden host entry point.
- `import_attachment(descriptor: &Descriptor, source: &Path) -> anyhow::Result<Attachment>`
  makes a private immutable image copy off the UI thread. Limit files to 20 MiB,
  verify image signatures/MIME, reject symlinks/non-regular files and ensure the
  final worker only reads attachments inside this runtime.
- `Client::connect(descriptor: Descriptor) -> Client` owns a background journal and
  command worker. `Client::drain() -> Vec<Envelope>` and
  `Client::send(command: Command) -> anyhow::Result<()>` never block on child I/O.
- `Transcript::apply(&Envelope)` reduces events deterministically. Include user and
  assistant messages, tool state/input/output/diff, pending approval/question,
  model choices, provider session ID and explicit lifecycle/error states.
- `Command` supports Submit(text, attachments, model), Respond(request ID,
  selected decision/answers), Interrupt and explicit restart when disconnected.
  Commands are assigned IDs and the current host generation by Client.
- `AgentPane` wraps Client, Transcript, composer and Markdown caches. It implements
  `from_spec`, `tick(ctx)`, `ui(ui)` and draft save/restore. It does not own a Child.

Use an `agent_views: HashMap<PaneId, AgentPane>` overlay on existing Term panes;
this retains existing session IDs and lifecycle machinery. Persist the GUI draft
in an optional `PaneP` field; derive runtime identity from the persisted spawn spec.
Existing saved state with no GUI markers remains a terminal. This is the controller's
implementation decision, not an owner-specified internal architecture.

## Verification and delivery

Use synthetic isolated CLI fixtures for deterministic protocol tests: streamed
text, tool progress, diffs, permission allow/deny, questions, interruption, provider
errors, restart/reconnect and exact-once sends. Include Korean IME events and image
attachment handling. No real credentials, terminal captures or local review images
are committed. Test both account-pinned subscription and connection launch paths.

Native UI verification must exercise all three actual installed engines in an
isolated workspace. Use harmless file operations on fixture files, verify visible
tool activity and resulting file contents, permission response and Stop, then close
and reopen the GUI while a worker remains active. Use official CLI with a local
test provider where supported and a minimal real provider request where necessary;
the owner's request authorizes this practical UI validation. Do not trigger OS
credential/password dialogs, change the owner's login or interrupt real sessions.
Record exact coverage and any external service blockage honestly.

Update English repository docs and all four product guide languages. Review the
whole branch, run required macOS/Windows CI and the release runbook. The existing
session authorization covers merge/release; publish only after required artifact
signing/notarization gates. Report the released version and how to open GUI mode.

## References checked

- https://code.claude.com/docs/en/headless
- https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode
- https://code.claude.com/docs/en/agent-sdk/user-input
- https://github.com/anthropics/claude-agent-sdk-python (control protocol transport)
- https://learn.chatgpt.com/docs/app-server
- https://antigravity.google/docs/cli/acp/
- Installed `agy 1.5.9` ACP help and generated local Codex app-server schema.
- Latest base: origin/main @ 1dff686680e2cb0e1e11a39c36979d409dc71a4d.
