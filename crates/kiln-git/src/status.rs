//! `git status --porcelain=v2 --branch -z` 파싱.

use std::path::PathBuf;

/// 브랜치 상태 헤더.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchStatus {
    /// 현재 브랜치 이름. detached 면 `None`.
    pub head: Option<String>,
    /// HEAD 커밋. 첫 커밋 전이면 `None`.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

impl BranchStatus {
    pub fn is_detached(&self) -> bool {
        self.head.is_none()
    }

    /// 표시용 이름. detached 면 짧은 SHA 를 쓴다.
    pub fn display_name(&self) -> String {
        match (&self.head, &self.oid) {
            (Some(h), _) => h.clone(),
            (None, Some(o)) => format!("({})", &o[..o.len().min(8)]),
            (None, None) => "(브랜치 없음)".into(),
        }
    }
}

/// 파일 한 건의 상태 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Ordinary,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
}

/// 상태 파일 한 건.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    /// 저장소 루트 기준 상대 경로('/' 구분).
    pub path: String,
    /// rename/copy 원본 경로.
    pub orig_path: Option<String>,
    /// 인덱스 상태 문자('.' = 변경 없음).
    pub index: char,
    /// 작업트리 상태 문자('.' = 변경 없음).
    pub worktree: char,
    pub kind: EntryKind,
    pub submodule: bool,
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        matches!(self.kind, EntryKind::Ordinary | EntryKind::Renamed | EntryKind::Copied) && self.index != '.'
    }

    pub fn is_unstaged(&self) -> bool {
        matches!(self.kind, EntryKind::Ordinary | EntryKind::Renamed | EntryKind::Copied) && self.worktree != '.'
    }

    pub fn is_conflicted(&self) -> bool {
        self.kind == EntryKind::Unmerged
    }

    pub fn is_untracked(&self) -> bool {
        self.kind == EntryKind::Untracked
    }

    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    /// 충돌 종류 설명(`양쪽 수정` 등).
    pub fn conflict_label(&self) -> &'static str {
        match (self.index, self.worktree) {
            ('D', 'D') => "양쪽 삭제",
            ('A', 'U') => "현재 쪽 추가",
            ('U', 'D') => "상대 쪽 삭제",
            ('U', 'A') => "상대 쪽 추가",
            ('D', 'U') => "현재 쪽 삭제",
            ('A', 'A') => "양쪽 추가",
            _ => "양쪽 수정",
        }
    }
}

/// 파싱된 `git status` 결과.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: BranchStatus,
    pub entries: Vec<StatusEntry>,
    /// `# stash <n>` 헤더 값.
    pub stash_count: u32,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_staged())
    }
    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_unstaged())
    }
    pub fn conflicted(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_conflicted())
    }
    pub fn untracked(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_untracked())
    }

    /// 변경 파일 수(충돌 제외, 무시 파일 제외).
    pub fn changed_count(&self) -> u32 {
        self.entries
            .iter()
            .filter(|e| !matches!(e.kind, EntryKind::Unmerged | EntryKind::Ignored))
            .count() as u32
    }

    pub fn conflicted_count(&self) -> u32 {
        self.conflicted().count() as u32
    }
}

/// porcelain v2 + `-z` 출력을 파싱한다.
pub fn parse_status_v2(raw: &[u8]) -> Status {
    let mut st = Status::default();
    let text = String::from_utf8_lossy(raw);
    let mut recs = text.split('\0').peekable();
    while let Some(rec) = recs.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(h) = rec.strip_prefix("# ") {
            parse_header(&mut st, h);
            continue;
        }
        let tag = rec.as_bytes()[0];
        match tag {
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() == 9 {
                    st.entries.push(entry(f[1], f[2], f[8].to_string(), None, EntryKind::Ordinary));
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path \0 origPath
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = recs.next().map(|s| s.to_string());
                if f.len() == 10 {
                    let kind = if f[8].starts_with('C') { EntryKind::Copied } else { EntryKind::Renamed };
                    st.entries.push(entry(f[1], f[2], f[9].to_string(), orig, kind));
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    st.entries.push(entry(f[1], f[2], f[10].to_string(), None, EntryKind::Unmerged));
                }
            }
            b'?' => {
                if let Some(p) = rec.get(2..) {
                    st.entries.push(StatusEntry {
                        path: p.to_string(),
                        orig_path: None,
                        index: '?',
                        worktree: '?',
                        kind: EntryKind::Untracked,
                        submodule: false,
                    });
                }
            }
            b'!' => {
                if let Some(p) = rec.get(2..) {
                    st.entries.push(StatusEntry {
                        path: p.to_string(),
                        orig_path: None,
                        index: '!',
                        worktree: '!',
                        kind: EntryKind::Ignored,
                        submodule: false,
                    });
                }
            }
            _ => {}
        }
    }
    st
}

fn entry(xy: &str, sub: &str, path: String, orig_path: Option<String>, kind: EntryKind) -> StatusEntry {
    let mut c = xy.chars();
    StatusEntry {
        path,
        orig_path,
        index: c.next().unwrap_or('.'),
        worktree: c.next().unwrap_or('.'),
        kind,
        submodule: sub.starts_with('S'),
    }
}

fn parse_header(st: &mut Status, h: &str) {
    if let Some(v) = h.strip_prefix("branch.oid ") {
        st.branch.oid = (v != "(initial)").then(|| v.to_string());
    } else if let Some(v) = h.strip_prefix("branch.head ") {
        st.branch.head = (v != "(detached)").then(|| v.to_string());
    } else if let Some(v) = h.strip_prefix("branch.upstream ") {
        st.branch.upstream = Some(v.to_string());
    } else if let Some(v) = h.strip_prefix("branch.ab ") {
        for part in v.split_whitespace() {
            if let Some(n) = part.strip_prefix('+') {
                st.branch.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix('-') {
                st.branch.behind = n.parse().unwrap_or(0);
            }
        }
    } else if let Some(v) = h.strip_prefix("stash ") {
        st.stash_count = v.trim().parse().unwrap_or(0);
    }
}

/// 파일 트리 장식에 쓰는 대표 상태 문자.
///
/// 충돌 `C`, 미추적 `?`, 추가 `A`, 삭제 `D`, 이름변경 `R`, 그 외 `M`.
pub fn decoration_char(e: &StatusEntry) -> char {
    match e.kind {
        EntryKind::Unmerged => 'C',
        EntryKind::Untracked => '?',
        EntryKind::Ignored => '!',
        _ => {
            let w = e.worktree;
            let i = e.index;
            if w == 'D' || i == 'D' {
                'D'
            } else if i == 'A' {
                'A'
            } else if matches!(e.kind, EntryKind::Renamed | EntryKind::Copied) && w == '.' {
                'R'
            } else {
                'M'
            }
        }
    }
}
