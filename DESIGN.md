---
name: Kiln
description: A graphite and ice-blue native development workbench.
colors:
  primary: "#91b4ff"
  primary-foreground: "#101726"
  canvas: "#14171c"
  panel: "#1a1e25"
  elevated: "#232832"
  hover: "#2b323e"
  selected: "#303c50"
  input: "#161b22"
  border: "#2b323e"
  border-strong: "#3b4656"
  text: "#ecf0f7"
  text-dim: "#b2bdcf"
  text-faint: "#a1a6b3"
  success: "#5fd08c"
  error: "#ff8383"
  warning: "#f2c46d"
  information: "#5eb1ff"
  purple: "#c49bff"
  attention: "#ff9f5a"
typography:
  heading:
    fontFamily: Pretendard
    fontSize: 18px
    fontWeight: 600
  body:
    fontFamily: Pretendard
    fontSize: 13.5px
    fontWeight: 400
  button:
    fontFamily: Pretendard
    fontSize: 13px
    fontWeight: 500
  small:
    fontFamily: Pretendard
    fontSize: 11.5px
    fontWeight: 400
  mono:
    fontFamily: JetBrains Mono
    fontSize: 13px
    fontWeight: 400
rounded:
  sm: 5px
  md: 8px
  lg: 12px
  control: 7px
  card: 10px
spacing:
  item-x: 8px
  item-y: 6px
  window: 14px
  indent: 16px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-foreground}"
    typography: "{typography.button}"
    rounded: "{rounded.control}"
    height: 32px
  button-secondary:
    backgroundColor: "{colors.elevated}"
    textColor: "{colors.text}"
    rounded: "{rounded.control}"
    height: 32px
  input:
    backgroundColor: "{colors.input}"
    textColor: "{colors.text}"
    rounded: "{rounded.control}"
    padding: 4px 8px
  setting-group:
    backgroundColor: "{colors.elevated}"
    rounded: "{rounded.card}"
    padding: 6px 14px 6px 16px
---

# Design System: Kiln

## Overview

**Creative North Star: "The Graphite Workbench"**

Kiln is a persistent native development workspace for developers using agents and terminals together with code and repository inspection. Graphite surfaces, ice-blue interaction states and restrained attention colors remain the visual identity. Korean copy should identify the action or decision, without filling working space with implementation explanations.

The primary context is a parent folder containing multiple repositories. Workspaces switch on the left, tasks run in the center, and files, changes and GitHub are inspected on the right. An agent can work across frontend and backend repositories without opening each as a separate workspace. Agent requests and ordinary terminals share the visible **새 작업** entry point. See [product purpose](PRODUCT.md) and [titlebar implementation](crates/kiln/src/app/ui.rs:177).

The application icon remains the ivory tile, graphite kiln mouth and copper ember defined by [Kiln.svg](assets/Kiln.svg). It is separate from the ice-blue interaction accent. Do not redraw the brand or introduce new colors as part of a workflow adjustment.

This document was refreshed from source on **2026-10-01**. Frontmatter records the existing Kiln Dark palette and shared primitives, with portable `px` notation representing egui logical points. The `text-faint` and `error` entries reflect the current shared theme; this documentation refresh does not change application colors. Other themes retain their own semantic values. [Theme source](crates/kiln-common/src/theme.rs:47) is authoritative.

**Key characteristics:**

- Stable parent-folder context and task/panel identity.
- A visible path from starting work to reviewing changes and returning to attention.
- Quiet healthy state, reported activity, and actionable errors.
- Flat working surfaces, compact controls, and explicit keyboard focus.
- Preserved drafts and access to the original request after execution.

**Evidence scope.** Current source and headless rendered/interaction fixtures support this contract. Directly inspected examples include [320-point composer](docs/reviews/2026-10-01-workflow/composer-320-dark.png), [320-point GitHub overview](docs/reviews/2026-10-01-workflow/github-320-dark.png), and [original request at 720 pixels / 130% scale](docs/reviews/2026-10-01-workflow/original-request-720-130.png). These are harness renders, not native macOS screenshots. Earlier native application inspections are separate evidence; they do not prove that every latest change has passed installed-app, multi-monitor, VoiceOver, Linux or Windows verification. This document does not certify commercial readiness or complete accessibility.

