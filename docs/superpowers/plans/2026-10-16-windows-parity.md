# Windows Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kiln runs on Windows x64 and ARM64 with the macOS feature set, installs with one
command, updates itself, and is released from CI.

**Architecture:** One codebase. Windows behavior lives beside the Unix code under
`cfg(windows)` in the module that owns each feature. Shared helpers live in
`kiln_common::paths` (paths, executable lookup) and `kiln_common::process` (hidden
helper processes, job objects). Distribution adds a Windows CI workflow, `install.ps1`
and a Windows appcast signed with the existing Sparkle key.

**Tech Stack:**
- Rust 2024
- egui/eframe 0.36 (wgpu)
- portable-pty 0.9 (ConPTY)
- windows-sys 0.61
- keyring 4
- ring 0.17 (Ed25519)
- embed-resource (build-time resources)
- PowerShell and cmd integration scripts
- GitHub Actions `windows-latest` and `windows-11-arm`

**Spec:** `docs/superpowers/specs/2026-10-16-windows-parity-design.md`

## Global Constraints

- macOS behavior, bundle id `dev.kiln.app`, the Sparkle key and the macOS release process
  stay unchanged.
- Windows 11 x64 and ARM64 are supported; Windows 10 is not tested.
- Unsigned Windows binaries for this release; the CI build stays SignPath-ready.
- Native Windows CLIs (`%USERPROFILE%\.claude`, `%USERPROFILE%\.codex`) are the agent
  environment; WSL is a terminal profile only.
- Tests never touch the user's real credential stores or daemons: isolated
  `KILN_CONFIG_DIR`, `KILN_SOCKET`, file credential store unless `KILN_KEYRING_TESTS=1`.
- Code comments describe current behavior only (no history, measurements or review notes).
- Public docs are English; README stays short; product copy is Korean source strings with
  the existing i18n tables.
- No new runtime dependency where an OS tool or an existing crate does the job:
  `curl.exe`/`tar.exe` ship with Windows 11; Ed25519 comes from `ring`.

## Review Focus

