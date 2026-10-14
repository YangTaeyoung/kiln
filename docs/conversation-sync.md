# Claude Code ↔ Codex conversations

Connect both CLI accounts in **Settings → Accounts**, then right-click a workspace
in the sidebar and choose **Link conversations**. Only explicitly linked workspace
folders synchronize while Kiln is open. Existing enabled connections do not opt
any workspace in automatically. Choose **Unlink conversations** in the same menu
to stop future synchronization; existing CLI copies remain available.

Synchronization discovers Claude Code and Codex transcripts within each selected
folder, including subfolders. Existing sessions and sessions opened outside Kiln
are included; reopening a panel or receiving a lifecycle hook is not required.

The initial synchronization selects the **20 most recently modified conversations
per workspace**, across both tools and their default/connected namespaces. The
selection is persisted so restarting Kiln continues the same initial batch. Version
0.1.28 refreshes that selection once to include Codex compressed (`.jsonl.zst`)
histories. Compression and native materialization retain the same conversation ID. Metadata-only CLI sessions and local-command-only sessions do not consume a slot. On upgrading from 0.1.24, the initial selection is refreshed once to exclude those empty sessions. New
and modified conversations are discovered afterward, including an older session
that saves another update. Closing a workspace pauses its synchronization; linking
a different workspace starts that folder's own initial batch. Overlapping folders
are assigned to the nearest linked workspace, and duplicate workspace paths count once.

Discovery checks both the ordinary CLI directories (`~/.claude`, `~/.codex`, or
`CLAUDE_CONFIG_DIR` / `CODEX_HOME`) and the two connected account directories.
Completed transcripts are made available in both opposite-tool locations, so an
ordinary terminal and a Kiln account panel can each use `/resume`. Equal directories
are deduplicated. Switching the active account does not redirect the pinned
connection. The status dialog identifies the pinned account pair. Sync remains opt-in.

In the other CLI, open the same project and use **`/resume`** to select the newest
linked conversation. Original user and assistant messages appear in the CLI's normal
conversation display and become part of the next model request. Kiln creates no
summary prompt, handoff note, or extra model request. Different conversations in
the same folder remain separate; workspace discovery does not merge their histories.

## Matching names and readable history

Kiln uses the source CLI's saved conversation name for both resume pickers. Codex
names come from its native metadata API, including names present only in its
database; the append-only text index is a fallback. A
native rename is detected even when no new response was written. Claude's custom
name takes precedence over its generated name; unnamed sessions use the first
prompt. Kiln pins a missing Claude title through its native title sidecar and a missing
Codex name through its native API, leaving source transcript bodies unchanged. Codex copies receive the same name through
`thread/name/set`, followed by a metadata read that checks persistence.

Historical tool calls/results use the same labeled, fenced text in both CLIs.
Complete JSON tool results are decoded into fields and multiline output, including
adjacent JSON responses and nested text blocks. Invalid or truncated JSON stays
verbatim. Code and angle-bracket text stay inside fences, and source-reference
footers are displayed as literal text. The exact original tool text is retained
in local copy metadata with a hash of its visible representation; an edited
visible message is never silently replaced by that metadata. Images retain their
original native blocks. A Codex visible answer and its raw response with the same
message ID appear once, including when their reference footers differ.

The native clients still control typography and terminal layout. This aligns names
and portable message content, not their entire UI.

## One canonical conversation and exclusive handover

Each linked conversation keeps one native ID per tool and account namespace.
Completed continuations append to that existing endpoint; switching back does not
create another copy. Kiln preserves the existing native log bytes, including its
provider-private records, and verifies every transferred message and image before
publishing the appended checkpoint. A conflicting branch remains intact and is
reported for attention rather than silently merged.

Inside Kiln, opening the same linked conversation requests ownership for the new
panel. Kiln interrupts the previous managed CLI and the newly opened reader,
waits for confirmed exits, synchronizes the final saved checkpoint, and reopens
only the requested panel at its canonical ID. Prompts wait while ownership is
being transferred. Capacity recovery and automatic CLI restarts cannot reclaim
an inactive conversation. Other conversations continue running. If a CLI does
not exit or history cannot be verified, the new panel stays waiting with a
visible status; Kiln does not force-kill it or claim that transfer completed.

Old generated snapshots are consolidated only when recorded publication/import
provenance proves that they belong to this conversation and their full history
is already present in its canonical endpoint. Superseded snapshots are archived
with an integrity manifest. Codex snapshots with descendant histories remain
available for review instead of archiving those descendants implicitly. Original conversations and divergent branches remain
intact. An old alias opened inside Kiln is redirected to the canonical ID.

Exclusive handover applies to managed Kiln agent panels. External terminals and
other desktop applications do not participate in Kiln's ownership protocol;
close those readers before continuing the same linked conversation in Kiln.

## Read synchronization status

Click a workspace's sync icon or choose **View conversation sync status** from its
context menu. The workspace icon and its status dialog show waiting, working, attention-needed, paused, or a green
check when all completed conversations have been synchronized. During work it
shows the number of transcript records checked and new copies created. The list
shows each source conversation's native title (or first prompt when unnamed), project, direction, status, and a
button to copy its continuation ID. Copies that have not been continued are
hidden from the list to avoid displaying both sides of the same transfer.
Codex native import provenance is also checked: importing an unchanged Kiln copy
through Codex Desktop does not start another transfer loop or consume an initial
conversation slot. A genuinely continued imported copy remains eligible.

