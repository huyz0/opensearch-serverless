//! A loopback proxy between a client and the emulator's S3 front end that
//! counts the GETs in flight through it and can hold them, so a test
//! asserts concurrency from counts rather than from wall-clock time.
//!
//! A GET is in flight from the moment its request head reaches the proxy
//! until its whole response has come back from the server. The count drops
//! before the response is passed on, so the next request a client sends on
//! the same connection is always counted after it: the peak is never one
//! too high. HEADs pass straight through and are only counted once
//! answered. Requests and responses are HTTP/1.1 framed by
//! `Content-Length`; the S3 front end answers GET and HEAD that way.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

#[derive(Debug, Default)]
struct Counts {
    /// GETs wait at the proxy, unforwarded, while this is set.
    hold: bool,
    /// GETs waiting at the proxy now.
    held: usize,
    /// GETs in flight now, held or forwarded.
    in_flight: usize,
    /// The most GETs ever in flight at once.
    peak: usize,
    /// HEADs answered.
    heads: usize,
    /// GETs forwarded to the server.
    gets: usize,
}

#[derive(Debug, Default)]
struct State {
    counts: Mutex<Counts>,
    changed: Condvar,
}

impl State {
    fn counts(&self) -> MutexGuard<'_, Counts> {
        self.counts.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The proxy. It runs until the test process ends.
pub(crate) struct Gate {
    addr: SocketAddr,
    state: Arc<State>,
}

impl Gate {
    /// Proxies to the server at `endpoint` (`http://host:port`).
    pub(crate) fn start(endpoint: &str) -> Self {
        let upstream: SocketAddr = endpoint
            .strip_prefix("http://")
            .and_then(|a| a.parse().ok())
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                std::thread::spawn(move || relay(&state, client, upstream));
            }
        });
        Self { addr, state }
    }

    /// The endpoint a client connects to.
    pub(crate) fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Holds every GET that arrives from now on until [`Gate::release`].
    pub(crate) fn hold(&self) {
        self.state.counts().hold = true;
    }

    /// Forwards the held GETs and every later one.
    pub(crate) fn release(&self) {
        self.state.counts().hold = false;
        self.state.changed.notify_all();
    }

    /// Waits until `n` GETs are held, or `deadline` passes, and returns how
    /// many are held. The deadline only stops a broken client hanging the
    /// test; a working one reaches `n` whatever the machine's speed.
    pub(crate) fn wait_held(&self, n: usize, deadline: Instant) -> usize {
        let mut counts = self.state.counts();
        while counts.held < n {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            counts = self
                .state
                .changed
                .wait_timeout(counts, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        counts.held
    }

    /// The most GETs ever in flight at once.
    pub(crate) fn peak(&self) -> usize {
        self.state.counts().peak
    }

    /// GETs forwarded to the server so far.
    pub(crate) fn gets(&self) -> usize {
        self.state.counts().gets
    }

    /// HEADs answered so far.
    pub(crate) fn heads(&self) -> usize {
        self.state.counts().heads
    }
}

/// Relays one client connection, a request at a time.
fn relay(state: &State, client: TcpStream, upstream: SocketAddr) {
    let Ok(server) = TcpStream::connect(upstream) else {
        return;
    };
    let (Ok(client_out), Ok(server_in)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    // One request at a time on a connection: HTTP/1.1, no pipelining.
    let (methods, answers) = sync_channel(1);
    std::thread::scope(|s| {
        s.spawn(move || answer(state, server_in, client_out, answers));
        let mut from_client = BufReader::new(client);
        let mut to_server = server;
        while let Some((head, len)) = read_head(&mut from_client) {
            let method = head.split(' ').next().unwrap_or_default().to_owned();
            if method == "GET" {
                let mut counts = state.counts();
                counts.in_flight = counts.in_flight.saturating_add(1);
                counts.peak = counts.peak.max(counts.in_flight);
                counts.held = counts.held.saturating_add(1);
                state.changed.notify_all();
                while counts.hold {
                    counts = state
                        .changed
                        .wait(counts)
                        .unwrap_or_else(|e| e.into_inner());
                }
                counts.held = counts.held.saturating_sub(1);
                counts.gets = counts.gets.saturating_add(1);
            }
            if methods.send(method).is_err()
                || to_server.write_all(head.as_bytes()).is_err()
                || copy(&mut from_client, &mut to_server, len).is_err()
            {
                break;
            }
        }
        drop(methods);
        let _ = to_server.shutdown(std::net::Shutdown::Write);
    });
}

/// Passes each response back to the client, counting it done first.
fn answer(state: &State, server: TcpStream, mut client: TcpStream, methods: Receiver<String>) {
    let mut from_server = BufReader::new(server);
    for method in methods {
        let Some((head, len)) = read_head(&mut from_server) else {
            break;
        };
        let len = if method == "HEAD" { 0 } else { len };
        let mut body = vec![0u8; usize::try_from(len).unwrap()];
        if from_server.read_exact(&mut body).is_err() {
            break;
        }
        {
            let mut counts = state.counts();
            match method.as_str() {
                "GET" => counts.in_flight = counts.in_flight.saturating_sub(1),
                "HEAD" => counts.heads = counts.heads.saturating_add(1),
                _ => {}
            }
        }
        if client.write_all(head.as_bytes()).is_err() || client.write_all(&body).is_err() {
            break;
        }
    }
    // A server that closed or failed closes the client's connection too,
    // rather than leaving it waiting out its I/O timeout.
    let _ = client.shutdown(std::net::Shutdown::Both);
}

/// Reads one message head, through its blank line, and the body length it
/// declares. `None` once the peer has closed. A chunked body would be
/// misframed, so it panics on one.
fn read_head(from: &mut impl BufRead) -> Option<(String, u64)> {
    let mut head = String::new();
    let mut len = 0;
    loop {
        let mut line = String::new();
        if from.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let lower = line.to_ascii_lowercase();
        assert!(
            !lower.starts_with("transfer-encoding:"),
            "the gate frames by content-length only"
        );
        if let Some(value) = lower.strip_prefix("content-length:") {
            len = value.trim().parse().ok()?;
        }
        head.push_str(&line);
        if line == "\r\n" {
            return Some((head, len));
        }
    }
}

fn copy(from: &mut impl Read, to: &mut impl Write, len: u64) -> std::io::Result<()> {
    let copied = std::io::copy(&mut from.take(len), to)?;
    if copied == len {
        Ok(())
    } else {
        Err(std::io::ErrorKind::UnexpectedEof.into())
    }
}
