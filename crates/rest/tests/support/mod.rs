//! A minimal HTTP client and server setup for the integration tests.

#![allow(dead_code)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_rest::{Router, Server, ServerConfig};

/// A reply as the client read it.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!("json body ({e}): {:?}", String::from_utf8_lossy(&self.body))
        })
    }
}

/// A client on one connection.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    pub fn connect(addr: SocketAddr) -> Client {
        let s = TcpStream::connect(addr).expect("connects");
        s.set_read_timeout(Some(Duration::from_secs(20)))
            .expect("timeout");
        Client {
            writer: s.try_clone().expect("clones"),
            reader: BufReader::new(s),
        }
    }

    pub fn send(&mut self, raw: &[u8]) -> io::Result<()> {
        self.writer.write_all(raw)
    }

    pub fn request(&mut self, raw: &[u8]) -> Reply {
        self.send(raw).expect("sends");
        self.reply().expect("a reply")
    }

    pub fn reply(&mut self) -> io::Result<Reply> {
        self.read_reply(false)
    }

    /// Reads a reply that has no body whatever its headers say (HEAD).
    pub fn reply_head(&mut self) -> io::Result<Reply> {
        self.read_reply(true)
    }

    fn line(&mut self) -> io::Result<String> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed"));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    fn read_reply(&mut self, head_only: bool) -> io::Result<Reply> {
        let status_line = self.line()?;
        let status: u16 = status_line
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::other(format!("bad status line {status_line:?}")))?;
        let mut headers = Vec::new();
        loop {
            let l = self.line()?;
            if l.is_empty() {
                break;
            }
            let (n, v) = l.split_once(':').expect("header");
            headers.push((n.trim().to_owned(), v.trim().to_owned()));
        }
        let mut reply = Reply {
            status,
            headers,
            body: Vec::new(),
        };
        if head_only || status == 100 {
            return Ok(reply);
        }
        if reply.header("transfer-encoding") == Some("chunked") {
            loop {
                let size = usize::from_str_radix(&self.line()?, 16).expect("chunk size");
                if size == 0 {
                    assert_eq!(self.line()?, "");
                    break;
                }
                let start = reply.body.len();
                reply.body.resize(start.saturating_add(size), 0);
                self.reader.read_exact(&mut reply.body[start..])?;
                assert_eq!(self.line()?, "");
            }
        } else {
            let n: usize = reply
                .header("content-length")
                .expect("a length")
                .parse()
                .expect("number");
            reply.body.resize(n, 0);
            self.reader.read_exact(&mut reply.body)?;
        }
        Ok(reply)
    }

    /// Whether the server closed the connection.
    pub fn is_closed(&mut self) -> bool {
        let mut b = [0u8; 1];
        matches!(self.reader.read(&mut b), Ok(0) | Err(_))
    }

    pub fn writer(&self) -> &TcpStream {
        &self.writer
    }
}

/// A budget whose request-body pool is `bytes`.
pub fn budget(bytes: usize) -> MemoryBudget {
    let c = BudgetConfig::new(bytes)
        .with_pool(Pool::RequestBodies, bytes)
        .expect("fits");
    MemoryBudget::new(&c)
}

/// A small server configuration for tests.
pub fn small() -> ServerConfig {
    ServerConfig {
        workers: 4,
        max_connections: 16,
        body_buffer_size: 4096,
        body_buffers: 4,
        body_wait: Duration::ZERO,
        write_buffer_size: 4096,
        idle_timeout: Duration::from_secs(10),
        request_timeout: Duration::from_secs(10),
        idle_poll: Duration::from_millis(20),
        ..ServerConfig::default()
    }
}

pub fn start(config: ServerConfig, router: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    Server::start(listener, config, router, &budget(64 << 20)).expect("starts")
}

/// `POST path` with a JSON body.
pub fn post(path: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nHost: t\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

pub fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: t\r\n\r\n").into_bytes()
}