The structure follows the [DESIGN.md format specification](https://raw.githubusercontent.com/google-labs-code/design.md/main/docs/spec.md). Native egui widgets remain the implementation authority; no browser component sidecar is introduced by this refresh.

## Colors

The default palette uses cool graphite neutrals with an ice-blue interaction accent. Preserve semantic roles when changing themes.

### Primary

**Ice blue** (`primary`) identifies primary actions, focused controls, selected tool accents and text cursors. `primary-foreground` provides dark text on filled accent buttons.

### Semantic color

**Warm orange** (`attention`) identifies agent attention and notification badges. Green, red and amber communicate success, errors and warnings; blue and purple remain available for information and technical distinctions. Terminal ANSI colors are a separate source palette in `Theme`, not a replacement for application control semantics.

**The Attention Rule.** Keep keyboard focus and pending attention distinguishable. Cards with pending attention use orange outlines; focused split cards use translucent ice-blue outlines. Attention takes precedence when both apply.

### Neutral

`canvas` is the terminal/editor working surface. `panel` supports rails and docks; `elevated` supports settings groups, notification items and controls. `input` recesses editable fields. `hover` and `selected` provide interaction feedback, while the two border roles separate quiet containment from interactive edges. `text`, `text-dim` and `text-faint` establish reading priority.

Kiln Dark, Midnight, Ember and Kiln Light are implemented in [theme.rs](crates/kiln-common/src/theme.rs:44). New components should consume `Theme::current()` rather than hardcode the default palette.

## Typography

Pretendard supplies regular, medium and semibold interface text. JetBrains Mono supplies regular and bold technical text, with Pretendard in its fallback chain for Korean. Platform symbol and installed Nerd Font fallbacks are loaded when available; their presence is not guaranteed. See [font registration](crates/kiln-common/src/fonts.rs:93).

The frontmatter captures the global heading, body, small and monospace defaults and the shared custom button role. Compact buttons use medium text (12.5 logical points); section labels generally use medium or semibold text (12–12.5 points). Terminal font size is user-controlled and must not be treated as fixed to the global monospace role.

**The Reading Order Rule.** Keep tool and project names stronger than paths, timestamps and shortcut hints. Use monospace for technical content, while interface explanations retain the Korean-capable proportional family. Do not introduce display typography into the working canvas.

## Layout

A single 42-point titlebar contains the workspace-sidebar toggle, **새 작업**, task tabs, **변경** review action, inspector toggle, command search, notifications and settings. The new-work menu offers **에이전트 요청** and **터미널**. The changes action opens the current workspace's Changes inspector directly and includes a count when known and nonzero. Neither path requires opening a secondary tool menu. Native traffic-light space is reserved outside fullscreen. Active tabs scroll into view on selection and viewport-width changes. [Shell source](crates/kiln/src/app/ui.rs:177).

The sidebar selects workspaces; it has **no tool footer**. Compact expanded widths are 144–160 logical points below a 900-point viewport; larger layouts allow 180–320, and collapsed width is 48. Main and linked worktrees use Git-reported identity and group beneath the main repository or containing parent workspace. Paths and secondary metadata belong in tooltips or detail views. Secondary tools, layouts and recovery are available through named commands and workspace context menus. [Sidebar dimensions and work-surface return](crates/kiln/src/app/mod.rs:1230).

The inspector defaults to 340 points, has a 300-point minimum, and caps its width at 45% of available workspace width within a 300–560-point range. It opens beside the canvas when space allows. Below 700 points of available workspace width, it occupies the work surface and exposes **작업으로 돌아가기**. New tasks and task selection return to the canvas in this compact mode; they preserve the dock in larger windows. Content stays within an explicitly allocated boundary and must not grow the persisted panel width. [Inspector source](crates/kiln/src/app/ui.rs).

The inspector exposes **파일 / 변경 / GitHub** only while open. File actions stay in the File tab's menu. A sheet switch explicitly opens the requested surface; invoking the inspector toggle remembers the last primary inspection tab. Composer state must not make a Changes action reopen the request editor.

The split tree remains intact during automatic focus view. Current thresholds distinguish tools below 360 logical points from terminals below 240, with a 150-point height threshold. Users can restore splits, use an explicit focus view, or choose equal strips/balanced grid. Focus preferences persist per task. New documents do not evict existing documents; reopening an existing document reveals its panel.

Shared spacing is compact rather than a universal grid. Use shared defaults and observed component padding. Settings descriptions wrap and controls stack when needed. Inspect at minimum size and increased UI scale; a large-window screenshot alone is insufficient.

**The Context Rule.** Preserve workspace → task → panel identity. A parent workspace's Changes action covers that workspace's repositories, not an invented attribution of every changed file to the active agent.

## Elevation & Depth

Working surfaces primarily use tonal separation and fine borders. The canvas surround is a midpoint blend of panel and canvas colors in dark themes. Cards stay visually flat; dialogs, menus and toasts use shadows to establish temporary foreground layers.

The shared native shadow has offset `[0, 8]`, blur `28`, spread `0`, and black alpha `140` in dark themes or `40` in the light theme. Modals add a dimmed backdrop. See [shadow and visuals](crates/kiln-common/src/theme.rs:203). Keep these native values in code; a CSS shadow translation is not an implementation token.

## Shapes

Controls use gently rounded rectangles. The base small, medium and large radii are in the frontmatter, with actual shared buttons and inputs using the separate control radius. Workspace cards and settings groups use the card radius. Rows use a smaller curve (6 points), and text status pills use a rounded capsule (9-point radius, 18-point height).

Use narrow strokes to define containment. Avoid giving every nested region a heavy border: the hierarchy should come from surface tones and purposeful spacing before additional decoration.

## Components

### Buttons and focus

Primary buttons use ice-blue fill with a dark foreground. Secondary buttons use an elevated fill and strong border; ghost buttons use transparent fill until hovered or pressed. Danger buttons use the error color. Regular custom buttons are 32 points high and compact variants 28 points high. Their horizontal allowance is 26 or 20 points respectively, plus label and optional icon width. Hover lightens primary/danger fill or raises neutral fill. Disabled variants reduce emphasis and do not act as enabled controls. See [button implementation](crates/kiln-common/src/widgets.rs:155).

Shared custom focus rings use a 2-point accent stroke outside a rectangle expanded by 2 points. Buttons, icon buttons, toggles and segments provide egui widget metadata. This documents the implemented affordance, not a claim of complete keyboard traversal or screen-reader conformance. See [focus treatment](crates/kiln-common/src/widgets.rs:9).

### Inputs and segmented controls

Inputs have a recessed fill, 1-point strong border and compact padding. Focus switches the border to the accent; errors switch it to red. Segmented controls sit in a recessed track, with a selected surface and outline. Toggles use a 36 × 24-point interactive area and animate state over 0.12 seconds. See [shared controls](crates/kiln-common/src/widgets.rs:16).

### Workspaces, tasks and activity

Workspace rows stay compact. Task titles describe the work or terminal folder; automatic titles are disambiguated in the shared tab-title helper rather than repeatedly shortened by unrelated components. Full names remain available through hover. Active tabs have selected treatment and a close control; renaming and secondary actions use double-click/context menus.

Task activity icons aggregate reported session state: warning/red for failure, bell/orange for input or attention, play/blue for running, and check/green for completion. Unknown activity stays quiet. State labels appear in hover/accessibility metadata; color alone must not carry meaning. A foreground process is not proof that an agent is running or has succeeded. [Activity aggregation](crates/kiln/src/app/ui.rs:153).

Panel headers retain identity and local commands. History belongs to the terminal panel; integration details are secondary. Split and close commands use the panel menu, while focus/restore stays direct. Do not restore a persistent helper toolbar beneath terminal output.

### Agent request composer

Put the editable request first. Repository context is structured as the parent root, repository names, relative paths where useful, and relevant instruction/project filenames. The raw generated context is a separate collapsed **전달 원문** section. Do not replace this hierarchy with a long unformatted metadata dump.

Choose **Codex / Claude**, then use one primary **작업 시작** action. Preserve the draft while repository discovery refreshes and when preparation fails. Starting work creates its own task without splitting the current terminal. Long requests use a private request file and a short launch command; the GUI must not impose the old 2,000-byte Korean-input limit. [Composer and launch forwarding](crates/kiln/src/app/workspace_repos.rs), [launch preparation](crates/kiln/src/app/agent_launch.rs:26).

### Repository changes and GitHub

Changes defaults to **변경만**, with **전체 저장소** available explicitly. GitHub defaults to **PR 있음** and presents up to three PR titles directly for each matching repository; the **PR …개 더 보기** action opens repository detail for additional results. The all-repositories view retains expandable rows. Avoid showing local branch/change summaries in the GitHub queue. Keep errors visible even when filtering for active work, and aggregate pending discovery rather than creating a page of loading rows.

Changes search matches repository identity and changed paths. GitHub search matches repositories/aliases, PR titles and numbers. A nonmatching search has a relevant empty state. Preserve visible result order as background results arrive. [Repository overview and search](crates/kiln/src/app/workspace_repos.rs:934).

Periodic refresh preserves the last usable snapshot and repository identity. Failed refreshes keep the snapshot with an error indication. Worktrees resolving to the same host-qualified GitHub repository share results; hidden aliases still recheck their target. A confirmed target change invalidates that checkout's old PR/detail state while preserving drafts and explicit repository selection; active submissions defer detail replacement. [Refresh lifecycle](crates/kiln/src/app/workspace_repos.rs).

### Original request

Agent-task context menus provide **요청 보기** and **요청 복사**. The request reference and body boundary persist with the task. Viewing opens a read-only **원래 요청** modal with the task title, exact human-written request, and separately collapsed workspace context. Long content scrolls; copy and close remain outside the scrolling body. Clipboard changes occur only from an explicit copy action. Missing or invalid files show an error rather than an empty successful view. [Request view](crates/kiln/src/app/agent_request.rs).

### Notification inbox and recovery

The bell distinguishes unread state. The inbox provides all/unread and category filters, individual read/dismiss controls, mark-all-read, do-not-disturb and clear-read actions. Titles navigate to the relevant work; read and dismiss remain separate operations. Keep session availability and failed operations visible.

Only an actually visible, focused terminal can automatically acknowledge attention. A model-selected pane behind an inspector, modal, or another input has not been seen. Returning to its visible terminal clears attention; opening the original-request modal does not count as reviewing terminal output. Routine daemon upgrade/reconnection notices are transient toasts and do not accumulate in permanent history. [Notification UI](crates/kiln/src/app/notifications.rs), [observation and acknowledgment](crates/kiln/src/app/mod.rs).

Recovery routes each draft to its editing surface. Agent-request recovery opens the composer with its contents; repository/GitHub drafts open the correct inspector. A failed shell's settings action opens Terminal settings directly. Recovery does not run retained commands or submit drafts automatically.

### Settings and typography validation

Settings use a modal with category navigation and grouped content. Theme options preview actual palettes and show selected state. Descriptions wrap before controls overlap them. Keep selected text on the semantic body foreground and filled controls on their explicit contrasting foreground; custom disabled controls apply disabled opacity once.

Terminal measurements include font size, line height and display scale independently. Editor measurements refresh with scale changes. Shared-theme contrast tests and controlled-shell renders provide scoped evidence, not a guarantee of every font fallback, native monitor transition, or accessibility path.

## Do's and Don'ts

- **Do** retain the graphite identity and consume semantic `Theme` roles.
- **Do** keep agent requests and terminals equally discoverable through **새 작업**.
- **Do** give work a direct route to workspace change review, attention and its original request.
- **Do** preserve parent-folder context while applying Git actions to the correct repository.
- **Do** distinguish reported activity, selection, keyboard focus and pending attention.
- **Do** keep last usable data, drafts and stable ordering during asynchronous work.
- **Do** test compact layouts, long Korean content, alternate themes and increased scale.
- **Don't** restore tool footers or spread the same tool collection across persistent chrome.
- **Don't** add explanatory text when a named action, icon tooltip or structured detail answers the need.
- **Don't** infer success from a running process or claim that all changes belong to one agent task.
- **Don't** let background content expand a user's dock or hide the task they selected.
- **Don't** treat harness screenshots, passing tests or this document as commercial/accessibility certification.
