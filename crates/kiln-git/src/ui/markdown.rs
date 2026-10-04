//! PR 본문/코멘트용 간단한 마크다운 파서와 렌더러.
//!
//! 지원: 제목, 목록(순서/비순서/체크박스), 코드 블록, 인용, 구분선, 표(고정폭 표시),
//! 굵게/기울임/취소선/인라인 코드/링크/자동 링크. HTML 주석과 태그는 제거한다.

use egui::{CornerRadius, FontId, Margin, RichText, Stroke, Ui, vec2};

use super::widgets::theme;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    Strike(String),
    Code(String),
    Link { text: String, url: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Item { indent: u8, marker: String, checked: Option<bool>, spans: Vec<Span> },
    Code { lang: String, text: String },
    Quote(Vec<Span>),
    Table(Vec<Vec<String>>),
    Rule,
}

/// 파싱된 마크다운 문서.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Markdown {
    pub blocks: Vec<Block>,
    /// 폰트에 없는 글리프를 걸러낸 표시용 블록(첫 렌더링 때 만든다).
    display: std::cell::OnceCell<Vec<Block>>,
}

impl Markdown {
    pub fn parse(src: &str) -> Self {
        let cleaned = strip_html(src);
        let mut blocks = Vec::new();
        let mut para: Vec<String> = Vec::new();
        let mut lines = cleaned.lines().peekable();
        let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
            if !para.is_empty() {
                let mut text = String::new();
                for (i, l) in para.iter().enumerate() {
                    if i > 0 {
                        text.push(if para[i - 1].ends_with("  ") { '\n' } else { ' ' });
                    }
                    text.push_str(l.trim());
                }
                blocks.push(Block::Para(parse_inline(&text)));
                para.clear();
            }
        };
        while let Some(raw) = lines.next() {
            let line = raw.trim_end_matches('\r');
            let trimmed = line.trim_start();
            if let Some(fence) = trimmed.strip_prefix("```").or_else(|| trimmed.strip_prefix("~~~")) {
                flush(&mut para, &mut blocks);
                let lang = fence.trim().to_string();
                let mut text = String::new();
                for l in lines.by_ref() {
                    let lt = l.trim_start();
                    if lt.starts_with("```") || lt.starts_with("~~~") {
                        break;
                    }
                    text.push_str(l.trim_end_matches('\r'));
                    text.push('\n');
                }
                blocks.push(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
                continue;
            }
            if trimmed.is_empty() {
                flush(&mut para, &mut blocks);
                continue;
            }
            if trimmed.starts_with('#') {
                let level = trimmed.chars().take_while(|&c| c == '#').count();
                if level <= 6 && trimmed[level..].starts_with(' ') {
                    flush(&mut para, &mut blocks);
                    blocks.push(Block::Heading(level as u8, parse_inline(trimmed[level..].trim())));
                    continue;
                }
            }
            if is_rule(trimmed) {
                flush(&mut para, &mut blocks);
                blocks.push(Block::Rule);
                continue;
            }
            if let Some(q) = trimmed.strip_prefix('>') {
                flush(&mut para, &mut blocks);
                let mut text = q.trim().to_string();
                while let Some(n) = lines.peek() {
                    let nt = n.trim_start();
                    if let Some(q2) = nt.strip_prefix('>') {
                        text.push(' ');
                        text.push_str(q2.trim());
                        lines.next();
                    } else {
                        break;
                    }
                }
                blocks.push(Block::Quote(parse_inline(text.trim())));
                continue;
            }
            if trimmed.starts_with('|') {
                flush(&mut para, &mut blocks);
                let mut rows = vec![table_cells(trimmed)];
                while let Some(n) = lines.peek() {
                    let nt = n.trim();
                    if nt.starts_with('|') {
                        let cells = table_cells(nt);
                        if !cells.iter().all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && !c.is_empty()) {
                            rows.push(cells);
                        }
                        lines.next();
                    } else {
                        break;
                    }
                }
                blocks.push(Block::Table(rows));
                continue;
            }
            if let Some((marker, rest)) = list_marker(trimmed) {
                flush(&mut para, &mut blocks);
                let indent = ((line.len() - trimmed.len()) / 2).min(6) as u8;
                let (checked, rest) = if let Some(r) = rest.strip_prefix("[ ] ") {
                    (Some(false), r)
                } else if let Some(r) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
                    (Some(true), r)
                } else {
                    (None, rest)
                };
                blocks.push(Block::Item { indent, marker, checked, spans: parse_inline(rest.trim()) });
                continue;
            }
            para.push(line.to_string());
        }
        flush(&mut para, &mut blocks);
        Markdown { blocks, display: Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// 마크다운을 그린다.
    pub fn show(&self, ui: &mut Ui) {
        let t = theme();
        ui.spacing_mut().item_spacing.y = 6.0;
        let blocks = self.display.get_or_init(|| ui.ctx().fonts_mut(|f| sanitize_blocks(&self.blocks, f)));
        for b in blocks {
            match b {
                Block::Heading(level, spans) => {
                    let size = match level {
                        1 => 19.0,
                        2 => 16.5,
                        3 => 14.5,
                        _ => 13.5,
                    };
                    ui.add_space(if *level <= 2 { 6.0 } else { 2.0 });
                    inline(ui, spans, size, true);
                    if *level <= 2 {
                        let r = ui.available_rect_before_wrap();
                        ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, t.border));
                        ui.add_space(2.0);
                    }
                }
                Block::Para(spans) => inline(ui, spans, 13.0, false),
                Block::Item { indent, marker, checked, spans } => {
                    ui.horizontal_top(|ui| {
                        ui.add_space(6.0 + *indent as f32 * 16.0);
                        let m = match checked {
                            Some(true) => "☑".to_string(),
                            Some(false) => "☐".to_string(),
                            None if marker == "•" => if *indent == 0 { "•" } else { "·" }.to_string(),
                            None => marker.clone(),
                        };
                        ui.label(RichText::new(m).color(t.text_dim).size(13.0));
                        ui.vertical(|ui| inline(ui, spans, 13.0, false));
                    });
                }
                Block::Code { text, .. } => {
                    egui::Frame::new()
                        .fill(t.bg)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::horizontal().id_salt(text.len()).show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(text).monospace().size(12.0).color(t.text))
                                        .extend()
                                        .selectable(true),
                                );
                            });
                        });
                }
                Block::Quote(spans) => {
                    egui::Frame::new().inner_margin(Margin { left: 10, right: 0, top: 2, bottom: 2 }).show(ui, |ui| {
                        let r = ui.available_rect_before_wrap();
                        inline(ui, spans, 13.0, false);
                        let used = ui.min_rect();
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(egui::pos2(r.left() - 10.0, used.top()), egui::pos2(r.left() - 7.0, used.bottom())),
                            CornerRadius::same(2),
                            t.border_strong,
                        );
                    });
                }
                Block::Table(rows) => {
                    let ncol = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let mut widths = vec![0usize; ncol];
                    for r in rows {
                        for (i, c) in r.iter().enumerate() {
                            widths[i] = widths[i].max(c.chars().count().min(60));
                        }
                    }
                    let mut s = String::new();
                    for (ri, r) in rows.iter().enumerate() {
                        for (i, w) in widths.iter().enumerate() {
                            let c = r.get(i).map(String::as_str).unwrap_or("");
                            let c: String = c.chars().take(60).collect();
                            s.push_str(&format!("{c:<w$}  "));
                        }
                        s.push('\n');
                        if ri == 0 {
                            s.push_str(&"─".repeat(widths.iter().map(|w| w + 2).sum()));
                            s.push('\n');
                        }
                    }
                    egui::Frame::new()
                        .fill(t.bg)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            egui::ScrollArea::horizontal().id_salt(("md_table", s.len())).show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(s.trim_end()).monospace().size(11.5).color(t.text_dim)).extend());
                            });
                        });
                }
                Block::Rule => {
                    ui.add_space(4.0);
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, t.border));
                    ui.add_space(6.0);
                }
            }
        }
    }
}

