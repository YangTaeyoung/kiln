//! 프로젝트 전체 검색 백엔드: ignore 병렬 워커 + grep-searcher, 결과는 채널로 흘려보낸다.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher as _;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};

use crate::buffer::FindOptions;

/// 검색 조건. `include`/`exclude` 는 쉼표로 구분한 글롭 목록.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    pub opts: FindOptions,
    pub include: String,
    pub exclude: String,
}

/// 한 줄의 일치. `line`, `col` 은 1부터. `ranges` 는 `preview` 안의 바이트 범위.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMatch {
    pub line: usize,
    pub col: usize,
    pub preview: String,
    pub ranges: Vec<Range<usize>>,
}

/// 파일 하나의 일치 목록.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub rel: String,
    pub lines: Vec<LineMatch>,
}

impl FileMatches {
    pub fn match_count(&self) -> usize {
        self.lines.iter().map(|l| l.ranges.len().max(1)).sum()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchSummary {
    pub files: usize,
    pub matches: usize,
    pub truncated: bool,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub enum SearchMsg {
    File(FileMatches),
    Done(SearchSummary),
    Error(String),
}

/// 실행 중인 검색. 버리면 취소된다.
pub struct SearchHandle {
    cancel: Arc<AtomicBool>,
    pub rx: Receiver<SearchMsg>,
}

impl SearchHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

const PREVIEW_MAX: usize = 220;
const PREVIEW_LEAD: usize = 48;
const PER_FILE_LINE_CAP: usize = 2_000;

/// 쉼표 구분 글롭 목록을 만든다. `/` 가 없는 패턴은 어느 깊이에서든 이름으로 맞춘다.
pub fn build_globs(spec: &str) -> Result<Option<GlobSet>, String> {
    let mut b = GlobSetBuilder::new();
    let mut any = false;
    for raw in spec.split(',') {
        let p = raw.trim().trim_start_matches("./");
        if p.is_empty() {
            continue;
        }
        let p = p.trim_end_matches('/');
        let pats: Vec<String> = if p.contains('/') {
            let p = p.trim_start_matches('/');
            vec![p.to_owned(), format!("{p}/**")]
        } else {
            vec![format!("**/{p}"), format!("**/{p}/**")]
        };
        for pat in pats {
            let g = GlobBuilder::new(&pat).literal_separator(true).build().map_err(|e| e.to_string())?;
            b.add(g);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    b.build().map(Some).map_err(|e| e.to_string())
}

fn build_matcher(q: &SearchQuery) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_insensitive(!q.opts.case_sensitive)
        .word(q.opts.whole_word)
        .fixed_strings(!q.opts.regex)
        .line_terminator(Some(b'\n'))
        .build(&q.pattern)
        .map_err(|e| e.to_string())
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 한 줄을 미리보기 문자열로 줄이고 범위를 맞춘다.
fn make_preview(line: &str, ranges: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let line = line.trim_end_matches(['\n', '\r']);
    let lead = line.len() - line.trim_start().len();
    let first = ranges.first().map_or(lead, |r| r.start);
    let mut start = lead.min(first);
    if line.len() - start > PREVIEW_MAX && first > start + PREVIEW_LEAD {
        start = floor_boundary(line, first - PREVIEW_LEAD);
    }
    let end = floor_boundary(line, start + PREVIEW_MAX);
    let mut out = String::new();
    let mut shift = start as isize;
    if start > lead {
        out.push('…');
        shift -= '…'.len_utf8() as isize;
    }
    out.push_str(&line[start..end]);
    if end < line.len() {
        out.push('…');
    }
    let adj = ranges
        .iter()
        .filter(|r| r.start >= start && r.start < end)
        .map(|r| {
            let s = (r.start as isize - shift) as usize;
            let e = (r.end.min(end) as isize - shift) as usize;
            s..e.max(s)
        })
        .collect();
    (out, adj)
}

/// 백그라운드 검색을 시작한다. 결과가 올 때마다 `notify` 를 부른다.
pub fn start_search(
    root: PathBuf,
    query: SearchQuery,
    max_matches: usize,
    notify: impl Fn() + Send + Sync + 'static,
) -> SearchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let c = cancel.clone();
    let notify = Arc::new(notify);
    std::thread::Builder::new()
        .name("kiln-search".into())
        .spawn(move || {
            let n2 = notify.clone();
            run_search(&root, &query, max_matches, &c, &tx, &move || n2());
            notify();
        })
        .expect("spawn search thread");
    SearchHandle { cancel, rx }
}

/// 검색을 실행하고 결과를 `tx` 로 보낸다. 끝나면 `Done` 을 보낸다.
pub fn run_search(
    root: &Path,
    query: &SearchQuery,
    max_matches: usize,
    cancel: &AtomicBool,
    tx: &Sender<SearchMsg>,
    notify: &(dyn Fn() + Send + Sync),
) {
    let started = Instant::now();
    if query.pattern.is_empty() {
        let _ = tx.send(SearchMsg::Done(SearchSummary::default()));
        return;
    }
    let matcher = match build_matcher(query) {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(SearchMsg::Error(e));
            return;
        }
    };
    let include = match build_globs(&query.include) {
        Ok(g) => g,
        Err(e) => {
            let _ = tx.send(SearchMsg::Error(format!("포함할 파일: {e}")));
            return;
        }
    };
    let exclude = match build_globs(&query.exclude) {
        Ok(g) => g,
        Err(e) => {
            let _ = tx.send(SearchMsg::Error(format!("제외할 파일: {e}")));
            return;
        }
    };
    let total = AtomicUsize::new(0);
    let files = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    let excl_root = root.to_path_buf();
    let exclude_f = exclude.clone();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .threads(threads)
        .filter_entry(move |e| {
            if e.file_name() == ".git" {
                return false;
            }
            match (&exclude_f, e.path().strip_prefix(&excl_root)) {
                (Some(g), Ok(rel)) if !rel.as_os_str().is_empty() => !g.is_match(rel),
                _ => true,
            }
        })
        .build_parallel();
    walker.run(|| {
        let matcher = matcher.clone();
        let tx = tx.clone();
        let include = include.clone();
        let root = root.to_path_buf();
        let (total, files, truncated) = (&total, &files, &truncated);
        let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
        Box::new(move |res| {
            if cancel.load(Ordering::Relaxed) || truncated.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Ok(entry) = res else { return ignore::WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            if let Some(inc) = &include
                && !inc.is_match(rel)
            {
                return ignore::WalkState::Continue;
            }
            let mut lines: Vec<LineMatch> = Vec::new();
            let _ = searcher.search_path(
                &matcher,
                path,
                Lossy(|lnum, line| {
                    let mut ranges = Vec::new();
                    let _ = matcher.find_iter(line.as_bytes(), |m| {
                        if m.start() < m.end() {
                            ranges.push(m.start()..m.end());
                        }
                        true
                    });
                    if ranges.is_empty() {
                        return Ok(true);
                    }
                    let col = line[..floor_boundary(line, ranges[0].start)].chars().count() + 1;
                    let n = ranges.len();
                    let (preview, ranges) = make_preview(line, &ranges);
                    lines.push(LineMatch { line: lnum as usize, col, preview, ranges });
                    let t = total.fetch_add(n, Ordering::Relaxed) + n;
                    if t >= max_matches {
                        truncated.store(true, Ordering::Relaxed);
                        return Ok(false);
                    }
                    Ok(lines.len() < PER_FILE_LINE_CAP && !cancel.load(Ordering::Relaxed))
                }),
            );
            if !lines.is_empty() {
                files.fetch_add(1, Ordering::Relaxed);
                let rel_s = rel.to_string_lossy().replace('\\', "/");
                if tx.send(SearchMsg::File(FileMatches { path: path.to_path_buf(), rel: rel_s, lines })).is_err() {
                    return ignore::WalkState::Quit;
                }
                notify();
            }
            ignore::WalkState::Continue
        })
    });
    if !cancel.load(Ordering::Relaxed) {
        let _ = tx.send(SearchMsg::Done(SearchSummary {
            files: files.load(Ordering::Relaxed),
            matches: total.load(Ordering::Relaxed),
            truncated: truncated.load(Ordering::Relaxed),
            elapsed: started.elapsed(),
        }));
    }
}

/// 동기 검색(테스트·도구용). 결과는 상대 경로 순으로 정렬된다.
pub fn search_blocking(root: &Path, query: &SearchQuery, max_matches: usize) -> Result<(Vec<FileMatches>, SearchSummary), String> {
    let (tx, rx) = channel();
    run_search(root, query, max_matches, &AtomicBool::new(false), &tx, &|| {});
    drop(tx);
    let mut files = Vec::new();
    let mut summary = SearchSummary::default();
    for m in rx {
        match m {
            SearchMsg::File(f) => files.push(f),
            SearchMsg::Done(s) => summary = s,
            SearchMsg::Error(e) => return Err(e),
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((files, summary))
}

/// 파일 전체에 대한 치환용 정규식.
fn file_regex(q: &SearchQuery) -> Result<regex::Regex, regex::Error> {
    let mut pat = if q.opts.regex { q.pattern.clone() } else { regex::escape(&q.pattern) };
    if q.opts.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    regex::RegexBuilder::new(&pat).case_insensitive(!q.opts.case_sensitive).multi_line(true).build()
}

/// 주어진 파일들에서 검색어를 모두 바꾼다. (바뀐 파일 수, 바꾼 개수)를 돌려준다.
pub fn replace_in_files(files: &[PathBuf], query: &SearchQuery, replacement: &str) -> anyhow::Result<(usize, usize)> {
    let re = file_regex(query)?;
    let mut changed_files = 0;
    let mut count = 0;
    for f in files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let n = re.find_iter(&text).filter(|m| !m.is_empty()).count();
        if n == 0 {
            continue;
        }
        let new = if query.opts.regex {
            re.replace_all(&text, replacement)
        } else {
            re.replace_all(&text, regex::NoExpand(replacement))
        };
        if new != text {
            std::fs::write(f, new.as_bytes())?;
            changed_files += 1;
            count += n;
        }
    }
    Ok((changed_files, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/util")).unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::write(r.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main() {\n    let foo = Foo::new();\n    foo.run(); // foo\n}\n").unwrap();
        std::fs::write(r.join("src/util/food.rs"), "pub fn food() -> u32 { 42 }\n").unwrap();
        std::fs::write(r.join("README.md"), "# Foo\nfoo bar\n").unwrap();
        std::fs::write(r.join("target/gen.rs"), "foo foo foo\n").unwrap();
        std::fs::write(r.join("blob.bin"), b"foo\0\0\0binary").unwrap();
        d
    }

    fn q(pattern: &str) -> SearchQuery {
        SearchQuery { pattern: pattern.into(), ..Default::default() }
    }

    fn count(r: &Path, query: &SearchQuery) -> (usize, usize) {
        let (files, s) = search_blocking(r, query, 10_000).unwrap();
        (files.len(), s.matches)
    }

    #[test]
    fn plain_search_is_case_insensitive_and_skips_ignored_and_binary() {
        let d = fixture();
        let (files, s) = search_blocking(d.path(), &q("foo"), 10_000).unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["README.md", "src/main.rs", "src/util/food.rs"]);
        assert_eq!(s.matches, 2 + 4 + 1);
        let main = &files[1];
        assert_eq!(main.lines[0].line, 2);
        assert_eq!(main.lines[0].col, 9);
        assert_eq!(main.lines[0].preview, "let foo = Foo::new();");
        assert_eq!(main.lines[0].ranges, vec![4..7, 10..13]);
    }

    #[test]
    fn case_sensitive_whole_word_and_regex() {
        let d = fixture();
        let cs = SearchQuery { opts: FindOptions { case_sensitive: true, ..Default::default() }, ..q("Foo") };
        assert_eq!(count(d.path(), &cs), (2, 2));
        let ww = SearchQuery { opts: FindOptions { whole_word: true, ..Default::default() }, ..q("foo") };
        assert_eq!(count(d.path(), &ww), (2, 6));
        let re = SearchQuery { opts: FindOptions { regex: true, ..Default::default() }, ..q(r"fo+d?\(") };
        assert_eq!(count(d.path(), &re), (1, 1));
        let literal = q("Foo::new(");
        assert_eq!(count(d.path(), &literal), (1, 1));
        let bad = SearchQuery { opts: FindOptions { regex: true, ..Default::default() }, ..q("(") };
        assert!(search_blocking(d.path(), &bad, 10).is_err());
    }

    #[test]
    fn include_and_exclude_globs() {
        let d = fixture();
        let inc = SearchQuery { include: "*.rs".into(), ..q("foo") };
        assert_eq!(count(d.path(), &inc).0, 2);
        let inc_dir = SearchQuery { include: "src/util".into(), ..q("foo") };
        assert_eq!(count(d.path(), &inc_dir).0, 1);
        let exc = SearchQuery { exclude: "util, *.md".into(), ..q("foo") };
        assert_eq!(count(d.path(), &exc).0, 1);
    }

    #[test]
    fn result_cap_marks_truncated() {
        let d = fixture();
        let (_, s) = search_blocking(d.path(), &q("foo"), 2).unwrap();
        assert!(s.truncated);
    }

    #[test]
    fn long_line_preview_is_windowed_around_match() {
        let line = format!("{}needle{}", "a".repeat(500), "b".repeat(500));
        let (p, r) = make_preview(&line, std::slice::from_ref(&(500..506)));
        assert!(p.starts_with('…') && p.ends_with('…'));
        assert_eq!(&p[r[0].clone()], "needle");
        assert!(p.len() <= PREVIEW_MAX + 8);
    }

    #[test]
    fn preview_trims_indent_and_keeps_ranges_aligned() {
        let (p, r) = make_preview("\t    let x = 1;\r\n", std::slice::from_ref(&(9..10)));
        assert_eq!(p, "let x = 1;");
        assert_eq!(&p[r[0].clone()], "x");
    }

    #[test]
    fn replace_across_files_with_literal_and_regex() {
        let d = fixture();
        let (files, _) = search_blocking(d.path(), &q("foo"), 10_000).unwrap();
        let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        let query = SearchQuery { opts: FindOptions { case_sensitive: true, whole_word: true, ..Default::default() }, ..q("foo") };
        let (nf, n) = replace_in_files(&paths, &query, "$bar").unwrap();
        assert_eq!((nf, n), (2, 4));
        assert_eq!(
            std::fs::read_to_string(d.path().join("src/main.rs")).unwrap(),
            "fn main() {\n    let $bar = Foo::new();\n    $bar.run(); // $bar\n}\n"
        );
        let re = SearchQuery { opts: FindOptions { regex: true, ..Default::default() }, ..q(r"fn (\w+)\(\)") };
        replace_in_files(&paths, &re, "fn ${1}_v2()").unwrap();
        assert!(std::fs::read_to_string(d.path().join("src/util/food.rs")).unwrap().starts_with("pub fn food_v2()"));
    }

    #[test]
    fn glob_spec_parsing() {
        let g = build_globs("*.rs, docs/").unwrap().unwrap();
        assert!(g.is_match("a/b/c.rs"));
        assert!(g.is_match("docs/x/y.md"));
        assert!(!g.is_match("src/docs.md"));
        assert!(build_globs("  , ").unwrap().is_none());
        assert!(build_globs("a[").is_err());
    }
}
