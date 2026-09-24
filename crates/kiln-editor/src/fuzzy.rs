//! 파일 목록 수집(백그라운드 ignore 워커)과 nucleo 기반 퍼지 매칭 워커.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use parking_lot::RwLock;

/// 루트 아래 파일 경로 목록. 워커가 채우는 동안에도 읽을 수 있다.
pub struct FileIndex {
    pub root: PathBuf,
    items: RwLock<Vec<Box<str>>>,
    len: AtomicUsize,
    done: AtomicBool,
    cancel: AtomicBool,
    pub started: Instant,
}

impl FileIndex {
    /// 백그라운드 스레드로 파일 목록을 수집한다. 묶음마다 `on_progress` 를 부른다.
    pub fn spawn(root: PathBuf, on_progress: impl Fn() + Send + Sync + 'static) -> Arc<FileIndex> {
        let idx = Arc::new(FileIndex {
            root: root.clone(),
            items: RwLock::new(Vec::new()),
            len: AtomicUsize::new(0),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            started: Instant::now(),
        });
        let w = idx.clone();
        let on_progress = Arc::new(on_progress);
        std::thread::Builder::new()
            .name("kiln-file-index".into())
            .spawn(move || {
                let (tx, rx) = channel::<Vec<Box<str>>>();
                let walker_root = root.clone();
                let cancel_idx = w.clone();
                let walker = std::thread::spawn(move || walk_files(&walker_root, &tx, &cancel_idx.cancel));
                for batch in rx {
                    let mut items = w.items.write();
                    items.extend(batch);
                    w.len.store(items.len(), Ordering::Release);
                    drop(items);
                    on_progress();
                }
                let _ = walker.join();
                w.done.store(true, Ordering::Release);
                on_progress();
            })
            .expect("spawn file index thread");
        idx
    }

    /// 주어진 목록으로 완료된 인덱스를 만든다.
    pub fn from_items(root: PathBuf, items: Vec<String>) -> Arc<FileIndex> {
        let len = items.len();
        Arc::new(FileIndex {
            root,
            len: AtomicUsize::new(len),
            items: RwLock::new(items.into_iter().map(String::into_boxed_str).collect()),
            done: AtomicBool::new(true),
            cancel: AtomicBool::new(false),
            started: Instant::now(),
        })
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn get(&self, i: usize) -> Option<String> {
        self.items.read().get(i).map(|s| s.to_string())
    }

    /// 상대 경로를 절대 경로로 바꾼다.
    pub fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// ignore 병렬 워커로 파일을 모아 묶음 단위로 보낸다. 경로는 `/` 로 구분된 상대 경로.
pub fn walk_files(root: &Path, tx: &Sender<Vec<Box<str>>>, cancel: &AtomicBool) {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .threads(threads)
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel();
    walker.run(|| {
        let tx = tx.clone();
        let root = root.to_path_buf();
        let mut local = Batcher { tx, batch: Vec::with_capacity(512) };
        Box::new(move |res| {
            if cancel.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Ok(e) = res else { return ignore::WalkState::Continue };
            if !e.file_type().is_some_and(|t| t.is_file() || t.is_symlink()) {
                return ignore::WalkState::Continue;
            }
            if let Ok(rel) = e.path().strip_prefix(&root) {
                let s = rel.to_string_lossy();
                let s = if std::path::MAIN_SEPARATOR == '\\' { s.replace('\\', "/") } else { s.into_owned() };
                local.push(s.into_boxed_str());
            }
            ignore::WalkState::Continue
        })
    });
}

struct Batcher {
    tx: Sender<Vec<Box<str>>>,
    batch: Vec<Box<str>>,
}

impl Batcher {
    fn push(&mut self, s: Box<str>) {
        self.batch.push(s);
        if self.batch.len() >= 512 {
            let _ = self.tx.send(std::mem::replace(&mut self.batch, Vec::with_capacity(512)));
        }
    }
}

impl Drop for Batcher {
    fn drop(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.tx.send(std::mem::take(&mut self.batch));
        }
    }
}

/// 한 항목의 매칭 결과. `indices` 는 경로의 문자 인덱스(정렬됨).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub index: u32,
    pub score: u32,
    pub path: String,
    pub indices: Vec<u32>,
}

