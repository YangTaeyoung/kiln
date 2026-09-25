//! LSP JSON 값과 크레이트 타입 사이 변환, `file://` URI 처리.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::edit::snippet_to_plain;
use super::position::byte_col;
use super::{
    CompletionItem, CompletionList, Diagnostic, Location, Position, Range, Severity, SignatureHelp, TextEdit,
    WorkspaceEdit,
};

/// 경로를 `file://` URI 로 바꾼다.
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `file://` URI 를 경로로 바꾼다. 다른 스킴이면 `None`.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8_lossy(&out).into_owned();
    // `/C:/x` 형태의 Windows 드라이브 경로는 앞 `/` 를 뗀다.
    let b = s.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[2] == b':' {
        return Some(PathBuf::from(&s[1..]));
    }
    Some(PathBuf::from(s))
}

pub fn position_json(p: Position) -> Value {
    json!({"line": p.line, "character": p.character})
}

pub fn range_json(r: Range) -> Value {
    json!({"start": position_json(r.start), "end": position_json(r.end)})
}

pub fn parse_position(v: &Value) -> Option<Position> {
    Some(Position {
        line: v.get("line")?.as_u64()? as u32,
        character: v.get("character")?.as_u64()? as u32,
    })
}

pub fn parse_range(v: &Value) -> Option<Range> {
    Some(Range { start: parse_position(v.get("start")?)?, end: parse_position(v.get("end")?)? })
}

pub fn parse_text_edit(v: &Value) -> Option<TextEdit> {
    Some(TextEdit {
        range: parse_range(v.get("range")?)?,
        new_text: v.get("newText")?.as_str()?.to_owned(),
    })
}

pub fn parse_text_edits(v: &Value) -> Vec<TextEdit> {
    v.as_array().map(|a| a.iter().filter_map(parse_text_edit).collect()).unwrap_or_default()
}

pub fn parse_diagnostic(v: &Value) -> Option<Diagnostic> {
    let severity = match v.get("severity").and_then(Value::as_u64) {
        Some(2) => Severity::Warning,
        Some(3) => Severity::Information,
        Some(4) => Severity::Hint,
        _ => Severity::Error,
    };
    let code = match v.get("code") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Some(Diagnostic {
        range: parse_range(v.get("range")?)?,
        severity,
        message: v.get("message")?.as_str()?.to_owned(),
        source: v.get("source").and_then(Value::as_str).map(str::to_owned),
        code,
    })
}

/// `Location`, `Location[]`, `LocationLink[]`, `null` 을 모두 받는다.
pub fn parse_locations(v: &Value) -> Vec<Location> {
    let one = |v: &Value| -> Option<Location> {
        if let Some(uri) = v.get("targetUri").and_then(Value::as_str) {
            let range = v.get("targetSelectionRange").or_else(|| v.get("targetRange")).and_then(parse_range)?;
            return Some(Location { path: uri_to_path(uri)?, range });
        }
        let uri = v.get("uri")?.as_str()?;
        Some(Location { path: uri_to_path(uri)?, range: parse_range(v.get("range")?)? })
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn marked_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            let value = o.get("value")?.as_str()?;
            if o.contains_key("kind") {
                Some(value.to_owned())
            } else {
                let lang = o.get("language").and_then(Value::as_str).unwrap_or("");
                Some(format!("```{lang}\n{value}\n```"))
            }
        }
        _ => None,
    }
}

/// `Hover` 결과를 마크다운 비슷한 텍스트로. 내용이 비었으면 `None`.
pub fn parse_hover(v: &Value) -> Option<String> {
    let contents = v.get("contents")?;
    let text = match contents {
        Value::Array(a) => a.iter().filter_map(marked_string).filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("\n\n"),
        other => marked_string(other)?,
    };
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

pub fn parse_completion(v: &Value) -> CompletionList {
    let (items, is_incomplete) = match v {
        Value::Array(a) => (a.as_slice(), false),
        Value::Object(o) => (
            o.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]),
            o.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false),
        ),
        _ => (&[][..], false),
    };
    CompletionList { is_incomplete, items: items.iter().filter_map(parse_completion_item).collect() }
}

