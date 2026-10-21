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
`CLAUDE_CONFIG_DIR` / `CODEX_HOME`) and the two connected account directories. Each
of these four locations is a namespace. Every linked conversation appears exactly once
in every namespace, with the same title and working directory, so an ordinary terminal
and a Kiln account panel list the same conversations in `/resume` for both CLIs. Equal
directories are deduplicated. Switching the active account does not redirect the pinned
connection. The status dialog identifies the pinned account pair. Sync remains opt-in.

In the other CLI, open the same project and use **`/resume`** to select the newest
linked conversation. Original user and assistant messages appear in the CLI's normal
conversation display and become part of the next model request. Kiln creates no
summary prompt, handoff note, or extra model request. Different conversations in
the same folder remain separate; workspace discovery does not merge their histories.

## Matching names and readable history

Every copy of a conversation carries one shared name. A rename in any CLI becomes the
name everywhere; the most recent rename wins. Codex names come from its native metadata
API, including names present only in its database; the append-only text index is a
fallback, and a rename is detected even when no new response was written: Kiln asks Codex
again whenever its state database or title index changed. Claude's newest custom-title
record takes precedence over its title sidecar and its generated name, as in Claude Code. Unnamed conversations use the
first prompt with CLI command markup (`<command-name>…`, `<local-command-…>`) and image
placeholders (`<image …>`, `[Image #n]`) removed; sessions without a real prompt are not
synchronized. Claude copies receive the name as a native custom-title record and in
their title sidecar, Codex copies through `thread/name/set`, followed by a metadata read
that checks persistence.

Tool calls appear as the other CLI's own records where it has them:

- Claude Code shows a shell command run in Codex as a `Bash` tool use with its result, and
  a Codex program that runs several commands as parallel `Bash` calls. Claude Code's
  built-in tools (Read, Write, Edit, Glob, Grep, WebFetch, WebSearch, TodoWrite, Agent,
  NotebookEdit) keep their native records in another Claude Code account.
- Codex's resume view shows a shell command as a command cell and every other tool call as
  a tool call cell named by the CLI it came from (`claude_code.Read`, `codex.js`). Codex's
  model receives calls from Claude Code as text; a command run in Codex keeps its native
  code-mode records in another Codex account.
