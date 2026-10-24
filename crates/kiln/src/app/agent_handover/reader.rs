//! Periodic metadata IO lives here; the GUI only exchanges bounded snapshots.
use super::*;
use std::{fs, sync::{Arc, mpsc}, time::SystemTime};

#[derive(Default)]
pub(super) struct Request {
    pub config: PathBuf,
    pub paths: Vec<PathBuf>,
    pub leases: Vec<(PathBuf, Option<u32>)>,
    pub enabled: bool,
}
#[derive(Default)]
pub(super) struct Response {
    pub records: HashMap<PathBuf, Arc<agent_runtime::Record>>,
    pub links: Arc<Vec<handover::Link>>,
    pub owners: Arc<handover::Owners>,
    pub leases: Vec<(PathBuf, WriterLease)>,
    pub valid: bool,
}

pub(super) struct Reader {
    tx: mpsc::SyncSender<Request>,
    rx: mpsc::Receiver<Response>,
}
impl Reader {
    pub fn new(ctx: egui::Context) -> Self {
        let mut cache = Cache::default();
        Self::spawn(ctx, move |request| cache.read(request))
    }
    fn spawn(ctx: egui::Context, mut read: impl FnMut(Request) -> Response + Send + 'static) -> Self {
        let (tx, requests) = mpsc::sync_channel(1);
        let (responses, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new().name("kiln-handover-reader".into()).spawn(move || {
            while let Ok(request) = requests.recv() {
                if responses.send(read(request)).is_err() { break; }
                ctx.request_repaint();
            }
        }).expect("handover reader thread");
        Self { tx, rx }
    }
    pub fn request(&self, request: Request) -> bool { self.tx.try_send(request).is_ok() }
    pub fn poll(&mut self) -> Option<Response> { self.rx.try_recv().ok() }
}

type Stamp = Option<(u64, SystemTime, Option<SystemTime>)>;
struct Cached<T> { stamp: Stamp, value: Option<Arc<T>> }
impl<T> Default for Cached<T> { fn default() -> Self { Self { stamp: None, value: None } } }
impl<T> Cached<T> {
    fn get(&mut self, path: &Path, load: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<Arc<T>> {
        let stamp = fs::metadata(path).ok().and_then(|m| Some((m.len(), m.modified().ok()?, m.created().ok())));
        if self.stamp == stamp && let Some(value) = &self.value { return Ok(value.clone()); }
        let value = Arc::new(load()?);
        self.stamp = stamp;
        self.value = Some(value.clone());
        Ok(value)
    }
}
#[derive(Default)]
struct Cache {
    records: HashMap<PathBuf, Cached<agent_runtime::Record>>,
    links: Cached<Vec<handover::Link>>,
    owners: Cached<handover::Owners>,
    config: PathBuf,
}
impl Cache {
    fn read(&mut self, request: Request) -> Response {
        if self.config != request.config { *self = Self { config: request.config.clone(), ..Default::default() }; }
        let mut out = Response::default();
        self.records.retain(|path, _| request.paths.contains(path));
        for path in request.paths {
            if let Ok(record) = self.records.entry(path.clone()).or_default().get(&path, || agent_runtime::read(&path)) {
                out.records.insert(path, record);
            }
        }
        for (path, pid) in request.leases {
            if let Ok(file) = handover::live_lease(&path) {
                out.leases.push((path, WriterLease { _file: file, pid }));
            }
        }
        if request.enabled {
            let links = self.links.get(&request.config.join("conversation-sync/native-links.json"), || handover::links(&request.config));
            let owners = self.owners.get(&request.config.join("conversation-sync/owners.json"), || handover::owners(&request.config));
            if let (Ok(links), Ok(owners)) = (links, owners) { out.links = links; out.owners = owners; out.valid = true; }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_json_is_reused_and_changed_json_is_read_again() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("links.json");
        std::fs::write(&path, "[1]").unwrap();
        let mut cached = Cached::<Vec<u32>>::default();
        let reads = std::cell::Cell::new(0);
        let read = || { reads.set(reads.get()+1); Ok(serde_json::from_slice::<Vec<u32>>(&std::fs::read(&path)?)?) };
        let first = cached.get(&path, &read).unwrap();
        let again = cached.get(&path, &read).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(reads.get(), 1);
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(&*cached.get(&path, &read).unwrap(), &[1,2]);
        assert_eq!(reads.get(), 2);
    }
    #[test]
    fn a_slow_reader_does_not_block_ui_polling_or_queue_requests_without_bound() {
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let mut reader = Reader::spawn(egui::Context::default(), move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Response::default()
        });
        assert!(reader.request(Request::default()));
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let start=Instant::now();
        assert!(reader.poll().is_none());
        assert!(reader.request(Request::default()));
        assert!(!reader.request(Request::default()));
        let elapsed=start.elapsed();
        release.send(()).unwrap();
        release.send(()).unwrap();
        assert!(elapsed < Duration::from_millis(50));
    }
}
