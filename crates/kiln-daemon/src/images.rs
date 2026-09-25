//! 터미널 이미지 프로토콜: iTerm2 인라인 이미지(OSC 1337), kitty 그래픽(APC G), sixel(DCS q).
//!
//! `ImageScanner` 는 출력 바이트를 구간으로 나눠, 이미지 시퀀스가 끝나는 지점마다 `Segment::Image` 를 끼운다.
//! 바이트 자체는 그대로 에뮬레이터로 흘려보낸다(에뮬레이터는 이 시퀀스들을 무시한다).

use base64::Engine;
use std::collections::HashMap;

const MAX_SEQ: usize = 96 * 1024 * 1024;

#[derive(Debug)]
pub enum Segment {
    Bytes(usize, usize),
    Image(ImageCmd),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImageCmd {
    /// OSC 1337 의 `File=` / `MultipartFile=` 이후 내용(인자와 페이로드).
    Iterm(Vec<u8>),
    /// APC `G` 이후 내용(제어 데이터 ; base64).
    Kitty(Vec<u8>),
    /// DCS 매개변수와 sixel 데이터(`q` 이후).
    Sixel { params: Vec<u8>, data: Vec<u8> },
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum St {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
    Apc,
    ApcEsc,
    Dcs,
    DcsEsc,
}

#[derive(Default)]
pub struct ImageScanner {
    st: St,
    buf: Vec<u8>,
    /// 버퍼링 대상 시퀀스인지(이미지 후보만 모은다).
    capture: bool,
}

impl ImageScanner {
    pub fn feed(&mut self, data: &[u8]) -> Vec<Segment> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, &b) in data.iter().enumerate() {
            let done = self.step(b);
            if let Some(cmd) = done {
                out.push(Segment::Bytes(start, i + 1));
                out.push(Segment::Image(cmd));
                start = i + 1;
            }
        }
        if start < data.len() {
            out.push(Segment::Bytes(start, data.len()));
        }
        out
    }

    fn push(&mut self, b: u8) {
        if self.capture && self.buf.len() < MAX_SEQ {
            self.buf.push(b);
        }
    }

    fn step(&mut self, b: u8) -> Option<ImageCmd> {
        match self.st {
            St::Ground => {
                if b == 0x1b {
                    self.st = St::Esc;
                }
                None
            }
            St::Esc => {
                self.buf.clear();
                self.capture = true;
                self.st = match b {
                    b']' => St::Osc,
                    b'_' => St::Apc,
                    b'P' => St::Dcs,
                    0x1b => St::Esc,
                    _ => St::Ground,
                };
                None
            }
            St::Osc => {
                match b {
                    0x07 => {
                        self.st = St::Ground;
                        return self.finish_osc();
                    }
                    0x1b => self.st = St::OscEsc,
                    _ => {
                        self.push(b);
                        // 앞부분만 보고 이미지가 아니면 버퍼링을 멈춘다.
                        if self.capture && self.buf.len() == 5 && &self.buf[..] != b"1337;" {
                            self.capture = false;
                        }
                    }
                }
                None
            }
            St::OscEsc => {
                if b == b'\\' {
                    self.st = St::Ground;
                    return self.finish_osc();
                }
                self.st = if b == 0x1b { St::Esc } else { St::Ground };
                None
            }
            St::Apc => {
                if b == 0x1b {
                    self.st = St::ApcEsc;
                } else {
                    self.push(b);
                    if self.capture && self.buf.len() == 1 && self.buf[0] != b'G' {
                        self.capture = false;
                    }
                }
                None
            }
            St::ApcEsc => {
                if b == b'\\' {
                    self.st = St::Ground;
                    if self.capture && self.buf.first() == Some(&b'G') {
                        return Some(ImageCmd::Kitty(self.buf[1..].to_vec()));
                    }
                    return None;
                }
                self.st = if b == 0x1b { St::Esc } else { St::Ground };
                None
            }
            St::Dcs => {
                if b == 0x1b {
                    self.st = St::DcsEsc;
                } else {
                    self.push(b);
                }
                None
            }
            St::DcsEsc => {
                if b == b'\\' {
                    self.st = St::Ground;
                    return self.finish_dcs();
                }
                self.st = if b == 0x1b { St::Esc } else { St::Ground };
                None
            }
        }
    }

