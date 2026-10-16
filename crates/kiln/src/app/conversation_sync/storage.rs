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
/// Metadata of an opened transcript and the identity of the file it was opened from.
pub(super) struct Stamp {
    meta: fs::Metadata,
    id: FileId,
}
impl std::ops::Deref for Stamp {
    type Target = fs::Metadata;
    fn deref(&self) -> &fs::Metadata {
        &self.meta
    }
}
impl Stamp {
    fn of(file: &fs::File) -> Result<Self> {
        Ok(Self { meta: file.metadata()?, id: file_id(file)? })
    }
}
/// Device and inode on Unix; volume serial number and file index on Windows.
type FileId = (u64, u64);
fn file_id(file: &fs::File) -> Result<FileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        Ok((meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value of the plain struct.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle belongs to `file`, which outlives the call.
        let read = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        ensure!(read != 0, "Cannot read the transcript file identity: {}", std::io::Error::last_os_error());
        Ok((u64::from(info.dwVolumeSerialNumber), u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow)))
    }
}
/// Fails when `path` no longer names the file `before` was opened from.
pub(super) fn same_file(path: &Path, before: &Stamp) -> Result<()> {
    ensure!(
        file_id(&fs::File::open(path)?)? == before.id,
        "Transcript changed while reading; retrying"
    );
    Ok(())
}
pub(super) fn open(root: &Path, path: &Path) -> Result<(Box<dyn Read>, Stamp)> {
    open_at(root, path, 0)
}
/// Opens a transcript positioned at byte `start` of an uncompressed file.
pub(super) fn open_at(root: &Path, path: &Path, start: u64) -> Result<(Box<dyn Read>, Stamp)> {
    safe_path(root, path)?;
    let mut file = fs::File::open(path)?;
    let meta = Stamp::of(&file)?;
    ensure!(
        meta.is_file() && meta.len() <= COPY_LIMIT,
        "Transcript exceeds the 1 GiB reading limit"
    );
    ensure!(
        start == 0 || (!compressed(path) && start <= meta.len()),
        "Transcript changed while reading; retrying"
    );
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(start))?;
    let input = file.take(meta.len() - start);
    let reader: Box<dyn Read> = if compressed(path) {
        let mut decoder = zstd::stream::read::Decoder::new(input)?;
        decoder.window_log_max(27)?;
        Box::new(decoder)
    } else {
        Box::new(input)
    };
    Ok((reader, meta))
}
pub(super) fn unchanged(source: &Source, path: &Path, before: &Stamp) -> Result<()> {
    ensure!(
        physical(source) == path,
        "Transcript storage changed; retrying"
    );
    let after = metadata(source)?;
    same_file(path, before)?;
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
/// Whether the decoded transcript is exactly `expected`, compared in chunks without
/// buffering the file; fails if the file changes while it is read.
pub(super) fn equals(source: &Source, expected: &[u8]) -> Result<bool> {
    let path = physical(source);
    let (mut input, before) = open(&source.root, &path)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut offset = 0;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if expected.get(offset..offset + read) != Some(&buffer[..read]) {
            return Ok(false);
        }
        offset += read;
    }
    unchanged(source, &path, &before)?;
    Ok(offset == expected.len())
}
pub(super) fn tail(source: &Source, count: usize) -> Result<Vec<u8>> {
    let path = physical(source);
    if !compressed(&path) {
        safe_path(&source.root, &path)?;
        let mut file = fs::File::open(&path)?;
        let before = Stamp::of(&file)?;
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
