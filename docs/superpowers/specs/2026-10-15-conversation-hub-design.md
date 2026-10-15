# Conversation hub: one canonical conversation, native projections per agent

Date: 2026-10-15. Status: approved in conversation with the owner; implementation proceeds in two releases.

## Problem

Conversation sync copies each native transcript pairwise into the other tool's namespaces
(default CLI and connected account for Claude Code and Codex) and appends continuations
back. The original native file is the canonical record. On the owner's machine this model
produced, after 950 superseded snapshots had already been archived, 68 groups of live
transcripts sharing a title within one namespace (79 extra entries): 46 belong to one
linked conversation (diverged continuations kept side by side), 22 are unrelated or
pre-link copies. Users see:

1. duplicated conversations in `/resume`;
2. foreign histories that look broken in the other CLI (tool calls flattened into text,
   Claude command tags such as `<command-name>` shown as titles);
3. different resume lists in Claude Code and Codex;
4. a persistent warning badge for conditions the user cannot resolve (diverged
   histories, CLI versions missing from an exact allowlist).

## Goals

- One logical conversation appears exactly once in every linked agent namespace, with the
  same title and working directory.
- A conversation opened in another agent looks native to that agent.
- A turn completed in any agent reaches every other agent automatically.
- Adding an agent (for example Antigravity) means adding one adapter and its namespace
  discovery rule.
- Warnings appear only for problems the user can resolve, can be ignored, and clear
  automatically once resolved.

Unchanged requirements: original transcript bytes are never rewritten (Kiln only appends
or moves whole files into an archive); no summaries or extra model requests; accounts stay
isolated and no authentication files are copied; only completed turns transfer; images
keep their bytes; content that cannot be represented fails visibly instead of being
dropped; managed open conversations are never written while their CLI runs.

## Decisions

| Topic | Decision |
|---|---|
| Architecture | Central hub: native → canonical (ingest) → native (project). |
| Divergence | The most recently active branch becomes canonical; the other branch is archived and restorable. |
| Tool calls | Converted to the target agent's corresponding native tools (release 2). |
| Warnings | Only resolvable problems; each has an action and an **Ignore** option; resolved problems clear automatically. |
| CLI versions | Accept any version whose content is understood; unknown non-content records are skipped; unknown content pauses only that conversation. |
| Existing duplicates | Archive extra copies of the same linked conversation; keep unrelated conversations that merely share a title. |
| Delivery | Release 1: hub, dedup, titles, version tolerance, warning UX, migration. Release 2: native tool conversion after verification. |

## Architecture

### Namespaces

A namespace is `(tool, root)`: `~/.claude`, the connected Claude account directory,
`~/.codex`, the connected Codex account directory (equal directories count once). In
workspace mode the namespaces are `Scope::roots`; in pair mode they are the two pair roots.

### Hub store

`<config>/conversation-sync/hub/`:

