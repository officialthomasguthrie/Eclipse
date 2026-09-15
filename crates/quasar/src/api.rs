//! The local api on `127.0.0.1`, the OpenAI-compatible one llama-server serves. Programs on the
//! machine use it with no key and no setup. A web page cannot: a browser puts an `Origin` header
//! on every request a page makes that can do anything, and local programs do not, so a request
//! with an `Origin` from anywhere but the api itself is refused. So is a request for a host name
//! that is not the loopback address, which is what a page that points its own name at 127.0.0.1
//! sends. What gets through goes on to llama-server's unix socket, one request per connection, so
//! no request reaches the model without being looked at first.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::backend::{State, Status};
use crate::http;

/// How long a client has to send the request's headers.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// The most header bytes a request may have.
const LONGEST_HEAD: usize = 64 * 1024;
/// How long a refused client may go on sending its body before the connection closes.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// What a web page is told. The page cannot read it, the reply has no CORS headers.
const REFUSED: &str = "Quasar's local API does not answer web pages.";
/// Headers that belong to one connection and do not go on to llama-server.
const HOP_BY_HOP: [&str; 3] = ["connection", "keep-alive", "proxy-connection"];

/// A request's first line and its headers.
#[derive(Debug, PartialEq, Eq)]
pub struct Head {
    /// Method, path and version, such as `POST /v1/chat/completions HTTP/1.1`.
    pub line: String,
    /// Names and values, in the order they came.
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// Reads a head out of the bytes before the blank line. `None` when they are not a request.
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(raw).ok()?;
        let mut lines = text.split("\r\n");
        let line = lines.next().filter(|line| line.split(' ').count() == 3)?;
        let headers = lines
            .map(|header| {
                let (name, value) = header.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            line: line.to_string(),
            headers,
        })
    }

    fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.headers
            .iter()
            .filter(move |(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// True when a web page sent the request. `port` is the api's own.
    pub fn sent_by_a_web_page(&self, port: u16) -> bool {
        let own = |origin: &str| {
            ["127.0.0.1", "localhost", "[::1]"]
                .iter()
                .any(|host| origin.eq_ignore_ascii_case(&format!("http://{host}:{port}")))
        };
        self.values("origin").any(|origin| !own(origin))
            || self.values("host").any(|host| !loopback(host))
            || self
                .values("sec-fetch-site")
                .any(|site| !matches!(site, "none" | "same-origin"))
    }

    /// The head as it goes on to llama-server, which closes the connection after its reply.
    pub fn forward(&self) -> String {
        let mut text = format!("{}\r\n", self.line);
        for (name, value) in &self.headers {
            if !HOP_BY_HOP.iter().any(|hop| name.eq_ignore_ascii_case(hop)) {
                let _ = write!(text, "{name}: {value}\r\n");
            }
        }
        text.push_str("Connection: close\r\n\r\n");
        text
    }
}

/// True when a `Host` header names the loopback address, with or without a port.
fn loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    name == "127.0.0.1" || name == "::1" || name.eq_ignore_ascii_case("localhost")
}

