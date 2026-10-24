//! Minimal localhost HTTP client (hand-rolled: the supervisor only ever talks
//! to 127.0.0.1 over plain HTTP/1.1, so pulling in a TLS-capable HTTP stack
//! would be pure build weight) plus the address parsing the probes need.
//!
//! Anti-loop rule #2 lives here: every probe connects DIRECTLY to the
//! service it measures. Nothing is ever probed "through" the sibling, so a
//! dead sync-server can never make a healthy web reader look dead (or vice
//! versa), and probe results can safely feed restart decisions.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Issue `GET path` and return `(status, rtt-to-first-byte)`. Only the first
/// read chunk is consumed — enough to parse the status line without waiting
/// for a full HTML page, and `Connection: close` has the server hang up.
pub async fn get_status(
    host_port: &str,
    path: &str,
    timeout: Duration,
) -> io::Result<(u16, Duration)> {
    let started = std::time::Instant::now();
    let mut stream = connect(host_port, timeout).await?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUser-Agent: readest-supervisor\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let (status, rtt) = read_status_line(&mut stream, started).await?;
    // Do not wait for the body; dropping the socket is fine after one read.
    Ok((status, rtt))
}

/// Issue `POST path` with a JSON body and bearer auth; returns the status
/// and the response body (⑩: the host-snapshot push now carries pending
/// admin-page service commands back in its response). Accumulates until the
/// Content-Length body is complete — one buffer, no bytes dropped between
/// status line and body.
pub async fn post_json(
    host_port: &str,
    path: &str,
    bearer: &str,
    body: &str,
    timeout: Duration,
) -> io::Result<(u16, String)> {
    let mut stream = connect(host_port, timeout).await?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nUser-Agent: readest-supervisor\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some((status, body)) = parse_response(&buf, false) {
            return Ok((status, body));
        }
        if buf.len() >= 128 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response too large",
            ));
        }
        let n = match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk)).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out reading response",
                ))
            }
        };
        if n == 0 {
            // EOF: accept what arrived, else the response was truncated.
            return parse_response(&buf, true).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed mid-response",
                )
            });
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Split a buffered HTTP/1.1 response into (status, body). Returns None while
/// the header block or the Content-Length body is still incomplete; a body
/// without Content-Length is only complete at EOF (`eof`). The payloads here
/// are tiny JSON envelopes, so a full-buffer approach is fine.
fn parse_response(buf: &[u8], eof: bool) -> Option<(u16, String)> {
    let text = std::str::from_utf8(buf).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())?;
    let content_length = head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    match content_length {
        Some(n) if body.len() >= n => Some((status, body[..n].to_string())),
        // No Content-Length (a bare 204): only complete once the server hangs
        // up, otherwise a split packet would pass off a partial body as final.
        None if eof => Some((status, body.to_string())),
        None | Some(_) => None, // body still streaming in
    }
}

async fn connect(host_port: &str, timeout: Duration) -> io::Result<TcpStream> {
    match tokio::time::timeout(timeout, TcpStream::connect(host_port)).await {
        Ok(r) => r,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("connect to {host_port} timed out after {timeout:?}"),
        )),
    }
}

async fn read_status_line(
    stream: &mut TcpStream,
    started: std::time::Instant,
) -> io::Result<(u16, Duration)> {
    let mut buf = [0u8; 512];
    let fut = stream.read(&mut buf);
    let n = match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the status line",
            ))
        }
    };
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed before status line",
        ));
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unparseable status line: {}",
                    head.lines().next().unwrap_or("")
                ),
            )
        })?;
    Ok((status, started.elapsed()))
}

/// Turn the sync server's LISTEN_ADDR (or the web PORT) into a 127.0.0.1
/// host:port pair for direct probing. Wildcard binds are replaced with
/// loopback; bracketed IPv6 is handled.
pub fn probe_addr(listen_addr: &str, default_port: u16) -> String {
    let (host, port) = split_host_port(listen_addr, default_port);
    let trimmed = host.trim_matches(|c| c == '[' || c == ']');
    // "" / ":" both mean "no host was given" ("::" rsplit-splits to ":").
    let host = match trimmed {
        "" | "0.0.0.0" | "::" | ":" | "*" => "127.0.0.1",
        h => h,
    };
    format!("{host}:{port}")
}

fn split_host_port(s: &str, default_port: u16) -> (String, u16) {
    match s.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse().unwrap_or(default_port);
            (host.to_string(), port)
        }
        None => (s.to_string(), default_port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_binds_become_loopback() {
        assert_eq!(probe_addr("0.0.0.0:8080", 8080), "127.0.0.1:8080");
        assert_eq!(probe_addr(":9000", 8080), "127.0.0.1:9000");
        assert_eq!(probe_addr("::", 8080), "127.0.0.1:8080");
    }

    #[test]
    fn explicit_hosts_keep_their_address() {
        assert_eq!(probe_addr("127.0.0.1:8080", 8080), "127.0.0.1:8080");
        assert_eq!(probe_addr("192.168.1.5:9000", 8080), "192.168.1.5:9000");
        assert_eq!(probe_addr("[::1]:8080", 8080), "::1:8080");
    }

    #[test]
    fn garbage_falls_back_to_default_port() {
        assert_eq!(probe_addr("not-a-port", 3000), "not-a-port:3000");
        assert_eq!(probe_addr("abc:xyz", 3000), "abc:3000");
    }

    #[test]
    fn response_parse_waits_for_the_full_content_length_body() {
        let body = "{\"commands\":[]}";
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        let full = format!("{head}\r\n{body}");
        let (status, parsed) = parse_response(full.as_bytes(), false).unwrap();
        assert_eq!(status, 200);
        assert_eq!(parsed, body);
        // Header block present but body one byte short: keep buffering.
        let truncated = &full[..full.len() - 1];
        assert!(parse_response(truncated.as_bytes(), false).is_none());
        // Header block itself incomplete: nothing to say yet.
        assert!(parse_response(head.as_bytes(), false).is_none());
    }

    #[test]
    fn response_parse_takes_unframed_bodies_only_at_eof() {
        // No Content-Length (a legacy 204 from an older server): mid-stream
        // buffers stay incomplete so a split packet cannot pass as final.
        let resp = "HTTP/1.1 204 No Content\r\n\r\n";
        assert!(parse_response(resp.as_bytes(), false).is_none());
        let (status, body) = parse_response(resp.as_bytes(), true).unwrap();
        assert_eq!(status, 204);
        assert_eq!(body, "");
    }

    #[test]
    fn response_parse_handles_framed_204() {
        let resp = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        let (status, body) = parse_response(resp.as_bytes(), false).unwrap();
        assert_eq!(status, 204);
        assert_eq!(body, "");
    }
}
