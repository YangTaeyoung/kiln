# Bounded conversation synchronization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bound ongoing workspace sync and remove repeated expensive history reads, then release.
**Architecture:** Persist per-root limits in Settings and send them to worker scopes. Select logical conversation groups before adoption and reconciliation using native activity, leaving stored histories intact. Preserve existing hub and writer ownership contracts.
**Tech Stack:** Rust, egui, serde, existing JSONL worker and native CLI metadata.
**Spec:** docs/superpowers/specs/2026-10-23-bounded-conversation-sync-design.md

## Global Constraints
Default 20; valid range 1..4096; full contents preserved; no source deletion for pruning; no credential or real terminal captures in commits; isolated tests; all release platforms required.

## Review Focus
- Sync-generated mtime must not promote old copies or cause echo.
- Existing hub records must obey reduced limits, not just new discoveries.
- Native continuations and overlapping workspace roots must select exactly once.
- Restarts and changed settings must invalidate pending selection safely.
- Compaction preserves pre-boundary ordering and original duplicate-ID semantics.

### Task 1: Bounded selection and user setting
**Files:** state.rs, conversation_sync/{selection,discovery,engine,worker,mod,tests}.rs, localization catalogs, docs.
**Interfaces:** Settings.conversation_sync_limits: BTreeMap<PathBuf,usize>; Scope.limits; selection::choose(scope,hub,sources) -> selected group/source sets.
- [ ] Write rolling limit, logical deduplication, reduced-limit existing-hub, native continuation and self-write exclusion tests.
- [ ] Run targeted tests and observe missing behavior (FAIL expected).
- [ ] Implement root limits and ranking before adoption/reconciliation, UI input and backward-compatible settings/worker fields.
- [ ] Run focused library and GUI tests (PASS expected), including no-echo and preserved excluded files.
- [ ] Commit feature and validation.

### Task 2: Sync efficiency and cadence
**Files:** conversation_sync/{engine,format,discovery,worker,mod,tests}.rs.
**Interfaces:** bounded alias comparison cache keyed by source stamp and canonical generation/events; discovery cache; completed-scan scheduling.
- [ ] Write unchanged failed-alias retry regression and observe FAIL.
- [ ] Record synthetic compaction parser baseline; preserve existing semantic tests.
- [ ] Implement alias retry/cache and linear duplicate elimination; cache full discovery for 30 seconds, poll known histories separately; schedule three-second interval after completion.
- [ ] Run sync suite and synthetic benchmark (PASS and improved scaling expected).
- [ ] Commit fixes and record evidence.

### Task 3: Verification and release
**Files:** Cargo.toml, Cargo.lock, docs/releases, docs/maintainers.
**Interfaces:** Immutable new release artifacts and update feeds.
- [ ] Run workspace library, native IME, affected integration, localization and cross-platform checks (PASS expected).
- [ ] Fresh whole-branch review, fix important findings with regressions.
- [ ] Commit release source, push PR, wait for CI and merge under user authorization.
- [ ] Build/sign/notarize macOS and build Windows artifacts from full merge SHA; validate installers and signatures.
- [ ] Publish only after all checks, re-download assets and verify latest feeds; report observation boundaries.
