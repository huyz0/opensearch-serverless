//! The client: sends requests to a peer, Rust or Java, and waits for their
//! answers.
//!
//! Towards each peer it keeps up to `connections_per_peer` sockets, each
//! carrying many requests at once by request id, with one reader thread
//! routing answers to their waiters. A socket is opened with both
//! handshakes, under the caller's deadline: a frozen process accepts TCP and
//! hangs the handshake. Failures are told apart so a caller can act on the
//! one that matters: a refused connection (nothing listens on the peer's
//! port) is not a timeout. Retrying is the caller's choice.

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oss_mem::MemoryBudget;

use crate::channel::{ChannelError, FrameReader, HeldFrame, Inbound};
use crate::error::WireError;
use crate::exception::RemoteException;
use crate::frame::{self, FrameMeta, Status, VariableHeader};
use crate::handshake::{
    TCP_HANDSHAKE_ACTION, TRANSPORT_HANDSHAKE_ACTION, TcpHandshakeRequest, TcpHandshakeResponse,
    TransportHandshakeRequest, TransportHandshakeResponse,
};
use crate::node::DiscoveryNode;
use crate::stream::{Input, Output, TaskId};
use crate::version::WireVersion;

mod socket;

use socket::{classify, connect_error, dropped, io_error, remaining, route, send_and_wait};

/// Why a call failed.
#[derive(Debug)]
pub enum CallError {
    /// The peer's host refused the connection: nothing listens on its port.
    Refused,
    /// No answer before the deadline (connect, handshake or response), or
    /// the host or network is unreachable.
    Timeout,
    /// The socket was reset or closed during the call, or its bytes were not
    /// the wire.
    Io(io::ErrorKind),
    /// Every socket to the peer is in use or the client's socket cap is
    /// reached, and none came free before the deadline.
    Busy,
    /// The peer answered a failure.
    Remote(RemoteException),
    /// The peer refused the request's authentication (403 or 401).
    Unauthorized(RemoteException),
    /// The request or the answer is larger than a frame.
    TooLarge,
    /// The node that answered is not the one expected, or speaks another
    /// version.
    WrongPeer(&'static str),
}

/// How the client runs.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The largest frame sent or read.
    pub max_frame_bytes: usize,
    /// Sockets to one peer.
    pub connections_per_peer: usize,
    /// Sockets to all peers.
    pub max_outbound_sockets: usize,
    /// The longest a connect may take, within the caller's deadline.
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 100 * 1024 * 1024,
            connections_per_peer: 4,
            max_outbound_sockets: 4096,
            connect_timeout: Duration::from_secs(1),
        }
    }
}

/// A peer as a lease names it: where it listens and which process it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerId {
    /// The transport address.
    pub address: SocketAddr,
    /// The process's ephemeral id; a node answering under another is refused.
    pub ephemeral_id: Option<String>,
}

/// An answered call.
#[derive(Debug)]
pub struct Answer {
    /// The response message, held in reserved memory.
    pub frame: HeldFrame,
}

impl Answer {
    /// The response message.
    pub fn message(&self) -> &[u8] {
        &self.frame.message
    }
}

type Waiter = SyncSender<Result<HeldFrame, CallError>>;

/// One socket to a peer.
struct Connection {
    writer: Mutex<TcpStream>,
    waiters: Mutex<HashMap<i64, Waiter>>,
    in_flight: AtomicUsize,
    dead: AtomicBool,
    version: WireVersion,
    /// The ephemeral id of the process that answered the handshake.
    ephemeral_id: String,
}

impl Connection {
    /// Closes the socket; its reader fails what is in flight.
    fn close(&self) {
        let writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writer.shutdown(std::net::Shutdown::Both);
    }

    fn fail_all(&self, kind: io::ErrorKind) {
        self.dead.store(true, Ordering::Release);
        let waiters =
            std::mem::take(&mut *self.waiters.lock().unwrap_or_else(PoisonError::into_inner));
        for (_, waiter) in waiters {
            let _ = waiter.try_send(Err(CallError::Io(kind)));
        }
    }
}

/// One peer's sockets.
#[derive(Default)]
struct PeerSlot {
    connections: Vec<Arc<Connection>>,
    /// Sockets being opened, counted against the per-peer cap.
    opening: usize,
}

/// Sends requests to peers.
pub struct Client {
    config: ClientConfig,
    reader: FrameReader,
    peers: Mutex<HashMap<SocketAddr, PeerSlot>>,
    sockets: Arc<AtomicUsize>,
    next_id: AtomicI64,
}

