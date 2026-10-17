//! Windows process name, command line, working folder and foreground descendant.
//! Every query is read-only and returns `None` when access is denied or the process ended.
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    QueryFullProcessImageNameW,
};

struct Process(HANDLE);

impl Process {
    fn open(pid: u32, access: u32) -> Option<Process> {
        // SAFETY: the returned handle is checked and closed by Drop.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Process(handle))
    }

    fn image(&self) -> Option<std::path::PathBuf> {
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` UTF-16 units.
        let ok = unsafe { QueryFullProcessImageNameW(self.0, 0, buf.as_mut_ptr(), &mut len) } != 0;
        ok.then(|| OsString::from_wide(&buf[..len as usize]).into())
    }

    /// Creation time in 100 ns units.
    fn created(&self) -> Option<u64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: all out-pointers are valid FILETIME values.
        let ok = unsafe { GetProcessTimes(self.0, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
        ok.then(|| u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
    }

    fn read(&self, address: usize, out: &mut [u8]) -> Option<()> {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        let mut read = 0usize;
        // SAFETY: `out` is a writable buffer of the requested length.
        let ok = unsafe { ReadProcessMemory(self.0, address as _, out.as_mut_ptr().cast(), out.len(), &mut read) } != 0;
        (ok && read == out.len()).then_some(())
    }

    fn read_usize(&self, address: usize) -> Option<usize> {
        let mut bytes = [0u8; 8];
        self.read(address, &mut bytes)?;
        Some(u64::from_le_bytes(bytes) as usize)
    }

    /// A 64-bit UNICODE_STRING at `address`: length, then the buffer pointer at offset 8.
    fn read_unicode(&self, address: usize) -> Option<String> {
        let mut header = [0u8; 16];
        self.read(address, &mut header)?;
        let len = u16::from_le_bytes([header[0], header[1]]) as usize;
        let buffer = u64::from_le_bytes(header[8..16].try_into().ok()?) as usize;
        if len == 0 || len > 0x8000 || buffer == 0 {
            return None;
        }
        let mut bytes = vec![0u8; len];
        self.read(buffer, &mut bytes)?;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// Whether the process runs with the 64-bit PEB layout read below.
    fn native_layout(&self) -> bool {
        use windows_sys::Win32::System::Threading::IsWow64Process2;
        let (mut process, mut machine) = (0u16, 0u16);
        // SAFETY: both out-pointers are valid.
        let ok = unsafe { IsWow64Process2(self.0, &mut process, &mut machine) } != 0;
        // IMAGE_FILE_MACHINE_UNKNOWN (0) means the process is not under WOW64.
        ok && process == 0 && cfg!(target_pointer_width = "64")
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// The executable's file name without `.exe`.
pub fn name(pid: u32) -> Option<String> {
    let image = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?.image()?;
    Some(image.file_stem()?.to_string_lossy().into_owned())
}

/// Whether `pid` runs a window (GUI-subsystem) application rather than a console program.
pub fn is_window_application(pid: u32) -> bool {
    Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).and_then(|p| p.image()).and_then(|image| image_subsystem(&image)) == Some(2)
}

/// The `Subsystem` field of an executable's PE optional header: 2 for a window application,
/// 3 for a console program.
fn image_subsystem(path: &std::path::Path) -> Option<u16> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos).ok()?;
    if &dos[..2] != b"MZ" {
        return None;
    }
    let pe = u64::from(u32::from_le_bytes(dos[60..64].try_into().ok()?));
    // The 4-byte signature and the 20-byte file header precede the optional header, whose
    // Subsystem field is at offset 68 in both PE32 and PE32+.
    let mut header = [0u8; 24 + 70];
    file.seek(SeekFrom::Start(pe)).ok()?;
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([header[24 + 68], header[24 + 69]]))
}

/// The process's arguments, split as `CommandLineToArgvW` splits them.
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
    let process = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut needed = 0u32;
    // SAFETY: a zero-length query only reports the required size.
    unsafe { NtQueryInformationProcess(process.0, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 || needed > 0x10_0000 {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes, aligned for UNICODE_STRING.
    let status = unsafe { NtQueryInformationProcess(process.0, ProcessCommandLineInformation, buf.as_mut_ptr().cast(), needed, &mut needed) };
    if status < 0 {
        return None;
    }
    // SAFETY: on success the buffer starts with a UNICODE_STRING whose text follows in the same buffer.
    let line = unsafe {
        let text = &*(buf.as_ptr() as *const windows_sys::Win32::Foundation::UNICODE_STRING);
        if text.Buffer.is_null() {
            return None;
        }
        let mut units = std::slice::from_raw_parts(text.Buffer, text.Length as usize / 2).to_vec();
        units.push(0);
        units
    };
    let mut argc = 0i32;
    // SAFETY: `line` is NUL-terminated; the returned array is freed with LocalFree.
    unsafe {
        let argv = CommandLineToArgvW(line.as_ptr(), &mut argc);
        if argv.is_null() {
            return None;
        }
        let mut args = Vec::with_capacity(argc as usize);
        for i in 0..argc as usize {
            let arg = *argv.add(i);
            let len = (0..).take_while(|&n| *arg.add(n) != 0).count();
            args.push(String::from_utf16_lossy(std::slice::from_raw_parts(arg, len)));
        }
        windows_sys::Win32::Foundation::LocalFree(argv.cast());
        (!args.is_empty()).then_some(args)
    }
}

/// The process's current folder from its PEB (`ProcessParameters->CurrentDirectory`).
pub fn cwd(pid: u32) -> Option<String> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::System::Threading::PROCESS_BASIC_INFORMATION;
    // 64-bit offsets: PEB.ProcessParameters and RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath.
    const PROCESS_PARAMETERS: usize = 0x20;
    const CURRENT_DIRECTORY: usize = 0x38;
    let process = Process::open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    if !process.native_layout() {
        return None;
    }
    // SAFETY: an all-zero PROCESS_BASIC_INFORMATION is valid and sized for the query.
    let mut info: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let mut len = 0u32;
    // SAFETY: `info` matches the requested information class.
    let status = unsafe {
        NtQueryInformationProcess(process.0, ProcessBasicInformation, (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(), std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32, &mut len)
    };
    if status < 0 || info.PebBaseAddress.is_null() {
        return None;
    }
    let parameters = process.read_usize(info.PebBaseAddress as usize + PROCESS_PARAMETERS)?;
    let dir = process.read_unicode(parameters + CURRENT_DIRECTORY)?;
    let dir = dir.trim_end_matches('\\');
    Some(if dir.ends_with(':') { format!("{dir}\\") } else { dir.to_string() })
}

struct Entry {
    pid: u32,
    parent: u32,
    name: String,
}

fn snapshot() -> Vec<Entry> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is checked and closed; the entry size is set before use.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            out.push(Entry { pid: entry.th32ProcessID, parent: entry.th32ParentProcessID, name: String::from_utf16_lossy(&entry.szExeFile[..len]) });
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// The parent process ID recorded for `pid`, when that parent is older than `pid` (a newer
/// process with that ID is an unrelated reuse).
pub fn parent_of(pid: u32) -> Option<u32> {
    let parent = snapshot().into_iter().find(|e| e.pid == pid)?.parent;
    let created = |pid: u32| Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).and_then(|p| p.created());
    (parent != 0 && created(parent)? <= created(pid)?).then_some(parent)
}

fn is_agent(pid: u32) -> bool {
    matches!(super::display_name(pid).map(|n| n.to_ascii_lowercase()).as_deref(), Some("codex" | "claude"))
}

/// The process in the foreground of the console whose shell is `shell`: the single agent
/// (`codex`, `claude`) among its descendants, otherwise its newest descendant followed down
/// to the newest leaf, otherwise the shell itself. Console hosts and window applications (with
/// what they start) are ignored, and a child older than its recorded parent is a reused
/// process ID and is ignored too.
pub fn foreground_of(shell: u32) -> Option<u32> {
    let procs = snapshot();
    let created = |pid: u32| Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).and_then(|p| p.created());
    let shell_created = created(shell)?;
    let mut descendants: Vec<(u32, u64)> = Vec::new();
    let mut frontier = vec![(shell, shell_created)];
    while let Some((parent, parent_created)) = frontier.pop() {
        for entry in procs.iter().filter(|e| e.parent == parent && e.pid != parent) {
            let lower = entry.name.to_ascii_lowercase();
            if lower == "conhost.exe" || lower == "openconsole.exe" || is_window_application(entry.pid) {
                continue;
            }
            let Some(when) = created(entry.pid).filter(|&t| t >= parent_created) else { continue };
            if descendants.len() < 256 {
                descendants.push((entry.pid, when));
                frontier.push((entry.pid, when));
            }
        }
    }
    let agents: Vec<u32> = descendants.iter().map(|&(pid, _)| pid).filter(|&pid| is_agent(pid)).collect();
    if agents.len() == 1 {
        return Some(agents[0]);
    }
    let parents: std::collections::HashMap<u32, u32> = procs.iter().map(|e| (e.pid, e.parent)).collect();
    let mut current = (shell, shell_created);
    loop {
        let newest = descendants.iter().filter(|(pid, _)| parents.get(pid) == Some(&current.0)).max_by_key(|(_, when)| *when).copied();
        match newest {
            Some(next) => current = next,
            None => return Some(current.0),
        }
    }
}
