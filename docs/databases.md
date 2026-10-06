# Database tools

Open **Database** in the workspace inspector, add a connection, then open a table
or SQL console. PostgreSQL, MySQL, MariaDB and SQLite use their own identifier
quoting and metadata queries. Opening database tools does not change the workspace
root or move an agent into another repository.

## SQL completion

Suggestions appear as you type an identifier or an alias followed by a dot. You
can also use **Control+Space**, **Option+Escape**, or the console's code icon.
Command+Space is accepted when macOS delivers it to Kiln; the app does not override
Spotlight. Control+Space may be reserved for switching input sources, so the icon
and Option+Escape remain available without changing your Mac's shortcuts.

- After `FROM` or `JOIN`, choose a table or schema. Inserted table names are
  schema-qualified so Kiln does not guess a search path or current database.
- In `SELECT`, `WHERE`, `JOIN ... ON`, `UPDATE ... SET` and an `INSERT` column list,
  get columns for the referenced tables, including aliases defined after the caret.
  Columns shared by joined tables include the alias; self joins keep each alias.
- The connection's dialect supplies common SQL keywords and function names.
  Function insertion puts the caret inside the parentheses.
- Use arrows or the mouse wheel to browse; **Tab** or plain **Enter** inserts;
  **Escape** dismisses. **Command+Enter** still executes the current statement;
  **Command+Option+Enter** executes all statements. Completion never runs SQL.
- Hover a candidate for its full name and metadata. Use the refresh icon after
  external schema changes. Successful schema edits executed in this console
  invalidate the metadata cache automatically.

Completion preserves undo/redo and ignores selected text, comments, string
literals and active IME composition. Metadata loads asynchronously from an
existing connection; it does not reconnect a connection you explicitly closed.
Failures leave your SQL intact and can be retried with the refresh icon.

## Scope and connection state

Completion reads catalog metadata through the connection pool, independently of
the console's query session. Session-local `TEMP` objects, SQLite `ATTACH`, MySQL
`USE` and PostgreSQL `search_path` changes are not mirrored into that catalog.
Use explicit schema names for persistent tables. CTEs and derived-table columns
are not inferred; Kiln suppresses physical-table suggestions that would falsely
claim those columns. Keyword/function suggestions cover common syntax, rather
than every server extension or server-version-specific function signature.

SQL completion does not send query text or table rows to an external AI service.
Connection secrets stay in the configured credential store. Review SQL and pending
table changes before executing or applying them.

References: [DataGrip completion](https://www.jetbrains.com/help/datagrip/auto-completing-code.html),
[DBeaver SQL assist](https://dbeaver.com/docs/dbeaver/SQL-Assist-and-Auto-Complete/),
[PostgreSQL identifiers](https://www.postgresql.org/docs/current/sql-syntax-lexical.html),
and [macOS Spotlight shortcuts](https://support.apple.com/en-gb/guide/mac-help/mh26783/26/mac/26).
