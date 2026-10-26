//! Installed CLI help discovery. Only recognized command names and a final `--help`
//! reach the child; terminal buffers, option values, history and credentials do not.
use super::suggestions::{OptionDef, Spec};
use kiln_common::paths::Canonical;
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

const MAX_OUTPUT: usize = 256 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_DEPTH: usize = 4;
const TIMEOUT: Duration = Duration::from_millis(1000);
const NEGATIVE_TTL: Duration = Duration::from_secs(5 * 60);
const POSITIVE_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_RECORDS: usize = 256;
const MAX_DESCRIPTION: usize = 1024;

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    modified: SystemTime,
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn identity(path: &Path) -> Option<Identity> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Some(Identity {
        modified: meta.modified().ok()?,
        len: meta.len(),
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(unix)]
        inode: meta.ino(),
    })
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    canonical: PathBuf,
    identity: Identity,
    // rustup/cargo and other multicall symlinks share canonical metadata.
    command: String,
    context: Vec<String>,
}

struct Entry {
    key: Key,
    expires: Instant,
    spec: Option<Spec>,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
}

pub(super) fn discover(executable: &Path, command: &str, subcommands: &[String]) -> Option<Spec> {
    if !valid_context(command, subcommands) {
        return None;
    }
    validate_executable(executable, command)?;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    discover_cached(
        CACHE.get_or_init(Mutex::default),
        executable,
        command,
        subcommands,
    )
}

fn discover_cached(
    cache: &Mutex<Cache>,
    executable: &Path,
    command: &str,
    subcommands: &[String],
) -> Option<Spec> {
    if !valid_context(command, subcommands) {
        return None;
    }
    let canonical = executable.canonical().ok()?;
    let key = Key {
        identity: identity(&canonical)?,
        canonical,
        command: command.into(),
        context: subcommands.to_vec(),
    };
    {
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        cache.entries.retain(|e| {
            e.expires > now && (e.key.canonical != key.canonical || e.key.identity == key.identity)
        });
        if let Some(index) = cache.entries.iter().position(|e| e.key == key) {
            let entry = cache.entries.remove(index)?;
            let result = entry.spec.clone();
            cache.entries.push_back(entry);
            return result;
        }
        if cache.entries.len() >= MAX_CACHE_ENTRIES {
            cache.entries.pop_front();
        }
        // Reserve the key before dropping the lock: simultaneous requests fall
        // back while this one probes, without queueing more one-second children.
        cache.entries.push_back(Entry {
            key: key.clone(),
            expires: now + NEGATIVE_TTL,
            spec: None,
        });
    }
    // Execute the original path, not `canonical`: argv[0] selects rustup's tool.
    let spec = run_help(executable, subcommands)
        .and_then(|text| parse_help(&text, subcommands.last().map_or(command, String::as_str)));
    if executable.canonical().ok().as_ref() != Some(&key.canonical)
        || identity(&key.canonical).as_ref() != Some(&key.identity)
    {
        return None;
    }
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.entries.iter_mut().find(|e| e.key == key) {
        entry.expires = Instant::now()
            + if spec.is_some() {
                POSITIVE_TTL
            } else {
                NEGATIVE_TTL
            };
        entry.spec = spec.clone();
    }
    spec
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn valid_context(command: &str, subcommands: &[String]) -> bool {
    safe_name(command) && subcommands.len() <= MAX_DEPTH && subcommands.iter().all(|s| safe_name(s))
}

fn disallowed_program(path: &Path) -> bool {
    if path.components().any(|part| {
        part.as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".app")
    }) {
        return true;
    }
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    name == "daemon"
        || name.ends_with("-daemon")
        || name.starts_with("kiln-")
        || matches!(
            name.as_str(),
            "kiln"
                | "sh"
                | "bash"
                | "zsh"
                | "dash"
                | "fish"
                | "ksh"
                | "csh"
                | "tcsh"
                | "nu"
                | "python"
                | "python2"
                | "python3"
                | "ruby"
                | "irb"
                | "perl"
                | "node"
                | "deno"
                | "bun"
                | "lua"
                | "php"
                | "julia"
                | "rscript"
                | "pwsh"
                | "powershell"
                | "cmd"
                | "wscript"
                | "cscript"
                | "env"
                | "sudo"
                | "doas"
        )
        || name.starts_with("python3.")
}

fn install_roots() -> Vec<PathBuf> {
    #[cfg(unix)]
    let mut roots: Vec<PathBuf> = [
        "/bin",
        "/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/usr/local",
        "/opt/homebrew",
        "/opt/local",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    #[cfg(windows)]
    let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    #[cfg(windows)]
    if let Some(system) = std::env::var_os("SystemRoot") {
        roots.push(PathBuf::from(system).join("System32"));
    }
    if let Some(base) = directories::BaseDirs::new() {
        if let Ok(home) = base.home_dir().canonical() {
            for suffix in [
                ".local/bin",
                ".cargo/bin",
                ".docker/bin",
                ".docker/cli-plugins",
                ".rustup/toolchains",
                ".npm-global",
                ".nvm/versions/node",
                ".fnm/node-versions",
                ".local/share/fnm/node-versions",
                ".local/share/pnpm",
                ".local/share/mise/installs",
                ".local/share/mise/shims",
                ".local/share/uv/tools",
                ".local/share/pipx/venvs",
                ".local/pipx/venvs",
                ".volta/bin",
                ".volta/tools/image",
                ".bun/bin",
                ".deno/bin",
                ".asdf/shims",
                ".asdf/installs",
            ] {
                roots.push(home.join(suffix));
            }
            #[cfg(windows)]
            for suffix in [
                "scoop/shims",
                "scoop/apps",
                "AppData/Local/Programs",
                "AppData/Local/Microsoft/WinGet/Links",
                "AppData/Local/Microsoft/WinGet/Packages",
            ] {
                roots.push(home.join(suffix));
            }
        }
    }
    roots
}

fn validate_executable(executable: &Path, command: &str) -> Option<PathBuf> {
    let filename = executable.file_name()?.to_str()?;
    if filename != command
        && !(cfg!(windows) && filename.eq_ignore_ascii_case(&format!("{command}.exe")))
    {
        return None;
    }
    validate_in_roots(
        executable,
        &install_roots(),
        std::env::current_dir().ok().as_deref(),
    )
}

fn validate_in_roots(executable: &Path, roots: &[PathBuf], cwd: Option<&Path>) -> Option<PathBuf> {
    validate_policy(
        executable,
        roots,
        cwd,
        if cfg!(target_os = "macos") {
            Some(Path::new("/Applications/Docker.app"))
        } else {
            None
        },
    )
}

fn docker_desktop_cli(path: &Path, docker_app: Option<&Path>) -> bool {
    let Some(app) = docker_app else {
        return false;
    };
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some("docker" | "docker-compose" | "docker-buildx")
    ) && ["Contents/Resources/bin", "Contents/Resources/cli-plugins"]
        .iter()
        .any(|bin| path.parent() == Some(app.join(bin).as_path()))
}