impl Drop for Client {
    /// Closes every socket, so their reader threads end.
    fn drop(&mut self) {
        let peers = std::mem::take(&mut *self.peers.lock().unwrap_or_else(PoisonError::into_inner));
        for connection in peers.values().flat_map(|slot| &slot.connections) {
            connection.close();
        }
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What a call sends.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    /// The action.
    pub action: &'a str,
    /// Request headers (the MAC goes here).
    pub headers: &'a [(String, String)],
    /// Whether to compress the request (towards a peer in another zone).
    pub compress: bool,
    /// When to give up.
    pub deadline: Instant,
}

impl Client {
    /// A client whose frames are charged to `budget`'s transport pool.
    pub fn new(config: ClientConfig, budget: MemoryBudget) -> Self {
        let reader = FrameReader::new(budget, config.max_frame_bytes);
        Self {
            config,
            reader,
            // ALLOC: one entry per peer called, at most the fleet's members.
            peers: Mutex::new(HashMap::new()),
            sockets: Arc::new(AtomicUsize::new(0)),
            next_id: AtomicI64::new(1),
        }
    }

    /// Sockets open now.
    pub fn sockets(&self) -> usize {
        self.sockets.load(Ordering::Acquire)
    }

    /// Sends `call` to `peer` with the message `write_message` writes after
    /// the request's parent task id, and waits for the answer.
    ///
    /// # Errors
    ///
    /// [`CallError`], telling the failures apart.
    pub fn call<F>(
        &self,
        peer: &PeerId,
        call: Call<'_>,
        write_message: F,
    ) -> Result<Answer, CallError>
    where
        F: FnOnce(&mut Output<'_>) -> Result<(), WireError>,
    {
        let connection = self.connection(peer, call.deadline)?;
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let variable = VariableHeader {
            // ALLOC: the call's headers, a handful.
            request_headers: call.headers.to_vec(),
            // ALLOC: one action name per call.
            action: call.action.to_owned(),
            ..VariableHeader::default()
        };
        let meta = FrameMeta {
            request_id,
            status: Status::request(call.compress, false),
            version: connection.version,
        };
        // ALLOC: one request frame, at most the frame bound.
        let mut buf = Vec::new();
        frame::encode(
            &mut buf,
            self.config.max_frame_bytes,
            meta,
            &variable,
            |o| {
                TaskId::EMPTY.write(o)?;
                write_message(o)
            },
        )
        .map_err(|e| match e {
            WireError::TooLarge { .. } => CallError::TooLarge,
            _ => CallError::Io(io::ErrorKind::InvalidData),
        })?;
        let (waiter, answer) = sync_channel(1);
        connection
            .waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request_id, waiter);
        connection.in_flight.fetch_add(1, Ordering::AcqRel);
        let result = send_and_wait(&connection, &buf, &answer, call.deadline);
        connection.in_flight.fetch_sub(1, Ordering::AcqRel);
        connection
            .waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request_id);
        let frame = result?;
        if frame.header.status.is_error() {
            let failure = RemoteException::read(&mut Input::new(&frame.message))
                .ok()
                .flatten()
                .unwrap_or_default();
            return Err(classify(failure));
        }
        Ok(Answer { frame })
    }

