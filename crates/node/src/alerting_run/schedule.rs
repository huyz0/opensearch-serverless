//! The loop body of a node's alerting: the monitors this node runs, each run
//! by the slot it falls in.
//!
//! A tick costs nothing between refreshes. A refresh (every `refresh_ms`, or
//! when the live nodes change) reads the 256 registry shards, then, for each
//! registered tenant, its monitor index and the bodies this node does not
//! hold yet. Between refreshes a tick is the clock and the cached bodies: a
//! monitor's slot comes due by arithmetic, and the claim and the search
//! happen then.
//!
//! A monitor is tried by the two live nodes with the highest rendezvous
//! weight for it; the claim register decides which of them runs a slot.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_cluster::placement::{ShardKey, owner};
use oss_format::{MAX_MONITOR_BYTES, REGISTRY_SHARDS};
use oss_meta::{MetaError, Monitors, ScheduleRegistry};
use serde_json::Value;

use crate::ad_route::{Timing, latest_due};
use crate::alerting_route::AlertingService;

/// What a tick did.
#[derive(Debug, Default)]
pub struct TickReport {
    /// Whether the registry and the indexes were read.
    pub refreshed: bool,
    /// The runs this node made, as `tenant/monitor` and the slot.
    pub ran: Vec<(String, u64)>,
    /// The slots another node claimed first, as `tenant/monitor`.
    pub lost: Vec<(String, u64)>,
    /// The runs that failed, as `tenant/monitor` and the reason.
    pub failed: Vec<(String, String)>,
}

struct Owned {
    tenant: String,
    id: String,
    /// The content key of the body held, to see a replacement.
    content: String,
    version: i64,
    period_ms: i64,
    body: Value,
}

#[derive(Default)]
struct State {
    refreshed_at: Option<i64>,
    live: Vec<String>,
    owned: Vec<Owned>,
    /// The generation and slot of this node's last claim of each monitor.
    claims: HashMap<String, (u64, u64)>,
}

/// Decides, each tick, which monitors this node runs.
pub struct Scheduler {
    service: AlertingService,
    registry: ScheduleRegistry,
    store: Arc<dyn BlobStore>,
    base: String,
    node: String,
    refresh_ms: i64,
    timing: Timing,
    state: Mutex<State>,
}

/// A period schedule's length in milliseconds; a cron schedule has none.
fn period_ms(schedule: &str) -> Option<i64> {
    let mut parts = schedule.split(':');
    let (Some("period"), Some(interval), Some(unit)) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let unit = match unit {
        "MINUTES" => 60_000_i64,
        "HOURS" => 3_600_000,
        "DAYS" => 86_400_000,
        _ => return None,
    };
    interval.parse::<i64>().ok()?.checked_mul(unit)
}

impl Scheduler {
    /// A scheduler for node `node` over the monitors under `base`, running
    /// them through `service`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base the registry refuses.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        service: AlertingService,
        node: &str,
        refresh_ms: i64,
        timing: Timing,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            service,
            registry: ScheduleRegistry::new(Arc::clone(store), base)?,
            store: Arc::clone(store),
            // ALLOC: one copy of the base and the node id, once.
            base: base.to_owned(),
            node: node.to_owned(),
            refresh_ms,
            timing,
            state: Mutex::new(State::default()),
        })
    }

    /// One pass at `now_ms` with `live` the nodes this node believes are
    /// live (itself included): refresh when due, then run what is due.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the registry or an index cannot be read.
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
        let State { owned, claims, .. } = &mut *state;
        for m in owned.iter() {
            let Some(slot) = latest_due(now_ms, m.period_ms, self.timing) else {
                continue;
            };
            let name = format!("{}/{}", m.tenant, m.id);
            let hint = claims.get(&name).copied();
            if hint.is_some_and(|(_, last)| slot <= last) {
                continue;
            }
            let claim = match self
                .registry
                .claim(&m.tenant, &m.id, slot, hint.map(|h| h.0))
            {
                Ok(c) => c,
                Err(e) => {
                    report.failed.push((name, e.to_string()));
                    continue;
                }
            };
            claims.insert(name.clone(), (claim.generation, slot));
            if !claim.won {
                report.lost.push((name, slot));
                continue;
            }
            let end = i64::try_from(slot)
                .ok()
                .and_then(|s| s.checked_add(1))
                .and_then(|s| s.checked_mul(m.period_ms))
                .unwrap_or(now_ms);
            match self
                .service
                .run_scheduled(&m.tenant, &m.id, m.version, &m.body, end)
            {
                Ok(()) => report.ran.push((name, slot)),
                Err(e) => report.failed.push((
                    name,
                    e.as_exception()
                        .map_or_else(String::new, |x| x.reason().to_owned()),
                )),
            }
        }
        Ok(report)
    }

    /// The two live nodes that try `tenant/monitor`.
    fn tries(&self, name: &str, live: &[&str]) -> bool {
        let key = ShardKey::new(name, 0);
        let first = owner(key, live.iter().copied());
        if first == Some(self.node.as_str()) {
            return true;
        }
        let rest: Vec<&str> = live.iter().copied().filter(|n| Some(*n) != first).collect();
        owner(key, rest.iter().copied()) == Some(self.node.as_str())
    }

    fn refresh(&self, state: &mut State, now_ms: i64, live: &[&str]) -> Result<(), MetaError> {
        state.refreshed_at = Some(now_ms);
        // ALLOC: one id a live node, once a refresh.
        state.live = live.iter().map(|n| (*n).to_owned()).collect();
        let mut tenants: Vec<String> = Vec::new();
        for shard in 0..REGISTRY_SHARDS {
            tenants.extend(self.registry.tenants(shard)?);
        }
        let mut held: HashMap<(String, String), Owned> = std::mem::take(&mut state.owned)
            .into_iter()
            .map(|o| ((o.tenant.clone(), o.id.clone()), o))
            .collect();
        // ALLOC: one body buffer a refresh, MAX_MONITOR_BYTES.
        let mut buf = vec![0u8; MAX_MONITOR_BYTES];
        let mut owned = Vec::new();
        for tenant in tenants {
            let monitors = Monitors::new(&self.store, &self.base, &tenant)?;
            for (id, entry) in monitors.index()?.iter() {
                let Some(period) = period_ms(&entry.schedule).filter(|_| entry.enabled) else {
                    continue;
                };
                if !self.tries(&format!("{tenant}/{id}"), live) {
                    continue;
                }
                let kept = held
                    .remove(&(tenant.clone(), id.to_owned()))
                    .filter(|o| o.content == entry.content);
                let next = match kept {
                    Some(mut o) => {
                        o.period_ms = period;
                        o
                    }
                    None => {
                        let Some(n) = monitors.body_of(entry, &mut buf)? else {
                            continue;
                        };
                        let Ok(body) =
                            serde_json::from_slice::<Value>(buf.get(..n).unwrap_or_default())
                        else {
                            continue;
                        };
                        Owned {
                            tenant: tenant.clone(),
                            id: id.to_owned(),
                            content: entry.content.clone(),
                            version: entry.version,
                            period_ms: period,
                            body,
                        }
                    }
                };
                owned.push(Owned {
                    version: entry.version,
                    ..next
                });
            }
        }
        state.owned = owned;
        Ok(())
    }
}
