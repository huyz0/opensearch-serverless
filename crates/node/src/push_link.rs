//! How the writer's push reaches a reader: the readers placement ranks for
//! a shard, and the transport call that carries the push.
//!
//! Ported from `CommitPush.readerTargets` and the send of `CommitPush.published`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/CommitPush.java`).
//! Both read the leases the node already holds, so they make no request to
//! the store. The push is signed as every forwarded request is.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_cluster::placement::{ROLE_SEARCH, ShardKey, readers_for};
use oss_format::NodeLease;
use oss_transport::actions::push::{PUSH_ACTION, PushRequest, PushResponse};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::stream::{Input, Output};

use crate::push_sender::{LinkError, PushLink, PushTargets};

/// How long a push waits for a reader to take it.
pub const SEND_TIMEOUT: Duration = Duration::from_millis(500);

/// The largest push message: a commit register's value, with its blocks.
pub const MAX_PUSH_BYTES: usize = 8 << 20;

/// The nodes this node knows of, as the lease register last showed them.
pub trait Leases: Send + Sync {
    /// The current leases.
    fn current(&self) -> Arc<[NodeLease]>;
}

impl Leases for oss_cluster::lease::Membership {
    fn current(&self) -> Arc<[NodeLease]> {
        oss_cluster::lease::Membership::current(self)
    }
}

/// The readers placement ranks first for a shard, other than this node.
pub struct PlacementTargets<L> {
    leases: L,
    self_id: String,
    readers: usize,
}

impl<L: Leases> PlacementTargets<L> {
    /// Targets over `leases`: at least two readers, `readers` for a hot
    /// shard, never `self_id`.
    pub fn new(leases: L, self_id: &str, readers: usize) -> Self {
        Self {
            leases,
            // ALLOC: one id per node.
            self_id: self_id.to_owned(),
            readers,
        }
    }
}

impl<L: Leases> PushTargets for PlacementTargets<L> {
    fn readers(&self, index: &str, shard: u32) -> Vec<String> {
        let leases = self.leases.current();
        let key = ShardKey::new(index, i32::try_from(shard).unwrap_or(i32::MAX));
        // ALLOC: at most the ranked readers, a hot shard's few.
        readers_for(key, leases.iter(), ROLE_SEARCH, self.readers.max(2))
            .into_iter()
            .filter(|id| *id != self.self_id)
            .map(str::to_owned)
            .collect()
    }

    fn other_build_readers(&self, index: &str, shard: u32) -> Vec<String> {
        let leases = self.leases.current();
        let key = ShardKey::new(index, i32::try_from(shard).unwrap_or(i32::MAX));
        // ALLOC: at most the ranked readers, a hot shard's few.
        readers_for(key, leases.iter(), ROLE_SEARCH, self.readers.max(2))
            .into_iter()
            .filter(|id| *id != self.self_id)
            .filter(|id| {
                leases
                    .iter()
                    .find(|l| l.node_id == *id)
                    .is_some_and(|l| !crate::read_forward::same_build(l))
            })
            .map(str::to_owned)
            .collect()
    }
}

/// Sends pushes over the transport client, signed.
pub struct TransportPushLink<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
}

impl<L: Leases> TransportPushLink<L> {
    /// A link calling through `client`, signing with `auth`, finding peers
    /// in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L) -> Self {
        Self {
            client,
            auth,
            leases,
        }
    }
}

impl<L: Leases> PushLink for TransportPushLink<L> {
    fn send(&self, target: &str, request: &PushRequest) -> Result<bool, LinkError> {
        let leases = self.leases.current();
        let lease = leases
            .iter()
            .find(|l| l.node_id == target && !l.revoked)
            .ok_or(LinkError)?;
        let address: SocketAddr = lease.address.parse().map_err(|_| LinkError)?;
        // ALLOC: one push message, at most MAX_PUSH_BYTES.
        let mut message = Vec::new();
        request
            .write(&mut Output::new(&mut message, MAX_PUSH_BYTES))
            .map_err(|_| LinkError)?;
        let mac = self
            .auth
            .sign(
                PUSH_ACTION,
                &message,
                &identity(&lease.node_id, &lease.ephemeral_id),
                &[],
            )
            .ok_or(LinkError)?;
        // ALLOC: the MAC header, one entry.
        let headers = [(MAC_HEADER.to_owned(), mac)];
        let peer = PeerId {
            address,
            // ALLOC: one ephemeral id a call.
            ephemeral_id: Some(lease.ephemeral_id.clone()),
        };
        let call = Call {
            action: PUSH_ACTION,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(SEND_TIMEOUT).ok_or(LinkError)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .map_err(|_| LinkError)?;
        PushResponse::read(&mut Input::new(answer.message()))
            .map(|response| response.open)
            .map_err(|_| LinkError)
    }
}