fn validate_policy(
    executable: &Path,
    roots: &[PathBuf],
    cwd: Option<&Path>,
    docker_app: Option<&Path>,
) -> Option<PathBuf> {
    if !executable.is_absolute()
        || executable
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || disallowed_program(executable) && !docker_desktop_cli(executable, docker_app)
    {
        return None;
    }
    let canonical = executable.canonical().ok()?;
    if disallowed_program(&canonical) && !docker_desktop_cli(&canonical, docker_app) {
        return None;
    }
    // Check both sides of every symlink. An installed link to a project binary,
    // or a project link to an installed binary, must not become an execution grant.
    let in_roots = |path: &Path| {
        docker_desktop_cli(path, docker_app)
            || roots
                .iter()
                .any(|root| path.starts_with(root) && path != root)
    };
    if !in_roots(executable) || !in_roots(&canonical) {
        return None;
    }
    if let Some(cwd) = cwd.and_then(|p| p.canonical().ok()) {
        if cwd.parent().is_some() && (canonical.starts_with(&cwd) || executable.starts_with(&cwd)) {
            return None;
        }
    }
    let meta = fs::metadata(&canonical).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o111 == 0 || mode & 0o6000 != 0 {
            return None;
        }
    }
    #[cfg(windows)]
    if !canonical.extension()?.eq_ignore_ascii_case("exe") {
        return None;
    }
    Some(canonical)
}

fn help_command(executable: &Path, subcommands: &[String], home: &Path) -> Command {
    let mut cmd = Command::new(executable);
    cmd.args(subcommands)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .current_dir(home);
    for key in [
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "TMPDIR",
        "TEMP",
        "TMP",
    ] {
        cmd.env(key, home);
    }
    #[cfg(unix)]
    cmd.env(
        "PATH",
        "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin:/opt/local/bin",
    );
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let root = PathBuf::from(root);
            if let Ok(path) = std::env::join_paths([root.join("System32"), root.clone()]) {
                cmd.env("PATH", path);
            }
            cmd.env("SystemRoot", root);
        }
    }
    cmd.envs([
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("TERM", "dumb"),
        ("NO_COLOR", "1"),
        ("CLICOLOR", "0"),
        ("CLICOLOR_FORCE", "0"),
        ("FORCE_COLOR", "0"),
        ("PAGER", "cat"),
        ("GIT_PAGER", "cat"),
        ("MANPAGER", "cat"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "never"),
        ("COREPACK_ENABLE_NETWORK", "0"),
        ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        ("npm_config_offline", "true"),
        ("npm_config_ignore_scripts", "true"),
        ("CARGO_NET_OFFLINE", "true"),
        ("RUSTUP_AUTO_INSTALL", "0"),
        ("DO_NOT_TRACK", "1"),
        ("CI", "1"),
    ]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    kiln_common::process::hide_window(&mut cmd);
    cmd
}

struct HelpChild {
    child: Option<Child>,
    #[cfg(windows)]
    job: Option<kiln_common::process::Job>,
}

impl Drop for HelpChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        #[cfg(unix)]
        {
            // SAFETY: this child was started in its own process group, with its
            // PID as PGID. Never signal the GUI's or the user's shell's group.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_millis(75);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Err(_) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Ok(None) => break,
            }
        }
        // Reap even an unusually slow OS teardown without blocking completion.
        let _ = std::thread::Builder::new()
            .name("cli-help-reap".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
}

trait HelpPipe: Read {
    fn prepare(&self) -> io::Result<()>;
    fn ready(&self) -> io::Result<bool>;
}

