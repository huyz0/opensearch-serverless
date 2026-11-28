//! What a plugin may ask the host for, and how much per call
//! ([ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md),
//! section 4).

use crate::catalogue::Tier;

/// The most of any one budget item a call may declare.
pub const MAX_BUDGET: u64 = 64;
/// The longest host name an egress capability may name.
const MAX_HOST_BYTES: usize = 253;

/// One capability a manifest asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// Read the plugin's tenant-scoped state.
    StateRead,
    /// Write the plugin's tenant-scoped state.
    StateWrite,
    /// Run a search as the calling tenant.
    Search,
    /// Start a search the host runs on a thread of its own and read its
    /// result later ([ADR 0084](../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md)).
    Job,
    /// Make HTTP calls to this one host.
    Egress(String),
    /// Be told the caller's user name and roles.
    Identity,
    /// Write rate-limited log lines.
    Log,
    /// Add plugin-labelled counters.
    Metrics,
}

impl Capability {
    /// The capability a manifest string stands for, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "state.read" => Some(Self::StateRead),
            "state.write" => Some(Self::StateWrite),
            "search" => Some(Self::Search),
            "job" => Some(Self::Job),
            "identity" => Some(Self::Identity),
            "log" => Some(Self::Log),
            "metrics" => Some(Self::Metrics),
            other => {
                let host = other.strip_prefix("egress:")?;
                // ALLOC: one manifest, a host name of at most 253 bytes.
                plain_host(host).then(|| Self::Egress(host.to_owned()))
            }
        }
    }

    /// Whether a plugin on `tier` may hold it. An out-of-process plugin gets
    /// no host capability: it is told who is calling and nothing else. `job`
    /// is for a native plugin only: the host's thread is not offered to a
    /// sandbox.
    pub fn allowed_on(&self, tier: Tier) -> bool {
        match self {
            Self::Job => tier == Tier::Native,
            _ => tier != Tier::Grpc || matches!(self, Self::Identity),
        }
    }
}

/// A host name without wildcards, ports or paths.
fn plain_host(host: &str) -> bool {
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
        && edge(host.chars().next())
        && edge(host.chars().last())
        && !host.contains("..")
}

/// The most of each costly thing one call may do; every item defaults to zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Budget {
    /// State operations (a get, a put or a delete each count one).
    pub state_ops: u32,
    /// Searches.
    pub searches: u32,
    /// Outbound HTTP calls.
    pub egress: u32,
}
