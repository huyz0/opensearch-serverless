//! Answering: the TCP handshake, a response or a failure wrapped as the
//! node's `RemoteTransportException`, and the workers that run handlers.

use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, PoisonError};

use std::sync::mpsc::Receiver;

use super::{Job, Request, Shared};
use crate::channel::HeldFrame;
use crate::error::WireError;
use crate::exception::RemoteException;
use crate::frame::{self, FrameMeta, Status, VariableHeader};
use crate::handshake::{TcpHandshakeRequest, TcpHandshakeResponse};
use crate::stream::{Input, Output};
use crate::version::WireVersion;

pub(super) fn answer_tcp_handshake(
    writer: &Arc<Mutex<TcpStream>>,
    shared: &Shared,
    frame: &HeldFrame,
) -> Result<(), WireError> {
    let mut input = Input::new(&frame.message);
    let request = TcpHandshakeRequest::read(&mut input)?;
    let peer_min = request.version.unwrap_or(frame.header.version);
    if peer_min > WireVersion::SHELL
        || !(peer_min == WireVersion::MIN_COMPAT || peer_min.accepted())
    {
        return Err(WireError::UnsupportedVersion { id: peer_min.id() });
    }
    let answer = TcpHandshakeResponse {
        version: WireVersion::SHELL,
    };
    let meta = FrameMeta {
        request_id: frame.header.request_id,
        status: Status::response(false, true, false),
        version: frame.header.version,
    };
    // ALLOC: one small handshake frame.
    let mut buf = Vec::with_capacity(64);
    frame::encode(
        &mut buf,
        shared.config.max_frame_bytes,
        meta,
        &VariableHeader::default(),
        |o| answer.write(o),
    )?;
    write_all(writer, &buf).map_err(|_| WireError::Truncated {
        what: "a handshake response",
    })
}

pub(super) type MessageWriter<'a> = &'a dyn Fn(&mut Output<'_>) -> Result<(), WireError>;

/// Writes a response frame for `request_id`: the message, or the failure
/// wrapped as the node's `RemoteTransportException`.
pub(super) fn respond(
    writer: &Arc<Mutex<TcpStream>>,
    shared: &Shared,
    request_id: i64,
    action: &str,
    version: WireVersion,
    compress: bool,
    answer: Result<MessageWriter<'_>, RemoteException>,
) -> io::Result<()> {
    let config = &shared.config;
    // ALLOC: one response frame, at most the frame bound.
    let mut buf = Vec::new();
    let error = answer.is_err();
    let meta = FrameMeta {
        request_id,
        status: Status::response(compress && !error, false, error),
        version: if version.accepted() {
            version
        } else {
            WireVersion::SHELL
        },
    };
    let variable = VariableHeader::default();
    let encoded = match answer {
        Ok(write) => frame::encode(&mut buf, config.max_frame_bytes, meta, &variable, write),
        Err(cause) => {
            let remote = RemoteException::remote(
                &config.node.name,
                config.node.address.clone(),
                action,
                cause,
            );
            frame::encode(&mut buf, config.max_frame_bytes, meta, &variable, |o| {
                remote.write(o)
            })
        }
    };
    if encoded.is_err() {
        let failure = RemoteException::status(
            "the response is larger than a transport frame",
            "INTERNAL_SERVER_ERROR",
        );
        let remote = RemoteException::remote(
            &config.node.name,
            config.node.address.clone(),
            action,
            failure,
        );
        buf.clear();
        let meta = FrameMeta {
            status: Status::response(false, false, true),
            ..meta
        };
        frame::encode(&mut buf, config.max_frame_bytes, meta, &variable, |o| {
            remote.write(o)
        })
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    }
    write_all(writer, &buf)
}

pub(super) fn write_all(writer: &Arc<Mutex<TcpStream>>, bytes: &[u8]) -> io::Result<()> {
    let mut stream = writer.lock().unwrap_or_else(PoisonError::into_inner);
    stream.write_all(bytes)?;
    stream.flush()
}

pub(super) fn work(shared: &Arc<Shared>, queue: &Arc<Mutex<Receiver<Job>>>) {
    loop {
        let job = {
            let queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            queue.recv()
        };
        let Ok(job) = job else { return };
        let frame = &job.frame;
        let action = frame.variable.action.as_str();
        let header = &frame.header;
        let request = Request {
            action,
            variable: &frame.variable,
            message: &frame.message,
            version: header.version,
        };
        let compress = header.status.is_compressed();
        let result = match shared.handlers.get(action) {
            None => Err(RemoteException::action_not_found(action)),
            Some(handler) => {
                // ALLOC: the response message, at most the frame bound.
                let mut message = Vec::new();
                handler
                    .handle(
                        &request,
                        &mut Output::new(&mut message, shared.config.max_frame_bytes),
                    )
                    .map(|()| message)
            }
        };
        let sent = match result {
            Ok(message) => respond(
                &job.writer,
                shared,
                header.request_id,
                action,
                header.version,
                compress,
                Ok(&|o| o.bytes(&message)),
            ),
            Err(failure) => respond(
                &job.writer,
                shared,
                header.request_id,
                action,
                header.version,
                compress,
                Err(failure),
            ),
        };
        if sent.is_err() {
            // The peer is gone or stuck; its reader notices and ends the channel.
            let stream = job.writer.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}
