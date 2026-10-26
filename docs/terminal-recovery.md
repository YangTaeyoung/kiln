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

## Session daemon upgrades

If a daemon upgrade cannot finish, the existing sessions and GUI connection stay
active. Kiln reports the error once and does not repeatedly disconnect to retry the
same upgrade. To retry, open **Settings → About** and, under **Session Daemon**,
choose **Replace with This Version**.

Automatic attempts are scoped to the daemon's PID and build plus the GUI's target
build. A failed or incomplete attempt blocks further automatic attempts for that
identity. An explicit manual retry, a different daemon or a new target build allows
another attempt. Queuing the request does not announce an upgrade or a successful
reconnection. The GUI keeps reading the existing connection until the daemon
actually closes it for handover.

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

For daemon-upgrade reports, record the GUI target build, daemon build and PID, and
the socket in use. The GUI sends an upgrade with a unique request ID and matches
the daemon's framed `ServerMsg::Error` to that request; unrelated errors must not
complete the attempt, and duplicate errors must not produce repeated notices.
`ServerMsg::Upgrading` announces handover, not completion. Check for actual EOF and
the replacement daemon's `Hello` before treating a reconnect as a completed upgrade.
The CLI's separate upgrade-only connection uses request ID zero and must decode
the matching framed error rather than discard the bytes while waiting for EOF.
See the [GUI upgrade handling](../crates/kiln/src/app/conn/upgrade.rs) and
[CLI upgrade client](../crates/kiln-daemon/src/client.rs).

Daemon upgrade failures emit a `daemon upgrade failed:` warning with the underlying
cause. Check it for input-delivery or terminal-host pause failures, then verify that
the current sessions resume and the GUI connection remains usable. Use private
sockets and owned sessions to investigate; do not reproduce a retry loop by
disconnecting a user's active GUI or daemon. Diagnostic evidence should identify
builds, PIDs, request IDs and failure reasons without including terminal contents
or credentials.

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
