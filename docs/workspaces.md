# Workspaces and agents

## One workspace, several repositories

Open a parent folder such as `~/dev/projects` when frontend, backend and shared
libraries live in separate Git repositories. Kiln keeps that folder as the agent's
working root and can include the discovered repository layout with your request.
It does not switch the task's working directory merely because you inspect a repository.

Use the repository selector inside Git/GitHub tools to target a specific repository
for Git operations. A workspace is a navigation and agent context boundary, not a
replacement for each repository's Git history.

The right-hand workspace inspector includes Files, Changes, GitHub and a database
icon. Select the database icon to browse connections and tables or open a SQL
console. At narrow widths, tabs use icons with tooltips; the inspector remembers
the last selected tool when closed and reopened.

## Tasks and terminals

- The sidebar previews up to three tasks per workspace, with an option to show all.
- Running work appears first, followed by requests awaiting input and older failures.
  Recent work uses the same activity state and keeps its order stable while open.
- Clicking a task reveals its workspace, tab and split, including a pane hidden by zoom.
- Codex/Claude selection stays directly below the request input. Repository context
  scrolls separately so it cannot push the start action out of reach.
- Linked worktrees are distinguished from the main repository's workspace.

An agent must report its state, or expose a recognized running title, for Kiln to
show it accurately. An unfinished agent launch command does not mark an idle agent
as running. Shell-integrated commands can report their own running state. An idle
terminal is not automatically a completed task.

Panel headers provide split-right, split-down and close controls. Very narrow splits
move actions into the panel menu without overlapping their click targets. When
closing a running process, **Don’t ask again** disables process-termination prompts
for panels, tabs and workspaces. It is saved only after confirming the close and
can be re-enabled in Settings → Behavior. Unsaved content and launch-request
warnings remain enabled.

## Useful shortcuts on macOS

| Shortcut | Action |
| --- | --- |
| `⌘K` | Commands and navigation |
| `⌘J` | Recent work |
| `⌘B` | Toggle the workspace sidebar |
| `⌘,` | Settings |
| `Ctrl` + backtick | Quick terminal |

Navigation shortcuts can be customized in Settings. See
[terminal integration](terminal-integration.md) for notifications and activity hooks.
