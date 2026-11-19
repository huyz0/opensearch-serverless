//! Where one request's documents belong: the descriptor and owner lookups it
//! has already made, the placement of a document, and what to do when a
//! forward to the owner fails.
//!
//! A [`Router`] lives as long as one request, so its memos live exactly as
//! long as it is safe to: ten documents of one index read the descriptor
//! once and each shard's head once, however many documents landed on it.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// A metadata read that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceError(pub String);

/// What a router needs from the node it runs on.
pub trait Plane {
    /// What a descriptor carries that placement needs.
    type Desc: Clone;

    /// Whether `name` belongs to a plugin.
    fn is_system_index(&self, name: &str) -> bool;

    /// The descriptor of `index` through the routing cache, if it exists.
    ///
    /// # Errors
    ///
    /// A register that cannot be read.
    fn describe(&self, index: &str) -> Result<Option<Self::Desc>, PlaceError>;

    /// The uuid of the index incarnation `desc` describes.
    fn uuid(&self, desc: &Self::Desc) -> String;

    /// The shard `id` (or its custom routing) belongs to.
    fn shard_for(&self, desc: &Self::Desc, id: &str, routing: Option<&str>) -> u32;

    /// The owner this node last saw for a shard, before the register.
    fn owner_hint(&self, index: &str, shard: u32) -> Option<String>;

    /// Reads the shard's head, notes it for the node, and gives its owner.
    ///
    /// # Errors
    ///
    /// A register that cannot be read.
    fn read_head(&self, index: &str, shard: u32) -> Result<Option<String>, PlaceError>;

    /// Forgets the owner hint for a shard.
    fn forget_owner(&self, index: &str, shard: u32);

    /// Whether this node holds exactly this shard of exactly this
    /// incarnation as a writer.
    fn holds_shard(&self, index: &str, uuid: &str, shard: u32) -> bool;

    /// This node's id.
    fn local_node_id(&self) -> &str;

    /// Tells the node its idea of who owns a shard may be wrong.
    fn ownership_doubted(&self, index: &str, shard: u32);
}

/// Where a document belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// Whether this node holds the shard as a writer.
    pub local: bool,
    /// The shard.
    pub shard: u32,
    /// The node the shard's head names, if any.
    pub owner: Option<String>,
    /// The uuid of the index incarnation.
    pub uuid: String,
}

/// How a forward to an owner failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardFailure {
    /// The node is not the shard's owner.
    NotOwner,
    /// The owner could not be reached: the request was never sent.
    Unreachable,
    /// The owner did not answer in time: the request may have been applied.
    Deadline,
    /// The connection dropped with the request in flight.
    Interrupted,
    /// A compare-and-swap was lost on a healthy owner.
    Conflict,
    /// The owner rejected the request for load.
    Rejected,
    /// The owner answered with some other failure.
    Remote,
}

/// A failed forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// How it failed.
    pub kind: ForwardFailure,
    /// The failure's message.
    pub message: String,
    /// Whether the owner answered with a status below 500.
    pub client_status: bool,
    /// Whether the failure is the owner's missing live lease, which keeps
    /// its own words.
    pub owner_unreachable: bool,
}

/// Why a request cannot be placed or forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The index belongs to a plugin.
    SystemIndex(String),
    /// The index does not exist.
    NoSuchIndex(String),
    /// A register could not be read.
    Metadata(String),
    /// A lost condition, to be answered as itself.
    Conflict(Failure),
    /// The owner's own client-side answer, to be answered as it is.
    Answer(Failure),
    /// This node cannot serve the placement.
    NotHere {
        /// What to tell the caller.
        message: String,
        /// The node that owns the shard, if any.
        owner: Option<String>,
        /// Whether the caller can retry.
        retryable: bool,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemIndex(index) => write!(
                f,
                "[{index}] belongs to a plugin and is not reachable through the request path"
            ),
            Self::NoSuchIndex(index) => write!(f, "no such index: {index}"),
            Self::Metadata(message) => f.write_str(message),
            Self::Conflict(failure) | Self::Answer(failure) => f.write_str(&failure.message),
            Self::NotHere { message, .. } => f.write_str(message),
        }
    }
}

/// What to do after a forward failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Forward to this placement next.
    Forward(Placement),
    /// Answer the caller with this.
    Refuse(Refusal),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One request's placements.
pub struct Router<'p, P: Plane> {
    plane: &'p P,
    refuse_system_indices: bool,
    described: Mutex<HashMap<String, Option<P::Desc>>>,
    owners: Mutex<HashMap<String, Option<String>>>,
}

