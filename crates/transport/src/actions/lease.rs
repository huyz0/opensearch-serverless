//! A node that has joined tells its peers: `AnnounceRequest` and its answer.
//!
//! Rust nodes send this to each other only; the Java shell has no such
//! action, and a node that gets "no handler" for it loses nothing, because
//! the announce only hurries what the next read of the lease register would
//! show.

use crate::error::WireError;
use crate::stream::{Input, Output};

/// `internal:serverless/lease/announce`.
pub const ANNOUNCE_ACTION: &str = "internal:serverless/lease/announce";

/// A joined node naming itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnnounceRequest {
    /// The joiner's node id.
    pub node_id: String,
    /// The joiner's ephemeral id: a restart keeps the node id and changes it.
    pub ephemeral_id: String,
}

impl AnnounceRequest {
    /// Writes the message.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.string(&self.node_id)?;
        out.string(&self.ephemeral_id)
    }

    /// Reads the message.
    ///
    /// # Errors
    ///
    /// As the read it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        Ok(Self {
            node_id: input.string("a lease announce")?,
            ephemeral_id: input.string("a lease announce")?,
        })
    }
}

/// A peer's answer to an announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnnounceResponse {
    /// Whether the peer read the fleet's leases because of it.
    pub refreshed: bool,
}

impl AnnounceResponse {
    /// Writes the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.bool(self.refreshed)
    }

    /// Reads the answer.
    ///
    /// # Errors
    ///
    /// As the read it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        Ok(Self {
            refreshed: input.bool("a lease announce's answer")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_announce_round_trips() {
        let request = AnnounceRequest {
            node_id: "node-7".to_owned(),
            ephemeral_id: "e-1".to_owned(),
        };
        let mut bytes = Vec::new();
        request
            .write(&mut Output::new(&mut bytes, 64))
            .expect("write");
        assert_eq!(
            AnnounceRequest::read(&mut Input::new(&bytes)).expect("read"),
            request
        );
        for refreshed in [true, false] {
            let mut bytes = Vec::new();
            AnnounceResponse { refreshed }
                .write(&mut Output::new(&mut bytes, 8))
                .expect("write");
            assert_eq!(
                AnnounceResponse::read(&mut Input::new(&bytes)).expect("read"),
                AnnounceResponse { refreshed }
            );
        }
    }

    #[test]
    fn a_truncated_or_empty_announce_is_refused() {
        assert!(AnnounceRequest::read(&mut Input::new(&[])).is_err());
        assert!(AnnounceRequest::read(&mut Input::new(&[5, b'a'])).is_err());
        // The node id alone is not enough.
        assert!(AnnounceRequest::read(&mut Input::new(&[1, b'a'])).is_err());
        assert!(AnnounceResponse::read(&mut Input::new(&[])).is_err());
    }
}
