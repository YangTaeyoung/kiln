<p align="center">
  <img src="assets/Kiln.png" width="104" alt="Kiln app icon" />
</p>

<h1 align="center">Kiln</h1>

<p align="center">A terminal workspace for agents, code, and the work between repositories.</p>

<p align="center">
  <a href="https://github.com/YangTaeyoung/kiln/releases/latest">Download for macOS and Windows</a> ·
  <a href="docs/README.md">Documentation</a> ·
  <a href="https://github.com/YangTaeyoung/kiln/issues">Report an issue</a>
</p>

<p align="center">
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue" /></a>
  <img alt="macOS on Apple Silicon" src="https://img.shields.io/badge/macOS-Apple%20Silicon-24292f" />
  <img alt="Windows x64 and ARM64" src="https://img.shields.io/badge/Windows-x64%20%7C%20ARM64-0078d4" />
  <img alt="Built with Rust and egui" src="https://img.shields.io/badge/built%20with-Rust%20%2B%20egui-b7410e" />
</p>

Kiln brings terminals, AI agents, Git, files, and databases into one native workspace.
Open a repository—or a parent folder containing your frontend, backend, and shared
libraries—and keep the whole task in view.

![Kiln with two terminal tasks and workspace activity](docs/images/workspace.png)
<sub>Rendered by Kiln with illustrative terminal content. No private workspace data.</sub>

### Keep the task together

- **Work across repositories.** Give an agent one request at the parent-folder level,
  while keeping Git operations scoped to the repository you choose. Create separate
  named workspaces for the same folder when tasks need their own terminals and layouts.
- **See what needs attention.** Workspace task rows show reported running, waiting,
  completed, and failed states. Jump directly to the relevant split or session.
- **Keep sessions independent of the window.** A separate local daemon owns your
  terminals, so closing and reopening the GUI does not close their shells.
- **Keep agent conversations through CLI updates.** Track each panel's conversation,
  check CLI versions, and opt into updates with immediate restart in the same account.
- **Keep account usage current.** Refresh saved Codex and Claude Code accounts
  automatically, and sign in again without deleting profiles.
- **Review without leaving the workspace.** Inspect diffs and pull requests,
  drag-select commits to squash, cherry-pick from another branch, or drop a commit.
- **Use the tools around your terminal.** File editing and LSP, project search,
  SQL consoles, database grids, S3/OCI/GCS/R2 and FTP/SFTP files, SSH terminals, command history,
  and a notification center. Sign in to agent accounts through your browser.

### Start in a minute

Install the latest notarized build on macOS with one command:

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | sh
```

On Windows, run this in PowerShell:

```powershell
irm https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.ps1 | iex
```

Open **Kiln** from Applications or the Start menu, choose your project folder, and start
a terminal or a task with Codex or Claude. Prefer a download? Get the app from
[Releases](https://github.com/YangTaeyoung/kiln/releases/latest). Already installed? Use
**Kiln → Check for Updates** on macOS or **Settings → About → Check for Updates** on Windows.

Requires macOS 11+ on Apple Silicon, or Windows 11 on x64 or ARM64.
Windows builds are not code-signed yet; a downloaded archive may show a SmartScreen
prompt. Codex, Claude, and `gh` are external tools; install and authenticate the ones
you use. The app UI supports Korean, English, Japanese and Simplified Chinese.

[Installation and updates →](docs/getting-started.md)

### Explore

[Workspaces & agents](docs/workspaces.md) · [Git workflows](docs/git.md) ·
[File editor](docs/editor.md) · [Database tools](docs/databases.md) ·
[Remote files & SSH](docs/remote-files.md) · [Agent accounts](docs/accounts.md) ·
[CLI updates](docs/cli-updates.md) ·
[Terminal integration](docs/terminal-integration.md) · [Shell completion](docs/shell-completion.md) · [Architecture](docs/architecture.md)

### Build and contribute

```sh
git clone https://github.com/YangTaeyoung/kiln.git
cd kiln
cargo run -p kiln -- /path/to/workspace
```

See [development](docs/development.md) for prerequisites and tests. Maintainers and
AI contributors should start with [AGENTS.md](AGENTS.md), then follow the
[release runbook](docs/maintainers/releases.md).

### License

[MIT](LICENSE) © 2026 Taeyoung Yang. Bundled assets and dependencies retain their
own licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).
