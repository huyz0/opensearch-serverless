//! The writer role of a node: applies a write to the shard's engine, stages
//! its record in the node's one write log, and acknowledges only once the
//! log blob holding it landed. Ported from the write path of
//! `serverless/shell/src/main/java/org/opensearch/serverless/node/NodeWritePath.java`
//! (`indexUnderFence`, `appendOrRelease`): the engine runs first because it
//! assigns the sequence number the record carries; a stage that fails after
//! the engine applied the write releases the shard, which reopens from the
//! log. A bulk group is applied item by item and staged with one commit.

mod dead_log_pass;
mod dead_logs;
mod get;
mod heads;
mod hibernate;
mod incarnation;
mod janitor_pass;
mod janitor_timer;
mod lease;
mod limit;
mod newest_commit;
mod opener;
mod orphan_timer;
mod predecessors;
mod proactive;
mod publish_age;
mod publish_timer;
mod publishing;
mod redirect;
mod refresh;
mod routes;
mod slot;
mod snapshot;
mod tail;
mod takeover;
mod target;
mod tombstone_pass;

use std::collections::HashMap;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use oss_format::WalRecord;
use oss_writer::admission::WriteLimiter;
use oss_writer::bulk::{IndexInfo, Kind, Owner, Planned};
use oss_writer::index::{Refused, Written};
use oss_writer::log::NodeLogCommitter;
use oss_writer::publish::lazy::PublishAge;

use incarnation::Fenced;
use publishing::Key;
use redirect::Redirects;
use slot::ShardSlot;

pub use dead_log_pass::{
    DeadLogParts, DeadLogPass, DeadLogReport, DeadMailbox, DeadSnapshot, REAPERS,
};
pub use dead_logs::{StoreCommits, StoreNodeLogs};
pub use heads::{HeadAdmission, ShardNames};
pub use hibernate::{
    GIVE_BACK_PER_PASS, HIBERNATE_AFTER_MILLIS, HibernatePass, KEEP_ASLEEP_MILLIS,
    SETTINGS as HIBERNATE_SETTINGS,
};
pub use incarnation::{DescriptorIncarnation, FENCE_WINDOW, Incarnation};
pub use janitor_pass::{HeldShard, JanitorPass, JanitorReport, PassLimits};
pub use janitor_timer::{JanitorTimer, SweepConfig};
pub use lease::MembershipLeaseWrite;
pub use opener::{HeadOpener, OpenRefused, ShardOpener};
pub use orphan_timer::{LiveLeases, OrphanParts, OrphanTimer};
pub use proactive::{
    Config as ProactiveConfig, ProactiveTakeover, Report as ProactiveReport,
    SETTINGS as PROACTIVE_SETTINGS,
};
pub use publish_age::publish_age_of;
pub use publish_timer::PublishTimer;
pub use publishing::{PublishPass, Publishing};
pub use routes::Routes;
pub use tail::{RefreshVisibility, TailSink};
pub use takeover::Takeover;
pub use target::RoleTarget;
pub use tombstone_pass::{DeleteProbe, TombstoneParts, TombstonePass, TombstoneReport};

/// Why an item did not go ahead.
enum Failure {
    /// The engine refused it; it took no sequence number.
    Refused(Refused),
    /// The write could not be made durable or the shard is not held.
    Unavailable(String),
    /// The node's write limiter refused it before anything was applied.
    Throttled(String),
}

/// The shards held, by index uuid and shard number.
type Held = HashMap<(String, u32), Arc<Mutex<ShardSlot>>>;

/// The commit each held shard last published, by index uuid and shard.
type Newest = HashMap<(String, u32), Arc<[u8]>>;

/// The largest commit manifest a shard's newest-commit answer holds: the
/// largest a reader reads.
pub const MAX_NEWEST_BYTES: usize = 4 << 20;

