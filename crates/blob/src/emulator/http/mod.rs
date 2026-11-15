//! A minimal S3 HTTP front end for the emulator, so the S3 client is
//! tested over real sockets without Docker or a network (AGENTS.md
//! invariant 7).
//!
//! [`S3Server`] listens on a loopback port and answers path-style S3
//! requests (`/bucket/key`) from an [`Emulator`]: GET (whole or ranged),
//! PUT (with `If-None-Match: *`), HEAD, DELETE and `ListObjectsV2`. Every
//! request's SigV4 signature is checked by deriving its canonical request
//! again from what arrived, and a PUT without `Content-Length` is refused,
//! as S3 refuses it. Faults come from the emulator's own injection, plus
//! two the emulator cannot express: a scripted status
//! ([`S3Server::answer_next`]) and a body cut part-way
//! ([`S3Server::cut_body_next`]).
//!
//! It is test code behind the `emulator` feature: one thread per
//! connection, at most [`MAX_CONNECTIONS`] of them.

mod routes;
mod wire;

use std::collections::VecDeque;
use std::io::{BufReader, BufWriter, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::{Emulator, lock};
use crate::s3::sigv4::Credentials;
use wire::{MAX_LINE, ReadError, SocketBody, read_head, write_error};

/// The most connections served at once; more are closed on arrival.
pub const MAX_CONNECTIONS: usize = 256;

/// Requests kept in the log [`S3Server::seen`] returns.
const LOG: usize = 256;

/// One request as the server received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// `GET`, `PUT`, ...
    pub method: String,
    /// The path and query, still encoded.
    pub target: String,
    /// Header names lowercased, values as sent, in the order sent.
    pub headers: Vec<(String, String)>,
}

impl Seen {
    /// The first value of header `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.trim())
    }
}

#[derive(Debug, Default)]
struct Script {
    /// Answer the next `n` signed requests with this status and code.
    answer: Option<(u16, &'static str, u32)>,
    /// Cut the next GET body after this many bytes.
    cut: Option<u64>,
}

/// State every connection thread shares.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) store: Arc<Emulator>,
    pub(crate) bucket: String,
    pub(crate) region: String,
    pub(crate) credentials: Credentials,
    script: Mutex<Script>,
    requests: AtomicU64,
    log: Mutex<VecDeque<Seen>>,
    stop: AtomicBool,
    connections: Mutex<Vec<(u64, TcpStream)>>,
}

impl Shared {
    /// The scripted answer for this request, if one is pending.
    pub(crate) fn scripted_answer(&self) -> Option<(u16, &'static str)> {
        let mut script = lock(&self.script);
        let (status, code, left) = script.answer?;
        script.answer = left
            .checked_sub(1)
            .filter(|n| *n > 0)
            .map(|n| (status, code, n));
        Some((status, code))
    }

    /// The scripted cut for this GET body, if one is pending.
    pub(crate) fn take_cut(&self) -> Option<u64> {
        lock(&self.script).cut.take()
    }
}

/// An S3 HTTP front end over an [`Emulator`], on `127.0.0.1` and a port of
/// the system's choosing. Dropping it stops the server and closes every
/// connection.
#[derive(Debug)]
pub struct S3Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl S3Server {
    /// Serves `store` as `bucket`, accepting only requests signed by
    /// `credentials` for `region`.
    pub fn start(
        store: Arc<Emulator>,
        bucket: &str,
        region: &str,
        credentials: Credentials,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            store,
            // ALLOC: once per server.
            bucket: bucket.to_owned(),
            // ALLOC: once per server.
            region: region.to_owned(),
            credentials,
            script: Mutex::new(Script::default()),
            requests: AtomicU64::new(0),
            // ALLOC: at most LOG requests, each one bounded head.
            log: Mutex::new(VecDeque::with_capacity(LOG)),
            stop: AtomicBool::new(false),
            // ALLOC: at most MAX_CONNECTIONS entries.
            connections: Mutex::new(Vec::with_capacity(MAX_CONNECTIONS)),
        });
        let accepting = Arc::clone(&shared);
        let accept = std::thread::Builder::new()
            // ALLOC: one thread name.
            .name("s3-emulator-accept".to_owned())
            .spawn(move || accept_loop(&listener, &accepting))?;
        Ok(Self {
            addr,
            shared,
            accept: Some(accept),
        })
    }

    /// `http://127.0.0.1:<port>`, the endpoint a client is given.
    pub fn endpoint(&self) -> String {
        // ALLOC: one URL.
        format!("http://{}", self.addr)
    }

    /// The emulator behind the server.
    pub fn store(&self) -> &Emulator {
        &self.shared.store
    }

    /// Answers the next `n` correctly signed requests with `status` and an
    /// S3 error document carrying `code`, without touching the emulator.
    pub fn answer_next(&self, status: u16, code: &'static str, n: u32) {
        lock(&self.shared.script).answer = (n > 0).then_some((status, code, n));
    }

    /// Cuts the next GET's body after `after` bytes and closes its
    /// connection, as a connection lost part-way through a download.
    pub fn cut_body_next(&self, after: u64) {
        lock(&self.shared.script).cut = Some(after);
    }

    /// Requests received so far, whether or not they were served.
    pub fn requests(&self) -> u64 {
        self.shared.requests.load(Ordering::Relaxed)
    }

    /// The most recent requests received, oldest first, at most 256.
    pub fn seen(&self) -> Vec<Seen> {
        // ALLOC: at most LOG entries, for a test to inspect.
        lock(&self.shared.log).iter().cloned().collect()
    }
}

