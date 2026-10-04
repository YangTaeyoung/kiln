//! 테스트용 가짜 언어 서버. stdio 로 JSON-RPC 를 주고받는다.
//!
//! - `bad` 는 오류, `meh` 는 경고 진단
//! - hover: `**hover** \`단어\``
//! - definition: 같은 문서의 `fn 단어`/`let 단어`, `external` 은 `<root>/other.rs` 0줄
//! - references: 문서 안 모든 단어 일치
//! - completion(트리거 `.`): 일반 항목, textEdit 항목, 스니펫 항목
//! - rename: 문서와 디스크의 `<root>/other.rs` 안 단어 일치(documentChanges)
//! - formatting: 줄 끝 공백 제거
//! - initialized 뒤 `$/progress` begin·report(50%), 첫 didSave 에서 end
//!
//! `--crash-on METHOD` fails precisely on the requested method, independent of progress reply ordering.
//! 인수: `--full-sync` 전체 동기화, `--crash-after N` N번째 메시지에서 비정상 종료,
//! `--crash-marker 파일` 그 파일이 있을 때만(지우고) 비정상 종료.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(msg: &Value) {
    let body = serde_json::to_vec(msg).unwrap();
    let mut out = io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn utf16_col(line: &str, byte: usize) -> u32 {
    line[..byte].chars().map(char::len_utf16).sum::<usize>() as u32
}

