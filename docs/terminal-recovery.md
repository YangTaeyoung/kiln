# Terminal connection recovery

Kiln keeps the terminal connection separate from the shell or agent running inside it.
Recovery repairs that connection, republishes the screen and requests a CLI redraw; it does not restart the
agent, send a prompt, or run `/resume` on your behalf.

## When a panel stops responding

A lost PTY-host connection or a terminal reader that stops servicing its heartbeat
starts automatic recovery. Kiln tries up to three times. A quiet agent that is
thinking or waiting for input is not considered stalled merely because it has no output.

The panel and workspace task list show **Recovering** during repair. If the bounded
attempts fail, both show **Connection stalled** and the panel offers **Reconnect**.
That action is also available in every running terminal panel's **…** menu, including
when the connection still reports healthy. It resets the retry budget for that panel.

The existing session and child process are preserved. Previously displayed text stays
visible during recovery. Input is paused while recovery is reported, so typing does not
silently queue new commands behind a broken connection.
Completed panels retain their screen and ignore delayed connection-recovery events.

## What this fixes

- Continuously renewed synchronized redraw markers cannot hide the screen indefinitely.
- Host-transport loss is distinct from a shell exiting; blocked Unix writes have a deadline.
- A replacement host connection restores the input worker and screen publication.
- Reconnection starts consuming buffered output before waiting for resize controls or
  queued input, avoiding a backlog/input deadlock.
- A busy panel's emulator does not block another panel's frame publication.
- GUI message processing and outbound batching have bounded work per iteration.

## Limits and investigation

Connection recovery does not diagnose every internal Codex or Claude CLI failure.
A CLI can be unresponsive while its PTY connection remains healthy. The menu action
is available in that situation, but it is not a promise to resume an internally hung agent.
If connection repair does not help, open another panel and use the CLI's own resume flow.
Already accepted input is never deliberately replayed; a partially decoded output chunk
at transport loss is not byte-for-byte replayed into the terminal.

The reported past freeze had already been resumed before investigation, so its exact
cause is not established. The fixes above have separate local regression evidence.
Similar upstream reports are useful comparisons, not proof of the same cause:

- [cmux keyboard responder repair](https://github.com/manaflow-ai/cmux/pull/2505)
- [Codex concurrent-session freeze report](https://github.com/openai/codex/issues/20213)
- [Codex terminal-query blocking report](https://github.com/openai/codex/issues/24527)
- [Codex stale TUI while work continues](https://github.com/openai/codex/issues/30714)

Maintainers: test with private sockets and owned PTYs. Never stop the user's daemon or
restart a real agent to reproduce a failure. `crates/kiln/tests/terminal_recovery.rs`
exercises native and hosted redraw recovery and an interrupted hosted connection with
3 MiB of buffered output, 2 MiB of queued input, unchanged child PID, and a responsive
second panel. Recovery state and UI tests cover retry exhaustion, manual retry, all four
languages, narrow panels and light/dark appearance. Manual repair also verifies a real redraw
signal with the same child PID in both PTY modes. Recovery transitions log only
session IDs, state, attempt counts and connection/reader liveness, never terminal text.