    fn finish_osc(&mut self) -> Option<ImageCmd> {
        if !self.capture || !self.buf.starts_with(b"1337;") {
            return None;
        }
        let rest = &self.buf[5..];
        let keys: [&[u8]; 4] = [b"File=", b"MultipartFile=", b"FilePart=", b"FileEnd"];
        if keys.iter().any(|k| rest.starts_with(k)) {
            return Some(ImageCmd::Iterm(rest.to_vec()));
        }
        None
    }

    fn finish_dcs(&mut self) -> Option<ImageCmd> {
        if !self.capture {
            return None;
        }
        let q = self.buf.iter().position(|&c| c == b'q')?;
        if !self.buf[..q].iter().all(|c| c.is_ascii_digit() || *c == b';') {
            return None;
        }
        Some(ImageCmd::Sixel { params: self.buf[..q].to_vec(), data: self.buf[q + 1..].to_vec() })
    }
}

/// 디코딩된 이미지와 표시 크기(셀).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
    pub cursor: CursorMove,
}

/// 이미지를 표시한 뒤 커서 위치.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CursorMove {
    /// 이미지 아래 줄의 첫 열(iTerm2, sixel).
    Below,
    /// 이미지 마지막 줄의 오른쪽(kitty 기본).
    RightOfLastRow,
    /// 움직이지 않음(kitty `C=1`).
    Stay,
}

/// 이미지 명령 처리 결과.
pub enum Action {
    None,
    Show(Decoded),
    /// PTY 로 되돌려 보낼 응답(kitty 질의 응답 등).
    Reply(Vec<u8>),
    ShowAndReply(Decoded, Vec<u8>),
    /// kitty `a=d`: 표시 중인 이미지 삭제.
    DeleteAll,
}

/// 여러 청크로 나뉘어 오는 이미지를 모으는 상태.
#[derive(Default)]
pub struct ImageState {
    kitty_chunks: Option<(HashMap<String, String>, Vec<u8>)>,
    kitty_stored: HashMap<u32, (HashMap<String, String>, Vec<u8>)>,
    iterm_multi: Option<(HashMap<String, String>, Vec<u8>)>,
}

fn b64(data: &[u8]) -> Option<Vec<u8>> {
    let clean: Vec<u8> = data.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean))
        .ok()
}

