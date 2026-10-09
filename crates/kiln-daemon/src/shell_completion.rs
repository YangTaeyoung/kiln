//! Ephemeral completion capability. No shell text is evaluated or persisted.
use kiln_proto::ShellCompletion;
use std::path::Path;
fn decode(value: &str) -> Option<String> {
    if value.len() > 8192 || value.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<_>> = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(value.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}
pub fn parse(payload: &str) -> Option<ShellCompletion> {
    if payload.len() > 8192 {
        return None;
    }
    let mut fields = payload.split(';');
    let revision = fields.next()?.parse().ok()?;
    let cursor = fields.next()?.parse().ok()?;
    let buffer = decode(fields.next()?)?;
    let cwd = decode(fields.next()?)?;
    let request_file = decode(fields.next()?)?;
    let commands = decode(fields.next().unwrap_or(""))?
        .lines()
        .take(80)
        .map(str::to_owned)
        .collect();
    let explicit = fields.next().unwrap_or("1") == "1";
    if fields.next().is_some()
        || buffer.len() > 4096
        || buffer.contains(['\0', '\r', '\n'])
        || cursor > buffer.chars().count()
        || !Path::new(&cwd).is_absolute()
    {
        return None;
    }
    Some(ShellCompletion {
        explicit,
        revision,
        cursor,
        buffer,
        cwd,
        request_file,
        commands,
    })
}
#[cfg(unix)]
pub fn stage(request: &ShellCompletion, buffer: &str, cursor: usize) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid shell completion request",
        )
    };
    if buffer.len() > 4096 || buffer.contains(['\0', '\n', '\r']) || cursor > buffer.chars().count()
    {
        return Err(invalid());
    }
    let path = Path::new(&request.request_file);
    let dir = path.parent().ok_or_else(invalid)?;
    if path.file_name().and_then(|n| n.to_str()) != Some("request")
        || !dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("kiln-completion."))
        || dir.parent().and_then(|p| p.canonicalize().ok())
            != std::env::temp_dir().canonicalize().ok()
    {
        return Err(invalid());
    }
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_uid != unsafe { libc::geteuid() } || stat.st_mode & 0o077 != 0 {
        return Err(invalid());
    }
    let name =
        std::ffi::CString::new(path.file_name().unwrap().as_bytes()).map_err(|_| invalid())?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let hex: String = buffer.bytes().map(|b| format!("{b:02x}")).collect();
    let result =
        write!(file, "{}\n{cursor}\n{hex}\n", request.revision).and_then(|_| file.sync_all());
    if result.is_err() {
        unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    }
    result
}
#[cfg(not(unix))]
pub fn stage(_: &ShellCompletion, _: &str, _: usize) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Zsh completion requires Unix",
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_request_is_not_a_command() {
        assert!(parse("1;0;zz;2f;2f;").is_none());
        assert!(parse("1;9;61;2f;2f;").is_none());
        assert!(parse("1;1;61;2f;2f;").is_some());
    }
    #[test]
    #[cfg(unix)]
    fn private_request_is_exclusive_and_rejects_arbitrary_paths() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::Builder::new()
            .prefix("kiln-completion.")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut r = ShellCompletion {
            explicit: true,
            revision: 7,
            cursor: 0,
            buffer: String::new(),
            cwd: "/".into(),
            request_file: d.path().join("request").to_string_lossy().into_owned(),
            commands: vec![],
        };
        stage(&r, "cd '한 글'", 8).unwrap();
        assert!(stage(&r, "evil", 4).is_err());
        let saved = std::fs::read_to_string(&r.request_file).unwrap();
        assert!(saved.starts_with("7\n8\n"));
        r.request_file = d.path().join("other").to_string_lossy().into_owned();
        assert!(stage(&r, "x", 1).is_err());
    }
}

