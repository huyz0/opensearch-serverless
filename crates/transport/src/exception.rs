//! Exceptions on the wire (`StreamOutput.writeException`,
//! `StreamInput.readException`): what a failed request answers.
//!
//! A node answers a failed request with a `RemoteTransportException` naming
//! itself, its address and the action, wrapping the failure. Rust writes the
//! failures its handlers raise (a security refusal, a rejection, an action
//! not found, a failure with a status) without stack traces, and reads any
//! chain: the JDK kinds the stream encodes by number, and OpenSearch
//! exceptions by registry id. An OpenSearch exception whose own fields this
//! reader does not know ends the read after its common fields, since the
//! bytes after it cannot be found; what was read is kept.

use crate::error::WireError;
use crate::node::TransportAddress;
use crate::stream::{Input, Output};

/// Registry ids of the OpenSearch exceptions read or written in full
/// (`OpenSearchServerException`, `OpenSearchException`).
pub mod ids {
    /// `OpenSearchSecurityException`.
    pub const SECURITY: i32 = 4;
    /// `TransportException`.
    pub const TRANSPORT: i32 = 34;
    /// `NotSerializableExceptionWrapper`.
    pub const NOT_SERIALIZABLE: i32 = 62;
    /// `ActionNotFoundTransportException`.
    pub const ACTION_NOT_FOUND: i32 = 101;
    /// `RemoteTransportException`.
    pub const REMOTE_TRANSPORT: i32 = 103;
    /// `OpenSearchStatusException`.
    pub const STATUS: i32 = 145;
}

/// The stream's key for `OpenSearchRejectedExecutionException`.
const REJECTED_KEY: i32 = 18;

/// The deepest chain of causes and suppressed exceptions read.
pub const MAX_DEPTH: usize = 16;

/// What kind of exception a [`RemoteException`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExceptionKind {
    /// An OpenSearch exception, by registry id.
    OpenSearch(i32),
    /// A JDK or Lucene exception, by the stream's key (1 to 18:
    /// `IllegalArgumentException` is 6, `IllegalStateException` 14,
    /// `OpenSearchRejectedExecutionException` 18).
    Jdk(i32),
}

/// An exception read off the wire, or one to write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteException {
    /// What it is.
    pub kind: Option<ExceptionKind>,
    /// Its message.
    pub message: Option<String>,
    /// Its cause.
    pub cause: Option<Box<RemoteException>>,
    /// The REST status name it carries (`FORBIDDEN`), for the exceptions
    /// that write one.
    pub status: Option<String>,
    /// The action it names (a transport exception's or an action not
    /// found).
    pub action: Option<String>,
    /// The address a transport exception names.
    pub address: Option<Box<TransportAddress>>,
    /// Whether every byte of it was understood; false when an exception
    /// whose own fields this reader does not know stopped the read.
    pub complete: bool,
}