fn parse_kv(s: &str, sep: char) -> HashMap<String, String> {
    s.split(sep)
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

/// 픽셀 크기와 요청(셀/픽셀/퍼센트/auto)을 셀 수로 바꾼다.
fn cell_extent(req: Option<&str>, px: u32, cell_px: u32, max_cells: u16) -> Option<u16> {
    let cell_px = cell_px.max(1);
    let n = match req {
        None | Some("auto") | Some("") => px.div_ceil(cell_px),
        Some(v) if v.ends_with("px") => v.trim_end_matches("px").parse::<u32>().ok()?.div_ceil(cell_px),
        Some(v) if v.ends_with('%') => (v.trim_end_matches('%').parse::<u32>().ok()? * max_cells as u32) / 100,
        Some(v) => v.parse::<u32>().ok()?,
    };
    Some(n.clamp(1, 1000) as u16)
}

pub struct Geometry {
    pub cell_w: u32,
    pub cell_h: u32,
    pub cols: u16,
    pub rows: u16,
}

/// 표시 크기(셀)를 정하고 필요하면 비율을 유지하도록 한 축을 계산한다.
fn fit(img: &image::RgbaImage, w_req: Option<&str>, h_req: Option<&str>, keep_aspect: bool, g: &Geometry) -> (u16, u16) {
    let (pw, ph) = (img.width(), img.height());
    let max_cols = g.cols.max(1);
    let mut cols = cell_extent(w_req, pw, g.cell_w, max_cols).unwrap_or(1);
    let mut rows = cell_extent(h_req, ph, g.cell_h, g.rows.max(1)).unwrap_or(1);
    let w_auto = w_req.is_none_or(|v| v == "auto");
    let h_auto = h_req.is_none_or(|v| v == "auto");
    if cols > max_cols {
        cols = max_cols;
        if keep_aspect && h_auto {
            let disp_w = cols as f32 * g.cell_w as f32;
            rows = ((disp_w * ph as f32 / pw.max(1) as f32) / g.cell_h as f32).ceil().max(1.0) as u16;
        }
    }
    if keep_aspect && w_auto && !h_auto {
        let disp_h = rows as f32 * g.cell_h as f32;
        cols = ((disp_h * pw as f32 / ph.max(1) as f32) / g.cell_w as f32).ceil().clamp(1.0, max_cols as f32) as u16;
    } else if keep_aspect && h_auto && !w_auto {
        let disp_w = cols as f32 * g.cell_w as f32;
        rows = ((disp_w * ph as f32 / pw.max(1) as f32) / g.cell_h as f32).ceil().max(1.0) as u16;
    }
    (cols, rows)
}

/// 표시 크기보다 훨씬 큰 이미지는 줄여서 전송량을 줄인다.
fn finalize(img: image::RgbaImage, cols: u16, rows: u16, g: &Geometry, cursor: CursorMove) -> Decoded {
    let target_w = (cols as u32 * g.cell_w * 2).max(1);
    let target_h = (rows as u32 * g.cell_h * 2).max(1);
    let img = if img.width() > target_w || img.height() > target_h {
        image::imageops::resize(&img, target_w.min(img.width()), target_h.min(img.height()), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Decoded { width: img.width(), height: img.height(), rgba: img.into_raw(), cols, rows, cursor }
}

impl ImageState {
    pub fn handle(&mut self, cmd: ImageCmd, g: &Geometry) -> Action {
        match cmd {
            ImageCmd::Iterm(body) => self.iterm(&body, g),
            ImageCmd::Kitty(body) => self.kitty(&body, g),
            ImageCmd::Sixel { params, data } => match decode_sixel(&params, &data) {
                Some(img) => {
                    let cols = img.width().div_ceil(g.cell_w.max(1)).clamp(1, g.cols.max(1) as u32) as u16;
                    let rows = img.height().div_ceil(g.cell_h.max(1)).max(1) as u16;
                    Action::Show(finalize(img, cols, rows, g, CursorMove::Below))
                }
                None => Action::None,
            },
        }
    }

    fn iterm(&mut self, body: &[u8], g: &Geometry) -> Action {
        let text = String::from_utf8_lossy(body);
        let show = |args: &HashMap<String, String>, bytes: &[u8]| -> Action {
            if args.get("inline").map(String::as_str) != Some("1") {
                return Action::None;
            }
            let Ok(img) = image::load_from_memory(bytes) else { return Action::None };
            let img = img.to_rgba8();
            let keep = args.get("preserveAspectRatio").map(String::as_str) != Some("0");
            let (cols, rows) = fit(&img, args.get("width").map(String::as_str), args.get("height").map(String::as_str), keep, g);
            Action::Show(finalize(img, cols, rows, g, CursorMove::Below))
        };
        if let Some(rest) = text.strip_prefix("File=") {
            let Some((args, payload)) = rest.split_once(':') else { return Action::None };
            let args = parse_kv(args, ';');
            let Some(bytes) = b64(payload.as_bytes()) else { return Action::None };
            return show(&args, &bytes);
        }
        if let Some(args) = text.strip_prefix("MultipartFile=") {
            self.iterm_multi = Some((parse_kv(args, ';'), Vec::new()));
            return Action::None;
        }
        if let Some(chunk) = text.strip_prefix("FilePart=") {
            if let Some((_, buf)) = &mut self.iterm_multi {
                buf.extend_from_slice(chunk.as_bytes());
            }
            return Action::None;
        }
        if text.starts_with("FileEnd") {
            if let Some((args, buf)) = self.iterm_multi.take() {
                if let Some(bytes) = b64(&buf) {
                    return show(&args, &bytes);
                }
            }
        }
        Action::None
    }

    fn kitty(&mut self, body: &[u8], g: &Geometry) -> Action {
        let text = String::from_utf8_lossy(body);
        let (ctrl, payload) = text.split_once(';').unwrap_or((&text, ""));
        let mut args = parse_kv(ctrl, ',');
        // 청크 전송: 첫 청크의 제어 데이터를 유지하고 페이로드를 이어 붙인다.
        let more = args.get("m").map(String::as_str) == Some("1");
        if let Some((first, buf)) = &mut self.kitty_chunks {
            buf.extend_from_slice(payload.as_bytes());
            if more {
                return Action::None;
            }
            let (first, buf) = (first.clone(), std::mem::take(buf));
            self.kitty_chunks = None;
            args = first;
            return self.kitty_complete(args, &buf, g);
        }
        if more {
            self.kitty_chunks = Some((args, payload.as_bytes().to_vec()));
            return Action::None;
        }
        self.kitty_complete(args, payload.as_bytes(), g)
    }

    fn kitty_complete(&mut self, args: HashMap<String, String>, payload: &[u8], g: &Geometry) -> Action {
        let action = args.get("a").map(String::as_str).unwrap_or("t");
        let id: u32 = args.get("i").and_then(|v| v.parse().ok()).unwrap_or(0);
        let quiet: u8 = args.get("q").and_then(|v| v.parse().ok()).unwrap_or(0);
        let reply = |ok: &str| -> Vec<u8> {
            if id == 0 || quiet >= 1 && ok == "OK" || quiet >= 2 {
                Vec::new()
            } else {
                format!("\x1b_Gi={id};{ok}\x1b\\").into_bytes()
            }
        };
        match action {
            "q" => return Action::Reply(reply("OK")),
            "d" => return Action::DeleteAll,
            "t" => {
                if id != 0 {
                    self.kitty_stored.insert(id, (args.clone(), payload.to_vec()));
                }
                let r = reply("OK");
                return if r.is_empty() { Action::None } else { Action::Reply(r) };
            }
            "p" => {
                let Some((stored_args, data)) = self.kitty_stored.get(&id).cloned() else {
                    return Action::Reply(reply("ENOENT:no such image"));
                };
                let mut merged = stored_args;
                for (k, v) in args {
                    merged.insert(k, v);
                }
                return self.kitty_show(&merged, &data, g, id, quiet);
            }
            "T" => {
                if id != 0 {
                    self.kitty_stored.insert(id, (args.clone(), payload.to_vec()));
                }
                self.kitty_show(&args, payload, g, id, quiet)
            }
            _ => Action::None,
        }
    }

    fn kitty_show(&self, args: &HashMap<String, String>, payload: &[u8], g: &Geometry, id: u32, quiet: u8) -> Action {
        let err = |m: &str| -> Action {
            if id != 0 && quiet < 2 { Action::Reply(format!("\x1b_Gi={id};{m}\x1b\\").into_bytes()) } else { Action::None }
        };
        if args.get("t").is_some_and(|t| t != "d") {
            return err("EINVAL:only direct transmission is supported");
        }
        let Some(raw) = b64(payload) else { return err("EINVAL:bad base64") };
        let raw = if args.get("o").map(String::as_str) == Some("z") {
            match miniz_oxide::inflate::decompress_to_vec_zlib(&raw) {
                Ok(v) => v,
                Err(_) => return err("EINVAL:bad zlib"),
            }
        } else {
            raw
        };
        let fmt: u32 = args.get("f").and_then(|v| v.parse().ok()).unwrap_or(32);
        let w: u32 = args.get("s").and_then(|v| v.parse().ok()).unwrap_or(0);
        let h: u32 = args.get("v").and_then(|v| v.parse().ok()).unwrap_or(0);
        let img = match fmt {
            100 => match image::load_from_memory(&raw) {
                Ok(i) => i.to_rgba8(),
                Err(_) => return err("EINVAL:bad png"),
            },
            32 => match image::RgbaImage::from_raw(w, h, raw) {
                Some(i) => i,
                None => return err("EINVAL:size mismatch"),
            },
            24 => {
                if raw.len() != (w * h * 3) as usize {
                    return err("EINVAL:size mismatch");
                }
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for px in raw.chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                match image::RgbaImage::from_raw(w, h, rgba) {
                    Some(i) => i,
                    None => return err("EINVAL:size mismatch"),
                }
            }
            _ => return err("EINVAL:unsupported format"),
        };
        let c = args.get("c").map(String::as_str);
        let r = args.get("r").map(String::as_str);
        let (cols, rows) = fit(&img, c, r, true, g);
        let cursor = if args.get("C").map(String::as_str) == Some("1") { CursorMove::Stay } else { CursorMove::RightOfLastRow };
        let dec = finalize(img, cols, rows, g, cursor);
        let ok = if id != 0 && quiet == 0 { format!("\x1b_Gi={id};OK\x1b\\").into_bytes() } else { Vec::new() };
        if ok.is_empty() { Action::Show(dec) } else { Action::ShowAndReply(dec, ok) }
    }
}

/// sixel 데이터를 RGBA 이미지로 디코딩한다.
pub fn decode_sixel(params: &[u8], data: &[u8]) -> Option<image::RgbaImage> {
    let p: Vec<u32> = String::from_utf8_lossy(params).split(';').filter_map(|v| v.parse().ok()).collect();
    // P2 = 1 이면 0 비트 픽셀은 투명.
    let transparent_bg = p.get(1) == Some(&1);
    let mut palette: Vec<[u8; 4]> = default_palette();
    let mut color = 0usize;
    let (mut x, mut y) = (0usize, 0usize);
    let (mut w, mut h) = (0usize, 0usize);
    let mut pixels: HashMap<(usize, usize), [u8; 4]> = HashMap::new();
    let mut declared: Option<(usize, usize)> = None;
    let mut i = 0;
    let num = |i: &mut usize| -> Vec<u32> {
        let mut vals = Vec::new();
        let mut cur: Option<u32> = None;
        while *i < data.len() {
            let c = data[*i];
            if c.is_ascii_digit() {
                cur = Some(cur.unwrap_or(0).saturating_mul(10).saturating_add((c - b'0') as u32));
            } else if c == b';' {
                vals.push(cur.take().unwrap_or(0));
            } else {
                break;
            }
            *i += 1;
        }
        if let Some(v) = cur {
            vals.push(v);
        }
        vals
    };
    let put = |x: usize, y: usize, bits: u8, rep: usize, color: [u8; 4], pixels: &mut HashMap<(usize, usize), [u8; 4]>, w: &mut usize, h: &mut usize| {
        for dx in 0..rep {
            for bit in 0..6 {
                if bits & (1 << bit) != 0 {
                    pixels.insert((x + dx, y + bit), color);
                    *h = (*h).max(y + bit + 1);
                }
            }
            *w = (*w).max(x + dx + 1);
        }
    };
    while i < data.len() {
        let c = data[i];
        match c {
            b'"' => {
                i += 1;
                let v = num(&mut i);
                if v.len() >= 4 {
                    declared = Some((v[2] as usize, v[3] as usize));
                }
            }
            b'#' => {
                i += 1;
                let v = num(&mut i);
                let idx = *v.first()? as usize;
                if idx >= 1024 {
                    return None;
                }
                if palette.len() <= idx {
                    palette.resize(idx + 1, [0, 0, 0, 255]);
                }
                if v.len() >= 5 {
                    palette[idx] = if v[1] == 1 { hls(v[2], v[3], v[4]) } else { [pct(v[2]), pct(v[3]), pct(v[4]), 255] };
                }
                color = idx;
            }
            b'!' => {
                i += 1;
                let v = num(&mut i);
                let rep = (*v.first().unwrap_or(&1) as usize).clamp(1, 10_000);
                if i < data.len() && (0x3f..=0x7e).contains(&data[i]) {
                    put(x, y, data[i] - 0x3f, rep, palette[color.min(palette.len() - 1)], &mut pixels, &mut w, &mut h);
                    x += rep;
                    i += 1;
                }
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                x = 0;
                y += 6;
                i += 1;
            }
            0x3f..=0x7e => {
                put(x, y, c - 0x3f, 1, palette[color.min(palette.len() - 1)], &mut pixels, &mut w, &mut h);
                x += 1;
                i += 1;
            }
            _ => i += 1,
        }
        if w > 8192 || h > 8192 {
            return None;
        }
    }
    let (w, h) = match declared {
        Some((dw, dh)) if dw > 0 && dh > 0 => (dw.max(w).min(8192), dh.max(h).min(8192)),
        _ => (w, h),
    };
    if w == 0 || h == 0 {
        return None;
    }
    let bg = if transparent_bg { [0, 0, 0, 0] } else { [0, 0, 0, 255] };
    let mut img = image::RgbaImage::from_pixel(w as u32, h as u32, image::Rgba(bg));
    for ((px, py), c) in pixels {
        if px < w && py < h {
            img.put_pixel(px as u32, py as u32, image::Rgba(c));
        }
    }
    Some(img)
}

fn pct(v: u32) -> u8 {
    ((v.min(100) * 255) / 100) as u8
}

fn hls(h: u32, l: u32, s: u32) -> [u8; 4] {
    // sixel HLS 는 색상 0°가 파랑이다.
    let h = ((h + 240) % 360) as f32 / 360.0;
    let l = l.min(100) as f32 / 100.0;
    let s = s.min(100) as f32 / 100.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0), 255]
}

fn default_palette() -> Vec<[u8; 4]> {
    let c = |r: u32, g: u32, b: u32| [pct(r), pct(g), pct(b), 255];
    vec![
        c(0, 0, 0), c(20, 20, 80), c(80, 13, 13), c(20, 80, 20), c(80, 20, 80), c(20, 80, 80), c(80, 80, 20), c(53, 53, 53),
        c(26, 26, 26), c(33, 33, 60), c(60, 26, 26), c(33, 60, 33), c(60, 33, 60), c(33, 60, 60), c(60, 60, 33), c(80, 80, 80),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> Geometry {
        Geometry { cell_w: 10, cell_h: 20, cols: 80, rows: 24 }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn enc(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[test]
    fn scanner_splits_stream_at_image_end_across_chunks() {
        let seq = format!("ab\x1b]1337;File=inline=1:{}\x07cd", enc(&png(4, 4)));
        let bytes = seq.as_bytes();
        let mut s = ImageScanner::default();
        let mid = bytes.len() / 2;
        let mut segs = s.feed(&bytes[..mid]);
        assert!(segs.iter().all(|x| matches!(x, Segment::Bytes(..))));
        segs = s.feed(&bytes[mid..]);
        assert!(matches!(segs[1], Segment::Image(ImageCmd::Iterm(_))));
        match segs[2] {
            Segment::Bytes(a, b) => assert_eq!(&bytes[mid..][a..b], b"cd"),
            _ => panic!(),
        }
    }

    #[test]
    fn non_image_sequences_are_not_captured() {
        let mut s = ImageScanner::default();
        let segs = s.feed(b"\x1b]0;title\x07\x1bPzz\x1b\\\x1b_Xabc\x1b\\");
        assert!(segs.iter().all(|x| matches!(x, Segment::Bytes(..))));
    }

    #[test]
    fn iterm_inline_image_uses_native_size_in_cells() {
        let mut st = ImageState::default();
        let body = format!("File=inline=1;size=10:{}", enc(&png(40, 60)));
        match st.handle(ImageCmd::Iterm(body.into_bytes()), &geom()) {
            Action::Show(d) => assert_eq!((d.cols, d.rows), (4, 3)),
            _ => panic!(),
        }
    }

    #[test]
    fn iterm_width_in_cells_keeps_aspect() {
        let mut st = ImageState::default();
        let body = format!("File=inline=1;width=8:{}", enc(&png(40, 40)));
        match st.handle(ImageCmd::Iterm(body.into_bytes()), &geom()) {
            Action::Show(d) => assert_eq!((d.cols, d.rows), (8, 4)),
            _ => panic!(),
        }
    }

    #[test]
    fn iterm_non_inline_is_ignored() {
        let mut st = ImageState::default();
        let body = format!("File=name=eA==:{}", enc(&png(4, 4)));
        assert!(matches!(st.handle(ImageCmd::Iterm(body.into_bytes()), &geom()), Action::None));
    }

    #[test]
    fn iterm_multipart() {
        let mut st = ImageState::default();
        let data = enc(&png(20, 20));
        let (a, b) = data.split_at(data.len() / 2);
        assert!(matches!(st.handle(ImageCmd::Iterm(b"MultipartFile=inline=1".to_vec()), &geom()), Action::None));
        assert!(matches!(st.handle(ImageCmd::Iterm(format!("FilePart={a}").into_bytes()), &geom()), Action::None));
        assert!(matches!(st.handle(ImageCmd::Iterm(format!("FilePart={b}").into_bytes()), &geom()), Action::None));
        assert!(matches!(st.handle(ImageCmd::Iterm(b"FileEnd".to_vec()), &geom()), Action::Show(_)));
    }

    #[test]
    fn kitty_png_chunked_with_reply() {
        let mut st = ImageState::default();
        let data = enc(&png(30, 40));
        let (a, b) = data.split_at(8);
        assert!(matches!(st.handle(ImageCmd::Kitty(format!("a=T,f=100,i=7,m=1;{a}").into_bytes()), &geom()), Action::None));
        match st.handle(ImageCmd::Kitty(format!("m=0;{b}").into_bytes()), &geom()) {
            Action::ShowAndReply(d, r) => {
                assert_eq!((d.cols, d.rows), (3, 2));
                assert_eq!(r, b"\x1b_Gi=7;OK\x1b\\");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn kitty_query_and_rgb() {
        let mut st = ImageState::default();
        match st.handle(ImageCmd::Kitty(b"a=q,i=31,s=1,v=1,f=24;AAAA".to_vec()), &geom()) {
            Action::Reply(r) => assert_eq!(r, b"\x1b_Gi=31;OK\x1b\\"),
            _ => panic!(),
        }
        let rgb = enc(&[255u8, 0, 0, 0, 255, 0]);
        assert!(matches!(st.handle(ImageCmd::Kitty(format!("a=T,f=24,s=2,v=1;{rgb}").into_bytes()), &geom()), Action::Show(_)));
    }

    #[test]
    fn sixel_decodes_colors_and_repeat() {
        // 빨강 #1, 6픽셀 높이 3픽셀 너비.
        let img = decode_sixel(b"0;1;0", b"#1;2;100;0;0#1!3~").unwrap();
        assert_eq!((img.width(), img.height()), (3, 6));
        assert_eq!(img.get_pixel(2, 5).0, [255, 0, 0, 255]);
        let img2 = decode_sixel(b"", b"\"1;1;4;12#2;2;0;100;0~~~~-~~~~").unwrap();
        assert_eq!((img2.width(), img2.height()), (4, 12));
        assert_eq!(img2.get_pixel(0, 11).0, [0, 255, 0, 255]);
    }

    #[test]
    fn scanner_detects_sixel_dcs() {
        let mut s = ImageScanner::default();
        let segs = s.feed(b"\x1bP0;1;0q#1;2;100;0;0~~\x1b\\");
        assert!(segs.iter().any(|x| matches!(x, Segment::Image(ImageCmd::Sixel { .. }))));
    }
}
