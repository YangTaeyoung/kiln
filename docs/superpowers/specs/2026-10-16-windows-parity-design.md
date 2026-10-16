# Windows parity: Kiln on Windows with the same UI and features as macOS

Date: 2026-10-16. Status: approved in conversation with the owner; implementation and
release proceed without intermediate review.

## Problem

Kiln is validated on macOS only. Windows support from the project's start (ConPTY through
`portable-pty`, named pipes, one PTY host per terminal, daemon copies) was never built
again after later features landed. On 0.1.35:

1. The `kiln` crate and its integration tests did not compile for Windows (fixed in the
   first commit of this branch, with 127 → 1 failing `kiln` unit tests on Windows).
2. Running a cross-compiled `kiln.exe` in a Windows 11 ARM64 guest shows the same UI,
   but:
   - two console windows open with it;
   - workspace paths reach the shell as `\\?\C:\…`;
   - a terminal whose shell exited stays running;
   - a session adopted after a daemon crash does not receive input.
3. Features silently absent on Windows:
   - Foreground process and working-directory detection, which turns off:
     - agent detection
     - account rotation
     - capacity recovery
     - close confirmation
     - tab titles
   - Shell integration: command records, exit codes, working directory, completion.
   - OS notifications and the menu-bar companion.
   - Credential storage: tokens are written to plain files.
   - Remote tools: SFTP and cloud CLIs are never found.
   - Installer and updater.
4. There is no Windows CI, so regressions are invisible.

## Goals

- A Windows user installs Kiln with one command and receives updates in the app, as on
  macOS.
- Every user-visible macOS feature has a Windows behavior: the same feature, or the
  Windows convention for it (system tray for the menu bar, Credential Manager for the
  Keychain, toast notifications, `Ctrl+Shift` for `Cmd`).
- Windows x64 and Windows ARM64 are supported; both are tested natively in CI and ARM64
  is exercised end to end in the owner's Alcove VM.
- macOS behavior and its release process do not change.

## Owner decisions

| Topic | Decision |
|---|---|
| Completion | Official distribution: installer, in-app updates, GitHub release assets, Windows CI |
| Architectures | x64 and ARM64 |
| Code signing | Ship unsigned now; prepare the build for SignPath Foundation's free OSS signing. No paid certificate |
| Agent environment | Native Windows CLIs (`%USERPROFILE%\.claude`, `%USERPROFILE%\.codex`). WSL is a terminal profile only and is not synchronized |
| Release | One public Windows release after the parity checklist passes. macOS releases continue meanwhile |

## Non-goals

- Microsoft Store/MSIX packaging.
- Conversation sync with CLIs installed inside WSL.
- macOS-only concepts without a Windows counterpart: local-network permission, the
  process-responsibility identity, Unix descriptor handover during upgrades.
- Windows 10. Windows 11 is the supported OS. ConPTY and the APIs below exist on
  Windows 10 1809+, but only Windows 11 is tested.

## Design

### 1. Executables and process creation (M1)

- **Two executables from one crate:**
  - `kiln.exe` is a console-subsystem binary for the CLI (`kiln new/send/read/...`) and
    for the internal subcommands: daemon, PTY host and workers.
  - `kiln-app.exe` is a windows-subsystem binary for the GUI. It accepts the GUI's
    optional path argument only. It builds on Windows only; Cargo builds it as an empty
    `main` elsewhere.
  - Both call one library entry point.
  - Windows file names are case-insensitive, so the installed layout is
    `Kiln.exe` (the GUI) and `bin\kiln.exe` (the CLI, on the user `PATH`). This is the
    `Code.exe`/`bin\code` layout of VS Code.
  - The GUI starts the daemon from the CLI executable (`bin\kiln.exe` when installed, the
    sibling `kiln.exe` in a build folder). No console flashes, and a terminal user gets
    blocking CLI output.
  - Only `kiln` is shipped on macOS; nothing changes there.
- **No visible windows from helpers:**
  - Every helper started without a console uses `CREATE_NO_WINDOW`: daemon, PTY host,
    sign-in, Git, cloud CLI, LSP and the agent CLI updater.
  - The WMI fallback for job-bound launches passes a `Win32_ProcessStartup` with
    `ShowWindow = 0`.