fn owner_key(index: &str, shard: u32) -> String {
    // ALLOC: one name or id a failure or placement, bounded by the request.
    format!("{index}#{shard}")
}

fn describe_failure(owner: Option<&str>, failure: &Failure) -> String {
    let owner = owner.unwrap_or("null");
    match failure.kind {
        // ALLOC: one name or id a failure or placement, bounded by the request.
        ForwardFailure::Deadline => format!(
            "the owner {owner} did not answer within the deadline; the operation may or may not \
             have been applied there, so read before retrying a write"
        ),
        // ALLOC: one name or id a failure or placement, bounded by the request.
        ForwardFailure::Interrupted => format!(
            "the connection to the owner {owner} dropped while the request was in flight; the \
             operation may or may not have been applied there, so read before retrying a write"
        ),
        // ALLOC: one name or id a failure or placement, bounded by the request.
        _ => format!(
            "could not forward to {owner}, which the shard-head named as owner: {}",
            failure.message
        ),
    }
}

impl<'p, P: Plane> Router<'p, P> {
    /// A router for one request over `plane`; `refuse_system_indices` when
    /// the caller is on the request path.
    pub fn new(plane: &'p P, refuse_system_indices: bool) -> Self {
        Self {
            plane,
            refuse_system_indices,
            // ALLOC: one name or id a failure or placement, bounded by the request.
            described: Mutex::new(HashMap::new()),
            owners: Mutex::new(HashMap::new()),
        }
    }

    fn describe_once(&self, index: &str) -> Result<Option<P::Desc>, Refusal> {
        let mut memo = lock(&self.described);
        if let Some(known) = memo.get(index) {
            return Ok(known.clone());
        }
        let desc = self
            .plane
            .describe(index)
            .map_err(|e| Refusal::Metadata(e.0))?;
        // ALLOC: one entry a distinct index in the request.
        memo.insert(index.to_owned(), desc.clone());
        Ok(desc)
    }

    fn fresh_owner(&self, index: &str, shard: u32) -> Result<Option<String>, Refusal> {
        self.plane
            .read_head(index, shard)
            .map_err(|e| Refusal::Metadata(e.0))
    }

    /// The owner as this node last saw it, before the register: a wrong hint
    /// is refused by the node it names and corrected by one read then.
    fn owner_once(&self, index: &str, shard: u32) -> Result<Option<String>, Refusal> {
        let key = owner_key(index, shard);
        let mut memo = lock(&self.owners);
        if let Some(known) = memo.get(&key) {
            return Ok(known.clone());
        }
        let owner = match self.plane.owner_hint(index, shard) {
            Some(hint) => Some(hint),
            None => self.fresh_owner(index, shard)?,
        };
        // ALLOC: one entry a distinct shard in the request.
        memo.insert(key, owner.clone());
        Ok(owner)
    }

    /// Where the document `id` of `index` belongs.
    ///
    /// # Errors
    ///
    /// [`Refusal::SystemIndex`] before the descriptor is read,
    /// [`Refusal::NoSuchIndex`], or [`Refusal::Metadata`].
    pub fn place(
        &self,
        index: &str,
        id: &str,
        routing: Option<&str>,
    ) -> Result<Placement, Refusal> {
        if self.refuse_system_indices && self.plane.is_system_index(index) {
            // Before the descriptor is read: a refusal that first confirmed
            // the index exists would say so.
            // ALLOC: one name or id a failure or placement, bounded by the request.
            return Err(Refusal::SystemIndex(index.to_owned()));
        }
        let Some(desc) = self.describe_once(index)? else {
            // ALLOC: one name or id a failure or placement, bounded by the request.
            return Err(Refusal::NoSuchIndex(index.to_owned()));
        };
        let shard = self.plane.shard_for(&desc, id, routing);
        let uuid = self.plane.uuid(&desc);
        let local = self.plane.holds_shard(index, &uuid, shard);
        let owner = self.owner_once(index, shard)?;
        Ok(Placement {
            local,
            shard,
            owner,
            uuid,
        })
    }

