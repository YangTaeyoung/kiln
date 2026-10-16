//! Shell integration metadata, independent of terminal drawing and legacy upgrade snapshots.
//! OSC 133 A/C/D: https://code.visualstudio.com/docs/terminal/shell-integration
use crate::{emu::Emu, osc::{OscEvent, OscScanner}};
use kiln_proto::{AgentActivity, CommandRecord, SessionTelemetry};
use std::collections::HashMap;
const MAX_COMMANDS: usize = 40;
const MAX_OUTPUT: usize = 128 * 1024;

#[derive(Default)]
pub struct Tracker {
    scanner: OscScanner,
    pub state: SessionTelemetry,
    next: u64,
    cols: u16,
    rows: u16,
    pending: String,
    current: Option<u64>,
    capture: Vec<u8>,
    truncated: bool,
    outputs: HashMap<u64, (String, bool)>,
}
fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() }
impl Tracker {
    pub fn resize(&mut self, cols: u16, rows: u16) { self.cols = cols; self.rows = rows; }
    pub fn feed(&mut self, bytes: &[u8], cwd: Option<&str>) -> bool {
        let before = self.state.clone();
        let mut events = Vec::new();
        // Marker boundaries matter: start/output/end can arrive in the same PTY read.
        for byte in bytes {
            if self.current.is_some() {
                if self.capture.len() < MAX_OUTPUT { self.capture.push(*byte); } else { self.truncated = true; }
            }
            self.scanner.feed(std::slice::from_ref(byte), &mut events);
            for event in events.drain(..) {
                match event {
                    OscEvent::CommandText(text) => {
                        // Another integration may emit C before our command text. Attach late
                        // metadata to that boundary instead of leaking it into the next command.
                        if let Some(id)=self.current {
                            if let Some(record)=self.state.commands.iter_mut().find(|r|r.id==id) {
                                if record.command.is_empty(){record.command=text;}
                            }
                        } else {self.pending=text;}
                    },
                    OscEvent::Prompt => { self.state.shell_integration = true; self.pending.clear(); }
                    OscEvent::CommandStart => {
                        self.state.shell_integration = true;
                        if self.current.is_some() { continue; }
                        self.next += 1;
                        self.current = Some(self.next);
                        self.capture.clear(); self.truncated = false;
                        if self.state.commands.len() >= MAX_COMMANDS {
                            let old = self.state.commands.remove(0); self.outputs.remove(&old.id);
                        }
                        self.state.commands.push(CommandRecord { id: self.next, command: std::mem::take(&mut self.pending), cwd: cwd.map(str::to_owned), started_unix: now(), ..Default::default() });
                    }
                    OscEvent::CommandEnd(code) => self.finish(code),
                    OscEvent::Activity(activity) => self.state.activity = activity,
                    _ => {},
                }
            }
        }
        before != self.state
    }
    fn finish(&mut self, code: Option<i32>) {
        let Some(id) = self.current.take() else { return; };
        let mut emu = Emu::new(if self.cols == 0 { 80 } else { self.cols }, if self.rows == 0 { 24 } else { self.rows });
        emu.advance(&self.capture);
        let mut text = emu.text(10000).trim_end().to_string();
        if text.len() > MAX_OUTPUT { let mut end = MAX_OUTPUT; while !text.is_char_boundary(end) { end -= 1; } text.truncate(end); self.truncated = true; }
        self.outputs.insert(id, (text, self.truncated));
        if let Some(record) = self.state.commands.iter_mut().find(|r| r.id == id) {
            record.finished_unix = Some(now()); record.exit_code = code; record.output_available = true;
        }
        self.capture.clear();
    }
    pub fn exited(&mut self) { self.finish(None); self.state.activity = AgentActivity::Unknown; }
    pub fn output(&self, id: u64) -> Option<(String, bool)> { self.outputs.get(&id).cloned() }
}

