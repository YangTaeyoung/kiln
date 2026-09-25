//! Kiln 터미널 데몬. PTY 와 터미널 상태를 GUI 와 분리된 프로세스에서 소유해서
//! 앱이 종료·업데이트되어도 셸과 에이전트(claude, codex 등)가 계속 실행된다.

pub mod client;
pub mod emu;
pub mod images;
pub mod osc;
pub mod procinfo;
pub mod pty;
pub mod ptyhost;
pub mod server;
pub mod transport;

use std::sync::OnceLock;

/// 실행 파일의 크기와 수정 시각으로 만든 빌드 식별자.
pub fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| exe_build_id(&std::env::current_exe().unwrap_or_default()))
}

pub fn exe_build_id(path: &std::path::Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{}-{len:x}-{mtime:x}", env!("CARGO_PKG_VERSION"))
}
