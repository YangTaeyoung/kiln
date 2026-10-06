use crate::*;
use std::{
    io::{Read, Write},
    net::ToSocketAddrs,
};
use suppaftp::{NativeTlsConnector, NativeTlsFtpStream, native_tls::TlsConnector};
fn path(value: &str) -> Result<&str> {
    safe_text(value)?;
    if value.is_empty() {
        return Ok("/");
    }
    Ok(value)
}
fn copy(mut reader: impl Read, mut writer: impl Write, c: &Control) -> Result<()> {
    let mut buf = [0; 64 * 1024];
    loop {
        c.check()?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        c.advance(n as u64);
    }
    writer.flush()?;
    Ok(())
}
fn directory(ftp: &mut NativeTlsFtpStream, path: &str) -> Result<Vec<RemoteEntry>> {
    match ftp.mlsd(Some(path)) {
        Ok(lines) => mlsd(lines, path),
        Err(_) => parsed(ftp.list(Some(path))?, path),
    }
}
fn entry(ftp: &mut NativeTlsFtpStream, path: &str) -> Result<Option<RemoteEntry>> {
    let path = path.trim_end_matches('/');
    let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok(directory(ftp, parent)?.into_iter().find(|e| e.name == name))
}
fn exists(ftp: &mut NativeTlsFtpStream, path: &str) -> Result<bool> {
    Ok(entry(ftp, path)?.is_some())
}
fn no_replace(ftp: &mut NativeTlsFtpStream, path: &str, overwrite: bool) -> Result<()> {
    if !overwrite && exists(ftp, path)? {
        bail!("Destination already exists; confirm replacement")
    };
    Ok(())
}
fn parsed(lines: Vec<String>, directory: &str) -> Result<Vec<RemoteEntry>> {
    let mut entries = Vec::new();
    for line in lines {
        if line.starts_with("total ") {
            continue;
        }
        let f = suppaftp::list::ListParser::parse_posix(&line)
            .or_else(|_| suppaftp::list::ListParser::parse_dos(&line))
            .context("FTP server returned an unsupported directory listing")?;
        entries.push(RemoteEntry {
            name: f.name().into(),
            path: join(directory, f.name()),
            is_dir: f.is_directory(),
            size: f.size() as u64,
            modified: Some(modified_time(f.modified())),
        });
    }
    Ok(sorted(entries))
}
fn mlsd(lines: Vec<String>, directory: &str) -> Result<Vec<RemoteEntry>> {
    let mut entries = Vec::new();
    for line in lines {
        let (facts, name) = line.split_once(' ').context("Invalid FTP MLSD response")?;
        let name = name.trim_start();
        if name.is_empty() {
            continue;
        }
        let mut is_dir = false;
        let mut size = 0;
        let mut modified = None;
        let mut skip = false;
        for fact in facts.split(';') {
            if let Some((key, value)) = fact.split_once('=') {
                match key.to_lowercase().as_str() {
                    "type" => {
                        is_dir = value.eq_ignore_ascii_case("dir");
                        skip =
                            value.eq_ignore_ascii_case("cdir") || value.eq_ignore_ascii_case("pdir")
                    }
                    "size" => size = value.parse().unwrap_or(0),
                    "modify" => {
                        modified = chrono::NaiveDateTime::parse_from_str(
                            value.split('.').next().unwrap_or(value),
                            "%Y%m%d%H%M%S",
                        )
                        .ok()
                        .map(|v| v.format("%Y-%m-%d %H:%M UTC").to_string())
                    }
                    _ => {}
                }
            }
        }
        if !skip {
            entries.push(RemoteEntry {
                name: name.into(),
                path: join(directory, name),
                is_dir,
                size,
                modified,
            });
        }
    }
    Ok(sorted(entries))
}
pub fn run(
    profile: &ConnectionProfile,
    secrets: &Secrets,
    op: &Operation,
    c: &Control,
) -> Result<RemoteResult> {
    let RemoteEndpoint::Ftp {
        host,
        port,
        username,
        tls,
        ..
    } = &profile.endpoint
    else {
        unreachable!()
    };
    let addrs = (host.as_str(), *port)
        .to_socket_addrs()?
        .collect::<Vec<_>>();
    let mut last = None;
    let mut connected = None;
    for addr in addrs {
        c.check()?;
        match NativeTlsFtpStream::connect_timeout(addr, Duration::from_secs(15)) {
            Ok(s) => {
                connected = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut ftp = connected.context(
        last.map(|e| e.to_string())
            .unwrap_or_else(|| "Host did not resolve".into()),
    )?;
    ftp.get_ref()
        .set_read_timeout(Some(Duration::from_secs(30)))?;
    ftp.get_ref()
        .set_write_timeout(Some(Duration::from_secs(30)))?;
    if *tls {
        ftp = ftp.into_secure(NativeTlsConnector::from(TlsConnector::new()?), host)?;
    }
    ftp.login(username.as_str(), secrets.password.as_deref().unwrap_or(""))?;
    ftp.transfer_type(suppaftp::types::FileType::Binary)?;
    c.check()?;
    let result = match op {
        Operation::List { path: current, .. } => {
            let current = path(current)?;
            let entries = directory(&mut ftp, current)?;
            Ok(RemoteResult::Listed(ListPage {
                entries,
                next_cursor: None,
            }))
        }
        Operation::Stat { path: p } => {
            let p = path(p)?;
            Ok(RemoteResult::Entry(
                entry(&mut ftp, p)?.context("Remote file does not exist")?,
            ))
        }
        Operation::Download { path: p, local } => {
            let p = path(p)?;
            if let Ok(n) = ftp.size(p) {
                c.total(n as u64)
            }
            let mut temp = destination_temp(local)?;
            let mut stream = ftp.retr_as_stream(p)?;
            stream
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(30)))?;
            let transfer = copy(&mut stream, &mut temp, c);
            let finalized = ftp.finalize_retr_stream(stream);
            transfer?;
            finalized?;
            temp.as_file().sync_all()?;
            c.check()?;
            temp.persist(local).map_err(|e| e.error)?;
            Ok(RemoteResult::Done)
        }
        Operation::Upload {
            local,
            path: p,
            overwrite,
        } => {
            let p = path(p)?;
            no_replace(&mut ftp, p, *overwrite)?;
            let file = std::fs::File::open(local)?;
            c.total(file.metadata()?.len());
            let staged = temporary_remote(p);
            let mut stream = ftp.put_with_stream(&staged)?;
            stream
                .get_ref()
                .set_write_timeout(Some(Duration::from_secs(30)))?;
            let transfer = (|| {
                let transfer = copy(file, &mut stream, c);
                let finalized = ftp.finalize_put_stream(stream);
                transfer?;
                finalized?;
                c.check()?;
                no_replace(&mut ftp, p, *overwrite)?;
                ftp.rename(staged.as_str(), p)?;
                Ok::<_, anyhow::Error>(())
            })();
            if transfer.is_err() {
                let _ = ftp.rm(&staged);
            }
            transfer?;
            Ok(RemoteResult::Done)
        }
        Operation::Rename {
            from,
            to,
            overwrite,
        } => {
            let from = path(from)?;
            let to = path(to)?;
            no_replace(&mut ftp, to, *overwrite)?;
            c.check()?;
            ftp.rename(from, to)?;
            Ok(RemoteResult::Done)
        }
        Operation::Delete { path: p, is_dir } => {
            let p = path(p)?;
            if matches!(p, "/" | "." | "..") {
                bail!("Cannot delete the root directory")
            };
            c.check()?;
            if *is_dir {
                ftp.rmdir(p)?
            } else {
                ftp.rm(p)?
            };
            Ok(RemoteResult::Done)
        }
        Operation::CreateDir { path: p } => {
            c.check()?;
            ftp.mkdir(path(p)?)?;
            Ok(RemoteResult::Done)
        }
    };
    let _ = ftp.quit();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facts_preserve_space_and_unicode() {
        let entries = mlsd(
            vec![
                "type=dir;modify=20251005001000; 한글 폴더".into(),
                "type=file;size=17; notes with spaces.txt".into(),
                "type=pdir; ..".into(),
            ],
            "/",
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 17);
        assert_eq!(entries[1].name, "notes with spaces.txt");
    }
}
