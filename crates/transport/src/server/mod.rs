//! The server: answers both handshakes, pings, and requests for registered
//! actions, as a Java node's transport does.
//!
//! One accept thread; a reader thread a channel, which reads whole frames
//! into reserved memory; a fixed pool of workers that run handlers. A full
//! worker queue answers a rejection (429), an unknown action
//! `ActionNotFoundTransportException`, a frame that cannot be held an error
//! for its request id, and the channel stays open. A channel that sends no
//! whole frame in time after connecting, or carries nothing for the idle
//! timeout, is closed, and channels are capped.

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use oss_mem::MemoryBudget;

use crate::channel::{FrameReader, HeldFrame, Inbound};
use crate::exception::RemoteException;
use crate::frame::{FixedHeader, PING, VariableHeader};
use crate::handshake::{TRANSPORT_HANDSHAKE_ACTION, TransportHandshakeResponse};
use crate::node::DiscoveryNode;
use crate::stream::Output;
use crate::version::WireVersion;

mod reply;

use reply::{answer_tcp_handshake, respond, work, write_all};

/// A request a handler answers.
#[derive(Debug)]
pub struct Request<'a> {
    /// The action.
    pub action: &'a str,
    /// The variable header: the sender's request headers.
    pub variable: &'a VariableHeader,
    /// The message, after the parent task id is still to be read.
    pub message: &'a [u8],
    /// The sender's version.
    pub version: WireVersion,
}

/// Answers one action's requests.
pub trait Handler: Send + Sync {
    /// Writes the response message into `out`, or returns the failure to
    /// answer with. Never waits on a transport call of its own.
    ///
    /// # Errors
    ///
    /// The failure the sender gets.
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException>;
}

/// How the server runs.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// This node, as the transport handshake describes it.
    pub node: DiscoveryNode,
    /// This node's `cluster.name`.
    pub cluster_name: String,
    /// The largest frame read or written.
    pub max_frame_bytes: usize,
    /// Worker threads.
    pub workers: usize,
    /// Requests that may wait for a worker.
    pub queue: usize,
    /// The most inbound channels held at once.
    pub max_inbound_channels: usize,
    /// How long a new channel has to send its first whole frame.
    pub handshake_timeout: Duration,
    /// How long a channel may carry nothing before it is closed.
    pub idle_timeout: Duration,
    /// How long a response write may block.
    pub write_timeout: Duration,
}

impl ServerConfig {
    /// The defaults ADR 0016 names, for `node` in `cluster_name`, with
    /// `max_members` peers of 16 channels each.
    pub fn new(node: DiscoveryNode, cluster_name: &str, max_members: usize) -> Self {
        Self {
            node,
            // ALLOC: one name per server.
            cluster_name: cluster_name.to_owned(),
            max_frame_bytes: 100 * 1024 * 1024,
            workers: 8,
            queue: 256,
            max_inbound_channels: max_members.saturating_mul(16),
            handshake_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            write_timeout: Duration::from_secs(30),
        }
    }
}

/// The registered handlers, by action.
pub type Handlers = HashMap<String, Arc<dyn Handler>>;

struct Shared {
    config: ServerConfig,
    handlers: Handlers,
    reader: FrameReader,
    channels: AtomicUsize,
    stopping: AtomicBool,
    /// A handle on every open channel's socket, so stopping closes them.
    open: Mutex<HashMap<u64, TcpStream>>,
    next_channel: AtomicU64,
}

/// Answers `internal:transport/handshake` with this node.
struct Handshake(TransportHandshakeResponse);

impl Handler for Handshake {
    fn handle(&self, _: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        self.0.write(out).map_err(|_| {
            RemoteException::status(
                "the handshake is larger than a transport frame",
                "INTERNAL_SERVER_ERROR",
            )
        })
    }
}

/// One unit of work for a worker.
struct Job {
    frame: HeldFrame,
    writer: Arc<Mutex<TcpStream>>,
}

/// A running server; dropping it stops accepting and closes its channels.
pub struct Server {
    address: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Binds `address` and starts serving `handlers`. Bind before writing
    /// the node's first lease, so a live node's port never refuses.
    ///
    /// # Errors
    ///
    /// The bind's or a thread spawn's failure.
    pub fn bind(
        address: SocketAddr,
        config: ServerConfig,
        budget: MemoryBudget,
        mut handlers: Handlers,
    ) -> io::Result<Self> {
        // The transport handshake runs on a worker like any request: its
        // answer may be compressed, and a reader thread's small stack
        // cannot hold the compressor.
        let handshake = TransportHandshakeResponse {
            node: Some(config.node.clone()),
            cluster_name: config.cluster_name.clone(),
            version: WireVersion::SHELL,
        };
        handlers.insert(
            // ALLOC: one handler entry per server.
            TRANSPORT_HANDSHAKE_ACTION.to_owned(),
            Arc::new(Handshake(handshake)),
        );
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let reader = FrameReader::new(budget, config.max_frame_bytes);
        let shared = Arc::new(Shared {
            config,
            handlers,
            reader,
            channels: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            // ALLOC: one entry per open channel, at most the channel cap.
            open: Mutex::new(HashMap::new()),
            next_channel: AtomicU64::new(0),
        });
        let (jobs, queue) = sync_channel::<Job>(shared.config.queue);
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..shared.config.workers.max(1) {
            let queue = Arc::clone(&queue);
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("transport-worker".into())
                .spawn(move || work(&shared, &queue))?;
        }
        let accept_shared = Arc::clone(&shared);
        let accept = thread::Builder::new()
            .name("transport-accept".into())
            .spawn(move || accept_loop(&listener, &accept_shared, &jobs))?;
        Ok(Self {
            address,
            shared,
            accept: Some(accept),
        })
    }