1. A workspace path with spaces, Korean characters or on a UNC share
   (`\\server\share\proj`) reaches the shell, Git and the conversation-sync roots
   unchanged and without a `\\?\` prefix.
   Owner: Task 1, `canonical_paths_keep_unc_and_unicode_names`.
2. A shell that exits by itself, a shell killed by `Stop`, and a shell whose children
   keep running all end the pane with the correct exit code and leave no processes
   behind.
   Owner: Task 5, `hosted_exit_code_is_reported` and
   `killed_session_leaves_no_descendants`.
3. PowerShell with a custom `prompt` (oh-my-posh style) keeps its prompt text while Kiln
   adds its marks.
   Owner: Task 7, `powershell_integration_wraps_an_existing_prompt`.
4. An update archive whose signature, length or embedded version does not match is
   refused without touching the installed files.
   Owner: Task 14, `tampered_archive_is_refused_before_replacing_files`.
5. Running the installer twice, or while Kiln is running, leaves one working install and
   does not delete user data.
   Owner: Task 15, the VM installer scenario in `docs/windows-parity.md`.

## Execution notes

- **Loop:** Windows RED→GREEN runs in the Alcove VM through the scratchpad runner
  `wt.sh <cargo test selection> -- <filters>`. It cross-builds with `cargo xwin`,
  copies the binaries, runs them with `--test-threads 1` and prints the failures. The
  final judgement is native CI (Task 16).
- **macOS checks:** `cargo test` for the touched crates, run with the `KILN_*`
  variables unset. Use `KILN_PTY_HOST=0` for daemon unit tests.
- **Commits:** One commit per task, ending with the Co-Authored-By line from the session.

---

### Task 1: Shared paths, home folder and executable lookup

**Files:**
- Modify: `crates/kiln-common/src/paths.rs` and `crates/kiln-common/Cargo.toml`. This
  adds `canonicalize`, `simplified`, the `Canonical` trait, `home` and `find_executable`.
- Modify: every `.canonicalize()` and `fs::canonicalize` call in kiln, kiln-git,
  kiln-editor, kiln-remote and kiln-accounts. The daemon's comparison-only calls stay.
- Modify the lookups to use `kiln_common::paths::find_executable`:
  - `crates/kiln/src/app/agent_launch.rs:11` (`executable`)
  - `crates/kiln-editor/src/lsp/config.rs:189` (`find_executable`)
  - `crates/kiln-accounts/src/login.rs:471` (`find_cli`)
  - `crates/kiln/src/app/projects.rs:72` (`executable_in`)
  - `crates/kiln-remote/src/cloud_cli.rs:11-44`
  - `crates/kiln-remote/src/sftp.rs:32`
  - `crates/kiln-daemon/src/pty.rs:28` (`which_exists`). The daemon gains no kiln-common
    dependency; it keeps its own PATHEXT loop.
- Modify `HOME`-only lookups to use `kiln_common::paths::home()`:
  - `crates/kiln-remote/src/ssh_config.rs:80-82,158-162`
  - `aws_profiles.rs:32-35`
  - `object_profiles.rs:63-84,108-125`
  - `crates/kiln/src/app/mod.rs:2333`
  - `projects.rs:741`
  - `launchers.rs:38`
  - `workspace_dialog.rs:20`
- Modify: `crates/kiln/src/app/mod.rs:453-455`. When the launch folder is `/` or under
  `%SystemRoot%`, the first workspace is the home folder.

**Interfaces (Produces):**
- `kiln_common::paths::canonicalize(impl AsRef<Path>) -> io::Result<PathBuf>`
- `kiln_common::paths::simplified(&Path) -> &Path`
- `trait kiln_common::paths::Canonical { fn canonical(&self) -> io::Result<PathBuf> }`
  for `Path`
- `kiln_common::paths::home() -> Option<PathBuf>`
- `kiln_common::paths::find_executable(name: &str, extra: &[PathBuf]) -> Option<PathBuf>`

**Steps:**
- [ ] Write tests in `paths.rs`:
  - `canonical_paths_have_no_verbatim_prefix`
  - `canonical_paths_keep_unc_and_unicode_names`, which creates `한글 dir` and asserts
    the name survives; on Windows it also checks that
    `simplified(Path::new(r"\\?\UNC\srv\share\x")) == Path::new(r"\\srv\share\x")`
  - `home_is_an_existing_folder`
  - `executable_lookup_checks_explicit_paths_directly`
  - `executable_lookup_requires_the_execute_bit` (Unix)
  - `executable_lookup_follows_pathext_order_and_skips_extensionless_shims` (Windows)
- [ ] Run `cargo test -p kiln-common --lib paths`. Expected: does not compile (functions
  missing).
- [ ] Implement them with `dunce` and `directories::BaseDirs`.
- [ ] Run the tests on macOS, then `wt.sh -p kiln-common --lib -- paths` on Windows.
  Expected: all pass.
- [ ] Replace the call sites. Expected: `cargo check --workspace --all-targets` is clean
  on macOS and with `cargo xwin` for aarch64-pc-windows-msvc.
- [ ] Each converted lookup keeps its existing tests. Add
  `agent_cli_lookup_prefers_cmd_shim_over_extensionless_file` (Windows) to
  `agent_launch.rs`; it builds a temp PATH with `codex` and `codex.cmd`.
- [ ] Add `first_workspace_avoids_system_folders` to `mod.rs` tests. It calls the
  extracted `fn first_workspace(cwd: &Path, home: &Path) -> PathBuf`.
- [ ] Commit: `fix(windows): shared canonical paths, home folder and PATHEXT lookup`.

### Task 2: GUI and CLI executables, resources, hidden helpers

**Files:**
- Create: `crates/kiln/src/cli.rs`, which holds the body of `main.rs` as
  `pub fn main() -> anyhow::Result<()>`.
- Modify: `crates/kiln/src/main.rs` to a two-line `fn main` calling `kiln::cli::main()`.
- Create: `crates/kiln/src/bin/kiln-app.rs`, with
  `#![cfg_attr(windows, windows_subsystem = "windows")]` and a `main` that calls
  `kiln::cli::gui_main()`. On non-Windows it prints that the GUI binary is Windows-only
  and exits 2.
- Create: `crates/kiln/build.rs` and `crates/kiln/windows/kiln.rc` (icon `assets/Kiln.ico`,
  VERSIONINFO from `CARGO_PKG_VERSION`) and `crates/kiln/windows/kiln.manifest`:
  - per-monitor-v2 DPI
  - `activeCodePage` UTF-8
  - `longPathAware`
  - Windows 10/11 `supportedOS`
  - `asInvoker`

  The `.rc` is compiled only for `target_os = "windows"`, with `embed-resource`.
- Create: `crates/kiln-common/src/process.rs` with
  `pub fn hide_window(cmd: &mut std::process::Command) -> &mut Command`. It applies
  `CREATE_NO_WINDOW` on Windows and is a no-op elsewhere.
- Modify these Windows spawns to use it:
  - `crates/kiln-daemon/src/client.rs:285-307`: the WMI call gets
    `ProcessStartupInformation` with `ShowWindow=0` and `CreateFlags=0x08000000`
  - Git commands in `crates/kiln-git/src/cmd.rs:71-80`, which already hide
  - LSP spawns in `crates/kiln-editor/src/lsp/proto.rs`
  - `crates/kiln-remote/src/cloud_cli.rs`
  - `aws_profiles.rs`
  - `sftp.rs`
  - `crates/kiln-accounts/src/login.rs`
  - the agent CLI updater in `crates/kiln/src/app/agent_runtime.rs:540`
  - the Codex metadata reader in `crates/kiln/src/app/conversation_sync/codex_index.rs`
- Modify: `crates/kiln-daemon/src/client.rs`: add
  `pub fn cli_executable(current: &Path) -> PathBuf`. It is the sibling `kiln.exe` in a
  build folder and `bin\kiln.exe` under an installed `Kiln.exe`; the current exe
  elsewhere. `spawn_daemon` and the PTY-host spawn use it.

