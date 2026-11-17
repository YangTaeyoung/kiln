# Native agent GUI component notes

Updated on 2026-11-17 for the compact composer, settings, command catalog and presentation handoff. Historical links pinned to `996701312f834e262839f814d8b0ec69b2fb9931` record the earlier baseline; the current sections link to source files and name their entry points. These notes extend the existing [Graphite Workbench design system](../DESIGN.md) and [product commitments](../PRODUCT.md); they document implemented components, not a replacement visual identity. See [agent GUI behavior](agent-gui.md) for runtime and recovery rules. Dimensions below are egui logical points.

## Theme and reading hierarchy

Consume `Theme::current()` and its semantic surface, text, border and status roles. The shared typography remains Pretendard for interface/prose and JetBrains Mono for technical content: body 13.5, controls 13, metadata 11.5. Provider marks are existing native vectors: Claude uses orange, Codex blue and Antigravity purple. Keep account, status and applied model/mode in the compact, wrapping header; show actual runtime values.

Sources: [font assets](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/fonts.rs#L12), [type scale](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/theme.rs#L276), [provider marks](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L347), [header](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L585).

## Conversation and tools

The conversation and composer share a centered column capped at 900 points, with 40 points removed from available width before centering. The history scrolls independently above the composer. User messages use a quiet elevated fill, 6-point corners and 12-point padding; assistant text uses a transparent surface and Markdown rendering. Preserve the workspace and task chrome around this column.

A tool has one 1-point border and 6-point corners. Its full-width collapse header shares that boundary; it has no second outlined button inside the card. The real status sits at the right, with title truncation reserving its space. Expanded input/output and diffs scroll in a region capped at 260 points. Technical content is monospace; diffs retain explicit plus/minus characters as well as semantic red/green.

Sources: [column and messages](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L629), [composer allocation](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L668), [tool card](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L693).

## Composer

Use one quiet outer `bg_input` surface with a strong border, restrained corner radius
and an unframed two-row editor for the conversation. Settings and catalog surfaces
can use a one-row companion editor. Focus draws one accent boundary around the
composer. Do not add a second boxed panel around its controls.

One toolbar contains the attachment control, model, mode, Reasoning settings and a
28-point round Send/Stop control. The primary action reserves its own space at the
right. Below 560 points of toolbar content width, retain the model and place the
remaining controls in Conversation options. The toolbar stays on one row. Attachment
chips and validation errors can increase height; a suggestion list must not.

Reasoning settings contains the provider's advertised effort levels as an ordered
step slider and Claude's independent Ultracode switch. Compact Conversation options
contains those controls together with mode. Use purple to signal enabled Ultracode;
its indicator is native vector artwork rather than a font-dependent character.
Readable model names and localized mode/effort labels retain exact native values.
The header displays confirmed runtime values, separately from requested changes.
Keep the subordinate Enter/Shift+Enter hint and the IME submit guard.

Sources: [composer and round action](../crates/kiln/src/app/agent_gui/view.rs)
(`composer_ui`, `composer_action_ui`, `composer_icon`),
[responsive options](../crates/kiln/src/app/agent_gui/settings_ui.rs)
(`composer_options_ui`), and
[effort and Ultracode](../crates/kiln/src/app/agent_gui/settings_presentation.rs)
(`effort_slider`, `ultracode_switch`).

## Settings and command discovery

Conversation, Session settings and Commands and skills use quiet text navigation
with a thin selected underline. Search controls use the shared input surface,
vector search icon and explicit clear action. GUI settings omit terminal-only
controls, preserve the remaining scope labels and start Change history collapsed.
Model and mode choices retain native descriptions so the reader can inspect what
a selection means before choosing it.

The command/skill catalog scrolls inside a bounded region. Each compact row has a
thin expansion chevron, a left-aligned name, one-line description preview and a
right-aligned Insert into draft action. Reserve the row's full rectangle first:
intrinsic control widths must not grow successive rows. Expanded descriptions and
metadata remain inside the catalog's scrolling boundary. Terminal-only entries and
Codex skills without a native path are hidden from the invocable GUI catalog.

Completion is a floating list above the editor with short descriptions, a visible
active row and its own bounded scroll. Up/Down changes selection; Tab/Enter inserts
without submitting; Esc dismisses until the token changes. Recognized command/skill
tokens use an accent treatment in the editor. Reserve Tab/Esc in the focused
editor's event filter while the list is open, because egui processes focus
navigation before the completion handler runs. Preserve IME guards and restore the
cursor after the inserted token.

Sources: [settings and navigation](../crates/kiln/src/app/agent_gui/settings_ui.rs)
(`surface_navigation`, `settings_ui`),
[catalog and completion](../crates/kiln/src/app/agent_gui/commands_ui.rs)
(`command_row`, `slash_completion_input`, `slash_completion_ui`, `command_layouter`),
and [geometry and painted-text checks](../crates/kiln/src/app/agent_gui/commands_tests.rs).
The focus filter follows egui's [EventFilter contract](https://docs.rs/egui/0.36.2/egui/struct.EventFilter.html).

## Presentation ownership

GUI/Terminal changes reuse the confirmed native conversation with its original
account environment and working folder. The host must be idle, free of pending
approvals/tools/queued work, and exclusively own its backend before opening the
native terminal. Additional background helpers cause refusal; they are not killed
for a presentation switch.

A return to GUI waits for natural native CLI exit. A tab click records the request
without synthesizing termination or input. After exit, appended user/assistant text
is imported only if the prior native-history prefix still matches. Image notices
can appear in that text, but native tool cards and image payloads are not rebuilt.
Older terminal tasks require an exact tracked native conversation and daemon-confirmed
PTY exit before import. Missing or disconnected session state is not exit proof.

Sources: [handoff and prefix validation](../crates/kiln/src/app/agent_gui/presentation.rs)
(`can_handoff`, `serve_terminal`, `import_terminal_history`),
[exclusive child ownership](../crates/kiln/src/app/agent_gui/host.rs),
[recorded-conversation preparation](../crates/kiln/src/app/agent_gui/mod.rs)
(`prepare_existing_native`), and
[daemon exit gate](../crates/kiln/src/app/mod.rs) (`tick_agent_gui_imports`).

## Pending decisions

A pending request uses one panel-colored card with a strong 1-point border, 6-point corners, 12-point padding and an orange warning icon/title. Actual action details and domain arguments precede decision controls. Tool-input values are monospace; other labels remain proportional. The summary scroll area reserves and caps its viewport at 180 points while allowing short content to shrink. This explicit allocation preserves painted short arguments after a long conversation. Long arguments remain scrollable rather than pushing decisions into an unbounded region.

Sources: [request card and summary allocation](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L811), [full-history painted-content regression](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view_tests.rs#L1318).

## New Task menu and completion toast

The ghost New Task trigger gains a quiet selected fill while open. The menu has 268-point content width, reduced for narrow viewports, with 8-point margins (about 284 points outside), 12-point corners, a quiet border and the shared native shadow. GUI/Terminal are equal segments in one 32-point track. In light theme the track uses `bg_hover`; selection uses `bg_elevated` plus a strong border. Provider/action rows are full-width 40-point targets with 18-point vector marks and medium 13-point labels. Thin dividers separate provider, request/terminal and folder groups; existing launch actions remain authoritative.

The toast stack anchors 16 points from the top-right edge, 12 points below the toolbar. Each card uses 316-point content width capped by viewport width minus 56, 12-point horizontal/8-point vertical padding, 10-point corners and the shared shadow. A 16-point semantic icon leads a 13-point title; the body uses 11.5-point dim text. Short viewports combine title/body, and compact toasts retain title activation or an explicit action alongside close. Keep full information available through the existing hover/notification behavior.

Sources: [menu](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L28), [presentation segments](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L18), [toast](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L177), [shared shadow](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/theme.rs#L197).

## Verification boundary

The visual review disposition was **ship** for the scoped source/harness correction at `9967013`. Its approval-summary fix was **Approved** in dark/light full-pane renders at 1400/520 widths with short/long arguments. Subsequent actual native light approval/result/menu captures closed the earlier **CannotVerify** approval-visibility gate: actual MCP arguments and both decisions were readable before the controller selected one-time Allow, and the native result showed the matching completed echo. The scoped actual-native review was **Approved**, with a ship disposition. This is installed Claude plus local loopback inference and a real stdio MCP fixture, not authenticated remote Claude inference or whole-feature/release approval. See [current native verification](maintainers/0.3.0-status.md) for the later expanded controls/tools, authenticated Antigravity result and unresolved physical IME boundary.

The current compact composer, command catalog and settings changes are verified by
source and egui harness geometry/renders. Those checks do not establish physical
IME behavior, a real provider handoff or authenticated provider outcomes. Keep the
historical native evidence above separate from the latest UI candidate. See
[provider coverage](agent-feature-coverage.md) for control availability.
