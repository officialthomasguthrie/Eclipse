//! Just enough HTTP/1.1 to talk to llama-server on its unix socket: one request per connection, a
//! reply framed by its length, by chunks, or by the server closing the connection.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// A reply: the status code and the body.
#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    /// HTTP status.
    pub status: u16,
    /// The body, with any chunking taken off.
    pub body: Vec<u8>,
}

/// Sends one request to the server on `socket` and reads the reply. A body is sent as JSON.
///
/// # Errors
///
/// When nothing listens, the timeout passes, or the reply is not HTTP.
pub fn send(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request(method, path, body).as_bytes())?;

    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = stream.read(&mut buffer)?;
        raw.extend_from_slice(&buffer[..read]);
        match parse(&raw, read == 0) {
            Ok(Some(reply)) => return Ok(reply),
            Ok(None) => {}
            Err(why) => return Err(io::Error::new(io::ErrorKind::InvalidData, why)),
        }
    }
}

/// The request as it goes on the wire.
pub fn request(method: &str, path: &str, body: Option<&str>) -> String {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(body) = body {
        let _ = write!(
            text,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
    }
    text.push_str("\r\n");
    text.push_str(body.unwrap_or_default());
    text
}

/// Reads a reply out of what has arrived so far. `Ok(None)` means more has to arrive first;
/// once `closed` is true nothing more will, so the answer is a reply or an error.
///
/// # Errors
///
/// When the bytes are not an HTTP reply, or the connection closed before the reply was whole.
pub fn parse(raw: &[u8], closed: bool) -> Result<Option<Reply>, String> {
    let Some(head_end) = find(raw, b"\r\n\r\n") else {
        return if closed {
            Err("the connection closed before the reply's headers ended".into())
        } else {
            Ok(None)
        };
    };
    let head = std::str::from_utf8(&raw[..head_end])
        .map_err(|_| "the reply's headers are not text".to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .strip_prefix("HTTP/1.")
        .and_then(|rest| rest.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("not an HTTP status line: {status_line}"))?;

    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("bad content length {value}"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        }
    }

    let body = &raw[head_end + 4..];
    let whole = if chunked {
        dechunk(body)?
    } else if let Some(length) = length {
        (body.len() >= length).then(|| body[..length].to_vec())
    } else {
        closed.then(|| body.to_vec())
    };
    match whole {
        Some(body) => Ok(Some(Reply { status, body })),
        None if closed => Err("the connection closed before the reply ended".into()),
        None => Ok(None),
    }
}

/// Joins a chunked body. `Ok(None)` until the last chunk is in.
fn dechunk(mut body: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut out = Vec::new();
    loop {
        let Some(line_end) = find(body, b"\r\n") else {
            return Ok(None);
        };
        let size_text = std::str::from_utf8(&body[..line_end])
            .map_err(|_| "a chunk size is not text".to_string())?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text}"))?;
        let rest = &body[line_end + 2..];
        if size == 0 {
            return Ok(Some(out));
        }
        if rest.len() < size + 2 {
            return Ok(None);
        }
        out.extend_from_slice(&rest[..size]);
        body = &rest[size + 2..];
    }
}

/// Where `needle` first starts in `haystack`.
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_with_a_body_says_how_long_it_is() {
        assert_eq!(
            request("POST", "/v1/chat/completions", Some("{\"a\":1}")),
            "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}"
        );
        assert_eq!(
            request("GET", "/health", None),
            "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn a_reply_with_a_length_is_whole_once_the_length_is_in() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 15\r\n\r\n{\"status\":\"ok\"}";
        let reply = parse(raw, false).unwrap().unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"{\"status\":\"ok\"}");
        assert_eq!(parse(&raw[..raw.len() - 3], false), Ok(None));
        assert!(parse(&raw[..raw.len() - 3], true).is_err());
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nContent-", false), Ok(None));
    }

    #[test]
    fn a_chunked_reply_is_joined() {
        let raw = b"HTTP/1.1 503 Service Unavailable\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=y\r\n world\r\n0\r\n\r\n";
        let reply = parse(raw, false).unwrap().unwrap();
        assert_eq!(reply.status, 503);
        assert_eq!(reply.body, b"hello world");
        assert_eq!(parse(&raw[..raw.len() - 12], false), Ok(None));
        assert!(
            parse(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
                false
            )
            .is_err()
        );
    }

    #[test]
    fn without_a_length_the_reply_ends_when_the_connection_does() {
        let raw = b"HTTP/1.0 200 OK\r\n\r\npartial";
        assert_eq!(parse(raw, false), Ok(None));
        assert_eq!(parse(raw, true).unwrap().unwrap().body, b"partial");
    }

    #[test]
    fn something_that_is_not_http_is_an_error() {
        assert!(parse(b"SSH-2.0-OpenSSH\r\n\r\n", false).is_err());
        assert!(parse(b"", true).is_err());
    }
}
