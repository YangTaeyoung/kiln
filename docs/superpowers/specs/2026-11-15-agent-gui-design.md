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

## Amendment: complete native agent controls (owner request, November 15)

The owner explicitly expanded the original model-only scope to each AI's detailed
features, including effort and native Claude Ultracode. This amendment supersedes
any earlier narrow settings scope. The unpublished 0.3.0 candidate is held until
this work and native UI verification finish; prior artifacts are not new proof.

### Feature visibility and execution

- Preserve provider-advertised model metadata, choices, groups, descriptions,
  commands, skills, config options and current state. Keep account-dependent
  catalogs dynamic. Missing capability, explicit empty list, unsupported operation,
  pending intent, policy denial and confirmed applied state are distinct.
- Use typed safe controls, not an arbitrary JSON/settings editor. Preserve every
  advertised ACP option, including unknown categories. Unknown types remain visible
  as unsupported with their names/descriptions. Full snapshots, including empty
  ones, replace old catalogs; selection-only events must not erase them.
- A configuration command has a unique generation-bound identity. Serialize config
  operations with Submit. A successful write is not an applied setting: require the
  native ACK and effective readback/notification where the protocol provides one.
  Publish requested versus applied state separately. Optional discovery failures
  must not terminate a usable conversation. Keep errors local to the operation.
- Stop cancels deferred prompts, not already applied configuration. Retired replies
  and a fresh readback/reconfirmation barrier prevent stale confirmation and an
  O-to-A-to-Stop-to-O race. No prompt replay, auto retry or guessed rollback.
- Changing model must reconcile dependent effort, tier, mode and availability.
  A stale unsupported effort must never silently override a newly selected model.
- Settings are session-local. Do not write native global files to implement a
  session selector. Startup-only values must be labeled and routed through explicit
  new-conversation launch options, with effective launch metadata displayed. Never
  apply them by interrupting/restarting an existing task or replaying a prompt.

### Claude Code

Use installed 2.1.325 / SDK 0.3.325 as verified references, not guessed minimum
versions. The research and full 182-key inventory live in the SDD workspace.

- Model, supported effort levels, permission mode and independent Ultracode must
  be directly discoverable. Preserve Ultracode intent when effort changes by sending
  the compound flag patch. Read applied.ultracode/Requested/Available and applied
  effort, not only merged effective settings. Show policy caps/unavailable reasons.
- Typed advanced controls: Fast mode, thinking mode/budget/display where meaningful,
  dynamic workflows and size/keyword trigger, output style, main-thread agent,
  advisor, automatic compaction, response language, fallback models, cache TTLs,
  prompt suggestions, and per-skill exposure. Merge skill override maps safely.
  Native acceptance without a runtime-effect field is labeled requested/source
  state; never invent an applied-effect readback. Initial load never opts into a
  paid service or changes policy merely to discover capability.
- Discover commands/skills/agents/styles, support native reload operations and
  command-list changes. Native commands insert editable drafts and execute only on
  Send. Terminal-only commands are marked; unknown slash text may still be sent
  explicitly. Native MCP/tool/permission/context status uses a bounded safe
  projection; raw settings, auth helpers, env, headers, hooks and server configs
  never enter the event journal or transcript.
- Auto-compact window and verified creation-only budgets/system instructions/settings
  sources belong to new-conversation settings. Offer the normal Terminal launch for
  terminal-only configuration. The complete upstream typed setting inventory is
  searchable, with live/new-session/inherited/managed/terminal scope and documentation
  links; non-editable settings must not masquerade as working GUI toggles.

### Codex

- Preserve each model's advertised effort strings/default, service tiers/default,
  modalities, descriptions and relevant availability. Do not hardcode reasoning
  strings. Support native thread settings and effective settings notifications.
- Expose native default/plan collaboration with selected model and effort explicitly
  merged into collaboration settings, preserving their protocol precedence. Expose
  native permission profiles, approval policy/reviewer and sandbox constraints with
  policy availability. Never silently select a bypass policy.
- Use current native capabilities for multi-agent behavior: the installed schema's
  deprecated/ignored multiAgentMode is not a working toggle; model-advertised effort
  such as ultra remains available when the native catalog offers it.
- Discover skills and insert native skill inputs (name/path) on explicit Send;
  support skill refresh/invalidation, MCP health, context usage, explicit compaction
  and other verified native actions. Show startup/inherited settings and optional
  unavailable endpoints honestly. Do not production-call plugin APIs that official
  documentation labels under development.

### Antigravity

- Retain the complete ordered ACP configOptions snapshot, grouped choices, current
  values and optional categories. Installed 1.5.9 advertises model, thought_level
  (low/medium/high) and mode; these are runtime data, not static allowlists.
- Generic set_config_option uses exact advertised configId/value. Matching ACK
  consumes the returned full snapshot, including dependent option changes. Prefer
  config options over legacy modes; preserve legacy-only servers.
- Retain available_commands_update and usage_update during session load as well
  as live use. The actual catalog includes goal, compact, schedule, browser, plan,
  grill-me, teamwork, learn, boost, agy-customizations, antigravity-guide,
  run-workflow and swarm. Menus use actual native metadata, not a maintained list.
- Do not invent AGY Ultracode or MCP health from empty client-supplied mcpServers.
  Keep native permission choices and startup-only sandbox policy distinct.

### Extension inheritance

Saved account isolation must not hide the user's extension assets. Add a bounded,
profile-safe reconciliation of native user skills, legacy commands, agents, output
styles, workflows/routines and plugin metadata/assets where verified paths exist.
Keep profile-local overrides/deletions and project precedence; source changes and
deletions update only unchanged Kiln-owned imports. Never share/copy the entire
config namespace, auth/history, account/provider settings or original credentials.
Use synthetic global/profile/project fixtures and manifests containing fingerprints,
not secret values. Source extension assets stay unchanged. Unresolvable plugin
paths must yield visible errors rather than false parity claims.

