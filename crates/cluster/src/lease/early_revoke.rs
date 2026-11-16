//! Revoking a refused peer's lease before it expires (ADR 0016, point 7).
//!
//! A forward to a lease's holder that is refused at the TCP connect
//! (ECONNREFUSED: the host answered and nothing listens on the port) means
//! the process is gone while its lease still runs for up to a lifetime. A
//! node binds its transport before its first lease and keeps it until the
//! lease is released, so a live node's port never refuses. This revokes
//! such a lease early, so the normal takeover moves its shards a lifetime
//! sooner. A timeout, a reset mid-call or any other failure never revokes:
//! those cannot tell a dead process from a busy one.
//!
//! It revokes only when every check holds, each a reason to decline:
//! the lease read now names the process called (ephemeral id and address);
//! it was renewed at least 2 s ago, plus the skew margin; a bare connect at
//! least 500 ms after the first refusal is refused too; no revocation of
//! that process is running on this node; and none was declined for it in
//! the last lifetime. Then the lease is swapped to a revoked copy at the
//! generation read, so a renewal in between wins. A false positive costs a
//! needless handover, never two writers: the write log and shard heads
//! fence.
//!
//! Off by default (`serverless.lease.early_revoke`) until the takeover and
//! fencing suites pass with it on and the stall it cuts is measured. The
//! Java shell has no early revocation; Java nodes treat the revoked lease as
//! dead and take part in the takeover unchanged.
//!
//! Costs one lease read and one swap a detecting node, and nothing while
//! the fleet is healthy.

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::SwapOutcome;
use oss_format::MAX_LEASE_BYTES;
use oss_types::settings::{BoolSetting, Scope, Setting};

use super::lock;
use super::membership::Membership;

/// `serverless.lease.early_revoke`: revoke a refused peer's lease before
/// it expires. Default `false`.
pub const EARLY_REVOKE: BoolSetting = BoolSetting::new(
    "serverless.lease.early_revoke",
    false,
    Scope::Node,
    "Revoke the lease of a peer whose transport port refuses connections, \
     confirmed by a second refused connect, before the lease expires.",
);

/// Every setting this crate defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[EARLY_REVOKE.setting()];

/// How long ago a lease must have been renewed, beyond the skew margin,
/// for a refusal to revoke it: a process that renewed this recently was
/// live a moment ago, so a refusal now is more likely a restart in flight.
pub const MIN_RENEWED_AGO_MILLIS: i64 = 2_000;

/// How long after the first refusal the confirming connect is made.
pub const CONFIRM_AFTER_MILLIS: i64 = 500;

/// How long the confirming connect may take; past it, nothing is revoked.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(1);

/// Whether a bare TCP connect to an address is refused.
pub trait ConnectProbe: Send + Sync {
    /// True only when the connect was refused: a timeout, a success or any
    /// other error is false.
    fn refused(&self, address: SocketAddr) -> bool;
}

/// [`ConnectProbe`] over a real socket, closed at once.
#[derive(Debug, Clone, Copy)]
pub struct TcpProbe {
    /// The longest the connect may take.
    pub timeout: Duration,
}

impl Default for TcpProbe {
    fn default() -> Self {
        Self {
            timeout: CONFIRM_TIMEOUT,
        }
    }
}

impl ConnectProbe for TcpProbe {
    fn refused(&self, address: SocketAddr) -> bool {
        matches!(
            TcpStream::connect_timeout(&address, self.timeout),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused
        )
    }
}

/// What a refusal led to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The setting is off: nothing was read.
    Off,
    /// The lease was swapped to a revoked copy here.
    Revoked,
    /// The process called no longer holds a running lease: none, revoked,
    /// expired, or one of a later process. The normal takeover applies.
    Gone,
    /// Nothing was revoked, for this reason.
    Declined(Declined),
}

/// Why a refusal did not revoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declined {
    /// A revocation of this process is running on this node.
    InFlight,
    /// One was declined for this process within the last lifetime.
    RecentlyDeclined,
    /// The negative cache holds `max_members` processes declined within
    /// the last lifetime: refusing is the safe answer.
    Crowded,
    /// The lease names this process at another address.
    AnotherAddress,
    /// The lease was renewed too recently.
    RenewedRecently,
    /// The confirming connect was not refused.
    Answered,
    /// The lease changed between the read and the swap: renewed.
    Renewed,
    /// The store failed, or the lease did not parse or fit.
    Store,
}

#[derive(Default)]
struct State {
    running: HashSet<String>,
    // Process key to when the decline stops counting, epoch millis.
    declined: HashMap<String, i64>,
}

/// Early revocation over one node's membership.
pub struct EarlyRevoke {
    membership: Arc<Membership>,
    probe: Arc<dyn ConnectProbe>,
    enabled: bool,
    state: Mutex<State>,
}

/// Ends a revocation's single-flight claim when dropped.
struct Claim<'a> {
    state: &'a Mutex<State>,
    key: &'a str,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        lock(self.state).running.remove(self.key);
    }
}

