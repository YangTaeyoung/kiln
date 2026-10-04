# Git workflows

Git operations use the repository selected in the Git panel. In a multi-repository
workspace, confirm that selection before changing history.

## Browse and act on history

Open **Changes** and select a repository. Its **Commit history** is interactive by
default: click to select, drag or Shift-click to select a range, and right-click
for commit actions. There is no edit mode. Double-click a commit to open its diff.

Use the branch filter to browse another branch without checking it out. The expand
icon opens the same history interactions in a larger tab. In the multi-repository
overview, the history icon beside each repository also opens that larger view.

For a single-repository workspace, `Shift+Cmd+L` opens history directly. With
multiple repositories it opens the repository overview so you can choose the target.

## Edit a commit message

Right-click a commit in the current branch and choose **Edit message**. Edit the
message in the confirmation dialog and apply it. Rewriting a published commit
requires updating the remote history; review the warning before continuing.

## Squash selected commits

1. Drag across commit rows to select a range. Shift-click also selects a range.
2. Choose **Squash** from the selection actions or context menu.
3. Review the target branch and commits, and edit the combined message.
4. Review any shared-history warning and confirm the operation.

Squashing requires consecutive commits in a supported linear segment of the current
branch. Option/Alt-drag retains the separate commit reordering interaction.

## Cherry-pick from another branch

Choose the source branch in the history filter, select its commits, and choose
**Cherry-pick**. The filter changes what you browse; it does not check out that branch.
The selected changes are applied as new commits to your current branch.

## Drop commits

Select the commits and choose **Drop commits**. This removes their changes from
the current branch by rewriting history. It is different from creating a revert commit.
Review the affected range before confirming.

## Shared history and recovery

For squash, drop, and cherry-pick, Kiln checks the worktree, branch, HEAD and remote
state before execution and stores the original HEAD in a backup ref. Squashing or
dropping published commits offers an automatic push using an explicit
`--force-with-lease` expectation. If the remote changes,
the operation refuses to overwrite it.

The review warns when affected history is part of the remote default branch or
when that cannot be established. Repository branch protection still applies.
Kiln does not bypass server-side protection.

Conflicts stop the operation and expose continue/skip/abort actions. No automatic
push occurs while the operation is stopped. If a later push fails, the UI distinguishes
the completed local rewrite from the unsuccessful remote update. Backup information
remains available in the result details.
