//! unified diff 파싱, 행 단위 단어 강조, 헝크 패치 생성.

/// diff 행 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
    /// `\ No newline at end of file`
    NoNewline,
}

/// diff 본문 한 행.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// 앞의 `+`/`-`/` ` 를 뺀 내용.
    pub text: String,
    /// 행 내 변경 구간(바이트 범위). 짝지어진 추가/삭제 행에만 채워진다.
    pub emph: Option<(usize, usize)>,
}

/// 헝크.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// `@@ -a,b +c,d @@ ...` 원문.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// 헤더의 `@@` 뒤 함수 문맥 부분.
    pub fn section(&self) -> &str {
        self.header.splitn(3, "@@").nth(2).map(str::trim).unwrap_or("")
    }

    pub fn added(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Add).count()
    }

    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|l| l.kind == LineKind::Remove).count()
    }
}

/// 파일 변경 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
}

impl FileChange {
    pub fn letter(self) -> char {
        match self {
            FileChange::Modified => 'M',
            FileChange::Added => 'A',
            FileChange::Deleted => 'D',
            FileChange::Renamed => 'R',
            FileChange::Copied => 'C',
        }
    }
}

/// 파일 하나의 diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub change: FileChange,
    pub binary: bool,
    /// `diff --git` 부터 `+++` 까지의 헤더 원문 행.
    pub header_lines: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
}

impl FileDiff {
    /// 표시용 경로(새 경로 우선).
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }

    pub fn added(&self) -> usize {
        self.hunks.iter().map(Hunk::added).sum()
    }

    pub fn removed(&self) -> usize {
        self.hunks.iter().map(Hunk::removed).sum()
    }

    /// `hunk_index` 헝크 하나만 담은 패치 텍스트를 만든다. `git apply` 입력용.
    pub fn hunk_patch(&self, hunk_index: usize) -> Option<String> {
        let h = self.hunks.get(hunk_index)?;
        let mut s = String::new();
        for l in &self.header_lines {
            s.push_str(l);
            s.push('\n');
        }
        if !self.header_lines.iter().any(|l| l.starts_with("--- ")) {
            let old = self.old_path.as_deref().map(|p| format!("a/{p}")).unwrap_or_else(|| "/dev/null".into());
            let new = self.new_path.as_deref().map(|p| format!("b/{p}")).unwrap_or_else(|| "/dev/null".into());
            s.push_str(&format!("--- {old}\n+++ {new}\n"));
        }
        s.push_str(&h.header);
        s.push('\n');
        for l in &h.lines {
            let prefix = match l.kind {
                LineKind::Context => ' ',
                LineKind::Add => '+',
                LineKind::Remove => '-',
                LineKind::NoNewline => '\\',
            };
            s.push(prefix);
            s.push_str(&l.text);
            s.push('\n');
        }
        Some(s)
    }
}

