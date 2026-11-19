//! Judging cluster health from what the shard heads say.
//!
//! Nothing here relocates or initialises, and there are no replicas, so
//! yellow is never reported: a shard is `owned` by a node holding its lease,
//! `dormant` (nobody writes it now; the next write activates it, which is not
//! a fault) or `unservable` (its head could not be read), and only the last
//! makes an index red.

/// A cluster's or index's status: yellow cannot occur without replicas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing is wrong.
    #[default]
    Green,
    /// Something could not be read.
    Red,
}

impl Status {
    /// The word the status is reported as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
        }
    }
}

/// What a shard head said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadRead {
    /// The head could not be read.
    Unreadable,
    /// No head, or a head with no owner.
    Unowned,
    /// A head with an owner, and whether the owner still holds it.
    Owned {
        /// Whether the owner's lease still holds the shard.
        held: bool,
    },
}

/// What the heads of one index said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexInput {
    /// The index's descriptor could not be read.
    Unreadable,
    /// There is no such index.
    Missing,
    /// Each live shard and what its head said.
    Shards(Vec<(u32, HeadRead)>),
}

/// How a shard is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardState {
    /// A node holds its lease.
    Owned,
    /// Nobody writes it now; the next write activates it.
    Dormant,
    /// Its head could not be read.
    Unservable,
}

/// An index's shard counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Live shards.
    pub shards: usize,
    /// Owned and dormant shards.
    pub active: usize,
    /// Dormant shards.
    pub dormant: usize,
    /// Shards whose head could not be read.
    pub unservable: usize,
}

/// One index's health.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexHealth {
    /// The index's status.
    pub status: Status,
    /// Its shard counts.
    pub counts: Counts,
    /// Each shard's state, when asked for.
    pub per_shard: Vec<(u32, ShardState)>,
}

/// The cluster's health.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// The cluster's status.
    pub status: Status,
    /// Live nodes.
    pub nodes: usize,
    /// Owned and dormant shards of the indices asked about.
    pub active_shards: usize,
    /// Dormant shards.
    pub dormant_shards: usize,
    /// Shards whose head could not be read.
    pub unservable_shards: usize,
    /// The first index asked about that does not exist.
    pub missing: Option<String>,
    /// Each index asked about, in the order asked.
    pub indices: Vec<(String, IndexHealth)>,
}

/// A `wait_for_nodes` condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeCondition {
    /// One of `=`, `>=`, `>`, `<=`, `<`.
    pub operator: &'static str,
    /// The count compared against.
    pub count: i32,
}

impl NodeCondition {
    /// Reads `N`, `>=N`, `>N`, `<=N`, `<N` or `ge(N)`, `gt(N)`, `le(N)`,
    /// `lt(N)`; nothing for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let (operator, number) = if let Some(rest) = t.strip_prefix(">=") {
            (">=", rest)
        } else if let Some(rest) = t.strip_prefix("<=") {
            ("<=", rest)
        } else if let Some(rest) = t.strip_prefix('>') {
            (">", rest)
        } else if let Some(rest) = t.strip_prefix('<') {
            ("<", rest)
        } else if let Some(rest) = function_form(t, "ge") {
            (">=", rest)
        } else if let Some(rest) = function_form(t, "gt") {
            (">", rest)
        } else if let Some(rest) = function_form(t, "le") {
            ("<=", rest)
        } else if let Some(rest) = function_form(t, "lt") {
            ("<", rest)
        } else {
            ("=", t)
        };
        let count = number.trim().parse().ok()?;
        Some(Self { operator, count })
    }

    /// Whether `nodes` satisfies the condition.
    #[must_use]
    pub fn satisfied_by(&self, nodes: usize) -> bool {
        let nodes = i64::try_from(nodes).unwrap_or(i64::MAX);
        let count = i64::from(self.count);
        match self.operator {
            ">=" => nodes >= count,
            ">" => nodes > count,
            "<=" => nodes <= count,
            "<" => nodes < count,
            _ => nodes == count,
        }
    }
}

fn function_form<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// A refusal's text.
fn message(text: &str) -> Option<String> {
    // ALLOC: one message.
    Some(text.to_owned())
}