    /// A socket to `peer` with room for another request, opened if none has.
    ///
    /// An idle socket is reused; otherwise a socket is opened while the
    /// peer has fewer than `connections_per_peer` open or opening; at the
    /// cap the least busy is shared. A socket to another process at the
    /// address (another ephemeral id) is closed, never reused.
    fn connection(&self, peer: &PeerId, deadline: Instant) -> Result<Arc<Connection>, CallError> {
        loop {
            let open_one = {
                let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
                let slot = peers.entry(peer.address).or_default();
                slot.connections.retain(|c| {
                    let stale = peer
                        .ephemeral_id
                        .as_ref()
                        .is_some_and(|expected| *expected != c.ephemeral_id);
                    if stale {
                        c.close();
                    }
                    !stale && !c.dead.load(Ordering::Acquire)
                });
                let room = slot.connections.len().saturating_add(slot.opening)
                    < self.config.connections_per_peer;
                let least_busy = slot
                    .connections
                    .iter()
                    .min_by_key(|c| c.in_flight.load(Ordering::Acquire))
                    .filter(|c| !room || c.in_flight.load(Ordering::Acquire) == 0);
                if let Some(c) = least_busy {
                    return Ok(Arc::clone(c));
                }
                if room {
                    slot.opening = slot.opening.saturating_add(1);
                }
                room
            };
            if !open_one {
                // Every slot is opening: wait for one to finish.
                if Instant::now() >= deadline {
                    return Err(CallError::Busy);
                }
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let opened = self.open_counted(peer, deadline);
            let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
            let slot = peers.entry(peer.address).or_default();
            slot.opening = slot.opening.saturating_sub(1);
            match opened {
                Ok(Some(connection)) => {
                    slot.connections.push(Arc::clone(&connection));
                    return Ok(connection);
                }
                Ok(None) => {
                    drop(peers);
                    // The client's socket cap is reached.
                    if Instant::now() >= deadline {
                        return Err(CallError::Busy);
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Opens a socket within the client's socket cap: `None` at the cap.
    fn open_counted(
        &self,
        peer: &PeerId,
        deadline: Instant,
    ) -> Result<Option<Arc<Connection>>, CallError> {
        if self.sockets.fetch_add(1, Ordering::AcqRel) >= self.config.max_outbound_sockets {
            self.sockets.fetch_sub(1, Ordering::AcqRel);
            return Ok(None);
        }
        self.open(peer, deadline).map(Some).inspect_err(|_| {
            self.sockets.fetch_sub(1, Ordering::AcqRel);
        })
    }

    /// Opens a socket and makes both handshakes before its reader starts.
    fn open(&self, peer: &PeerId, deadline: Instant) -> Result<Arc<Connection>, CallError> {
        let left = remaining(deadline)?;
        let mut stream =
            TcpStream::connect_timeout(&peer.address, left.min(self.config.connect_timeout))
                .map_err(connect_error)?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        // The TCP handshake: our minimum compatible version, their version back.
        let request = TcpHandshakeRequest {
            parent: TaskId::EMPTY,
            version: Some(WireVersion::MIN_COMPAT),
        };
        let meta = FrameMeta {
            request_id: 0,
            status: Status::request(false, true),
            version: WireVersion::MIN_COMPAT,
        };
        let variable = VariableHeader {
            // ALLOC: one action name per socket opened.
            action: TCP_HANDSHAKE_ACTION.to_owned(),
            ..VariableHeader::default()
        };
        let answer = self.exchange(&mut stream, meta, &variable, |o| request.write(o))?;
        let theirs = TcpHandshakeResponse::read(&mut Input::new(&answer.message))
            .map_err(|_| CallError::Io(io::ErrorKind::InvalidData))?;
        let version = theirs
            .version
            .check_accepted()
            .map_err(|_| CallError::WrongPeer("a version this node does not speak"))?;
        // The transport handshake: who answers.
        let meta = FrameMeta {
            request_id: 0,
            status: Status::request(false, false),
            version,
        };
        let variable = VariableHeader {
            // ALLOC: one action name per socket opened.
            action: TRANSPORT_HANDSHAKE_ACTION.to_owned(),
            ..VariableHeader::default()
        };
        let answer = self.exchange(&mut stream, meta, &variable, |o| {
            TransportHandshakeRequest::default().write(o)
        })?;
        let described = TransportHandshakeResponse::read(&mut Input::new(&answer.message))
            .map_err(|_| CallError::Io(io::ErrorKind::InvalidData))?;
        let ephemeral_id = described
            .node
            .map(|n: DiscoveryNode| n.ephemeral_id)
            .unwrap_or_default();
        if peer
            .ephemeral_id
            .as_ref()
            .is_some_and(|expected| *expected != ephemeral_id)
        {
            return Err(CallError::WrongPeer(
                "another process answers at the lease's address",
            ));
        }
        stream.set_read_timeout(None).map_err(io_error)?;
        let reader_stream = stream.try_clone().map_err(io_error)?;
        let connection = Arc::new(Connection {
            writer: Mutex::new(stream),
            // ALLOC: one waiter per request in flight on this socket.
            waiters: Mutex::new(HashMap::new()),
            in_flight: AtomicUsize::new(0),
            dead: AtomicBool::new(false),
            version,
            ephemeral_id,
        });
        let routed = Arc::clone(&connection);
        let reader = self.reader.clone();
        let sockets = Arc::clone(&self.sockets);
        thread::Builder::new()
            .name("transport-client".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                route(reader_stream, &reader, &routed);
                sockets.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(io_error)?;
        Ok(connection)
    }

    /// Sends one handshake request and reads its answer, before the
    /// socket's reader runs.
    fn exchange<F>(
        &self,
        stream: &mut TcpStream,
        meta: FrameMeta,
        variable: &VariableHeader,
        write: F,
    ) -> Result<HeldFrame, CallError>
    where
        F: FnOnce(&mut Output<'_>) -> Result<(), WireError>,
    {
        // ALLOC: one small handshake frame.
        let mut buf = Vec::with_capacity(128);
        frame::encode(&mut buf, self.config.max_frame_bytes, meta, variable, write)
            .map_err(|_| CallError::Io(io::ErrorKind::InvalidData))?;
        stream.write_all(&buf).map_err(io_error)?;
        loop {
            match self.reader.read(stream, |_, _| Ok(())) {
                Ok(Some(Inbound::Frame(frame))) if frame.header.request_id == meta.request_id => {
                    if frame.header.status.is_error() {
                        let failure = RemoteException::read(&mut Input::new(&frame.message))
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        return Err(classify(failure));
                    }
                    return Ok(frame);
                }
                Ok(Some(Inbound::Ping | Inbound::Frame(_))) => {}
                Ok(Some(Inbound::Dropped { error, .. })) => return Err(dropped(&error)),
                Ok(None) => return Err(CallError::Io(io::ErrorKind::UnexpectedEof)),
                Err(ChannelError::Io(e)) => return Err(io_error(e)),
                Err(ChannelError::Wire(_)) => {
                    return Err(CallError::Io(io::ErrorKind::InvalidData));
                }
            }
        }
    }
}
