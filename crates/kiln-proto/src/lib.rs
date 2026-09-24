//! Kiln 데몬과 클라이언트(GUI, CLI) 사이의 메시지 정의와 프레이밍.
//!
//! 프레임 형식: 4바이트 little-endian 길이 + postcard 페이로드.
//! enum 변형은 뒤에만 추가한다(postcard 는 변형 인덱스로 인코딩한다).

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const PROTO_VERSION: u32 = 1;
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

pub type SessionId = u64;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct SpawnSpec {
    pub cwd: Option<String>,
    /// 비어 있으면 사용자 기본 셸.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    pub workspace: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMsg {
    Hello { proto: u32, build: String, client: String },
    ListSessions { req: u32 },
    Create { req: u32, spec: SpawnSpec },
    Attach { session: SessionId, cols: u16, rows: u16 },
    Detach { session: SessionId },
    Input { session: SessionId, data: Vec<u8> },
    Resize { session: SessionId, cols: u16, rows: u16 },
    Scroll { session: SessionId, scroll: ScrollTo },
    Kill { session: SessionId },
    Rename { session: SessionId, name: String },
    /// 화면(+스크롤백 `history` 줄)을 평문으로 읽는다.
    ReadText { req: u32, session: SessionId, history: u32 },
    /// 데몬을 `exe` 로 교체한다. 세션은 유지된다(Unix).
    Upgrade { req: u32, exe: String },
    Shutdown,
    Ping { req: u32 },
    ClearAttention { session: SessionId },
    Search { req: u32, session: SessionId, query: String, backward: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum ScrollTo {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMsg {
    Hello { proto: u32, build: String, pid: u32, can_upgrade: bool },
    Sessions { req: u32, sessions: Vec<SessionInfo> },
    Created { req: u32, session: SessionId },
    Frame(Frame),
    SessionUpdated(SessionInfo),
    SessionExited { session: SessionId, code: Option<i32> },
    Notification { session: SessionId, title: String, body: String },
    Clipboard { session: SessionId, text: String },
    Text { req: u32, text: String },
    Error { req: u32, message: String },
    Pong { req: u32 },
    Upgrading,
    SearchResult { req: u32, found: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct SessionInfo {
    pub id: SessionId,
    pub pid: u32,
    pub name: Option<String>,
    pub title: String,
    pub cwd: Option<String>,
    /// 포그라운드 프로세스 이름 (예: "claude", "vim").
    pub fg_process: Option<String>,
    pub workspace: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub exited: Option<i32>,
    pub created_unix: u64,
    /// 벨/알림이 와서 사용자 확인이 필요한 상태.
    pub attention: bool,
    pub last_notification: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    DefaultFg,
    DefaultBg,
    Idx(u8),
    Rgb(u8, u8, u8),
}

pub mod flags {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 1 << 1;
    pub const UNDERLINE: u16 = 1 << 2;
    pub const INVERSE: u16 = 1 << 3;
    pub const DIM: u16 = 1 << 4;
    pub const HIDDEN: u16 = 1 << 5;
    pub const STRIKE: u16 = 1 << 6;
    /// 두 칸짜리 문자의 첫 칸.
    pub const WIDE: u16 = 1 << 7;
    /// 두 칸짜리 문자의 두 번째(빈) 칸.
    pub const SPACER: u16 = 1 << 8;
    pub const UNDERCURL: u16 = 1 << 9;
    pub const WRAP: u16 = 1 << 10;
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { c: ' ', fg: Color::DefaultFg, bg: Color::DefaultBg, flags: 0 }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// 결합 문자(zero-width) — (열, 문자열).
    pub combining: Vec<(u16, String)>,
}

impl Line {
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            if c.flags & flags::SPACER != 0 {
                continue;
            }
            s.push(c.c);
            for (col, z) in &self.combining {
                if *col as usize == i {
                    s.push_str(z);
                }
            }
        }
        s
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Hidden,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
}

pub mod mode {
    pub const APP_CURSOR: u32 = 1;
    pub const APP_KEYPAD: u32 = 1 << 1;
    pub const MOUSE_CLICK: u32 = 1 << 2;
    pub const MOUSE_DRAG: u32 = 1 << 3;
    pub const MOUSE_MOTION: u32 = 1 << 4;
    pub const SGR_MOUSE: u32 = 1 << 5;
    pub const BRACKETED_PASTE: u32 = 1 << 6;
    pub const FOCUS_EVENTS: u32 = 1 << 7;
    pub const ALT_SCREEN: u32 = 1 << 8;
    pub const ALT_SCROLL: u32 = 1 << 9;
    pub const UTF8_MOUSE: u32 = 1 << 10;
    pub const KITTY_KEYBOARD: u32 = 1 << 11;
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Frame {
    pub session: SessionId,
    pub cols: u16,
    pub rows: u16,
    /// true 면 `lines` 가 화면 전체를 담는다.
    pub full: bool,
    pub lines: Vec<(u16, Line)>,
    pub cursor: Option<Cursor>,
    pub mode: u32,
    pub display_offset: u32,
    pub history: u32,
}

/// 메시지 하나를 길이 접두 프레임으로 쓴다.
pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> std::io::Result<()> {
    let payload = postcard::to_stdvec(msg).map_err(std::io::Error::other)?;
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&len)?;
    w.write_all(&payload)?;
    w.flush()
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let payload = postcard::to_stdvec(msg).expect("encode");
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// 프레임 하나를 읽는다. EOF 면 `Ok(None)`.
pub fn read_msg<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("frame too large: {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    postcard::from_bytes(&buf).map(Some).map_err(std::io::Error::other)
}

/// 소켓 이름. Unix 는 짧은 경로의 소켓 파일, Windows 는 네임드 파이프 이름.
pub fn socket_name() -> String {
    if let Ok(s) = std::env::var("KILN_SOCKET") {
        return s;
    }
    #[cfg(unix)]
    {
        let uid = unsafe_uid();
        let dir = std::env::temp_dir();
        let dir = if dir.to_string_lossy().len() > 60 { std::path::PathBuf::from("/tmp") } else { dir };
        dir.join(format!("kiln-{uid}")).join("daemon.sock").to_string_lossy().into_owned()
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
        format!("kiln-daemon-{user}")
    }
}

#[cfg(unix)]
fn unsafe_uid() -> u32 {
    // SAFETY: getuid 는 부작용이 없다.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = ServerMsg::Frame(Frame {
            session: 7,
            cols: 3,
            rows: 1,
            full: true,
            lines: vec![(0, Line { cells: vec![Cell { c: '한', fg: Color::Idx(2), bg: Color::Rgb(1, 2, 3), flags: flags::WIDE }, Cell { c: ' ', flags: flags::SPACER, ..Default::default() }, Cell::default()], combining: vec![] })],
            cursor: Some(Cursor { col: 1, row: 0, shape: CursorShape::Beam }),
            mode: mode::APP_CURSOR,
            display_offset: 0,
            history: 10,
        });
        let bytes = encode(&f);
        let back: ServerMsg = read_msg(&mut &bytes[..]).unwrap().unwrap();
        match back {
            ServerMsg::Frame(fr) => {
                assert_eq!(fr.lines[0].1.text(), "한 ");
                assert_eq!(fr.cursor.unwrap().shape, CursorShape::Beam);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn read_msg_returns_none_on_eof() {
        let empty: &[u8] = &[];
        let r: Option<ClientMsg> = read_msg(&mut &empty[..]).unwrap();
        assert!(r.is_none());
    }
}
