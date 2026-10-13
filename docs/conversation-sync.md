# Claude Code ↔ Codex conversations

In **Settings → Accounts → Conversation continuity**, choose **Connect workspace
conversations**. While the Kiln window is open, synchronization discovers native
Claude Code and Codex transcripts whose working directory is inside an open
workspace, including its subfolders. Existing sessions and sessions opened outside
Kiln are included; reopening a panel or receiving a lifecycle hook is not required.

The initial synchronization selects the **20 most recently modified conversations
per workspace**, across both tools and their default/connected namespaces. The
selection is persisted so restarting Kiln continues the same initial batch. New
and modified conversations are discovered afterward, including an older session
that saves another update. Closing a workspace pauses its synchronization; opening
a different workspace starts that folder's own initial batch. Overlapping folders
are assigned to the nearest open workspace, and duplicate workspace paths count once.

Discovery checks both the ordinary CLI directories (`~/.claude`, `~/.codex`, or
`CLAUDE_CONFIG_DIR` / `CODEX_HOME`) and the two connected account directories.
Completed transcripts are made available in both opposite-tool locations, so an
ordinary terminal and a Kiln account panel can each use `/resume`. Equal directories
are deduplicated. Switching the active account does not redirect the pinned
connection; connect again to change the account destinations. Sync remains opt-in.

In the other CLI, open the same project and use **`/resume`** to select the newest
conversation copy. Original user and assistant messages appear in the CLI's normal
conversation display and become part of the next model request. Kiln creates no
summary prompt, handoff note, or extra model request. Different conversations in
the same folder remain separate; workspace discovery does not merge their histories.

## Read synchronization status

The connection card shows waiting, working, attention-needed, paused, or a green
check when all completed conversations have been synchronized. During work it
shows the number of transcript records checked and new copies created. The list
shows each source conversation's first prompt, project, direction, status, and a
button to copy its continuation ID. Copies that have not been continued are
hidden from the list to avoid displaying both sides of the same transfer.

The card distinguishes discovery, no conversations found in the open folders, and
waiting for a completed response. A discovered unsupported transcript stays visible
with its failure reason; it is not reported as successfully synchronized. A failed item includes its
reason and is retried on subsequent checks. Pausing keeps the last result visible
and stops new publication (an atomic write already in progress may finish); existing copies remain available in `/resume`.
The check refers to **completed responses**, not an answer still being generated.

## Compatibility and boundaries

- This experimental adapter supports **Codex CLI 0.162.1** and **Claude Code
  2.1.296** transcript formats. Unknown transcript versions fail visibly instead
  of being silently converted. A CLI auto-update may require an adapter update.
- Text messages retain their wording, line breaks, and code blocks. Textual tool
  calls and results are represented as labeled assistant messages; historical
  tools are not executed. Provider instructions, model-specific thinking and
  signed reasoning are not transferred. Images, audio, files and unrecognized
  content types fail visibly. They are not silently dropped.
- Each changed, completed transcript creates a **new native session**. An already
  open CLI keeps its current history until you select the latest copy. Kiln never
  edits an existing conversation or merges parallel branches. Continuing an older
  copy creates a separate branch. Earlier copies remain in the resume picker.
- A complete local transcript is required. Codex inherited-history references,
  subagent conversations, compacted or broken Claude parent chains, and Claude project path
  encodings over 200 characters are rejected. Claude `/clear` starts a separate
  conversation. The adapter does not promise session continuity across Desktop,
  cloud sessions, context compaction boundaries, or future CLI formats.
- Sources and converted copies are limited to 64 MiB each and 4096 tracked
  records per connected pair. Full copies consume disk space; Kiln does not
  automatically delete CLI history. Context-window limits and provider-side
  compaction still apply when a CLI resumes a long conversation.

## Implementation and validation

`app/conversation_sync/discovery.rs` performs bounded metadata discovery with a
file-change cache and persisted initial selections. `format.rs` holds the native
adapters, and `mod.rs` owns the worker and Accounts UI. Managed lifecycle hooks
remain an additional source of transcript references. Account paths are pinned; no authentication
files are copied by the synchronizer. Private files, bounded stable reads,
symlink rejection, a process lock, atomic no-clobber publication and a persisted
retry plan protect original transcripts. Imported fingerprints suppress loops across both destination namespaces. A durable
import baseline and per-destination publication plans make partial failures retryable
without replacing an existing conversation. Codex copies are also registered with
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

References: [Codex app-server](https://learn.chatgpt.com/docs/app-server),
[Codex 0.162.1 native protocol](https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/protocol/src),
[Claude sessions](https://code.claude.com/docs/en/agent-sdk/sessions), and
[Claude lifecycle hooks](https://code.claude.com/docs/en/hooks).

Native indexing reference: [Codex thread/read and list regression](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/app-server/tests/suite/v2/thread_read.rs#L107).
