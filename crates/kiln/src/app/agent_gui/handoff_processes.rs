//! Read-only identity snapshots for an explicitly declared, bootstrap-owned MCP tree.
//! Arguments are used only for exact matching. Never log process arguments or environment.
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub stamp: String,
    pub executable: PathBuf,
    pub argv: Vec<String>,
}
const MAX_PROCESSES: usize = 512;
const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;

/// Inspect only supplied, owned PIDs. Missing/unreadable identities fail closed.
pub(super) fn inspect(pids: &[u32]) -> Result<Vec<Process>> {
    ensure!(pids.len() <= MAX_PROCESSES, "too many owned processes");
    ensure!(
        pids.iter().all(|pid| *pid > 0 && *pid <= i32::MAX as u32),
        "invalid owned process ID"
    );
    let unique: BTreeSet<_> = pids.iter().copied().collect();
    #[cfg(windows)]
    {
        inspect_windows(&unique)
    }
    #[cfg(not(windows))]
    {
        unique.into_iter().map(inspect_one).collect()
    }
}

#[cfg(target_os = "macos")]
fn inspect_one(pid: u32) -> Result<Process> {
    fn identity(pid: u32) -> Result<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        let count = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        ensure!(
            count == size && info.pbi_pid == pid && info.pbi_status != 5,
            "owned process identity unavailable"
        );
        Ok(info)
    }
    let before = identity(pid)?;
    let mut executable = vec![0u8; 4096];
    let count = unsafe {
        libc::proc_pidpath(
            pid as i32,
            executable.as_mut_ptr().cast(),
            executable.len() as u32,
        )
    };
    ensure!(count > 0, "owned process executable unavailable");
    let end = executable
        .iter()
        .position(|byte| *byte == 0)
        .context("invalid process executable")?;
    use std::os::unix::ffi::OsStringExt;
    let executable = PathBuf::from(std::ffi::OsString::from_vec(executable[..end].to_vec()));
    let mut args = vec![0u8; MAX_ARGUMENT_BYTES];
    let mut size = args.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            args.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    ensure!(
        result == 0 && size <= args.len(),
        "owned process arguments unavailable"
    );
    args.truncate(size);
    let argv = parse_macos_arguments(&args)?;
    let after = identity(pid)?;
    ensure!(
        before.pbi_start_tvsec == after.pbi_start_tvsec
            && before.pbi_start_tvusec == after.pbi_start_tvusec,
        "owned process identity changed during inspection"
    );
    Ok(Process {
        pid,
        parent: after.pbi_ppid,
        stamp: format!("{}:{}", after.pbi_start_tvsec, after.pbi_start_tvusec),
        executable,
        argv,
    })
}

/// KERN_PROCARGS2 includes argc, executable path, padding, argv, then environment.
/// Stop at exactly argc; environment contents are never parsed or retained.
#[cfg(any(target_os = "macos", test))]
fn parse_macos_arguments(bytes: &[u8]) -> Result<Vec<String>> {
    ensure!(
        bytes.len() >= 4 && bytes.len() <= MAX_ARGUMENT_BYTES,
        "invalid process argument buffer"
    );
    let argc = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
    ensure!(argc > 0 && argc <= 4096, "invalid process argument count");
    let mut at = 4;
    at += bytes[at..]
        .iter()
        .position(|b| *b == 0)
        .context("invalid process executable")?
        + 1;
    while bytes.get(at) == Some(&0) {
        at += 1;
    }
    let mut argv = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        let end = at
            + bytes
                .get(at..)
                .context("missing process arguments")?
                .iter()
                .position(|b| *b == 0)
                .context("unterminated process argument")?;
        argv.push(
            std::str::from_utf8(&bytes[at..end])
                .context("non-UTF8 process argument")?
                .to_owned(),
        );
        at = end + 1;
    }
    Ok(argv)
}

