//! The two handshakes a connection makes (`TransportHandshaker`,
//! `TransportService.HandshakeRequest` and `HandshakeResponse`).
//!
//! Each channel first exchanges versions (`internal:tcp/handshake`, frames
//! with the handshake bit); then the connection asks the peer to describe
//! itself (`internal:transport/handshake`) and keeps it only if the node
//! that answers is the one expected, compared by ephemeral id.

use crate::error::WireError;
use crate::node::DiscoveryNode;
use crate::stream::{Input, Output, TaskId};
use crate::version::WireVersion;

/// The TCP handshake's action.
pub const TCP_HANDSHAKE_ACTION: &str = "internal:tcp/handshake";

/// The transport handshake's action.
pub const TRANSPORT_HANDSHAKE_ACTION: &str = "internal:transport/handshake";

/// The TCP handshake request: the sender's minimum compatible version,
/// wrapped in a byte reference so a later version can append to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpHandshakeRequest {
    /// The parent task, empty in practice.
    pub parent: TaskId,
    /// The sender's minimum compatible version; `None` when the request
    /// ends before it, which the Java also accepts.
    pub version: Option<WireVersion>,
}

impl TcpHandshakeRequest {
    /// Writes the message.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        if let Some(version) = self.version {
            let mut inner = [0u8; 5];
            let len = {
                // ALLOC: none beyond five bytes; a version is a vint.
                let mut scratch = Vec::with_capacity(5);
                version.write(&mut Output::new(&mut scratch, 5))?;
                let len = scratch.len();
                if let Some(slot) = inner.get_mut(..len) {
                    slot.copy_from_slice(&scratch);
                }
                len
            };
            out.bytes_reference(inner.get(..len).unwrap_or_default())?;
        }
        Ok(())
    }

    /// Reads the message; bytes inside the version's byte reference after
    /// the version are ignored, as the Java ignores them.
    ///
    /// # Errors
    ///
    /// As the stream reads; [`WireError::Malformed`] when bytes follow the
    /// byte reference.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        let parent = TaskId::read(input)?;
        if input.remaining() == 0 {
            return Ok(Self {
                parent,
                version: None,
            });
        }
        let inner = input.bytes_reference("a tcp handshake request")?;
        let version = WireVersion::read(&mut Input::new(inner))?;
        input.finish("a tcp handshake request")?;
        Ok(Self {
            parent,
            version: Some(version),
        })
    }
}

/// The TCP handshake response: the answering node's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpHandshakeResponse {
    /// The answering node's version.
    pub version: WireVersion,
}

impl TcpHandshakeResponse {
    /// Writes the message.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.version.write(out)
    }

    /// Reads the message.
    ///
    /// # Errors
    ///
    /// As the stream reads; [`WireError::Malformed`] when bytes remain.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        let version = WireVersion::read(input)?;
        input.finish("a tcp handshake response")?;
        Ok(Self { version })
    }
}

/// The transport handshake request: only the parent task.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransportHandshakeRequest {
    /// The parent task, empty in practice.
    pub parent: TaskId,
}

impl TransportHandshakeRequest {
    /// Writes the message.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)
    }

    /// Reads the message.
    ///
    /// # Errors
    ///
    /// As the stream reads; [`WireError::Malformed`] when bytes remain.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        let parent = TaskId::read(input)?;
        input.finish("a transport handshake request")?;
        Ok(Self { parent })
    }
}

/// The transport handshake response: the node, written with its
/// attributes, the cluster name and the version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportHandshakeResponse {
    /// The answering node, when it describes itself (it always does).
    pub node: Option<DiscoveryNode>,
    /// The answering node's `cluster.name`.
    pub cluster_name: String,
    /// The answering node's version.
    pub version: WireVersion,
}

impl TransportHandshakeResponse {
    /// Writes the message.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        match &self.node {
            None => out.bool(false)?,
            Some(node) => {
                out.bool(true)?;
                node.write(out, true)?;
            }
        }
        out.string(&self.cluster_name)?;
        self.version.write(out)
    }

    /// Reads the message.
    ///
    /// # Errors
    ///
    /// As the stream reads; [`WireError::Malformed`] when bytes remain.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a transport handshake response";
        let node = if input.bool(WHAT)? {
            Some(DiscoveryNode::read(input)?)
        } else {
            None
        };
        let cluster_name = input.string(WHAT)?;
        let version = WireVersion::read(input)?;
        input.finish(WHAT)?;
        Ok(Self {
            node,
            cluster_name,
            version,
        })
    }
}
