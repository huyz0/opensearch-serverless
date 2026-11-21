//! What the writer role tells the tail overlay as shards are written,
//! published and let go.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use oss_format::WalRecord;

use super::WriterRole;

/// Takes the events the tail overlay keeps and pushes records for. Called
/// outside any shard lock except `acked`, which runs after the log has the
/// records.
pub trait TailSink: Send + Sync {
    /// `records` of the shard were acknowledged.
    fn acked(&self, index: &str, uuid: &str, shard: u32, records: &[WalRecord<'static>]);
    /// A commit holding every operation through `checkpoint` was published.
    fn published(&self, uuid: &str, shard: u32, checkpoint: i64);
    /// The node no longer writes the shard.
    fn released(&self, uuid: &str, shard: u32);
    /// Waits, bounded, until the nodes that search the shard hold every
    /// record acknowledged for it, and with `grants` until the grants this
    /// node cannot reach have run out. True when the shard's writes are
    /// visible to those searches, so a refresh need not publish a commit;
    /// false when they are not (the default: nothing is waited for).
    fn await_visible(&self, _uuid: &str, _shard: u32, _grants: bool) -> bool {
        false
    }
}

/// How far a refreshing write is made visible before it is answered (the
/// shell's `RefreshVisibility`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RefreshVisibility {
    /// The writer's own refresh and a published commit: no wait.
    #[default]
    Owner,
    /// Wait for the search nodes the shard is pushed to.
    Serving,
    /// As `Serving`, and every search node asks the writer for a grant
    /// before it answers.
    Everywhere,
}

impl RefreshVisibility {
    /// The mode named `word`: `owner`, `serving` or `everywhere`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "owner" => Some(Self::Owner),
            "serving" => Some(Self::Serving),
            "everywhere" => Some(Self::Everywhere),
            _ => None,
        }
    }

    /// Whether a refresh waits for search nodes at all.
    pub fn waits(self) -> bool {
        self != Self::Owner
    }

    /// Whether search nodes answer only under a grant from the writer.
    pub fn grants(self) -> bool {
        self == Self::Everywhere
    }

    fn code(self) -> u8 {
        match self {
            Self::Owner => 0,
            Self::Serving => 1,
            Self::Everywhere => 2,
        }
    }

    pub(super) fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Serving,
            2 => Self::Everywhere,
            _ => Self::Owner,
        }
    }

    pub(super) fn to_code(self) -> u8 {
        self.code()
    }
}

impl WriterRole {
    /// Sends what is acknowledged, published and released to `sink`. False,
    /// and nothing changed, when a sink is set already.
    pub fn set_tail_sink(&self, sink: Arc<dyn TailSink>) -> bool {
        self.tail.set(sink).is_ok()
    }

    /// How far a refresh is made visible before it returns. `owner`, the
    /// default, publishes a commit; the others wait for the search nodes
    /// instead, and publish only when the wait does not reach them.
    pub fn set_refresh_visibility(&self, visibility: RefreshVisibility) {
        self.visibility
            .store(visibility.to_code(), Ordering::Relaxed);
    }

    /// The mode a refresh is in.
    pub fn refresh_visibility(&self) -> RefreshVisibility {
        RefreshVisibility::from_code(self.visibility.load(Ordering::Relaxed))
    }
}
