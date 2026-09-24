//! egui 키 입력을 터미널 바이트 시퀀스로 변환한다(xterm 호환 + kitty 기본 모드).

use egui::{Key, Modifiers};
use kiln_proto::mode;

fn modifier_param(m: Modifiers) -> u8 {
    1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.ctrl as u8)
}

/// 텍스트 이벤트로 오지 않는 키를 인코딩한다. 처리하지 않을 키면 None.
pub fn encode_key(key: Key, m: Modifiers, term_mode: u32, option_as_meta: bool) -> Option<Vec<u8>> {
    let app_cursor = term_mode & mode::APP_CURSOR != 0;
    let kitty = term_mode & mode::KITTY_KEYBOARD != 0;
    let has_mods = m.shift || m.alt || m.ctrl;
    let mp = modifier_param(m);

    let csi_letter = |c: char| -> Vec<u8> {
        if has_mods {
            format!("\x1b[1;{mp}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if has_mods { format!("\x1b[{n};{mp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let ss3 = |c: char| -> Vec<u8> {
        if has_mods { format!("\x1b[1;{mp}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
    };

    let out = match key {
        Key::ArrowUp => csi_letter('A'),
        Key::ArrowDown => csi_letter('B'),
        Key::ArrowRight => csi_letter('C'),
        Key::ArrowLeft => csi_letter('D'),
        Key::Home => csi_letter('H'),
        Key::End => csi_letter('F'),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        Key::Enter => {
            if kitty && has_mods {
                format!("\x1b[13;{mp}u").into_bytes()
            } else if m.shift || m.alt {
                // Shift/Option+Enter: 에이전트 CLI 에서 줄바꿈으로 쓰이는 ESC+CR.
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        Key::Tab => {
            if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }
        }
        Key::Backspace => {
            if kitty && (m.ctrl || m.alt) {
                format!("\x1b[127;{mp}u").into_bytes()
            } else if m.alt {
                b"\x1b\x7f".to_vec()
            } else if m.ctrl {
                b"\x08".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        Key::Escape => {
            if kitty { b"\x1b[27u".to_vec() } else { b"\x1b".to_vec() }
        }
        Key::Space if m.ctrl => vec![0],
        _ => {
            if m.ctrl && !m.mac_cmd {
                return ctrl_key(key, m, kitty, mp);
            }
            if m.alt && option_as_meta {
                if let Some(c) = key_char(key) {
                    let c = if m.shift { c.to_ascii_uppercase() } else { c };
                    return Some(vec![0x1b, c as u8]);
                }
            }
            return None;
        }
    };
    Some(out)
}

fn ctrl_key(key: Key, m: Modifiers, kitty: bool, mp: u8) -> Option<Vec<u8>> {
    let c = key_char(key)?;
    if kitty && (m.shift || m.alt) {
        return Some(format!("\x1b[{};{mp}u", c as u32).into_bytes());
    }
    let b = match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '6' => 0x1e,
        '-' | '/' => 0x1f,
        '2' | '@' => 0,
        _ => return None,
    };
    if m.alt { Some(vec![0x1b, b]) } else { Some(vec![b]) }
}

fn key_char(key: Key) -> Option<char> {
    let name = key.symbol_or_name();
    let mut chars = name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return match key {
            Key::OpenBracket => Some('['),
            Key::CloseBracket => Some(']'),
            Key::Backslash => Some('\\'),
            Key::Minus => Some('-'),
            Key::Slash => Some('/'),
            _ => None,
        };
    }
    Some(c.to_ascii_lowercase())
}

/// 브래킷 붙여넣기 모드를 반영한 붙여넣기 바이트.
pub fn paste_bytes(text: &str, term_mode: u32) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if term_mode & mode::BRACKETED_PASTE != 0 {
        let clean = text.replace("\x1b[201~", "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// SGR(1006) 또는 기본 X10 마우스 보고 시퀀스.
pub fn mouse_report(button: u8, col: u16, row: u16, pressed: bool, m: Modifiers, term_mode: u32) -> Vec<u8> {
    let mut b = button;
    if m.shift {
        b += 4;
    }
    if m.alt {
        b += 8;
    }
    if m.ctrl {
        b += 16;
    }
    if term_mode & mode::SGR_MOUSE != 0 {
        format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if pressed { 'M' } else { 'm' }).into_bytes()
    } else {
        let b = if pressed { b } else { 3 };
        let enc = |v: u16| -> u8 { (32 + v + 1).min(255) as u8 };
        vec![0x1b, b'[', b'M', 32 + b, enc(col), enc(row)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_respect_app_cursor_and_modifiers() {
        assert_eq!(encode_key(Key::ArrowUp, Modifiers::NONE, 0, true).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(Key::ArrowUp, Modifiers::NONE, mode::APP_CURSOR, true).unwrap(), b"\x1bOA");
        assert_eq!(encode_key(Key::ArrowLeft, Modifiers::ALT, 0, true).unwrap(), b"\x1b[1;3D");
    }

    #[test]
    fn ctrl_letter_with_command_flag_is_encoded() {
        // Linux/Windows 에서는 Ctrl 이 command 플래그도 켠다.
        let m = Modifiers { ctrl: true, command: true, ..Default::default() };
        assert_eq!(encode_key(Key::A, m, 0, true).unwrap(), vec![1]);
        let mac_cmd = Modifiers { mac_cmd: true, command: true, ..Default::default() };
        assert!(encode_key(Key::A, mac_cmd, 0, true).is_none());
    }

    #[test]
    fn ctrl_letters_map_to_control_codes() {
        assert_eq!(encode_key(Key::C, Modifiers::CTRL, 0, true).unwrap(), vec![3]);
        assert_eq!(encode_key(Key::OpenBracket, Modifiers::CTRL, 0, true).unwrap(), vec![0x1b]);
    }

    #[test]
    fn shift_enter_sends_esc_cr_and_kitty_csi_u() {
        assert_eq!(encode_key(Key::Enter, Modifiers::SHIFT, 0, true).unwrap(), b"\x1b\r");
        assert_eq!(encode_key(Key::Enter, Modifiers::SHIFT, mode::KITTY_KEYBOARD, true).unwrap(), b"\x1b[13;2u");
    }

    #[test]
    fn option_as_meta_prefixes_escape() {
        assert_eq!(encode_key(Key::B, Modifiers::ALT, 0, true).unwrap(), b"\x1bb");
        assert!(encode_key(Key::B, Modifiers::ALT, 0, false).is_none());
    }

    #[test]
    fn bracketed_paste_wraps_and_normalizes_newlines() {
        assert_eq!(paste_bytes("a\nb", mode::BRACKETED_PASTE), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste_bytes("a\r\nb", 0), b"a\rb");
    }

    #[test]
    fn sgr_mouse_report() {
        assert_eq!(mouse_report(0, 4, 2, true, Modifiers::NONE, mode::SGR_MOUSE), b"\x1b[<0;5;3M");
        assert_eq!(mouse_report(0, 4, 2, false, Modifiers::NONE, mode::SGR_MOUSE), b"\x1b[<0;5;3m");
    }
}