/// The shards this node writes, and the log their writes are staged in.
pub struct WriterRole {
    committer: Arc<NodeLogCommitter>,
    /// What limits the writes waiting on the log; none, and every write is
    /// admitted.
    limiter: OnceLock<WriteLimiter>,
    /// What the limiter's times count from.
    epoch: Instant,
    routes: Arc<dyn Routes>,
    names: Arc<ShardNames>,
    incarnation: Option<Fenced>,
    shards: Mutex<Held>,
    newest: Mutex<Newest>,
    publishing: Option<Publishing>,
    tail: OnceLock<Arc<dyn TailSink>>,
    /// How far a refresh is made visible before it returns; the code of a
    /// [`RefreshVisibility`].
    visibility: AtomicU8,
    /// What opens a shard a write names and this node does not hold.
    opener: Option<Arc<dyn ShardOpener>>,
    /// Held while the opener runs, so racing groups open a shard once.
    opening: Mutex<()>,
    /// The live node whose head holds a shard this node was asked to write.
    redirects: Mutex<Redirects>,
    hibernation: hibernate::Hibernation,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WriterRole {
    /// A role staging into `committer`, asking `routes` about everything it
    /// does not hold.
    pub fn new(committer: Arc<NodeLogCommitter>, routes: Arc<dyn Routes>) -> Self {
        Self {
            committer,
            limiter: OnceLock::new(),
            epoch: Instant::now(),
            routes,
            names: Arc::new(ShardNames::default()),
            incarnation: None,
            // ALLOC: one entry a shard this node writes.
            shards: Mutex::new(HashMap::new()),
            // ALLOC: empty; at most one manifest a held shard, MAX_NEWEST_BYTES each.
            newest: Mutex::new(HashMap::new()),
            publishing: None,
            tail: OnceLock::new(),
            visibility: AtomicU8::new(0),
            opener: None,
            opening: Mutex::new(()),
            // ALLOC: one entry a shard another node holds that this node was asked to write.
            redirects: Mutex::new(HashMap::new()),
            hibernation: hibernate::Hibernation::default(),
        }
    }

    /// The highest sequence number the shard holds, or -1 when it is not
    /// held here.
    pub fn max_seq_no(&self, uuid: &str, shard: u32) -> i64 {
        self.slot(uuid, shard)
            .map_or(-1, |slot| lock(&slot).max_seq_no())
    }

    /// The names of held indexes are kept in `names`, which admission
    /// shares to find a head by name.
    #[must_use]
    pub fn with_names(mut self, names: Arc<ShardNames>) -> Self {
        self.names = names;
        self
    }

    /// Confirms through `check`, at most once a `window` for an index,
    /// that a held shard's index is still the incarnation it was opened for.
    #[must_use]
    pub fn with_incarnation(
        mut self,
        check: Arc<dyn Incarnation>,
        window: std::time::Duration,
    ) -> Self {
        self.incarnation = Some(Fenced::new(check, window));
        self
    }

    /// Publishes held shards' commits as `publishing` decides, when
    /// [`Self::publish_due`] is called.
    #[must_use]
    pub fn with_publishing(mut self, publishing: Publishing) -> Self {
        self.publishing = Some(publishing);
        self
    }

    /// Sets how long a write to `shard` of the index `uuid` may wait to be
    /// published: its index's `refresh_interval`. Nothing is recorded for a
    /// role that does not publish.
    pub fn set_publish_age(&self, uuid: &str, shard: u32, age: PublishAge) {
        if let Some(publishing) = &self.publishing {
            // ALLOC: one key, once a shard opened.
            publishing.set_age(&(uuid.to_owned(), shard), age);
        }
    }

    /// Publishes each held shard the lazy policy finds due, remembering the
    /// manifest as its newest commit. A shard that fails stays due and the
    /// pass goes on.
    pub fn publish_due(&self) -> PublishPass {
        let mut pass = PublishPass::default();
        let Some(publishing) = &self.publishing else {
            return pass;
        };
        // ALLOC: one handle a held shard, once a pass.
        let held: Vec<(Key, Arc<Mutex<ShardSlot>>)> = lock(&self.shards)
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        for (key, slot) in held {
            let Some(index) = self.names.name_of(&key.0) else {
                continue;
            };
            match self.publish_shard(publishing, &key, &index, &slot, true) {
                Ok(None) => {}
                Ok(Some((done, max_seq_no))) => {
                    if let Some(t) = self.tail.get() {
                        t.published(&key.0, key.1, max_seq_no);
                    }
                    self.staged(&key.0, key.1, &done.manifest);
                    pass.published = pass.published.saturating_add(1);
                }
                Err(reason) => {
                    // ALLOC: error path; one message a failed shard.
                    pass.failed.push(format!("[{}][{}] {reason}", key.0, key.1));
                }
            }
        }
        pass
    }

    /// The shards this node holds that have a name, each with the commit it
    /// last published (none before its first publish).
    pub fn held_shards(&self) -> Vec<HeldShard> {
        // Lock order: shards, then newest, everywhere both are held.
        let shards = lock(&self.shards);
        let newest = lock(&self.newest);
        // ALLOC: one entry a shard held, at most the shards of the node.
        shards
            .keys()
            .filter_map(|key| {
                Some(HeldShard {
                    name: self.names.name_of(&key.0)?,
                    uuid: key.0.clone(),
                    shard: key.1,
                    newest: newest.get(key).cloned(),
                })
            })
            .collect()
    }

    /// Whether this node holds the shard.
    pub fn held(&self, index_uuid: &str, shard: u32) -> bool {
        // ALLOC: one key a lookup.
        lock(&self.shards).contains_key(&(index_uuid.to_owned(), shard))
    }

    fn slot(&self, uuid: &str, shard: u32) -> Option<Arc<Mutex<ShardSlot>>> {
        // ALLOC: one key a lookup.
        lock(&self.shards).get(&(uuid.to_owned(), shard)).cloned()
    }

    /// Lets go of the shard: the engine reopens from the log.
    fn release(&self, uuid: &str, shard: u32, slot: &Arc<Mutex<ShardSlot>>) {
        let mut shards = lock(&self.shards);
        // ALLOC: one key a release.
        let key = (uuid.to_owned(), shard);
        // Not a slot opened since.
        if shards.get(&key).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            shards.remove(&key);
            lock(&self.newest).remove(&key);
            if let Some(p) = &self.publishing {
                p.forget(&key);
            }
            if let Some(t) = self.tail.get() {
                t.released(uuid, shard);
            }
        }
    }