/// Inject only zsh interactive launches. Unsupported/custom shells remain explicitly unintegrated.
/// User startup files are sourced, never rewritten. KILN_SHELL_INTEGRATION=0 disables injection.
#[cfg(unix)]
pub fn configure_zsh(cmd: &mut std::process::Command, program: &str, spec: &kiln_proto::SpawnSpec) -> std::io::Result<()> {
    if std::path::Path::new(program).file_name().is_none_or(|n| n != "zsh") || !spec.args.is_empty()
        || std::env::var("KILN_SHELL_INTEGRATION").as_deref() == Ok("0")
        || spec.env.iter().any(|(k,v)| k == "KILN_SHELL_INTEGRATION" && v == "0") { return Ok(()); }
    let Some(home) = std::env::var_os("HOME") else { return Ok(()); };
    let dir = std::path::PathBuf::from(&home).join(".local/share/kiln/shell-integration-v1");
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e|e.into_inner());
    std::fs::create_dir_all(&dir)?;
    let original = spec.env.iter().find(|(k,_)| k == "ZDOTDIR").map(|(_,v)| std::ffi::OsString::from(v))
        .or_else(|| std::env::var_os("ZDOTDIR")).unwrap_or(home);
    std::fs::write(dir.join("integration.zsh"), include_str!("shell-integration.zsh"))?;
    for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        let mut script = format!("# Kiln shell integration, generated. User configuration is not modified.\nexport ZDOTDIR=\"$KILN_ORIGINAL_ZDOTDIR\"\n[[ -f \"$KILN_ORIGINAL_ZDOTDIR/{name}\" ]] && source \"$KILN_ORIGINAL_ZDOTDIR/{name}\"\n");
        // Keep startup lookup in the wrapper until zshrc, even if user's zshenv changes ZDOTDIR.
        if name == ".zshenv" || name == ".zprofile" { script.push_str("export KILN_ORIGINAL_ZDOTDIR=\"$ZDOTDIR\"\nexport ZDOTDIR=\"$KILN_INTEGRATION_DIR\"\n"); }
        if name == ".zshrc" { script.push_str(include_str!("shell-integration.zsh")); script.push_str("\n[[ -o login ]] && export ZDOTDIR=\"$KILN_INTEGRATION_DIR\"\n"); }
        if name == ".zlogin" { script.push_str("export ZDOTDIR=\"$KILN_ORIGINAL_ZDOTDIR\"\n"); }
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
            let temporary = dir.join(format!("{name}.{}.tmp",std::process::id()));
            std::fs::write(&temporary,script)?;
            std::fs::rename(temporary,path)?;
        }
    }
    cmd.env("KILN_ORIGINAL_ZDOTDIR", original).env("KILN_INTEGRATION_DIR", &dir).env("ZDOTDIR", dir);
    Ok(())
}

/// The PowerShell integration script.
#[cfg(windows)]
pub const POWERSHELL_INTEGRATION: &str = include_str!("shell-integration.ps1");

#[cfg(windows)]
fn integration_disabled(spec: &kiln_proto::SpawnSpec) -> bool {
    std::env::var("KILN_SHELL_INTEGRATION").as_deref() == Ok("0")
        || spec.env.iter().any(|(k, v)| k == "KILN_SHELL_INTEGRATION" && v == "0")
}