- `state.json`
  - `conversations: {cid → Record}` where `Record` holds `title`, `cwd`, `created`,
    `updated` (time the canonical history last changed), `projections: [Projection]`,
    `aliases: [NativeRef]` (archived native IDs, used to redirect old IDs) and
    `branches: [Branch]` (archived divergent continuations).
  - `Projection`: `source` (tool, root, path, cwd, native id), `synced` (number of
    canonical events contained), `stamp` (length and modification time last ingested),
    `origin` (the agent's own original transcript).
  - `Branch`: id, `base` (number of shared canonical events), the archived native file and
    its integrity manifest, the branch's events file, `archived_at`.
- `events/<cid>.jsonl`: the canonical history. Each line is one event
  `{seq, at, author: {tool, root}, kind, ...}`. Release 1 stores portable messages
  (`role`, `text`, `images`, exactly the existing `format::Message`); release 2 adds
  structured tool events (below) under a new event schema revision.
- `branches/<branch>.jsonl`: events of archived branches.
- `migrated`: marker written after the one-time migration.

`cid` is a UUID derived from the first native reference (`stable_id`) so retries and
crashes reproduce the same identity.

### Adapters

Each agent provides `parse(rows) -> events` and `render(events) -> native rows` plus
`render_suffix` for appends. Release 1 reuses `format.rs` (Claude Code and Codex) and
`canonical::update` (verified append that preserves existing bytes). Agent-specific
title pinning (`custom-title.json`, Codex `thread/name/set`) and Codex index registration
(`codex_index`) stay per adapter.

## Synchronization

Each scan (existing worker cadence: stat-based change checks, 10 s quiet window):

1. **Discover** native transcripts in the namespaces (existing discovery, initial twenty
   per workspace, tracked hook references in pair mode).
2. **Adopt** a native transcript that is not a known projection or alias and contains a
   completed user conversation: create a conversation with its events; it becomes the
   origin projection of its namespace.
3. **Ingest** each projection whose file changed and is not being written: parse its
   portable messages `M`. With canonical events `E` and its `synced = k`:
   - `M == E[..k]`: nothing new.
   - `M` extends `E[..k]` and `k == len(E)`: append `M[k..]` to `E`.
   - Otherwise the projection diverged from canonical at `p` = common prefix of `M` and `E`
     (this includes a native rewind or rollback). The side with the more recent activity
     wins: the projection's file modification time against `updated`.
     - Projection wins: `E[p..]` is saved as a branch, `E` becomes `E[..p] + M[p..]`.
     - Canonical wins: `M[p..]` is saved as a branch.
     Every projection whose content is no longer a prefix of the new `E` is archived (Claude:
     moved to the Kiln archive with a SHA-256 manifest; Codex: native `thread/archive`) and
     recreated with a new native ID; its old ID becomes an alias.
4. **Project** to every namespace of the conversation's workspace: create a missing
   projection (full render, new deterministic native ID, title pinned, Codex registered),
   or append `E[synced..]` with `canonical::update`. A projection is not written while a
   managed agent holds it (existing writer guard) or while it was modified within the quiet
   window.
5. **Publish links** for handover: group = `cid`; every projection and alias maps to the
   current projection ID of its namespace. `settled` contains conversations whose
   projections all hold `len(E)` events and are unchanged since ingest.

A turn completed in one agent therefore reaches the others within roughly the quiet window
plus one scan (about 10–15 s).

### Incremental ingest

Unchanged projections cost one `stat`. When a projection only grew (its previously
ingested bytes are unchanged), the reader parses the appended records from a saved parser
checkpoint instead of the whole file. Any case the checkpoint cannot represent — a changed
prefix, a Claude append that does not continue the saved leaf, a compaction boundary, an
interruption record or a Codex rollback — falls back to a full parse. A randomized test
requires incremental and full parsing to produce identical events.

### Titles and resume lists

The canonical title is the most recently changed native name (Claude custom title, Codex
thread name), else the first user prompt with CLI command tags (`<command-name>`,
`<local-command-…>`) and image placeholders removed. The same title is pinned on every
projection. Sessions without a real prompt are not adopted. Claude's own `bridge-session`
files are not Kiln's and are left alone. Projections use the conversation's working
directory, so both CLIs list the same conversations in the same folder.

### Version tolerance

Exact CLI version allowlists are removed from the transcript reader and from Codex index
registration. Unknown record types that do not carry conversation content are skipped
(Claude rows other than `user`/`assistant`; Codex `event_msg`/metadata records). Unknown
content (a new Claude block type, a new Codex `response_item` type) pauses only that
conversation with an informational status; it is retried automatically when Kiln's build
changes. Projections are written in the newest format Kiln knows.

### Status, warnings and recovery

The status list shows one row per conversation with per-agent projection state.
Conditions are classified:

- **Actionable warning** (sidebar badge and "needs attention"): connected account
  directory missing or unusable → *Open account settings*; storage full or permission
  denied → reason and *Try again*; Codex CLI missing for `/resume` registration →
  installation guidance.
- **Information** (grey, no badge): archived branch (with *Restore*), new transcript format
  awaiting a Kiln update, transcript too large or incomplete, response still being
  written, conversation open in another panel.

Every warning offers **Ignore**. Ignored warnings leave the badge and count, remain visible
under *Ignored* in the dialog and can be restored; a different condition on the same
conversation warns again. A resolved condition clears on the next successful scan.
*Restore* turns an archived branch into a separate conversation titled with a
"(restored)" suffix and projects it everywhere.

## Migration (one time, in the worker)

From the existing state (`workspace-state.json` or the pair state):

1. Group live entries by recorded publication edges (lineage and copies).
2. Per group, the most recently modified live transcript defines the canonical events.
3. Per namespace, the live transcript whose messages are the longest prefix of the
   canonical events becomes the projection (original before Kiln copies, then most
   recent). Other live transcripts of the group are archived; those that are not a prefix
   are archived as branches.
4. Transcripts that cannot be parsed are left untouched and reported as information.
5. Ungrouped transcripts become their own conversations.

The legacy state files are kept so an older Kiln still works. Migration is idempotent and
resumes after interruption; `hub/migrated` marks completion.

## Release 2: native tool conversion

Canonical tool events (event schema revision 2):

| Canonical | Claude Code | Codex |
|---|---|---|
| `shell {command, cwd}` | `Bash` | `exec_command` (in code mode, `exec` calling `tools.exec_command`) |
| `read_file {path, offset, limit}` | `Read` | shell command reading the file |
| `write_file {path, content}` | `Write` | `apply_patch` (add file) |
| `edit_file {path, old, new}` / `apply_patch {patch}` | `Edit` per file | `apply_patch` |
| `web_fetch {url}` / `web_search {query}` | `WebFetch` / `WebSearch` | web search record |
| `plan {items}` | `TodoWrite` | `update_plan` |
| `subagent {prompt, result}` | `Agent` | `spawn_agent` |
| `other {agent, name, input}` | readable text block | readable text block |

Each tool result keeps output text, images, error flag and exit code. A Codex `exec`
program that calls exactly one recognised tool is decomposed into that tool; anything else
stays `other`. Shell commands are never reinterpreted as other tools.

Before enabling a mapping, each target CLI is run in an isolated home against a loopback
model server with synthetic projections: the resume view must render the native tool
records without errors and the next request must carry them. Kinds that fail fall back to
the readable text block for that agent only. Acceptance by the live provider APIs is
confirmed separately with the owner's approval.

## Safety invariants

- Native transcripts are only appended to or moved whole into an archive with an integrity
  manifest; archived files remain recoverable.
- A projection is written only after its full prospective content parses back to exactly
  the canonical events it should contain (existing verification).
- Hub state and event files are written atomically; events are replaced only by an atomic
  rewrite that first saves the replaced tail as a branch.
- Each namespace holds at most one live projection per conversation.

## Testing

- Adapter round trips (text, images, interruptions, compaction, compressed Codex
  histories) — existing tests retained.
- Hub invariants under randomized continuation sequences across namespaces: exactly one
  live projection per namespace; divergence picks the most recent side; the losing side
  is archived and restorable.
- Migration from synthetic states shaped like the observed duplicates (copy pairs, lineage
  chains, unrelated conversations with equal titles).
- Titles without command tags; command-only and metadata-only sessions not adopted.
- Version tolerance: unknown metadata skipped, unknown content pauses one conversation,
  retry after a build change.
- Warning UI: action, ignore, restore from ignored, automatic clearing; restore of a branch.
- Isolated real-CLI checks: both `/resume` lists show each conversation once with the same
  title.
- Release 2: tool mapping round trips and the loopback verification above.
