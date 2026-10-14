//! Codex identifies both native storage representations by the plain JSONL path.
//! Keep that identity stable while bounding and validating the physical file.
use super::*;
use std::io::{Seek, SeekFrom};

pub(super) fn compressed(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".jsonl.zst"))
}
pub(super) fn logical(tool: Tool, path: &Path) -> PathBuf {
    if tool == Tool::Codex && compressed(path) {
        path.with_extension("")
    } else {
        path.into()
    }
}
pub(super) fn physical(source: &Source) -> PathBuf {
    let path = logical(source.tool, &source.path);
    if source.tool == Tool::Codex && !path.exists() {
        let packed = path.with_extension("jsonl.zst");
        if packed.exists() {
            return packed;
        }
    }
    path
}
pub(super) fn exists(source: &Source) -> bool {
    physical(source).exists()
}
pub(super) fn metadata(source: &Source) -> Result<fs::Metadata> {
    let path = physical(source);
    safe_path(&source.root, &path)?;
    let meta = fs::metadata(path)?;
    ensure!(meta.is_file(), "Transcript is not a regular file");
    Ok(meta)
}
pub(super) fn open(root: &Path, path: &Path) -> Result<(Box<dyn Read>, fs::Metadata)> {
    safe_path(root, path)?;
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= COPY_LIMIT,
        "Transcript exceeds the 1 GiB reading limit"
    );
    let input = file.take(meta.len());
    let reader: Box<dyn Read> = if compressed(path) {
        let mut decoder = zstd::stream::read::Decoder::new(input)?;
        decoder.window_log_max(27)?;
        Box::new(decoder)
    } else {
        Box::new(input)
    };
    Ok((reader, meta))
}
pub(super) fn unchanged(source: &Source, path: &Path, before: &fs::Metadata) -> Result<()> {
    ensure!(
        physical(source) == path,
        "Transcript storage changed; retrying"
    );
    let after = metadata(source)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev() && before.ino() == after.ino(),
            "Transcript changed while reading; retrying"
        );
    }
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Transcript changed while reading; retrying"
    );
    Ok(())
}
pub(super) fn bytes(source: &Source, limit: u64) -> Result<Vec<u8>> {
    let path = physical(source);
    let (input, before) = open(&source.root, &path)?;
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Decoded transcript exceeds its reading limit"
    );
    unchanged(source, &path, &before)?;
    Ok(bytes)
}
pub(super) fn tail(source: &Source, count: usize) -> Result<Vec<u8>> {
    let path = physical(source);
    if !compressed(&path) {
        safe_path(&source.root, &path)?;
        let mut file = fs::File::open(&path)?;
        let before = file.metadata()?;
        file.seek(SeekFrom::Start(before.len().saturating_sub(count as u64)))?;
        let mut bytes = Vec::new();
        file.take(count as u64).read_to_end(&mut bytes)?;
        unchanged(source, &path, &before)?;
        return Ok(bytes);
    }
    let (mut input, before) = open(&source.root, &path)?;
    let mut tail = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(
            total <= COPY_LIMIT,
            "Decoded transcript exceeds the 1 GiB reading limit"
        );
        tail.extend_from_slice(&buffer[..n]);
        if tail.len() > count {
            tail.drain(..tail.len() - count);
        }
    }
    unchanged(source, &path, &before)?;
    Ok(tail)
}
