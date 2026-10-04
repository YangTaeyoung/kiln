# Terminal integration

Kiln supports OSC terminal notifications and explicit agent activity. These are
local terminal escape sequences; they do not send your prompt to another service.

```sh
kiln activity running
kiln activity waiting
kiln activity done
kiln activity failed
```

Use `kiln activity --help` and `kiln notify --help` for the installed CLI syntax.
The notification center retains recent notifications and links them to their sessions.
Reading a notification clears its associated attention state.

New default zsh sessions receive shell integration for command boundaries and
working-directory tracking. Existing shells may require a new session before
new integration behavior applies. Plain shells and external agents that do not
report activity are shown as open sessions rather than falsely marked complete.

Run `kiln status` to inspect the session daemon and `kiln ls --json` to list sessions.
Do not use `shutdown-daemon` as a way to refresh the GUI: it terminates sessions.

## Attach an existing zsh session

Automatic integration currently applies to a new default zsh session, without
modifying your `.zshrc`. Custom launches with arguments, bash/fish and Windows
shells do not receive automatic injection. Set `KILN_SHELL_INTEGRATION=0` to disable it.

To initialize the integration files, choose `/bin/zsh` as the default shell in
Settings and open a new Kiln terminal. Then, at an idle prompt in the existing zsh:

```zsh
source ~/.local/share/kiln/shell-integration-v1/integration.zsh
```

Wait for any active foreground task to finish before sourcing it. Starting `zsh`
inside an existing terminal does not itself perform Kiln's new-session setup.
The integration also provides `kiln-agent-status running|waiting|done|failed|unknown`.
Hook output must reach the Kiln terminal for the activity sequence to be received.

## Command history limits

The daemon keeps the most recent **40 commands**, with up to the first **128 KiB**
of output per command, in memory. This command history does not survive a daemon
restart or upgrade. It is a reconstructed text view, not a raw terminal log or a
record of every intermediate screen of a full-screen TUI.

Without command-boundary integration, only the currently visible output is
available. An edited history command runs only through its explicit execution
action in a new terminal. The readable-output view does not imply full VoiceOver
support for the entire terminal surface.