#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> HelpPipe for T {
    fn prepare(&self) -> io::Result<()> {
        // SAFETY: fcntl only changes this owned pipe's nonblocking status.
        let flags = unsafe { libc::fcntl(self.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn ready(&self) -> io::Result<bool> {
        Ok(true)
    }
}

#[cfg(windows)]
impl<T: Read + std::os::windows::io::AsRawHandle> HelpPipe for T {
    fn prepare(&self) -> io::Result<()> {
        Ok(())
    }
    fn ready(&self) -> io::Result<bool> {
        // Peek avoids blocking Read on inherited handles after the leader exits.
        // Local declaration keeps this sidecar independent of extra Cargo features.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn PeekNamedPipe(
                pipe: *mut std::ffi::c_void,
                buffer: *mut std::ffi::c_void,
                size: u32,
                read: *mut u32,
                available: *mut u32,
                left: *mut u32,
            ) -> i32;
        }
        let mut available = 0;
        // SAFETY: pipe is a live owned ChildStdout/ChildStderr handle; all optional
        // buffers are null and the available-byte output is valid for this call.
        let result = unsafe {
            PeekNamedPipe(
                self.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(available > 0)
        }
    }
}

fn drain(
    pipe: &mut impl HelpPipe,
    output: &mut Vec<u8>,
    total: &mut usize,
    eof: &mut bool,
) -> Option<()> {
    if *eof {
        return Some(());
    }
    for _ in 0..16 {
        match pipe.ready() {
            Ok(false) => break,
            Ok(true) => (),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe || e.raw_os_error() == Some(233) => {
                *eof = true;
                break;
            }
            Err(_) => return None,
        }
        let mut chunk = [0; 4096];
        match pipe.read(&mut chunk) {
            Ok(0) => {
                *eof = true;
                break;
            }
            Ok(n) => {
                if *total + n > MAX_OUTPUT {
                    return None;
                }
                *total += n;
                output.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(())
}

/// Lower-level runner intentionally permits owned fixture executables. Production
/// callers must go through `discover`, which enforces the installed-path policy.
fn run_help(executable: &Path, subcommands: &[String]) -> Option<String> {
    if !executable.is_absolute()
        || subcommands.len() > MAX_DEPTH
        || !subcommands.iter().all(|s| safe_name(s))
    {
        return None;
    }
    let deadline = Instant::now() + TIMEOUT;
    let home = tempfile::Builder::new()
        .prefix("kiln-cli-help-")
        .tempdir()
        .ok()?;
    let child = help_command(executable, subcommands, home.path())
        .spawn()
        .ok()?;
    #[cfg(windows)]
    let job = kiln_common::process::Job::for_process(child.id(), true);
    let mut owned = HelpChild {
        child: Some(child),
        #[cfg(windows)]
        job,
    };
    #[cfg(windows)]
    if owned.job.is_none() {
        return None;
    }
    let child = owned.child.as_mut()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    stdout.prepare().ok()?;
    stderr.prepare().ok()?;
    let (mut out, mut err, mut total) = (Vec::new(), Vec::new(), 0);
    let (mut out_eof, mut err_eof, mut status) = (false, false, None);
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        drain(&mut stdout, &mut out, &mut total, &mut out_eof)?;
        drain(&mut stderr, &mut err, &mut total, &mut err_eof)?;
        if status.is_none() {
            status = child.try_wait().ok()?;
        }
        if let Some(status) = status {
            if !status.success() && !matches!(status.code(), Some(1 | 2)) {
                return None;
            }
            if out_eof && err_eof {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Kill any background children even when they closed the capture handles.
    drop(owned);
    let stdout = String::from_utf8(out).ok()?;
    let stderr = String::from_utf8(err).ok()?;
    let text = if stdout.is_empty() {
        stderr
    } else if stderr.is_empty() {
        stdout
    } else {
        if stdout.len() + stderr.len() >= MAX_OUTPUT {
            return None;
        }
        format!("{stdout}\n{stderr}")
    };
    if !status?.success() {
        // Some utilities (including macOS open) deliberately exit 1 for --help.
        // open also prints "unrecognized option" before its complete option table.
        // Mere error/usage/help-only boilerplate is still a discovery miss.
        let clean = clean_help(&text)?;
        let parsed = parse_help(&clean, "help")?;
        let only_boilerplate = parsed.subcommands.is_empty()
            && parsed.options.iter().all(|option| {
                option
                    .name
                    .iter()
                    .all(|name| matches!(name.as_str(), "-h" | "--help" | "-V" | "--version"))
            });
        if only_boilerplate
            && clean
                .lines()
                .filter(|s| !s.trim().is_empty())
                .take(3)
                .any(|line| {
                    let line = line.to_ascii_lowercase();
                    line.contains("error:")
                        || [
                            "unrecognized option",
                            "unknown option",
                            "unknown flag",
                            "invalid option",
                            "unsupported option",
                        ]
                        .iter()
                        .any(|error| line.contains(error))
                })
        {
            return None;
        }
    }
    Some(text)
}

#[derive(Clone, Copy)]
enum Section {
    None,
    Options(bool),
    Commands,
    Positionals,
    Other,
}
#[derive(Clone, Copy)]
enum Last {
    Option(usize, usize),
    Commands(usize, usize, usize),
}

fn append_description(description: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() || description.len() >= MAX_DESCRIPTION {
        return;
    }
    if !description.is_empty() {
        description.push(' ');
    }
    for ch in text.chars() {
        if description.len() + ch.len_utf8() > MAX_DESCRIPTION {
            break;
        }
        description.push(ch);
    }
}

fn clean_help(text: &str) -> Option<String> {
    if text.len() > MAX_OUTPUT || text.contains('\0') {
        return None;
    }
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' || (ch == '\x1b' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => (),
            },
            '\x08' => {
                output.pop();
            }
            '\r' => (),
            '\n' | '\t' => output.push(ch),
            c if !c.is_control() => output.push(c),
            _ => (),
        }
    }
    Some(output)
}

fn section_heading(line: &str) -> Option<Section> {
    let heading = line.strip_suffix(':')?.trim().to_ascii_lowercase();
    Some(match heading.as_str() {
        // "Global Options" can still belong before the subcommand (Docker).
        // Child help is authoritative for options available at that child.
        "options" | "flags" | "optional arguments" | "named arguments" | "global options" => Section::Options(false),
        "global flags"
        | "inherited flags"
        | "options inherited from parent commands" => Section::Options(true),
        "commands"
        | "subcommands"
        | "sub-commands"
        | "available commands"
        | "available subcommands"
        | "common commands"
        | "management commands"
        | "swarm commands" => Section::Commands,
        "positional arguments" => Section::Positionals,
        "arguments"
        | "args"
        | "usage"
        | "aliases"
        | "examples"
        | "example"
        | "notes"
        | "environment"
        | "environment variables"
        | "see also" => Section::Other,
        _ => return None,
    })
}

// Two or more spaces separate a declaration from its description. Preserve gaps
// inside placeholders and between comma-separated option aliases.
fn columns(line: &str) -> (&str, &str) {
    let line = line.trim();
    let bytes = line.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'[' | b'{' | b'(' => depth += 1,
            b'>' | b']' | b'}' | b')' => depth = depth.saturating_sub(1),
            b' ' | b'\t' if depth == 0 => {
                let start = i;
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                if (i - start >= 2 || bytes[start] == b'\t')
                    && !line[..start].ends_with([',', '/', '|'])
                {
                    return (line[..start].trim_end(), line[i..].trim());
                }
                continue;
            }
            _ => (),
        }
        i += 1;
    }
    (line, "")
}

fn valid_flag(name: &str) -> bool {
    let bare = name
        .strip_prefix("--")
        .or_else(|| name.strip_prefix('-'))
        .unwrap_or("");
    safe_name(bare) && !bare.contains('.')
}

fn placeholder(token: &str) -> bool {
    token.starts_with(['<', '[', '{'])
        || (token.bytes().any(|c| c.is_ascii_uppercase())
            && token.bytes().all(|c| !c.is_ascii_lowercase()))
        || matches!(
            token.to_ascii_lowercase().as_str(),
            "string"
                | "strings"
                | "stringarray"
                | "stringslice"
                | "int"
                | "int32"
                | "int64"
                | "uint"
                | "uint32"
                | "uint64"
                | "float"
                | "float64"
                | "duration"
                | "bool"
                | "dir"
                | "directory"
                | "path"
                | "file"
                | "filename"
                | "configdir"
        )
}

fn parse_option(declaration: &str, description: &str, persistent: bool) -> Option<OptionDef> {
    if !declaration.starts_with('-') {
        return None;
    }
    let mut option = OptionDef {
        persistent,
        ..Default::default()
    };
    append_description(&mut option.description, description);
    let (mut rest, mut hint) = (declaration, String::new());
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',', '|', '/']);
        if rest.is_empty() {
            break;
        }
        if rest.starts_with('-') {
            let end = rest
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(c, ',' | '=' | '<' | '|' | '/')
                        || c == '[' && !rest.starts_with("--[no-]")
                })
                .unwrap_or(rest.len());
            let raw = &rest[..end];
            if let Some(suffix) = raw.strip_prefix("--[no-]") {
                if !safe_name(suffix) {
                    return None;
                }
                option
                    .name
                    .extend([format!("--{suffix}"), format!("--no-{suffix}")]);
            } else if valid_flag(raw) {
                option.name.push(raw.into());
            } else {
                return None;
            }
            rest = &rest[end..];
            continue;
        }
        rest = rest.trim_start_matches('=');
        let end = if let Some(close) = rest.chars().next().and_then(|c| match c {
            '<' => Some('>'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        }) {
            rest.find(close).map(|i| i + 1).unwrap_or(rest.len())
        } else {
            rest.find(|c: char| c.is_whitespace() || c == ',')
                .unwrap_or(rest.len())
        };
        let token = &rest[..end];
        if !placeholder(token) && description.is_empty() {
            if description.is_empty() {
                append_description(&mut option.description, rest);
            }
            break;
        }
        let value = token
            .trim_matches(['<', '>', '[', ']', '{', '}', '='])
            .trim_end_matches("...");
        if hint.is_empty() {
            hint.push_str(value);
        }
        if (token.starts_with('{') || token.starts_with('[')) && value.contains(['|', ',']) {
            option.values.extend(
                value
                    .split(['|', ','])
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && s.len() <= 128)
                    .take(128)
                    .map(str::to_owned),
            );
        }
        rest = &rest[end..];
    }
    if option.name.is_empty() {
        return None;
    }
    option.hint = hint;
    Some(option)
}

fn bracket_values(description: &str, marker: &str) -> Vec<String> {
    let lower = description.to_ascii_lowercase();
    let Some(start) = lower.find(marker) else {
        return vec![];
    };
    let rest = &description[start + marker.len()..];
    let Some(end) = rest.find(']') else {
        return vec![];
    };
    rest[..end]
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
        .take(128)
        .collect()
}

