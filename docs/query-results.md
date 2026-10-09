# Running and editing query results

**Run** and **Command+Enter** execute the statement at the editor caret. Indentation
before a statement on its line belongs to that statement. A semicolon belongs to
the preceding statement. Select text to execute only that selection; a blank or
comment-only selection executes nothing. **Run all** and **Command+Option+Enter**
are explicit ways to execute the entire editor. SQL strings, comments and dialect
quoting do not split a statement at an embedded semicolon.

## Edit a SELECT result

For a supported single-table SELECT, double-click a cell or press Enter/F2 to
change it. Changes remain local until **Apply changes**. **Revert** discards them.
The value sidebar supports longer values. Stage its text with **Apply to cell**
or reset it before changing the selected cell or running another query.
Copying selected cells includes staged values.

The result must identify one permanent base table and direct source columns.
Aliases and filters are allowed. Include every column of a primary key, or an
eligible non-null unique key. The key does not need to be named `id`.
A read-only result explains why editing is unavailable. Joins, aggregates,
computed projections, duplicate source columns, views, temporary tables and
queries without the complete row key remain read-only. Generated, binary and
unsupported typed columns are not editable. Use SQL for operations outside this
safe subset; result editing does not offer row insertion or deletion.

Kiln captures source metadata on the query's exact connection and verifies native
column origins. Catalog discovery is bounded; if it cannot establish the proof,
the result still displays normally and remains read-only. An update uses bound
parameters, the row key, and the projected original values. Each changed row must
match exactly one database row. A deleted, externally changed or ambiguous row
fails the entire transaction and retains local changes for review. A subsequent
successful refresh captures new originals for another edit.

Applying changes uses a separate pooled transaction against the fully qualified
base table. Within that transaction, it locks the target and rechecks its catalog
identity and column/key definitions before writing. PostgreSQL checks relation
identity; SQLite checks the main database schema version, so intervening schema
changes require a fresh query. MySQL/MariaDB check the table definition, engine,
keys and creation time while holding a metadata lock. An identically defined
MySQL table recreated within the same creation-time second cannot be distinguished
without privileged engine metadata; row-key and original-value checks still apply.
It does not commit a console's explicit `BEGIN`. Uncommitted console
values cannot be silently used to change a different committed row. PostgreSQL,
MySQL/MariaDB with transactional InnoDB tables, and SQLite use their own catalog,
quoting, comparison and parameter behavior. If a write succeeded but refreshing
failed, Kiln reports that the write was saved and makes the stale result read-only;
it never resubmits the old change automatically.

## Closing and recovery

Kiln blocks closing the tab, workspace or application while a submitted database
write is still running. Hidden database tabs continue polling completion, so this
block clears after the write finishes. Keeping work on exit preserves staged cell
values and unfinished grid/sidebar text in the SQL document's local recovery
record, including the complete original result rows. These local recovery files
can contain database values; they do not contain a live write authorization. On restart, they appear in a **review-only** section with the original
query and values, copy/export actions and confirmed deletion. Run the query
manually to compare current rows before re-entering a change; no saved write plan,
UPDATE, query execution or submission is replayed. After an abrupt exit during
submission, check whether the database already committed before applying again.

## References

The execution interaction was checked against [DataGrip query
execution](https://www.jetbrains.com/help/datagrip/run-a-query.html); result-grid
editing against [DataGrip query
results](https://www.jetbrains.com/help/datagrip/viewing-query-results.html).
Kiln deliberately supports a stricter editing subset than tools that let users
choose a target table for a join. See [database tools](databases.md) for table
structure editing and [development](development.md) for isolated fixture tests.