impl Drop for S3Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop, which then sees the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        for (_, conn) in lock(&self.shared.connections).drain(..) {
            let _ = conn.shutdown(Shutdown::Both);
        }
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    let mut next_id = 0u64;
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let id = next_id;
        next_id = next_id.wrapping_add(1);
        {
            let mut conns = lock(&shared.connections);
            let Ok(clone) = stream.try_clone() else {
                continue;
            };
            if conns.len() >= MAX_CONNECTIONS {
                continue;
            }
            conns.push((id, clone));
        }
        let serving = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            // ALLOC: one thread name per connection.
            .name("s3-emulator-conn".to_owned())
            .spawn(move || {
                serve(&serving, stream);
                forget(&serving, id);
            });
        if spawned.is_err() {
            forget(shared, id);
        }
    }
}

/// Drops connection `id` from the set a dropped server shuts down.
fn forget(shared: &Shared, id: u64) {
    lock(&shared.connections).retain(|(i, _)| *i != id);
}

/// Serves requests on one connection until it closes or a reply needs it
/// closed.
fn serve(shared: &Shared, stream: TcpStream) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    // ALLOC: 16 KiB per connection, at most MAX_CONNECTIONS of them.
    let mut reader = BufReader::with_capacity(16 * 1024, read_half);
    // ALLOC: 16 KiB per connection, at most MAX_CONNECTIONS of them.
    let mut out = BufWriter::with_capacity(16 * 1024, stream);
    // ALLOC: one line buffer per connection, MAX_LINE bytes.
    let mut line = Vec::with_capacity(MAX_LINE);
    loop {
        let head = match read_head(&mut reader, &mut line) {
            Ok(head) => head,
            Err(ReadError::Closed) => return,
            Err(ReadError::Malformed(_)) => {
                let _ = write_error(&mut out, 400, "BadRequest", true, false);
                let _ = out.flush();
                return;
            }
        };
        shared.requests.fetch_add(1, Ordering::Relaxed);
        {
            let mut log = lock(&shared.log);
            if log.len() >= LOG {
                log.pop_front();
            }
            log.push_back(Seen {
                // ALLOC: one request head per entry, at most LOG entries.
                method: head.method.clone(),
                // ALLOC: as above.
                target: if head.query.is_empty() {
                    head.path.clone()
                } else {
                    // ALLOC: as above.
                    format!("{}?{}", head.path, head.query)
                },
                // ALLOC: as above.
                headers: head.headers.clone(),
            });
        }
        if head.header("transfer-encoding").is_some() {
            let _ = write_error(&mut out, 411, "MissingContentLength", true, false);
            let _ = out.flush();
            return;
        }
        let len = match head.header("content-length").map(str::parse::<u64>) {
            None => 0,
            Some(Ok(n)) => n,
            Some(Err(_)) => {
                let _ = write_error(&mut out, 400, "BadRequest", true, false);
                let _ = out.flush();
                return;
            }
        };
        let mut body = SocketBody {
            reader: &mut reader,
            len,
            left: len,
        };
        let reply = routes::respond(shared, &head, &mut body, &mut out);
        let drained = body.drain();
        let close = !drained
            || reply == routes::Reply::Close
            || head.header("connection") == Some("close")
            || shared.stop.load(Ordering::SeqCst);
        let head_only = head.method == "HEAD";
        if routes::finish(&reply, &mut out, close, head_only).is_err() || out.flush().is_err() {
            return;
        }
        if close {
            return;
        }
    }
}