**Interfaces (Produces):**
- `kiln::cli::main() -> anyhow::Result<()>`
- `kiln::cli::gui_main() -> anyhow::Result<()>`
- `kiln_common::process::hide_window`
- `kiln_daemon::client::cli_executable`

**Steps:**
- [ ] Write the tests:
  - `cli_executable_finds_the_console_binary_beside_or_under_the_gui`
    (`kiln-daemon/src/client.rs`, cfg(windows)), with a temp layout
    `Kiln.exe` + `bin\kiln.exe` and `kiln-app.exe` + `kiln.exe`
  - `hidden_helpers_do_not_inherit_a_console` (Windows, `kiln-common`): spawn
    `cmd /c exit 0` with `hide_window` and assert success; the flag value is asserted
    through `CommandExt` being applied (exit status)
- [ ] RED on Windows, implement, GREEN on Windows.
- [ ] Build `cargo xwin build --release -p kiln --bins`. Expected: `kiln.exe` and
  `kiln-app.exe`.
- [ ] VM scenario: deploy both as `Kiln.exe` + `bin\kiln.exe`, start `Kiln.exe` from
  Explorer through `alcove_launch_app`. Expected: `alcove_list_windows` shows only the
  Kiln window (no console windows); `bin\kiln.exe ls` prints the sessions in a console.
- [ ] Commit: `feat(windows): separate GUI and CLI executables with resources and hidden helpers`.

### Task 3: Windows CI and portable tests

**Files:**
- Create: `.github/workflows/windows.yml`, with jobs `windows-x64` (`windows-latest`)
  and `windows-arm64` (`windows-11-arm`). Each job:
  - installs Rust stable;
  - runs `cargo check --locked -p kiln --bins`;
  - runs the unit and integration tests that run on Windows, with
    `--test-threads=1` for GUI and daemon tests;
  - uploads failing test logs.
- Modify the Unix-bound tests:
  - `crates/kiln/tests/daemon.rs`: socket per OS (`kiln-test-<pid>-<tag>` pipe name on
    Windows), shell per OS (PowerShell with the `sh` module pattern from `hosted.rs`), and
    capture paths under `temp_dir`
  - `crates/kiln/tests/lifecycle_reliability.rs`, `notification_visibility.rs`,
    `recovery_navigation.rs` and `remote_inspector.rs`: socket names as above
  - `crates/kiln-editor/tests/lsp_fake.rs`: build the fake LSP server as a Rust test
    binary (`[[bin]] fake-lsp` behind a dev-only feature, or `tests/support` compiled with
    `CARGO_BIN_EXE`), instead of a shell/Python script
  - `crates/kiln-editor/src/lsp/config.rs` `finds_executables_on_path`: look for `cmd`
    on Windows
  - `crates/kiln-daemon/src/shell_completion.rs` `malformed_request_is_not_a_command`:
    use an absolute cwd valid on the OS
  - `crates/kiln-remote/src/object_oci.rs:348`: compare with the OS path form
- Gate (`#[cfg(unix)]` with a module doc line naming the fixture) only the tests whose
  subject is Unix-only:
  - zsh completion
  - Unix descriptor handover
  - the macOS responsibility tests

**Steps:**
- [ ] Push the branch and run the workflow (`gh workflow run windows.yml --ref
  feat/windows-parity`). Expected: the first run lists the failing tests.
- [ ] Fix each failure as a test-portability change, or record it as a product defect
  in the owning task below. Test fixes need no RED on macOS. They are verified by the
  Windows job.
- [ ] Expected end state: both Windows jobs are green except tests owned by later tasks,
  which are listed in the job as `--skip` with the task number in a comment until that
  task lands.
- [ ] Commit: `ci: Windows x64 and ARM64 test jobs`.

### Task 4: Session exit and process trees on ConPTY

**Files:**
- Modify: `crates/kiln-daemon/src/pty.rs:308-400`:
  - The `master` field becomes `Mutex<Option<Box<dyn MasterPty + Send>>>`.
  - `spawn` creates a Job Object with `JOB_OBJECT_LIMIT_BREAKAWAY_OK` off, assigns the
    child (`OpenProcess(PROCESS_SET_QUOTA|PROCESS_TERMINATE)`), and starts a waiter
    thread. The waiter does `WaitForSingleObject(child)` and then
    `master.lock().take()` (drop closes the pseudoconsole).
  - `kill()` calls `TerminateJobObject(job, 1)` and then kills the child.
  - `resize` on a closed pseudoconsole is a no-op.
- Modify: `crates/kiln-daemon/Cargo.toml` to add the windows-sys features
  `Win32_System_JobObjects` and `Win32_System_Threading` (present).
- Test: `crates/kiln/tests/hosted.rs`:
  - `hosted_exit_code_is_reported` (exists; fails on Windows today)
  - new `killed_session_leaves_no_descendants`: start a shell that starts a child
    (`Start-Process -NoNewWindow powershell -ArgumentList '-Command','Start-Sleep 60'`
    or `sh -c 'sleep 60 & wait'`), print its pid, Kill the session, and assert the child
    pid is gone within 5 s
