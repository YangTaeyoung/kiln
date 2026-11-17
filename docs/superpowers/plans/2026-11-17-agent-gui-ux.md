# Agent GUI usability implementation plan

**Goal:** Implement the ten requested usability improvements and verify actual native rendering and interaction.
**Architecture:** Retain daemon/provider contracts, improve egui presentation, and add exclusive same-session presentation handoff.
**Spec:** docs/superpowers/specs/2026-11-17-agent-gui-ux-design.md

## Constraints
Preserve original checkout, user daemon and running release bundle. No user data in commits. Keep exact native IDs behind localized presentation. Product guide changes cover en/ko/ja/zh-CN.

## Tasks
- [x] Native metadata and compact localized settings: preserve display names/descriptions; slider commits one supported value at gesture end; unknown state never silently applies; independent accessible switch; terminal controls hidden and outcome history collapsed. Root owns settings_ui, controls metadata, claude_controls and view.
- [x] Commands: bounded search/results, expandable details, keyboard completion and highlighted command syntax. Agent owns commands_ui and its tests; root adds pre-TextEdit integration.
- [x] Presentation: exclusive host handoff using same native session/account/cwd; no work interruption or duplicate prompt. Agent owns host/model/app lifecycle files; root adds view-facing APIs.
- [ ] Integrate tests, inspect real dark/light/narrow renders and keyboard interaction, update localized site guide; one whole-branch review, fix material findings, merge/release under existing authorization and clean owned QA resources.

## Review focus
IME Enter must not select or submit; unsupported effort values retain their native identity; dragging sends once; large catalogs stay bounded; handoff cannot kill active work or show stale GUI as live.

- [x] Add provider-native `@` file/folder completion, bounded background search and exact path preservation; verify real isolated Claude pre-read and provider outgoing inputs.