/// 여러 파일이 담긴 unified diff 텍스트를 파싱한다.
pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut in_header = false;
    let mut old_no = 0u32;
    let mut new_no = 0u32;

    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(finish(f));
            }
            let (a, b) = split_git_header_paths(rest);
            cur = Some(FileDiff {
                old_path: a,
                new_path: b,
                change: FileChange::Modified,
                binary: false,
                header_lines: vec![line.to_string()],
                hunks: Vec::new(),
                old_mode: None,
                new_mode: None,
            });
            in_header = true;
            continue;
        }
        if line.starts_with("diff --cc ") || line.starts_with("diff --combined ") {
            if let Some(f) = cur.take() {
                files.push(finish(f));
            }
            in_header = false;
            continue;
        }
        let Some(f) = cur.as_mut() else {
            // `diff --git` 없이 `---` 로 시작하는 순수 unified diff
            if let Some(rest) = line.strip_prefix("--- ") {
                cur = Some(FileDiff {
                    old_path: parse_marker_path(rest),
                    new_path: None,
                    change: FileChange::Modified,
                    binary: false,
                    header_lines: vec![line.to_string()],
                    hunks: Vec::new(),
                    old_mode: None,
                    new_mode: None,
                });
                in_header = true;
            }
            continue;
        };
        if in_header {
            if let Some(h) = parse_hunk_header(line) {
                in_header = false;
                old_no = h.old_start;
                new_no = h.new_start;
                f.hunks.push(h);
                continue;
            }
            f.header_lines.push(line.to_string());
            if let Some(v) = line.strip_prefix("new file mode ") {
                f.change = FileChange::Added;
                f.new_mode = Some(v.to_string());
                f.old_path = None;
            } else if let Some(v) = line.strip_prefix("deleted file mode ") {
                f.change = FileChange::Deleted;
                f.old_mode = Some(v.to_string());
                f.new_path = None;
            } else if let Some(v) = line.strip_prefix("old mode ") {
                f.old_mode = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("new mode ") {
                f.new_mode = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("rename from ") {
                f.change = FileChange::Renamed;
                f.old_path = Some(unquote(v));
            } else if let Some(v) = line.strip_prefix("rename to ") {
                f.change = FileChange::Renamed;
                f.new_path = Some(unquote(v));
            } else if let Some(v) = line.strip_prefix("copy from ") {
                f.change = FileChange::Copied;
                f.old_path = Some(unquote(v));
            } else if let Some(v) = line.strip_prefix("copy to ") {
                f.change = FileChange::Copied;
                f.new_path = Some(unquote(v));
            } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
                f.binary = true;
            } else if let Some(v) = line.strip_prefix("--- ") {
                let p = parse_marker_path(v);
                if p.is_none() {
                    f.change = FileChange::Added;
                }
                f.old_path = p;
            } else if let Some(v) = line.strip_prefix("+++ ") {
                let p = parse_marker_path(v);
                if p.is_none() {
                    f.change = FileChange::Deleted;
                }
                f.new_path = p;
            }
            continue;
        }
        if let Some(h) = parse_hunk_header(line) {
            old_no = h.old_start;
            new_no = h.new_start;
            f.hunks.push(h);
            continue;
        }
        let Some(h) = f.hunks.last_mut() else { continue };
        let (kind, body) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Add, &line[1..]),
            Some(b'-') => (LineKind::Remove, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            Some(b'\\') => (LineKind::NoNewline, &line[1..]),
            None => {
                // 헝크가 끝난 뒤의 빈 행(출력 끝)은 무시한다. 헝크 안의 빈 문맥 행은 공백 접두가 있다.
                continue;
            }
            _ => continue,
        };
        let (o, n) = match kind {
            LineKind::Context => {
                let r = (Some(old_no), Some(new_no));
                old_no += 1;
                new_no += 1;
                r
            }
            LineKind::Add => {
                let r = (None, Some(new_no));
                new_no += 1;
                r
            }
            LineKind::Remove => {
                let r = (Some(old_no), None);
                old_no += 1;
                r
            }
            LineKind::NoNewline => (None, None),
        };
        h.lines.push(DiffLine { kind, old_no: o, new_no: n, text: body.to_string(), emph: None });
    }
    if let Some(f) = cur.take() {
        files.push(finish(f));
    }
    files
}

fn finish(mut f: FileDiff) -> FileDiff {
    for h in &mut f.hunks {
        compute_word_emphasis(&mut h.lines);
    }
    if f.change == FileChange::Modified
        && f.old_path.is_some()
        && f.new_path.is_some()
        && f.old_path != f.new_path
    {
        f.change = FileChange::Renamed;
    }
    f
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let end = rest.find(" @@")?;
    let ranges = &rest[..end];
    let (old, new) = ranges.split_once(" +")?;
    let parse = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (os, ol) = parse(old)?;
    let (ns, nl) = parse(new)?;
    Some(Hunk {
        header: line.to_string(),
        old_start: os,
        old_lines: ol,
        new_start: ns,
        new_lines: nl,
        lines: Vec::new(),
    })
}

/// `--- a/path` 류 경로를 해석한다. `/dev/null` 이면 `None`.
fn parse_marker_path(v: &str) -> Option<String> {
    let v = v.strip_suffix('\t').unwrap_or(v);
    let v = match v.split_once('\t') {
        Some((p, _)) => p,
        None => v,
    };
    let p = unquote(v);
    if p == "/dev/null" {
        return None;
    }
    Some(strip_ab(&p).to_string())
}

fn strip_ab(p: &str) -> &str {
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p)
}

/// `diff --git a/X b/Y` 의 경로 둘을 나눈다.
fn split_git_header_paths(rest: &str) -> (Option<String>, Option<String>) {
    if rest.starts_with('"') {
        let (a, remain) = take_quoted(rest);
        let b = remain.trim_start();
        let b = if b.starts_with('"') { take_quoted(b).0 } else { b.to_string() };
        return (Some(strip_ab(&a).to_string()), Some(strip_ab(&b).to_string()));
    }
    if let Some(idx) = rest.rfind(" \"b/") {
        let a = &rest[..idx];
        let (b, _) = take_quoted(&rest[idx + 1..]);
        return (Some(strip_ab(a).to_string()), Some(strip_ab(&b).to_string()));
    }
    // 양쪽 경로가 같으면 길이가 대칭이다: "a/" + P + " b/" + P
    let len = rest.len();
    if len >= 7 && (len - 3).is_multiple_of(2) {
        let half = (len - 3) / 2;
        if rest.is_char_boundary(half) && rest.is_char_boundary(half + 1) {
            let a = &rest[..half];
            let b = &rest[half + 1..];
            if strip_ab(a) == strip_ab(b) {
                return (Some(strip_ab(a).to_string()), Some(strip_ab(b).to_string()));
            }
        }
    }
    match rest.find(" b/") {
        Some(i) => (Some(strip_ab(&rest[..i]).to_string()), Some(strip_ab(&rest[i + 1..]).to_string())),
        None => (None, None),
    }
}