fn path_provider(hint: &str, names: &[String], description: &str) -> String {
    let description = description.to_ascii_lowercase();
    if hint.is_empty() || description.contains("container") {
        return String::new();
    }
    let hint = hint.to_ascii_lowercase();
    let words: Vec<&str> = hint
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    if words
        .iter()
        .any(|s| matches!(*s, "dir" | "dirs" | "directory" | "directories") || s.ends_with("dir"))
    {
        return "directories".into();
    }
    if words.iter().any(|s| {
        matches!(*s, "file" | "files" | "filename" | "path" | "paths") || s.ends_with("file")
    }) {
        return "files".into();
    }
    let names = names.join(" ").to_ascii_lowercase();
    if names.contains("directory")
        || names.contains("-dir")
        || names.contains("configdir")
        || description.contains("directory")
    {
        return "directories".into();
    }
    if names.contains("-file")
        || names.contains("-path")
        || description.contains("file")
        || description.contains(" path")
    {
        return "files".into();
    }
    String::new()
}

fn command_rows(declaration: &str, description: &str) -> Vec<Spec> {
    let name_end = declaration
        .find(|c: char| c.is_whitespace() || matches!(c, ',' | '('))
        .unwrap_or(declaration.len());
    // Docker marks installed CLI plugins with one trailing '*'. It is display
    // metadata, not part of the command passed to a help probe.
    let main = declaration[..name_end].strip_suffix('*').unwrap_or(&declaration[..name_end]);
    if !safe_name(main) {
        return vec![];
    }
    let mut names = vec![main.to_owned()];
    let remainder = declaration[name_end..].trim();
    // Free-form prose in a Commands epilog must not become an executable child.
    if !remainder.is_empty()
        && !(remainder.starts_with('(') && remainder.ends_with(')')
            || remainder.starts_with(',')
            || remainder.starts_with("[alias:")
            || remainder.starts_with("[aliases:"))
    {
        return vec![];
    }
    let aliases = remainder.trim_matches(['(', ')', ',']);
    if !aliases.is_empty() && !aliases.starts_with('[') {
        for alias in aliases.split(',').map(str::trim).filter(|s| safe_name(s)) {
            if !names.iter().any(|s| s == alias) {
                names.push(alias.into());
            }
        }
    }
    let text = format!("{declaration} {description}");
    for alias in bracket_values(&text, "[aliases:")
        .into_iter()
        .chain(bracket_values(&text, "[alias:"))
        .filter(|s| safe_name(s))
    {
        if !names.contains(&alias) {
            names.push(alias);
        }
    }
    names
        .into_iter()
        .take(16)
        .map(|name| Spec {
            name,
            description: description.into(),
            ..Default::default()
        })
        .collect()
}

fn parse_help(text: &str, name: &str) -> Option<Spec> {
    let text = clean_help(text)?;
    let mut spec = Spec {
        name: name.into(),
        ..Default::default()
    };
    let (mut section, mut last, mut usage, mut recognized_header, mut possible_values) =
        (Section::None, None, false, false, false);
    let mut positional_choices = Vec::<String>::new();
    let mut choice_indent = 0;
    for line in text.lines().take(8192) {
        if line.len() > 4096 {
            return None;
        }
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("usage:") {
            usage = true;
            section = Section::None;
            last = None;
            possible_values = false;
            continue;
        }
        if lower == "possible values:" && matches!(last, Some(Last::Option(_, row)) if indent > row)
        {
            possible_values = true;
            continue;
        }
        if let Some(heading) = section_heading(trimmed) {
            recognized_header |= matches!(heading, Section::Options(_) | Section::Commands);
            section = heading;
            last = None;
            possible_values = false;
            continue;
        }
        if (trimmed.ends_with(':') && indent <= 2 && !trimmed.starts_with('-'))
            || lower.starts_with("use \"")
            || lower.starts_with("for more information")
            || lower.starts_with("report bugs")
        {
            section = Section::Other;
            last = None;
            continue;
        }
        if possible_values
            && let Some(Last::Option(index, row)) = last
            && indent > row
            && let Some(value) = trimmed.strip_prefix("- ")
        {
            let value = value.split_once(':').map_or(value, |(v, _)| v).trim();
            if !value.is_empty() && value.len() <= 128 && spec.options[index].values.len() < 128 {
                spec.options[index].values.push(value.into());
            }
            continue;
        }
        let (declaration, description) = columns(trimmed);
        let options_allowed = matches!(section, Section::Options(_))
            || matches!(section, Section::None) && usage && indent > 0;
        if options_allowed && trimmed.starts_with('-') && !trimmed.starts_with("- ") {
            if let Some(option) = parse_option(
                declaration,
                description,
                matches!(section, Section::Options(true)),
            ) {
                if spec.options.len() >= MAX_RECORDS {
                    continue;
                }
                let index = spec.options.len();
                spec.options.push(option);
                last = Some(Last::Option(index, indent));
                possible_values = false;
                continue;
            }
        }
        if matches!(section, Section::Positionals)
            && let Some(choices) = declaration
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
        {
            positional_choices = choices
                .split(',')
                .map(str::trim)
                .filter(|s| safe_name(s))
                .take(MAX_RECORDS)
                .map(str::to_owned)
                .collect();
            choice_indent = indent;
            last = None;
            continue;
        }
        let command_allowed = matches!(section, Section::Commands)
            && !matches!(last, Some(Last::Commands(_,_,row)) if indent > row)
            || matches!(section, Section::Positionals)
                && indent > choice_indent
                && !positional_choices.is_empty();
        if command_allowed && indent > 0 {
            let rows = command_rows(declaration, description);
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|s| {
                    !matches!(section, Section::Positionals) || positional_choices.contains(&s.name)
                })
                .collect();
            if !rows.is_empty() && spec.subcommands.len() < MAX_RECORDS {
                let start = spec.subcommands.len();
                spec.subcommands
                    .extend(rows.into_iter().take(MAX_RECORDS - start));
                last = Some(Last::Commands(start, spec.subcommands.len(), indent));
                possible_values = false;
                continue;
            }
        }
        match last {
            Some(Last::Option(index, row)) if indent > row => {
                append_description(&mut spec.options[index].description, trimmed)
            }
            Some(Last::Commands(start, end, row)) if indent > row => {
                for child in &mut spec.subcommands[start..end] {
                    append_description(&mut child.description, trimmed);
                }
            }
            _ if !usage
                && matches!(section, Section::None)
                && spec.description.is_empty()
                && !trimmed.starts_with('<') =>
            {
                append_description(&mut spec.description, trimmed)
            }
            _ => (),
        }
    }
    for option in &mut spec.options {
        option.name.extend(
            bracket_values(&option.description, "[aliases:")
                .into_iter()
                .chain(bracket_values(&option.description, "[alias:"))
                .filter(|s| valid_flag(s)),
        );
        option.name.dedup();
        for value in bracket_values(&option.description, "[possible values:") {
            if !option.values.contains(&value) {
                option.values.push(value);
            }
        }
        option.values.truncate(128);
        if option.values.is_empty() {
            option.provider = path_provider(&option.hint, &option.name, &option.description);
        }
    }
    spec.options.retain(|o| !o.name.is_empty());
    let mut seen = std::collections::HashSet::new();
    spec.subcommands.retain(|s| seen.insert(s.name.clone()));
    ((usage || recognized_header) && (!spec.options.is_empty() || !spec.subcommands.is_empty()))
        .then_some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::OnceLock,
        time::{Duration, Instant},
    };

    // A real executable, not a shell mock. Every run records its argv outside the
    // disposable HOME so tests can prove that no user values were forwarded.
    const FIXTURE: &str = r###"
