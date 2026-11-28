//! The loop body of a node's index state management: which managed indices
//! this node owns and evaluates, found without polling any index.
//!
//! A refresh (every `refresh_ms`, or when the live nodes change) reads the
//! 256 registry shards and the managed set's marker, and only when the
//! marker moved or the owned set changed the documents of the indices this
//! node owns and their policies. Between refreshes a tick is the clock and
//! the cached records: an index comes due by arithmetic on the `next_eval`
//! its record carries, and the claim and the write happen then.
//!
//! The owner of an index is the one live node with the highest rendezvous
//! weight for its name; when two nodes disagree about who is live the claim
//! register decides which of them evaluates the slot (the due time).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_cluster::placement::{ShardKey, owner};
use oss_format::{DocKind, REGISTRY_SHARDS};
use oss_meta::{DocSet, MetaError, ScheduleRegistry};
use serde_json::{Value, json};

use super::evaluate;

/// The registry namespace of this plugin.
pub const NAMESPACE: &str = "ism";

/// The one tenant of a deployment, until tenants are placed.
pub const TENANT: &str = "_default_";

/// What a tick did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Whether the registry and the sets were read.
    pub refreshed: bool,
    /// The indices this node evaluated and won the claim for.
    pub evaluated: Vec<String>,
    /// The indices whose claim another node won.
    pub lost: Vec<String>,
}

struct Owned {
    index: String,
    record: Value,
    policy: Value,
}

#[derive(Default)]
struct State {
    refreshed_at: Option<i64>,
    version: Option<i64>,
    live: Vec<String>,
    registered: bool,
    owned: Vec<Owned>,
    /// Per index: the register generation after this node's last claim, and
    /// the slot of that claim.
    held: HashMap<String, (u64, u64)>,
}

/// Decides, each tick, which managed indices this node evaluates.
pub struct Scheduler {
    registry: ScheduleRegistry,
    managed: DocSet,
    policies: DocSet,
    node: String,
    refresh_ms: i64,
    buffer_bytes: usize,
    state: Mutex<State>,
}

/// When the record next needs evaluating: its stored time, else its state's
/// start (an index just attached is due at once); `None` is "not on a time".
fn due_of(record: &Value) -> Option<i64> {
    if record.get("next_eval").is_some() {
        return record["next_eval"].as_i64();
    }
    record["state"]["start_time"].as_i64()
}