/// A whole reply with an error in it, in the shape llama-server's own errors have.
pub fn error_reply(status: &str, kind: &str, message: &str) -> String {
    let body = json!({ "error": { "message": message, "type": kind } }).to_string();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Why llama-server does not take the request, in a sentence.
fn unavailable(status: &Status) -> String {
    match status.state {
        State::Loading => "The model is still loading. Try again in a moment.".into(),
        _ if !status.error.is_empty() => status.error.clone(),
        _ => "The model is not running.".into(),
    }
}

/// Answers on `listener` for as long as quasard runs, every connection on its own thread.
/// Requests that get through go to llama-server on `socket`.
pub fn serve(listener: &TcpListener, socket: &Path, status: &Arc<Mutex<Status>>) {
    let port = listener.local_addr().map_or(0, |address| address.port());
    for client in listener.incoming() {
        match client {
            Ok(client) => {
                let socket = socket.to_path_buf();
                let status = Arc::clone(status);
                thread::spawn(move || {
                    let _ = handle(client, port, &socket, &status);
                });
            }
            Err(e) => {
                eprintln!("quasard: the local api could not take a connection: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle(
    mut client: TcpStream,
    port: u16,
    socket: &Path,
    status: &Mutex<Status>,
) -> io::Result<()> {
    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    let head_end = loop {
        if let Some(end) = http::find(&raw, b"\r\n\r\n") {
            break end;
        }
        if raw.len() > LONGEST_HEAD {
            return refuse(
                client,
                "431 Request Header Fields Too Large",
                "invalid_request_error",
                "The request's headers are too long.",
            );
        }
        let read = client.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&buffer[..read]);
    };

    let Some(head) = Head::parse(&raw[..head_end]) else {
        return refuse(
            client,
            "400 Bad Request",
            "invalid_request_error",
            "The request is not HTTP.",
        );
    };
    if head.sent_by_a_web_page(port) {
        println!(
            "quasar: the local api refused a request from a web page: {}",
            head.line
        );
        return refuse(client, "403 Forbidden", "permission_error", REFUSED);
    }
    let Ok(mut server) = UnixStream::connect(socket) else {
        let why = unavailable(&status.lock().unwrap_or_else(PoisonError::into_inner));
        return refuse(client, "503 Service Unavailable", "unavailable_error", &why);
    };

    server.write_all(head.forward().as_bytes())?;
    server.write_all(&raw[head_end + 4..])?;
    // the rest of the body goes on as it comes, the reply comes back as llama-server writes it,
    // a stream of tokens included, and the connection ends with the reply
    client.set_read_timeout(None)?;
    let mut from_client = client.try_clone()?;
    let mut to_server = server.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut from_client, &mut to_server);
    });
    let copied = io::copy(&mut server, &mut client);
    let _ = client.shutdown(Shutdown::Both);
    let _ = server.shutdown(Shutdown::Both);
    copied.map(|_| ())
}

/// Sends an error and closes the connection. What the client still sends is read and dropped
/// first, a socket closed with unread bytes in it resets, and the client would lose the reply.
fn refuse(mut client: TcpStream, status: &str, kind: &str, message: &str) -> io::Result<()> {
    client.write_all(error_reply(status, kind, message).as_bytes())?;
    client.shutdown(Shutdown::Write)?;
    client.set_read_timeout(Some(DRAIN_TIMEOUT))?;
    let _ = io::copy(
        &mut (&client).take(LONGEST_HEAD as u64 * 16),
        &mut io::sink(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::Ipv4Addr;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;

    fn head(lines: &[&str]) -> Head {
        Head::parse(lines.join("\r\n").as_bytes()).unwrap()
    }

    #[test]
    fn a_local_program_goes_through() {
        let curl = head(&[
            "POST /completion HTTP/1.1",
            "Host: localhost:11434",
            "User-Agent: curl/8.14.1",
            "Accept: */*",
            "Content-Length: 45",
        ]);
        assert!(!curl.sent_by_a_web_page(11434));
        for host in ["127.0.0.1:11434", "LOCALHOST", "[::1]:11434", "localhost"] {
            let request = head(&["GET /v1/models HTTP/1.1", &format!("Host: {host}")]);
            assert!(!request.sent_by_a_web_page(11434), "{host}");
        }
        assert!(!head(&["GET /health HTTP/1.0"]).sent_by_a_web_page(11434));
        let own = head(&[
            "POST /v1/chat/completions HTTP/1.1",
            "Host: 127.0.0.1:11434",
            "Origin: http://127.0.0.1:11434",
            "Sec-Fetch-Site: same-origin",
        ]);
        assert!(!own.sent_by_a_web_page(11434));
        let typed = head(&["GET /v1/models HTTP/1.1", "Sec-Fetch-Site: none"]);
        assert!(!typed.sent_by_a_web_page(11434));
    }

    #[test]
    fn a_web_page_is_refused() {
        for origin in [
            "https://example.com",
            "null",
            "",
            "http://localhost:8080",
            "http://127.0.0.1:11434.example.com",
            "moz-extension://1234",
        ] {
            let request = head(&[
                "POST /completion HTTP/1.1",
                "Host: localhost:11434",
                &format!("oRiGiN: {origin}"),
            ]);
            assert!(request.sent_by_a_web_page(11434), "{origin}");
        }
        for host in ["example.com:11434", "127.0.0.1.example.com:11434", "[::2]"] {
            let request = head(&["GET /v1/models HTTP/1.1", &format!("Host: {host}")]);
            assert!(request.sent_by_a_web_page(11434), "{host}");
        }
        for site in ["cross-site", "same-site"] {
            let request = head(&[
                "GET /v1/models HTTP/1.1",
                &format!("Sec-Fetch-Site: {site}"),
            ]);
            assert!(request.sent_by_a_web_page(11434), "{site}");
        }
        let second_host = head(&[
            "GET /v1/models HTTP/1.1",
            "Host: localhost",
            "Host: example.com",
        ]);
        assert!(second_host.sent_by_a_web_page(11434));
    }

    #[test]
    fn a_head_that_is_not_a_request_is_not_read() {
        assert_eq!(Head::parse(b"hello"), None);
        assert_eq!(Head::parse(b"GET / HTTP/1.1\r\nno colon here"), None);
        assert_eq!(Head::parse(b"GET / HTTP/1.1\r\nHost: \xff"), None);
    }

    #[test]
    fn what_goes_on_closes_the_connection_after_the_reply() {
        let request = head(&[
            "POST /v1/chat/completions HTTP/1.1",
            "Host: localhost:11434",
            "Connection: keep-alive",
            "Keep-Alive: timeout=5",
            "Content-Length: 2",
        ]);
        assert_eq!(
            request.forward(),
            "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost:11434\r\nContent-Length: 2\r\n\
             Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn the_api_answers_programs_and_refuses_pages() {
        // a stand-in for llama-server on its socket: it reads one request and says ok
        let dir = std::env::temp_dir().join(format!("quasar-api-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("llama.sock");
        let _ = fs::remove_file(&socket);
        let server = UnixListener::bind(&socket).unwrap();
        let (seen, requests) = mpsc::channel();
        thread::spawn(move || {
            for stream in server.incoming() {
                let mut stream = stream.unwrap();
                let mut raw = Vec::new();
                let mut buffer = [0u8; 1024];
                while !raw.ends_with(b"hello") {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                }
                seen.send(String::from_utf8(raw).unwrap()).unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .unwrap();
            }
        });

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let status = Arc::new(Mutex::new(Status::default()));
        {
            let socket = socket.clone();
            let status = Arc::clone(&status);
            thread::spawn(move || serve(&listener, &socket, &status));
        }
        let send = |headers: &str| {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            write!(
                stream,
                "POST /completion HTTP/1.1\r\n{headers}Content-Length: 5\r\n\r\nhello"
            )
            .unwrap();
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).unwrap();
            http::parse(&raw, true).unwrap().unwrap()
        };
        let plain = format!("Host: 127.0.0.1:{port}\r\nConnection: keep-alive\r\n");

        let reply = send(&plain);
        assert_eq!((reply.status, reply.body.as_slice()), (200, &b"ok"[..]));
        let request = requests.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(request.starts_with("POST /completion HTTP/1.1\r\n"));
        assert!(request.ends_with("\r\nConnection: close\r\n\r\nhello"));
        assert!(!request.contains("keep-alive"));

        for page in [
            format!("{plain}Origin: https://example.com\r\n"),
            "Host: attacker.example:11434\r\n".to_string(),
        ] {
            let reply = send(&page);
            assert_eq!(reply.status, 403);
            assert!(String::from_utf8_lossy(&reply.body).contains(REFUSED));
        }
        assert!(requests.recv_timeout(Duration::from_millis(200)).is_err());

        // llama-server gone: the reason instead
        fs::remove_file(&socket).unwrap();
        status.lock().unwrap().state = State::Loading;
        let reply = send(&plain);
        assert_eq!(reply.status, 503);
        assert!(String::from_utf8_lossy(&reply.body).contains("still loading"));
        let _ = fs::remove_dir_all(&dir);
    }
}
