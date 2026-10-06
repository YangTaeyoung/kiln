use crate::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};
/// OpenSSH's makeargv protects glob metacharacters inside quotes itself.
/// Only quote/backslash delimiters are escaped here; explicitly escaping '['
/// inside quotes double-escapes it and addresses a different remote filename.
fn quoted(path: &str) -> Result<String> {
    safe_text(path)?;
    let mut q = String::from("\"");
    for ch in path.chars() {
        if matches!(ch, '\\' | '"') {
            q.push('\\')
        }
        q.push(ch)
    }
    q.push('"');
    Ok(q)
}
fn glob_quoted(path: &str) -> Result<String> {
    quoted(path)
}
fn command(profile: &ConnectionProfile) -> Result<Command> {
    let RemoteEndpoint::Sftp {
        alias, config_path, ..
    } = &profile.endpoint
    else {
        unreachable!()
    };
    ssh_config::validate_alias(alias)?;
    let mut cmd = Command::new("/usr/bin/sftp");
    cmd.args([
        "-q",
        "-b",
        "-",
        "-oBatchMode=yes",
        "-oStrictHostKeyChecking=yes",
        "-oConnectTimeout=15",
        "-oConnectionAttempts=1",
        "-oServerAliveInterval=15",
        "-oServerAliveCountMax=2",
        "-oPermitLocalCommand=no",
        "-oClearAllForwardings=yes",
    ]);
    if let Some(config) = config_path {
        cmd.arg("-F").arg(config);
    }
    cmd.arg("--").arg(alias);
    Ok(cmd)
}
fn batch(
    profile: &ConnectionProfile,
    text: &str,
    c: &Control,
    progress_file: Option<&Path>,
) -> Result<String> {
    c.check()?;
    let mut stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    let mut cmd = command(profile)?;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    let mut child = cmd.spawn().context("Cannot start OpenSSH sftp")?;
    let send = child
        .stdin
        .take()
        .context("Cannot write SFTP batch")?
        .write_all(text.as_bytes());
    if let Err(e) = send {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.into());
    }
    let status = loop {
        if let Err(e) = c.check() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        if let Some(path) = progress_file {
            if let Ok(meta) = std::fs::metadata(path) {
                c.bytes.store(meta.len(), Ordering::Relaxed);
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    use std::io::{Read, Seek};
    let mut err = stderr;
    err.rewind()?;
    let mut errors = Vec::new();
    err.take(64 * 1024).read_to_end(&mut errors)?;
    if !status.success() {
        bail!("SFTP: {}", String::from_utf8_lossy(&errors).trim())
    };
    stdout.rewind()?;
    let mut out = Vec::new();
    stdout.take(16 * 1024 * 1024).read_to_end(&mut out)?;
    if text.starts_with("@ls ") {
        String::from_utf8(out).context("Remote filenames are not valid UTF-8")
    } else {
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}
fn listing(profile: &ConnectionProfile, path: &str, c: &Control) -> Result<Vec<RemoteEntry>> {
    let out = batch(
        profile,
        &format!("@ls -lan {}\n", glob_quoted(path)?),
        c,
        None,
    )?;
    let mut entries = Vec::new();
    for line in out.lines() {
        if line.is_empty()
            || line.starts_with("sftp>")
            || line.starts_with("Connected to ")
            || line.starts_with("total ")
        {
            continue;
        };
        let f = parse_listing(line)?;
        let name = leaf(f.name());
        entries.push(RemoteEntry {
            name: name.clone(),
            path: join(path, &name),
            is_dir: f.is_directory(),
            size: f.size() as u64,
            modified: Some(modified_time(f.modified())),
        });
    }
    Ok(sorted(entries))
}
fn find(profile: &ConnectionProfile, path: &str, c: &Control) -> Result<Option<RemoteEntry>> {
    let path = path.trim_end_matches('/');
    let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok(listing(profile, parent, c)?
        .into_iter()
        .find(|v| v.name == name))
}
fn no_replace(profile: &ConnectionProfile, path: &str, overwrite: bool, c: &Control) -> Result<()> {
    if !overwrite && find(profile, path, c)?.is_some() {
        bail!("Destination already exists; confirm replacement")
    };
    Ok(())
}
pub fn run(profile: &ConnectionProfile, op: &Operation, c: &Control) -> Result<RemoteResult> {
    match op {
        Operation::List { path, .. } => Ok(RemoteResult::Listed(ListPage {
            entries: listing(profile, if path.is_empty() { "." } else { path }, c)?,
            next_cursor: None,
        })),
        Operation::Stat { path } => Ok(RemoteResult::Entry(
            find(profile, path, c)?.context("Remote file does not exist")?,
        )),
        Operation::Download { path, local } => {
            let entry = find(profile, path, c)?.context("Remote file does not exist")?;
            if entry.is_dir {
                bail!("Select a file to download")
            };
            c.total(entry.size);
            let temp = destination_temp(local)?;
            let tmp_path = temp.path().to_str().context("Local path is not UTF-8")?;
            batch(
                profile,
                &format!("@get {} {}\n", glob_quoted(path)?, quoted(tmp_path)?),
                c,
                Some(temp.path()),
            )?;
            c.check()?;
            temp.as_file().sync_all()?;
            temp.persist(local).map_err(|e| e.error)?;
            Ok(RemoteResult::Done)
        }
        Operation::Upload {
            local,
            path,
            overwrite,
        } => {
            no_replace(profile, path, *overwrite, c)?;
            let meta = std::fs::metadata(local)?;
            if !meta.is_file() {
                bail!("Select a regular file to upload")
            };
            c.total(meta.len());
            let staged = temporary_remote(path);
            let local = local.to_str().context("Local path is not UTF-8")?;
            let transfer = (|| {
                batch(
                    profile,
                    &format!("@put {} {}\n", quoted(local)?, quoted(&staged)?),
                    c,
                    None,
                )?;
                c.bytes.store(meta.len(), Ordering::Relaxed);
                c.check()?;
                no_replace(profile, path, *overwrite, c)?;
                batch(
                    profile,
                    &format!("@rename {} {}\n", quoted(&staged)?, quoted(path)?),
                    c,
                    None,
                )?;
                Ok::<_, anyhow::Error>(())
            })();
            if transfer.is_err() {
                let _ = batch(
                    profile,
                    &format!("-@rm {}\n", glob_quoted(&staged)?),
                    &Control::default(),
                    None,
                );
            }
            transfer?;
            Ok(RemoteResult::Done)
        }
        Operation::Rename {
            from,
            to,
            overwrite,
        } => {
            if from == to {
                return Ok(RemoteResult::Done);
            };
            no_replace(profile, to, *overwrite, c)?;
            batch(
                profile,
                &format!("@rename {} {}\n", quoted(from)?, quoted(to)?),
                c,
                None,
            )?;
            Ok(RemoteResult::Done)
        }
        Operation::Delete { path, is_dir } => {
            if matches!(path.as_str(), "" | "/" | "." | "..") {
                bail!("Cannot delete the root directory")
            };
            batch(
                profile,
                &format!(
                    "@{} {}\n",
                    if *is_dir { "rmdir" } else { "rm" },
                    if *is_dir {
                        quoted(path)?
                    } else {
                        glob_quoted(path)?
                    }
                ),
                c,
                None,
            )?;
            Ok(RemoteResult::Done)
        }
        Operation::CreateDir { path } => {
            batch(profile, &format!("@mkdir {}\n", quoted(path)?), c, None)?;
            Ok(RemoteResult::Done)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_batch_paths_cannot_inject() {
        assert_eq!(glob_quoted("한글 a*[x]?").unwrap(), "\"한글 a*[x]?\"");
        assert!(quoted("a\n!rm").is_err());
    }
    #[test]
    fn command_never_uses_shell_or_disables_host_check() {
        let p = ConnectionProfile {
            id: "a".into(),
            name: "fixture".into(),
            endpoint: RemoteEndpoint::Sftp {
                alias: "fixture".into(),
                config_path: None,
                root: ".".into(),
            },
        };
        let c = command(&p).unwrap();
        assert_eq!(c.get_program(), "/usr/bin/sftp");
        let args = c
            .get_args()
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.contains(&"-oStrictHostKeyChecking=yes".into()));
        assert!(args.contains(&"-oPermitLocalCommand=no".into()));
        assert_eq!(args.last().unwrap(), "fixture");
    }
}

fn parse_listing(line: &str) -> Result<suppaftp::list::File> {
    // OpenSSH uses '?' for the unsupported hard-link count. Normalize only
    // that field, then require an actual POSIX metadata parse (no MLSD fallback).
    let split = line
        .find(char::is_whitespace)
        .context("Invalid SFTP listing")?;
    let rest = line[split..].trim_start();
    let end = rest
        .find(char::is_whitespace)
        .context("Invalid SFTP listing")?;
    let normalized = if &rest[..end] == "?" {
        format!("{} 1 {}", &line[..split], rest[end..].trim_start())
    } else {
        line.to_owned()
    };
    suppaftp::list::ListParser::parse_posix(&normalized)
        .context("SFTP server returned an unsupported directory listing")
}
#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn openssh_unknown_linkcount_keeps_size_and_timestamp() {
        let f=parse_listing("-rw-r--r--    ? 1001     100            26 Oct  6 17:26 /files/한글 [literal] with spaces.txt").unwrap();
        assert_eq!(f.size(), 26);
        assert!(f.modified() > std::time::UNIX_EPOCH);
        assert_eq!(leaf(f.name()), "한글 [literal] with spaces.txt");
    }
}
