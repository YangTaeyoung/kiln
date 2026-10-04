//! 텍스트 편집 적용: LSP `TextEdit` 목록, 파일 편집, 문서 미러, 스니펫을 일반 텍스트로.

use std::path::Path;

use anyhow::Context as _;

use super::position::byte_col;
use super::{Position, TextChange, TextEdit};

/// 줄마다 (내용 시작, 내용 끝) 바이트 오프셋. 줄 끝 문자(`\n`, `\r\n`, `\r`)는 내용에서 뺀다.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push((start, i));
                start = i + 1;
            }
            b'\r' => {
                out.push((start, i));
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push((start, b.len()));
    out
}

fn offset_of(text: &str, spans: &[(usize, usize)], p: Position) -> usize {
    match spans.get(p.line as usize) {
        Some(&(s, e)) => s + byte_col(&text[s..e], p.character),
        None => text.len(),
    }
}

/// LSP 규칙으로 편집들을 적용한다. 모든 범위는 원본 문서 기준이며 순서는 상관없다.
/// 같은 위치의 삽입은 배열 순서대로 이어 붙는다.
pub fn apply_text_edits(text: &str, edits: &[TextEdit]) -> String {
    let spans = line_spans(text);
    let mut resolved: Vec<(usize, usize, usize, &str)> = edits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let a = offset_of(text, &spans, e.range.start);
            let b = offset_of(text, &spans, e.range.end).max(a);
            (a, b, i, e.new_text.as_str())
        })
        .collect();
    resolved.sort_by_key(|x| std::cmp::Reverse((x.0, x.2)));
    let mut out = text.to_owned();
    let mut limit = usize::MAX;
    for (a, b, _, t) in resolved {
        let b = b.min(limit);
        let a = a.min(b);
        out.replace_range(a..b, t);
        limit = a;
    }
    out
}

/// 디스크의 파일에 편집을 적용한다. BOM 과 주된 줄 끝(CRLF/LF)을 유지한다.
pub fn apply_edits_to_file(path: &Path, edits: &[TextEdit]) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).with_context(|| kiln_common::trf!("{}을(를) 읽을 수 없습니다", path.display()))?;
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let text = std::str::from_utf8(body).with_context(|| kiln_common::trf!("{}은(는) UTF-8 파일이 아닙니다", path.display()))?;
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count();
    let use_crlf = crlf > 0 && crlf * 2 >= lf;
    let edits: Vec<TextEdit> = if use_crlf {
        edits
            .iter()
            .map(|e| TextEdit {
                range: e.range,
                new_text: e.new_text.replace("\r\n", "\n").replace('\n', "\r\n"),
            })
            .collect()
    } else {
        edits.to_vec()
    };
    let out = apply_text_edits(text, &edits);
    let mut data = Vec::with_capacity(out.len() + 3);
    if bom {
        data.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    data.extend_from_slice(out.as_bytes());
    std::fs::write(path, data).with_context(|| kiln_common::trf!("{}에 쓸 수 없습니다", path.display()))
}

/// 서버에 보낸 문서 내용의 줄 단위 사본.
#[derive(Clone, Debug)]
pub struct Mirror {
    pub lines: Vec<String>,
}

impl Mirror {
    pub fn new(text: &str) -> Self {
        let mut m = Self { lines: Vec::new() };
        m.set_text(text);
        m
    }

    fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).collect();
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn clamp(&self, p: Position) -> (usize, usize) {
        let l = p.line as usize;
        if l >= self.lines.len() {
            let last = self.lines.len() - 1;
            return (last, self.lines[last].len());
        }
        (l, byte_col(&self.lines[l], p.character))
    }

    /// 변경 하나를 적용한다.
    pub fn apply(&mut self, change: &TextChange) {
        let Some(range) = change.range else {
            self.set_text(&change.text);
            return;
        };
        let (al, ac) = self.clamp(range.start);
        let (bl, bc) = self.clamp(range.end);
        let ((al, ac), (bl, bc)) = if (al, ac) <= (bl, bc) { ((al, ac), (bl, bc)) } else { ((bl, bc), (al, ac)) };
        let text = change.text.replace("\r\n", "\n");
        let prefix = self.lines[al][..ac].to_owned();
        let suffix = self.lines[bl][bc..].to_owned();
        let mut new_lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        new_lines[0].insert_str(0, &prefix);
        new_lines.last_mut().expect(kiln_common::i18n::tr("split 은 최소 한 조각")).push_str(&suffix);
        self.lines.splice(al..=bl, new_lines);
    }

    /// 줄 번호의 내용. 범위를 벗어나면 `None`.
    pub fn line(&self, i: usize) -> Option<&str> {
        self.lines.get(i).map(String::as_str)
    }
}

