//! JSON-RPC 2.0 메시지의 `Content-Length` 헤더 프레이밍.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// 메시지 하나를 헤더와 함께 직렬화한다.
pub fn encode(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// 메시지 하나를 쓰고 비운다.
pub fn write_message(w: &mut impl Write, msg: &Value) -> io::Result<()> {
    w.write_all(&encode(msg))?;
    w.flush()
}

/// 메시지 하나를 읽는다. 스트림이 헤더 전에 끝나면 `Ok(None)`.
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    let mut saw_header = false;
    loop {
        line.clear();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            if saw_header {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, kiln_common::i18n::tr("헤더 도중 스트림 끝")));
            }
            return Ok(None);
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            if saw_header {
                break;
            }
            continue;
        }
        saw_header = true;
        if let Some((name, value)) = l.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(
                value
                    .trim()
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, kiln_common::i18n::tr("잘못된 Content-Length")))?,
            );
        }
    }
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, kiln_common::i18n::tr("Content-Length 없음")))?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// 받은 메시지 분류.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Response { id: Value, result: Result<Value, String> },
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    Invalid,
}

/// 메시지를 응답·요청·알림으로 나눈다.
pub fn classify(msg: Value) -> Incoming {
    let Value::Object(mut m) = msg else { return Incoming::Invalid };
    let method = m.get("method").and_then(Value::as_str).map(str::to_owned);
    let id = m.remove("id");
    let params = m.remove("params").unwrap_or(Value::Null);
    match (method, id) {
        (Some(method), Some(id)) if !id.is_null() => Incoming::Request { id, method, params },
        (Some(method), _) => Incoming::Notification { method, params },
        (None, Some(id)) => {
            let result = match m.remove("error") {
                Some(e) if !e.is_null() => {
                    let msg = e.get("message").and_then(Value::as_str).unwrap_or(kiln_common::i18n::tr("알 수 없는 오류"));
                    Err(msg.to_owned())
                }
                _ => Ok(m.remove("result").unwrap_or(Value::Null)),
            };
            Incoming::Response { id, result }
        }
        (None, None) => Incoming::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Read};

    /// 한 번에 최대 `chunk` 바이트씩만 돌려주는 읽기 도구.
    struct Chunked<'a> {
        data: &'a [u8],
        chunk: usize,
    }

    impl Read for Chunked<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.chunk.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn encode_writes_content_length_of_utf8_body() {
        let msg = json!({"jsonrpc": "2.0", "method": "x", "params": "한글"});
        let bytes = encode(&msg);
        let s = String::from_utf8(bytes).unwrap();
        let (head, body) = s.split_once("\r\n\r\n").unwrap();
        assert_eq!(head, format!("Content-Length: {}", body.len()));
        assert!(body.len() > body.chars().count());
    }

    #[test]
    fn reads_multiple_messages_from_one_buffer() {
        let mut data = encode(&json!({"id": 1, "result": null}));
        data.extend(encode(&json!({"method": "a", "params": {"x": "é"}})));
        let mut r = BufReader::new(&data[..]);
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"id": 1, "result": null})));
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"method": "a", "params": {"x": "é"}})));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn reads_across_split_reads_and_extra_headers() {
        let body = serde_json::to_vec(&json!({"method": "m", "params": [1, 2, 3]})).unwrap();
        let mut data =
            format!("Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length:  {}\r\n\r\n", body.len())
                .into_bytes();
        data.extend(&body);
        data.extend(encode(&json!({"id": 2, "result": "ok"})));
        let mut r = BufReader::with_capacity(3, Chunked { data: &data, chunk: 2 });
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"method": "m", "params": [1, 2, 3]})));
        assert_eq!(read_message(&mut r).unwrap(), Some(json!({"id": 2, "result": "ok"})));
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    #[test]
    fn truncated_body_and_missing_length_are_errors() {
        let data = b"Content-Length: 50\r\n\r\n{\"id\":1}".to_vec();
        assert!(read_message(&mut BufReader::new(&data[..])).is_err());
        let data = b"X-Other: 1\r\n\r\n{}".to_vec();
        assert!(read_message(&mut BufReader::new(&data[..])).is_err());
    }

    #[test]
    fn classify_distinguishes_message_kinds() {
        assert_eq!(
            classify(json!({"id": 3, "result": [1]})),
            Incoming::Response { id: json!(3), result: Ok(json!([1])) }
        );
        assert_eq!(
            classify(json!({"id": 3, "error": {"code": -1, "message": "no"}})),
            Incoming::Response { id: json!(3), result: Err("no".into()) }
        );
        assert_eq!(
            classify(json!({"id": "a", "method": "workspace/configuration", "params": {}})),
            Incoming::Request { id: json!("a"), method: "workspace/configuration".into(), params: json!({}) }
        );
        assert_eq!(
            classify(json!({"method": "$/progress", "params": 1})),
            Incoming::Notification { method: "$/progress".into(), params: json!(1) }
        );
        assert_eq!(classify(json!([1])), Incoming::Invalid);
    }
}