fn is_rule(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    s.len() >= 3 && (s.chars().all(|c| c == '-') || s.chars().all(|c| c == '*') || s.chars().all(|c| c == '_'))
}

fn list_marker(s: &str) -> Option<(String, &str)> {
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = s.strip_prefix(m) {
            return Some(("•".into(), r));
        }
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 4 {
        let rest = &s[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &s[..digits]), r));
        }
    }
    None
}

fn table_cells(line: &str) -> Vec<String> {
    let l = line.trim().trim_start_matches('|').trim_end_matches('|');
    l.split('|').map(|c| strip_inline_marks(c.trim())).collect()
}

fn strip_inline_marks(s: &str) -> String {
    s.replace("**", "").replace(['`', '\u{200b}'], "")
}

/// HTML 주석과 태그를 지우고 흔한 엔티티를 푼다. `<br>` 은 줄바꿈으로 바꾼다.
fn strip_html(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let mut in_code = false;
    while !rest.is_empty() {
        if rest.starts_with("```") {
            in_code = !in_code;
            out.push_str("```");
            rest = &rest[3..];
            continue;
        }
        if !in_code && rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(e) => rest = &rest[e + 3..],
                None => break,
            }
            continue;
        }
        if !in_code && rest.starts_with('<') {
            let tag_end = rest.find('>');
            let looks_tag = rest[1..].chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
            if let (Some(e), true) = (tag_end, looks_tag)
                && !rest[..e].contains('\n')
            {
                let tag = rest[1..e].trim_start_matches('/').split(|c: char| c.is_whitespace()).next().unwrap_or("").to_lowercase();
                let closing = rest[1..].starts_with('/');
                if tag == "summary" {
                    out.push_str(if closing { "**\n" } else { "\n**" });
                } else if tag == "br" || tag == "p" || tag == "details" || tag == "div" {
                    out.push('\n');
                }
                rest = &rest[e + 1..];
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// 인라인 마크다운을 조각으로 나눈다.
pub(crate) fn parse_inline(s: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    let flush = |buf: &mut String, spans: &mut Vec<Span>| {
        if !buf.is_empty() {
            spans.push(Span::Text(std::mem::take(buf)));
        }
    };
    while i < b.len() {
        let rest = &s[i..];
        if rest.starts_with('`')
            && let Some(e) = rest[1..].find('`')
        {
            flush(&mut buf, &mut spans);
            spans.push(Span::Code(rest[1..1 + e].to_string()));
            i += e + 2;
            continue;
        }
        if (rest.starts_with("**") || rest.starts_with("__"))
            && let Some(e) = rest[2..].find(&rest[..2])
            && e > 0
        {
            flush(&mut buf, &mut spans);
            spans.push(Span::Bold(rest[2..2 + e].to_string()));
            i += e + 4;
            continue;
        }
        if rest.starts_with("~~")
            && let Some(e) = rest[2..].find("~~")
            && e > 0
        {
            flush(&mut buf, &mut spans);
            spans.push(Span::Strike(rest[2..2 + e].to_string()));
            i += e + 4;
            continue;
        }
        if rest.starts_with('*')
            && !rest.starts_with("* ")
            && let Some(e) = rest[1..].find('*')
            && e > 0
        {
            flush(&mut buf, &mut spans);
            spans.push(Span::Italic(rest[1..1 + e].to_string()));
            i += e + 2;
            continue;
        }
        let img = rest.starts_with("![");
        if (rest.starts_with('[') || img)
            && let Some((text, url, len)) = parse_link(if img { &rest[1..] } else { rest })
        {
            flush(&mut buf, &mut spans);
            let text = if img { format!("🖼 {}", if text.is_empty() { kiln_common::i18n::tr("이미지") } else { &text }) } else { text };
            spans.push(Span::Link { text, url });
            i += len + usize::from(img);
            continue;
        }
        if (rest.starts_with("https://") || rest.starts_with("http://"))
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            let end = rest.find(|c: char| c.is_whitespace() || c == ')' || c == '>').unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';']);
            flush(&mut buf, &mut spans);
            spans.push(Span::Link { text: url.to_string(), url: url.to_string() });
            i += url.len();
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        buf.push(c);
        i += c.len_utf8();
    }
    flush(&mut buf, &mut spans);
    spans
}

