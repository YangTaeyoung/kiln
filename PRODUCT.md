# Kiln
<!-- impeccable:product-schema 1 -->

## Platform
Desktop native application, Rust + egui. macOS is the current validation platform; existing Windows and Linux support is preserved.

## Users
Developers working with multiple AI agents and terminal sessions, and developers editing code and using Git and databases. Both workflows have equal priority.

## Product Purpose
A persistent development workspace combining terminal sessions, code, Git, GitHub and database tools. Sessions are owned by a separate daemon.

## Capabilities and Constraints
Keep existing sessions, projects, editor content and settings compatible. Treat notifications, recovery, keyboard navigation and empty states as part of the core workflow.

## Brand Commitments
Kiln name. An ivory tile, graphite kiln mouth and copper ember, maintained as an editable SVG. Korean product copy. No fictional commercial claims.

## Quality checks
Review representative logical resolutions, Retina rendering, long workspace names and overflowing page navigation. Use the GUI harnesses and integration tests alongside native interaction. Review design and developer workflows independently.

## Product Principles
- Equal access to agent sessions and IDE tools.
- Clear project, task tab and panel hierarchy.
- Persistent, actionable notifications with user control over interruptions.
- Visible state and keyboard-accessible controls.
- Preserve work and make destructive operations deliberate.

## Workflow priorities
The main unit is a parent folder such as `~/dev/personal`, containing related repositories. An agent receives a goal and works across frontend/backend repositories while keeping that parent context. Individual Git operations remain repository-specific.

Paying-user value must come from continuity and reduced coordination: open a folder, start or resume work, return to the session needing input, inspect changes, and retain work safely. Do not add functionality merely to fill an IDE feature checklist.

- Prefer recognizable icon buttons with accessible names over repetitive explanatory text.
- Normal/idle state stays quiet. Surface actionable attention, failure, changes, and unsaved work.
- Main and linked worktrees use Git-reported identity and a stable visual hierarchy; never infer identity from a folder name.
- Saved work opens directly; management and customization are secondary actions.
- Reconsider feature placement and flow when necessary. Small diffs are not a product-quality goal.

- Navigation quality is about role and workflow, not merely replacing labels with icons. Workspaces switch on the left; results are inspected on the right. Do not expose every installed tool in persistent chrome.