- **Process trees:** Each PTY host places its child in a Job Object
  (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is not set, so a crash keeps sessions). Killing a
  session terminates the job, which is the Windows counterpart of the Unix SIGHUP to the
  process group. Helpers that must stop with their tree use the same: agent run, Codex
  metadata reader, cloud CLI calls, sign-in.
- **Resources:** `Kiln.exe` and `kiln.exe` embed the icon (`assets/…/Kiln.ico`), version
  info and an application manifest:
  - per-monitor-v2 DPI awareness
  - UTF-8 active code page
  - long-path awareness
  - Windows 11 `supportedOS`

### 2. Paths and executable lookup (M1)

- **Canonical paths:** `kiln_common::paths::canonicalize` returns
  `std::fs::canonicalize` without the verbatim prefix for drive and UNC paths. All 69
  production call sites use it. This fixes:
  - the `\\?\` prompt;
  - conversation-sync root matching (`safe_path`);
  - path comparisons.
- **Home directory:** `kiln_common::paths::home` is `HOME` on Unix and the Known Folder
  profile on Windows (`directories::BaseDirs`). All `HOME`-only lookups use it:
  - `~` expansion
  - ssh config
  - AWS/OCI/gcloud/wrangler config
  - the launchers
- **Executable lookup:** One `kiln_common::paths::find_executable(name)` follows `PATH`
  × `PATHEXT` order on Windows and skips extension-less files (npm's sh shims). On Unix it
  checks the execute bit. Agent CLIs, cloud CLIs, `sftp`, `ssh`, LSP servers and the
  account sign-in use it.
- **Terminal links:**
  - Clickable paths accept drive-letter and UNC paths.
  - `C:\a\b.rs:12:3` and `C:/a/b.rs:12` split their line and column after the drive
    colon.
  - Windows paths are matched case-insensitively.
- **First run:** The first workspace is the user's home folder when the launch directory
  is a system folder (`C:\Windows\System32`) or `/`.

### 3. Terminal on ConPTY (M2)

- **Exit detection:**
  - A waiter thread waits on the child process handle and then closes the pseudoconsole.
  - The PTY output pipe then reaches EOF after the last output, and the host reports the
    exit code as on Unix.
  - The pseudoconsole is held in an `Option` so closing it does not race resizes.
- **Transport:**
  - The named-pipe attachment gets the Unix behaviors: a handshake deadline, write
    deadlines, and an interrupt that cancels pending I/O (`CancelIoEx`) so a stalled
    attachment can be repaired.
  - The adoption-after-crash input failure is reproduced by
    `hosted::daemon_crash_is_recovered_by_adopting_hosts`, which now runs on Windows,
    and fixed at its cause.
- **Foreground process, working directory and command line:**
  - `procinfo` on Windows walks the console's process tree with Toolhelp32. The
    foreground process is the newest descendant of the shell that is attached to the
    pseudoconsole.
  - It reads the command line and current directory from the target's PEB
    (`NtQueryInformationProcess` + `ReadProcessMemory`, wow64-aware is not needed on
    ARM64/x64 native targets).
  - Process names are compared without `.exe`.
- **Shell integration:** macOS injects only zsh, through a `ZDOTDIR` of wrapper files
  that source the user's files, and the integration script is `shell-integration.zsh`.
  - A new PowerShell shell (pwsh 7 or Windows PowerShell 5.1) without arguments gets the
    same treatment: `-NoExit -Command . '<script>'`. The user's profile still loads,
    and the user's files are never edited. `KILN_SHELL_INTEGRATION=0` turns it off, as on
    macOS.
  - The script wraps the existing `prompt` function and PSReadLine's accept-line. It
    emits what the zsh script emits:
    - `777;kiln-command;<hex>` and `133;C` before a command;
    - `133;D;<exit>` and `133;A` at the prompt;
    - `777;kiln-agent;<state>` from a `kiln-agent-status` function.
  - It adds OSC 7 with `$PWD.ProviderPath`, because PowerShell's location is not its
    process directory.
  - `cmd.exe` gets a `PROMPT` that emits OSC 7 and `133;A`.
  - The OSC 7 parser maps `file://host/C:/…` to `C:\…`.