fn parse_link(s: &str) -> Option<(String, String, usize)> {
    let close = s.find("](")?;
    let text = &s[1..close];
    if text.contains('\n') {
        return None;
    }
    let after = &s[close + 2..];
    let end = after.find(')')?;
    let url = after[..end].split_whitespace().next().unwrap_or("").to_string();
    Some((strip_inline_marks(text), url, close + 2 + end + 1))
}

/// 인라인 조각을 줄바꿈되는 한 문단으로 그린다.
fn inline(ui: &mut Ui, spans: &[Span], size: f32, heading: bool) {
    let t = theme();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        {
            for s in spans {
                let base = |txt: &str| {
                    let r = RichText::new(txt).size(size).color(t.text);
                    if heading { r.font(kiln_common::fonts::semibold(size)) } else { r }
                };
                match s {
                    Span::Text(x) => {
                        ui.add(egui::Label::new(base(x)).wrap());
                    }
                    Span::Bold(x) => {
                        ui.add(egui::Label::new(base(x).font(kiln_common::fonts::semibold(size))).wrap());
                    }
                    Span::Italic(x) => {
                        ui.add(egui::Label::new(base(x).italics()).wrap());
                    }
                    Span::Strike(x) => {
                        ui.add(egui::Label::new(base(x).strikethrough().color(t.text_dim)).wrap());
                    }
                    Span::Code(x) => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(x)
                                    .font(FontId::monospace(size - 1.0))
                                    .color(t.text)
                                    .background_color(t.bg_hover),
                            )
                            .wrap(),
                        );
                    }
                    Span::Link { text, url } => {
                        let r = ui.add(
                            egui::Label::new(RichText::new(text).size(size).color(t.accent)).wrap().sense(egui::Sense::click()),
                        );
                        if r.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if r.clicked() && url.starts_with("http") {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        r.on_hover_text(url);
                    }
                }
            }
        }
    });
}

