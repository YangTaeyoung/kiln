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

## Tables, columns and indexes

Open a table and use **Data**, **Columns**, **Indexes** or **DDL**. The explorer's
context menu opens the same sections and actions, including an already open tab.
The table menu offers rename and delete. Column rows offer edit, rename and drop;
use **Add column** for a new name, SQL type, nullability and optional SQL default.
String defaults need SQL quotes, for example `'guest'`, while `CURRENT_TIMESTAMP`
is an expression. Generated/identity columns and primary-key removal require
manual DDL rather than a partial form that would silently change their properties.

The **Indexes** section lists key columns in order, uniqueness, method and full
available definitions. Partial predicates, included columns and invalid status
appear when supplied by the database. Add an index by selecting columns, adjusting
their order and direction, and optionally enforcing uniqueness. Primary-key and
constraint-owned indexes cannot be dropped through this form. Expression, partial,
included-column and engine-specific index creation remain available in the SQL
console; the form creates ordinary column indexes.

Every form follows **Preview SQL → review → Apply to database**. Opening the form
and previewing do not execute DDL. Editing a field invalidates the preview and its
approval. Deleting a table also requires its exact name. Failed operations keep
your input for correction. Restored forms keep input only: saved SQL, approval and
execution are never replayed. If Kiln closed during an operation, inspect the
actual database result before preparing another change.

Schema operations require the existing connected pool. A changed connection or
reviewed structure invalidates a plan. Changes in another table tab refresh clean
views and preserve pending row edits while blocking submission against stale
metadata. Successful operations refresh the explorer and completion metadata;
external DDL can be refreshed explicitly. Renamed tables retain the correct tab
identity and restore target. Deleted tables are not restored on restart.

PostgreSQL applies changes transactionally after acquiring a table lock and
revalidating raw catalogs on that connection. MySQL/MariaDB apply a single native
DDL statement; no multi-statement rollback or atomic external-DDL race guarantee
is implied. SQLite attribute edits rebuild within a transaction, preserving
values, generated columns, rowids, sequence high-water marks, table constraints,
explicit indexes, triggers and dependent views, then check foreign keys before
commit. A failed copy or validation rolls back. Dependent objects can prevent a
column or table drop; Kiln does not silently cascade their deletion.

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

Schema references: [DataGrip modification dialogs](https://www.jetbrains.com/help/datagrip/create-and-modify-dialogs.html),
[SQLite ALTER TABLE](https://www.sqlite.org/lang_altertable.html),
[PostgreSQL ALTER TABLE](https://www.postgresql.org/docs/current/sql-altertable.html),
and [MySQL ALTER TABLE](https://dev.mysql.com/doc/refman/8.4/en/alter-table.html).
