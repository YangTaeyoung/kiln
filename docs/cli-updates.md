# CLI versions and conversation restart

Open **Settings → Accounts → CLI updates** to see the installed executable,
installed version and latest published package version. Version checks run in
background workers. The installed version appears as soon as its single local
probe completes; the latest-package lookup has its own visible pending/error
state and never withholds the installed version. Enable **Automatically update Codex and Claude Code** for
six-hour checks while Kiln is open, or update either CLI explicitly. Both automatic updating and
immediate session restart are off by default.

Claude Code uses its official `claude update` command, preserving its configured
update channel. Codex uses its detected Homebrew or npm installer. Custom or
unrecognized installations report that their installer is required; Kiln does
not invoke `codex update`, which can restart the shared background server. Kiln
never invokes sudo or replaces an unknown custom launcher. The latest package
label describes the public npm stable version, which can differ from a Claude
installation's configured stable channel. An installer must complete and the
executable must report a changed version before an update-triggered restart.

## Restart immediately after an update

Enable **Restart sessions in the same conversation immediately after updates**
to interrupt a managed panel's current response and reopen its exact saved
conversation using the new executable. The panel, layout, working directory and
account configuration are retained. This restores the conversation; it does not
replay a previous user request or automatically repeat tool calls.

Kiln gives the CLI its normal Ctrl-C exit sequence, with a bounded timeout. It
waits for the terminal process to report exit. A child shell in the foreground
is not evidence of exit. If the CLI refuses to stop, Kiln leaves it running and
shows an error. It does not force-kill a live agent to finish the upgrade. After
launch, a lifecycle callback must confirm the original conversation ID. Failure
to confirm produces a visible warning and a manual retry path.

Use the panel's **… → Restart in the same conversation** action for manual
restart. The same checks apply. Panels without a confirmed conversation UUID or
a pinned saved-account configuration are not automatically interrupted.

## Conversation ID tracking

New **Codex**, **Claude Code**, and **Start task** panels receive additive
`SessionStart` and `UserPromptSubmit` command hooks. These report the real
conversation UUID into a private per-launch record, rather than guessing the
latest transcript in a folder. Two panels in the same folder have independent
records. Stale callbacks and subagent callbacks cannot replace another panel's
identity. Open the panel menu to inspect its CLI version and conversation ID.

Codex requires trust for a new non-managed hook definition. If tracking is
pending, review the Kiln hooks in `/hooks`, trust them, and send the next message.
The callback command stays the same across panels and restarts. Kiln does not
bypass hook trust or enable hooks forbidden by administrator policy. When hooks
are disabled or unavailable, automatic restart is unavailable.

If a Start task terminal disappears, Kiln resumes its recorded conversation instead of resending the original request. A request that may already have been sent but has no conversation ID requires an explicit new task.

Pre-existing panels and CLIs typed manually into an ordinary shell are not
retroactively instrumented. Use a new Kiln agent panel to enable tracking. The
tracking record contains identifiers, executable path and environment paths;
it does not store hook payloads, transcripts or credential tokens. Login
credentials remain in the CLI-owned account namespace and Keychain.

## Maintainer checks

Use fake CLI executables, isolated configuration and an owned session daemon.
Exercise two conversations in the same directory, token rotation in separate
accounts, explicit re-login, logout, hook trust, stale callbacks, update failure,
exit timeout, exact UUID resume and original-account preservation. Never update
or terminate a developer's real CLI sessions as part of a test. Native provider
sign-in expiration cannot be proven by a short fixture test.

References:

- [Codex lifecycle hooks and trust](https://learn.chatgpt.com/docs/hooks)
- [Codex commands and exact session resume](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
- [Claude hooks](https://code.claude.com/docs/en/hooks)
- [Claude CLI flags](https://code.claude.com/docs/en/cli-reference)
- [Claude update channels](https://code.claude.com/docs/en/setup#auto-updates)
- [Claude account isolation](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts)

Managed panels keep their original account. Legacy account rotation is disabled for these panels; when a usage limit is reached, choose another account in Settings and open a new panel. Kiln does not transfer private conversation history between accounts. Verified legacy Codex Keyring namespaces retain that store during new launches
and conversation resume rather than being forced into an empty `auth.json`.
Package-manager updates verify the selected executable belongs to the active npm global root or Homebrew prefix before installing. Version probes run in the PTY launch helper or a background worker, never on the UI thread.
