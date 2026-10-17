//! 출력 바이트 스트림에서 알림(OSC 9/99/777)과 작업 디렉토리(OSC 7)를 뽑아낸다.
//! 청크 경계에 걸친 시퀀스도 처리한다. 바이트 자체는 변경하지 않는다.

#[derive(Debug, PartialEq, Clone)]
pub enum OscEvent {
    Notify { title: String, body: String },
    Cwd(String),
    CommandText(String),
    CommandStart,
    CommandEnd(Option<i32>),
    Prompt,
    Completion(kiln_proto::ShellCompletion),
    CompletionCancelled,
    Activity(kiln_proto::AgentActivity),
}

#[derive(Default)]
pub struct OscScanner {
    state: State,
    buf: Vec<u8>,
    overflow: bool,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

const MAX_OSC: usize = 8192;

impl OscScanner {
    pub fn feed(&mut self, data: &[u8], out: &mut Vec<OscEvent>) {
        for &b in data {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.state = State::Osc;
                        self.buf.clear();
                        self.overflow = false;
                    } else if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::Osc => match b {
                    0x07 => {
                        self.finish(out);
                        self.state = State::Ground;
                    }
                    0x1b => self.state = State::OscEsc,
                    _ => {
                        if self.buf.len() < MAX_OSC {
                            self.buf.push(b);
                        } else { self.overflow = true; }
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(out);
                        self.state = State::Ground;
                    } else if b == b']' {
                        self.state = State::Osc;
                        self.buf.clear();
                        self.overflow = false;
                    } else {
                        self.state = State::Ground;
                    }
                }
            }
        }
    }

    fn finish(&mut self, out: &mut Vec<OscEvent>) {
        if self.overflow { self.buf.clear(); self.overflow = false; return; }
        let s = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        let (code, rest) = match s.split_once(';') {
            Some(x) => x,
            None => return,
        };
        match code {
            "133" | "633" => {
                let (marker, payload) = rest.split_once(';').unwrap_or((rest, ""));
                match marker {
                    "A" => out.push(OscEvent::Prompt),
                    "C" => out.push(OscEvent::CommandStart),
                    "D" => out.push(OscEvent::CommandEnd(payload.split(';').next().and_then(|s| s.parse().ok()))),
                    "E" if code == "633" => out.push(OscEvent::CommandText(payload.split(';').next().unwrap_or("").replace("\\x3b", ";").replace("\\x0a", "\n").replace("\\x5c", "\\"))),
                    _ => {},
                }
            }

            "9" => {
                // OSC 9;4;... 는 ConEmu 진행률 표시다.
                if rest.starts_with("4;") || rest == "4" {
                    return;
                }
                out.push(OscEvent::Notify { title: String::new(), body: rest.to_string() });
            }
            "777" => {
                let mut parts = rest.splitn(3, ';');
                let kind = parts.next();
                if kind == Some("kiln-complete-cancel") {out.push(OscEvent::CompletionCancelled);
                } else if kind == Some("kiln-complete") {
                    if let Some(payload)=rest.strip_prefix("kiln-complete;").and_then(crate::shell_completion::parse){out.push(OscEvent::Completion(payload));}
                } else if kind == Some("kiln-command") {
                    if let Some(hex) = parts.next() {
                        if hex.len() % 2 == 0 && hex.len() <= 8192 {
                            let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|i| hex.get(i..i+2).and_then(|s| u8::from_str_radix(s, 16).ok())).collect();
                            if let Some(bytes) = bytes { out.push(OscEvent::CommandText(String::from_utf8_lossy(&bytes).into_owned())); }
                        }
                    }
                } else if kind == Some("kiln-agent") {
                    use kiln_proto::AgentActivity::*;
                    let state = match parts.next() { Some("running") => Running, Some("waiting") => Waiting, Some("done") => Done, Some("failed") => Failed, Some("unknown") => Unknown, _ => return };
                    out.push(OscEvent::Activity(state));
                } else if kind == Some("notify") {
                    let title = parts.next().unwrap_or("").to_string();
                    let body = parts.next().unwrap_or("").to_string();
                    out.push(OscEvent::Notify { title, body });
                }
            }
            "99" => {
                if let Some((_meta, payload)) = rest.split_once(';') {
                    out.push(OscEvent::Notify { title: String::new(), body: payload.to_string() });
                }
            }
            "7" => {
                if let Some(path) = parse_file_url(rest) {
                    out.push(OscEvent::Cwd(path));
                }
            }
            _ => {}
        }
    }
}