- **Completion:**
  - Ctrl+Space in PowerShell sends the zsh request (`777;kiln-complete;…`) from
    PSReadLine's buffer state.
  - The accept trigger is a key sequence that ConPTY translates into a key PSReadLine can
    bind; this is verified in the VM before it is chosen.
  - The request file lives in an owner-only folder under `%TEMP%`.
  - The daemon and GUI gates accept `pwsh` and `powershell` next to `zsh`.
  - Typing previews come from key handlers for the same command prefixes as zsh.
- **Default shell:**
  - The default is pwsh, then Windows PowerShell; `cmd.exe` and installed WSL
    distributions are offered as choices.
  - The setting hint shows Windows examples.

### 4. Agents, conversation sync, accounts and remote tools (M3)

- **Agent launch:**
  - Hook commands are quoted for the shell that runs them: PowerShell `& '…'`, cmd
    `"…"`.
  - `kiln agent-run` runs the CLI in a Job Object, forwards Ctrl+C and returns the
    child's exit code.
  - Codex install detection compares path components.
- **Conversation sync:**
  - Roots are `%USERPROFILE%\.claude` and `%USERPROFILE%\.codex`, plus connected
    accounts.
  - Transcript paths are canonicalized on both sides.
  - File identity uses the NTFS file index (done).
  - Folders created for sync get an owner-only ACL.
- **Credentials:** macOS keeps account snapshots in the Keychain through
  `/usr/bin/security`; other systems use files.
  - Account tokens on Windows use Credential Manager through `keyring` 4 (windows-native),
    which `kiln-db` already uses.
  - Existing plain files migrate on first read and are deleted after a verified write.
  - Codex's own Windows credential entry is read in keyring mode. That is service
    `Codex Auth`, account `cli|<first 16 hex of sha256(canonical CODEX_HOME)>`. How Codex
    canonicalizes the path on Windows is checked against Codex's source and a real
    install.
  - Tests use the file store unless `KILN_KEYRING_TESTS=1`, so no OS credential prompt
    appears.
- **Remote:**
  - `sftp`/`ssh` resolve through `find_executable` (Windows ships OpenSSH in
    `System32\OpenSSH`).
  - Cloud CLIs resolve `.exe`/`.cmd`.
  - Their config folders follow each tool's Windows location: AWS and OCI under the home
    folder, gcloud under `%APPDATA%\gcloud`, wrangler under `%APPDATA%\xdg.config\.wrangler`
    or `%USERPROFILE%\.wrangler`. These locations are checked against each tool's
    documentation during implementation.

### 5. OS integration and UI (M4)

- **Notifications:**
  - macOS shows OS notifications through `osascript` for:
    - notify sequences from unobserved sessions;
    - agent Waiting/Done/Failed while the window is unfocused and Do Not Disturb is off;
    - clicking does nothing.
  - Windows shows the same notifications as toasts under Kiln's AppUserModelID, which the
    installer's Start-menu shortcut registers. A build without that shortcut uses
    Windows PowerShell's ID, so toasts still appear.
  - The settings toggle works on Windows.
- **Tray companion:** A notification-area icon mirrors the macOS menu-bar companion.
  - It shows the summary (connecting / idle / N running / N need attention), the
    attention dot, and the unread count from `menu-bar-unread`.
  - Its actions are:
    - Open Kiln;
    - Settings…, through the `open-settings` request file;
    - Stop background sessions…, with a confirmation that defaults to Cancel and that
      kills only sessions whose id, pid and creation time still match.
  - It starts with the GUI and the default daemon, is single-instance through a named
    mutex, and exits after ten failed polls.
  - It runs from the per-build copy under `%LOCALAPPDATA%\Kiln\daemon`, so updates can
    replace the installed files, and restarts itself when a newer build appears.
- **Quit guard:** `WM_QUERYENDSESSION` and window close go through the same draft/save
  handling as the macOS termination guard.