impl EarlyRevoke {
    /// Early revocation over `membership`'s leases, confirming with
    /// `probe`; `enabled` is [`EARLY_REVOKE`] as the node read it.
    pub fn new(membership: Arc<Membership>, probe: Arc<dyn ConnectProbe>, enabled: bool) -> Self {
        Self {
            membership,
            probe,
            enabled,
            state: Mutex::new(State::default()),
        }
    }

    /// Whether the setting is on.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Called when a connect to `node_id`'s process `ephemeral_id` at
    /// `address` was refused at `first_refused_at_millis` (this node's
    /// clock). Blocks until 500 ms after that refusal for the confirming
    /// connect, then up to [`CONFIRM_TIMEOUT`] for it.
    pub fn on_refused(
        &self,
        node_id: &str,
        ephemeral_id: &str,
        address: SocketAddr,
        first_refused_at_millis: i64,
    ) -> Verdict {
        if !self.enabled {
            return Verdict::Off;
        }
        // ALLOC: one key a refusal, the ids' length.
        let key = format!("{node_id}#{ephemeral_id}");
        let _claim = match self.claim(&key) {
            Ok(claim) => claim,
            Err(declined) => return Verdict::Declined(declined),
        };
        let verdict = self.decide(node_id, ephemeral_id, address, first_refused_at_millis);
        if let Verdict::Declined(
            Declined::AnotherAddress
            | Declined::RenewedRecently
            | Declined::Answered
            | Declined::Renewed,
        ) = verdict
        {
            let until = self
                .membership
                .clock
                .now_millis()
                .saturating_add(self.membership.config.ttl_millis);
            // ALLOC: one entry a declined process, at most max_members (claim).
            lock(&self.state).declined.insert(key.clone(), until);
        }
        verdict
    }

    /// Takes the single-flight claim on `key`, unless one is running, one
    /// was declined within a lifetime, or the negative cache is full.
    fn claim<'a>(&'a self, key: &'a str) -> Result<Claim<'a>, Declined> {
        let now = self.membership.clock.now_millis();
        let mut state = lock(&self.state);
        if state.running.contains(key) {
            return Err(Declined::InFlight);
        }
        if state.declined.get(key).is_some_and(|until| now < *until) {
            return Err(Declined::RecentlyDeclined);
        }
        state.declined.retain(|_, until| now < *until);
        if state.declined.len() >= self.membership.config.max_members {
            return Err(Declined::Crowded);
        }
        // ALLOC: one key a revocation running, at most the peers refused at once.
        state.running.insert(key.to_owned());
        Ok(Claim {
            state: &self.state,
            key,
        })
    }

    fn decide(
        &self,
        node_id: &str,
        ephemeral_id: &str,
        address: SocketAddr,
        first_refused_at_millis: i64,
    ) -> Verdict {
        let membership = &self.membership;
        let Ok(lease_key) = membership.lease_key(node_id) else {
            return Verdict::Declined(Declined::Store);
        };
        // ALLOC: two lease buffers a revocation, MAX_LEASE_BYTES each.
        let (mut read, mut write) = (vec![0u8; MAX_LEASE_BYTES], vec![0u8; MAX_LEASE_BYTES]);
        let (generation, lease) = match membership.read_lease(&lease_key, &mut read) {
            Ok(Some(found)) => found,
            Ok(None) => return Verdict::Gone,
            Err(_) => return Verdict::Declined(Declined::Store),
        };
        let now = membership.clock.now_millis();
        if lease.revoked || lease.ephemeral_id != ephemeral_id || lease.is_expired_at(now) {
            return Verdict::Gone;
        }
        if lease.address.parse::<SocketAddr>().ok() != Some(address) {
            return Verdict::Declined(Declined::AnotherAddress);
        }
        let renewed_at = lease
            .expires_at_millis
            .saturating_sub(membership.config.ttl_millis);
        let settle = MIN_RENEWED_AGO_MILLIS.saturating_add(membership.skew_margin_millis());
        if now.saturating_sub(renewed_at) < settle {
            return Verdict::Declined(Declined::RenewedRecently);
        }
        let wait = first_refused_at_millis
            .saturating_add(CONFIRM_AFTER_MILLIS)
            .saturating_sub(now);
        if wait > 0 {
            membership
                .clock
                .pause(Duration::from_millis(wait.unsigned_abs()));
        }
        if !self.probe.refused(address) {
            return Verdict::Declined(Declined::Answered);
        }
        let mut revoked = lease;
        revoked.revoked = true;
        let Ok(len) = revoked.write(&mut write) else {
            return Verdict::Declined(Declined::Store);
        };
        match membership.store.swap_register(
            &lease_key,
            generation,
            write.get(..len).unwrap_or_default(),
        ) {
            Ok(SwapOutcome::Applied(_)) => Verdict::Revoked,
            Ok(SwapOutcome::Conflict(_)) => Verdict::Declined(Declined::Renewed),
            Err(_) => Verdict::Declined(Declined::Store),
        }
    }
}
