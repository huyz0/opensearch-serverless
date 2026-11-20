//! `GET /_cluster/health[/{index}]` and `GET /_cat/health`.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/ClusterHealthHandler.java`.
//! The judging (what a head says, how a status follows, whether a wait is
//! over) and the refusals' words are `oss_coordinator::cluster::health`'s;
//! this asks a [`HealthSource`], polls while a wait holds and draws the
//! answer. The colours mean what that module says: green is every shard
//! asked about can be served, red is some cannot be, and yellow never occurs.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_cluster::LeaseClock;
use oss_cluster::LivenessOracle;
use oss_cluster::Membership;
use oss_cluster::heads::ShardHeadStore;
use oss_coordinator::cat::health::row;
use oss_coordinator::cat::table::unsupported;
use oss_coordinator::cluster::health::{
    HeadRead, Health, IndexHealth, IndexInput, NodeCondition, Params, ShardState, Status, compute,
    refuse,
};
use oss_meta::DescriptorStore;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router, write_json_str};

use crate::route_util::{flag, refuse as error, send_table};
use crate::time_value::time_millis;

/// How long a health wait lasts when the caller does not say.
const DEFAULT_TIMEOUT_MILLIS: u64 = 30_000;

/// How often a wait reads again.
const POLL: Duration = Duration::from_millis(200);

/// The most bytes a descriptor read holds.
const DESCRIPTOR_BUFFER: usize = 1 << 20;

/// What a health request takes: the waits, the level, the cat table's options
/// and the hints the shell reads and ignores.
const PARAMS: &[&str] = &[
    "wait_for_status",
    "wait_for_nodes",
    "wait_for_active_shards",
    "wait_for_events",
    "wait_for_no_relocating_shards",
    "wait_for_no_initializing_shards",
    "level",
    "timeout",
    "local",
    "master_timeout",
    "cluster_manager_timeout",
    "expand_wildcards",
    "awareness_attribute",
    "ensure_node_weighed_in",
    "ts",
    "time",
    "help",
    "format",
    "v",
    "h",
    "s",
];

/// What a health answer is made from.
pub trait HealthSource: Send + Sync {
    /// The live nodes, or nothing when the control plane cannot be read.
    fn nodes(&self) -> Option<usize>;

    /// What the heads of `name` say.
    fn index(&self, name: &str) -> IndexInput;
}

/// A [`HealthSource`] over the stores: the node count from the membership's
/// last refresh (no request), each index from its descriptor and one head
/// read a shard.
pub struct StoreHealth {
    membership: Arc<Membership>,
    oracle: Arc<LivenessOracle>,
    heads: ShardHeadStore,
    descriptors: DescriptorStore,
    clock: Arc<dyn LeaseClock>,
}

impl StoreHealth {
    /// A source over these stores, telling the time by `clock`.
    pub fn new(
        membership: Arc<Membership>,
        oracle: Arc<LivenessOracle>,
        heads: ShardHeadStore,
        descriptors: DescriptorStore,
        clock: Arc<dyn LeaseClock>,
    ) -> Self {
        Self {
            membership,
            oracle,
            heads,
            descriptors,
            clock,
        }
    }
}

impl HealthSource for StoreHealth {
    fn nodes(&self) -> Option<usize> {
        let now = self.clock.now_millis();
        Some(
            self.membership
                .current()
                .iter()
                .filter(|lease| !lease.is_expired_at(now))
                .count(),
        )
    }

    fn index(&self, name: &str) -> IndexInput {
        // ALLOC: one descriptor buffer an index asked about.
        let mut buf = vec![0u8; DESCRIPTOR_BUFFER];
        let shards = match self.descriptors.get(name, &mut buf) {
            Ok(Some(record)) => record.number_of_shards,
            Ok(None) => return IndexInput::Missing,
            Err(_) => return IndexInput::Unreadable,
        };
        let now = self.clock.now_millis();
        // Whether each owner whose stamp lapsed is alive, asked once an owner:
        // an index whose thousand shards one node holds costs one lease
        // read, not a thousand.
        // ALLOC: one entry an owner of the index.
        let mut alive: HashMap<String, bool> = HashMap::new();
        // ALLOC: one entry a shard of an index asked about.
        let reads = (0..shards)
            .map(|shard| {
                let read = match self.heads.read(name, shard) {
                    Err(_) => HeadRead::Unreadable,
                    Ok(None) => HeadRead::Unowned,
                    Ok(Some(head)) => match head.owner_node_id.as_deref() {
                        None => HeadRead::Unowned,
                        Some(owner) => HeadRead::Owned {
                            held: head.is_held_at(now)
                                || *alive
                                    .entry(format!(
                                        "{owner}/{}",
                                        head.owner_ephemeral_id.as_deref().unwrap_or("")
                                    ))
                                    .or_insert_with(|| {
                                        self.oracle
                                            .is_live(owner, head.owner_ephemeral_id.as_deref())
                                    }),
                        },
                    },
                };
                (shard, read)
            })
            .collect();
        IndexInput::Shards(reads)
    }
}

