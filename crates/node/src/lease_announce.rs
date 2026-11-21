//! A joined node tells its peers (`internal:serverless/lease/announce`), and
//! a peer that does not hold it reads the fleet's leases once.
//!
//! Without it a join goes unseen for up to the lease read period. The
//! announce is advisory: a peer that misses it reads at its next period, and
//! nothing that decides ownership consults the view it fills.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use oss_cluster::lease::Membership;
use oss_transport::actions::lease::{ANNOUNCE_ACTION, AnnounceRequest, AnnounceResponse};
use oss_transport::auth::{Authenticator, MAC_HEADER, identity};
use oss_transport::client::{Call, Client, PeerId};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::push_link::Leases;

/// The most peers a joiner announces to; the rest read it at their period.
pub const MAX_ANNOUNCE: usize = 256;

/// How long one announce waits for its answer.
pub const SEND_TIMEOUT: Duration = Duration::from_millis(500);

/// The largest announce message.
const MAX_ANNOUNCE_BYTES: usize = 1024;

/// The fewest a node waits between two reads an announce asked for: a fleet
/// that starts together costs each node one read a gap.
pub const ANNOUNCE_GAP: Duration = Duration::from_secs(1);

/// What the handler needs of the node's view of the fleet.
pub trait Fleet: Send + Sync {
    /// Whether the view holds this incarnation of the node, live.
    fn holds(&self, node_id: &str, ephemeral_id: &str) -> bool;

    /// Reads the fleet's leases again; whether it could.
    fn read(&self) -> bool;
}

impl Fleet for Membership {
    fn holds(&self, node_id: &str, ephemeral_id: &str) -> bool {
        self.current()
            .iter()
            .any(|l| l.node_id == node_id && l.ephemeral_id == ephemeral_id && !l.revoked)
    }

    fn read(&self) -> bool {
        self.refresh().is_ok()
    }
}

/// When the node last read the fleet because of an announce, and whether a
/// read is already waiting for the gap to pass.
struct Reads {
    last: Option<Instant>,
    waiting: bool,
}

/// Answers the announce on every node.
pub struct AnnounceHandler {
    fleet: Arc<dyn Fleet>,
    gap: Duration,
    reads: Arc<Mutex<Reads>>,
}

enum Due {
    /// Read now.
    Now,
    /// A read is queued for when the gap ends: it covers this announce.
    Later,
    /// A read is queued already.
    Queued,
}

impl AnnounceHandler {
    /// A handler reading through `fleet`, at most once a `gap`: an announce
    /// inside the gap queues one read for when it ends, however many come.
    pub fn new(fleet: Arc<dyn Fleet>, gap: Duration) -> Self {
        Self {
            fleet,
            gap,
            reads: Arc::new(Mutex::new(Reads {
                last: None,
                waiting: false,
            })),
        }
    }

    /// The action it answers.
    pub const ACTION: &'static str = ANNOUNCE_ACTION;

    fn due(&self) -> Due {
        let mut reads = self.reads.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let wait = reads
            .last
            .map(|at| self.gap.saturating_sub(now.duration_since(at)))
            .filter(|wait| !wait.is_zero());
        let Some(wait) = wait else {
            reads.last = Some(now);
            return Due::Now;
        };
        if reads.waiting {
            return Due::Queued;
        }
        reads.waiting = true;
        let fleet = Arc::clone(&self.fleet);
        let shared = Arc::clone(&self.reads);
        let spawned = std::thread::Builder::new()
            .name("lease-announce-read".to_owned())
            .spawn(move || {
                std::thread::sleep(wait);
                fleet.read();
                let mut reads = shared.lock().unwrap_or_else(PoisonError::into_inner);
                reads.last = Some(Instant::now());
                reads.waiting = false;
            });
        if spawned.is_err() {
            reads.waiting = false;
        }
        Due::Later
    }
}

impl Handler for AnnounceHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let announce = AnnounceRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed lease announce", "BAD_REQUEST"))?;
        let refreshed = !self.fleet.holds(&announce.node_id, &announce.ephemeral_id)
            && match self.due() {
                Due::Now => self.fleet.read(),
                Due::Later | Due::Queued => false,
            };
        AnnounceResponse { refreshed }
            .write(out)
            .map_err(|_| RemoteException::status("lease announce answer", "INTERNAL_SERVER_ERROR"))
    }
}

/// Sends the announce, signed as every transport call is.
pub struct Announcer<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
    node_id: String,
    ephemeral_id: String,
}

impl<L: Leases> Announcer<L> {
    /// An announcer for the node `node_id` incarnation `ephemeral_id`,
    /// finding peers in `leases`.
    pub fn new(
        client: Arc<Client>,
        auth: Arc<Authenticator>,
        leases: L,
        node_id: &str,
        ephemeral_id: &str,
    ) -> Self {
        Self {
            client,
            auth,
            leases,
            // ALLOC: this node's two ids, once.
            node_id: node_id.to_owned(),
            ephemeral_id: ephemeral_id.to_owned(),
        }
    }