- Test: `crates/kiln-daemon/src/pty.rs` unit `windows_child_exit_closes_output`. Spawn
  `cmd /c echo done`, read until EOF within 5 s, and check that `try_wait` returns
  `Some(0)`.

**Steps:**
- [ ] Run `wt.sh -p kiln --test hosted -- hosted_exit_code_is_reported`. Expected: FAIL
  (timeout).
- [ ] Write `killed_session_leaves_no_descendants` and the pty unit test. Run them on
  Windows. Expected: FAIL.
- [ ] Implement the waiter, the Option master and the job.
- [ ] Run both test files on Windows and macOS. Expected: PASS.
- [ ] Commit: `fix(windows): report shell exit and stop whole process trees on ConPTY`.

### Task 5: Named-pipe attachment recovery

**Files:**
- Modify: `crates/kiln-daemon/src/transport.rs` (Windows `imp`):
  - `connect_with_shutdown` returns a `Shutdown` that calls `CancelIoEx` on the pipe
    handle (taken from the stream with `AsRawHandle`).
  - The handshake read gets a deadline through a watcher thread that cancels I/O after
    1 s.
- Modify: `crates/kiln-daemon/src/ptyhost.rs:304-330`. The Windows attach path uses
  that deadline. `interrupt_transport` works on Windows (its comment changes to describe
  the current behavior).
- Investigate and fix: `hosted::daemon_crash_is_recovered_by_adopting_hosts`. On
  Windows, input after adoption does not reach the shell. Instrument the host's accept
  loop (log lines go to the daemon log) and reproduce in the VM. The fix goes where the
  cause is.
  - The candidates are the host keeping the dead connection's writer, and the input
    thread blocked on the old pipe.
  - Record the cause in the commit message.

**Steps:**
- [ ] Run `wt.sh -p kiln --test hosted --`. Expected: `daemon_crash…` FAILS.
- [ ] Add `attach_to_an_unresponsive_host_times_out` to the `ptyhost.rs` tests (Windows):
  a pipe server that accepts and never answers. Expected: FAIL (hangs; bounded by the
  test harness at 10 s).
- [ ] Fix, then run hosted tests and ptyhost unit tests on Windows and macOS. Expected:
  PASS.
- [ ] Commit: `fix(windows): bounded named-pipe attachment and input after crash recovery`.

### Task 6: Foreground process, command line and working directory

**Files:**
- Modify: `crates/kiln-daemon/src/procinfo.rs` (Windows functions at `154-204`):
  - `name(pid)`: `QueryFullProcessImageNameW`, file stem.
  - `cmdline(pid)` and `cwd(pid)`: `NtQueryInformationProcess(ProcessBasicInformation)`
    gives the PEB address. Then `ReadProcessMemory` reads `ProcessParameters`
    (`RTL_USER_PROCESS_PARAMETERS`: `CurrentDirectory.DosPath` at the documented
    offsets for 64-bit processes). Then `CommandLineToArgvW`.
  - `foreground_pid(shell_pid)`: a Toolhelp32 snapshot builds the descendant tree of the
    shell. It picks the newest live descendant that is not `conhost`/`OpenConsole`, and
    prefers a single `codex`/`claude` descendant as macOS does.
- Modify: `crates/kiln-daemon/src/pty.rs` Windows `fg_pid` returns
  `procinfo::foreground_pid(self.pid)`.
- Modify: `crates/kiln/src/app/mod.rs:323-325`, the shell-name list, and any name
  comparison, to strip `.exe` and compare ASCII case-insensitively on Windows.

**Steps:**
- [ ] Port the Unix tests at `procinfo.rs:209-282` to cover Windows:
  `cmdline_of_self`, `cwd_of_self_matches_current_dir` and
  `foreground_prefers_the_agent_child` (spawn `cmd /c powershell -Command Start-Sleep
  30`, assert the foreground is `powershell`). Run them on Windows. Expected: FAIL
  (`None`).
- [ ] Implement.
- [ ] Run on Windows and macOS. Expected: PASS.
- [ ] VM scenario: a Kiln tab running `ping -t 127.0.0.1` shows `ping` in the sidebar,
  and closing it asks for confirmation.
- [ ] Commit: `feat(windows): foreground process, command line and directory of terminal sessions`.

### Task 7: PowerShell and cmd shell integration

**Files:**
- Create: `crates/kiln-daemon/src/shell-integration.ps1`, embedded with
  `include_str!`. It defines:
  - `kiln-agent-status`;
  - a `prompt` wrapper: it saves the existing `function:prompt` and emits
    `133;D;<exit>` when a command ran, then OSC 7 with `$PWD.ProviderPath` as a
    `file://` URL and `133;A`, then returns the original prompt text;
  - a PSReadLine `AcceptLine` key handler that emits `777;kiln-command;<hex>` and
    `133;C` before accepting;
  - exit codes from `$?`/`$LASTEXITCODE`.
