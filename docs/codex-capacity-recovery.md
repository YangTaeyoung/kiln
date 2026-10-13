# Codex capacity recovery

While Kiln is open, managed Codex panels retry a completed turn whose native
`task_complete`/`turn_complete` event contains `error.codex_error_info =
"server_overloaded"`. Display text alone never triggers recovery. Usage limits,
authentication failures, transient stream errors, aborted turns and successful
turns are not capacity failures.

Retries wait 15, 30, 60, 120 and 240 seconds. The panel shows the countdown and a
Cancel auto-retry button. Keyboard input after the failure cancels the pending
retry, including a partially typed prompt. A new manually submitted task starts
a new retry budget. Automatic retries do not change the account or model.

An idle native Codex TUI receives one explicit continuation prompt in the same
conversation. If the CLI has exited, Kiln resumes the exact confirmed UUID in its
original account namespace and working directory, then supplies that continuation
prompt. The prompt asks Codex to check current state before repeating a completed
action; it does not replay the original request. Side effects from the previous
turn are not rolled back.

Only hook-confirmed, account-pinned panels are eligible. Read-only checks validate
transcript identity, workspace, bounded native event data and an unchanged file
checkpoint immediately before dispatch. A newer turn or user intervention
supersedes the retry. Older failures in a still-running CLI opened before Kiln's
current observation are left for manual action. Transcript files are never edited.

Sources checked against Codex 0.162.1:

- [Native error definitions](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/protocol/src/error.rs)
- [TurnCompleteEvent and ErrorEvent](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/protocol/src/protocol.rs)
- [CLI resume reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- Local `codex resume --help`: `codex resume [OPTIONS] [SESSION_ID] [PROMPT]`.
