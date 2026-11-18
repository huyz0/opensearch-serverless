//! The reader's side of the tail's transport: taking a writer's push and
//! asking a writer for what it acknowledged past a commit.
//!
//! The receive side of the shell's `TailPush` and the asking side of its
//! fetch
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`,
//! `TailOverlays.java`). A writer the node's leases do not show live is
//! asked nothing, so finding that out costs no request; one that does not
//! answer in [`FETCH_TIMEOUT`] gives no answer and the shard is served from
//! its commit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_transport::actions::tail::{FETCH_ACTION, FetchRequest, FetchResponse, PushRequest};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::newest_ask::LiveWriters;
use crate::tail_held::TailHeld;

/// How long a reader waits for a writer's fetch answer.
pub const FETCH_TIMEOUT: Duration = Duration::from_millis(500);

/// The most a fetch request takes: names, a uuid, numbers.
const MAX_FETCH_BYTES: usize = 4096;

/// The milliseconds the held writes count their keep in.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Answers `internal:serverless/tail/push` on a reader.
pub struct TailPushHandler {
    held: Arc<TailHeld>,
    clock: Clock,
}

impl TailPushHandler {
    /// A handler giving what it takes to `held`, counting time by `clock`.
    pub fn new(held: Arc<TailHeld>, clock: Clock) -> Self {
        Self { held, clock }
    }
}

impl Handler for TailPushHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let push = PushRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed tail push", "BAD_REQUEST"))?;
        self.held
            .handle(&push, (self.clock)())
            .write(out)
            .map_err(|_| RemoteException::status("tail push answer", "INTERNAL_SERVER_ERROR"))
    }
}

/// Asks writers for tail records over the transport client, signed.
pub struct TransportTailFetch<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    writers: L,
}

impl<L: LiveWriters> TransportTailFetch<L> {
    /// A fetch calling through `client`, signing with `auth`, finding
    /// writers in `writers`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, writers: L) -> Self {
        Self {
            client,
            auth,
            writers,
        }
    }

    /// The writer `writer`'s answer to `request`, or nothing when it is not
    /// shown live, does not answer in time, fails, or answers bytes that do
    /// not decode.
    pub fn fetch(&self, writer: &str, request: &FetchRequest) -> Option<FetchResponse> {
        let live = self.writers.live(writer)?;
        // ALLOC: one request message, at most MAX_FETCH_BYTES.
        let mut message = Vec::new();
        request
            .write(&mut Output::new(&mut message, MAX_FETCH_BYTES))
            .ok()?;
        let mac = self.auth.sign(
            FETCH_ACTION,
            &message,
            &identity(writer, &live.ephemeral_id),
            &[],
        )?;
        // ALLOC: the MAC header, one entry.
        let headers = [(MAC_HEADER.to_owned(), mac)];
        let peer = PeerId {
            address: live.address,
            ephemeral_id: Some(live.ephemeral_id),
        };
        let call = Call {
            action: FETCH_ACTION,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(FETCH_TIMEOUT)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .ok()?;
        FetchResponse::read(&mut Input::new(answer.message())).ok()
    }
}
