//! 테스트용 임시 git 저장소 헬퍼.
#![allow(dead_code)]

pub mod fake_gh;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BASE_TS: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z

/// 결정적인 작성자/날짜로 커밋하는 임시 저장소.
pub struct Repo {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    tick: Cell<i64>,
}

impl Repo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        let r = Repo { _dir: dir, path, tick: Cell::new(0) };
        r.git(&["init", "-q", "-b", "main"]);
        r.configure();
        r
    }

    /// 기존 경로(예: clone 결과)를 감싼다.
    pub fn at(path: PathBuf) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = Repo { _dir: dir, path, tick: Cell::new(100) };
        r.configure();
        r
    }

    fn configure(&self) {
        for (k, v) in [
            ("user.name", "Kiln Tester"),
            ("user.email", "tester@kiln.dev"),
            ("commit.gpgsign", "false"),
            ("tag.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("pull.rebase", "false"),
            ("protocol.file.allow", "always"),
        ] {
            self.git(&["config", k, v]);
        }
    }

    pub fn tempdir(&self) -> &Path {
        self._dir.path()
    }

    /// 다음 커밋 시각(1시간 간격).
    pub fn next_ts(&self) -> i64 {
        let n = self.tick.get() + 1;
        self.tick.set(n);
        BASE_TS + n * 3600
    }

    pub fn git_in(dir: &Path, args: &[&str], ts: i64) -> String {
        let date = format!("@{ts} +0000");
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_AUTHOR_NAME", "Kiln Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@kiln.dev")
            .env("GIT_COMMITTER_NAME", "Kiln Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@kiln.dev")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let ts = BASE_TS + self.tick.get() * 3600;
        Self::git_in(&self.path, args, ts)
    }

    /// 실패해도 패닉하지 않는 git 실행.
    pub fn git_status(&self, args: &[&str]) -> bool {
        Command::new("git").current_dir(&self.path).args(args).output().map(|o| o.status.success()).unwrap_or(false)
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.path.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path.join(rel)).unwrap()
    }

    pub fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        let ts = self.next_ts();
        Self::git_in(&self.path, &["commit", "-q", "-m", msg], ts);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }
}

/// 여러 줄 텍스트 파일 내용(1..=n).
pub fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// 한글 글리프가 있는 시스템 폰트를 Proportional·Monospace 대체 폰트로 컨텍스트마다 한 번 설치한다.
pub fn install_korean_font(ctx: &egui::Context) {
    // 번들 글꼴(Pretendard·JetBrains Mono)을 한 번만 설치한다.
    let flag = egui::Id::new("kiln-test-korean-font");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    ctx.set_fonts(kiln_common::fonts::definitions(false));
    ctx.request_repaint();
}

/// 번들 글꼴 가족(semibold)이 이번 프레임에 쓸 수 있는지.
pub fn fonts_ready(ctx: &egui::Context) -> bool {
    let fam = egui::FontFamily::Name(kiln_common::fonts::SEMIBOLD.into());
    ctx.fonts(|f| f.families().contains(&fam))
}