fn parse_file_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let path = percent_decode(&rest[slash..]);
    if cfg!(windows) {
        // `/C:/x` names a drive path; `///server/share` or a host other than this computer
        // names a network share.
        let bytes = path.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            let drive = path[1..].replace('/', "\\");
            return Some(if drive.len() == 2 { format!("{drive}\\") } else { drive });
        }
        if path.starts_with("//") {
            return Some(format!("\\\\{}", path.trim_start_matches('/').replace('/', "\\")));
        }
        let host = &rest[..slash];
        let local = host.is_empty() || host.eq_ignore_ascii_case("localhost") || std::env::var("COMPUTERNAME").is_ok_and(|me| host.eq_ignore_ascii_case(&me));
        if !local {
            return Some(format!("\\\\{host}{}", path.replace('/', "\\")));
        }
    }
    Some(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<OscEvent> {
        let mut s = OscScanner::default();
        let mut out = vec![];
        for c in chunks {
            s.feed(c, &mut out);
        }
        out
    }

    #[test]
    fn osc9_notification_with_bel() {
        assert_eq!(scan(&[b"hi\x1b]9;Claude needs input\x07"]), vec![OscEvent::Notify { title: "".into(), body: "Claude needs input".into() }]);
    }

    #[test]
    fn osc777_split_across_chunks_with_st() {
        let ev = scan(&[b"\x1b]777;noti", b"fy;Codex;Task done\x1b", b"\\rest"]);
        assert_eq!(ev, vec![OscEvent::Notify { title: "Codex".into(), body: "Task done".into() }]);
    }

    #[test]
    fn osc9_progress_is_ignored() {
        assert!(scan(&[b"\x1b]9;4;1;50\x07"]).is_empty());
    }

    #[test]
    fn osc7_cwd_decodes_percent() {
        assert_eq!(scan(&[b"\x1b]7;file://localhost/Users/me/my%20dir\x07"]), vec![OscEvent::Cwd("/Users/me/my dir".into())]);
    }

    #[cfg(unix)]
    #[test]
    fn osc7_on_unix_ignores_the_host() {
        assert_eq!(parse_file_url("file://my-mac.local/Users/me"), Some("/Users/me".into()));
    }

    #[cfg(windows)]
    #[test]
    fn osc7_on_windows_names_drive_and_share_folders() {
        assert_eq!(parse_file_url("file://localhost/C:/Users/me"), Some(r"C:\Users\me".into()));
        assert_eq!(parse_file_url("file://localhost///fileserver/team/proj%20x"), Some(r"\\fileserver\team\proj x".into()));
        assert_eq!(parse_file_url("file://fileserver/team/proj"), Some(r"\\fileserver\team\proj".into()));
        let me = std::env::var("COMPUTERNAME").unwrap();
        assert_eq!(parse_file_url(&format!("file://{me}/C:/x")), Some(r"C:\x".into()));
    }

    #[test]
    fn title_osc_is_not_reported() {
        assert!(scan(&[b"\x1b]0;title\x07\x1b]2;x\x1b\\"]).is_empty());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn file_url_with_a_drive_letter_is_a_windows_path() {
        assert_eq!(parse_file_url("file://host/C:/Users/a%20b").as_deref(), Some(r"C:\Users\a b"));
        assert_eq!(parse_file_url(r"file://localhost/D:\work\proj").as_deref(), Some(r"D:\work\proj"));
        assert_eq!(parse_file_url("file://localhost/C:/").as_deref(), Some(r"C:\"));
    }
}
