//! A one-request-per-connection HTTP server that answers from a queue of
//! canned responses and records every request, in place of the vitest
//! `fetch` mocks.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

pub enum Reply {
    Json(u16, String),
    /// Close the connection without an answer, like a refused request.
    Drop,
}

pub fn json(status: u16, body: serde_json::Value) -> Reply {
    Reply::Json(status, body.to_string())
}

pub fn text(status: u16, body: &str) -> Reply {
    Reply::Json(status, body.to_string())
}

/// An NDJSON body, one line per item.
pub fn ndjson(lines: &[&str]) -> Reply {
    let mut body = String::new();
    for l in lines {
        body.push_str(l);
        body.push('\n');
    }
    Reply::Json(200, body)
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

impl Request {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub struct MockServer {
    pub url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

impl MockServer {
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (q, r) = (queue.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':')
                        && k.eq_ignore_ascii_case("content-length")
                    {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; len];
                let _ = reader.read_exact(&mut body);
                r.lock().unwrap().push(Request {
                    method,
                    path,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let reply = q
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(Reply::Json(500, String::new()));
                if let Reply::Json(status, body) = reply {
                    let head = format!(
                        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reason(status),
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
        Self { url, requests }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.path).collect()
    }

    pub fn called(&self, path: &str) -> bool {
        self.paths().iter().any(|p| p == path)
    }
}
