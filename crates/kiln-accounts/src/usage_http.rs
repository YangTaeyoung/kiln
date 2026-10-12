//! Read-only provider usage requests. Never infer, refresh tokens, or follow redirects.
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub(crate) struct RetryLater(pub i64);
impl std::fmt::Display for RetryLater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "usage temporarily unavailable; retry later") }
}
impl std::error::Error for RetryLater {}

pub(crate) fn retry_delay(headers: &[(String, String)], now: i64) -> i64 {
    headers.iter().rev().find(|(key, _)| key == "retry-after").and_then(|(_, value)| {
        value.parse::<i64>().ok().or_else(|| chrono::DateTime::parse_from_rfc2822(value).ok().map(|date| date.timestamp().saturating_sub(now)))
    }).unwrap_or(300).clamp(60, 86_400)
}

pub(crate) fn get(url: &str, headers: &str) -> Result<serde_json::Value> {
    get_impl(url, headers, "=https")
}

fn get_impl(url: &str, headers: &str, protocol: &str) -> Result<serde_json::Value> {
    // NamedTempFile permissions are 0600. Bound responses; suppress curl diagnostics
    // and provider bodies, which can contain private account information.
    let dump = tempfile::NamedTempFile::new()?;
    let mut child = Command::new("curl")
        .args(["-q", "-sS", "--max-time", "15", "--connect-timeout", "5", "--max-filesize", "262144", "--proto", protocol, "-H", "@-", "-D"])
        .arg(dump.path()).arg(url)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().context("usage request could not start")?;
    let sent = child.stdin.take().context("usage request input unavailable")?.write_all(headers.as_bytes());
    let mut bytes = Vec::new();
    let received = child.stdout.take().context("usage response unavailable")?.take(262_145).read_to_end(&mut bytes);
    // This bound also works with old macOS curl versions and chunked responses.
    if bytes.len() > 262_144 || received.is_err() { let _ = child.kill(); }
    let status = child.wait()?;
    sent.context("usage request input failed")?;
    if bytes.len() > 262_144 { bail!("usage response too large"); }
    received.context("usage response failed")?;
    if !status.success() { bail!("usage request failed or timed out"); }
    let mut header_dump = String::new();
    dump.reopen()?.take(32_769).read_to_string(&mut header_dump)?;
    if header_dump.len() > 32_768 { bail!("usage response headers too large"); }
    let (code, headers) = crate::claude::parse_header_dump(&header_dump);
    if code == 429 || code == 403 { return Err(RetryLater(retry_delay(&headers, crate::now_unix())).into()); }
    if code == 401 { return Err(RetryLater(3600).into()); }
    if code != 200 { bail!("usage request failed (HTTP {code})"); }
    // Do not expose parsing errors with excerpts of the response.
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid usage response"))
}

pub(crate) fn header_value(value: &str) -> Result<&str> {
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_control()) { bail!("invalid usage credentials"); }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_seconds_date_and_bounds() {
        let h = |value: &str| vec![("retry-after".into(), value.into())];
        assert_eq!(retry_delay(&h("900"), 0), 900);
        assert_eq!(retry_delay(&h("Thu, 01 Jan 1970 00:15:00 GMT"), 0), 900);
        assert_eq!(retry_delay(&h("0"), 0), 60);
        assert_eq!(retry_delay(&h("999999"), 0), 86_400);
        assert_eq!(retry_delay(&h("bad"), 0), 300);
        assert!(header_value("token\nX-Foo: bad").is_err());
    }

    // Local loopback only: no real provider credentials or requests.
    fn fixture(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/usage", server.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                stream.read_exact(&mut byte).unwrap(); request.push(byte[0]);
            }
            let _ = stream.write_all(&response);
            String::from_utf8(request).unwrap()
        });
        (url, thread)
    }

    #[test]
    fn read_only_get_uses_private_header_input_and_never_follows_redirects() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_vec();
        let (url, server) = fixture(response);
        assert_eq!(get_impl(&url, "Authorization: Bearer fake-fixture\n", "=http").unwrap()["ok"], true);
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /usage "));
        assert!(request.contains("Authorization: Bearer fake-fixture"));
        assert!(!request.to_ascii_lowercase().contains("content-length:"));
        let destination = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let (url, server) = fixture(format!("HTTP/1.1 302 Found\r\nLocation: http://{}/secret\r\nContent-Length: 0\r\n\r\n", destination.local_addr().unwrap()).into_bytes());
        assert!(get_impl(&url, "Authorization: Bearer fake-fixture\n", "=http").is_err());
        server.join().unwrap();
        assert_eq!(destination.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn rate_limits_errors_and_chunked_body_are_bounded_and_redacted() {
        let (url, server) = fixture(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 900\r\nContent-Length: 13\r\n\r\nprivate-token".to_vec());
        let error = get_impl(&url, "", "=http").unwrap_err();
        assert_eq!(error.downcast_ref::<RetryLater>().unwrap().0, 900);
        assert!(!error.to_string().contains("private-token")); server.join().unwrap();
        let (url, server) = fixture(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nprivate-token".to_vec());
        assert!(!get_impl(&url, "", "=http").unwrap_err().to_string().contains("private-token")); server.join().unwrap();
        let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n50000\r\n".to_vec();
        response.extend(vec![b'x'; 0x50000]); response.extend(b"\r\n0\r\n\r\n");
        let (url, server) = fixture(response);
        assert!(get_impl(&url, "", "=http").is_err()); server.join().unwrap();
    }
}