/// 스니펫 문법을 일반 텍스트로 바꾼다. 첫 탭 정지(`$1`, 없으면 가장 작은 번호, 그다음 `$0`)의
/// 바이트 위치를 함께 돌려준다. 탭 정지가 없으면 `None`.
pub fn snippet_to_plain(snippet: &str) -> (String, Option<usize>) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::new();
    let mut stops: Vec<(u32, usize)> = Vec::new();
    parse_snippet(&chars, &mut 0, &mut out, &mut stops, false);
    let best = stops
        .iter()
        .filter(|(n, _)| *n > 0)
        .min_by_key(|(n, _)| *n)
        .or_else(|| stops.iter().find(|(n, _)| *n == 0))
        .map(|&(_, at)| at);
    (out, best)
}

/// `nested` 면 짝이 맞는 `}` 에서 멈춘다.
fn parse_snippet(c: &[char], i: &mut usize, out: &mut String, stops: &mut Vec<(u32, usize)>, nested: bool) {
    while *i < c.len() {
        let ch = c[*i];
        match ch {
            '\\' if *i + 1 < c.len() && matches!(c[*i + 1], '$' | '}' | '\\' | ',' | '|') => {
                out.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => {
                *i += 1;
                return;
            }
            '$' if *i + 1 < c.len() && c[*i + 1].is_ascii_digit() => {
                *i += 1;
                let mut n = 0u32;
                while *i < c.len() && c[*i].is_ascii_digit() {
                    n = n.saturating_mul(10).saturating_add(c[*i].to_digit(10).unwrap_or(0));
                    *i += 1;
                }
                stops.push((n, out.len()));
            }
            '$' if *i + 1 < c.len() && (c[*i + 1].is_ascii_alphabetic() || c[*i + 1] == '_') => {
                *i += 1;
                while *i < c.len() && (c[*i].is_ascii_alphanumeric() || c[*i] == '_') {
                    *i += 1;
                }
            }
            '$' if *i + 1 < c.len() && c[*i + 1] == '{' => {
                *i += 2;
                let start = *i;
                while *i < c.len() && (c[*i].is_ascii_alphanumeric() || c[*i] == '_') {
                    *i += 1;
                }
                let name: String = c[start..*i].iter().collect();
                let tabstop = name.parse::<u32>().ok();
                if let Some(n) = tabstop {
                    stops.push((n, out.len()));
                }
                match c.get(*i) {
                    Some('}') => *i += 1,
                    Some(':') => {
                        *i += 1;
                        parse_snippet(c, i, out, stops, true);
                    }
                    Some('|') => {
                        *i += 1;
                        let mut first = true;
                        while *i < c.len() && c[*i] != '|' {
                            if c[*i] == ',' {
                                first = false;
                            } else if c[*i] == '\\' && *i + 1 < c.len() {
                                *i += 1;
                                if first {
                                    out.push(c[*i]);
                                }
                            } else if first {
                                out.push(c[*i]);
                            }
                            *i += 1;
                        }
                        *i += 1;
                        if c.get(*i) == Some(&'}') {
                            *i += 1;
                        }
                    }
                    Some('/') => {
                        // 변환(`${VAR/re/fmt/}`)은 빈 문자열로 둔다.
                        let mut slashes = 0;
                        while *i < c.len() && !(slashes >= 3 && c[*i] == '}') {
                            if c[*i] == '/' {
                                slashes += 1;
                            }
                            *i += 1;
                        }
                        *i += 1;
                    }
                    _ => {}
                }
            }
            _ => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::Range;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn edit(a: (u32, u32), b: (u32, u32), t: &str) -> TextEdit {
        TextEdit { range: Range { start: pos(a.0, a.1), end: pos(b.0, b.1) }, new_text: t.into() }
    }

    #[test]
    fn multiple_edits_use_original_coordinates_in_any_order() {
        let text = "let foo = foo + 1;\nfoo();\n";
        let edits = [edit((1, 0), (1, 3), "bar"), edit((0, 4), (0, 7), "bar"), edit((0, 10), (0, 13), "bar")];
        assert_eq!(apply_text_edits(text, &edits), "let bar = bar + 1;\nbar();\n");
    }

    #[test]
    fn inserts_at_same_position_keep_array_order() {
        let edits = [edit((0, 1), (0, 1), "a"), edit((0, 1), (0, 1), "b")];
        assert_eq!(apply_text_edits("xy", &edits), "xaby");
    }

    #[test]
    fn crlf_lines_and_utf16_columns() {
        let text = "한😀x\r\nsecond\r\n";
        let edits = [edit((0, 3), (0, 4), "Y"), edit((1, 0), (2, 0), "")];
        assert_eq!(apply_text_edits(text, &edits), "한😀Y\r\n");
        let joined = [edit((0, 4), (1, 1), "-")];
        assert_eq!(apply_text_edits(text, &joined), "한😀x-econd\r\n");
    }

    #[test]
    fn positions_past_end_clamp() {
        assert_eq!(apply_text_edits("ab", &[edit((0, 9), (5, 0), "!")]), "ab!");
    }

    #[test]
    fn file_edit_preserves_bom_and_crlf() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rs");
        std::fs::write(&p, b"\xEF\xBB\xBFfn a() {}\r\nfn b() {}\r\n").unwrap();
        apply_edits_to_file(&p, &[edit((1, 3), (1, 4), "c"), edit((0, 9), (0, 9), "\n// x")]).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"\xEF\xBB\xBFfn a() {}\r\n// x\r\nfn c() {}\r\n");
    }

    #[test]
    fn mirror_applies_incremental_changes_with_utf16_ranges() {
        let mut m = Mirror::new("가나다\n😀 end\nlast");
        m.apply(&TextChange { range: Some(Range { start: pos(0, 1), end: pos(0, 2) }), text: "X".into() });
        assert_eq!(m.text(), "가X다\n😀 end\nlast");
        m.apply(&TextChange { range: Some(Range { start: pos(1, 2), end: pos(2, 0) }), text: "\n".into() });
        assert_eq!(m.text(), "가X다\n😀\nlast");
        m.apply(&TextChange { range: Some(Range { start: pos(0, 3), end: pos(1, 2) }), text: "".into() });
        assert_eq!(m.text(), "가X다\nlast");
        m.apply(&TextChange { range: Some(Range { start: pos(1, 4), end: pos(1, 4) }), text: "\nnew\n".into() });
        assert_eq!(m.text(), "가X다\nlast\nnew\n");
        m.apply(&TextChange { range: None, text: "reset".into() });
        assert_eq!(m.text(), "reset");
    }

    #[test]
    fn snippets_degrade_to_plain_text() {
        assert_eq!(snippet_to_plain("println!($1)"), ("println!()".into(), Some(9)));
        assert_eq!(snippet_to_plain("fn ${1:name}(${2:args}) {$0}"), ("fn name(args) {}".into(), Some(3)));
        assert_eq!(snippet_to_plain("${2:b} ${1:a}"), ("b a".into(), Some(2)));
        assert_eq!(snippet_to_plain("x$0"), ("x".into(), Some(1)));
        assert_eq!(snippet_to_plain("${1|one,two|}!"), ("one!".into(), Some(0)));
        assert_eq!(snippet_to_plain("${1:outer ${2:inner}}"), ("outer inner".into(), Some(0)));
        assert_eq!(snippet_to_plain(r"cost \$5 \} $TM_FILENAME ${VAR:def}"), ("cost $5 }  def".into(), None));
        assert_eq!(snippet_to_plain("plain"), ("plain".into(), None));
    }
}
