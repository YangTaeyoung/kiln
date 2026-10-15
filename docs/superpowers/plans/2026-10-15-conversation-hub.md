# Conversation Hub Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace pairwise conversation copies with a canonical hub so every linked conversation has exactly one native projection per agent namespace, then convert tool calls to each agent's native tools.

**Architecture:** A new `hub` module owns canonical records (`hub/state.json`) and event logs (`hub/events/<cid>.jsonl`). The worker scan keeps discovery, the native adapters (`format.rs`, `reader.rs`, `titles.rs`, `codex_index.rs`) and the verified append (`canonical::update`), but replaces the per-source copy loop, snapshot retirement and link publication with hub ingest → reconcile → project. Release 2 extends events with structured tool calls and renders them as native tools.

**Tech Stack:** Rust 2024, serde_json, egui (status UI), egui_kittest (UI tests), existing `kiln sync-worker` process.

**Spec:** `docs/superpowers/specs/2026-10-15-conversation-hub-design.md`

## Global Constraints

- Native transcripts are only appended to or moved whole into an archive with a SHA-256 manifest.
- A projection is published only after its full prospective content parses back to exactly the events it should contain.
- No summaries or model requests; no authentication files copied; account roots stay isolated.
- Only completed turns transfer; images keep their bytes; unrepresentable content fails visibly for that conversation only.
- A projection held by a managed agent (writer guard) or modified within the quiet window (10 s) is not written.
- At most one live projection per conversation per namespace.
- Code comments describe only what the code does (owner rule); public docs in English; UI strings via `tr`/`trf!` with en/ja/zh-CN catalog entries.
- Tests use isolated config/socket/account paths; never the owner's daemon or data.

## Review Focus

1. A projection file that is open in an external CLI and appended between Kiln's read and publish — publication must abort (`storage::equals` check) and retry, never clobber. Test in Task 3.
2. Both sides continued with identical modification times — the tie must be deterministic (canonical wins) and must not loop on every scan. Test in Task 2.
3. A Claude projection whose title sidecar exists from an older Kiln with a different title — the canonical title is pinned without rewriting the transcript. Test in Task 6.
4. Migration interrupted after archiving some files — rerun must not archive the projection it already chose or create a second projection. Test in Task 4.
5. An ignored warning whose condition changes to a different actionable condition must warn again. Test in Task 7.

---

### Task 1: Hub state, events and identity

**Files:**
- Create: `crates/kiln/src/app/conversation_sync/hub.rs`
- Modify: `crates/kiln/src/app/conversation_sync/mod.rs` (add `mod hub;`)
- Test: `crates/kiln/src/app/conversation_sync/tests.rs`

**Interfaces:**
- Produces:
```rust
pub(super) struct Hub { dir: PathBuf, pub state: HubState }
#[derive(Default, Serialize, Deserialize)] pub(super) struct HubState { pub conversations: BTreeMap<String, Record> }
#[derive(Clone, Serialize, Deserialize)] pub(super) struct Record {
    pub title: String, pub cwd: String, pub created: SystemTime, pub updated: SystemTime,
    pub projections: Vec<Projection>, #[serde(default)] pub aliases: Vec<NativeRef>,
    #[serde(default)] pub branches: Vec<Branch>, #[serde(default)] pub events: usize,
}
#[derive(Clone, Serialize, Deserialize)] pub(super) struct Projection {
    pub source: Source, pub synced: usize, pub stamp: Option<(u64, SystemTime)>, pub origin: bool,
}
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)] pub(super) struct NativeRef { pub tool: Tool, pub root: PathBuf, pub id: String }
#[derive(Clone, Serialize, Deserialize)] pub(super) struct Branch {
    pub id: String, pub base: usize, pub events: usize, pub archived: Option<PathBuf>,
    pub native: Option<NativeRef>, pub at: SystemTime, #[serde(default)] pub restored: bool,
}
impl Hub {
    pub fn open(config: &Path) -> Result<Self>;                      // loads or defaults
    pub fn save(&self) -> Result<()>;                                // atomic replace_changed
    pub fn events(&self, cid: &str) -> Result<Vec<format::Message>>;
    pub fn append(&mut self, cid: &str, messages: &[format::Message]) -> Result<()>;
    pub fn rewrite(&mut self, cid: &str, base: usize, tail: &[format::Message]) -> Result<Vec<format::Message>>; // returns replaced tail
    pub fn save_branch(&mut self, cid: &str, base: usize, tail: &[format::Message], native: Option<NativeRef>, archived: Option<PathBuf>) -> Result<String>;
    pub fn find(&self, r: &NativeRef) -> Option<&str>;               // projection or alias → cid
}
pub(super) fn conversation_id(first: &NativeRef) -> String;          // stable_id("hub:{tool}:{root}:{id}")
```
- Events file line: `{"seq":n,"role":..,"text":..,"images":[..]}`; `Record.events` caches the count.

