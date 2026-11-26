//! A TCP proxy a node's object-store traffic can be frozen on: while frozen
//! it holds every byte in both directions without closing a connection, so
//! the node sees a stall rather than an error, and when thawed it delivers
//! what it held in order. This is the stalled-process fault with the node's
//! own timers left running (docs/specs/frozen-node-suite).

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The most bytes a direction of one connection holds while frozen.
const CHUNK: usize = 16 * 1024;

/// A proxy to one upstream address, freezable.
pub struct Freezer {
    addr: SocketAddr,
    frozen: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

/// Copies `from` to `to` a chunk at a time, holding each chunk it has read
/// while `frozen`.
fn pump(mut from: TcpStream, mut to: TcpStream, frozen: &AtomicBool, stop: &AtomicBool) {
    let _ = from.set_read_timeout(Some(Duration::from_millis(20)));
    let mut buf = vec![0u8; CHUNK];
    while !stop.load(Ordering::Relaxed) {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                while frozen.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

impl Freezer {
    /// A proxy to `upstream` (`host:port`), listening on a free local port.
    pub fn start(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let addr = listener.local_addr().expect("an address");
        let frozen = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let upstream = upstream.to_owned();
        let (f, s) = (Arc::clone(&frozen), Arc::clone(&stop));
        std::thread::spawn(move || {
            for client in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(client) = client else { continue };
                let Ok(server) = TcpStream::connect(&upstream) else {
                    continue;
                };
                let (Ok(c2), Ok(s2)) = (client.try_clone(), server.try_clone()) else {
                    continue;
                };
                let (f1, s1) = (Arc::clone(&f), Arc::clone(&s));
                std::thread::spawn(move || pump(client, server, &f1, &s1));
                let (f2, s2s) = (Arc::clone(&f), Arc::clone(&s));
                std::thread::spawn(move || pump(s2, c2, &f2, &s2s));
            }
        });
        Self { addr, frozen, stop }
    }

    /// The URL a client of the upstream uses to go through this proxy.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// From now on, holds every byte it reads.
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::Relaxed);
    }

    /// Delivers what is held and passes bytes on again.
    pub fn thaw(&self) {
        self.frozen.store(false, Ordering::Relaxed);
    }
}

impl Drop for Freezer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
    }
}