The status dialog distinguishes discovery, no conversations found in the open folders, and
waiting for a completed response. A discovered unsupported transcript stays visible
with its failure reason; it is not reported as successfully synchronized. A failed item includes its
reason above its title. Expand **Conversation details** for the original diagnostic or copy it for troubleshooting. A single scrollable list keeps the summary and controls visible. Unchanged background checks retain the last result, row positions, and progress bar; they do not reset completed or failed rows to queued. Unchanged failed/waiting files are checked again after a 30-second backoff; changed files are checked immediately. **Check again** starts a fresh check. Unlinking stops new publication (an atomic write already in progress may finish); existing copies remain available in `/resume`.
The check refers to **completed responses**, not an answer still being generated.

## Compatibility and boundaries

- New continuations target **Codex CLI 0.162.1** and **Claude Code 2.1.296**.
  Verified input versions include Codex 0.154.0, 0.155.0-alpha.9.2, 0.155.1,
  0.160.0, 0.160.1, 0.161.0, 0.162.0-alpha.17.2 and 0.162.1; Claude Code
  2.1.272, 2.1.281, 2.1.289, 2.1.291, 2.1.292, 2.1.295, 2.1.296 and 2.1.299.
  Identity and content validation still apply to every transcript. Changing
  working directories within the same Claude session does not invalidate it.
  Unknown versions fail visibly; an auto-update may require an adapter update.
- Text messages retain their wording, line breaks, and code blocks. Textual tool
  calls and results are represented as labeled assistant messages; historical
  tools are not executed. Provider instructions, model-specific thinking and
  signed reasoning and encrypted provider-private collaboration records are not
  transferred. Images embedded in user messages or tool output retain their bytes
  (PNG, JPEG, GIF and WebP); HTTP(S) image references retain their URL. Native image
  blocks are created in both destinations. Image references without their original
  payload, provider-specific file IDs, documents, audio and unknown content fail
  visibly. A conversation is **not marked complete** by dropping an attachment.
  When a Codex visible answer has no corresponding Responses message, its original
  native `AgentMessage` text is retained without duplicating both projections.
- Each linked conversation retains its canonical native IDs. Existing history is
  preserved byte for byte and new messages are appended after prefix validation.
  Managed open endpoints wait for handover before publication. Unrelated or
  divergent histories are never merged based on matching titles or prompts.
  Interrupted turns retain their saved request, tool records, and explicit
  interruption marker; unpersisted streamed output cannot be reconstructed.
- A complete local transcript is required. Codex inherited-history references,
  subagent conversations, broken Claude parent chains, and Claude project path
  encodings over 200 characters are rejected. Claude compaction can be traversed
  when original messages and a complete pre-boundary branch remain locally;
  the synthetic summary is excluded, while original messages are retained. Claude `/clear` starts a separate
  conversation. The adapter does not promise session continuity across Desktop,
  cloud sessions, context compaction boundaries, or future CLI formats.
- The reader streams source files up to 1 GiB, with a 64 MiB per-record bound and
  a 512 MiB retained-conversation bound. Provider instructions and reasoning do
  not count toward the retained bound. While a CLI appends records, the reader
  verifies that the captured prefix is unchanged and transfers only complete
  records from that checkpoint. Rewrites and replacements retry. Converted copies are limited to 1 GiB;
  the tracked-record limit is 4096. Full histories and archived snapshots consume
  disk space; archived contents remain available locally. Context-window limits and provider-side
  compaction still apply when a CLI resumes a long conversation.

## Implementation and validation

`app/conversation_sync/discovery.rs` performs bounded metadata discovery with a
file-change cache and persisted initial selections. `format.rs` holds the native
adapters, `reader.rs` bounds streamed records, and `mod.rs` owns the worker and
workspace status UI. Managed lifecycle hooks
remain an additional source of transcript references. Account paths are pinned; no authentication
files are copied by the synchronizer. Private files, bounded stable reads,
symlink rejection, a process lock, atomic no-clobber publication and a persisted
retry plan protect original transcripts. Imported fingerprints suppress loops across both destination namespaces. A durable
import baseline and per-destination publication plans make partial failures retryable
without generating another conversation ID. `canonical.rs` validates append-only
updates and provenance-based snapshot retirement; `handover.rs` stores ownership,
and `agent_handover.rs` coordinates managed PTY exit and restart. Codex endpoints are registered with
its native metadata index through `codex --no-daemon app-server` and `thread/read`
(without turns); this prevents an existing `/resume` database from hiding a newly
written file. This metadata read does not start a model turn. Registration has a
bounded deadline, and failure remains visible until a successful retry.

Tests cover full-text round trips, native visible Codex items, bidirectional
continuations, parallel branches, restart idempotence, unsupported content,
partial writes, account isolation, symlinks, progress and UI states. Installed
CLIs were exercised with isolated synthetic sessions and loopback inference:
native `/resume`, visible user/assistant history, exact original messages in the
next request, and parsing/re-exporting native appended responses were checked.
This does not claim live paid-provider inference or the owner's personal history
has been exercised. Local review captures and fixture credentials are not shipped.

Image references: [Claude vision blocks](https://platform.claude.com/docs/en/build-with-claude/vision)
and [Codex native content types](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/protocol/src/models.rs).

References: [Codex app-server](https://learn.chatgpt.com/docs/app-server),
[Codex 0.162.1 native protocol](https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/protocol/src),
[Claude sessions](https://code.claude.com/docs/en/agent-sdk/sessions), and
[Claude lifecycle hooks](https://code.claude.com/docs/en/hooks).

Native indexing reference: [Codex thread/read and list regression](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/app-server/tests/suite/v2/thread_read.rs#L107).

Title references: [Claude session naming](https://code.claude.com/docs/en/sessions),
[Codex name persistence regression](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/app-server/tests/suite/v2/thread_name_persistence.rs),
and [Codex append-only title index](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/rollout/src/session_index.rs).
