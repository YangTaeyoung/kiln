# Native agent feature coverage

This matrix describes the 0.3.0 source implementation. Reference
versions are Claude Code **2.1.325** / Agent SDK **0.3.325**, Codex CLI
**0.167.2**, and Antigravity CLI **1.5.9**. They are the versions inspected during
implementation, not minimum requirements or a compatibility promise for later
versions. Available models, commands, policies and entitlements come from the
installed provider and selected account. See [the conversation guide](agent-gui.md).

## Read the scope before changing a setting

| Scope or result | Meaning in Kiln |
| --- | --- |
| Current conversation | A typed session operation exists. Native availability, policy and successful settlement are still required. |
| New conversation | An explicit startup form opens a separate pane with the currently selected account. It never restarts the current worker or replays its prompt. |
| Inherited | Native configuration or a dedicated native host workflow owns the feature. Its reference entry is not an editable GUI switch. |
| Managed | Provider or organization policy owns the setting; Kiln does not override it. |
| Terminal only | Use a separate normal terminal conversation for the native workflow. The existing GUI pane and draft remain intact. |
| Acknowledged | The provider accepted a source/session flag without a verified runtime-effect field. This is weaker than Applied. |
| Unconfirmed | Native application has not been established. Send and conflicting settings remain blocked while the operation is unsettled, including after Stop or timeout. |