#[cfg(windows)]
fn program_stem(program: &str) -> String {
    std::path::Path::new(program).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Arguments that load the integration into a new interactive PowerShell without arguments.
/// The user's profile loads first; the script is read as text, so the execution policy for
/// script files does not apply. `KILN_SHELL_INTEGRATION=0` disables it.
#[cfg(windows)]
pub fn powershell_args(program: &str, spec: &kiln_proto::SpawnSpec) -> Option<Vec<String>> {
    if !matches!(program_stem(program).as_str(), "pwsh" | "powershell") || !spec.args.is_empty() || integration_disabled(spec) {
        return None;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Kiln").join("shell-integration-v1");
    let path = dir.join("integration.ps1");
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if std::fs::read_to_string(&path).ok().as_deref() != Some(POWERSHELL_INTEGRATION) {
        std::fs::create_dir_all(&dir).ok()?;
        let temporary = dir.join(format!("integration.ps1.{}.tmp", std::process::id()));
        std::fs::write(&temporary, POWERSHELL_INTEGRATION).ok()?;
        std::fs::rename(&temporary, &path).ok()?;
    }
    let quoted = path.to_string_lossy().replace('\'', "''");
    Some(vec!["-NoExit".into(), "-Command".into(), format!("Invoke-Expression ([System.IO.File]::ReadAllText('{quoted}'))")])
}

/// The `PROMPT` for a new interactive cmd.exe: OSC 7 with the current folder and the prompt
/// mark before the user's prompt (`$P$G` when none is set).
#[cfg(windows)]
pub fn cmd_prompt(program: &str, spec: &kiln_proto::SpawnSpec) -> Option<String> {
    if program_stem(program) != "cmd" || !spec.args.is_empty() || integration_disabled(spec) {
        return None;
    }
    let user = spec.env.iter().find(|(k, _)| k.eq_ignore_ascii_case("PROMPT")).map(|(_, v)| v.clone())
        .or_else(|| std::env::var("PROMPT").ok())
        .unwrap_or_else(|| "$P$G".into());
    Some(format!("$E]7;file://localhost/$P$E\\$E]133;A$E\\{user}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn captures_lifecycle_across_chunks_and_does_not_guess_agent_status() {
        let mut t = Tracker::default();
        for c in b"\x1b]777;kiln-command;6563686f206f6b\x07\x1b]133;C\x07ok\r\n\x1b]133;D;0\x07".chunks(3) { t.feed(c, Some("/tmp")); }
        assert_eq!(t.state.commands.len(), 1);
        let r = &t.state.commands[0]; assert_eq!(r.command, "echo ok"); assert_eq!(r.exit_code, Some(0));
        assert_eq!(t.output(r.id).unwrap().0, "ok"); assert_eq!(t.state.activity, AgentActivity::Unknown);
        t.feed(b"\x1b]777;kiln-agent;waiting\x07", None); assert_eq!(t.state.activity, AgentActivity::Waiting);
        t.feed(b"\x1b]777;notify;agent;done\x07", None); assert_eq!(t.state.activity, AgentActivity::Waiting);
    }
    #[test] fn foreign_start_before_kiln_text_attaches_metadata_to_current_command() {
        let mut t=Tracker::default();
        for name in ["alpha","beta"] {
            let text=format!("echo kiln-purpose-{name}");
            let hex=text.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>();
            let seq=format!("\x1b]133;C\x07\x1b]777;kiln-command;{hex}\x07\x1b]133;C\x07{name}\r\n\x1b]133;D;0\x07\x1b]133;D;0\x07\x1b]133;A\x07");
            for chunk in seq.as_bytes().chunks(3){t.feed(chunk,None);}
            assert_eq!(t.state.commands.last().unwrap().command,text);
        }
        assert_eq!(t.state.commands.len(),2);
        assert_eq!(t.output(1).unwrap().0,"alpha");assert_eq!(t.output(2).unwrap().0,"beta");
    }
    #[test] fn rejects_oversized_command_without_reusing_previous_text() {
        let mut t=Tracker::default();
        t.feed(b"\x1b]777;kiln-command;6563686f\x07\x1b]133;A\x07",None);
        let mut oversized=b"\x1b]777;kiln-command;".to_vec();
        oversized.extend(vec![b'6';9000]); oversized.extend_from_slice(b"\x07\x1b]133;C\x07\x1b]133;D;0\x07");
        t.feed(&oversized,None);
        assert_eq!(t.state.commands[0].command, "");
    }
    #[test] fn missing_exit_status_remains_unknown_and_no_duplicate_command() {
        let mut t=Tracker::default(); t.feed(b"\x1b]133;C\x07\x1b]133;C\x07err\x1b]133;D\x07",None);
        assert_eq!(t.state.commands.len(),1); assert_eq!(t.state.commands[0].exit_code,None);
    }
    #[cfg(unix)]
    #[test]
    fn real_zsh_preserves_startup_and_reports_success_failure_output() {
        use std::io::Write;
        let temp=tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".zshrc"), "export KILN_USER_RC_WORKED=yes\nPS1='test> '\n").unwrap();
        let spec=kiln_proto::SpawnSpec {program:Some("/bin/zsh".into()),env:vec![("ZDOTDIR".into(),temp.path().to_string_lossy().into_owned())],cols:80,rows:24,..Default::default()};
        let pty=crate::pty::Pty::spawn(&spec,991).unwrap();
        let mut reader=pty.reader().unwrap(); let mut writer=pty.writer().unwrap(); let mut tracker=Tracker::default();
        let mut bytes=[0;16384]; let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        let mut sent=false;
        while std::time::Instant::now()<deadline {
            if let crate::pty::ReadResult::Data(n)=reader.read_timeout(&mut bytes,100).unwrap() {tracker.feed(&bytes[..n],Some("/tmp"));}
            if tracker.state.shell_integration && !sent {writer.write_all(b"printf 'rc=%s\\n' \"$KILN_USER_RC_WORKED\"; false\r").unwrap();sent=true;}
            if tracker.state.commands.iter().any(|c|c.finished_unix.is_some()) {break;}
        }
        pty.kill();
        let command=tracker.state.commands.last().expect("command start reported");
        assert_eq!(command.exit_code,Some(1));
        assert!(command.command.contains("KILN_USER_RC_WORKED"));
        assert!(tracker.output(command.id).unwrap().0.contains("rc=yes"));
    }

}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn run_powershell(script: &str) -> String {
        let output = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", script]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn powershell_integration_wraps_an_existing_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("integration.ps1");
        std::fs::write(&script, POWERSHELL_INTEGRATION).unwrap();
        let out = run_powershell(&format!(
            "function prompt {{ 'CUSTOM> ' }}; Set-Location -LiteralPath '{}'; Invoke-Expression ([IO.File]::ReadAllText('{}')); [Console]::Out.Write((prompt))",
            dir.path().display(),
            script.display()
        ));
        assert!(out.ends_with("CUSTOM> "), "{out:?}");
        assert!(out.contains("\x1b]133;A\x07"), "{out:?}");
        let location = format!("\x1b]7;file://localhost/{}", dir.path().to_string_lossy().replace('\\', "/").replace(':', "%3A"));
        assert!(out.contains(&location), "{out:?}");
    }

    #[test]
    fn powershell_integration_is_offered_only_to_plain_interactive_powershell() {
        let spec = |program: &str, args: &[&str]| kiln_proto::SpawnSpec { program: Some(program.into()), args: args.iter().map(|a| a.to_string()).collect(), ..Default::default() };
        assert!(powershell_args("pwsh.exe", &spec("pwsh.exe", &[])).is_some());
        assert!(powershell_args(r"C:\Windows\System32\WindowsPowerShell\v1.0\PowerShell.exe", &spec("powershell", &[])).is_some());
        assert!(powershell_args("pwsh.exe", &spec("pwsh.exe", &["-Command", "dir"])).is_none());
        assert!(powershell_args("cmd.exe", &spec("cmd.exe", &[])).is_none());
        let mut off = spec("pwsh.exe", &[]);
        off.env.push(("KILN_SHELL_INTEGRATION".into(), "0".into()));
        assert!(powershell_args("pwsh.exe", &off).is_none());
    }
}
