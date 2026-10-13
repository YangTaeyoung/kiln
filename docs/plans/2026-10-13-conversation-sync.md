# Native conversation synchronization

The requested outcome is to continue a Claude Code conversation in Codex and back
without preparing summaries or handoff notes, including native conversation display
and each CLI's `/resume` picker. Kiln performs synchronization while its GUI is open.

Implementation:

- Add an opt-in Accounts setting that explicitly connects the current Claude Code
  and Codex namespaces. Pin the pair; later account switching never redirects data.
- Reuse managed-panel lifecycle hooks to locate the exact main transcript. Read only
  those transcripts and Kiln-created continuations, not arbitrary account history.
- Parse completed conversations on a background worker. Preserve user/assistant text
  and textual tool inputs/results. Never invoke a model or execute historical tools.
- Publish immutable native continuation sessions. Codex needs both model ResponseItems
  and visible turn items; Claude needs a native UUID/parent message chain. Existing
  sessions are never rewritten, including sessions still open in another CLI.
- Fingerprint canonical conversations and retain the imported baseline to suppress
  feedback loops. Resuming an older copy creates a branch, never a silent merge.
- Surface pending identity, unsupported input and I/O failures. Unsupported attachments
  must not silently disappear. Provide recent continuation IDs and pinned account labels.
- Use private files, path containment, bounded reads, interprocess locking, atomic
  no-clobber publication and durable retry state. Disable stops new publishing.

Validation: format round trips, true bidirectional additions, no-op/restart idempotence,
partial writes, branching, malformed and unsupported input, account isolation, symlink
and collision rejection, settings persistence/UI, and installed CLI resume/display and
local mock inference. Run the release checks, source CI, signing/notarization, public
artifact verification and isolated installer checks before reporting a release.

References: [Codex app-server](https://learn.chatgpt.com/docs/app-server),
[Claude sessions](https://code.claude.com/docs/en/agent-sdk/sessions),
[Claude hooks](https://code.claude.com/docs/en/hooks), and the Codex `rust-v0.162.1`
`protocol/src/protocol.rs`, `protocol/src/items.rs`, and thread history projection.
