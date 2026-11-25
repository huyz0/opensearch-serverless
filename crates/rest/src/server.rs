//! The server: one accept thread and a fixed pool of blocking workers.
//!
//! Every resource is bounded when the server starts (docs/adr/0006-http-stack.md):
//! `workers` threads, each with its own head, read and write buffers;
//! `body_buffers` body buffers shared by all; `max_connections` open
//! connections, the ones beyond `workers` waiting in a bounded queue. A
//! connection past the limit is answered 503 and closed at once. A worker
//! whose kept-alive connection sits idle while others wait closes it and takes
//! the next, so idle clients cannot starve waiting ones.

use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use oss_mem::{BufferPool, Exhausted, MemoryBudget, Pool, Reservation};

use crate::body::BodyBuffers;
use crate::config::{ConfigError, ServerConfig};
use crate::conn::{Replay, Shared, Worker, serve};
use crate::parked::{Entry, QueueStats, SearchQueue};
use crate::router::Router;

/// Why the server did not start.
#[derive(Debug)]
pub enum StartError {
    /// The configuration is invalid.
    Config(ConfigError),
    /// The request-body pool cannot hold the server's buffers.
    Memory(Exhausted),
    /// A socket or thread could not be set up.
    Io(io::Error),
}

impl From<io::Error> for StartError {
    fn from(e: io::Error) -> Self {
        StartError::Io(e)
    }
}

impl From<Exhausted> for StartError {
    fn from(e: Exhausted) -> Self {
        StartError::Memory(e)
    }
}

/// Counters, for operators and tests.
#[derive(Debug, Default)]
struct Counters {
    open: AtomicUsize,
    refused: AtomicU64,
    accepted: AtomicU64,
}

/// A running server. Dropping it without [`Server::shutdown`] leaves its
/// threads running until the process exits.
pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
    counters: Arc<Counters>,
    acceptor: Option<JoinHandle<()>>,
    workers: Vec<JoinHandle<()>>,
    /// What the search queue's entries may take, held for the server's life.
    _queue_memory: Option<Reservation>,
}

/// The 503 a connection past `max_connections` gets before it is closed.
const BUSY: &[u8] =
    b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

impl Server {
    /// Starts serving `router` on `listener`, charging every buffer to the
    /// budget's request-body pool first: refused if the pool cannot hold them.
    pub fn start(
        listener: TcpListener,
        config: ServerConfig,
        router: Router,
        budget: &MemoryBudget,
    ) -> Result<Server, StartError> {
        config.validate().map_err(StartError::Config)?;
        let head_size = config.head_buffer_size();
        let heads = BufferPool::new(budget, Pool::RequestBodies, head_size, config.workers)?;
        let reads = BufferPool::new(budget, Pool::RequestBodies, head_size, config.workers)?;
        let writes = BufferPool::new(
            budget,
            Pool::RequestBodies,
            config.write_buffer_size,
            config.workers,
        )?;
        let bodies = Arc::new(BodyBuffers::new(
            budget,
            config.body_buffer_size,
            config.body_buffers,
        )?);
        // The queue's entries each copy up to one head buffer: charged at
        // start, so a full queue cannot starve the body buffers.
        let admission = router
            .search_admission()
            .filter(|_| config.search_queue_size > 0);
        let queue_memory = match &admission {
            Some(_) => Some(budget.try_reserve(
                Pool::RequestBodies,
                config.search_queue_size.saturating_mul(head_size),
            )?),
            None => None,
        };
        let queue = admission.map(|admission| {
            SearchQueue::new(
                admission,
                config.search_queue_size,
                config.search_queue_wait,
                config.idle_poll,
            )
        });
        let addr = listener.local_addr()?;
        let (tx, rx) = sync_channel::<TcpStream>(config.max_connections);
        let rx = Arc::new(Mutex::new(rx));
        let shared = Arc::new(Shared {
            config,
            router,
            budget: budget.clone(),
            bodies,
            queued: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            queue,
        });
        let counters = Arc::new(Counters::default());
        // ALLOC: startup only; one handle per worker.
        let mut workers = Vec::with_capacity(shared.config.workers);
        for i in 0..shared.config.workers {
            let worker = Worker::new(
                &shared.config,
                heads.try_checkout()?,
                reads.try_checkout()?,
                writes.try_checkout()?,
            );
            let (rx, shared, counters) =
                (Arc::clone(&rx), Arc::clone(&shared), Arc::clone(&counters));
            let handle = thread::Builder::new()
                // ALLOC: startup only; one name per worker.
                .name(format!("oss-http-{i}"))
                .spawn(move || work(worker, &rx, &shared, &counters))?;
            workers.push(handle);
        }
        let acceptor = {
            let (shared, counters) = (Arc::clone(&shared), Arc::clone(&counters));
            thread::Builder::new()
                // ALLOC: startup only; the acceptor's name.
                .name("oss-http-accept".to_owned())
                .spawn(move || accept(&listener, &tx, &shared, &counters))?
        };
        Ok(Server {
            addr,
            shared,
            counters,
            acceptor: Some(acceptor),
            workers,
            _queue_memory: queue_memory,
        })
    }

    /// The address the server listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Connections open now, served and waiting.
    pub fn open_connections(&self) -> usize {
        self.counters.open.load(Ordering::Acquire)
    }

    /// Connections refused because `max_connections` were open.
    pub fn refused_connections(&self) -> u64 {
        self.counters.refused.load(Ordering::Acquire)
    }

    /// Connections accepted and served since the start.
    pub fn accepted_connections(&self) -> u64 {
        self.counters.accepted.load(Ordering::Acquire)
    }

