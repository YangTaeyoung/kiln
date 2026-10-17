# Windows support

Kiln on Windows has the same interface and features as on macOS. This page lists each
feature's Windows behavior and the test or VM scenario that checks it. Automated
tests run in the `windows-x64` and `windows-arm64` CI jobs; VM scenarios run in a
Windows 11 ARM64 virtual machine against release builds.

## Install, update and identity

| Feature | Windows behavior | Checked by |
| --- | --- | --- |
| Installation | `install.ps1` picks the archive for the machine Windows runs on (also from an emulated x64 PowerShell), installs `Kiln.exe` and `bin\kiln.exe` into `%LOCALAPPDATA%\Programs\Kiln`, adds a Start-menu shortcut with the app ID `dev.kiln.app` and adds `bin` to the user `Path`, keeping its value type and unexpanded entries. An existing installation is left in place. | VM: installer run twice against a local release mirror, from x86 PowerShell and over a partial folder |
| Second start | Starting Kiln again brings the running window forward and opens the given folder there. | `a_second_start_finds_the_first`, `a_folder_request_carries_its_path_once`; VM: a second start with a folder |
| Executables | `Kiln.exe` is a windows-subsystem GUI with icon, version and DPI-aware manifest; `bin\kiln.exe` is the console CLI and daemon. Helpers start without console windows. | `cli_executable` tests; VM: window list shows only the Kiln window |
| Updates | A signed `appcast-windows.xml` offers one archive per architecture and plain `N.N.N` versions. Kiln checks once a day, verifies length and the Sparkle Ed25519 signature, extracts with `tar.exe`, checks `bin\kiln.exe --version`, renames running files to `.old` and restarts. | `updater_windows` tests (appcast, signature, length, install, rollback), `a_feed_version_that_is_not_a_plain_release_number_is_refused`, `scripts/test-make-appcast.py`; VM: update from the previous release |
| Notifications | Agent notifications show as Windows toasts under Kiln's app ID (Windows PowerShell's before the Start-menu shortcut exists). | `toast_text_travels_in_the_environment_not_the_script`; VM: toast screenshot |
| Background companion | A notification-area icon shows the session summary, a red dot for attention or unread notifications, Open Kiln, Settings and Stop background sessions (Cancel is the default). It runs from the daemon's per-build copy, is replaced by newer builds and returns after Explorer restarts. | `companion` and `tray_windows` tests, `the_companion_window_is_a_top_level_window_that_hears_broadcasts`; VM: menu, settings request, stop confirmation, attention dot |

## Terminals

| Feature | Windows behavior | Checked by |
| --- | --- | --- |
| Shells | PowerShell 7, else Windows PowerShell, by default; `cmd.exe`, Git Bash and WSL distributions can be chosen. | `hosted` and `daemon` integration tests |
| Session ownership | ConPTY inside a Job Object owned by a per-terminal host; closing a terminal ends its console programs; window applications it started, such as a browser, keep running. Hosts survive a daemon restart and are adopted. | `killed_session_leaves_no_descendants`, `killing_a_session_spares_window_applications_it_started`, `daemon_crash_is_recovered_by_adopting_hosts`, `attach_to_an_unresponsive_host_times_out` |
| Shell integration | PowerShell and cmd report commands, exit codes and the location (OSC 7, 133, 777); a custom prompt still sees the last command's status and an existing `PSConsoleHostReadLine` is chained. Share and mapped-drive folders keep their form. Task names come from the command line. | `powershell_integration_reports_commands_exit_codes_and_location`, `the_original_prompt_still_sees_the_last_command_fail`, `an_existing_read_line_function_is_chained`, `osc7_on_windows_names_drive_and_share_folders`, `a_mapped_drive_keeps_its_letter`, `cmd_prompt_reports_location`; VM: task names in the sidebar |
| Foreground program | The newest console descendant of the shell (preferring a single agent) names the pane and drives close confirmations; window applications are skipped. Programs started by Git Bash's MSYS runtime are not followed. | `procinfo` Windows tests, `window_applications_are_not_the_foreground`; VM: "claude" detected inside PowerShell |
| Korean input | IME composition and commit in terminals and the editor through winit's IMM32 handling. | VM: Korean IME in a terminal |
| Completion popup | PowerShell keeps PSReadLine's completion (Tab, Ctrl+Space); Kiln's popup stays zsh-only. | Ruling in the implementation ledger |