- [ ] **Step 1: Write failing tests** `hub_events_append_rewrite_and_branch_round_trip` (append 3 messages, rewrite from base 1 with 2 new, assert returned tail = old messages[1..], events = old[..1]+new, branch file holds the tail) and `hub_identity_is_stable_and_aliases_resolve`.
- [ ] **Step 2: Run** `cargo test -p kiln --lib hub_` → FAIL (module missing).
- [ ] **Step 3: Implement `hub.rs`** with `private_dir`, `replace` and `publish` from `mod.rs`; events written with `replace` (whole-file atomic) because rewrites must stay atomic; files under `<config>/conversation-sync/hub/{state.json,events/,branches/}`.
- [ ] **Step 4: Run tests** → PASS.
- [ ] **Step 5: Commit** `feat(sync): add the conversation hub store`.

### Task 2: Reconcile (pure ingest/divergence decision)

**Files:**
- Modify: `crates/kiln/src/app/conversation_sync/hub.rs`
- Test: `crates/kiln/src/app/conversation_sync/tests.rs`

**Interfaces:**
- Produces:
```rust
#[derive(Debug, PartialEq)] pub(super) enum Outcome {
    Unchanged,
    Append(usize),                       // native[synced..] extends canonical; value = new event count
    NativeWins { base: usize },          // canonical[base..] becomes a branch; canonical = native
    CanonicalWins { base: usize },       // native[base..] becomes a branch; projection is recreated
    Behind,                              // native is a strict prefix of canonical beyond synced: projection needs append only
}
pub(super) fn reconcile(canonical: &[Message], synced: usize, native: &[Message], native_newer: bool) -> Outcome;
```
- Rules: `base` = common prefix length of `canonical` and `native`. If `native == canonical[..native.len()]` and `native.len() >= synced` → `Behind` when shorter, `Unchanged` when equal length. If `native` extends all of `canonical` → `Append`. Otherwise divergence at `base`: `NativeWins` iff `native_newer` (strictly newer); ties → `CanonicalWins`.

