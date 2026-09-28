// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Liveness probe against the local server's `/health` endpoint.
//!
//! The request is written by hand over TCP. The server is always on loopback
//! over plain HTTP, so a full HTTP client would buy nothing here.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// How long the probe waits before calling the server unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Default port the installer configures the server to listen on.
const DEFAULT_PORT: u16 = 4123;

/// Whether the server answered its health check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerStatus {
    Running,
    Stopped,
}

impl ServerStatus {
    /// Short word for the tooltip.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "not responding",
        }
    }
}

/// Probes `GET /health`, treating any failure as a stopped server.
pub fn probe(server_url: &str) -> ServerStatus {
    match request(server_url) {
        Ok(true) => ServerStatus::Running,
        _ => ServerStatus::Stopped,
    }
}

fn request(server_url: &str) -> std::io::Result<bool> {
    let authority = authority(server_url);
    let address = authority
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("no address for {authority}")))?;

    let mut stream = TcpStream::connect_timeout(&address, PROBE_TIMEOUT)?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
    write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )?;

    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line)?;
    Ok(status_line.contains(" 200"))
}

/// Extracts `host:port` from a base URL, defaulting the port.
fn authority(server_url: &str) -> String {
    let without_scheme = server_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(server_url);
    let host_port = without_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(without_scheme)
        .trim();
    if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{host_port}:{DEFAULT_PORT}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serves one canned response, then closes.
    fn serve(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn reports_running_on_a_200() {
        let url = serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");

        assert_eq!(probe(&url), ServerStatus::Running);
    }

    #[test]
    fn reports_stopped_on_an_error_status() {
        let url = serve("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");

        assert_eq!(probe(&url), ServerStatus::Stopped);
    }

    #[test]
    fn reports_stopped_when_nothing_is_listening() {
        // Binding then dropping yields a port with no listener.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        assert_eq!(
            probe(&format!("http://127.0.0.1:{port}")),
            ServerStatus::Stopped
        );
    }

    #[test]
    fn derives_the_authority_from_the_url() {
        assert_eq!(authority("http://127.0.0.1:4123"), "127.0.0.1:4123");
        assert_eq!(authority("http://127.0.0.1:4123/v1"), "127.0.0.1:4123");
        assert_eq!(authority("127.0.0.1:9000"), "127.0.0.1:9000");
        assert_eq!(authority("http://localhost"), "localhost:4123");
    }
}