### Native presentation and acceptance

Keep the approved graphite native identity and the existing light theme. Compact
quick controls beside the composer cover model/effort/mode and independent
Ultracode where applicable; wrap at 320pt. A labeled Session settings control opens
a searchable, organized surface for every additional native feature. Commands and
skills have a discoverable searchable palette, short descriptions and insert-only
behavior. MCP/context status belongs in the same understandable settings surface.
No system version strings or raw protocol fields in the normal task flow.

Acceptance requires parser/reducer/transport tests for native metadata, full
replacement, effective ACK/readback, unsupported queries, dependent choices,
cancel/stale reply barriers, no auto-send, and no sensitive projections. UI tests
must drive actual interactions and meaningful resulting commands at normal/narrow
widths in both themes. Finish with actual isolated native GUI selection, observed
confirmed native state and a task executed under the selected settings for all
three providers, plus an exclusive Korean IME trial. Synthetic inference and real
authenticated provider proof remain separately reported. User interaction takes
priority; do not overwrite or close the QA app while the owner is using it.

References: [Claude model controls](https://code.claude.com/docs/en/model-config.md),
[Claude SDK types](https://code.claude.com/docs/en/agent-sdk/typescript),
[settings reference](https://code.claude.com/docs/en/settings-reference.md),
[Codex App Server](https://learn.chatgpt.com/docs/app-server),
[ACP config](https://agentclientprotocol.com/protocol/v1/session-config-options),
[ACP commands](https://agentclientprotocol.com/protocol/v1/slash-commands).

### Amendment: localized tool presentation (owner's latest screenshots)

The owner additionally requests tool-call names and action summaries in the app's
selected language, including `AskUserQuestion` and `mcp__kiln_qa__echo`. Centralize
presentation naming for all three providers, tool cards, approval/question cards
and accessibility labels. Known native operations use the existing four-language
catalog (for example AskUserQuestion → 사용자에게 질문). MCP server/tool identity is
separate from its display title: use a meaningful localized known operation with
readable server identity (for example Kiln QA · 메시지 반환), supported native display
metadata when available, and a truthful localized external-tool fallback otherwise.
Do not invent a specific operation from an unknown name or translate unknown tools
by sending private metadata to an external translation service.

Original invocation names/IDs remain unchanged in the protocol and stored history;
show the original tool name in expanded details/tooltips for inspection. Do not
translate commands, file paths, arguments, user questions, model output or diffs.
Locale changes update existing tool headers immediately without rewriting history.
Test the supplied examples and common native tools in en/ko/ja/zh-CN, unknown MCP
names, exact original identifiers, unchanged approval behavior and visible native
header/details at normal/narrow widths. Existing code currently falls back to raw
tool.name for Claude/ACP and MCP command calls in `approval.rs::tool_title`.

Reference: [MCP tool names and display titles](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#tool).


## Windows extension filesystem correction (November 16)

Native x64 and ARM64 acceptance disproved the original directory share-read
assumption: retained-root publication fails and an already-open empty directory
can acquire a junction. Extension reconciliation must therefore use opened-object
relative filesystem operations throughout the Windows backend. This amendment
replaces the method-level requirement to deny every directory write-open with the
following observable preservation contract; it does not relax account isolation.

- Bootstrap a supported drive/UNC namespace root once, then descend by validated
  single components relative to retained handles. Reject unsupported namespace
  forms explicitly. Reject reparse descent; `Dir.path` is diagnostic only after
  anchoring. Use typed windows-sys NT bindings and noninheritable synchronous RAII
  handles. Ordinary ancestors share read/write but never delete; retain the whole
  chain, validate opened objects and preserve object identity.
- Every Windows child operation (creation, metadata, read, lock, enumeration,
  rename and recursive deletion) must operate on the accepted object or fail
  closed after a live root/ancestor reparse mutation. It must never traverse or
  alter the external junction target. Existing held regular-file reads retain
  protection from data writes, append and deletion. Directories remain pinned
  against external rename/delete/replacement.
- Preserve readonly source namespaces, bounded scanning, exclusive creation,
  local overrides, tombstones, ownership fingerprints, private staging outside
  native scanners, journal/backup/publish/recovery ordering and account pinning.
  An unowned destination appearing during publication must survive and prevent
  takeover. No share-delete/guard-drop workaround, POSIX replacement/delete,
  forced external-handle closure, auth/config/history copy or native-default
  fallback is allowed.
- Serialize entire directory scans across aliases; reject malformed/truncated
  records, errors and incomplete inventories. Preserve current scan bounds
  (16,384 files, depth 16, 256 MiB, five-second cooperative deadline). A duplicate
  handle does not create an independent scan cursor. Recursive removal acquires
  one DELETE-capable mutation-root handle, avoiding self-blocking duplicates.
- Keep all existing checked file-data/journal flushes and ordering points. The
  supported recovery guarantee is process interruption/restart, with a bounded
  Windows backup-to-publish absence window. Unix directory fsync remains intact.
  Windows directory-entry persistence across sudden power loss is unverified and
  must not be advertised; do not add admin/volume flushes or swallow a failed
  supported flush. This is a precise limit, not permission to erase recovery
  intent on failure.
- Acceptance requires actual x64 and ARM64 publication/recovery and controlled
  post-validation root/ancestor retarget effects, with successful unguarded
  junction controls and unchanged external canaries. Cross-compilation alone is
  insufficient. Existing product test gates remain enabled; no blanket ignores.

References: [NtCreateFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile),
[NtQueryDirectoryFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfile),
[FileDispositionInformation](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information).