impl Scheduler {
    /// A scheduler for node `node` over the managed indices under `base`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base the registry refuses.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        node: &str,
        refresh_ms: i64,
        buffer_bytes: usize,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            registry: ScheduleRegistry::in_namespace(Arc::clone(store), base, NAMESPACE)?,
            managed: DocSet::new(Arc::clone(store), base, DocKind::IsmManaged)?,
            policies: DocSet::new(Arc::clone(store), base, DocKind::IsmPolicies)?,
            // ALLOC: one copy of the node id, once.
            node: node.to_owned(),
            refresh_ms,
            buffer_bytes,
            state: Mutex::new(State::default()),
        })
    }

    /// One pass at `now_ms` with `live` the nodes this node believes are
    /// live (itself included): refresh when due, then evaluate what is due.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the registry, a set or a claim fails.
    pub fn tick(&self, now_ms: i64, live: &[&str]) -> Result<TickReport, MetaError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut report = TickReport::default();
        let moved = state
            .live
            .iter()
            .map(String::as_str)
            .ne(live.iter().copied());
        let due = state
            .refreshed_at
            .is_none_or(|at| now_ms.saturating_sub(at) >= self.refresh_ms);
        if due || moved {
            self.refresh(&mut state, now_ms, live)?;
            report.refreshed = true;
        }
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut stale = false;
        for i in 0..state.owned.len() {
            let Some(owned) = state.owned.get(i) else {
                break;
            };
            let Some(at) = due_of(&owned.record).filter(|at| *at <= now_ms) else {
                continue;
            };
            let index = owned.index.clone();
            let slot = u64::try_from(at).unwrap_or(0);
            let held = state.held.get(&index).copied();
            if held.is_some_and(|(_, last)| slot <= last) {
                continue;
            }
            let claim = self
                .registry
                .claim(TENANT, &index, slot, held.map(|(g, _)| g))?;
            if !claim.won {
                state.held.remove(&index);
                report.lost.push(index);
                stale = true;
                continue;
            }
            state.held.insert(index.clone(), (claim.generation, slot));
            if let Some(next) = self.run(&state.owned[i], slot, now_ms, &mut marker)? {
                state.owned[i].record = next;
            }
            report.evaluated.push(index);
        }
        if stale {
            // Another node moved this index; the next tick re-reads the set.
            state.version = None;
            state.refreshed_at = None;
        }
        Ok(report)
    }

    /// Re-reads the record (another writer may have changed it since the
    /// refresh), evaluates it and writes it back if it changed.
    fn run(
        &self,
        owned: &Owned,
        slot: u64,
        now_ms: i64,
        marker: &mut [u8],
    ) -> Result<Option<Value>, MetaError> {
        let mut doc = vec![0u8; DocKind::IsmManaged.max_doc_bytes()];
        let Some(n) = self.managed.get(&owned.index, marker, &mut doc)? else {
            return Ok(None);
        };
        let Ok(fresh) = serde_json::from_slice::<Value>(doc.get(..n).unwrap_or_default()) else {
            return Ok(None);
        };
        if fresh["enabled"].as_bool() != Some(true)
            || fresh["policy_id"] != owned.record["policy_id"]
            || !fresh["change_policy"].is_null()
        {
            return Ok(None);
        }
        let step = evaluate(&fresh, &owned.policy, now_ms);
        let mut next = step.record;
        let floor = i64::try_from(slot).unwrap_or(i64::MAX).saturating_add(1);
        next["next_eval"] = step.next_eval.map_or(Value::Null, |t| json!(t.max(floor)));
        if next == fresh {
            return Ok(Some(next));
        }
        // ALLOC: the document's text, a few hundred bytes.
        let bytes = next.to_string().into_bytes();
        self.managed
            .put_counted(&owned.index, &bytes, false, marker)?;
        Ok(Some(next))
    }

    fn refresh(&self, state: &mut State, now_ms: i64, live: &[&str]) -> Result<(), MetaError> {
        state.refreshed_at = Some(now_ms);
        let mut registered = false;
        for shard in 0..REGISTRY_SHARDS {
            registered |= self.registry.tenants(shard)?.iter().any(|t| t == TENANT);
        }
        let mut marker = vec![0u8; self.buffer_bytes];
        let version = if registered {
            Some(self.managed.version(&mut marker)?)
        } else {
            None
        };
        let same = state.registered == registered
            && state.version == version
            && state
                .live
                .iter()
                .map(String::as_str)
                .eq(live.iter().copied());
        state.registered = registered;
        // ALLOC: one id a live node, once a refresh.
        state.live = live.iter().map(|n| (*n).to_owned()).collect();
        if same {
            return Ok(());
        }
        state.version = version;
        state.owned.clear();
        if !registered {
            return Ok(());
        }
        let mut doc = vec![0u8; DocKind::IsmManaged.max_doc_bytes()];
        let mut policies: HashMap<String, Value> = HashMap::new();
        for name in self.managed.names(&mut marker)? {
            let key = ShardKey::new(&name, 0);
            if owner(key, live.iter().copied()) != Some(self.node.as_str()) {
                continue;
            }
            let Some(n) = self.managed.get(&name, &mut marker, &mut doc)? else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<Value>(doc.get(..n).unwrap_or_default())
            else {
                continue;
            };
            if record["enabled"].as_bool() != Some(true) {
                continue;
            }
            let id = record["policy_id"].as_str().unwrap_or_default().to_owned();
            if !policies.contains_key(&id) {
                let mut body = vec![0u8; DocKind::IsmPolicies.max_doc_bytes()];
                let stored = self
                    .policies
                    .get(&id, &mut marker, &mut body)?
                    .and_then(|n| serde_json::from_slice::<Value>(body.get(..n)?).ok());
                policies.insert(
                    id.clone(),
                    stored.map_or(Value::Null, |s| s["policy"].clone()),
                );
            }
            let policy = policies.get(&id).cloned().unwrap_or(Value::Null);
            if policy.is_null() {
                continue;
            }
            state.owned.push(Owned {
                index: name,
                record,
                policy,
            });
        }
        Ok(())
    }
}