- A tool Claude Code does not define (Codex's browser tool, MCP tools) reads as
  `name(summary)` with its input in a code block and its result after a `⎿` marker.

A call is converted only when its result and a later answer follow it, parallel calls stay
text, and history is never re-run. Conversations synchronized by earlier versions are
written again once in the current form: when a Kiln-written copy has been unchanged for ten
minutes and holds no records its CLI wrote itself, it is archived (recoverable) and its CLI
receives a new copy. Originals, copies continued in their CLI and copies imported by Codex
Desktop are never rewritten.

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

## One canonical conversation per linked conversation

Kiln keeps the canonical history of each linked conversation in its own configuration
folder (`conversation-sync/hub`), independent of either CLI's format. Each namespace
holds one native session of it, called a projection; the conversation's original
transcript is the projection of its own namespace. A turn completed in any CLI is read
into the canonical history and appended to every other projection, keeping their
native IDs. Kiln preserves the existing native log bytes, including provider-private
records, and reads every appended record back before publishing it. Records are appended
in place after the existing bytes are verified, so a CLI that keeps its transcript open
continues writing to the same file; compressed histories are replaced atomically.

When two CLIs continued the same conversation before synchronizing, the side that was
active most recently becomes the canonical history. The other continuation is kept as
an archived branch: its native transcript is moved out of the resume list with an
integrity manifest (Codex through its native archive), its namespace receives a new
projection of the canonical history, and **Restore** in the status dialog turns the
branch into a separate conversation in every CLI. A restore that cannot complete is shown
on its own row and tried again; other conversations keep synchronizing. A native session
that is waiting to leave its resume list and is continued after the canonical history last
changed becomes its namespace's session again, and its history becomes the canonical one.

Copies created by earlier versions are migrated once: each group of recorded copies
becomes one conversation, the most recently modified copy defines its history, each
namespace keeps its most complete matching copy, and the remaining copies are archived.
Migration only records this; the regular check archives the remaining copies (a copy that
continued past the canonical history becomes a restorable branch). A copy that cannot be
archived yet, for example without the Codex CLI, shows the reason on its conversation's
row and is archived once possible. Unrelated conversations that merely share a title stay
separate.

The hub writes a rewritten history to a new file and saves its state after every archive.
After an interrupted check, a projection Kiln had already archived is recognized by its
archive manifest and is not mistaken for a deleted conversation.

## Exclusive handover

Inside Kiln, opening the same linked conversation requests ownership for the new
panel. Kiln interrupts the previous managed CLI and the newly opened reader,
waits for confirmed exits, synchronizes the final saved checkpoint, and reopens
only the requested panel at its canonical ID. Prompts wait while ownership is
being transferred. Capacity recovery and automatic CLI restarts cannot reclaim
an inactive conversation. Other conversations continue running. If a CLI does
not exit or history cannot be verified, the new panel stays waiting with a
visible status; Kiln does not force-kill it or claim that transfer completed.

An archived native ID opened inside Kiln is redirected to the current projection.

Exclusive handover applies to managed Kiln agent panels. External terminals and
other desktop applications do not participate in Kiln's ownership protocol;
close those readers before continuing the same linked conversation in Kiln.

## Read synchronization status

Click a workspace's sync icon or choose **View conversation sync status** from its
context menu. The workspace icon and its status dialog show waiting, working, attention-needed, paused, or a green
check when all completed conversations have been synchronized. During work it
shows the number of transcript records checked and copies written. The list shows one
row per conversation with its shared title, the CLIs holding it, where to continue it
with `/resume`, and, under **Conversation details**, buttons that copy each native ID.

A warning appears only for a problem you can resolve, with its action: a missing
connected account folder (**Open account settings**), full or unwritable storage
(**Try again**), or a missing Codex CLI needed for Codex `/resume` registration.
**Ignore** removes a warning from the badge and count until a different problem occurs;
ignored warnings stay listed under *Show ignored warnings*. A check that failed as a whole
for such a reason offers the same action and **Ignore**. A resolved problem clears on
its own after the next check. Conditions that need no action — a conversation on hold
because it contains a record format Kiln does not know yet, an oversized or incomplete
history, a response still being written — are shown in grey without a badge.
Codex native import provenance is also checked: importing an unchanged Kiln copy
through Codex Desktop does not start another transfer loop or consume an initial
conversation slot. An imported copy without a Codex turn of its own leaves the Codex
resume list and keeps the conversation's Codex session, whatever form the import gave its
tool calls. An imported copy continued in Codex becomes the conversation's Codex session.

The status dialog distinguishes discovery, no conversations found in the open folders, and
waiting for a completed response. A discovered unsupported transcript stays visible
with its failure reason; it is not reported as successfully synchronized. A failed item includes its
reason above its title. Expand **Conversation details** for the original diagnostic or copy it for troubleshooting. A single scrollable list keeps the summary and controls visible. Unchanged background checks retain the last result, row positions, and progress bar; they do not reset completed or failed rows to queued. A conversation whose check failed is checked again when one of its histories changes; otherwise a resolvable problem is retried after 30 seconds, then after 1 and 2 minutes, and other failures after an hour or after Kiln updates. A changed transcript is read once it has been unchanged for 10 seconds, so a response that is still being written is not copied repeatedly. A Kiln handover and **Check again** skip both waits; a handover keeps the results of unchanged files. **Check again** starts a fresh check. Unlinking stops new publication (an atomic write already in progress may finish); existing copies remain available in `/resume`.
The check refers to **completed responses**, not an answer still being generated.

## Compatibility and boundaries

- Kiln reads any Claude Code or Codex version whose records it understands. Records
  that carry no conversation content are skipped; a conversation containing content
  Kiln does not know is put on hold and checked again after Kiln updates, while other
  conversations continue. New copies are written in the **Codex CLI 0.162.1** and
  **Claude Code 2.1.296** formats. Identity and content validation still apply to every
  transcript. Changing working directories within the same Claude session does not
  invalidate it.
- Text messages retain their wording, line breaks, and code blocks. Tool calls are
  native records where the other CLI has a matching tool and readable assistant
  messages otherwise. Historical tools are not executed. Provider instructions, model-specific thinking and
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
  the tracked-record limit is 4096. Full histories and archived branches consume
  disk space; archived contents remain available locally. Context-window limits and provider-side
  compaction still apply when a CLI resumes a long conversation.

## Implementation and validation

Scans run in a separate `kiln sync-worker` process started by the GUI, so reading
large transcripts does not grow the GUI's memory. After a scan whose peak memory
exceeded 512 MiB, and when Kiln closes, the worker saves its file-stamped results to
the private `conversation-sync/worker-cache.json`; the next worker loads and removes
that file, so a restart or update does not read every linked history again. Changing
the linked workspaces or account pair keeps those results; **Check again** clears them. If the worker cannot start, the scan runs inside
the GUI process. Each projection keeps a checkpoint after its last completed turn, so a
grown history is read from that point and its new records are appended to the other
projections without parsing the earlier history; a changed prefix, a Claude branch,
compaction or interruption, a Codex rollback and compressed histories are read in full.
`hub.rs` stores canonical conversations and decides divergence, `engine.rs` adopts,
ingests and projects them, `migrate.rs` converts earlier copies and `checkpoint.rs`
implements incremental reading. `app/conversation_sync/discovery.rs` performs bounded metadata discovery with a
file-change cache and persisted initial selections. `format.rs` holds the native
adapters, `reader.rs` bounds streamed records, and `mod.rs` owns the worker and
workspace status UI. Managed lifecycle hooks
remain an additional source of transcript references. Account paths are pinned; no authentication
files are copied by the synchronizer. Private files, bounded stable reads,
symlink rejection, a process lock and atomic no-clobber publication protect original
transcripts. Projection IDs are derived from the conversation and namespace, so an
interrupted publication is resumed without generating another conversation ID, and
import baselines keep Kiln-written copies from being adopted as new conversations.
`canonical.rs` validates append-only updates and archives native transcripts;
`handover.rs` stores ownership,
and `agent_handover.rs` coordinates managed PTY exit and restart. Codex endpoints are registered with
its native metadata index through `codex --no-daemon app-server` and `thread/read`
(without turns); this prevents an existing `/resume` database from hiding a newly
written file. This metadata read does not start a model turn. One reader per account
directory is reused across scans and restarted after a minute; an unchanged account
directory (state database and title index) is not asked for names again. Registration has a
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