- Modify: `crates/kiln-daemon/src/shell.rs`:
  - `configure_powershell(program, args, env) -> Option<Vec<String>>` writes the script
    to the integration folder (the same atomic write as zsh) and returns the args
    `-NoExit -Command ". '<script>'"` when the program stem is `pwsh` or `powershell`
    and args are empty.
  - `configure_cmd` sets `PROMPT=$e]7;file://localhost/$P$e\$e]133;A$e\$P$G`.
  - `KILN_SHELL_INTEGRATION=0` disables both.
- Modify: `crates/kiln-daemon/src/pty.rs` Windows `spawn` calls them.
- Modify: `crates/kiln-daemon/src/osc.rs:148-152`. `parse_file_url` maps
  `/C:/x` to `C:\x` on Windows (and keeps Unix paths).

**Steps:**
- [ ] Unit tests in `osc.rs`: `file_url_with_a_drive_letter_is_a_windows_path`
  (`file://h/C:/Users/a%20b` → `C:\Users\a b`, cfg(windows)) and the existing Unix
  case.
- [ ] Integration test in `crates/kiln/tests/hosted.rs` (Windows):
  - `powershell_integration_reports_commands_exit_codes_and_location`: create a default
    shell, type `cd $env:TEMP; cmd /c exit 3`, wait for telemetry with a command record
    `cmd /c exit 3` and exit code 3, and a session cwd equal to the temp folder.
  - `powershell_integration_wraps_an_existing_prompt`: spawn env sets
    `KILN_TEST_PROFILE` → a profile that defines `function prompt { "CUSTOM> " }` (passed
    with `-NoProfile` replaced by `. $env:KILN_TEST_PROFILE` inside the script is not
    allowed; instead write a temp profile and set `$PROFILE` through `-Command`). Assert
    that the screen shows `CUSTOM> ` and the telemetry has prompt marks.
- [ ] Run them on Windows. Expected: FAIL.
- [ ] Implement and run them on Windows. Expected: PASS. Run the zsh tests on macOS.
  Expected: PASS (unchanged).
- [ ] Commit: `feat(windows): PowerShell and cmd shell integration`.

### Task 8: PowerShell completion

**Files:**
- Modify: `crates/kiln-daemon/src/shell-integration.ps1` to add the Ctrl+Space
  PSReadLine handler. It builds the `777;kiln-complete;<rev>;<cursor>;<hex
  buffer>;<hex cwd>;<hex request file>;<hex commands>;1` request from
  `[Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState`.
- Modify the accept handler. It binds the key chosen in Step 1, reads and deletes the
  request file, checks the revision, buffer and cursor, then `Replace`s the buffer and
  sets the cursor.
- Modify: `crates/kiln-daemon/src/shell_completion.rs:109-115`. The Windows `stage()`
  creates `%TEMP%\kiln-completion-<user>\<rev>`. The folder's DACL grants only the
  current user SID (reuse `transport::current_user_sid`). `create_new` makes the file.
- Modify the gates to accept `pwsh`/`powershell` (case-insensitive, without `.exe`):
  - `crates/kiln-daemon/src/server.rs:593,759`
  - `crates/kiln/src/app/terminal.rs:455`
  - `crates/kiln/src/app/conn.rs:392-411`

**Steps:**
- [ ] Step 1, probe in the VM. In a Kiln PowerShell session, run
  `Set-PSReadLineKeyHandler -Chord F24 -ScriptBlock { "F24" | Out-File $env:TEMP\k.txt }`
  and inject candidate sequences (`ESC[99;1~`, `ESC[24~` with modifiers) with
  `kiln send`. Pick the first chord PSReadLine receives, and record the choice in the
  commit message.
- [ ] Integration test in `crates/kiln/tests/shell_completion.rs` (enable on Windows):
  `powershell_completion_request_and_accept_replace_the_buffer`.
- [ ] RED on Windows, implement, GREEN on Windows. macOS zsh completion tests: PASS.
- [ ] Commit: `feat(windows): shell completion for PowerShell`.

### Task 9: Agent launch, hooks and conversation sync on Windows

**Files:**
- Modify: `crates/kiln/src/app/agent_runtime.rs:28-30,136-137`. Hook commands are
  quoted for the hook runner's shell. Claude Code on Windows runs hooks through Git Bash
  when present, else PowerShell; this is checked against the installed CLI in the VM
  before quoting is chosen.
- Modify: `agent_runtime.rs:361-371`. `agent-run` on Windows starts the CLI in a Job
  Object, forwards Ctrl+C (`GenerateConsoleCtrlEvent` to the child group) and exits
  with the child's code.
- Modify: `agent_runtime.rs:385,402`. Install detection compares `Path::components`.
- Modify: `crates/kiln/src/app/conversation_sync/mod.rs:217-226,287-305`:
  - Roots and transcript paths use `kiln_common::paths::canonicalize`.
  - `private_dir` on Windows sets an owner-only DACL.
- Test: `crates/kiln/src/app/conversation_sync/tests.rs`
  `hook_transcript_paths_match_canonical_roots`. A transcript path given in the
  hook's raw form (`C:\…`, mixed case) is tracked.
