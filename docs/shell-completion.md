# Shell command completion

In a newly opened, integrated Zsh terminal, typing `cd` paths or common Git/Docker/npm/Cargo subcommands shows lightweight suggestions. Press **Ctrl+Space** to actively choose a suggestion or look up installed command names. Use the arrow keys to select a result, **Tab** or **Enter** to insert it, and **Esc** to close the menu. Inserting a suggestion never executes the command. Native Tab completion is unchanged whenever this menu is closed.

The menu includes installed command names, common Git/Docker/npm/Cargo options and explanations, and local directory choices for `cd`. It follows Kiln's light or dark theme. Filesystem lookup runs outside the UI thread. Suggestions are bounded; advanced shell expressions, tilde expansion, and edits in the middle of a command remain the responsibility of native shell completion.

This first implementation supports Zsh only. Existing terminals retain their current shell setup; open a new terminal to load the integration. Bash, Fish, SSH sessions, agent chats, alternate-screen applications, and unsupported shells keep their normal input behavior. Integration help is available from the terminal context menu.

Kiln reads safe `cd` and known-command preview contexts, or the current edit buffer after an explicit completion request. Automatic previews decline unknown commands, environment assignments, shell expressions, and argument-value contexts. Turn off command suggestions in Terminal settings to hide automatic previews immediately. Set `KILN_COMPLETION_PREVIEW=0` in your shell to also stop producing preview metadata. Explicit Ctrl+Space remains available. It does not read shell history files, execute commands to generate help, or send suggestions to a cloud service. The buffer is kept in memory; accepting a suggestion briefly writes an owner-only request file which the shell validates and removes. The Zsh line editor verifies the original buffer and cursor before applying a replacement. Closing or upgrading Kiln does not restart running shells.

References: [Kiro autocomplete](https://kiro.dev/docs/cli/autocomplete/), [Zsh line editor](https://zsh.sourceforge.io/Doc/Release/Zsh-Line-Editor.html).

Command descriptions are based on the official [Git reference](https://git-scm.com/docs/git), [npm reference](https://docs.npmjs.com/cli/v11/commands/npm/), [Docker CLI reference](https://docs.docker.com/reference/cli/docker/), and [Cargo command index](https://doc.rust-lang.org/cargo/commands/index.html). Kiln does not execute installed binaries to obtain help.