impl RemoteException {
    /// The failure a `RemoteTransportException` wraps, or this one.
    pub fn root(&self) -> &Self {
        match (&self.kind, &self.cause) {
            (Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT)), Some(cause)) => cause,
            _ => self,
        }
    }

    /// The HTTP status a failure maps to, as OpenSearch's `status()` does
    /// for the kinds read in full: the carried status, 429 for a
    /// rejection, 400 for an illegal argument, else 500.
    pub fn http_status(&self) -> u16 {
        let root = self.root();
        if let Some(name) = &root.status {
            return status_code(name).unwrap_or(500);
        }
        match root.kind {
            Some(ExceptionKind::Jdk(REJECTED_KEY)) => 429,
            Some(ExceptionKind::Jdk(6)) => 400,
            _ => 500,
        }
    }

    /// `OpenSearchSecurityException(message, status)`.
    pub fn security(message: &str, status: &str) -> Self {
        Self::with_status(ids::SECURITY, message, status)
    }

    /// `OpenSearchStatusException(message, status)`.
    pub fn status(message: &str, status: &str) -> Self {
        Self::with_status(ids::STATUS, message, status)
    }

    fn with_status(id: i32, message: &str, status: &str) -> Self {
        Self {
            kind: Some(ExceptionKind::OpenSearch(id)),
            // ALLOC: one message and status name per failed request.
            message: Some(message.to_owned()),
            // ALLOC: as above.
            status: Some(status.to_owned()),
            complete: true,
            ..Self::default()
        }
    }

    /// `OpenSearchRejectedExecutionException(message)`, not for a shut-down
    /// executor.
    pub fn rejected(message: &str) -> Self {
        Self {
            kind: Some(ExceptionKind::Jdk(REJECTED_KEY)),
            // ALLOC: one message per rejected request.
            message: Some(message.to_owned()),
            complete: true,
            ..Self::default()
        }
    }

    /// `ActionNotFoundTransportException(action)`.
    pub fn action_not_found(action: &str) -> Self {
        Self {
            kind: Some(ExceptionKind::OpenSearch(ids::ACTION_NOT_FOUND)),
            // ALLOC: one message and action per refused request.
            message: Some(format!("No handler for action [{action}]")),
            // ALLOC: as above.
            action: Some(action.to_owned()),
            complete: true,
            ..Self::default()
        }
    }

    /// The `RemoteTransportException` a node answers with: `[name][address]
    /// [action]` wrapping `cause`.
    pub fn remote(name: &str, address: TransportAddress, action: &str, cause: Self) -> Self {
        let port = address.port;
        let host = match address.ip {
            // ALLOC: one address text per failed request.
            std::net::IpAddr::V4(v4) => v4.to_string(),
            // ALLOC: one address text per failed request.
            std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        };
        Self {
            kind: Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT)),
            // ALLOC: one message per failed request.
            message: Some(format!("[{name}][{host}:{port}][{action}]")),
            cause: Some(Box::new(cause)),
            // ALLOC: one action per failed request.
            action: Some(action.to_owned()),
            address: Some(Box::new(address)),
            complete: true,
            ..Self::default()
        }
    }

    /// Writes this exception (`writeException`), without stack traces,
    /// suppressed exceptions, headers or metadata.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit;
    /// [`WireError::Malformed`] for a kind this writer does not write.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        const WHAT: &str = "an exception";
        out.bool(true)?;
        match self.kind {
            Some(ExceptionKind::Jdk(REJECTED_KEY)) => {
                out.vint(REJECTED_KEY)?;
                out.bool(false)?;
                out.optional_string(self.message.as_deref())?;
                // No cause; then no stack frames and no suppressed.
                out.count(0)?;
                return out.count(0);
            }
            Some(ExceptionKind::OpenSearch(id)) => {
                out.vint(0)?;
                out.vint(id)?;
            }
            _ => {
                return Err(WireError::malformed(
                    WHAT,
                    "only OpenSearch exceptions and rejections are written",
                ));
            }
        }
        out.optional_string(self.message.as_deref())?;
        match &self.cause {
            None => out.bool(false)?,
            Some(cause) => cause.write(out)?,
        }
        // Stack frames, suppressed, headers, metadata: none.
        for _ in 0..4 {
            out.count(0)?;
        }
        match self.kind {
            Some(ExceptionKind::OpenSearch(ids::SECURITY | ids::STATUS)) => {
                out.string(self.status.as_deref().unwrap_or("INTERNAL_SERVER_ERROR"))
            }
            Some(ExceptionKind::OpenSearch(ids::ACTION_NOT_FOUND)) => {
                out.optional_string(self.action.as_deref())
            }
            Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT)) => {
                match &self.address {
                    None => out.bool(false)?,
                    Some(address) => {
                        out.bool(true)?;
                        address.write(out)?;
                    }
                }
                out.optional_string(self.action.as_deref())
            }
            Some(ExceptionKind::OpenSearch(ids::TRANSPORT)) => Ok(()),
            _ => Err(WireError::malformed(
                WHAT,
                "an OpenSearch exception this writer does not write",
            )),
        }
    }

    /// Reads an exception, or `None` for none (`readException`).
    ///
    /// # Errors
    ///
    /// As the stream reads; [`WireError::Malformed`] for an unknown key or
    /// a chain deeper than [`MAX_DEPTH`].
    pub fn read(input: &mut Input<'_>) -> Result<Option<Self>, WireError> {
        let mut stopped = false;
        read_at(input, 0, &mut stopped)
    }
}

/// The code of a REST status name, for the statuses a node's failures
/// carry.
fn status_code(name: &str) -> Option<u16> {
    Some(match name {
        "BAD_REQUEST" => 400,
        "UNAUTHORIZED" => 401,
        "FORBIDDEN" => 403,
        "NOT_FOUND" => 404,
        "CONFLICT" => 409,
        "GONE" => 410,
        "REQUEST_ENTITY_TOO_LARGE" => 413,
        "TOO_MANY_REQUESTS" => 429,
        "SERVICE_UNAVAILABLE" => 503,
        "GATEWAY_TIMEOUT" => 504,
        _ => return None,
    })
}