fn byte_col(line: &str, u: u32) -> usize {
    let mut n = 0usize;
    for (i, c) in line.char_indices() {
        if n + c.len_utf16() > u as usize {
            return i;
        }
        n += c.len_utf16();
    }
    line.len()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 줄 안 모든 단어 일치의 바이트 범위.
fn word_hits(line: &str, word: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find(word) {
        let a = from + i;
        let b = a + word.len();
        let before = line[..a].chars().next_back().is_some_and(is_word);
        let after = line[b..].chars().next().is_some_and(is_word);
        if !before && !after {
            out.push((a, b));
        }
        from = b;
    }
    out
}

fn range(line: u32, text: &str, a: usize, b: usize) -> Value {
    json!({"start": {"line": line, "character": utf16_col(text, a)}, "end": {"line": line, "character": utf16_col(text, b)}})
}

fn uri_to_path(uri: &str) -> PathBuf {
    let rest = uri.trim_start_matches("file://");
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

fn path_to_uri(p: &std::path::Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

struct Server {
    docs: HashMap<String, Vec<String>>,
    root: PathBuf,
    progress_open: bool,
    full_sync: bool,
}

impl Server {
    fn apply_change(&mut self, uri: &str, change: &Value) {
        let lines = self.docs.entry(uri.to_owned()).or_default();
        let text = change["text"].as_str().unwrap_or("");
        let Some(r) = change.get("range") else {
            *lines = text.split('\n').map(str::to_owned).collect();
            return;
        };
        let pos = |p: &Value, lines: &Vec<String>| -> (usize, usize) {
            let l = (p["line"].as_u64().unwrap() as usize).min(lines.len() - 1);
            (l, byte_col(&lines[l], p["character"].as_u64().unwrap() as u32))
        };
        let (al, ac) = pos(&r["start"], lines);
        let (bl, bc) = pos(&r["end"], lines);
        let prefix = lines[al][..ac].to_owned();
        let suffix = lines[bl][bc..].to_owned();
        let mut new: Vec<String> = text.split('\n').map(str::to_owned).collect();
        new[0].insert_str(0, &prefix);
        new.last_mut().unwrap().push_str(&suffix);
        lines.splice(al..=bl, new);
    }

    fn publish(&self, uri: &str) {
        let mut diags = Vec::new();
        for (i, l) in self.docs[uri].iter().enumerate() {
            for (word, sev) in [("bad", 1), ("meh", 2)] {
                for (a, b) in word_hits(l, word) {
                    diags.push(json!({
                        "range": range(i as u32, l, a, b), "severity": sev,
                        "message": format!("`{word}` 발견"), "source": "fake", "code": word.len(),
                    }));
                }
            }
        }
        send(&json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": diags}}));
    }

    fn word_at(&self, params: &Value) -> Option<(String, String)> {
        let uri = params["textDocument"]["uri"].as_str()?.to_owned();
        let lines = self.docs.get(&uri)?;
        let line = lines.get(params["position"]["line"].as_u64()? as usize)?;
        let col = byte_col(line, params["position"]["character"].as_u64()? as u32);
        let start = line[..col].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(col, |(i, _)| i);
        let end = col + line[col..].chars().take_while(|c| is_word(*c)).map(char::len_utf8).sum::<usize>();
        (end > start).then(|| (uri, line[start..end].to_owned()))
    }

    fn occurrences(lines: &[String], uri: &str, word: &str) -> Vec<Value> {
        let mut out = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            for (a, b) in word_hits(l, word) {
                out.push(json!({"uri": uri, "range": range(i as u32, l, a, b)}));
            }
        }
        out
    }

    fn handle_request(&mut self, method: &str, params: &Value) -> Value {
        match method {
            "initialize" => {
                if let Some(uri) = params["rootUri"].as_str() {
                    self.root = uri_to_path(uri);
                }
                json!({"capabilities": {
                    "textDocumentSync": {"openClose": true, "change": if self.full_sync { 1 } else { 2 }, "save": {"includeText": false}},
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                    "completionProvider": {"triggerCharacters": ["."]},
                    "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                    "renameProvider": true,
                    "documentFormattingProvider": true,
                }, "serverInfo": {"name": "fake"}})
            }
            "shutdown" => Value::Null,
            "textDocument/hover" => match self.word_at(params) {
                Some((_, w)) => json!({"contents": {"kind": "markdown", "value": format!("**hover** `{w}`")}}),
                None => Value::Null,
            },
            "textDocument/definition" => {
                let Some((uri, w)) = self.word_at(params) else { return Value::Null };
                if w == "external" {
                    let p = self.root.join("other.rs");
                    return json!({"uri": path_to_uri(&p), "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}});
                }
                for (i, l) in self.docs[&uri].iter().enumerate() {
                    for kw in ["fn ", "let "] {
                        let pat = format!("{kw}{w}");
                        if let Some(a) = l.find(&pat) {
                            let a = a + kw.len();
                            return json!([{"uri": uri, "range": range(i as u32, l, a, a + w.len())}]);
                        }
                    }
                }
                Value::Null
            }
            "textDocument/references" => {
                let Some((uri, w)) = self.word_at(params) else { return json!([]) };
                Value::Array(Self::occurrences(&self.docs[&uri], &uri, &w))
            }
            "textDocument/completion" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
                let ch = params["position"]["character"].as_u64().unwrap_or(0) as u32;
                let text = self.docs.get(uri).and_then(|d| d.get(line as usize)).cloned().unwrap_or_default();
                let col = byte_col(&text, ch);
                let start = text[..col].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(col, |(i, _)| i);
                let r = json!({"start": {"line": line, "character": utf16_col(&text, start)}, "end": {"line": line, "character": ch}});
                json!({"isIncomplete": false, "items": [
                    {"label": "alpha", "kind": 6, "detail": "i32"},
                    {"label": "beta", "kind": 3, "detail": "fn beta()", "textEdit": {"range": r, "newText": "beta_edit"}},
                    {"label": "println!", "kind": 15, "insertText": "println!(\"${1:msg}\")", "insertTextFormat": 2},
                    {"label": "gamma", "kind": 6, "sortText": "zz"},
                ]})
            }
            "textDocument/signatureHelp" => {
                let (uri, line, ch) = (
                    params["textDocument"]["uri"].as_str().unwrap_or(""),
                    params["position"]["line"].as_u64().unwrap_or(0) as usize,
                    params["position"]["character"].as_u64().unwrap_or(0) as u32,
                );
                let text = self.docs.get(uri).and_then(|d| d.get(line)).cloned().unwrap_or_default();
                let before = &text[..byte_col(&text, ch)];
                let Some(open) = before.rfind('(') else { return Value::Null };
                let active = before[open..].matches(',').count();
                json!({"signatures": [{"label": "fn f(a: i32, b: i32)", "parameters": [{"label": "a: i32"}, {"label": "b: i32"}]}], "activeParameter": active})
            }
            "textDocument/rename" => {
                let Some((uri, w)) = self.word_at(params) else { return Value::Null };
                let new_name = params["newName"].as_str().unwrap_or("");
                let edits = |occ: Vec<Value>| -> Vec<Value> {
                    occ.into_iter().map(|o| json!({"range": o["range"], "newText": new_name})).collect()
                };
                let mut changes = vec![json!({"textDocument": {"uri": uri, "version": null}, "edits": edits(Self::occurrences(&self.docs[&uri], &uri, &w))})];
                let other = self.root.join("other.rs");
                let other_uri = path_to_uri(&other);
                if other_uri != uri
                    && let Ok(text) = std::fs::read_to_string(&other)
                {
                    let lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_owned()).collect();
                    changes.push(json!({"textDocument": {"uri": other_uri, "version": null}, "edits": edits(Self::occurrences(&lines, &other_uri, &w))}));
                }
                json!({"documentChanges": changes})
            }
            "textDocument/formatting" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let mut edits = Vec::new();
                for (i, l) in self.docs.get(uri).map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
                    let t = l.trim_end().len();
                    if t < l.len() {
                        edits.push(json!({"range": range(i as u32, l, t, l.len()), "newText": ""}));
                    }
                }
                Value::Array(edits)
            }
            _ => json!({"__error": format!("unknown {method}")}),
        }
    }

    fn handle_notification(&mut self, method: &str, params: &Value) {
        match method {
            "initialized" => {
                send(&json!({"jsonrpc": "2.0", "id": "p1", "method": "window/workDoneProgress/create", "params": {"token": "idx"}}));
                send(&json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx", "value": {"kind": "begin", "title": "Indexing", "percentage": 0}}}));
                send(&json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx", "value": {"kind": "report", "message": "1/2", "percentage": 50}}}));
                self.progress_open = true;
            }
            "textDocument/didOpen" => {
                let d = &params["textDocument"];
                let uri = d["uri"].as_str().unwrap_or("").to_owned();
                let text = d["text"].as_str().unwrap_or("");
                self.docs.insert(uri.clone(), text.split('\n').map(str::to_owned).collect());
                self.publish(&uri);
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_owned();
                for c in params["contentChanges"].as_array().cloned().unwrap_or_default() {
                    self.apply_change(&uri, &c);
                }
                self.publish(&uri);
            }
            "textDocument/didSave" => {
                if self.progress_open {
                    self.progress_open = false;
                    send(&json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx", "value": {"kind": "end"}}}));
                }
            }
            "textDocument/didClose" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_owned();
                self.docs.remove(&uri);
            }
            "exit" => std::process::exit(0),
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let crash_after: Option<usize> = arg("--crash-after").and_then(|s| s.parse().ok());
    let crash_on = arg("--crash-on");
    let crash_marker = arg("--crash-marker").map(PathBuf::from);
    let crash_enabled = match &crash_marker {
        Some(m) => std::fs::remove_file(m).is_ok(),
        None => true,
    };
    let mut server = Server {
        docs: HashMap::new(),
        root: PathBuf::from("."),
        progress_open: false,
        full_sync: args.iter().any(|a| a == "--full-sync"),
    };
    let stdin = io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut count = 0usize;
    while let Some(msg) = read_message(&mut r) {
        count += 1;
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        if crash_enabled && (crash_after.is_some_and(|n| count >= n) || crash_on.as_ref().is_some_and(|wanted| method.as_ref()==Some(wanted))) {
            std::process::exit(3);
        }
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match (method, msg.get("id")) {
            (Some(m), Some(id)) => {
                let result = server.handle_request(&m, &params);
                if let Some(e) = result.get("__error") {
                    send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": e}}));
                } else {
                    send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
                }
            }
            (Some(m), None) => server.handle_notification(&m, &params),
            (None, _) => {}
        }
    }
}
