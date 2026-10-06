# File editor

The editor highlights files using bundled syntect grammars. Automatic detection
uses the filename and extension first, then a shebang or a conservative content
signature. Content inspection is bounded: it recognizes JSON documents, XML,
HTML doctypes and PHP headers, rather than guessing from ordinary prose.

Use the language selector above the document to override detection, including
**Plain Text**, or return to **Auto detect**. The selection is preserved with clean and unsaved
files across application restarts. Changes to a script's shebang and
external reloads update detection. Language selection controls highlighting,
comment commands and the language identity sent to a configured language server.

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
keyword** or **Document word**. These are not semantic analysis: they do not
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

- [syntect SyntaxSet detection](https://docs.rs/syntect/latest/syntect/parsing/struct.SyntaxSet.html)
- [Language Server Protocol completion](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#completionItem)
