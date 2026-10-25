# Shell command completion

New integrated Zsh terminals show a compact completion menu next to the cursor.
It uses the terminal font, Kiln's theme and vector icons, and stays inside the
active terminal pane. Type a command or option to filter the list.

| Key | Action |
| --- | --- |
| Tab | Insert the highlighted suggestion |
| Down | Start choosing from the menu, then move down |
| Up | Move up after selecting the menu; otherwise keep shell history navigation |
| Enter | Run the original command until the menu is selected; then insert the selected suggestion |
| Escape | Close the menu |
| Control+Space | Open suggestions explicitly or reopen a dismissed menu |

Inserting a suggestion edits the shell buffer and never submits it. When the
menu is closed or has no suggestions, native Tab completion remains available.
Clicking an item also inserts it and keeps terminal focus.

## Commands, options and arguments

The shell supplies installed command names and resolves the executable with its
own PATH. Kiln reads structured `--help` output for recognized commands and nested
subcommands. This discovers command and option names from the installed version
without adding a specification for each CLI. Supported layouts include Clap,
Cobra, Click and argparse help. Help that cannot be recognized falls back to the
bundled catalog and native shell completion.

The bundled catalog supplies common Git, Cargo and package-manager explanations.
The Docker Engine, Buildx and Compose fallback is generated from pinned official
machine-readable references, including visible documented options and aliases.
See [catalog provenance and regeneration](maintainers/completion-sources.md).
It does not inspect a Docker daemon, cluster or remote registry.

Argument kinds change the choices:

- `cd ` offers directories under the current shell directory. Inserting a
  directory exposes its children. Spaces, quotes and Unicode names are escaped
  for the shell.
- File arguments such as `cat `, `open ` and `git add ` offer local files and
  folders. Hidden entries appear when the current path component starts with a dot.
- Directory options such as `docker --config` and
  `docker compose --project-directory` offer directories. Compose `--file` and
  `--env-file` offer files. Container working directories remain literal values.
- `git switch` and related branch arguments read local loose and packed refs,
  including Git worktrees. The repository is not changed.
- `npm run`, `pnpm run`, `yarn run` and `bun run` read script names from the nearest
  `package.json`. They do not execute those scripts.
- Typed option values, such as commit messages, are not treated as subcommands.
  Recognized enum values and `--option=value` forms have their own completion path.

Filesystem and help discovery run outside the UI thread. Results are bounded and
superseded requests are replaced by the latest edit. An accepted item is applied
only if the originating shell buffer, cursor and revision still match.

## Installed help discovery

Help probes use a resolved executable in a supported system or package-manager
installation directory, or a supported user installation such as `.local/bin`
or `.cargo/bin`. Project executables, shells, interpreters, GUI entry points and
daemons are not automatically probed. Docker Desktop's packaged CLI paths are
recognized separately from its GUI.

Each probe receives only recognized subcommand names and a final `--help`, with
closed stdin, a disposable home/working directory and a minimal environment.
Typed arguments, credentials and shell history are not forwarded. A probe has a
one-second deadline, 256 KiB combined output limit and a maximum command depth of
four. Its process group or Windows Job is cleaned up on completion or timeout.

The in-memory cache holds at most 128 entries. Positive results expire after
30 minutes, failures after five minutes, and changing the installed binary
invalidates its results. A CLI that needs its ordinary configuration or does not
provide recognizable help uses the fallback instead.

## Availability and controls

Open a new integrated Zsh terminal after updating Kiln. Existing shells keep
their current integration. Bash, Fish, SSH sessions, agent chats, alternate-screen
applications and unsupported shells retain their normal input behavior.

Turn off **Command suggestions** in Terminal settings to hide automatic menus.
Set `KILN_COMPLETION_PREVIEW=0` in the shell to stop preview metadata as well.
Control+Space remains available. Expansion expressions, pipelines, redirections
and editing before the end of the buffer use native shell completion.

Completion requires no agent account or model connection. It reads no history
files. Shell edit metadata is kept in memory and filtered before terminal logging;
acceptance briefly uses a private request file that Zsh validates and removes.
Closing or updating the GUI does not restart running shells.

References: [Fig command/option specifications](https://fig.io/docs/reference/option),
[Kiro's command completion menu](https://kiro.dev/docs/cli/autocomplete/),
[Zsh line editor](https://zsh.sourceforge.io/Doc/Release/Zsh-Line-Editor.html),
[Clap help](https://docs.rs/clap/latest/clap/_tutorial/index.html),
[Click help](https://click.palletsprojects.com/en/stable/documentation/),
[Cobra help](https://pkg.go.dev/github.com/spf13/cobra),
[Git repository layout](https://git-scm.com/docs/gitrepository-layout),
[npm scripts](https://docs.npmjs.com/cli/v11/commands/npm-run/).