- **Labels:**
  - Shortcut labels render per platform: `Ctrl+Shift+T`, not `⌘T`.
  - "Finder" reads "File Explorer".
  - Settings hints use Windows examples.
- **Fonts:** The Nerd Font search includes `C:\Windows\Fonts` and the per-user fonts
  folder. Korean text uses Malgun Gothic when the bundled fonts lack a glyph (present).
- **IME:** Korean IME composition is verified in the VM. A Windows IME regression test
  covers commit and cancel through winit's `Ime` events, as the macOS test does.
- **Window chrome:** The native Windows title bar stays. Kiln's tab bar keeps its layout
  below it. A custom title bar is not part of parity.

### 6. Distribution (M5)

- **Artifacts per release:**
  - `Kiln-<v>-windows-x64.zip` and `Kiln-<v>-windows-arm64.zip`, each with `Kiln.exe`,
    `bin\kiln.exe` and licenses.
  - These are listed in the release's `SHA256SUMS.txt` next to the macOS archive.
- **Installer:** `install.ps1` (`irm …/install.ps1 | iex`) mirrors `install.sh`:
  - resolves the latest stable release;
  - verifies the checksum;
  - installs per user to `%LOCALAPPDATA%\Programs\Kiln`;
  - adds `kiln` to the user `PATH`;
  - creates a Start-menu shortcut with the AppUserModelID;
  - is idempotent;
  - an existing install is updated from the app.
- **Updater:**
  - The GUI reads `appcast-windows.xml` from the latest release. It has one item per
    architecture, and each enclosure carries the same Sparkle EdDSA signature format.
  - The app verifies the Ed25519 signature over the archive with the Sparkle public key
    from `scripts/release-config.json`, which is embedded at build time. It then replaces
    `Kiln.exe` and `bin\kiln.exe`. The daemon and the tray run from per-build copies, so
    the installed files are not locked.
  - The signatures come from Sparkle's `sign_update` on the maintainer's Mac, so the
    private key stays in the Keychain. The CI-built archives are downloaded, verified and
    signed there.
  - The updater then restarts the GUI.
  - Check for Updates and automatic checks match macOS.
- **Signing:**
  - Release archives are built by a GitHub Actions workflow from the tagged source
    (SignPath requires CI builds).
  - A signing step is left to be enabled once SignPath approves the project.
  - README and the installer explain the SmartScreen prompt for unsigned builds.
- **Release runbook:** `docs/maintainers/releases.md` gains Windows steps:
  - build in CI from the tag;
  - download and verify the checksums and signatures;
  - install twice into an empty folder in the VM;
  - check the update from the previous version in the VM;
  - publish.

### 7. Verification

- **CI:**
  - Jobs on `windows-latest` (x64) and `windows-11-arm` run the workspace tests that do
    not need a database server.
  - Tests that need Unix fixtures (`/bin/sh`, Python scripts, Unix sockets) are either
    ported to PowerShell/Rust fixtures or gated with the reason in the test name's
    module.
  - The macOS job is unchanged.
- **Alcove VM scenarios (ARM64), recorded with screenshots next to macOS captures:**
  - install
  - first run
  - split and close
  - shell exit
  - daemon restart and crash recovery
  - Claude Code and Codex sessions against a loopback model endpoint, with agent
    detection and conversation sync
  - Git panel
  - SFTP to the Ubuntu VM
  - notifications and tray
  - update from the previous build
  - uninstall
- **Parity checklist:** Each row of the feature inventory in
  `docs/windows-parity.md` names the Windows behavior and the test or scenario that
  proves it. The release is cut when every row passes.

## Risks

- **ConPTY behavior:** It differs across Windows builds. The exit and transport changes
  are tested on Windows 11 25H2 (VM) and the CI image.
- **PEB reading:** It depends on undocumented layouts. Failures return `None`, which keeps
  today's behavior.
- **Unsigned binaries:** These trigger SmartScreen until SignPath signs them.
- **Prompt hooks:** Kiln's PowerShell prompt hook must compose with prompt frameworks
  (oh-my-posh, starship). It wraps the existing `prompt` function instead of replacing
  it.
