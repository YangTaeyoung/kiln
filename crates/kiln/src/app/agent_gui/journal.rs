//! Owner-private append journal and atomic command mailbox; no credential material.
use super::{Envelope, Event, QueuedCommand, Transcript};
use anyhow::{Context, Result, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub(super) const MAX_RECORD: usize = 2 * 1024 * 1024;
pub(super) const MAX_JOURNAL: u64 = 256 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) fn unique() -> String {
    let mut bytes = [0; 16];
    use ring::rand::{SecureRandom, SystemRandom};
    SystemRandom::new().fill(&mut bytes).expect("OS randomness");
    format!(
        "{:039}-{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub(super) fn directory(id: &str) -> Result<PathBuf> {
    ensure!(valid_id(id), "invalid agent ID");
    Ok(std::path::absolute(kiln_common::paths::config_dir())?
        .join("agent-gui")
        .join(id))
}
pub(super) fn private_dir(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            fs::symlink_metadata(path)?.file_type().is_dir(),
            "invalid private directory"
        );
    }
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(super) fn open(path: &Path, append: bool) -> Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let f = o.open(path)?;
    ensure!(f.metadata()?.is_file(), "invalid private file");
    Ok(f)
}
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
pub(super) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= MAX_RECORD, "oversized agent record");
    let tmp = path.with_extension(format!("{}.tmp", unique()));
    let result = (|| {
        let mut f = open(&tmp, false)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        sync_directory(path.parent().context("missing private parent")?)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "invalid agent record"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_RECORD, "oversized agent record");
    Ok(serde_json::from_slice(&bytes)?)
}
pub(super) struct Journal {
    file: File,
    pub generation: String,
    pub transcript: Transcript,
}
impl Journal {
    pub fn open(dir: &Path, generation: String) -> Result<Self> {
        let file = open(&dir.join("events.jsonl"), true)?;
        ensure!(
            file.metadata()?.len() <= MAX_JOURNAL,
            "agent journal is full"
        );
        let mut transcript = Transcript::default();
        let mut offset = 0;
        loop {
            let batch = tail(&dir.join("events.jsonl"), &mut offset)?;
            if batch.is_empty() {
                break;
            }
            for event in batch {
                transcript.apply(&event);
            }
        }
        // Drop an incomplete final append after a crash, before writing a new generation.
        file.set_len(offset)?;
        Ok(Self {
            file,
            generation,
            transcript,
        })
    }
    pub fn push(&mut self, event: Event) -> Result<Envelope> {
        let envelope = Envelope {
            sequence: self.transcript.sequence + 1,
            generation: self.generation.clone(),
            event,
        };
        let mut bytes = serde_json::to_vec(&envelope)?;
        ensure!(bytes.len() <= MAX_RECORD, "oversized agent event");
        bytes.push(b'\n');
        ensure!(
            self.file.metadata()?.len() + bytes.len() as u64 <= MAX_JOURNAL,
            "agent journal is full"
        );
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.transcript.apply(&envelope);
        Ok(envelope)
    }
    pub fn flush(&self) -> Result<()> {
        Ok(self.file.sync_all()?)
    }
}
pub(super) fn tail(path: &Path, offset: &mut u64) -> Result<Vec<Envelope>> {
    let mut f = File::open(path)?;
    ensure!(
        f.metadata()?.len() <= MAX_JOURNAL,
        "oversized agent journal"
    );
    f.seek(SeekFrom::Start(*offset))?;
    let mut r = BufReader::new(f);
    let mut out = Vec::new();
    for _ in 0..128 {
        let mut bytes = Vec::new();
        let n = r
            .by_ref()
            .take((MAX_RECORD + 2) as u64)
            .read_until(b'\n', &mut bytes)?;
        if n == 0 {
            break;
        }
        ensure!(n <= MAX_RECORD + 1, "oversized agent event");
        if bytes.last() != Some(&b'\n') {
            break;
        }
        let event = serde_json::from_slice(&bytes).context("invalid agent journal event")?;
        *offset += n as u64;
        out.push(event);
    }
    Ok(out)
}
pub(super) fn enqueue(dir: &Path, command: &QueuedCommand) -> Result<()> {
    ensure!(valid_id(&command.id), "invalid command ID");
    let queue = dir.join("commands");
    private_dir(&queue)?;
    ensure!(
        fs::read_dir(&queue)?.take(257).count() < 256,
        "agent command queue is full"
    );
    atomic(
        &queue.join(format!("{}.json", command.id)),
        &serde_json::to_vec(command)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_recovers_native_tool_identity_and_incomplete_tail() {
        let d = tempfile::tempdir().unwrap();
        let mut j = Journal::open(d.path(), "a".into()).unwrap();
        j.push(Event::Generation).unwrap();
        j.push(Event::Session("native".into())).unwrap();
        j.push(Event::Tool(crate::app::agent_gui::ToolCall {
            id: "tool".into(),
            output: "result".into(),
            ..Default::default()
        }))
        .unwrap();
        drop(j);
        open(&d.path().join("events.jsonl"), true)
            .unwrap()
            .write_all(b"{\"partial\"")
            .unwrap();
        let mut j = Journal::open(d.path(), "b".into()).unwrap();
        j.push(Event::Generation).unwrap();
        assert_eq!(j.transcript.provider_session_id.as_deref(), Some("native"));
        assert_eq!(j.transcript.entries.len(), 1);
        let mut offset = 0;
        let replay = tail(&d.path().join("events.jsonl"), &mut offset).unwrap();
        assert_eq!(replay.len(), 4);
    }
    #[test]
    fn ids_never_escape_private_directory() {
        for id in ["", "../x", "a/b", ".", "a\\b"] {
            assert!(!valid_id(id));
        }
        assert!(valid_id(&unique()));
    }
}