- [ ] **Step 1: Write failing tests** covering: unchanged, append, behind, native rewind (native shorter and different) newer → NativeWins, both continued with equal time → CanonicalWins, randomized property: applying the outcome yields a canonical history of which the winning side is a prefix.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement `reconcile`.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(sync): decide hub ingest and divergence outcomes`.

### Task 3: Hub scan engine (adopt, ingest, project, links, status)

**Files:**
- Modify: `crates/kiln/src/app/conversation_sync/mod.rs` (`scan_with_metadata` body after discovery), `hub.rs`, `handover.rs` (`publish_links`/`settled` from hub), `canonical.rs` (extract `archive_native`)
- Test: `crates/kiln/src/app/conversation_sync/tests.rs`

**Interfaces:**
- Consumes: Task 1–2 types; `format::render_titled`, `canonical::update`, `discovery::remember_projection`, `titles::pin_claude`, `metadata(source, Metadata::Name|Archive)`, `handover::write_guard`.
- Produces:
```rust
pub(super) struct Namespaces(Vec<(Tool, PathBuf)>);                 // scope.roots or pair roots
pub(super) fn sync(ctx: &mut Context<'_>, sources: Vec<Source>) -> Result<()>; // adopt + ingest + project for all
pub(super) fn archive_native(config: &Path, source: &Source, metadata: &mut impl FnMut(&Source, Metadata<'_>) -> Result<Option<String>>) -> Result<PathBuf>; // in canonical.rs
pub(super) fn projection_id(cid: &str, root: &Path, generation: usize) -> String;   // stable_id("hub:{cid}:{root}:{generation}")
handover::publish_hub_links(config, &Hub, scope) -> Result<()>;
handover::hub_settled(&Hub, &Cache) -> BTreeSet<String>;
```
- Status rows: key = `cid`; `Conversation.source` = origin projection source; `copies` = the other projections; `phase` aggregated (Synced when every projection holds all events).
- Projection paths follow `destination()` (Claude project encoding, Codex dated rollout path) using the conversation `cwd`.

- [ ] **Step 1: Adapt fixture helpers**: `Fixture::hub()`, `Fixture::peer(tool) -> Source` (projection in the opposite tool namespace), `Fixture::projections()`.
- [ ] **Step 2: Rewrite the bidirectional tests** (`bidirectional_changes_are_native_and_do_not_echo`, `repeated_resume_updates_the_same_two_native_ids_and_preserves_existing_bytes`) on the helpers and add `diverged_histories_keep_the_newest_branch_and_archive_the_other`, `a_namespace_never_holds_two_live_projections` (randomized continuations in all four namespaces), `external_append_during_publication_retries_without_clobbering`.
- [ ] **Step 3: Run** → FAIL.
- [ ] **Step 4: Implement** `hub::sync`: build `NativeRef` index; adopt unknown discovered sources (skip Kiln imports without a hub record, handled by migration); for each record in scope: ingest changed projections (stamp differs, not quiet, parse with `read_messages_titled`), apply `reconcile`, save branches, archive projections whose content is no longer a prefix (`archive_native`, push alias, recreate), then project missing namespaces and append pending events (`canonical::update` under `write_guard`); pin titles; update stamps; build rows; publish links; compute settled.
- [ ] **Step 5: Replace** the per-entry loop, `canonical::retire` and `publish_links` calls in `scan_with_metadata` with `hub::sync`; keep discovery, `tracked` handling and progress notifications.
- [ ] **Step 6: Run the whole `conversation_sync` test module**; adapt tests that assert legacy `State` internals to hub helpers; delete tests whose subject no longer exists (lineage recovery of pre-hub snapshot IDs, snapshot retirement, `Links` group equivalence) and replace them with hub equivalents.
- [ ] **Step 7: Commit** `feat(sync): synchronize through the conversation hub`.

### Task 4: Migration from the legacy state

**Files:**
- Modify: `hub.rs`, `mod.rs` (call before discovery), `canonical.rs` (keep `Links` for grouping legacy entries)
- Test: `tests.rs`

**Interfaces:**
- Produces: `pub(super) fn migrate(config: &Path, legacy: &State, hub: &mut Hub, namespaces: &Namespaces, metadata: ...) -> Result<()>`, marker `hub/migrated`.

- [ ] **Step 1: Failing tests**: `migration_keeps_one_projection_per_namespace_and_archives_extra_copies` (group with original, two Codex copies of which one is a prefix of the newest), `migration_keeps_unrelated_conversations_with_equal_titles`, `migration_resumes_after_interruption_without_new_projections` (run, delete marker, rerun → same hub), `unparseable_legacy_transcripts_are_left_untouched`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement**: group live entries with `canonical::Links`; newest live entry defines events; per namespace choose longest-prefix entry (origin first, then newest); archive the rest (`archive_native`), prefix-incompatible ones as branches; ungrouped entries become conversations; write marker last.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(sync): migrate legacy copies into the hub`.

### Task 5: Version tolerance

**Files:**
- Modify: `mod.rs` (`read_rows_titled`), `codex_index.rs` (`Registry::metadata` version check), `worker.rs` (build id in cache), `format.rs` (unknown non-content Codex `event_msg`/metadata stay skipped)
- Test: `tests.rs`

- [ ] **Step 1: Failing tests**: `unknown_cli_versions_with_known_content_synchronize`, `unknown_content_pauses_only_that_conversation`, `paused_conversations_retry_after_a_build_change`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement**: remove version allowlists; Codex registration accepts any CLI exposing `thread/read`; `Cache.build` stores the build id, mismatch clears `attempts`, `failures`, `linked`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `fix(sync): accept CLI versions whose content is understood`.

### Task 6: Titles

**Files:**
- Modify: `titles.rs` (`fallback`, `clean_prompt`), `hub.rs` (canonical title selection and pinning)
- Test: `tests.rs`

- [ ] **Step 1: Failing tests**: `fallback_titles_drop_command_tags_and_image_placeholders`, `canonical_title_follows_the_latest_native_rename_in_every_projection`, `old_title_sidecar_is_replaced_by_the_canonical_title_without_rewriting_history`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement**: strip `<command-name>…</command-name>`, `<command-message>…`, `<command-args>…`, `<local-command-…>` blocks and `<image …>`/`[Image #n]` placeholders before choosing the first line; canonical title = newest native rename across projections, else stored title, else fallback; pin via sidecar (replace when it holds a Kiln-pinned title) and `Metadata::Name`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `fix(sync): share clean titles across agents`.

### Task 7: Attention model, ignore and restore

**Files:**
- Modify: `mod.rs` (`Conversation` gains `attention: Option<Attention>` and `branches`; `Badge`; `progress_status_ui`; dialog actions), `hub.rs` (restore requests), `crates/kiln/src/app/state.rs` (ignored warnings persisted per workspace), `crates/kiln-common/locales/messages.json`
- Test: `tests.rs`

**Interfaces:**
```rust
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] enum Attention { Account, Storage, CodexCli }
fn classify(detail: &str) -> Option<Attention>;                        // None = information
// restore: GUI writes <config>/conversation-sync/hub/restore/<branch>.json; hub::sync consumes it
```
- [ ] **Step 1: Failing tests**: `only_resolvable_conditions_raise_the_badge`, `ignored_warnings_leave_the_badge_until_a_different_condition`, `resolved_conditions_clear_on_the_next_scan`, `restoring_a_branch_creates_a_separate_conversation_everywhere`, UI test `progress_ui_offers_actions_ignore_and_restore`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** classification (account root missing/symlinked → Account; `StorageFull`/`PermissionDenied`/`ENOSPC` → Storage; Codex CLI missing → CodexCli), row rendering (warning with action button + Ignore; information grey; Ignored section with Undo; branch Restore), badge uses non-ignored warnings.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(sync): warn only about resolvable problems`.

### Task 8: Incremental ingest

**Files:**
- Modify: `reader.rs`, `format.rs`, `hub.rs` (projection checkpoint), `mod.rs`
- Test: `tests.rs`

- [ ] **Step 1: Failing tests**: `incremental_and_full_parsing_agree_under_random_appends` (both tools), `prefix_change_falls_back_to_a_full_parse`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** checkpoint `{len, prefix_sha256, tail_state}`: for append-only growth parse only new rows and extend; fall back to full parse on prefix change, Claude parent mismatch, compaction boundary, interruption, Codex rollback.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `perf(sync): parse only appended transcript records`.

### Task 9: Documentation and release 1

- [ ] Update `docs/conversation-sync.md` to the hub model; release notes `docs/releases/0.1.33.md`; run the CI-equivalent suite; release per `docs/maintainers/releases.md`.

### Task 10: Structured tool events (release 2)

**Files:**
- Modify: `format.rs` (`Message` gains `tool: Option<ToolEvent>`), new `tools.rs` (mapping), `hub.rs` (event schema revision 2 with upgrade of stored events by re-ingesting the origin)
- Test: `tests.rs`

**Interfaces:**
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolEvent {
    Call { id: String, action: Action },
    Result { id: String, output: String, images: Vec<String>, error: bool, exit_code: Option<i64> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Shell { command: String, cwd: Option<String> },
    ReadFile { path: String, offset: Option<u64>, limit: Option<u64> },
    WriteFile { path: String, content: String },
    EditFile { path: String, old: String, new: String, all: bool },
    ApplyPatch { patch: String },
    WebFetch { url: String, prompt: Option<String> },
    WebSearch { query: String },
    Plan { items: Vec<PlanItem> },
    Other { agent: Tool, name: String, input: String },
}
```
- [ ] Failing round-trip tests per action for Claude (`tool_use`/`tool_result`) and Codex (`function_call`/`custom_tool_call` incl. code-mode `exec` with one `tools.exec_command` call); implement parsing; commit.

### Task 11: Native tool rendering and verification (release 2)

- [ ] Render actions as target-native records (table in the spec); `Other` as the readable text block; failing tests first; loopback verification with isolated CLIs; fall back per kind on failure; docs, release notes `0.1.34`, release.
