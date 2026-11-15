# Native agent GUI component notes

Recorded from `996701312f834e262839f814d8b0ec69b2fb9931` on 2026-11-15. These notes extend the existing [Graphite Workbench design system](../DESIGN.md) and [product commitments](../PRODUCT.md); they document implemented components, not a replacement visual identity. See [agent GUI behavior](agent-gui.md) for runtime and recovery rules. Dimensions below are egui logical points.

## Theme and reading hierarchy

Consume `Theme::current()` and its semantic surface, text, border and status roles. The shared typography remains Pretendard for interface/prose and JetBrains Mono for technical content: body 13.5, controls 13, metadata 11.5. Provider marks are existing native vectors: Claude uses orange, Codex blue and Antigravity purple. Keep account, status and applied model/mode in the compact, wrapping header; show actual runtime values.

Sources: [font assets](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/fonts.rs#L12), [type scale](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/theme.rs#L276), [provider marks](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L347), [header](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L585).

## Conversation and tools

The conversation and composer share a centered column capped at 900 points, with 40 points removed from available width before centering. The history scrolls independently above the composer. User messages use a quiet elevated fill, 6-point corners and 12-point padding; assistant text uses a transparent surface and Markdown rendering. Preserve the workspace and task chrome around this column.

A tool has one 1-point border and 6-point corners. Its full-width collapse header shares that boundary; it has no second outlined button inside the card. The real status sits at the right, with title truncation reserving its space. Expanded input/output and diffs scroll in a region capped at 260 points. Technical content is monospace; diffs retain explicit plus/minus characters as well as semantic red/green.

Sources: [column and messages](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L629), [composer allocation](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L668), [tool card](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L693).

## Composer

One outer surface uses `bg_input`, a 1-point `border_strong`, 12-point corners and 12-point padding. The three-row multiline editor has no inner frame. Focus adds a 1.5-point accent stroke at 0.7 tint around the outer surface. Keep the attachment plus button, model/mode selectors and Send/Stop inside this surface, with a subordinate shortcut hint below them.

The attachment control has a 28-point target and an accessible name. Model and mode selectors allocate 125 and 105 points respectively, use rounded quiet fills, and expose available provider choices. Send uses the native arrow and primary button; Stop uses the native stop mark and secondary button. Both use the shared 32-point button height. Below 400 points of composer content width, controls wrap and the action moves to its own right-aligned row. The outer layout reserves additional height below a 440-point pane width, supporting the 320-point fixture without clipping its action or outer border. Retain Enter/Shift+Enter copy and the IME submit guard when refining geometry.

Sources: [outer frame/editor](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L993), [responsive footer and focus](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L1136), [selectors](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L1213), [actions](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L1299), [button dimensions](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/widgets.rs#L209).

## Pending decisions

A pending request uses one panel-colored card with a strong 1-point border, 6-point corners, 12-point padding and an orange warning icon/title. Actual action details and domain arguments precede decision controls. Tool-input values are monospace; other labels remain proportional. The summary scroll area reserves and caps its viewport at 180 points while allowing short content to shrink. This explicit allocation preserves painted short arguments after a long conversation. Long arguments remain scrollable rather than pushing decisions into an unbounded region.

Sources: [request card and summary allocation](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L811), [full-history painted-content regression](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view_tests.rs#L1318).

## New Task menu and completion toast

The ghost New Task trigger gains a quiet selected fill while open. The menu has 268-point content width, reduced for narrow viewports, with 8-point margins (about 284 points outside), 12-point corners, a quiet border and the shared native shadow. GUI/Terminal are equal segments in one 32-point track. In light theme the track uses `bg_hover`; selection uses `bg_elevated` plus a strong border. Provider/action rows are full-width 40-point targets with 18-point vector marks and medium 13-point labels. Thin dividers separate provider, request/terminal and folder groups; existing launch actions remain authoritative.

The toast stack anchors 16 points from the top-right edge, 12 points below the toolbar. Each card uses 316-point content width capped by viewport width minus 56, 12-point horizontal/8-point vertical padding, 10-point corners and the shared shadow. A 16-point semantic icon leads a 13-point title; the body uses 11.5-point dim text. Short viewports combine title/body, and compact toasts retain title activation or an explicit action alongside close. Keep full information available through the existing hover/notification behavior.

Sources: [menu](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L28), [presentation segments](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/agent_gui/view.rs#L18), [toast](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln/src/app/ui.rs#L177), [shared shadow](https://github.com/YangTaeyoung/kiln/blob/996701312f834e262839f814d8b0ec69b2fb9931/crates/kiln-common/src/theme.rs#L197).

## Verification boundary

The final visual review disposition was **ship** for the scoped source/harness correction at `9967013`. Its approval-summary fix was **Approved** in dark/light full-pane renders at 1400/520 widths with short/long arguments. Earlier actual native dark conversation/menu and light conversation/Stop captures informed the component record. They precede the final approval fix and do not establish corrected native approval visibility or a real decision result. That actual native gate remained **CannotVerify** when these notes were written. Harness geometry, native interaction, physical IME input and provider outcomes remain separate evidence.