Opening a panel or selecting a catalog entry never sends a prompt. Commands insert
editable drafts; actions require their own explicit button. Goals and budgets are
never created automatically. Missing discovery, an explicitly empty catalog,
unsupported operations and partial results are separate states. Optional discovery
failure does not end an otherwise usable conversation. Bounded results display
their coverage instead of claiming completeness. These contracts are defined in
[controls.rs](../crates/kiln/src/app/agent_gui/controls.rs#L58) and
[native.rs](../crates/kiln/src/app/agent_gui/providers/native.rs).

## Provider matrix

Each row names the complete supported feature family and its boundary. “Live”
describes an implemented typed route, not proof of authenticated model execution.

| Provider / feature | Native GUI coverage and limit | Implementation / primary reference |
| --- | --- | --- |
| Claude model, effort and Ultracode | Live advertised model aliases/resolved IDs, supported efforts, permission modes and an independent Ultracode toggle. Effort changes preserve Ultracode intent. Requested, active and available are distinct; native caps and workflow policy remain authoritative. | [Claude controls](../crates/kiln/src/app/agent_gui/providers/claude_controls.rs#L129); [model configuration](https://code.claude.com/docs/en/model-config.md) |
| Claude advanced session controls | Live Fast mode; workflows, size and keyword trigger; output style; main agent and advisor; automatic compaction; response language; fallback models; main/subagent cache TTL; prompt suggestions; per-skill exposure; thinking budget/display. Numerical thinking budget is unavailable unless model metadata establishes meaning. Source acceptance without runtime readback remains Acknowledged. No global settings-file write implements these selectors. | [typed setters and readback](../crates/kiln/src/app/agent_gui/providers/claude_controls.rs#L312); [SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript) |
| Claude catalogs and reload | Native commands, skills, agents and output styles retain descriptions, hints, aliases, origin, enabled/builtin and model metadata. Skills/styles have explicit reload. Plugin reload requires the native safe busy-check capability; cache impact can hold it. Terminal-only commands cannot be inserted as working GUI commands. Compaction uses an explicit `/compact` draft and Send. | [catalogs](../crates/kiln/src/app/agent_gui/providers/claude_controls.rs#L24), [actions](../crates/kiln/src/app/agent_gui/providers/claude.rs#L147); [SDK](https://code.claude.com/docs/en/agent-sdk/typescript) |
| Claude status and reference inventory | Safe MCP connection/tool counts, context summary, permission counts/managed state, availability and CLI status. The full 182-key reference catalog is searchable; only the typed live subset above is editable. Startup, inherited, managed and terminal entries remain labeled. Raw settings, auth helpers, environment, headers, hooks and server configuration are not copied into status/history. | [182-key catalog](../crates/kiln/src/app/agent_gui/providers/claude_settings_catalog.json), [safe projections](../crates/kiln/src/app/agent_gui/providers/claude_controls.rs); [settings reference](https://code.claude.com/docs/en/settings-reference.md) |
| Claude startup | New-conversation turn/cost budgets, auto-compact window, system/append instructions, user/project/local setting sources, tool list and allowed/disallowed tools. Existing/resumed workers retain their original launch. | [launch types and arguments](../crates/kiln/src/app/agent_gui/launch.rs#L14); [CLI reference](https://code.claude.com/docs/en/cli-reference.md) |
| Codex model and thread controls | Live model-dependent effort strings/defaults, service tiers/default, collaboration mode, permission profile, approval policy/reviewer, reasoning summary, typed sandbox and granular approval policy. Includes advertised hidden models, modalities and availability/upgrade metadata. `multiAgentMode` is not exposed as a GUI toggle; `ultra` is available only if the native model advertises it as an effort. Native requirements constrain profiles/policies. | [Codex controls](../crates/kiln/src/app/agent_gui/providers/codex_controls.rs#L193); [App Server](https://learn.chatgpt.com/docs/app-server) |
| Codex confirmation and current turn | Thread changes require ACK plus compatible effective settings notification. A mismatch offers explicit acceptance of the visible native state, not a claim that the request applied. A separate current-turn form targets only the reported active turn and uses that turn's model defaults/choices. It does not change inference the provider already captured. | [thread settlement](../crates/kiln/src/app/agent_gui/providers/codex_controls.rs), [turn actions](../crates/kiln/src/app/agent_gui/providers/codex_actions.rs#L75); [App Server](https://learn.chatgpt.com/docs/app-server) |
| Codex skills and status | Native skills/list plus forced refresh, exact name/path skill input on explicit Send; removed/disabled/mismatched entries are rejected. Native MCP state/auth/tool counts, requirements, context and usage are safe projections; last-call usage, total usage and context window stay separate. No generic slash-command catalog is invented. | [discovery](../crates/kiln/src/app/agent_gui/providers/codex_controls.rs#L4), [insert-only catalog](../crates/kiln/src/app/agent_gui/commands_ui.rs); [skills](https://learn.chatgpt.com/docs/build-skills.md) |
| Codex explicit actions | Compaction with native completion; goal get/set/status/budget/clear; queue list/add/edit/delete/start; inline review of uncommitted changes, a base branch, commit or custom instructions; exact active-turn settings; memory enable/disable/read status; MCP reload. Queue edits can run during a turn; Start/review/goal actions require idle state. Optional native rejection stays local and visible. | [action forms](../crates/kiln/src/app/agent_gui/actions_ui.rs), [typed actions](../crates/kiln/src/app/agent_gui/providers/codex_actions.rs#L75); [App Server](https://learn.chatgpt.com/docs/app-server) |
| Codex startup | New-conversation model/effort/tier, permission profile, summary, verbosity, web search, context/auto-compact limits, base/developer instructions, ephemeral persistence and absolute workspace roots. Explicit model-dependent startup values wait for native catalog validation. | [launch types and parameters](../crates/kiln/src/app/agent_gui/launch.rs#L34); [App Server](https://learn.chatgpt.com/docs/app-server) |
| Codex remaining native methods | The 180-method inventory documents typed routes versus inherited/native host workflows. Account/enrollment changes, realtime/audio hosting, global destructive memory reset and other unimplemented host workflows are not dispatched by an inventory row. Plugin/marketplace APIs marked under development have no production dispatch. | [method inventory](../crates/kiln/src/app/agent_gui/providers/codex_method_catalog.json), [dispatch allowlist](../crates/kiln/src/app/agent_gui/providers/codex_actions.rs#L453); [App Server restrictions](https://learn.chatgpt.com/docs/app-server) |
| Antigravity session controls | Live ordered/grouped ACP select options, including model, thought level and mode when advertised. Unknown/missing categories remain searchable; unsupported types remain read-only. Full/empty snapshots replace prior choices. Successful changes require ACK plus a matching full native snapshot; legacy model/mode support remains when config options are absent. | [ACP adapter](../crates/kiln/src/app/agent_gui/providers/antigravity.rs#L281); [config options](https://agentclientprotocol.com/protocol/v1/session-config-options) |
| Antigravity commands and status | Native command names, hints and descriptions insert editable slash drafts; command/usage updates survive native session load. Optional manual discovery and typed action endpoints are unavailable in this adapter; advertised commands run only on Send. No invented Ultracode or MCP-health badge: client-supplied empty MCP configuration is not a health report. | [live/load updates](../crates/kiln/src/app/agent_gui/providers/antigravity.rs#L710); [ACP commands](https://agentclientprotocol.com/protocol/v1/slash-commands), [Google ACP guide](https://antigravity.google/docs/ide/extensions/acp) |
| Antigravity startup | New-conversation sandbox request, main agent and additional absolute directories. Native permissions remain the exact offered decisions. These are startup requests, not a live sandbox-policy editor. | [launch options](../crates/kiln/src/app/agent_gui/launch.rs#L82), [permission decisions](../crates/kiln/src/app/agent_gui/providers/antigravity.rs#L65); [Google ACP guide](https://antigravity.google/docs/ide/extensions/acp) |

## Permissions, tools and lifecycle

All providers keep native permission decisions authoritative. Claude cards show
actual tool/MCP inputs; ACP uses explicitly supplied tool input. Codex cards project
protocol envelopes into action summaries and support exact offered decisions or
selected requested filesystem/network subsets with turn/session scope and strict
review choice. A generic Allow cannot copy an entire native permission request.
See [approval projection](../crates/kiln/src/app/agent_gui/approval.rs),
[subset validation](../crates/kiln/src/app/agent_gui/providers/codex_controls.rs#L1002)
and [the App Server permission contract](https://learn.chatgpt.com/docs/app-server).

Known tool/question headers follow the app language; unknown operations use an
external-tool label. Original names remain inspectable. Arguments, output, model
text, questions and diffs are unchanged. Coarse ACP read/search/fetch categories
describe data operations without inventing a file-specific operation. See
[tool presentation](../crates/kiln/src/app/agent_gui/tool_presentation.rs),
[ACP tool metadata](https://agentclientprotocol.com/protocol/v1/tool-calls) and
[MCP names/display titles](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#tool).

The daemon owns reusable workers; GUI quit saves drafts and disconnects clients.
Pane close ends its session. Restore never resends a prompt. Stop remains available
during settings settlement/replay. Codex command-process termination needs native
confirmation in addition to turn interruption; an unsupported/rejected/timeout
result stays unconfirmed. See [lifecycle and recovery](agent-gui.md).

## Personal extensions and saved accounts

Preparation runs once for a new launch on the existing background account worker,
not during status polls or resume. Source author files remain unchanged. Supported
Claude skills, legacy commands, agents, styles, workflow bundles and self-contained
user-scope plugins are reconciled into the selected private profile. Codex legacy
private skills and validated local cached plugins are prepared; shared HOME skills
remain native, as do project precedence and provider policy. Antigravity assets
remain native rather than being imported by this reconciler. See
[launch preparation](../crates/kiln-accounts/src/manager.rs#L1034),
[scanner](../crates/kiln-accounts/src/extensions/scan.rs#L177),
[Claude loading rules](https://code.claude.com/docs/en/plugins/loading.md) and
[Codex plugin schema](https://github.com/openai/codex/blob/ce21072c90d32c40af7a55b60b2fb35c84a43a6a/codex-rs/core-plugins/src/agent_plugin_manifest.rs).

Typed portable plugin declarations can retain command/interface metadata and safe
bundled hooks, MCP and LSP configuration. Preparation does not execute those
components. Actual user configuration, credential-bearing headers/environment,
account-linked connectors, unknown schemas, external paths/symlinks and unsupported
dependencies require native setup or show a failure. Explicit marketplace
declarations outside importer ownership can require native setup. Routines,
scheduler registrations/state and synced assets do not gain parity through a file
copy. Existing credential/bootstrap/MCP behavior is preserved; this reconciler does
not mirror the entire user configuration, credentials or history. See
[component validation](../crates/kiln-accounts/src/extensions/components.rs#L384)
and [Claude manifest rules](https://code.claude.com/docs/en/plugins/manifest-reference.md).

Profile edits and deletions remain authoritative, including across source version
changes. Only unchanged Kiln-owned imports advance; retained cache versions remain
for readers, with no implicit garbage collection or forced worker reload. Shared
personal paths and directory-marketplace payloads still follow native mutable
scope; retention is not a universal immutable-reader guarantee. The UI
distinguishes **Extension prepared**, **Existing extension retained**, native scope,
profile override/deletion, source removal, absent, unsupported and failed states.
Prepared/retained proves neither enabled nor loaded nor executed. Native catalog
discovery, loaded status and successful execution require their own evidence. See
[ownership](../crates/kiln-accounts/src/extensions/ownership.rs),
[report states](../crates/kiln-accounts/src/extensions/mod.rs#L126) and
[native plugin loading stages](https://code.claude.com/docs/en/plugins/loading.md).

## Verification boundary

Source tests, generated-schema checks, inert native catalog discovery and local
synthetic inference are distinct from authenticated provider execution. This
matrix does not certify account entitlement, managed-policy behavior, native
visual/physical-IME acceptance, Windows runtime, signed archives or publication.
The [dated native verification record](maintainers/0.3.0-status.md) documents the
observed Claude/Codex native controls and tool execution with loopback inference,
and Antigravity's authenticated native read task. It also retains the unexecuted
Codex permission-profile change, unresolved physical IME and incomplete cleanup.
The [0.3.0 notes](releases/0.3.0.md) keep delivery status explicit.