fn parse_completion_item(v: &Value) -> Option<CompletionItem> {
    let label = v.get("label")?.as_str()?.to_owned();
    let snippet = v.get("insertTextFormat").and_then(Value::as_u64) == Some(2);
    let degrade = |s: &str| -> (String, Option<usize>) {
        if snippet { snippet_to_plain(s) } else { (s.to_owned(), None) }
    };
    let edit = v.get("textEdit").and_then(|te| {
        let range = te.get("range").or_else(|| te.get("insert")).and_then(parse_range)?;
        Some((range, te.get("newText")?.as_str()?.to_owned()))
    });
    let raw_insert = v.get("insertText").and_then(Value::as_str).unwrap_or(&label).to_owned();
    let (insert_text, mut cursor_offset) = degrade(&raw_insert);
    let edit = edit.map(|(range, text)| {
        let (t, off) = degrade(&text);
        cursor_offset = off;
        TextEdit { range, new_text: t }
    });
    let detail = v
        .get("detail")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| v.get("labelDetails").and_then(|d| d.get("description")).and_then(Value::as_str).map(str::to_owned));
    Some(CompletionItem {
        filter_text: v.get("filterText").and_then(Value::as_str).unwrap_or(&label).to_owned(),
        sort_text: v.get("sortText").and_then(Value::as_str).unwrap_or(&label).to_owned(),
        detail,
        kind: v.get("kind").and_then(Value::as_u64).map(|k| k as u32),
        insert_text,
        edit,
        additional_edits: v.get("additionalTextEdits").map(parse_text_edits).unwrap_or_default(),
        cursor_offset,
        label,
    })
}

pub fn parse_signature_help(v: &Value) -> Option<SignatureHelp> {
    let sigs = v.get("signatures")?.as_array()?;
    let active = v.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
    let sig = sigs.get(active).or_else(|| sigs.first())?;
    let label = sig.get("label")?.as_str()?.to_owned();
    let active_param = sig
        .get("activeParameter")
        .or_else(|| v.get("activeParameter"))
        .and_then(Value::as_u64)
        .and_then(|i| sig.get("parameters")?.as_array()?.get(i as usize).cloned())
        .and_then(|p| match p.get("label")? {
            Value::String(s) => label.find(s.as_str()).map(|a| (a, a + s.len())),
            Value::Array(a) => {
                let s = a.first()?.as_u64()? as u32;
                let e = a.get(1)?.as_u64()? as u32;
                Some((byte_col(&label, s), byte_col(&label, e)))
            }
            _ => None,
        });
    let doc = sig.get("documentation").and_then(|d| match d {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    });
    Some(SignatureHelp { label, active_param, doc })
}