#[cfg(target_os = "linux")]
fn inspect_one(pid: u32) -> Result<Process> {
    use std::io::Read;
    fn stat(pid: u32) -> Result<(u32, String)> {
        let bytes = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let (_, tail) = bytes.rsplit_once(')').context("invalid process stat")?;
        let fields: Vec<_> = tail.split_whitespace().collect();
        ensure!(
            fields.len() > 19 && fields[0] != "Z",
            "owned process is unavailable"
        );
        Ok((fields[1].parse()?, fields[19].to_owned()))
    }
    let before = stat(pid)?;
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    let mut bytes = Vec::new();
    std::fs::File::open(format!("/proc/{pid}/cmdline"))?
        .take(MAX_ARGUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_ARGUMENT_BYTES && bytes.last() == Some(&0),
        "invalid process arguments"
    );
    bytes.pop();
    let argv = bytes
        .split(|byte| *byte == 0)
        .map(|arg| std::str::from_utf8(arg).map(str::to_owned))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let after = stat(pid)?;
    ensure!(
        before.1 == after.1,
        "owned process identity changed during inspection"
    );
    Ok(Process {
        pid,
        parent: after.0,
        stamp: after.1,
        executable,
        argv,
    })
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn inspect_one(_: u32) -> Result<Process> {
    anyhow::bail!("process inspection unavailable on this platform")
}

#[cfg(windows)]
fn inspect_windows(pids: &BTreeSet<u32>) -> Result<Vec<Process>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    };
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this owns the checked OpenProcess handle exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }
    fn open(pid: u32) -> Result<Handle> {
        // SAFETY: read-only query rights; never inherited by children.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        ensure!(!handle.is_null(), "owned process identity unavailable");
        Ok(Handle(handle))
    }
    fn identity(handle: &Handle) -> Result<u64> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let mut status = 0;
        // SAFETY: output pointers refer to correctly sized values.
        let valid = unsafe {
            GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) != 0
                && GetExitCodeProcess(handle.0, &mut status) != 0
        };
        ensure!(valid && status == 259, "owned process is unavailable"); // STILL_ACTIVE
        Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
    }
    pids.iter()
        .map(|&pid| {
            // Keep the original handle open across separate daemon metadata queries.
            // Reopen and compare creation times afterwards to reject PID reuse.
            let handle = open(pid)?;
            let before = identity(&handle)?;
            let mut image = vec![0u16; 32768];
            let mut len = image.len() as u32;
            // SAFETY: image has space for the advertised number of UTF-16 units.
            ensure!(
                unsafe { QueryFullProcessImageNameW(handle.0, 0, image.as_mut_ptr(), &mut len) }
                    != 0,
                "owned process executable unavailable"
            );
            let executable = PathBuf::from(std::ffi::OsString::from_wide(&image[..len as usize]));
            // Reuse the daemon's bounded native command-line reader and parent
            // creation-time check, without spawning PowerShell/CIM on every poll.
            let parent = kiln_daemon::procinfo::parent_of(pid)
                .context("owned process parent unavailable")?;
            let argv = kiln_daemon::procinfo::cmdline(pid)
                .context("owned process arguments unavailable")?;
            ensure!(
                !argv.is_empty()
                    && argv.len() <= 4096
                    && argv.iter().map(String::len).sum::<usize>() <= MAX_ARGUMENT_BYTES,
                "invalid process arguments"
            );
            ensure!(
                identity(&handle)? == before && identity(&open(pid)?)? == before,
                "owned process identity changed during inspection"
            );
            Ok(Process {
                pid,
                parent,
                stamp: before.to_string(),
                executable,
                argv,
            })
        })
        .collect()
}

fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}
fn same_file(left: &Path, right: &Path) -> bool {
    canonical(left)
        .zip(canonical(right))
        .is_some_and(|(left, right)| left == right)
}
fn command_path(command: &str, config: &serde_json::Value) -> Option<PathBuf> {
    let file = Path::new(command);
    if file.is_absolute() || file.components().count() > 1 {
        return canonical(file);
    }
    let path = config
        .get("env")
        .and_then(|env| env.get("PATH"))
        .and_then(serde_json::Value::as_str)
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(file);
        if let Some(path) = canonical(&candidate) {
            return Some(path);
        }
        #[cfg(windows)]
        for extension in ["exe", "com", "cmd", "bat"] {
            if let Some(path) = canonical(&candidate.with_extension(extension)) {
                return Some(path);
            }
        }
    }
    None
}
fn same_argument(left: &str, right: &str) -> bool {
    left == right
        || (Path::new(left).is_absolute()
            && Path::new(right).is_absolute()
            && same_file(Path::new(left), Path::new(right)))
}
fn matches_command(process: &Process, command: &Path, args: &[String]) -> bool {
    let tail = if same_file(&process.executable, command) {
        process.argv.get(1..)
    } else if process
        .argv
        .get(1)
        .is_some_and(|script| same_file(Path::new(script), command))
    {
        process.argv.get(2..)
    } else {
        None
    };
    tail.is_some_and(|tail| {
        tail.len() == args.len()
            && tail
                .iter()
                .zip(args)
                .all(|(left, right)| same_argument(left, right))
    })
}
fn primary_matches(process: &Process, program: &Path) -> bool {
    let resolved = program
        .to_str()
        .and_then(|program| command_path(program, &serde_json::Value::Null));
    let program = resolved.as_deref().unwrap_or(program);
    same_file(&process.executable, program)
        || process
            .argv
            .get(1)
            .is_some_and(|script| same_file(Path::new(script), program))
}