    /// Tells up to [`MAX_ANNOUNCE`] live peers, one after another; gives how
    /// many answered that they read the fleet because of it.
    pub fn announce(&self) -> usize {
        let leases = self.leases.current();
        leases
            .iter()
            .filter(|l| l.node_id != self.node_id && !l.revoked)
            .take(MAX_ANNOUNCE)
            .filter(|l| self.tell(l).unwrap_or(false))
            .count()
    }

    fn tell(&self, lease: &oss_format::NodeLease) -> Option<bool> {
        let address: SocketAddr = lease.address.parse().ok()?;
        // ALLOC: one announce message, at most MAX_ANNOUNCE_BYTES.
        let mut message = Vec::new();
        AnnounceRequest {
            // ALLOC: this node's two ids, a message.
            node_id: self.node_id.clone(),
            ephemeral_id: self.ephemeral_id.clone(),
        }
        .write(&mut Output::new(&mut message, MAX_ANNOUNCE_BYTES))
        .ok()?;
        let mac = self.auth.sign(
            ANNOUNCE_ACTION,
            &message,
            &identity(&lease.node_id, &lease.ephemeral_id),
            &[],
        )?;
        // ALLOC: the MAC header, one entry.
        let headers = [(MAC_HEADER.to_owned(), mac)];
        let peer = PeerId {
            address,
            // ALLOC: one ephemeral id a call.
            ephemeral_id: Some(lease.ephemeral_id.clone()),
        };
        let call = Call {
            action: ANNOUNCE_ACTION,
            headers: &headers,
            compress: false,
            deadline: Instant::now().checked_add(SEND_TIMEOUT)?,
        };
        let answer = self
            .client
            .call(&peer, call, |out| out.bytes(&message))
            .ok()?;
        AnnounceResponse::read(&mut Input::new(answer.message()))
            .ok()
            .map(|r| r.refreshed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_transport::frame::VariableHeader;
    use oss_transport::version::WireVersion;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        known: Vec<(String, String)>,
        reads: AtomicUsize,
        works: bool,
    }

    impl Fleet for Fake {
        fn holds(&self, node_id: &str, ephemeral_id: &str) -> bool {
            self.known
                .iter()
                .any(|(n, e)| n == node_id && e == ephemeral_id)
        }
        fn read(&self) -> bool {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.works
        }
    }

    fn fake(known: &[(&str, &str)], works: bool) -> Arc<Fake> {
        Arc::new(Fake {
            known: known
                .iter()
                .map(|(n, e)| ((*n).to_owned(), (*e).to_owned()))
                .collect(),
            reads: AtomicUsize::new(0),
            works,
        })
    }

    fn ask(handler: &AnnounceHandler, node: &str, ephemeral: &str) -> bool {
        let mut message = Vec::new();
        AnnounceRequest {
            node_id: node.to_owned(),
            ephemeral_id: ephemeral.to_owned(),
        }
        .write(&mut Output::new(&mut message, 64))
        .expect("write");
        answer(handler, &message).expect("answered")
    }

    fn answer(handler: &AnnounceHandler, message: &[u8]) -> Result<bool, RemoteException> {
        let variable = VariableHeader::default();
        let request = Request {
            action: AnnounceHandler::ACTION,
            variable: &variable,
            message,
            version: WireVersion::SHELL,
        };
        let mut sink = Vec::new();
        handler.handle(&request, &mut Output::new(&mut sink, 16))?;
        Ok(AnnounceResponse::read(&mut Input::new(&sink))
            .expect("read")
            .refreshed)
    }

    #[test]
    fn an_unknown_joiner_makes_the_node_read_the_fleet_once() {
        let fleet = fake(&[("a", "e")], true);
        let handler = AnnounceHandler::new(fleet.clone(), Duration::ZERO);
        assert!(ask(&handler, "b", "e2"));
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_joiner_already_held_reads_nothing() {
        let fleet = fake(&[("b", "e2")], true);
        let handler = AnnounceHandler::new(fleet.clone(), Duration::ZERO);
        assert!(!ask(&handler, "b", "e2"));
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_restarted_joiner_is_not_the_one_held() {
        let fleet = fake(&[("b", "old")], true);
        let handler = AnnounceHandler::new(fleet.clone(), Duration::ZERO);
        assert!(ask(&handler, "b", "new"));
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn announces_inside_the_gap_cost_one_read_now_and_one_when_it_ends() {
        let fleet = fake(&[], true);
        let handler = AnnounceHandler::new(fleet.clone(), Duration::from_millis(150));
        assert!(ask(&handler, "b", "e"));
        for i in 0..20 {
            assert!(!ask(&handler, &format!("c{i}"), "e"));
        }
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while fleet.reads.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Past the gap the queued read has run, once.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_read_that_failed_is_not_reported() {
        let fleet = fake(&[], false);
        let handler = AnnounceHandler::new(fleet, Duration::ZERO);
        assert!(!ask(&handler, "b", "e"));
    }

    #[test]
    fn a_malformed_announce_is_refused_without_a_read() {
        let fleet = fake(&[], true);
        let handler = AnnounceHandler::new(fleet.clone(), Duration::ZERO);
        assert!(answer(&handler, &[9, 1]).is_err());
        assert_eq!(fleet.reads.load(Ordering::SeqCst), 0);
    }
}