/// `changes` 와 `documentChanges` 를 경로별 편집 목록으로 합친다. 파일 생성·이름 바꾸기·삭제는 건너뛴다.
pub fn parse_workspace_edit(v: &Value) -> WorkspaceEdit {
    let mut out: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
    let mut push = |path: PathBuf, edits: Vec<TextEdit>| {
        if edits.is_empty() {
            return;
        }
        match out.iter_mut().find(|(p, _)| *p == path) {
            Some((_, e)) => e.extend(edits),
            None => out.push((path, edits)),
        }
    };
    if let Some(dc) = v.get("documentChanges").and_then(Value::as_array) {
        for c in dc {
            if c.get("kind").is_some() {
                continue;
            }
            let Some(path) = c.get("textDocument").and_then(|d| d.get("uri")).and_then(Value::as_str).and_then(uri_to_path)
            else {
                continue;
            };
            push(path, c.get("edits").map(parse_text_edits).unwrap_or_default());
        }
    } else if let Some(ch) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in ch {
            if let Some(path) = uri_to_path(uri) {
                push(path, parse_text_edits(edits));
            }
        }
    }
    WorkspaceEdit { changes: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip_with_spaces_and_korean() {
        let p = PathBuf::from("/tmp/내 폴더/a b#.rs");
        let uri = path_to_uri(&p);
        assert!(uri.starts_with("file:///tmp/"));
        assert!(!uri.contains(' '));
        assert_eq!(uri_to_path(&uri).unwrap(), p);
        assert_eq!(uri_to_path("file:///x/%E1%84%80").unwrap(), PathBuf::from("/x/\u{1100}"));
        assert_eq!(uri_to_path("untitled:1"), None);
    }

    #[test]
    fn workspace_edit_from_changes_and_document_changes() {
        let v = json!({"changes": {"file:///a.rs": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"}
        ]}});
        let e = parse_workspace_edit(&v);
        assert_eq!(e.changes.len(), 1);
        assert_eq!(e.changes[0].0, PathBuf::from("/a.rs"));
        let v = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///a.rs", "version": 1}, "edits": [
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"}]},
            {"kind": "create", "uri": "file:///new.rs"},
            {"textDocument": {"uri": "file:///b.rs", "version": null}, "edits": [
                {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 0}}, "newText": "y", "annotationId": "a"}]},
            {"textDocument": {"uri": "file:///a.rs", "version": 1}, "edits": [
                {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 0}}, "newText": "z"}]}
        ]});
        let e = parse_workspace_edit(&v);
        assert_eq!(e.changes.iter().map(|(p, v)| (p.clone(), v.len())).collect::<Vec<_>>(), vec![
            (PathBuf::from("/a.rs"), 2),
            (PathBuf::from("/b.rs"), 1)
        ]);
    }

    #[test]
    fn locations_accept_all_shapes() {
        let r = json!({"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 4}});
        assert_eq!(parse_locations(&json!({"uri": "file:///a", "range": r})).len(), 1);
        assert_eq!(parse_locations(&json!([{"uri": "file:///a", "range": r}, {"uri": "file:///b", "range": r}])).len(), 2);
        let links = parse_locations(&json!([{"targetUri": "file:///c", "targetRange": r, "targetSelectionRange": r}]));
        assert_eq!(links[0].path, PathBuf::from("/c"));
        assert_eq!(links[0].range.start, Position { line: 1, character: 2 });
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn hover_contents_variants() {
        assert_eq!(parse_hover(&json!({"contents": {"kind": "markdown", "value": "**x**"}})).as_deref(), Some("**x**"));
        assert_eq!(
            parse_hover(&json!({"contents": [{"language": "rust", "value": "fn a()"}, "doc"]})).as_deref(),
            Some("```rust\nfn a()\n```\n\ndoc")
        );
        assert_eq!(parse_hover(&json!({"contents": ""})), None);
    }

    #[test]
    fn completion_items_degrade_snippets_and_use_insert_range() {
        let r = json!({"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}});
        let list = parse_completion(&json!({"isIncomplete": true, "items": [
            {"label": "a"},
            {"label": "p", "insertText": "p($1)", "insertTextFormat": 2, "kind": 3},
            {"label": "q", "textEdit": {"insert": r, "replace": r, "newText": "q${1:x}"}, "insertTextFormat": 2,
             "additionalTextEdits": [{"range": r, "newText": "use q;"}]}
        ]}));
        assert!(list.is_incomplete);
        assert_eq!(list.items[0].insert_text, "a");
        assert_eq!(list.items[1].insert_text, "p()");
        assert_eq!(list.items[1].cursor_offset, Some(2));
        let q = &list.items[2];
        assert_eq!(q.edit.as_ref().unwrap().new_text, "qx");
        assert_eq!(q.cursor_offset, Some(1));
        assert_eq!(q.additional_edits.len(), 1);
    }

    #[test]
    fn signature_help_active_parameter() {
        let s = parse_signature_help(&json!({"signatures": [
            {"label": "fn f(a: i32, b: 한)", "parameters": [{"label": "a: i32"}, {"label": [13, 17]}]}
        ], "activeParameter": 1}))
        .unwrap();
        let (a, b) = s.active_param.unwrap();
        assert_eq!(&s.label[a..b], "b: 한");
    }
}