const WHAT: &str = "an exception";

fn read_at(
    input: &mut Input<'_>,
    depth: usize,
    stopped: &mut bool,
) -> Result<Option<RemoteException>, WireError> {
    if *stopped || !input.bool(WHAT)? {
        return Ok(None);
    }
    if depth >= MAX_DEPTH {
        return Err(WireError::malformed(WHAT, "causes nest too deep"));
    }
    let key = input.vint(WHAT)?;
    let mut out = RemoteException {
        complete: true,
        ..RemoteException::default()
    };
    if key == 0 {
        let id = input.vint(WHAT)?;
        out.kind = Some(ExceptionKind::OpenSearch(id));
        out.message = input.optional_string(WHAT)?;
        out.cause = read_at(input, depth.saturating_add(1), stopped)?.map(Box::new);
        if *stopped {
            out.complete = false;
            return Ok(Some(out));
        }
        skip_traces(input, depth, stopped)?;
        skip_map_of_lists(input)?;
        skip_map_of_lists(input)?;
        match id {
            ids::SECURITY | ids::STATUS => out.status = Some(input.string(WHAT)?),
            ids::NOT_SERIALIZABLE => {
                input.string(WHAT)?;
                out.status = Some(input.string(WHAT)?);
            }
            ids::ACTION_NOT_FOUND => out.action = input.optional_string(WHAT)?,
            ids::REMOTE_TRANSPORT => {
                if input.bool(WHAT)? {
                    // ALLOC: one address per failure read.
                    out.address = Some(Box::new(TransportAddress::read(input)?));
                }
                out.action = input.optional_string(WHAT)?;
            }
            ids::TRANSPORT => {}
            _ => {
                // Its own fields follow, unknown here: nothing after them can be found.
                *stopped = true;
                out.complete = false;
            }
        }
        return Ok(Some(out));
    }
    out.kind = Some(ExceptionKind::Jdk(key));
    // (message read, cause read) per key, as `readException` reads them.
    let (message, cause) = match key {
        1 => {
            out.message = input.optional_string(WHAT)?;
            input.optional_string(WHAT)?;
            (false, true)
        }
        2 => {
            input.optional_string(WHAT)?;
            for _ in 0..3 {
                input.int(WHAT)?;
            }
            (false, false)
        }
        3 => {
            input.optional_string(WHAT)?;
            if input.bool(WHAT)? {
                for _ in 0..3 {
                    input.int(WHAT)?;
                }
            } else {
                input.optional_string(WHAT)?;
            }
            (false, false)
        }
        6 | 7 | 9 | 14 | 15 | 17 => (true, true),
        4 | 5 | 8 | 10 | 11 | 12 | 16 => (true, false),
        13 => {
            input.vint(WHAT)?;
            for _ in 0..3 {
                input.optional_string(WHAT)?;
            }
            (true, false)
        }
        REJECTED_KEY => {
            input.bool(WHAT)?;
            (true, false)
        }
        _ => return Err(WireError::malformed(WHAT, "no such exception key")),
    };
    if message {
        out.message = input.optional_string(WHAT)?;
    }
    if cause {
        out.cause = read_at(input, depth.saturating_add(1), stopped)?.map(Box::new);
        if *stopped {
            out.complete = false;
            return Ok(Some(out));
        }
    }
    skip_traces(input, depth, stopped)?;
    if *stopped {
        out.complete = false;
    }
    Ok(Some(out))
}

/// Skips stack frames and suppressed exceptions (`readStackTrace`).
fn skip_traces(input: &mut Input<'_>, depth: usize, stopped: &mut bool) -> Result<(), WireError> {
    // A frame is at least two empty strings, a boolean and a vint.
    let frames = input.count(4, WHAT)?;
    for _ in 0..frames {
        input.string(WHAT)?;
        input.optional_string(WHAT)?;
        input.string(WHAT)?;
        input.vint(WHAT)?;
    }
    let suppressed = input.count(1, WHAT)?;
    for _ in 0..suppressed {
        read_at(input, depth.saturating_add(1), stopped)?;
        if *stopped {
            break;
        }
    }
    Ok(())
}

/// Skips a map of string lists (`readMapOfLists`).
fn skip_map_of_lists(input: &mut Input<'_>) -> Result<(), WireError> {
    let entries = input.count(2, WHAT)?;
    for _ in 0..entries {
        input.string(WHAT)?;
        input.string_array(WHAT)?;
    }
    Ok(())
}