    /// The bound address.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Inbound channels open now.
    pub fn channels(&self) -> usize {
        self.shared.channels.load(Ordering::Acquire)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
        // Wake the accept loop with a connection it will drop.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let open = std::mem::take(
            &mut *self
                .shared
                .open
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for stream in open.values() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>, jobs: &SyncSender<Job>) {
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::Acquire) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let open = shared.channels.fetch_add(1, Ordering::AcqRel);
        if open >= shared.config.max_inbound_channels {
            shared.channels.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let channel_shared = Arc::clone(shared);
        let jobs = jobs.clone();
        let spawned = thread::Builder::new()
            .name("transport-channel".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let id = channel_shared.next_channel.fetch_add(1, Ordering::Relaxed);
                if let Ok(handle) = stream.try_clone() {
                    channel_shared
                        .open
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id, handle);
                }
                // A stop that began before the insert would miss this channel.
                if !channel_shared.stopping.load(Ordering::Acquire) {
                    serve_channel(stream, &channel_shared, &jobs);
                }
                channel_shared
                    .open
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                channel_shared.channels.fetch_sub(1, Ordering::AcqRel);
            });
        if spawned.is_err() {
            // Out of threads: the closure never ran, so give its count back here.
            shared.channels.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn serve_channel(mut stream: TcpStream, shared: &Arc<Shared>, jobs: &SyncSender<Job>) {
    let config = &shared.config;
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let _ = write_half.set_write_timeout(Some(config.write_timeout));
    let writer = Arc::new(Mutex::new(write_half));
    let _ = stream.set_read_timeout(Some(config.handshake_timeout));
    let mut seen = false;
    loop {
        let inbound = shared
            .reader
            .read(&mut stream, |header, _| precheck(header));
        let inbound = match inbound {
            Ok(Some(inbound)) => inbound,
            Ok(None) | Err(_) => return,
        };
        if !seen {
            // A Java peer makes the TCP handshake on its connection's first
            // channel only; any whole frame shows the channel is the wire.
            seen = true;
            let _ = stream.set_read_timeout(Some(config.idle_timeout));
        }
        match inbound {
            Inbound::Ping => {
                if write_all(&writer, &PING).is_err() {
                    return;
                }
            }
            Inbound::Dropped {
                request_id,
                is_request,
                error,
            } => {
                if is_request
                    && respond(
                        &writer,
                        shared,
                        request_id,
                        "",
                        WireVersion::SHELL,
                        false,
                        Err(error),
                    )
                    .is_err()
                {
                    return;
                }
            }
            Inbound::Frame(frame) if !frame.header.status.is_request() => {
                // A response on a channel this node did not send a request on.
                drop(frame);
                return;
            }
            Inbound::Frame(frame) if frame.header.status.is_handshake() => {
                if answer_tcp_handshake(&writer, shared, &frame).is_err() {
                    return;
                }
            }
            Inbound::Frame(frame) => {
                let (request_id, version, compress) = (
                    frame.header.request_id,
                    frame.header.version,
                    frame.header.status.is_compressed(),
                );
                // ALLOC: one action name per request refused for a full queue.
                let action = frame.variable.action.clone();
                let job = Job {
                    frame,
                    writer: Arc::clone(&writer),
                };
                match jobs.try_send(job) {
                    Ok(()) => {}
                    Err(TrySendError::Full(job)) => {
                        drop(job);
                        let busy =
                            RemoteException::rejected("transport workers are busy; retry later");
                        if respond(
                            &writer,
                            shared,
                            request_id,
                            &action,
                            version,
                            compress,
                            Err(busy),
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }
    }
}

/// The checks a request's headers pass before its message is held: a
/// version this node speaks. The TCP handshake is not required first: a
/// Java peer makes it on one channel of its connection and sends requests
/// on the others without one.
fn precheck(header: &FixedHeader) -> Result<(), RemoteException> {
    if header.status.is_handshake() || header.version.accepted() {
        return Ok(());
    }
    // ALLOC: one message per refused request.
    let message = format!("version {} is not supported", header.version.id());
    Err(RemoteException::status(&message, "BAD_REQUEST"))
}
