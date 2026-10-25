# File editor

## Open from Finder on macOS

In Finder, select a file and choose **Open With → Kiln**. Ordinary files,
dotfiles and files without an extension open in the editor. You can open several
files together, whether Kiln is already running or starts for the request.

Folders open or focus a workspace. A file uses the most specific existing
workspace that contains it; if none exists, Kiln opens a workspace at the file's
parent folder. Opening a file that is already open focuses its existing editor,
including across workspaces, and preserves unsaved changes.

## Language detection

The editor highlights files using bundled syntect grammars. Automatic detection
uses the filename and extension first, then a shebang or a conservative content
signature. Content inspection is bounded: it recognizes JSON documents, XML,
HTML doctypes and PHP headers, rather than guessing from ordinary prose.

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

In both light and dark themes, DotENV distinguishes keys, assignment signs,
values and `#` comments, including spaced assignments. Quoted values keep
escaped characters and multiline content; a `#` inside quotes remains part of
the value. SSH client and server settings distinguish directives, values,
ports, quoted strings, comments and expansions such as `%h` and `${HOME}`.
`Host`, `Match` and `Include` also receive highlighting. Similar names such as
`.environment` and `.envoy` do not select DotENV.

Detection covers common formats. Use a manual language choice when a file needs
a different grammar; unrecognized content stays plain text.

Use the language selector above the document to override detection, including
**Plain Text**, or return to **Auto detect**. The selection is preserved with clean and unsaved
files across application restarts. Changes to a script's shebang and
external reloads update detection. Language selection controls highlighting,
comment commands and the language identity sent to a configured language server.

## File explorer

Git-ignored files and folders are visible by default, including `.env`,
`.env.prod`, logs and generated directories. Muted names and icons plus a small
**Ignored** badge distinguish them from other entries; hover explains the rule.
Click or use keyboard navigation to open them normally. Folders load only when
expanded. Visibility does not change Git tracking or repository rules.

**Hide ignored files** is an explicit view filter, rather than the default.
Internal `.git` and `.DS_Store` metadata stays hidden independently of Git rules.

The reference behavior distinguishes file visibility from Git tracking:
[VS Code Explorer](https://code.visualstudio.com/docs/editing/getting-started/userinterface#_explorer).

## Completion

Type an identifier prefix, use **Control+Space** / **Option+Escape**, or click
**Complete**. Arrow keys select a candidate; plain **Enter** or **Tab** inserts;
**Escape** closes the list. Hover a truncated entry for its full name and details.
IME composition keeps ownership of its input and candidate keys.

Installed language servers provide semantic suggestions. Defaults cover
Rust (`rust-analyzer`), Go (`gopls`), JavaScript/TypeScript
(`typescript-language-server`), Python (`pyright-langserver` or `pylsp`),
C/C++ (`clangd`) and Lua (`lua-language-server`). Customize languages, extensions,
commands and arguments in the existing `lsp.json` configuration.

Without a connected language server, local suggestions are labeled **Language
suggestion** or **Document word**. Language lists are separate, including JavaScript
and TypeScript; they include common keywords, literals and built-in names.
These are not semantic analysis: they do not
resolve imports, types or symbols from other files. Common language keyword sets
are bundled; other grammars still offer matching words from the open document.
Local scanning is bounded to avoid blocking large documents. Binary and large
file modes do not request completion.

LSP subprocesses inherit normal PATH entries plus supported install locations
and the server's directory so interpreter-based launchers can run. Each nested
project uses its nearest recognized project marker for LSP; the terminal and
workspace working directory are unchanged. Completion applies the full server
replacement range and additional edits in one undo group. Responses received
after the document or caret changes are discarded.

## Implementation references

- [Finder Open With](https://support.apple.com/guide/mac-help/choose-an-app-to-open-a-file-on-mac-mh35597/mac)
- [DotENV values, comments and spacing](https://nodejs.org/api/environment_variables.html#dotenv)
- [OpenSSH client configuration](https://man.openbsd.org/ssh_config), [OpenSSH server configuration](https://man.openbsd.org/sshd_config)
- [syntect SyntaxSet detection](https://docs.rs/syntect/latest/syntect/parsing/struct.SyntaxSet.html)
- [Language Server Protocol completion](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#completionItem)
- [Java lexical structure](https://docs.oracle.com/javase/specs/jls/se25/html/jls-3.html#jls-3.9), [Kotlin keywords](https://kotlinlang.org/docs/keyword-reference.html), [Swift lexical structure](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/lexicalstructure/), [C# keywords](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/)