/// Permit only unchanged bootstrap identities, the backend, explicitly declared stdio MCP
/// roots and their bootstrap descendants, and the launcher ancestry joining those roots.
pub(super) fn helpers_are_declared(
    backend: u32,
    initial: &[Process],
    current: &[Process],
    declarations: &[serde_json::Value],
    provider_program: &Path,
) -> bool {
    if initial.len() > MAX_PROCESSES || current.is_empty() || current.len() > MAX_PROCESSES {
        return false;
    }
    let old: BTreeMap<_, _> = initial.iter().map(|p| (p.pid, p)).collect();
    let now: BTreeMap<_, _> = current.iter().map(|p| (p.pid, p)).collect();
    if now.len() != current.len() {
        return false;
    }
    if !now.contains_key(&backend) {
        return false;
    }
    let Some(_) = old.get(&backend) else {
        return false;
    };
    if !initial
        .iter()
        .any(|candidate| candidate.pid == backend && primary_matches(candidate, provider_program))
    {
        return false;
    }
    for process in current {
        let Some(previous) = old.get(&process.pid) else {
            return false;
        };
        if process.stamp.is_empty()
            || process.stamp != previous.stamp
            || process.parent != previous.parent
            || process.executable != previous.executable
        {
            return false;
        }
    }
    // Multiple argv observations of one stable identity allow fork/exec and
    // native process-title changes; identity disagreement still fails closed.
    if initial.iter().any(|process| {
        old.get(&process.pid).is_none_or(|other| {
            process.stamp != other.stamp
                || process.parent != other.parent
                || process.executable != other.executable
        })
    }) {
        return false;
    }
    let mut allowed = BTreeSet::from([backend]);
    for declaration in declarations {
        let Some(config) = declaration.get("config") else {
            continue;
        };
        if config
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("stdio"))
        {
            continue;
        }
        let Some(command) = config
            .get("command")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let args = match config.get("args") {
            None => Vec::new(),
            Some(serde_json::Value::Array(values)) => {
                let Some(args) = values
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                args
            }
            _ => continue,
        };
        let Some(command) = command_path(command, config) else {
            continue;
        };
        for root in initial
            .iter()
            .filter(|process| process.pid != backend && matches_command(process, &command, &args))
        {
            let mut chain = BTreeSet::new();
            let mut at = root.pid;
            while at != backend {
                if !chain.insert(at) {
                    break;
                }
                let Some(process) = old.get(&at) else {
                    break;
                };
                at = process.parent;
            }
            if at != backend {
                continue;
            }
            allowed.extend(chain);
            for process in initial {
                let mut at = process.pid;
                let mut seen = BTreeSet::new();
                while at != backend && seen.insert(at) {
                    if at == root.pid {
                        allowed.insert(process.pid);
                        break;
                    }
                    let Some(parent) = old.get(&at) else {
                        break;
                    };
                    at = parent.parent;
                }
            }
        }
    }
    current.iter().all(|process| allowed.contains(&process.pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn process(pid: u32, parent: u32, executable: &Path, argv: &[&str]) -> Process {
        Process {
            pid,
            parent,
            stamp: format!("created-{pid}"),
            executable: executable.into(),
            argv: argv.iter().map(|s| (*s).into()).collect(),
        }
    }
    #[test]
    fn declared_python_and_shell_script_trees_require_exact_argv_and_identity() {
        let temp = tempfile::tempdir().unwrap();
        let backend = temp.path().join("provider");
        let python = temp.path().join("python");
        let shell = temp.path().join("shell");
        let script = temp.path().join("mcp.sh");
        for path in [&backend, &python, &shell, &script] {
            std::fs::write(path, "fixture").unwrap();
        }
        let python_name = python.to_str().unwrap();
        let script_name = script.to_str().unwrap();
        let initial = vec![
            process(1, 0, &backend, &["provider"]),
            process(2, 1, &shell, &["launcher"]),
            process(3, 2, &python, &[python_name, "server.py", "--stdio"]),
            process(4, 3, &python, &["worker"]),
            process(5, 1, &shell, &["shell", script_name, "--stdio"]),
        ];
        let declarations = vec![
            json!({"config":{"type":"stdio","command":python,"args":["server.py","--stdio"]}}),
            json!({"config":{"command":script,"args":["--stdio"]}}),
        ];
        assert!(helpers_are_declared(
            1,
            &initial,
            &initial,
            &declarations,
            &backend
        ));
        assert!(!helpers_are_declared(1, &initial, &initial, &[], &backend));
        let mut wrong = declarations.clone();
        wrong[0]["config"]["args"] = json!(["other.py", "--stdio"]);
        assert!(!helpers_are_declared(
            1, &initial, &initial, &wrong, &backend
        ));
        let mut titled = initial.clone();
        titled[0].argv = vec!["provider title after ready".into()];
        titled[2].argv = vec!["native MCP updated title".into()];
        assert!(helpers_are_declared(
            1,
            &initial,
            &titled,
            &declarations,
            &backend
        ));
        let mut reparented = initial.clone();
        reparented[3].parent = 7;
        assert!(!helpers_are_declared(
            1,
            &initial,
            &reparented,
            &declarations,
            &backend
        ));
        let mut current = initial.clone();
        current[3].stamp = "reused-pid".into();
        assert!(!helpers_are_declared(
            1,
            &initial,
            &current,
            &declarations,
            &backend
        ));
        let mut current = initial.clone();
        current.push(process(6, 3, &python, &["new worker"]));
        assert!(!helpers_are_declared(
            1,
            &initial,
            &current,
            &declarations,
            &backend
        ));
        let mut initial = initial;
        initial.push(process(7, 1, &shell, &["unrelated background"]));
        assert!(!helpers_are_declared(
            1,
            &initial,
            &initial,
            &declarations,
            &backend
        ));
        assert!(!helpers_are_declared(
            9,
            &initial,
            &initial,
            &declarations,
            &backend
        ));
    }
    #[cfg(unix)]
    #[test]
    fn canonical_scripts_and_explicit_path_lookup_are_not_basename_guesses() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("server.py");
        let link = temp.path().join("mcp");
        std::fs::write(&script, "fixture").unwrap();
        std::os::unix::fs::symlink(&script, &link).unwrap();
        let backend = temp.path().join("provider");
        let interpreter = temp.path().join("interpreter");
        for path in [&backend, &interpreter] {
            std::fs::write(path, "fixture").unwrap();
        }
        let initial = vec![
            process(
                1,
                0,
                &interpreter,
                &["interpreter", backend.to_str().unwrap()],
            ),
            process(
                2,
                1,
                &interpreter,
                &["interpreter", script.to_str().unwrap(), "stdio"],
            ),
        ];
        let declaration =
            json!({"config":{"command":"mcp","args":["stdio"],"env":{"PATH":temp.path()}}});
        let mut titled = initial.clone();
        titled[0].argv = vec!["provider process title".into()];
        titled[1].argv = vec!["MCP process title".into()];
        assert!(helpers_are_declared(
            1,
            &initial,
            &titled,
            &[declaration],
            &backend
        ));
        assert!(!helpers_are_declared(
            1,
            &initial,
            &initial,
            &[json!({"config":{"command":"mcp","args":["stdio"],"type":"http"}})],
            &backend
        ));
        let wrong_dir = temp.path().join("other");
        std::fs::create_dir(&wrong_dir).unwrap();
        let other = wrong_dir.join("mcp");
        std::fs::write(&other, "other").unwrap();
        assert!(!helpers_are_declared(
            1,
            &initial,
            &initial,
            &[json!({"config":{"command":other,"args":["stdio"]}})],
            &backend
        ));
    }
    #[test]
    fn macos_argc_parser_stops_before_environment_and_preserves_empty_arguments() {
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(
            b"/usr/bin/python\0\0python\0server.py\0\0SENSITIVE_ENV=never-read\0",
        );
        assert_eq!(
            parse_macos_arguments(&bytes).unwrap(),
            ["python", "server.py", ""]
        );
        bytes[..4].copy_from_slice(&4097i32.to_ne_bytes());
        assert!(parse_macos_arguments(&bytes).is_err());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "owned subprocess fixture"]
    fn windows_inspection_child() {
        if std::env::var("KILN_INSPECTION_CHILD").as_deref() != Ok("1") { return; }
        use std::io::{Read, Write};
        println!("INSPECTION_READY");
        std::io::stdout().flush().unwrap();
        let mut byte = [0];
        let _ = std::io::stdin().read_exact(&mut byte);
    }
    #[cfg(windows)]
    #[test]
    fn native_windows_inspection_preserves_owned_child_identity_and_quoted_arguments() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let program = std::env::current_exe().unwrap();
        let marker = "kiln process inspection 한국어 fixture";
        // Explicit readiness and an owned stdin barrier keep both snapshots on
        // the same live process, independent of cmd.exe's console lifetime.
        let mut child = Child(
            Command::new(&program)
                .args(["--exact", "app::agent_gui::host::handoff_processes::tests::windows_inspection_child",
                    "--ignored", "--nocapture", "--skip", marker])
                .env("KILN_INSPECTION_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (ready, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if matches!(line.as_deref(), Ok("INSPECTION_READY")) {
                    let _ = ready.send(());
                    break;
                }
            }
        });
        received.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        reader.join().unwrap();
        let pid = child.0.id();
        let snapshot = inspect(&[pid]).unwrap();
        assert_eq!(snapshot[0].parent, std::process::id());
        assert!(same_file(&snapshot[0].executable, &program));
        assert_eq!(snapshot[0].argv.last().map(String::as_str), Some(marker));
        assert_eq!(snapshot[0].stamp, inspect(&[pid]).unwrap()[0].stamp);
        drop(child);
        assert!(inspect(&[pid]).is_err());
    }
    #[test]
    fn own_identity_is_stable_across_read_only_snapshots() {
        let pid = std::process::id();
        let first = inspect(&[pid]).unwrap();
        let second = inspect(&[pid]).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].stamp, second[0].stamp);
        assert!(first[0].executable.is_absolute());
        assert!(!first[0].argv.is_empty());
    }
}