- VM scenario: install Claude Code and Codex in the VM and point them at the loopback
  model server on the Mac (port forward). Run a Claude session and a Codex session in
  Kiln. Expected:
  - the sidebar shows the agent identity;
  - the conversation appears in the other CLI's resume list;
  - the handover lease is released after the pane closes.

**Steps:**
- [ ] RED (tests above) on Windows, implement, GREEN on Windows and macOS.
- [ ] Run the VM scenario and capture screenshots for `docs/windows-parity.md`.
- [ ] Commit: `fix(windows): agent hooks, agent-run and conversation sync paths`.

### Task 10: Credentials in Windows Credential Manager

**Files:**
- Modify: `crates/kiln-accounts/src/store.rs`. Add `KeyringStore` (Windows) on
  `keyring::Entry` (keyring 4, `windows-native`).
- Modify: `crates/kiln-accounts/src/manager.rs:36-61`:
  - On Windows the default store is `KeyringStore`.
  - On first read it migrates an existing `FileStore` entry: read, write to the
    keyring, read back, delete the file.
  - Tests get `FileStore` unless `KILN_KEYRING_TESTS=1`.
- Modify: `crates/kiln-accounts/src/codex.rs:10-46`. In keyring/auto mode on Windows,
  read `Codex Auth` / `cli|<hash>`, using the canonical form Codex uses (verified in the
  VM against Codex's own entry).
- Modify: `crates/kiln-accounts/Cargo.toml`. Add
  `[target.'cfg(windows)'.dependencies] keyring = "4"`.

**Steps:**
- [ ] Tests (Windows):
  - `file_credentials_migrate_to_the_keyring_once`: uses a test keyring service name
    `kiln-accounts-test-<pid>`, runs only with `KILN_KEYRING_TESTS=1`, and deletes its
    entries at the end;
  - `tests_use_the_file_store_by_default`.
- [ ] Run them on Windows with `KILN_KEYRING_TESTS=1` in the VM. Expected: FAIL. Then
  implement, then PASS.
- [ ] VM scenario: sign in to nothing real. Use a fixture Codex `config.toml` with
  `cli_auth_credentials_store = "keyring"` and an entry written by a test helper.
  Expected: the account row reads it.
- [ ] Commit: `feat(windows): account credentials in Credential Manager`.

### Task 11: Remote and cloud tools on Windows

**Files:**
- Modify: `crates/kiln-remote/src/sftp.rs:32`: use
  `find_executable("sftp", &[System32\OpenSSH])`.
- Modify: `crates/kiln-remote/src/cloud_cli.rs:11-44`: use `find_executable`.
- Modify these config locations (`directories::BaseDirs` + documented paths):
  - `aws_profiles.rs:32-35`: `%USERPROFILE%\.aws`
  - `object_profiles.rs:63-84`: OCI at `%USERPROFILE%\.oci`, gcloud at
    `%APPDATA%\gcloud`
  - `object_profiles.rs:108-125`: wrangler, at the location from the wrangler docs
    (verify before coding)
- Modify: `crates/kiln-remote/src/object_oci.rs:348` test: OS path form.
- Modify: the CLI calls that are cancelled. They use a Job Object helper
  `kiln_common::process::Tree` (create job, assign, `TerminateJobObject` on drop/kill).

**Steps:**
- [ ] Tests:
  - `cloud_cli_lookup_finds_cmd_shims` (Windows)
  - `gcloud_config_dir_follows_appdata` (Windows)
  - `cancelled_listing_stops_the_cli_tree` (Windows, with a `.cmd` that starts
    `powershell Start-Sleep`)
- [ ] RED on Windows, implement, GREEN on Windows and macOS.
- [ ] VM scenario: SFTP from the Windows VM to the Ubuntu VM (`ssh 2223` forwarded on the
  host, reachable as `10.0.2.2:2223` from the guest). Browse a folder and open a file.
- [ ] Commit: `fix(windows): remote and cloud tools on Windows`.

### Task 12: Notifications, quit guard, labels, fonts, IME

**Files:**
- Modify: `crates/kiln/src/app/mod.rs:2258-2277`. On Windows, `os_notify` runs hidden
  `powershell.exe -NoProfile -NonInteractive -Command <script>`. The script loads
  `Windows.UI.Notifications`, builds a `ToastText02` template with escaped
  title/body, and calls `ToastNotificationManager::CreateToastNotifier(<AUMID>)`.
  - The AUMID is `dev.kiln.app` when the Start-menu shortcut carries it; otherwise the
    Windows PowerShell AUMID.
  - Title and body are passed as base64 UTF-16 arguments, never interpolated into the
    script.
- Modify the shortcut labels to come from one function
  `kiln_common::widgets::shortcut_label(&KeyboardShortcut) -> String` (macOS glyphs;
  `Ctrl+Shift+T` elsewhere):
  - `crates/kiln/src/app/tools.rs:66-76`
  - `settings.rs:293-297`
- Modify: `crates/kiln/src/app/ui.rs:846`. The "Finder" label is per OS ("파일 탐색기에서
  보기" source string with translations).
- Modify: `crates/kiln/src/app/settings.rs:273-274`. The default-shell hint is per OS.
- Modify: `crates/kiln-common/src/fonts.rs:56-63`. The Nerd Font search adds
  `%WINDIR%\Fonts` and `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
- Modify: `crates/kiln/src/app/mod.rs:2026-2029`. The quit guard on Windows handles the
  viewport close request (`ViewportCommand::CancelClose`) the same way as the macOS
  termination guard. It also handles `WM_QUERYENDSESSION` through eframe's close event,
  which winit maps.
- Test: `crates/kiln/tests/native_ime.rs`. It is enabled on Windows, with the winit IME
  event path (`Ime::Preedit`/`Ime::Commit`) for a Korean syllable sequence.

**Steps:**
- [ ] Unit tests:
  - `shortcut_labels_follow_the_platform`
  - `toast_arguments_are_encoded_not_interpolated`, on the script-building function
    `toast_command(title, body) -> Vec<String>`
  - `nerd_font_search_includes_windows_font_folders` (Windows)
- [ ] Windows RED → GREEN, then macOS PASS.
- [ ] VM scenarios:
  - a notify sequence from an unobserved session with the window unfocused shows a toast
    (screenshot);
  - Korean IME composition in a terminal and in the editor (screenshot);
  - closing the window with an unsaved editor asks first.
- [ ] Commit: `feat(windows): toasts, quit guard, platform labels, fonts and IME coverage`.

### Task 13: Tray companion

**Files:**
- Create: `crates/kiln/src/tray_windows.rs` (`cfg(windows)`), modeled on
  `status_bar.rs`:
  - a message-only window and `Shell_NotifyIconW` with the app icon from the exe
    resources, plus an attention icon (red dot drawn into an icon at runtime with
    `CreateIconIndirect`);
  - a `TrackPopupMenu` menu: summary, Open Kiln, Settings…, Stop background sessions…;
  - the confirmation is a `MessageBoxW` with `MB_DEFBUTTON2` and Cancel as the default;
  - polling `ListSessions` every 2 s on a thread, exiting after 10 failures;
  - unread from `menu-bar-unread`;
  - a single instance through `CreateMutexW("Local\\kiln-tray-<socket>")`;
  - a restart when the per-build copy changes.
- Modify: `crates/kiln/src/cli.rs`. The `status-bar` subcommand exists on Windows. The
  GUI and the default daemon call `tray::ensure_running()`, which spawns
  `kiln.exe status-bar` from the per-build copy (`kiln_daemon::client::daemon_copy`),
  hidden.
- Modify: `crates/kiln/src/app/mod.rs:2344-2365`. The settings-request consumer and the
  unread publisher run on Windows too.
- Move pure logic shared with macOS into `crates/kiln/src/companion.rs`: summary text,
  the stop-session match rule, and the unread file. `status_bar.rs` uses it as well.

**Steps:**
- [ ] Unit tests in `companion.rs`:
  - `summary_text_counts_running_and_attention`
  - `stop_only_matches_sessions_with_the_same_identity`
  - `unread_file_round_trips`

  These move from `status_bar.rs` tests if present.
- [ ] macOS PASS (behavior unchanged), Windows PASS.
- [ ] VM scenario: the tray icon appears, its menu opens, Settings… opens settings in a
  running GUI, Stop asks and stops, and the attention dot shows for a notify sequence
  (screenshots).
- [ ] Commit: `feat(windows): notification-area companion`.

### Task 14: In-app updater for Windows

**Files:**
- Create: `crates/kiln/src/app/updater_windows.rs` (`cfg(windows)`):
  - `fn feed_url() -> &'static str` is
    `https://github.com/YangTaeyoung/kiln/releases/latest/download/appcast-windows.xml`.
  - `fn parse_appcast(xml: &str, arch: &str) -> Result<Release>` returns version, url,
    length and the signature for `sparkle:os="windows"` with `kiln:arch`.
  - `fn verify(archive: &[u8], release: &Release, public_key: &[u8; 32]) -> Result<()>`:
    length, then Ed25519 with `ring::signature::ED25519`.
  - `fn install(archive: &Path, install_dir: &Path) -> Result<()>`:
    - extracts with `%SystemRoot%\System32\tar.exe -xf` into `install_dir\.update-<v>`;
    - checks that `Kiln.exe` and `bin\kiln.exe` exist and that `bin\kiln.exe --version`
      matches;
    - renames the current files to `*.old` (allowed for running images) and moves the
      new ones in;
    - deletes `*.old` on the next start.
  - The download runs `curl.exe -fL --retry 3` to a temp file, hidden.
  - The automatic check runs once a day when the setting is on (default on, as
    Sparkle's `SUEnableAutomaticChecks=true`), never during tests (`KILN_CONFIG_DIR` set),
    and only from an installed layout (`Kiln.exe` with `bin\kiln.exe`).
- Create: `crates/kiln/build.rs` (from Task 2). It also reads
  `scripts/release-config.json` and exports `KILN_UPDATE_PUBLIC_KEY` (base64) through
  `cargo:rustc-env`.
- Modify: `crates/kiln/src/app/settings.rs:333-336`. The Windows update section has
  the same checkbox and button text as macOS, plus an "update available" row with
  Install and Restart.
- Modify: `scripts/make-appcast.py`. Add `--windows` mode that writes one item with two
  enclosures (x64, arm64), each with `sparkle:edSignature` and `length`.

**Steps:**
- [ ] Unit tests in `updater_windows.rs` (all platforms; the module's pure parts are
  `cfg(any(windows, test))`):
  - `appcast_selects_the_matching_architecture`
  - `tampered_archive_is_refused_before_replacing_files` (flip one byte → `verify` Err,
    and `install` is never called; signature from a test keypair)
  - `wrong_length_is_refused`
  - `install_replaces_both_executables_and_keeps_old_copies_until_restart`
    (Windows, temp install dir with fake exes; `--version` checked through a stub
    `bin\kiln.exe` built from the test binary itself)
  - `make-appcast --windows` output is validated by `scripts/test-make-appcast.py`
- [ ] RED, implement, GREEN (macOS for the pure parts, Windows for install).
- [ ] VM scenario (in Task 15): install the previous build, publish a local appcast
  through a port-forwarded loopback server with a test key (`KILN_UPDATE_FEED` and
  `KILN_UPDATE_PUBLIC_KEY_TEST`, available only in `updater-test` builds), check for
  updates, install and restart.
- [ ] Commit: `feat(windows): in-app updates signed with the Sparkle key`.

### Task 15: Installer, packaging, release workflow, docs

**Files:**
- Create: `install.ps1`. It mirrors `install.sh`:
  - accepts x64/ARM64 only;
  - resolves the latest tag through the redirect;
  - downloads the zip and `SHA256SUMS.txt` and verifies them;
  - validates entry names (no absolute paths or `..`);
  - extracts to `%LOCALAPPDATA%\Programs\Kiln` (or `KILN_INSTALL_DIR`);
  - creates a Start-menu shortcut with AUMID `dev.kiln.app` (`IPropertyStore` through a
    small embedded C# `Add-Type`), and adds `bin` to the user PATH;
  - exits with the update hint if already installed.
- Create: `scripts/package-windows.ps1`, which builds both arches in CI
  (`cargo build --locked --release -p kiln --bins --target <triple>`). It lays out
  `Kiln.exe` (from `kiln-app.exe`), `bin\kiln.exe`, `LICENSE` and
  `THIRD_PARTY_NOTICES.md`, zips them and prints the SHA-256.
- Create: `.github/workflows/release-windows.yml`. It runs on `workflow_dispatch` with a
  tag input, builds both zips from the tag and uploads them as workflow artifacts. A
  SignPath step stays commented out with the slugs to fill in.
- Modify: `docs/maintainers/releases.md`. Add Windows steps:
  - dispatch the workflow;
  - download the artifacts;
  - `sign_update` both zips on the Mac;
  - `make-appcast.py --windows`;
  - add both zips, `appcast-windows.xml` and the extended `SHA256SUMS.txt` to the
    draft;
  - run the VM verification;
  - publish.
- Modify: `README.md` (Windows install one-liner, SmartScreen note) and
  `docs/getting-started.md`.
- Create: `docs/windows-parity.md`. This is the feature inventory: every macOS feature
  with its Windows behavior and the test or VM scenario proving it.
- Modify: `PRODUCT.md` (platform line), `AGENTS.md` (Windows verification commands)
  and `docs/architecture.md` (Windows components).

**Steps:**
- [ ] `install.ps1` test: run it in the VM twice into an empty `KILN_INSTALL_DIR`
  against a local release mirror served from the Mac. Expected: the second run changes
  nothing, the shortcut exists with the AUMID, and `kiln --version` works in a new shell.
- [ ] `release-windows.yml` dry run on the branch with a test tag input. Expected: two
  zip artifacts. Download them and run the VM scenarios from `docs/windows-parity.md` on
  ARM64 and on x64 (emulated) builds.
- [ ] Commit: `feat(windows): installer, packaging, release workflow and docs`.

### Task 16: Final verification and release

- [ ] Full macOS CI-equivalent locally (the scratchpad `ci.sh`), the Windows CI jobs,
  and all VM scenarios. Fill the evidence column of `docs/windows-parity.md`.
- [ ] Fresh-context review of the whole branch (most capable model). Fix the Critical
  and Important findings with RED→GREEN.
- [ ] Version: `0.2.0`, the first release with Windows. Release notes are in
  `docs/releases/0.2.0.md`.
- [ ] Release:
  - macOS through the existing runbook;
  - Windows archives from `release-windows.yml` at the tag;
  - `sign_update` and the Windows appcast;
  - one GitHub release with all assets;
  - public download checks on macOS and in the VM, where `install.ps1` runs twice.
- [ ] Status doc `docs/maintainers/0.2.0-status.md` and the AGENTS.md link, committed
  with "[skip ci]".