fn take_quoted(s: &str) -> (String, &str) {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return (unquote(&s[..=i]), &s[i + 1..]),
            _ => i += 1,
        }
    }
    (unquote(s), "")
}

/// git 의 C 스타일 따옴표 경로를 푼다(`"a\303\251"` → `aé`).
pub fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
        return s.to_string();
    };
    let b = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            match c {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'a' => out.push(7),
                b'b' => out.push(8),
                b'f' => out.push(12),
                b'v' => out.push(11),
                b'0'..=b'7' if i + 3 < b.len() => {
                    let oct = &inner[i + 1..i + 4];
                    if let Ok(v) = u8::from_str_radix(oct, 8) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 연속된 삭제/추가 블록을 짝지어 공통 접두·접미를 뺀 변경 구간을 표시한다.
pub fn compute_word_emphasis(lines: &mut [DiffLine]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Remove {
            i += 1;
            continue;
        }
        let rs = i;
        while i < lines.len() && lines[i].kind == LineKind::Remove {
            i += 1;
        }
        let re = i;
        let as_ = i;
        while i < lines.len() && lines[i].kind == LineKind::Add {
            i += 1;
        }
        let ae = i;
        let n = (re - rs).min(ae - as_);
        for k in 0..n {
            let (old, new) = (&lines[rs + k].text, &lines[as_ + k].text);
            if let Some((o, nn)) = emphasis_ranges(old, new) {
                lines[rs + k].emph = Some(o);
                lines[as_ + k].emph = Some(nn);
            }
        }
    }
}

/// 두 행의 공통 접두·접미(단어 경계로 맞춤)를 뺀 변경 범위. 공통 부분이 거의 없으면 `None`.
fn emphasis_ranges(old: &str, new: &str) -> Option<((usize, usize), (usize, usize))> {
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let max = ob.len().min(nb.len());
    let mut p = 0;
    while p < max && ob[p] == nb[p] {
        p += 1;
    }
    let mut s = 0;
    while s < max - p && ob[ob.len() - 1 - s] == nb[nb.len() - 1 - s] {
        s += 1;
    }
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    // 접두 끝을 단어 시작으로 되돌린다.
    while p > 0 && is_word(ob[p - 1]) && (p < ob.len() && is_word(ob[p]) || p < nb.len() && is_word(nb[p])) {
        p -= 1;
    }
    // 접미 시작을 단어 끝으로 민다.
    while s > 0 {
        let oi = ob.len() - s;
        let ni = nb.len() - s;
        let prev_word = (oi > 0 && is_word(ob[oi - 1])) || (ni > 0 && is_word(nb[ni - 1]));
        if is_word(ob[oi]) && prev_word {
            s -= 1;
        } else {
            break;
        }
    }
    while p > 0 && !(old.is_char_boundary(p) && new.is_char_boundary(p)) {
        p -= 1;
    }
    while s > 0 && !(old.is_char_boundary(ob.len() - s) && new.is_char_boundary(nb.len() - s)) {
        s -= 1;
    }
    let oe = (ob.len() - s).max(p);
    let ne = (nb.len() - s).max(p);
    let common = p + s;
    if common < 3 || common * 3 < max {
        return None;
    }
    if oe == p && ne == p {
        return None;
    }
    Some(((p, oe), (p, ne)))
}

/// 파일 내용 전체를 "추가됨" diff 로 만든다(미추적 파일 표시용).
pub fn synth_added_file(path: &str, content: &[u8]) -> FileDiff {
    let binary = content.iter().take(8000).any(|&b| b == 0);
    let mut f = FileDiff {
        old_path: None,
        new_path: Some(path.to_string()),
        change: FileChange::Added,
        binary,
        header_lines: vec![
            format!("diff --git a/{path} b/{path}"),
            "new file mode 100644".into(),
            "--- /dev/null".into(),
            format!("+++ b/{path}"),
        ],
        hunks: Vec::new(),
        old_mode: None,
        new_mode: Some("100644".into()),
    };
    if binary || content.is_empty() {
        return f;
    }
    let text = String::from_utf8_lossy(content);
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let lines: Vec<DiffLine> = body
        .split('\n')
        .enumerate()
        .map(|(i, l)| DiffLine {
            kind: LineKind::Add,
            old_no: None,
            new_no: Some(i as u32 + 1),
            text: l.strip_suffix('\r').unwrap_or(l).to_string(),
            emph: None,
        })
        .collect();
    let n = lines.len() as u32;
    f.hunks.push(Hunk {
        header: format!("@@ -0,0 +1,{n} @@"),
        old_start: 0,
        old_lines: 0,
        new_start: 1,
        new_lines: n,
        lines,
    });
    f
}