/// The parameters a health request carries that can be refused.
#[derive(Clone, Copy, Debug)]
pub struct Params<'a> {
    /// Whether the request is `_cat/health`.
    pub cat: bool,
    /// The cat table's own refusal, if it has one.
    pub cat_unsupported: Option<&'a str>,
    /// `wait_for_status`.
    pub wait_for_status: Option<&'a str>,
    /// `wait_for_active_shards`.
    pub wait_for_active_shards: Option<&'a str>,
    /// `wait_for_events`.
    pub wait_for_events: Option<&'a str>,
    /// `level`.
    pub level: &'a str,
}

/// Why a health request is refused, in the shell's words; nothing for an
/// accepted one.
#[must_use]
pub fn refuse(params: &Params<'_>) -> Option<String> {
    if params.cat
        && let Some(unsupported) = params.cat_unsupported
    {
        return message(unsupported);
    }
    if params
        .wait_for_status
        .is_some_and(|s| s != "green" && s != "yellow")
    {
        return message(
            "wait_for_status must be 'green' or 'yellow'; 'red' is not a state to wait for, and \
             yellow is never returned here because it means 'replicas are missing' and there are \
             no replicas -- a caller waiting for yellow is satisfied by green",
        );
    }
    if params.wait_for_active_shards.is_some() {
        return message(
            "wait_for_active_shards counts shards an allocator has placed, and there is no \
             allocator here: a shard is activated by the write that needs it, not by a placement \
             decision. Wait for the write instead, or use wait_for_status",
        );
    }
    if params.wait_for_events.is_some() {
        return message(
            "wait_for_events waits on a cluster manager's pending-task queue; there is no cluster \
             manager and no queue, so there is no event to wait for",
        );
    }
    if !matches!(params.level, "cluster" | "indices" | "shards") {
        return message("level must be 'cluster', 'indices' or 'shards'");
    }
    None
}

/// The health from the live node count (`Err` when it could not be read) and
/// what each index asked about said; `per_shard` keeps each shard's state.
#[must_use]
pub fn compute(
    nodes: Result<usize, ()>,
    inputs: &[(String, IndexInput)],
    per_shard: bool,
) -> Health {
    let mut health = Health::default();
    match nodes {
        Ok(n) => health.nodes = n,
        // The control plane could not be read: that is the red that matters,
        // reported rather than thrown.
        Err(()) => {
            health.status = Status::Red;
            return health;
        }
    }
    for (name, input) in inputs {
        let mut each = IndexHealth::default();
        match input {
            IndexInput::Missing => {
                // ALLOC: one index name.
                health.missing = Some(name.clone());
                return health;
            }
            IndexInput::Unreadable => each.status = Status::Red,
            IndexInput::Shards(reads) => {
                each.counts.shards = reads.len();
                for (shard, read) in reads {
                    let state = match read {
                        HeadRead::Unreadable => {
                            each.counts.unservable = each.counts.unservable.saturating_add(1);
                            each.status = Status::Red;
                            ShardState::Unservable
                        }
                        HeadRead::Owned { held: true } => {
                            each.counts.active = each.counts.active.saturating_add(1);
                            ShardState::Owned
                        }
                        // Dormant, not unassigned: nobody writes it now.
                        HeadRead::Unowned | HeadRead::Owned { held: false } => {
                            each.counts.active = each.counts.active.saturating_add(1);
                            each.counts.dormant = each.counts.dormant.saturating_add(1);
                            ShardState::Dormant
                        }
                    };
                    if per_shard {
                        // ALLOC: one entry for each shard of an index asked about.
                        each.per_shard.push((*shard, state));
                    }
                }
            }
        }
        health.active_shards = health.active_shards.saturating_add(each.counts.active);
        health.dormant_shards = health.dormant_shards.saturating_add(each.counts.dormant);
        health.unservable_shards = health
            .unservable_shards
            .saturating_add(each.counts.unservable);
        if each.status == Status::Red {
            health.status = Status::Red;
        }
        // ALLOC: one entry for each index asked about.
        health.indices.push((name.clone(), each));
    }
    health
}

impl Health {
    /// Whether a wait is over: always for a missing index; otherwise when the
    /// node condition holds and, for a wait on status, the status is green
    /// (green satisfies a wait for yellow too, as yellow is never reached).
    #[must_use]
    pub fn satisfied(
        &self,
        wait_for_status: Option<&str>,
        wait_for_nodes: Option<&NodeCondition>,
    ) -> bool {
        if self.missing.is_some() {
            return true;
        }
        if wait_for_nodes.is_some_and(|c| !c.satisfied_by(self.nodes)) {
            return false;
        }
        wait_for_status.is_none() || self.status == Status::Green
    }
}
