---
title: Editor and search
description: Open files from Finder, edit source and configuration files with syntax highlighting, and use language-server completion and project search.
---

Kiln includes a file editor next to your terminals, with syntax highlighting, completion from language servers and a file explorer. Open files, search and problems from the same workspace without leaving your task.

## Open the editor and tools {#open-the-editor-and-tools}

The workspace inspector includes **Files**, which opens the file explorer. The titlebar **Tools** menu lists files, search, source control, GitHub, databases, remote connections and problems together. Choosing one opens it alongside your current work. In compact windows, **Back to Task** returns you to the terminal.

### Open from Finder on macOS {#open-from-finder-on-macos}

In Finder, select a file and choose **Open With → Kiln**. Ordinary files, dotfiles and files without an extension open in the editor. You can open several files together, whether Kiln is already running or starts for the request.

Folders open or focus a workspace. A file uses the most specific existing workspace that contains it. If none exists, Kiln opens a workspace at the file's parent folder. Opening a file that is already open focuses its existing editor and preserves unsaved changes, even when that editor belongs to another workspace.

## File explorer {#file-explorer}

The explorer shows Git-ignored files and folders by default, including `.env`, `.env.prod`, logs and generated directories. You can open them like any other file, by clicking or with keyboard navigation.

- Ignored entries have muted names and icons and a small **Ignored** badge.
- Hover an ignored entry to see which rule excludes it.
- Folders load only when you expand them.
- Choose **Hide ignored files** to filter them out of the view.

Showing or hiding ignored files does not change Git tracking or your repository's ignore rules. Internal `.git` and `.DS_Store` metadata always stays hidden.

## Language detection {#language-detection}

The editor highlights files with bundled grammars. It picks a language from the filename and extension first, then from a shebang or a recognizable content signature such as a JSON document, XML, an HTML doctype or a PHP header. It does not guess a language from ordinary prose.

Common configuration names include:

| Files | Language |
| --- | --- |
| `.env`, `.env.local`, `.env.production.local`, `service.env` | DotENV |
| `.envrc` | Bourne Again Shell (bash) |
| `~/.ssh/config`, `ssh_config`, `~/.ssh/config.d/*.conf`, `ssh_config.d/*.conf` | SSH Config |
| `sshd_config`, `sshd_config.d/*.conf` | SSHD Config |
| `.git/config`, `.gitconfig`, `.gitmodules` | Git Config |
| `.editorconfig`, `.npmrc` | INI |
| `.eslintrc`, `.prettierrc`, `tsconfig.json` | JSON |

In light and dark themes, DotENV separates keys, assignment signs, values and `#` comments, including spaced assignments. Escaped characters and multiline quoted values keep their highlighting; a `#` inside quotes stays part of the value. SSH settings distinguish directives, values, ports, quotes, comments and expansions such as `%h` and `${HOME}`, with highlighting for `Host`, `Match` and `Include`. Names such as `.environment` and `.envoy` do not select DotENV.

Detection covers common formats. If a file needs a different grammar, choose it manually; unrecognized content stays plain text.

To override detection, use the language selector above the document. Choose a language, including **Plain Text**, or return to **Auto detect**.

- Your choice is kept across restarts, for both saved files and files with unsaved changes.
- Editing a script's shebang, or reloading a file changed outside Kiln, updates detection.
- The selected language controls highlighting, comment commands, and the language sent to a configured language server.

## Completion {#completion}

Completion suggestions appear when you type an identifier prefix. You can also open the list yourself.

| Action | How |
| --- | --- |
| Open suggestions | `⌃Space`, `⌥Esc`, or click **Complete** |
| Choose a candidate | Arrow keys |
| Insert | `Enter` or `Tab` |
| Close the list | `Esc` |
| See a full name and details | Hover a truncated entry |

While you compose text with an input method (IME), the IME keeps its own input and candidate keys. Kiln's completion does not take them over.

When you accept a suggestion, Kiln applies the server's full replacement and any additional edits as one undo step. A suggestion that arrives after you have moved the caret or changed the document is discarded.

### Language servers {#language-servers}

Installed language servers provide semantic suggestions. Kiln has defaults for these languages:

| Language | Language server |
| --- | --- |
| Rust | `rust-analyzer` |
| Go | `gopls` |
| JavaScript and TypeScript | `typescript-language-server` |
| Python | `pyright-langserver` or `pylsp` |
| C and C++ | `clangd` |
| Lua | `lua-language-server` |

Install the server for your language, and Kiln uses it. To change languages, file extensions, commands or arguments, edit the `lsp.json` configuration.

Language servers start with your normal `PATH` plus supported install locations and the server's own folder, so servers launched through an interpreter can run. In a project with nested sub-projects, each file uses the nearest recognized project marker. This does not change the working directory of your terminal or workspace.

### Without a language server {#without-a-language-server}

When no language server is connected, Kiln still offers local suggestions, labeled **Language suggestion** or **Document word**.

- Common keywords, literals and built-in names are bundled per language. JavaScript and TypeScript have separate lists.
- Other grammars suggest matching words from the open document.
- Local suggestions are not semantic. They do not resolve imports, types or symbols from other files.
- Scanning is limited so large documents stay responsive. Binary and large-file modes do not offer completion.

## Search and problems {#search-and-problems}

Project search and the problems list are in the titlebar **Tools** menu, next to files and source control. Either one opens beside your current work, so your terminal task stays in place.

## Remote files {#remote-files}

Files on S3, OCI, GCS, R2, FTP and SFTP open in this same editor. See [Remote files and SSH](remote.html#browse-and-edit).

## Windows {#windows}

The editor works the same on Windows. Korean IME composition and commit work in the editor as they do in terminals.