## Agents and conversations

| Feature | Windows behavior | Checked by |
| --- | --- | --- |
| Agent launch | Codex and Claude Code run as native Windows programs from `%USERPROFILE%\.codex` and `%USERPROFILE%\.claude`. npm's `.cmd` launchers are replaced by the script or program they start, so requests can span lines. `kiln agent-run` survives Ctrl+C and returns the agent's exit code. | `a_contained_agent_reports_its_exit_code`, `an_npm_launcher_runs_its_script_with_node_so_requests_can_span_lines`; VM: Codex 0.162.1 and Claude Code 2.1.301 in Kiln |
| Hooks | Codex hooks run in PowerShell with a plain or short path; Claude Code hooks run in Git Bash. Hooks find their launch record from the process tree because Codex clears the environment. | `hook_commands_follow_the_shell_each_cli_uses`, `a_launch_is_found_in_the_agent_run_command_line`; VM: records confirmed for both CLIs |
| Conversation sync | Transcript paths are matched to account roots spelled another way (case, short names, `\\?\`); sync folders admit only the current user. Conversations continue in the other CLI. | `hook_transcript_paths_match_canonical_roots`, `a_restricted_folder_admits_only_the_current_user_and_passes_that_on`; VM: `claude --resume` and `codex resume` continue each other's conversations |
| Install detection | npm's `codex.cmd` launchers and Homebrew paths are recognized by path components. | `npm_installation_is_found_from_the_windows_launcher` |
| Accounts | Kiln's account snapshots are DPAPI-protected files; Codex's keyring entry is read from the Credential Manager under the key Codex computes. | `file_secrets_on_windows_are_protected_with_the_user_key`, `keyring_key_hashes_the_path_as_the_cli_canonicalizes_it`; CI: Credential Manager step with a real Codex sign-in |

## Tools

| Feature | Windows behavior | Checked by |
| --- | --- | --- |
| SFTP and SSH | Windows OpenSSH (`System32\OpenSSH`); masked permission listings parse; downloads hand sftp a closed temporary file. | `windows_openssh_masked_permissions_still_parse`; VM: `sftp_round_trip` from Windows to an Ubuntu VM |
| SSH config | Backslashes follow OpenSSH: only quotes, backslashes and unquoted spaces are escaped, so `C:\…` paths stay intact. | `backslashes_escape_only_quotes_backslashes_and_unquoted_spaces` |
| Cloud CLIs | `.cmd` shims are found through `PATHEXT`; cancelling ends the interpreter behind the shim; gcloud and Wrangler folders follow `%APPDATA%`. | `cancelled_listing_stops_the_cli_tree`, `default_cli_folders_follow_each_cli_on_this_system` |
| Labels and shortcuts | App shortcuts read ⌘ as Ctrl+Shift (⇧⌘ as Ctrl+Shift+Alt, ⌥⌘ as Ctrl+Alt) and widget shortcuts as Ctrl; "Open in File Explorer"; PowerShell shell hint; Nerd Fonts in `%WINDIR%\Fonts`. | `app_shortcut_names_match_the_chords_each_platform_binds`, `shortcut_names_follow_the_platform`, `nerd_font_search_includes_the_windows_font_folders` |
| Window close | Closing the window asks about unsaved files and drafts and waits for transfers, database writes and worktree creation, as on macOS; terminals keep running in the background service. | `native_quit_*` GUI tests on Windows |

## Known limits

- Kiln is tested on Windows 11. Windows 10 1809 and later have the APIs Kiln uses, but
  Windows 10 is not tested.
- Windows executables are not code-signed until the SignPath Foundation application is
  approved; browser downloads may show a SmartScreen prompt.
- The Credential Manager needs a sign-in session that can keep credentials; accounts
  that only allow session credentials fall back to Kiln's protected files for Kiln's
  own data, while CLIs that require the Credential Manager report their own error.
- Programs started by Git Bash's MSYS runtime do not appear as the pane's foreground
  program, because the MSYS runtime breaks the Windows parent-process chain.