/// 폰트에 글리프가 없는 문자를 뺀다.
fn clean(s: &str, f: &mut egui::epaint::text::FontsView<'_>, font: &FontId) -> String {
    if f.has_glyphs(font, s) {
        return s.to_string();
    }
    let out: String = s.chars().filter(|&c| c.is_whitespace() || f.has_glyph(font, c)).collect();
    if out.len() != s.len() && s.starts_with(|c: char| !c.is_whitespace()) { out.trim_start().to_string() } else { out }
}

fn sanitize_spans(spans: &[Span], f: &mut egui::epaint::text::FontsView<'_>) -> Vec<Span> {
    let prop = FontId::proportional(13.0);
    spans
        .iter()
        .map(|s| match s {
            Span::Text(x) => Span::Text(clean(x, f, &prop)),
            Span::Bold(x) => Span::Bold(clean(x, f, &prop)),
            Span::Italic(x) => Span::Italic(clean(x, f, &prop)),
            Span::Strike(x) => Span::Strike(clean(x, f, &prop)),
            Span::Code(x) => Span::Code(clean(x, f, &prop)),
            Span::Link { text, url } => Span::Link { text: clean(text, f, &prop), url: url.clone() },
        })
        .filter(|s| match s {
            Span::Text(x) | Span::Bold(x) | Span::Italic(x) | Span::Strike(x) | Span::Code(x) => !x.is_empty(),
            Span::Link { text, .. } => !text.is_empty(),
        })
        .collect()
}

fn sanitize_blocks(blocks: &[Block], f: &mut egui::epaint::text::FontsView<'_>) -> Vec<Block> {
    let prop = FontId::proportional(13.0);
    blocks
        .iter()
        .map(|b| match b {
            Block::Heading(l, s) => Block::Heading(*l, sanitize_spans(s, f)),
            Block::Para(s) => Block::Para(sanitize_spans(s, f)),
            Block::Item { indent, marker, checked, spans } => {
                Block::Item { indent: *indent, marker: marker.clone(), checked: *checked, spans: sanitize_spans(spans, f) }
            }
            Block::Code { lang, text } => Block::Code { lang: lang.clone(), text: clean(text, f, &prop) },
            Block::Quote(s) => Block::Quote(sanitize_spans(s, f)),
            Block::Table(rows) => {
                Block::Table(rows.iter().map(|r| r.iter().map(|c| clean(c, f, &prop)).collect()).collect())
            }
            Block::Rule => Block::Rule,
        })
        .collect()
}