    /// What the search queue holds and has done; all zero with no queue.
    pub fn search_queue(&self) -> QueueStats {
        self.shared
            .queue
            .as_ref()
            .map(SearchQueue::stats)
            .unwrap_or_default()
    }

    /// Stops accepting, lets each worker finish the request it is serving,
    /// closes idle connections within one idle poll, and joins every thread.
    pub fn shutdown(mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        // Wake the acceptor, which blocks in `accept`.
        drop(TcpStream::connect(wake_addr(self.addr)));
        if let Some(a) = self.acceptor.take() {
            let _ = a.join();
        }
        // Parked searches are answered at once, not after the searches that
        // hold the workers; what a worker parks while stopping is drained
        // after the join.
        if let Some(queue) = &self.shared.queue {
            queue.drain(&self.counters.open);
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        if let Some(queue) = &self.shared.queue {
            queue.drain(&self.counters.open);
        }
    }
}

/// Where to connect to wake the acceptor: an unspecified listen address
/// (`0.0.0.0`, `::`) is not connectable on every platform, so its loopback.
fn wake_addr(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port())
        }
        _ => addr,
    }
}

/// Counts a served connection as closed when dropped, so the count stays
/// right even if serving it panics.
struct Open<'c>(&'c Counters);

impl Drop for Open<'_> {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

fn accept(
    listener: &TcpListener,
    tx: &SyncSender<TcpStream>,
    shared: &Shared,
    counters: &Counters,
) {
    for stream in listener.incoming() {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let open = counters.open.fetch_add(1, Ordering::AcqRel);
        if open >= shared.config.max_connections {
            counters.open.fetch_sub(1, Ordering::AcqRel);
            counters.refused.fetch_add(1, Ordering::AcqRel);
            let _ = stream.set_write_timeout(Some(shared.config.request_timeout));
            let mut s = &stream;
            let _ = s.write_all(BUSY);
            continue;
        }
        shared.queued.fetch_add(1, Ordering::AcqRel);
        match tx.try_send(stream) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                // The open count keeps the queue below its capacity, so this
                // only happens while workers are gone.
                shared.queued.fetch_sub(1, Ordering::AcqRel);
                counters.open.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

fn work(mut worker: Worker, rx: &Mutex<Receiver<TcpStream>>, shared: &Shared, counters: &Counters) {
    loop {
        // A resumed search has waited already: it goes before new connections.
        if let Some(queue) = &shared.queue
            && let Some((entry, permit)) = queue.take_resumed()
        {
            let replay = Replay {
                bytes: entry.bytes,
                permit,
            };
            run(&mut worker, entry.stream, shared, counters, Some(replay));
            // The permit may have gone unused (a denied replay, a failed
            // read): a parked search of that tenant may run now.
            queue.offer();
            queue.sweep(&counters.open);
            continue;
        }
        let next = {
            let rx = rx.lock().unwrap_or_else(PoisonError::into_inner);
            match &shared.queue {
                // Wakes to take resumed entries and to sweep the queue.
                Some(_) => rx.recv_timeout(shared.config.idle_poll),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            }
        };
        match next {
            Ok(stream) => {
                shared.queued.fetch_sub(1, Ordering::AcqRel);
                counters.accepted.fetch_add(1, Ordering::AcqRel);
                run(&mut worker, stream, shared, counters, None);
                // Under steady load the idle timeout never fires: sweep here
                // (rate-limited inside) so waits end and closed peers go.
                if let Some(queue) = &shared.queue {
                    queue.sweep(&counters.open);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(queue) = &shared.queue {
                    queue.sweep(&counters.open);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Serves one connection, new or resumed; a connection parked by the search
/// queue stays counted open until its entry leaves the queue.
fn run(
    worker: &mut Worker,
    stream: TcpStream,
    shared: &Shared,
    counters: &Counters,
    replay: Option<Replay>,
) {
    let open = Open(counters);
    // A handler's panic is answered inside `serve`; one anywhere else
    // drops this connection, not the worker.
    let served = panic::catch_unwind(AssertUnwindSafe(|| serve(worker, &stream, shared, replay)));
    if let (Ok(Some(ticket)), Some(queue)) = (served, &shared.queue) {
        std::mem::forget(open);
        queue.park(
            Entry::new(stream, ticket.bytes, ticket.tenant),
            &counters.open,
        );
        return;
    }
    drop(stream);
    drop(open);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_acceptor_is_woken_on_loopback_for_an_unspecified_address() {
        let v4: SocketAddr = "0.0.0.0:9200".parse().expect("addr");
        assert_eq!(wake_addr(v4), "127.0.0.1:9200".parse().expect("addr"));
        let v6: SocketAddr = "[::]:9200".parse().expect("addr");
        assert_eq!(wake_addr(v6), "[::1]:9200".parse().expect("addr"));
        let given: SocketAddr = "10.1.2.3:9200".parse().expect("addr");
        assert_eq!(wake_addr(given), given);
    }

    #[test]
    fn a_server_on_an_unspecified_address_shuts_down() {
        let listener = TcpListener::bind("0.0.0.0:0").expect("binds");
        let budget = MemoryBudget::new(
            &oss_mem::BudgetConfig::new(64 << 20)
                .with_pool(Pool::RequestBodies, 64 << 20)
                .expect("fits"),
        );
        let config = ServerConfig {
            workers: 1,
            max_connections: 1,
            body_buffers: 1,
            ..ServerConfig::default()
        };
        let server = Server::start(listener, config, Router::new(), &budget).expect("starts");
        server.shutdown();
    }
}
