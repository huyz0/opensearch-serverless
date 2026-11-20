//! Forwarding a bulk to the node that owns its shard, and the one place a
//! refused connect to that node can revoke its lease early.
//!
//! The send side of the shell's forwarded bulk: the owner's lease comes from
//! the leases this node already holds, so a forward makes no request to the
//! store. A connect refused (nothing listens on the owner's port) is the
//! signal `serverless.lease.early_revoke` acts on; any other failure is not.

use std::sync::Arc;
use std::time::Duration;

use oss_cluster::{EarlyRevoke, LeaseClock, Verdict};
use oss_transport::actions::write::{BULK_ACTION, BulkRequest, BulkResponse};
use oss_transport::auth::Authenticator;
use oss_transport::client::{CallError, Client};
use oss_transport::stream::Input;

use crate::forward_call::{owner_of, signed_call};
use crate::push_link::Leases;

/// How long a forwarded bulk waits for its owner.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest forwarded request message. The bulk's own size limits keep a
/// request far below it; it bounds the one buffer a forward builds.
pub const MAX_FORWARD_BYTES: usize = 32 << 20;

/// Why a bulk did not come back from its owner.
#[derive(Debug)]
pub enum ForwardError {
    /// No unrevoked lease names the node, its address does not parse, or the
    /// request could not be signed: nothing was sent.
    NoPeer,
    /// The request is larger than [`MAX_FORWARD_BYTES`] or cannot be
    /// encoded: it can never succeed, so it is not retried.
    TooLarge,
    /// The owner's answer did not decode.
    BadAnswer,
    /// The owner refused the connection and its lease is now revoked or
    /// gone: look the owner up again; the normal takeover applies.
    Gone,
    /// The call failed.
    Failed(CallError),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPeer => f.write_str("no live lease names the owner"),
            Self::TooLarge => f.write_str("the forwarded bulk is too large to send"),
            Self::BadAnswer => f.write_str("the owner's answer did not decode"),
            Self::Gone => f.write_str("the owner refused the connection and its lease is gone"),
            Self::Failed(e) => write!(f, "the forward failed: {e:?}"),
        }
    }
}

impl std::error::Error for ForwardError {}

/// Sends bulks to shard owners over the transport client, signed.
pub struct BulkForwarder<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
    early: Arc<EarlyRevoke>,
    clock: Arc<dyn LeaseClock>,
    timeout: Duration,
}

impl<L: Leases> BulkForwarder<L> {
    /// A forwarder calling through `client`, signing with `auth`, finding
    /// owners in `leases`, and asking `early` about a refused connect
    /// (`clock` stamps the refusal).
    pub fn new(
        client: Arc<Client>,
        auth: Arc<Authenticator>,
        leases: L,
        early: Arc<EarlyRevoke>,
        clock: Arc<dyn LeaseClock>,
    ) -> Self {
        Self {
            client,
            auth,
            leases,
            early,
            clock,
            timeout: FORWARD_TIMEOUT,
        }
    }

    /// The same, waiting `timeout` for an owner.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sends `request` to node `node_id` and reads its answer.
    ///
    /// # Errors
    ///
    /// [`ForwardError`]: nothing sent, the owner gone, or the call failed.
    pub fn bulk(&self, node_id: &str, request: &BulkRequest) -> Result<BulkResponse, ForwardError> {
        let (lease, address) = owner_of(&self.leases, node_id)?;
        match signed_call(
            &self.client,
            &self.auth,
            BULK_ACTION,
            (&lease, address),
            self.timeout,
            |out| request.write(out),
        ) {
            Ok(answer) => BulkResponse::read(&mut Input::new(answer.message()))
                .map_err(|_| ForwardError::BadAnswer),
            Err(ForwardError::Failed(CallError::Refused)) => {
                let refused_at = self.clock.now_millis();
                match self.early.on_refused(
                    &lease.node_id,
                    &lease.ephemeral_id,
                    address,
                    refused_at,
                ) {
                    Verdict::Revoked | Verdict::Gone => Err(ForwardError::Gone),
                    Verdict::Off | Verdict::Declined(_) => {
                        Err(ForwardError::Failed(CallError::Refused))
                    }
                }
            }
            Err(e) => Err(e),
        }
    }
}