use std::{env, fs::{self, OpenOptions}, io::{self, Read, Write}, process::{self, Command}, thread, time::Duration};
fn main() {
    let exe = env::current_exe().unwrap().canonicalize().unwrap();
    let mode = exe.file_stem().unwrap().to_str().unwrap();
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--child"] {
        fs::write(exe.with_extension("descendant"), process::id().to_string()).unwrap();
        thread::sleep(Duration::from_millis(1250));
        fs::write(exe.with_extension("survived"), b"not killed").unwrap();
        thread::sleep(Duration::from_secs(30));
        return;
    }
    writeln!(OpenOptions::new().create(true).append(true).open(exe.with_extension("calls")).unwrap(), "{}", args.join(" ")).unwrap();
    fs::write(exe.with_extension("invoked"), env::args().next().unwrap()).unwrap();
    if mode == "timeout" || mode == "orphan" {
        Command::new(&exe).arg("--child").spawn().unwrap();
        if mode == "orphan" { return; }
        thread::sleep(Duration::from_secs(30));
    }
    if mode == "flood" {
        let chunk = [b'x'; 8192];
        loop { io::stdout().write_all(&chunk).unwrap(); io::stderr().write_all(&chunk).unwrap(); }
    }
    if mode == "invalid" { io::stdout().write_all(&[0xff, 0xfe]).unwrap(); return; }
    if mode == "unsupported" { eprintln!("error: unrecognized option '--help'\nUsage: unsupported [OPTIONS]"); process::exit(2); }
    if mode == "errorhelp" { eprintln!("error: unrecognized option '--help'\nUsage: errorhelp [OPTIONS]\n\nOptions:\n  --help  Print help"); process::exit(2); }
    if mode == "empty" { return; }
    if mode == "malformed" { println!("a random response with no help sections"); return; }
    if mode == "isolated" {
        let home = env::var("HOME").unwrap();
        let cwd = env::current_dir().unwrap();
        let mut input = String::new(); io::stdin().read_to_string(&mut input).unwrap();
        assert!(input.is_empty());
        for key in ["LANG", "LC_ALL"] { assert_eq!(env::var(key).unwrap(), "C"); }
        assert_eq!(env::var("TERM").unwrap(), "dumb");
        assert_eq!(env::var("COREPACK_ENABLE_NETWORK").unwrap(), "0");
        for key in ["AWS_ACCESS_KEY_ID", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "SSH_AUTH_SOCK", "NODE_OPTIONS", "PYTHONPATH", "RUSTC_WRAPPER"] { assert!(env::var_os(key).is_none()); }
        assert_eq!(fs::canonicalize(&home).unwrap(), fs::canonicalize(&cwd).unwrap());
        assert_eq!(fs::read_dir(&home).unwrap().filter_map(Result::ok).filter(|p|p.file_name().to_str().unwrap().starts_with('.')).count(), 0);
        fs::write(exe.with_extension("home"), home).unwrap();
    }
    let help = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--help"] => "A previously unknown installed CLI\n\nUsage: nova [OPTIONS] <COMMAND>\n\nCommands:\n  collect  Collect local items [aliases: c]\n\nOptions:\n  -C, --directory <DIRECTORY>  Search directory\n  -f, --file <FILE>            Input file\n  -h, --help                   Print help\n",
        ["collect", "--help"] => "Collect items\n\nUsage: nova collect [OPTIONS] <COMMAND>\n\nCommands:\n  list  List collected items\n\nOptions:\n  -d, --directory <DIR>  Output directory\n  -f, --file <FILE>      Input file\n  --configdir configdir  Configuration directory\n  --mode <MODE>         Select mode\n                        continued description [possible values: fast, slow]\n  -h, --help            Print help\n",
        ["collect", "list", "--help"] => "Usage: nova collect list [OPTIONS]\n\nOptions:\n  --path PATH  Input path\n  --help       Show this message and exit.\n",
        _ => { fs::write(exe.with_extension("executed"), b"unexpected argv").unwrap(); process::exit(9); }
    };
    if mode == "stderr" || mode == "exit1" || mode == "exit2" { eprint!("{help}"); } else { print!("{help}"); }
    if mode == "exit1" { process::exit(1); }
    if mode == "exit2" { process::exit(2); }
}
"###;

    struct Fixture {
        _dir: tempfile::TempDir,
        exe: PathBuf,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            static BINARY: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
            let (_, binary) = BINARY.get_or_init(|| {
                let dir = tempfile::tempdir().unwrap();
                let source = dir.path().join("fixture.rs");
                let binary = dir
                    .path()
                    .join(format!("help-fixture{}", std::env::consts::EXE_SUFFIX));
                fs::write(&source, FIXTURE).unwrap();
                let status =
                    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                        .arg("--edition=2024")
                        .arg(&source)
                        .arg("-o")
                        .arg(&binary)
                        .status()
                        .unwrap();
                assert!(status.success(), "compile the owned help fixture");
                (dir, binary)
            });
            let dir = tempfile::tempdir().unwrap();
            let exe = dir
                .path()
                .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
            fs::copy(binary, &exe).unwrap();
            Self { _dir: dir, exe }
        }
        fn calls(&self) -> String {
            fs::read_to_string(self.exe.with_extension("calls")).unwrap_or_default()
        }
    }
    fn option<'a>(spec: &'a Spec, name: &str) -> &'a OptionDef {
        spec.options
            .iter()
            .find(|o| o.name.iter().any(|n| n == name))
            .unwrap_or_else(|| panic!("missing {name}"))
    }
    fn context(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).into()).collect()
    }

    #[test]
    fn real_unknown_cli_root_and_recognized_nested_help_only() {
        let cli = Fixture::new("nova");
        let cache = Mutex::new(Cache::default());
        let root =
            discover_cached(&cache, &cli.exe, "nova", &[]).expect("discover unknown CLI root");
        assert_eq!(root.name, "nova");
        assert!(root.subcommands.iter().any(|s| s.name == "collect"));
        assert!(root.subcommands.iter().any(|s| s.name == "c"));
        assert_eq!(option(&root, "--directory").provider, "directories");
        assert_eq!(option(&root, "--file").provider, "files");
        assert!(option(&root, "--directory").name.contains(&"-C".into()));
        let recognized = vec![
            root.subcommands
                .iter()
                .find(|s| s.name == "collect")
                .unwrap()
                .name
                .clone(),
        ];
        let child =
            discover_cached(&cache, &cli.exe, "nova", &recognized).expect("discover child options");
        assert_eq!(child.name, "collect");
        assert_eq!(option(&child, "--directory").provider, "directories");
        assert_eq!(option(&child, "--configdir").provider, "directories");
        assert_eq!(option(&child, "--mode").values, ["fast", "slow"]);
        assert!(
            option(&child, "--mode")
                .description
                .contains("continued description")
        );
        let nested =
            discover_cached(&cache, &cli.exe, "nova", &context(&["collect", "list"])).unwrap();
        assert_eq!(option(&nested, "--path").provider, "files");
        assert_eq!(cli.calls(), "--help\ncollect --help\ncollect list --help\n");
        assert!(!cli.calls().contains("typedsecretvalues"));
        assert!(!cli.exe.with_extension("executed").exists());
    }

    #[test]
    fn cache_reuses_help_and_binary_mtime_invalidates_all_contexts() {
        let cli = Fixture::new("nova");
        let cache = Mutex::new(Cache::default());
        for _ in 0..2 {
            assert!(discover_cached(&cache, &cli.exe, "nova", &[]).is_some());
        }
        assert_eq!(cli.calls().lines().count(), 1);
        assert!(discover_cached(&cache, &cli.exe, "nova", &context(&["collect"])).is_some());
        let modified = fs::metadata(&cli.exe).unwrap().modified().unwrap();
        let mut attributes = fs::OpenOptions::new();
        attributes.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // SetFileTime needs attribute access, without opening executable
            // contents for writing. Unix retains the original read-only handle.
            attributes.access_mode(windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES);
        }
        attributes.open(&cli.exe)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified + Duration::from_secs(2)))
            .unwrap();
        assert!(discover_cached(&cache, &cli.exe, "nova", &[]).is_some());
        assert!(discover_cached(&cache, &cli.exe, "nova", &context(&["collect"])).is_some());
        assert_eq!(cli.calls().lines().count(), 4);
    }

    #[test]
    fn timeout_is_bounded_and_negative_result_is_cached() {
        let cli = Fixture::new("timeout");
        let cache = Mutex::new(Cache::default());
        let started = Instant::now();
        assert!(discover_cached(&cache, &cli.exe, "timeout", &[]).is_none());
        assert!(started.elapsed() < Duration::from_millis(1500));
        let started = Instant::now();
        assert!(discover_cached(&cache, &cli.exe, "timeout", &[]).is_none());
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(cli.calls().lines().count(), 1);
    }

    #[test]
    fn timeout_cleans_descendants_including_inherited_output_handles() {
        for mode in ["timeout", "orphan"] {
            let cli = Fixture::new(mode);
            assert!(run_help(&cli.exe, &[]).is_none());
            assert!(cli.exe.with_extension("descendant").exists());
            std::thread::sleep(Duration::from_millis(350));
            assert!(
                !cli.exe.with_extension("survived").exists(),
                "descendant escaped cleanup: {mode}"
            );
            #[cfg(unix)]
            {
                let pid: i32 = fs::read_to_string(cli.exe.with_extension("descendant"))
                    .unwrap()
                    .parse()
                    .unwrap();
                // SAFETY: signal zero only checks the recorded owned fixture PID.
                assert_eq!(
                    unsafe { libc::kill(pid, 0) },
                    -1,
                    "fixture descendant remains alive"
                );
            }
        }
    }

    #[test]
    fn combined_stdout_stderr_limit_does_not_hang() {
        let cli = Fixture::new("flood");
        let started = Instant::now();
        assert!(run_help(&cli.exe, &[]).is_none());
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn stderr_help_and_disposable_home_are_supported() {
        let cli = Fixture::new("stderr");
        let text = run_help(&cli.exe, &[]).expect("help on stderr");
        assert!(parse_help(&text, "stderr").is_some());
        let cli = Fixture::new("isolated");
        assert!(run_help(&cli.exe, &[]).is_some());
        let home = PathBuf::from(fs::read_to_string(cli.exe.with_extension("home")).unwrap());
        assert_ne!(
            home,
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
        );
        assert!(
            !home.exists(),
            "owned HOME should be removed after the child exits"
        );
    }

    #[test]
    fn unsupported_empty_and_malformed_help_fall_back() {
        for mode in ["unsupported", "errorhelp", "empty", "malformed", "invalid"] {
            let cli = Fixture::new(mode);
            assert!(
                discover_cached(&Mutex::new(Cache::default()), &cli.exe, mode, &[]).is_none(),
                "{mode}"
            );
        }
    }

    #[test]
    fn user_options_values_and_deep_or_unsafe_contexts_are_never_run() {
        let cli = Fixture::new("nova");
        for names in [
            context(&["--token=typedsecretvalues"]),
            context(&["$(touch secret)"]),
            context(&["../run"]),
            context(&[".."]),
            context(&["typedsecretvalues argument"]),
            vec!["child".into(); MAX_DEPTH + 1],
        ] {
            assert!(
                discover_cached(&Mutex::new(Cache::default()), &cli.exe, "nova", &names).is_none()
            );
        }
        assert!(cli.calls().is_empty());
        assert!(
            discover(&cli.exe, "nova", &[]).is_none(),
            "project/temp executables are never automatic discovery targets"
        );
        assert!(cli.calls().is_empty());
    }

    #[test]
    fn clap_long_help_values_aliases_and_continuations() {
        let text = "A useful program\n\nUsage: nova [OPTIONS] <FILE>\n\nArguments:\n  <FILE>  A positional input\n\nOptions:\n  -d, --directory <DIRECTORY>\n          Output directory\n          on a second line\n  --format <FORMAT>\n          Select output format\n          Possible values:\n          - json: JSON output\n          - yaml: YAML output\n  --config <FILE>  Configuration [aliases: --config-file]\n  -h, --help  Print help\n\nExamples:\n  --not-an-option  This is an example\n";
        let spec = parse_help(text, "nova").unwrap();
        assert!(spec.subcommands.is_empty());
        assert_eq!(spec.options.len(), 4);
        assert_eq!(option(&spec, "--directory").provider, "directories");
        assert!(
            option(&spec, "--directory")
                .description
                .contains("second line")
        );
        assert_eq!(option(&spec, "--format").values, ["json", "yaml"]);
        assert!(
            option(&spec, "--config")
                .name
                .contains(&"--config-file".into())
        );
    }

    #[test]
    fn cobra_global_flags_and_command_aliases() {
        let text = "Usage:\n  nova [command]\n\nAvailable Commands:\n  collect (c, gather)  Collect data\n  list                List data\n\nFlags:\n  -o, --output string       Output file\n  --configdir configdir     Config directory\n  --verbose                 Enable verbose output\n\nGlobal Flags:\n  --cache-dir string        Cache directory\n\nUse \"nova [command] --help\" for more information about a command.\n";
        let spec = parse_help(text, "nova").unwrap();
        assert_eq!(
            spec.subcommands
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["collect", "c", "gather", "list"]
        );
        assert!(option(&spec, "--cache-dir").persistent);
        assert_eq!(option(&spec, "--cache-dir").provider, "directories");
        assert_eq!(option(&spec, "--output").provider, "files");
        assert!(option(&spec, "--verbose").hint.is_empty());
    }

    #[test]
    fn click_choices_dual_flags_and_indented_examples() {
        let text = "Usage: nova [OPTIONS] COMMAND [ARGS]...\n\nOptions:\n  --color / --no-color  Color output\n  --mode [fast|slow]    Execution mode\n  --directory DIRECTORY  Working directory\n  --file FILENAME      Input file\n  --help               Show this message and exit.\n\nCommands:\n  collect  Collect data.\n\n  Examples:\n    nova collect --not-a-flag\n    fake  Never a subcommand\n";
        let spec = parse_help(text, "nova").unwrap();
        assert_eq!(spec.subcommands.len(), 1);
        assert_eq!(option(&spec, "--mode").values, ["fast", "slow"]);
        assert!(option(&spec, "--color").name.contains(&"--no-color".into()));
        assert_eq!(option(&spec, "--directory").provider, "directories");
        assert_eq!(option(&spec, "--file").provider, "files");
    }

    #[test]
    fn argparse_subparser_rows_are_distinct_from_positional_choices() {
        let text = "usage: nova [-h] [--file FILE] {collect,c,list} ...\n\npositional arguments:\n  {collect,c,list}\n    collect (c)    Collect data\n    list           List data\n  FILE             A positional file\n\noptions:\n  -h, --help       show this help message and exit\n  -f FILE, --file FILE  Input file\n  --dir DIR        Output directory\n";
        let spec = parse_help(text, "nova").unwrap();
        assert_eq!(
            spec.subcommands
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["collect", "c", "list"]
        );
        assert_eq!(option(&spec, "--file").name, ["-f", "--file"]);
        assert_eq!(option(&spec, "--dir").provider, "directories");
        let spec = parse_help("usage: nova [-h] {red,green}\n\npositional arguments:\n  {red,green}  Color to use\n\noptional arguments:\n  -h, --help  Show help\n", "nova").unwrap();
        assert!(spec.subcommands.is_empty());
    }

    #[test]
    fn common_equals_optional_values_ansi_and_boilerplate() {
        let text = "Usage: nova [OPTION]...\n\n  -d, --directory=DIR    Output directory\n  --config[=FILE]        Configuration file\n  --[no-]color           Enable color\n  \x1b[32m--mode <MODE>\x1b[0m         Mode [possible values: fast, slow]\n\nReport bugs to nobody.\nExamples:\n  --fake=DIR             Never a flag\n";
        let spec = parse_help(text, "nova").unwrap();
        assert_eq!(spec.options.len(), 4);
        assert_eq!(option(&spec, "--config").provider, "files");
        assert_eq!(option(&spec, "--directory").provider, "directories");
        assert!(option(&spec, "--color").name.contains(&"--no-color".into()));
        assert_eq!(option(&spec, "--mode").values, ["fast", "slow"]);
    }

    #[test]
    fn malformed_and_oversized_parser_inputs_are_rejected() {
        for text in [
            "",
            "Some text\n  --fake text",
            "<html>Usage: fake --fake</html>",
            "Usage: nova\nExamples:\n  --fake  Example only\n",
            "Usage: nova\0\nOptions:\n --help  Help",
        ] {
            assert!(parse_help(text, "nova").is_none(), "{text:?}");
        }
        assert!(parse_help(&"x".repeat(MAX_OUTPUT + 1), "nova").is_none());
    }

    #[test]
    fn container_value_paths_do_not_request_host_completions() {
        let spec = parse_help("Usage: docker run [OPTIONS]\n\nOptions:\n  --volume list  Bind mount a volume\n  -u, --user string  Username or UID (format:\n                    <name|uid>[:<group|gid>])\n  -w, --workdir string  Working directory inside the\n                       container\n  --config DIR          Local configuration directory\n", "run").unwrap();
        assert_eq!(option(&spec, "--volume").hint, "list");
        assert!(option(&spec, "--workdir").provider.is_empty());
        assert!(!option(&spec, "--workdir").hint.is_empty());
        assert_eq!(option(&spec, "--config").provider, "directories");
    }

    #[test]
    fn installed_path_policy_rejects_projects_symlink_escapes_apps_and_daemons() {
        let cli = Fixture::new("nova");
        let install = tempfile::tempdir().unwrap();
        let installed = install
            .path()
            .join(format!("nova{}", std::env::consts::EXE_SUFFIX));
        fs::copy(&cli.exe, &installed).unwrap();
        let roots = vec![
            install.path().to_owned(),
            install.path().canonical().unwrap(),
        ];
        assert!(validate_in_roots(&installed, &roots, None).is_some());
        assert!(validate_in_roots(&cli.exe, &roots, None).is_none());
        assert!(validate_in_roots(&installed, &roots, Some(install.path())).is_none());
        assert!(validate_in_roots(&installed.with_file_name("../nova"), &roots, None).is_none());
        let app = install.path().join("Agent.app/Contents/MacOS");
        fs::create_dir_all(&app).unwrap();
        let daemon = app.join(format!("nova{}", std::env::consts::EXE_SUFFIX));
        fs::copy(&cli.exe, &daemon).unwrap();
        assert!(validate_in_roots(&daemon, &roots, None).is_none());
        #[cfg(unix)]
        {
            let escaped = install.path().join("escaped");
            std::os::unix::fs::symlink(&cli.exe, &escaped).unwrap();
            assert!(validate_in_roots(&escaped, &roots, None).is_none());
            let alias = install.path().join("alias");
            std::os::unix::fs::symlink(&installed, &alias).unwrap();
            assert!(validate_in_roots(&alias, &roots, None).is_some());
            let daemon = install.path().join("nova-daemon");
            fs::copy(&cli.exe, &daemon).unwrap();
            assert!(validate_in_roots(&daemon, &roots, None).is_none());
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&installed, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(validate_in_roots(&installed, &roots, None).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn original_multicall_symlink_is_executed_and_aliases_have_separate_cache_keys() {
        let cli = Fixture::new("multicall");
        let alias = cli.exe.with_file_name("nova");
        std::os::unix::fs::symlink(&cli.exe, &alias).unwrap();
        let cache = Mutex::new(Cache::default());
        assert!(discover_cached(&cache, &alias, "nova", &[]).is_some());
        assert_eq!(
            PathBuf::from(fs::read_to_string(cli.exe.with_extension("invoked")).unwrap()),
            alias
        );
        assert!(discover_cached(&cache, &cli.exe, "multicall", &[]).is_some());
        assert_eq!(cli.calls().lines().count(), 2);
    }

    #[test]
    fn cache_capacity_and_expired_negative_entries_are_bounded() {
        let cli = Fixture::new("unsupported");
        let key = Key {
            canonical: cli.exe.canonical().unwrap(),
            identity: identity(&cli.exe).unwrap(),
            command: "unsupported".into(),
            context: vec![],
        };
        let mut entries = VecDeque::new();
        for index in 0..MAX_CACHE_ENTRIES {
            let mut key = key.clone();
            key.command = format!("unrelated-{index}");
            entries.push_back(Entry {
                key,
                expires: Instant::now() + NEGATIVE_TTL,
                spec: None,
            });
        }
        let cache = Mutex::new(Cache { entries });
        assert!(discover_cached(&cache, &cli.exe, "unsupported", &[]).is_none());
        {
            let mut locked = cache.lock().unwrap();
            assert_eq!(locked.entries.len(), MAX_CACHE_ENTRIES);
            assert_eq!(locked.entries.front().unwrap().key.command, "unrelated-1");
            let entry = locked.entries.back_mut().unwrap();
            assert!(entry.expires.duration_since(Instant::now()) > Duration::from_secs(4 * 60));
            entry.expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(discover_cached(&cache, &cli.exe, "unsupported", &[]).is_none());
        assert_eq!(cli.calls().lines().count(), 2);
        assert!(cache.lock().unwrap().entries.len() <= MAX_CACHE_ENTRIES);
    }

    #[test]
    fn structured_help_on_exit_one_or_two_is_accepted() {
        for mode in ["exit1", "exit2"] {
            let cli = Fixture::new(mode);
            let spec = discover_cached(&Mutex::default(), &cli.exe, mode, &[])
                .expect("structured help despite exit status");
            assert_eq!(option(&spec, "--directory").provider, "directories");
            assert_eq!(cli.calls(), "--help\n");
        }
    }

    #[cfg(unix)]
    #[test]
    fn docker_desktop_installed_links_allow_cli_but_not_gui_daemon_or_generic_apps() {
        let cli = Fixture::new("docker");
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonical().unwrap();
        let docker_app = base.join("Applications/Docker.app");
        let user_bin = base.join("home/.docker/bin");
        let system_bin = base.join("usr/local/bin");
        let user_plugins = base.join("home/.docker/cli-plugins");
        for path in [&user_bin, &system_bin, &user_plugins] {
            fs::create_dir_all(path).unwrap();
        }
        let roots = vec![user_bin.clone(), system_bin.clone(), user_plugins.clone()];
        for (name, relative, link_root) in [
            ("docker", "Contents/Resources/bin", &user_bin),
            ("docker", "Contents/Resources/bin", &system_bin),
            (
                "docker-compose",
                "Contents/Resources/cli-plugins",
                &user_plugins,
            ),
            (
                "docker-buildx",
                "Contents/Resources/cli-plugins",
                &user_plugins,
            ),
        ] {
            let target = docker_app.join(relative).join(name);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(&cli.exe, &target).unwrap();
            let link = link_root.join(name);
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(
                validate_policy(&link, &roots, None, Some(&docker_app)).is_some(),
                "CLI {target:?}"
            );
            assert!(validate_policy(&target, &roots, None, Some(&docker_app)).is_some());
        }
        for relative in [
            "Contents/MacOS/Docker",
            "Contents/MacOS/docker",
            "Contents/MacOS/com.docker.backend",
            "Contents/Resources/bin/com.docker.backend",
            "Contents/Resources/bin/docker-daemon",
            "Contents/Resources/bin/docker-credential-desktop",
            "Contents/Resources/cli-plugins/arbitrary",
        ] {
            let target = docker_app.join(relative);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::copy(&cli.exe, &target).unwrap();
            let link = user_bin.join("unsafe");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(
                validate_policy(&link, &roots, None, Some(&docker_app)).is_none(),
                "non-CLI {target:?}"
            );
            fs::remove_file(link).unwrap();
        }
        let generic = base.join("Applications/Other.app/Contents/Resources/bin/docker");
        fs::create_dir_all(generic.parent().unwrap()).unwrap();
        fs::copy(&cli.exe, &generic).unwrap();
        let link = user_bin.join("generic");
        std::os::unix::fs::symlink(&generic, &link).unwrap();
        assert!(validate_policy(&link, &roots, None, Some(&docker_app)).is_none());
        let project_link = base.join("project/docker");
        fs::create_dir_all(project_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(
            docker_app.join("Contents/Resources/bin/docker"),
            &project_link,
        )
        .unwrap();
        assert!(validate_policy(&project_link, &roots, None, Some(&docker_app)).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires host-installed macOS CLIs; run explicitly with --ignored --test-threads=1"]
    fn installed_macos_open_and_docker_desktop_help() {
        let spec =
            discover(Path::new("/usr/bin/open"), "open", &[]).expect("macOS open help exits one");
        assert!(
            spec.options
                .iter()
                .any(|o| o.name.iter().any(|n| n == "--args"))
        );
        let cli = Path::new("/usr/local/bin/docker");
        if cli.is_file() {
            let root = discover(cli, "docker", &[]).expect("installed Docker Desktop CLI");
            assert!(!root.options.is_empty());
            assert!(root.subcommands.iter().any(|s| s.name == "run"));
            let child = discover(cli, "docker", &context(&["run"])).expect("Docker run --help");
            assert!(option(&child, "--workdir").provider.is_empty());
            if root.subcommands.iter().any(|s| s.name == "compose") {
                let dir = tempfile::tempdir().unwrap();
                fs::create_dir(dir.path().join("source folder")).unwrap();
                let input = "docker compose --project-directory ";
                let rows = super::super::candidates(input, input.len(), dir.path(), &[cli.to_string_lossy().into_owned()]);
                assert!(rows.iter().any(|r| r.buffer == "docker compose --project-directory 'source folder/'"), "installed Compose path completion: {rows:?}");
                let input = "docker compose --con";
                let rows = super::super::candidates(input, input.len(), dir.path(), &[cli.to_string_lossy().into_owned()]);
                assert!(!rows.iter().any(|r| matches!(r.label.as_str(), "--config" | "--context")), "root-only Docker flags must precede compose: {rows:?}");
            }
        }
    }

    #[test]
    fn actual_clap_rendered_help_includes_singular_aliases_and_long_values() {
        let mut command = clap::Command::new("nova")
            .arg(
                clap::Arg::new("config")
                    .long("config")
                    .visible_alias("config-file")
                    .value_name("FILE")
                    .help("Configuration file"),
            )
            .arg(clap::Arg::new("mode").long("mode").value_parser([
                clap::builder::PossibleValue::new("fast").help("Run quickly"),
                clap::builder::PossibleValue::new("slow").help("Run slowly"),
            ]))
            .subcommand(
                clap::Command::new("collect")
                    .visible_alias("c")
                    .about("Collect items"),
            );
        let text = command.render_long_help().to_string();
        let spec = parse_help(&text, "nova").unwrap();
        assert!(
            option(&spec, "--config")
                .name
                .contains(&"--config-file".into())
        );
        assert_eq!(option(&spec, "--mode").values, ["fast", "slow"]);
        assert!(spec.subcommands.iter().any(|s| s.name == "c"));
    }

    #[test]
    fn command_epilog_prose_and_examples_are_not_children() {
        let spec = parse_help("Usage: nova [command]\n\nCommands:\n  collect  Collect items\n\n  Use nova help for more details\n  Read the manual for more information\n\nExamples:\n  fake     Never a command\n", "nova").unwrap();
        assert_eq!(
            spec.subcommands
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["collect"]
        );
    }

    #[test]
    fn docker_plugin_markers_keep_installed_command_names() {
        let spec = parse_help(
            "Usage: docker [OPTIONS] COMMAND\n\nManagement Commands:\n  buildx*  Docker Buildx\n  compose*    Docker Compose\n  odd**    Invalid wildcard\n  bad*name  Invalid embedded wildcard\n\nCommands:\n  run      Run a command in a new container\n\nGlobal Options:\n  --config string  Client configuration directory\n",
            "docker",
        ).unwrap();
        let names: Vec<_> = spec.subcommands.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"buildx"), "{names:?}");
        assert!(names.contains(&"compose"), "{names:?}");
        assert!(names.contains(&"run"), "{names:?}");
        assert!(!names.iter().any(|name| name.contains('*')));
    }
}