fn config() -> Config {
    Config::DEFAULT.match_paths()
}

fn basename_start(path: &str) -> usize {
    path.rfind('/').map_or(0, |i| i + 1)
}

/// 한 경로의 점수. 파일 이름에 맞으면 가산점을 준다.
fn score_one(pattern: &Pattern, matcher: &mut Matcher, path: &str, buf: &mut Vec<char>, name_bonus: bool) -> Option<u32> {
    let full = pattern.score(Utf32Str::new(path, buf), matcher)?;
    let mut s = full * 2;
    if name_bonus {
        let name = &path[basename_start(path)..];
        if let Some(ns) = pattern.score(Utf32Str::new(name, buf), matcher) {
            s += ns;
        }
    }
    Some(s)
}

/// 점수 계산 상태(마지막 질의의 전체 일치 목록을 기억해 좁혀 가는 질의를 빠르게 처리).
pub struct Scorer {
    matchers: Vec<Matcher>,
    last_query: String,
    last_index: usize,
    last_len: usize,
    last_matched: Vec<u32>,
    last_complete: bool,
}

impl Default for Scorer {
    fn default() -> Self {
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
        Self {
            matchers: (0..n).map(|_| Matcher::new(config())).collect(),
            last_query: String::new(),
            last_index: 0,
            last_len: 0,
            last_matched: Vec::new(),
            last_complete: false,
        }
    }
}

/// 한 번의 매칭 결과.
#[derive(Clone, Debug, Default)]
pub struct MatchSet {
    pub query: String,
    pub matches: Vec<FuzzyMatch>,
    pub total: usize,
    pub scanned: usize,
}