    /// Whether the group's shard is held here; otherwise who owns it.
    pub(crate) fn owner(&self, index: &IndexInfo, shard: u32) -> Owner {
        if self.held(&index.uuid, shard) {
            Owner::Local
        } else if let Some(holder) = self.redirect(&index.uuid, shard) {
            Owner::Node(holder)
        } else {
            self.routes.owner(index, shard)
        }
    }

    /// Applies each item of a group to the shard and stages the written
    /// ones in one commit, so every written item is in the log when this
    /// returns `Ok`.
    fn group(
        &self,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<Result<Written, Failure>> {
        match self.slot_or_open(index, shard) {
            Ok(slot) => self.group_on(slot, index, shard, group),
            Err(refused) => unavailable(group.len(), || refused.reason.clone()),
        }
    }

    /// Applies a group to the opened `slot` of the shard, once the write
    /// limiter admits it; a group it refuses is neither applied nor logged.
    fn group_on(
        &self,
        slot: Arc<Mutex<ShardSlot>>,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<Result<Written, Failure>> {
        // Held to the end: the slot is given back once the group's records
        // have landed or failed.
        let _admitted = match self.admit_group(group.len()) {
            Ok(permit) => permit,
            Err(refused) => return refused,
        };
        self.group_admitted(slot, index, shard, group)
    }

    /// Applies an admitted group to the opened `slot` of the shard.
    fn group_admitted(
        &self,
        slot: Arc<Mutex<ShardSlot>>,
        index: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<Result<Written, Failure>> {
        if let Some(fenced) = &self.incarnation {
            match fenced.ensure(&index.name, &index.uuid) {
                Ok(true) => {}
                Ok(false) => {
                    // The index was deleted and created again: this shard
                    // serves a deleted one.
                    self.release(&index.uuid, shard, &slot);
                    return unavailable(group.len(), || {
                        // ALLOC: error path; one message.
                        format!(
                            "[{}] is no longer the index this shard was opened for",
                            index.name
                        )
                    });
                }
                Err(e) => return unavailable(group.len(), || e.clone()),
            }
        }
        let held = Arc::clone(&slot);
        let mut slot = lock(&slot);
        if slot.asleep() {
            // Slept after this write took the slot: wake the shard.
            drop(slot);
            return self.group(index, shard, group);
        }
        if slot.released() {
            return unavailable(group.len(), || {
                // ALLOC: error path; one message.
                format!(
                    "cannot index into [{}][{shard}]: released after a failed write",
                    index.name
                )
            });
        }
        // ALLOC: one outcome and one record an item of the group.
        let mut outcomes: Vec<Result<Written, Failure>> = Vec::with_capacity(group.len());
        let mut records: Vec<WalRecord<'_>> = Vec::with_capacity(group.len());
        let mut staged: Vec<usize> = Vec::with_capacity(group.len());
        for item in group {
            let a = &item.op.action;
            let source = match std::str::from_utf8(&item.op.source) {
                Ok(s) => s,
                Err(_) if a.kind == Kind::Delete => "",
                Err(_) => {
                    outcomes.push(Err(Failure::Unavailable(
                        // ALLOC: error path; one message.
                        "the document source is not valid UTF-8".to_owned(),
                    )));
                    continue;
                }
            };
            let routing = a.routing.as_deref().filter(|r| !r.is_empty());
            // Checked before the engine runs: a write the engine took must
            // always have a record to stage.
            if let Err(e) = WalRecord::index(item.id, source, 0, 0, 0).with_routing(routing) {
                outcomes.push(Err(Failure::Unavailable(e.to_string())));
                continue;
            }
            match slot.apply(
                a.kind,
                item.id,
                &item.op.source,
                routing,
                a.create,
                &a.expect,
            ) {
                Ok(written) => {
                    let record = if a.kind == Kind::Delete {
                        WalRecord::deletion(
                            item.id,
                            written.seq_no,
                            written.primary_term,
                            written.version,
                        )
                    } else {
                        WalRecord::index(
                            item.id,
                            source,
                            written.seq_no,
                            written.primary_term,
                            written.version,
                        )
                    };
                    // The routing was checked above, so this keeps the record.
                    let record = record.with_routing(routing).unwrap_or_else(|_| {
                        WalRecord::index(
                            item.id,
                            source,
                            written.seq_no,
                            written.primary_term,
                            written.version,
                        )
                    });
                    staged.push(outcomes.len());
                    records.push(record);
                    outcomes.push(Ok(written));
                }
                Err(refused) => outcomes.push(Err(Failure::Refused(refused))),
            }
        }
        let appended_at = self.limiter_now();
        let committed = self.committer.commit(slot.key(), slot.term(), &records);
        if !records.is_empty() {
            self.appended(appended_at, committed.is_ok());
        }
        if let (Ok(_), Some(p), false) = (&committed, &self.publishing, records.is_empty()) {
            // ALLOC: one key a staged group.
            p.written(&(index.uuid.clone(), shard));
        }
        if let (Ok(_), Some(t)) = (&committed, self.tail.get()) {
            // ALLOC: one owned copy an acknowledged record, the keep's.
            let owned: Vec<WalRecord<'static>> =
                records.iter().cloned().map(WalRecord::into_owned).collect();
            t.acked(&index.name, &index.uuid, shard, &owned);
        }
        if let Err(e) = committed {
            // The engine holds writes the log does not: the shard reopens
            // from the log, and these writes are refused.
            slot.release();
            drop(slot);
            self.release(&index.uuid, shard, &held);
            for at in staged {
                if let Some(o) = outcomes.get_mut(at) {
                    // ALLOC: error path; one message an item.
                    *o = Err(Failure::Unavailable(format!(
                        "the write could not be made durable: {e}"
                    )));
                }
            }
        }
        outcomes
    }
}

fn unavailable(n: usize, reason: impl Fn() -> String) -> Vec<Result<Written, Failure>> {
    // ALLOC: one outcome an item of the group.
    (0..n)
        .map(|_| Err(Failure::Unavailable(reason())))
        .collect()
}
