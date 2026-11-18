//! How a reader asks a staging writer for its newest commit
//! (`internal:serverless/commit/newest`).
//!
//! The asking side of the shell's `NewestCommits.newerThan`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/NewestCommits.java`).
//! A writer the node's leases do not show live is asked nothing, so finding
//! that out costs no request; a writer that does not answer in
//! [`ASK_TIMEOUT`] leaves the register's commit as the answer.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_reader::commit_push::NewestSource;
use oss_transport::actions::push::{NEWEST_ACTION, NewestRequest, NewestResponse};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::stream::{Input, Output, TaskId};
use oss_types::settings::{BoolSetting, Scope, Setting};

/// `serverless.reader.newest_commit_ask`: whether a cold open asks a
/// staging writer for its newest commit. Default `false` until a cold-open
/// latency is measured with it on.
pub const NEWEST_COMMIT_ASK: BoolSetting = BoolSetting::new(
    "serverless.reader.newest_commit_ask",
    false,
    Scope::Node,
    "On a cold open, ask the writer a register's commit names for the newer \
     commit it staged since the register was written.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[NEWEST_COMMIT_ASK.setting()];

/// Installs `source` on `service` when the setting is on (`enabled`); off,
/// a cold open is as it was. Gives whether the source was installed.
pub fn install<S: oss_blob::BlobStore + 'static>(
    service: &crate::search::SearchService<S>,
    enabled: bool,
    source: Arc<dyn NewestSource>,
) -> bool {
    enabled && service.set_newest_source(source)
}

/// How long a reader waits for a writer's answer before serving the
/// register.
pub const ASK_TIMEOUT: Duration = Duration::from_millis(500);

/// The most a request message takes: a name, a uuid and a shard.
const MAX_ASK_BYTES: usize = 4096;

/// A writer a node's leases show live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveWriter {
    /// Where it listens.
    pub address: SocketAddr,
    /// Which process it is.
    pub ephemeral_id: String,
}

/// The nodes the leases this node holds show live.
pub trait LiveWriters: Send + Sync {
    /// The node, if its lease is unexpired and not revoked.
    fn live(&self, node_id: &str) -> Option<LiveWriter>;
}

/// Asks writers over the transport client, signed.
pub struct TransportNewestAsk<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    writers: L,
}

impl<L: LiveWriters> TransportNewestAsk<L> {
    /// A source calling through `client`, signing with `auth`, finding
    /// writers in `writers`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, writers: L) -> Self {
        Self {
            client,
            auth,
            writers,
        }
    }
}

impl<L: LiveWriters> NewestSource for TransportNewestAsk<L> {
    fn newest(&self, index: &str, index_uuid: &str, shard: u32, writer: &str) -> Option<Vec<u8>> {
        let live = self.writers.live(writer)?;
        let request = NewestRequest {
            parent: TaskId::EMPTY,
            // ALLOC: one name and one uuid an ask.
            index_name: index.to_owned(),
            index_uuid: index_uuid.to_owned(),
            shard: i32::try_from(shard).ok()?,
        };
        // ALLOC: one request message, at most MAX_ASK_BYTES.
        let mut message = Vec::new();
        request
            .write(&mut Output::new(&mut message, MAX_ASK_BYTES))
            .ok()?;
        let mac = self.auth.sign(
            NEWEST_ACTION,
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
            action: NEWEST_ACTION,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(ASK_TIMEOUT)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .ok()?;
        NewestResponse::read(&mut Input::new(answer.message()))
            .ok()?
            .commit
    }
}