    /// Why this node cannot serve `placement`: no node owns the shard, this
    /// node is acquiring it, or another node owns it.
    pub fn not_here(&self, index: &str, placement: &Placement) -> Refusal {
        let shard = placement.shard;
        match placement.owner.as_deref() {
            None => {
                self.plane.ownership_doubted(index, shard);
                Refusal::NotHere {
                    // ALLOC: one name or id a failure or placement, bounded by the request.
                    message: format!("no node currently owns shard {shard} of {index}"),
                    owner: None,
                    retryable: false,
                }
            }
            Some(owner) if owner == self.plane.local_node_id() => {
                // Brief only if an activation is running; a wedge otherwise,
                // so the doubt goes up either way.
                self.plane.ownership_doubted(index, shard);
                Refusal::NotHere {
                    // ALLOC: one name or id a failure or placement, bounded by the request.
                    message: format!("this node is acquiring shard {shard} of {index}; retry"),
                    owner: Some(owner.to_owned()),
                    retryable: true,
                }
            }
            Some(owner) => Refusal::NotHere {
                // ALLOC: one name or id a failure or placement, bounded by the request.
                message: format!("shard {shard} of {index} is owned by {owner}"),
                owner: Some(owner.to_owned()),
                retryable: true,
            },
        }
    }

    /// What to do after a forward to `stale`'s owner failed with `failure`.
    ///
    /// The hint is forgotten first, whatever the failure was. A refusal or an
    /// unreachable owner means the request was never applied: with
    /// `may_retry` the head is read once and, if it names somebody new, the
    /// caller forwards there. A deadline or a dropped connection means it may
    /// have been applied, so it is not sent again. An answer from the owner is
    /// the owner's answer.
    pub fn after_forward_failure(
        &self,
        index: &str,
        stale: &Placement,
        failure: &Failure,
        may_retry: bool,
    ) -> Outcome {
        let shard = stale.shard;
        self.plane.forget_owner(index, shard);
        lock(&self.owners).remove(&owner_key(index, shard));
        let stale_owner = stale.owner.as_deref();
        let not_here = |message: String, owner: Option<String>, retryable: bool| {
            Outcome::Refuse(Refusal::NotHere {
                message,
                owner,
                retryable,
            })
        };
        match failure.kind {
            ForwardFailure::Conflict => {
                // A healthy owner answered correctly: not stale routing.
                return Outcome::Refuse(Refusal::Conflict(failure.clone()));
            }
            ForwardFailure::Rejected | ForwardFailure::Remote => {
                if failure.client_status {
                    return Outcome::Refuse(Refusal::Answer(failure.clone()));
                }
                return not_here(
                    describe_failure(stale_owner, failure),
                    stale_owner.map(str::to_owned),
                    true,
                );
            }
            ForwardFailure::Deadline | ForwardFailure::Interrupted => {
                self.plane.ownership_doubted(index, shard);
                return not_here(
                    describe_failure(stale_owner, failure),
                    stale_owner.map(str::to_owned),
                    true,
                );
            }
            ForwardFailure::NotOwner | ForwardFailure::Unreachable => {}
        }
        // The owner never applied it: one register read, and one more
        // forward to whoever it names now.
        let fresh = if may_retry {
            match self.fresh_owner(index, shard) {
                Ok(fresh) => {
                    // ALLOC: one entry a distinct shard in the request.
                    lock(&self.owners).insert(owner_key(index, shard), fresh.clone());
                    fresh
                }
                Err(refusal) => return Outcome::Refuse(refusal),
            }
        } else {
            None
        };
        let local = self.plane.local_node_id();
        if let Some(next) = fresh.as_deref()
            && Some(next) != stale_owner
            && next != local
        {
            return Outcome::Forward(Placement {
                local: false,
                shard,
                // ALLOC: one owner id.
                owner: Some(next.to_owned()),
                // ALLOC: one uuid.
                uuid: stale.uuid.clone(),
            });
        }
        self.plane.ownership_doubted(index, shard);
        if may_retry {
            match fresh.as_deref() {
                None => {
                    // Whoever the hint named is gone and nobody has taken
                    // over: the answer of a register that never named anyone.
                    return not_here(
                        // ALLOC: one name or id a failure or placement, bounded by the request.
                        format!(
                            "no node currently owns shard {shard} of {index}; activate it \
                             before writing"
                        ),
                        None,
                        false,
                    );
                }
                Some(next) if next == local => {
                    return not_here(
                        // ALLOC: one name or id a failure or placement, bounded by the request.
                        format!("this node is acquiring shard {shard} of {index}; retry"),
                        Some(next.to_owned()),
                        true,
                    );
                }
                Some(_) => {}
            }
        }
        // An owner with no live lease keeps its own words, which name the
        // missing lease; anything else is worded as the forward that failed.
        let message = if failure.owner_unreachable {
            failure.message.clone()
        } else {
            describe_failure(stale_owner, failure)
        };
        not_here(message, stale_owner.map(str::to_owned), true)
    }
}