/// Serves the health routes over a [`HealthSource`].
pub struct HealthService {
    source: Arc<dyn HealthSource>,
    cluster: String,
}

/// What a health request asked for, once read.
struct Ask {
    indices: Vec<String>,
    wait_for_status: Option<String>,
    wait_for_nodes: Option<NodeCondition>,
    per_shard: bool,
}

impl HealthService {
    /// A service over `source`, answering as the cluster named `cluster`.
    pub fn new(source: Arc<dyn HealthSource>, cluster: &str) -> Self {
        Self {
            source,
            // ALLOC: the name, once a node.
            cluster: cluster.to_owned(),
        }
    }

    fn compute(&self, ask: &Ask) -> Health {
        // ALLOC: one entry an index asked about.
        let inputs: Vec<(String, IndexInput)> = ask
            .indices
            .iter()
            .map(|name| (name.clone(), self.source.index(name)))
            .collect();
        compute(self.source.nodes().ok_or(()), &inputs, ask.per_shard)
    }

    /// Reads until the wait holds or `timeout` passes.
    fn await_health(&self, ask: &Ask, timeout: Duration) -> Health {
        let deadline = Instant::now().checked_add(timeout);
        let mut health = self.compute(ask);
        loop {
            if health.satisfied(ask.wait_for_status.as_deref(), ask.wait_for_nodes.as_ref()) {
                return health;
            }
            let left = deadline.map_or(Duration::MAX, |d| {
                d.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return health;
            }
            std::thread::sleep(POLL.min(left));
            health = self.compute(ask);
        }
    }

    fn ask(req: &Request<'_>) -> Result<(Ask, Duration, Option<String>), RestError> {
        let cat = req.raw_path().starts_with("/_cat");
        let status = req.param("wait_for_status");
        let level = req.param("level");
        let columns = req.param("h");
        let sort = req.param("s");
        let events = req.param("wait_for_events");
        let active = req.param("wait_for_active_shards");
        let cat_unsupported = unsupported(columns.as_deref(), sort.as_deref());
        let params = Params {
            cat,
            cat_unsupported,
            wait_for_status: status.as_deref(),
            wait_for_active_shards: active.as_deref(),
            wait_for_events: events.as_deref(),
            level: level.as_deref().unwrap_or("cluster"),
        };
        if let Some(reason) = refuse(&params) {
            return Err(error(501, "unsupported_health_parameter", reason));
        }
        let wait_for_nodes = match req.param("wait_for_nodes") {
            None => None,
            Some(text) => Some(NodeCondition::parse(&text).ok_or_else(|| {
                // ALLOC: error path; one message.
                error(
                    400,
                    "invalid_parameter",
                    format!(
                        "wait_for_nodes must be a number, or one of >=N, >N, <=N, <N, ge(N), gt(N), \
                         le(N), lt(N); got [{text}]"
                    ),
                )
            })?),
        };
        // ALLOC: one entry a name in the request, bounded by its length.
        let indices: Vec<String> = req
            .path_param("index")
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if indices.is_empty() && status.is_some() {
            return Err(error(
                400,
                "unscoped_wait_for_status",
                "wait_for_status needs an index: GET /_cluster/health examines no shard, so there is no \
                 shard status to wait for; ask /_cluster/health/{index}, or wait_for_nodes for the fleet",
            ));
        }
        let timeout = match req.param("timeout") {
            Some(text) => time_millis("timeout", &text)?,
            None => DEFAULT_TIMEOUT_MILLIS,
        };
        let ask = Ask {
            indices,
            wait_for_status: status.map(|s| s.into_owned()),
            wait_for_nodes,
            per_shard: level.as_deref() == Some("shards"),
        };
        let format = req.param("format").map(|f| f.into_owned());
        Ok((ask, Duration::from_millis(timeout), format))
    }

    /// `GET /_cluster/health[/{index}]` and `GET /_cat/health`.
    pub fn handle(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        cat: bool,
    ) -> Result<(), RestError> {
        let (ask, timeout, format) = Self::ask(req)?;
        let verbose = flag(req, "v", false);
        let health = self.await_health(&ask, timeout);
        if let Some(missing) = &health.missing {
            // ALLOC: error path; the name.
            return Err(error(
                404,
                "index_not_found",
                format!("no such index: {missing}"),
            ));
        }
        if cat {
            let table = row(&self.cluster, &health);
            return send_table(resp, &table.answer(format.as_deref(), verbose, None));
        }
        let satisfied =
            health.satisfied(ask.wait_for_status.as_deref(), ask.wait_for_nodes.as_ref());
        let status = if satisfied { 200 } else { 408 };
        // ALLOC: the body, bounded by the indices and shards asked about.
        let mut out = Vec::new();
        self.render(&mut out, &health, &ask, satisfied)?;
        resp.send(status, JSON, &out)
    }

    fn render(
        &self,
        out: &mut Vec<u8>,
        health: &Health,
        ask: &Ask,
        satisfied: bool,
    ) -> Result<(), RestError> {
        out.write_all(b"{\"cluster_name\":")?;
        write_json_str(&mut *out, &self.cluster)?;
        write!(out, ",\"status\":\"{}\"", health.status.as_str())?;
        let scoped = !health.indices.is_empty();
        if !scoped {
            out.write_all(b",\"status_scope\":\"control_plane\"")?;
        }
        write!(
            out,
            ",\"timed_out\":{},\"number_of_nodes\":{n},\"discovered_cluster_manager\":false,\"number_of_data_nodes\":{n}",
            !satisfied,
            n = health.nodes
        )?;
        if scoped {
            let total = health
                .active_shards
                .saturating_add(health.unservable_shards);
            let percent = if total == 0 {
                100.0
            } else {
                100.0 * health.active_shards as f64 / total as f64
            };
            write!(
                out,
                ",\"complete\":true,\"active_primary_shards\":{a},\"active_shards\":{a},\"relocating_shards\":0,\
                 \"initializing_shards\":0,\"unassigned_shards\":{u},\"delayed_unassigned_shards\":0,\
                 \"dormant_shards\":{d},\"active_shards_percent_as_number\":{percent:?},\
                 \"active_shards_percent\":\"{percent:.1}%\"",
                a = health.active_shards,
                u = health.unservable_shards,
                d = health.dormant_shards,
            )?;
        } else {
            out.write_all(
                b",\"complete\":false,\"shard_counts\":\"not reported without an index; ask /_cluster/health/{index}\"",
            )?;
        }
        out.write_all(
            b",\"number_of_pending_tasks\":0,\"number_of_in_flight_fetch\":0,\"task_max_waiting_in_queue_millis\":0,\
              \"replication\":\"object-store\",\"yellow_reachable\":false",
        )?;
        if scoped {
            out.write_all(b",\"indices\":{")?;
            for (i, (name, each)) in health.indices.iter().enumerate() {
                if i > 0 {
                    out.write_all(b",")?;
                }
                write_json_str(&mut *out, name)?;
                out.write_all(b":")?;
                index(out, each, ask.per_shard)?;
            }
            out.write_all(b"}")?;
        }
        out.write_all(b"}")?;
        Ok(())
    }
}

fn index(out: &mut Vec<u8>, each: &IndexHealth, per_shard: bool) -> Result<(), RestError> {
    write!(
        out,
        "{{\"status\":\"{}\",\"number_of_shards\":{},\"number_of_replicas\":0,\"active_primary_shards\":{a},\
         \"active_shards\":{a},\"relocating_shards\":0,\"initializing_shards\":0,\"unassigned_shards\":{},\
         \"dormant_shards\":{}",
        each.status.as_str(),
        each.counts.shards,
        each.counts.unservable,
        each.counts.dormant,
        a = each.counts.active,
    )?;
    if per_shard {
        out.write_all(b",\"shards\":{")?;
        for (i, (shard, state)) in each.per_shard.iter().enumerate() {
            if i > 0 {
                out.write_all(b",")?;
            }
            let (colour, word) = match state {
                ShardState::Owned => (Status::Green, "owned"),
                ShardState::Dormant => (Status::Green, "dormant"),
                ShardState::Unservable => (Status::Red, "unservable"),
            };
            write!(
                out,
                "\"{shard}\":{{\"status\":\"{}\",\"state\":\"{word}\",\"number_of_replicas\":0}}",
                colour.as_str()
            )?;
        }
        out.write_all(b"}")?;
    }
    out.write_all(b"}")?;
    Ok(())
}

/// The routes of cluster health and the cat health table.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<HealthService>) -> Result<Router, RouteError> {
    let table: [(&'static str, bool); 3] = [
        ("/_cluster/health", false),
        ("/_cluster/health/{index}", false),
        ("/_cat/health", true),
    ];
    let mut router = router;
    for (path, cat) in table {
        let service = Arc::clone(&service);
        router = router.route(
            Method::Get,
            path,
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp, cat),
        )?;
    }
    Ok(router)
}