impl Scorer {
    /// `items` 에서 `query` 를 찾아 상위 `limit` 개를 돌려준다. `cancelled` 가 참이 되면 `None`.
    pub fn run(&mut self, index_id: usize, items: &[Box<str>], query: &str, limit: usize, cancelled: &(dyn Fn() -> bool + Sync)) -> Option<MatchSet> {
        let query = query.trim();
        if query.is_empty() {
            let mut top: Vec<u32> = (0..items.len() as u32).collect();
            let key = |i: &u32| {
                let p = &items[*i as usize];
                (p.matches('/').count(), p.to_ascii_lowercase())
            };
            if top.len() > limit {
                top.select_nth_unstable_by_key(limit, |i| key(i));
                top.truncate(limit);
            }
            top.sort_by_key(key);
            self.last_complete = false;
            return Some(MatchSet {
                query: String::new(),
                matches: top.into_iter().map(|i| FuzzyMatch { index: i, score: 0, path: items[i as usize].to_string(), indices: Vec::new() }).collect(),
                total: items.len(),
                scanned: items.len(),
            });
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let name_bonus = !query.contains('/');
        let narrowing = self.last_complete
            && self.last_index == index_id
            && self.last_len == items.len()
            && !self.last_query.is_empty()
            && query.starts_with(&self.last_query)
            && !query.contains(['!', '^', '$', '\'', ' ']);
        let candidates: Vec<u32> = if narrowing { std::mem::take(&mut self.last_matched) } else { (0..items.len() as u32).collect() };

        let chunk = candidates.len().div_ceil(self.matchers.len()).max(4096);
        let chunks: Vec<&[u32]> = candidates.chunks(chunk).collect();
        let pattern_ref = &pattern;
        let results: Vec<Option<Vec<(u32, u32)>>> = std::thread::scope(|s| {
            let handles: Vec<_> = chunks
                .iter()
                .zip(self.matchers.iter_mut())
                .map(|(ids, m)| {
                    s.spawn(move || {
                        let mut buf = Vec::new();
                        let mut out = Vec::new();
                        for (k, &i) in ids.iter().enumerate() {
                            if k % 2048 == 0 && cancelled() {
                                return None;
                            }
                            if let Some(sc) = score_one(pattern_ref, m, &items[i as usize], &mut buf, name_bonus) {
                                out.push((i, sc));
                            }
                        }
                        Some(out)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        let mut scored: Vec<(u32, u32)> = Vec::new();
        for r in results {
            scored.extend(r?);
        }
        let total = scored.len();
        self.last_matched = scored.iter().map(|&(i, _)| i).collect();
        self.last_matched.sort_unstable();
        self.last_query = query.to_owned();
        self.last_index = index_id;
        self.last_len = items.len();
        self.last_complete = true;

        let cmp = |a: &(u32, u32), b: &(u32, u32)| {
            b.1.cmp(&a.1)
                .then_with(|| items[a.0 as usize].len().cmp(&items[b.0 as usize].len()))
                .then_with(|| items[a.0 as usize].cmp(&items[b.0 as usize]))
        };
        if scored.len() > limit {
            scored.select_nth_unstable_by(limit, cmp);
            scored.truncate(limit);
        }
        scored.sort_by(cmp);
        let m = &mut self.matchers[0];
        let mut buf = Vec::new();
        let matches = scored
            .into_iter()
            .map(|(i, score)| {
                let path = items[i as usize].to_string();
                let mut indices = Vec::new();
                let _ = pattern.indices(Utf32Str::new(&path, &mut buf), m, &mut indices);
                indices.sort_unstable();
                indices.dedup();
                FuzzyMatch { index: i, score, path, indices }
            })
            .collect();
        Some(MatchSet { query: query.to_owned(), matches, total, scanned: items.len() })
    }
}

struct Job {
    generation: u64,
    query: String,
    index: Arc<FileIndex>,
    limit: usize,
}

/// 결과 묶음과 그 세대 번호.
pub struct WorkerResult {
    pub generation: u64,
    pub set: MatchSet,
    pub index_len: usize,
}

/// 매칭 전용 백그라운드 스레드. 새 질의가 오면 진행 중인 계산을 버린다.
pub struct FuzzyWorker {
    tx: Sender<Job>,
    rx: Receiver<WorkerResult>,
    latest: Arc<AtomicU64>,
    generation: u64,
}

impl FuzzyWorker {
    pub fn new(on_result: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (res_tx, rx) = channel();
        let latest = Arc::new(AtomicU64::new(0));
        let latest_w = latest.clone();
        std::thread::Builder::new()
            .name("kiln-fuzzy".into())
            .spawn(move || {
                let mut scorer = Scorer::default();
                while let Ok(mut job) = jobs.recv() {
                    while let Ok(j) = jobs.try_recv() {
                        job = j;
                    }
                    let generation = job.generation;
                    let items = job.index.items.read();
                    let index_id = Arc::as_ptr(&job.index) as usize;
                    let cancelled = || latest_w.load(Ordering::Relaxed) != generation;
                    if let Some(set) = scorer.run(index_id, &items, &job.query, job.limit, &cancelled) {
                        let index_len = items.len();
                        drop(items);
                        if res_tx.send(WorkerResult { generation, set, index_len }).is_err() {
                            break;
                        }
                        on_result();
                    }
                }
            })
            .expect("spawn fuzzy worker");
        Self { tx, rx, latest, generation: 0 }
    }

    /// 질의를 보낸다. 세대 번호를 돌려준다.
    pub fn submit(&mut self, index: Arc<FileIndex>, query: &str, limit: usize) -> u64 {
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Relaxed);
        let _ = self.tx.send(Job { generation: self.generation, query: query.to_owned(), index, limit });
        self.generation
    }

    /// 가장 최근 세대의 결과가 있으면 돌려준다.
    pub fn poll(&mut self) -> Option<WorkerResult> {
        let mut last = None;
        while let Ok(r) = self.rx.try_recv() {
            if r.generation == self.generation {
                last = Some(r);
            }
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(v: &[&str]) -> Vec<Box<str>> {
        v.iter().map(|s| (*s).into()).collect()
    }

    fn top(v: &[Box<str>], q: &str) -> Vec<String> {
        Scorer::default().run(1, v, q, 10, &|| false).unwrap().matches.into_iter().map(|m| m.path).collect()
    }

    #[test]
    fn fuzzy_ranks_filename_and_contiguous_matches_first() {
        let v = items(&[
            "src/editor/view.rs",
            "src/main.rs",
            "docs/maintenance/notes.md",
            "crates/kiln-editor/src/main_window.rs",
            "tests/mainline_test.rs",
        ]);
        let r = top(&v, "main");
        assert_eq!(r[0], "src/main.rs");
        assert!(!r.contains(&"src/editor/view.rs".to_string()));
        let r = top(&v, "edview");
        assert_eq!(r[0], "src/editor/view.rs");
    }

    #[test]
    fn fuzzy_is_smart_case_and_reports_indices() {
        let v = items(&["README.md", "readme_old.txt", "src/Reader.rs"]);
        let set = Scorer::default().run(1, &v, "rdm", 10, &|| false).unwrap();
        let readme = set.matches.iter().find(|m| m.path == "README.md").unwrap();
        assert_eq!(readme.indices, vec![0, 3, 4]);
        // 대문자가 있으면 대소문자를 구분한다.
        assert_eq!(top(&v, "RE"), vec!["README.md"]);
    }

    #[test]
    fn narrowing_query_reuses_previous_matches_and_matches_fresh_run() {
        let v: Vec<Box<str>> = (0..5000).map(|i| format!("dir{}/file_{i}.rs", i % 37).into_boxed_str()).collect();
        let mut s = Scorer::default();
        s.run(7, &v, "fi", 50, &|| false).unwrap();
        let narrowed = s.run(7, &v, "fi12", 50, &|| false).unwrap();
        let fresh = Scorer::default().run(7, &v, "fi12", 50, &|| false).unwrap();
        assert_eq!(narrowed.total, fresh.total);
        assert_eq!(narrowed.matches, fresh.matches);
    }

    #[test]
    fn empty_query_lists_shallow_paths_first() {
        let v = items(&["a/b/c.rs", "z.rs", "a/y.rs"]);
        assert_eq!(top(&v, ""), vec!["z.rs", "a/y.rs", "a/b/c.rs"]);
    }

    #[test]
    fn cancellation_returns_none() {
        let v: Vec<Box<str>> = (0..10_000).map(|i| format!("f{i}").into_boxed_str()).collect();
        assert!(Scorer::default().run(1, &v, "f1", 10, &|| true).is_none());
    }

    #[test]
    fn index_walk_respects_gitignore() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::create_dir_all(d.path().join("target")).unwrap();
        std::fs::write(d.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::write(d.path().join("src/a.rs"), "").unwrap();
        std::fs::write(d.path().join("target/x.o"), "").unwrap();
        let (tx, rx) = channel();
        walk_files(d.path(), &tx, &AtomicBool::new(false));
        drop(tx);
        let mut all: Vec<String> = rx.into_iter().flatten().map(|s| s.to_string()).collect();
        all.sort();
        assert_eq!(all, vec![".gitignore", "src/a.rs"]);
    }

    #[test]
    fn scoring_100k_paths_is_fast_enough() {
        let v: Vec<Box<str>> = (0..100_000)
            .map(|i| format!("crates/pkg{}/src/module_{}/file_{i}.rs", i % 50, i % 300).into_boxed_str())
            .collect();
        let t = Instant::now();
        let set = Scorer::default().run(1, &v, "pkg7modfile99", 100, &|| false).unwrap();
        let el = t.elapsed();
        println!("PERF fuzzy 100k paths, specific query: {:.1} ms, {} matches", el.as_secs_f64() * 1000.0, set.total);
        assert!(set.total > 0);
        let mut s = Scorer::default();
        let t = Instant::now();
        let broad = s.run(1, &v, "f", 100, &|| false).unwrap();
        let broad_t = t.elapsed();
        let t = Instant::now();
        let narrowed = s.run(1, &v, "fmo", 100, &|| false).unwrap();
        let narrow_t = t.elapsed();
        println!(
            "PERF fuzzy 100k paths, 1-char query: {:.1} ms ({} matches); narrowed to 3 chars: {:.1} ms",
            broad_t.as_secs_f64() * 1000.0,
            broad.total,
            narrow_t.as_secs_f64() * 1000.0
        );
        assert!(narrowed.total <= broad.total);
    }
}
