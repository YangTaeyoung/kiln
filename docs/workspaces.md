# Workspaces and agents

## One workspace, several repositories

Open a parent folder such as `~/dev/projects` when frontend, backend and shared
libraries live in separate Git repositories. Kiln keeps that folder as the agent's
working root and can include the discovered repository layout with your request.
It does not switch the task's working directory merely because you inspect a repository.

Use the repository selector inside Git/GitHub tools to target a specific repository
for Git operations. A workspace is a navigation and agent context boundary, not a
replacement for each repository's Git history.

## Tasks and terminals

- The sidebar previews up to three tasks per workspace, with an option to show all.
- Waiting and running work takes priority over older failures.
- Clicking a task reveals its workspace, tab and split, including a pane hidden by zoom.
- Codex/Claude selection stays directly below the request input. Repository context
  scrolls separately so it cannot push the start action out of reach.
- Linked worktrees are distinguished from the main repository's workspace.

An agent must report its state, or expose a recognized running title, for Kiln to
show it accurately. An idle terminal is not automatically a completed task.

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