#[cfg(all(test, unix))]
mod pty_tests {
    use super::*;
    use std::io::Write;
    fn request(reader: &mut crate::pty::PtyReader) -> ShellCompletion {
        request_mode(reader, true, None)
    }
    fn request_mode(
        reader: &mut crate::pty::PtyReader,
        explicit: bool,
        expected: Option<&str>,
    ) -> ShellCompletion {
        let mut scanner = crate::osc::OscScanner::default();
        let mut bytes = [0; 16384];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            if let crate::pty::ReadResult::Data(n) = reader.read_timeout(&mut bytes, 100).unwrap() {
                let mut events = vec![];
                scanner.feed(&bytes[..n], &mut events);
                for e in events {
                    if let crate::osc::OscEvent::Completion(r) = e {
                        if r.explicit == explicit && expected.is_none_or(|value| r.buffer == value)
                        {
                            return r;
                        }
                    }
                }
            }
        }
        panic!("ZLE completion request missing");
    }
    #[test]
    fn real_zle_accepts_unicode_without_execution_rejects_stale_and_preserves_tab() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("integration.zsh");
        std::fs::write(&script, include_str!("shell-integration.zsh")).unwrap();
        let pty = crate::pty::Pty::spawn(
            &kiln_proto::SpawnSpec {
                program: Some("/bin/zsh".into()),
                args: vec!["-f".into()],
                cwd: Some(temp.path().to_string_lossy().into_owned()),
                env: vec![("ZDOTDIR".into(), temp.path().to_string_lossy().into_owned())],
                cols: 80,
                rows: 24,
                ..Default::default()
            },
            992,
        )
        .unwrap();
        struct Guard(crate::pty::Pty);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.kill();
            }
        }
        let guard = Guard(pty);
        let mut reader = guard.0.reader().unwrap();
        let mut writer = guard.0.writer().unwrap();
        write!(writer,"source '{}' ; _fixture_tab() {{ LBUFFER+='native-tab'; }}; zle -N _fixture_tab; bindkey '^I' _fixture_tab\r",script.display()).unwrap();
        writer.write_all(b"\0").unwrap();
        let empty = request(&mut reader);
        assert_eq!(empty.buffer, "");
        stage(&empty, "cd '한 글'", 8).unwrap();
        writer.write_all(b"\x1b[99;1~\0").unwrap();
        let filled = request(&mut reader);
        assert_eq!(filled.buffer, "cd '한 글'");
        assert_eq!(filled.cursor, 8);
        assert_eq!(
            std::path::Path::new(&filled.cwd).canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
        // The shell revalidates the saved buffer/cursor even if a stale GUI tries acceptance.
        writer.write_all(b"x").unwrap();
        stage(&filled, "touch should-not-exist", 21).unwrap();
        writer.write_all(b"\x1b[99;1~\0").unwrap();
        let stale = request(&mut reader);
        assert_eq!(stale.buffer, "cd '한 글'x");
        assert!(!temp.path().join("should-not-exist").exists());
        writer.write_all(b"\x15\t\0").unwrap();
        let native = request(&mut reader);
        assert_eq!(native.buffer, "native-tab");
        writer.write_all(b"\x15git st").unwrap();
        let preview = request_mode(&mut reader, false, Some("git st"));
        assert_eq!(preview.buffer, "git st");
        writer.write_all(b"\x15exit\r").unwrap();
    }
}

/// Strip private edit metadata before VTE (whose unknown-OSC debug log prints payloads).
/// The control scanner consumes the original stream separately.
#[derive(Default)]
pub struct PrivateOscFilter {
    prefix: Vec<u8>,
    private: bool,
    escape: bool,
}
impl PrivateOscFilter {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        const PREFIX: &[u8] = b"\x1b]777;kiln-complete";
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            if self.private {
                if byte == 7 || (self.escape && byte == b'\\') {
                    self.private = false;
                    self.escape = false;
                } else {
                    self.escape = byte == 27;
                }
                continue;
            }
            if byte == PREFIX[self.prefix.len()] {
                self.prefix.push(byte);
                if self.prefix.len() == PREFIX.len() {
                    self.prefix.clear();
                    self.private = true;
                }
                continue;
            }
            output.extend_from_slice(&self.prefix);
            self.prefix.clear();
            if byte == 27 {
                self.prefix.push(byte);
            } else {
                output.push(byte);
            }
        }
        output
    }
}
#[cfg(test)]
mod filter_tests {
    use super::*;
    #[test]
    fn private_payload_never_reaches_vte_across_any_boundary() {
        let original=b"hello\x1b[31mred\x1b]777;notify;a;b\x07!\x1b]777;kiln-complete;private-buffer\x07next\x1b]777;kiln-complete-cancel\x1b\\end";
        let expected = b"hello\x1b[31mred\x1b]777;notify;a;b\x07!nextend";
        for size in 1..original.len() {
            let mut f = PrivateOscFilter::default();
            let output: Vec<_> = original
                .chunks(size)
                .flat_map(|chunk| f.feed(chunk))
                .collect();
            assert_eq!(output, expected);
        }
    }
}
